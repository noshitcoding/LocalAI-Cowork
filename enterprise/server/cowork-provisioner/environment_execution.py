"""Exactly one original process dispatch; incomplete command ACKs stay held."""
import base64,json,re
from pathlib import Path
import backend_profile,environment_lifecycle as lifecycle,executor_host

def authorize(manager,infra,job):
    result=manager.call('/environment-execution/'+job['id']+'/authorize',{'executor_host_binding':executor_host.manager_binding(infra.config),'lease_token':job['lease_token'],'plan_sha256':job['plan_sha256'],'result':{}})
    lifecycle.require(result['id']==job['id'] and result['plan_sha256']==job['plan_sha256'] and result['authorized'] is True,'current original environment task authority changed')

def actual(infra,plan):
    runtime=plan['environment']['runtime'];backend_profile.validate(infra,plan,physical=True)
    if runtime['backend']=='open_shell':
        import openshell_environment as openshell
        c=openshell.selected(infra,plan);value=openshell.get(infra,c,runtime);openshell.original_resources(infra,runtime)
        lifecycle.require(value['status']['mainProcessInstanceId']==runtime['runtime_generation'],'original OpenShell task generation changed');lifecycle.guest(infra,plan,'inspect',runtime['storage']);return runtime
    if runtime['backend']=='docker':
        value=lifecycle.original_docker(infra,plan,runtime)
        lifecycle.require(value['State']['Running'] and value['State']['Pid']>0 and value['State']['StartedAt']==runtime['started_at'],'original environment task runtime restarted')
        lifecycle.guest(infra,plan,'inspect',runtime['storage'])
    elif runtime['backend']=='kubernetes':
        value=lifecycle.original_kube(infra,plan,runtime);status=value['status']['containerStatuses'][0]
        lifecycle.require(value['status']['phase']=='Running' and status['ready'] and status['restartCount']==0 and status['containerID']==runtime['container_runtime_id'] and status['state']['running']['startedAt']==runtime['started_at'],'original environment task pod restarted')
        node=lifecycle.object(infra,plan,'node',runtime['node_name']);lifecycle.require(node['metadata']['uid']==runtime['node_uid'] and value['spec']['nodeName']==runtime['node_name'],'original task node UID changed')
        claim=lifecycle.object(infra,plan,'pvc',runtime['storage']['volume_name']);lifecycle.require(claim['metadata']['uid']==runtime['storage']['storage_uid'],'original task workspace UID changed')
    else:raise RuntimeError('environment task backend adapter unavailable')
    return runtime

def execute(manager,infra,job):
    plan=job['plan'];lifecycle.require(plan['format']==1 and lifecycle.sha(plan)==job['plan_sha256'] and plan['id']==job['id'],'original environment task plan differs')
    if plan.get('environment_controller') and plan['environment_controller']['backend']!='open_shell':
        import external_vm
        infra=external_vm.Infrastructure(infra,plan)
    # Reuse the exact qualified template and limits, rather than inventing a
    # fresh probe plan or model task. The task request is frozen separately.
    request=plan['request'];environment=plan['environment']
    template=plan['template']
    execution={**plan,'action':'task','template':template}
    path=infra.directory(plan['instance_id'])/'environments'/('task-'+lifecycle.identifier(job['id'])+'.json')
    if path.exists():
        marker=lifecycle.read(path);lifecycle.require(marker['plan_sha256']==job['plan_sha256'] and marker['lease_token']==job['lease_token'],'original environment task journal changed')
        if marker.get('result') is not None:return marker['result']
        raise RuntimeError('original process output unproven; command replay forbidden')
    authorize(manager,infra,job);runtime=actual(infra,execution);task=request['task_input'];stdin=base64.b64decode(task.get('stdin_base64',''),validate=True)
    lifecycle.require(task['operation'] in ('process','file_read','file_write'),'actual bounded runtime operation required')
    marker={'format':1,'plan_sha256':job['plan_sha256'],'lease_token':job['lease_token'],'runtime':runtime,'stage':'exec_dispatched'};lifecycle.save(infra,path,marker)
    authorize(manager,infra,job)
    if task['operation'] in ('file_read','file_write'):
        lifecycle.require(request['capabilities']==['file.read' if task['operation']=='file_read' else 'file.write'],'original file capability changed')
        source=Path(__file__).with_name('environment_file_operation.py').read_text(encoding='utf-8')
        original={**task,'request_id':request['request_id']}
        # The workspace is untrusted: isolate Python from cwd/PYTHONPATH/user
        # site imports so a json.py or fcntl.py cannot forge an IO receipt.
        out=dispatch(infra,execution,runtime,['/usr/bin/python3','-I','-S','-c',source],lifecycle.canonical(original),60)
        file_result=json.loads(out);lifecycle.require(file_result['operation']==task['operation'] and file_result['relative_path']==task['relative_path'] and file_result['transfer_request_id']==task['transfer_request_id'] and file_result['transfer_grant_id']==task['transfer_grant_id'] and file_result['expected_transfer_revision']==task['expected_transfer_revision'],'actual original file completion differs')
        lifecycle.require(actual(infra,execution)==runtime,'original runtime changed during file operation')
        result={'format':1,'job_id':job['id'],'plan_sha256':job['plan_sha256'],'runtime':runtime,'original_runtime_verified':True,'delivery_complete':True,'exit_code':0,'stdout_base64':'','stderr_base64':'','file_result':file_result}
        marker.update(stage='completed',result=result);lifecycle.save(infra,path,marker);return result
    lifecycle.require(request['capabilities']==['process.execute'] and len(stdin)<=262144 and type(task['timeout_seconds']) is int and 1<=task['timeout_seconds']<=300,'actual bounded process runtime required')
    # The sandbox owns these temporary files, under its fixed memory tmpfs.
    # argv is passed as positional arguments, without interpolation or parsing.
    script='set -eu; umask 077; d=/tmp/cowork-task-'+lifecycle.identifier(job['id'])+'; mkdir -- "$d"; printf %s '+job['plan_sha256']+' > "$d/plan"; cat > "$d/in"; cd /workspace; ulimit -f 256; set +e; timeout -k 5 "$1" "${@:2}" < "$d/in" > "$d/out" 2> "$d/err"; code=$?; set -e; test "$(wc -c < "$d/out")" -le 262144; test "$(wc -c < "$d/err")" -le 262144; printf "%s\\n" "$code" > "$d/code.pending"; mv "$d/code.pending" "$d/code"; printf "%s\\n" "$code"; base64 < "$d/out" | tr -d "\\n"; printf "\\n"; base64 < "$d/err" | tr -d "\\n"; printf "\\n"'
    # POSIX sh has no ${@:2}; shift preserves the original argument vector.
    script=script.replace('timeout -k 5 "$1" "${@:2}"','seconds=$1; shift; timeout -k 5 "$seconds" "$@"')
    command=['/bin/sh','-c',script,'cowork-environment-task',str(task['timeout_seconds']),*task['argv']]
    out=dispatch(infra,execution,runtime,command,stdin,task['timeout_seconds']+30)
    lines=out.decode('ascii').splitlines();lifecycle.require(len(lines)==3 and re.fullmatch('[0-9]{1,3}',lines[0]) and 0<=int(lines[0])<=255,'actual original process completion framing invalid')
    lifecycle.require(all(len(base64.b64decode(v,validate=True))<=262144 for v in lines[1:]),'actual process output exceeded bound')
    lifecycle.require(actual(infra,execution)==runtime and environment['runtime']==runtime,'original runtime changed during process operation')
    result={'format':1,'job_id':job['id'],'plan_sha256':job['plan_sha256'],'runtime':runtime,'original_runtime_verified':True,'delivery_complete':True,'exit_code':int(lines[0]),'stdout_base64':lines[1],'stderr_base64':lines[2]}
    marker.update(stage='completed',result=result);lifecycle.save(infra,path,marker);return result

def dispatch(infra,plan,runtime,command,data,timeout):
    if runtime['backend']=='open_shell':
        import openshell_environment
        return openshell_environment.exec(infra,plan,runtime,command,data,timeout)
    if runtime['backend']=='docker':return lifecycle.docker(infra,'exec','-i',runtime['container_id'],*command,data=data,timeout=timeout)
    return infra.run(['kubectl','--namespace',plan['backend_profile']['specification']['namespace'],'--request-timeout='+str(timeout-5)+'s','exec','-i',runtime['pod_name'],'--container=environment','--',*command],data=data,timeout=timeout)

def recover(infra,job,path):
    marker=lifecycle.read(path);plan=job['plan'];lifecycle.require(marker['plan_sha256']==job['plan_sha256'] and marker['lease_token']==job['lease_token'] and marker['runtime']==plan['environment']['runtime'],'original process journal binding changed')
    if plan.get('environment_controller') and plan['environment_controller']['backend']!='open_shell':
        import external_vm
        infra=external_vm.Infrastructure(infra,plan)
    if marker.get('result') is not None:return marker['result']
    # The process owns its sandbox files and may forge any wrapper marker.
    # Only the operator journal persisted after a real terminal RPC/CLI ACK can
    # settle an original dispatch. Unknown command ACKs remain held forever
    # until an independent physical stop/reconciliation is provided.
    raise RuntimeError('original terminal command acknowledgement missing; replay and sandbox-file adoption forbidden')

def process(manager,infra):
    if lifecycle.settings(infra.config) is None:return
    binding={'executor_host_binding':executor_host.manager_binding(infra.config)}
    for job in manager.call('/environment-execution/pending',binding):
        path=infra.directory(job['instance_id'])/'environments'/('task-'+lifecycle.identifier(job['id'])+'.json')
        if path.exists():
            try:result=recover(infra,job,path)
            except Exception:continue
            manager.call('/environment-execution/'+job['id']+'/complete',{**binding,'lease_token':job['lease_token'],'plan_sha256':job['plan_sha256'],'result':result})
    job=manager.call('/environment-execution/claim',binding)
    if job:
        try:result=execute(manager,infra,job)
        except Exception:
            manager.call('/environment-execution/'+job['id']+'/hold',{**binding,'lease_token':job['lease_token'],'plan_sha256':job['plan_sha256'],'result':{}});return
        manager.call('/environment-execution/'+job['id']+'/complete',{**binding,'lease_token':job['lease_token'],'plan_sha256':job['plan_sha256'],'result':result})
