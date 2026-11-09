"""Qualified digest images and actual isolated persistent environments.

One original manager job owns every mutation. Lost create acknowledgements are
held; retries only deliver a durable original completion or stop an acknowledged
original runtime. No name/label based adoption and no create/exec replay.
"""
import hashlib,json,os,re,shlex,time,uuid
from pathlib import Path
import backend_profile,executor_host,execution_generation

FINALIZER='open-cowork.io/environment-physical-stop'
def require(ok,message):
    if not ok:raise RuntimeError(message)
def canonical(value):return json.dumps(value,sort_keys=True,ensure_ascii=False,separators=(',',':')).encode()
def sha(value):return hashlib.sha256(canonical(value)).hexdigest()
def identifier(value):require(str(uuid.UUID(value))==value,'noncanonical environment identity');return uuid.UUID(value).hex
def read(path):
    require(path.is_file() and not path.is_symlink() and path.stat().st_size<=2097152,'original environment journal missing')
    return json.loads(path.read_bytes())
def settings(config):
    value=config.get('environment_lifecycle')
    if value is None:return None
    require(value['version']==1 and set(value)<= {'version','guest_storage_helper','guest_storage_helper_sha256'},'invalid environment lifecycle configuration')
    require(backend_profile.settings(config) is not None,'environment original physical backend profile required')
    if config['backend']=='compose':
        require(re.fullmatch(r'/[a-zA-Z0-9_./-]+',value['guest_storage_helper']) and '..' not in Path(value['guest_storage_helper']).parts
                and re.fullmatch('[a-f0-9]{64}',value['guest_storage_helper_sha256']),'source-pinned original guest helper required')
    return value
def operation(job):
    plan=job['plan'];require(plan['format']==1 and sha(plan)==job['plan_sha256'] and plan['id']==job['id'],'manager original environment plan differs')
    return plan
def scope(infra,job):
    plan=operation(job);backend_profile.validate(infra,plan,physical=True)
    directory=infra.directory(plan['instance_id'])/'environments';directory.mkdir(mode=0o700,exist_ok=True)
    require(not directory.is_symlink() and directory.resolve().is_relative_to(infra.root),'unsafe environment journal scope')
    original=directory/(identifier(job['id'])+'.json')
    return plan,directory,original
def save(infra,path,value):infra.private_write(path,canonical(value))
def authorize(infra,plan):
    guard=getattr(infra,'_environment_authorizer',None)
    require(guard is not None,'current original manager environment authorization required');guard(plan)
def spec(plan):
    if plan['action']=='qualify':return {'limits':{'cpu_millis':500 if plan['template']['backend']=='open_shell' else 250,'memory_mib':512 if plan['template']['backend']=='open_shell' else 128,'storage_gib':1},'persistent':False,'shared_workspace':False}
    return plan['environment']['spec']
def environment_id(plan):return plan['environment']['id'] if plan['environment'] else plan['id']
def labels(plan):return {'cowork.managed':'environment-lifecycle','cowork.instance':plan['instance_id'],
    'cowork.organisation':plan['organisation_id'],'cowork.environment':environment_id(plan),'cowork.environment-create':plan['id']}
def limits(plan):
    value=spec(plan)['limits'];require(100<=value['cpu_millis']<=1024000 and 128<=value['memory_mib']<=4194304 and 1<=value['storage_gib']<=1048576,'invalid bounded environment limits');return value
def guest(infra,plan,action,storage=None,container_id=None):
    if action in ('ensure','bind','release'):authorize(infra,plan)
    if plan.get('environment_controller') and plan['environment_controller']['backend']!='open_shell':
        import external_vm
        return external_vm.storage(infra,plan,action,storage,container_id)
    selected=settings(infra.config);profile=backend_profile.settings(infra.config);argv=profile['probe_command']
    require('--config' in argv,'original VM producer configuration missing')
    config=read(Path(argv[argv.index('--config')+1]));root=Path(config['vm_root'])
    metadata=read(root/'vm.json');physical=plan['backend_profile']['specification']
    require(metadata['id']==physical['vm_id'] and metadata['organisation_id']==plan['organisation_id'],'original VM guest assignment changed')
    target=selected['guest_storage_helper'];known=root/'known_hosts'
    require(known.is_file() and not known.is_symlink(),'pinned guest SSH identity missing')
    ssh=['ssh','-i',str(root/'ssh-key'),'-p',str(metadata['ssh_port']),'-o','BatchMode=yes','-o','IdentitiesOnly=yes','-o','StrictHostKeyChecking=yes','-o','UserKnownHostsFile='+str(known),'-o','ConnectTimeout=5','cowork@127.0.0.1']
    actual=infra.run([*ssh,'sudo -n sha256sum -- '+shlex.quote(target)],timeout=20).decode().split()[0]
    require(actual==selected['guest_storage_helper_sha256'],'installed guest helper source differs')
    request={'format':1,'job_id':str(uuid.uuid5(uuid.UUID(plan['id']),'storage-'+action)),'environment_id':environment_id(plan),
        'instance_id':plan['instance_id'],'organisation_id':plan['organisation_id'],'vm_id':physical['vm_id'],'daemon_id':physical['daemon_id'],
        'action':action,'storage_gib':limits(plan)['storage_gib'],'shared_workspace':spec(plan)['shared_workspace'],'expected_storage_uid':None if storage is None else storage['storage_uid']}
    if container_id is not None:request['container_id']=container_id
    result=json.loads(infra.run([*ssh,'sudo -n python3 '+shlex.quote(target)],data=canonical(request),timeout=180))
    require(result['job_id']==request['job_id'] and result['quota_bytes']<=limits(plan)['storage_gib']*1073741824
            and (storage is None or result['storage_uid']==storage['storage_uid']),'original guest physical quota readback differs')
    return result
def docker(infra,*args,**kwargs):return infra.run(['docker',*args],**kwargs)
def docker_inspect(infra,identity):
    value=json.loads(docker(infra,'inspect',identity));require(len(value)==1 and value[0]['Id']==identity,'original environment container UID changed');return value[0]
def original_docker(infra,plan,runtime):
    value=docker_inspect(infra,runtime['container_id']);cap=value['HostConfig'];configured=value['Config'];allocation=limits(plan)
    require(configured['Labels']==runtime['labels'] and configured['Image']==plan['template']['image_reference']
        and value['Name']=='/'+runtime['container_name'] and cap['NanoCpus']==allocation['cpu_millis']*1000000
        and cap['Memory']==allocation['memory_mib']*1048576 and cap['MemorySwap']==cap['Memory'] and cap['PidsLimit']==64
        and cap['NetworkMode']=='none' and cap['ReadonlyRootfs'] is True and cap['CapDrop']==['ALL'] and cap['Privileged'] is False
        and configured['User']=='10001:10001' and not cap.get('Devices') and not cap.get('PortBindings'),'original environment isolation/resource caps differ')
    mounts=[m for m in value['Mounts'] if m['Destination']=='/workspace']
    require(len(mounts)==1 and mounts[0]['Type']=='volume' and mounts[0]['Name']==runtime['storage']['volume_name'] and mounts[0]['RW'] is True,'original workspace mount differs')
    return value
def create_docker(infra,plan,marker,path):
    image=plan['template']['image_reference'];require(re.fullmatch('[a-zA-Z0-9][a-zA-Z0-9._:/-]*@sha256:[a-f0-9]{64}',image),'digest image required')
    # Pulling is an explicit job-owned operator action; no mutable tag is used.
    docker(infra,'pull',image)
    previous=plan['environment'].get('runtime') if plan['environment'] else None
    storage=guest(infra,plan,'ensure',previous.get('storage') if previous and not previous.get('storage_released') else None);caps=limits(plan);name='cowork-'+identifier(plan['instance_id'])+'-environment-'+identifier(environment_id(plan))
    require(not docker(infra,'ps','-aq','--no-trunc','--filter','name=^/'+name+'$').strip(),'foreign/original unacknowledged environment name exists')
    marker.update(stage='create_dispatched',storage=storage);save(infra,path,marker)
    authorize(infra,plan)
    args=['create','--name',name,'--network','none','--read-only','--cap-drop','ALL','--security-opt','no-new-privileges:true',
        '--security-opt','apparmor='+infra.config.get('sandbox_apparmor_profile','open-cowork-sandbox'),'--user','10001:10001','--pids-limit','64',
        '--cpus',str(caps['cpu_millis']/1000),'--memory',str(caps['memory_mib'])+'m','--memory-swap',str(caps['memory_mib'])+'m',
        '--tmpfs','/tmp:rw,nosuid,nodev,size=16777216','--mount','type=volume,src='+storage['volume_name']+',dst=/workspace','--entrypoint','/bin/sh']
    for key,value in sorted(labels(plan).items()):args+=['--label',key+'='+value]
    identity=docker(infra,*args,image,'-c','exec sleep infinity').decode().strip();require(re.fullmatch('[a-f0-9]{64}',identity),'original container create ACK missing')
    controller=plan.get('environment_controller')
    runtime={'backend':'docker','instance_id':plan['instance_id'],'organisation_id':plan['organisation_id'],'image_reference':image,'daemon_id':controller['specification']['guest_daemon_id'] if controller else plan['backend_profile']['specification']['daemon_id'],
        'container_id':identity,'container_name':name,'labels':labels(plan),'storage':storage,'limits':caps}
    acknowledged=docker_inspect(infra,identity)
    require(all(acknowledged['Config']['Labels'].get(key)==value for key,value in labels(plan).items()),'original environment create labels differ')
    runtime['labels']=acknowledged['Config']['Labels']
    if controller:runtime.update(environment_backend=controller['backend'],vm_controller_id=controller['id'],controller_sha256=controller['specification_sha256'],physical_vm_id=controller['specification']['vm_id'],physical_vm_hardware_uuid=controller['specification']['guest_hardware_uuid'])
    marker.update(stage='created',runtime=runtime);save(infra,path,marker);original_docker(infra,plan,runtime)
    guest(infra,plan,'bind',storage,identity)
    marker['stage']='start_dispatched';save(infra,path,marker);authorize(infra,plan);docker(infra,'start',identity);probe_docker(infra,plan,runtime)
    return runtime
def probe_docker(infra,plan,runtime):
    authorize(infra,plan)
    value=original_docker(infra,plan,runtime);require(value['State']['Running'] and value['State']['Pid']>0,'original environment runtime not physically running')
    challenge=plan['challenge'];probe='set -eu; test "$(id -u)" = 10001; test "$(cat /sys/fs/cgroup/pids.max)" = 64; test "$(cat /sys/fs/cgroup/memory.max)" = '+str(limits(plan)['memory_mib']*1048576)+'; printf %s '+shlex.quote(challenge)+' > /workspace/.cowork-probe-'+identifier(plan['id'])+'; cat /workspace/.cowork-probe-'+identifier(plan['id'])
    # Read/probe is nonce-bound. Persisting a mutation job before this exec means
    # lost exec ACKs are held, never automatically repeated as task execution.
    require(docker(infra,'exec',runtime['container_id'],'/bin/sh','-c',probe,timeout=30).decode()==challenge,'actual environment challenge/workspace probe differs')
    guest(infra,plan,'inspect',runtime['storage']);after=original_docker(infra,plan,runtime)
    require(after['State']['Running'] and after['State']['StartedAt']==value['State']['StartedAt'],'environment restarted during challenge probe')
    runtime.update(challenge_observed=challenge,workspace_write_verified=True,resource_limits_verified=True,started_at=value['State']['StartedAt'])
def stop_docker(infra,plan,runtime,marker,path):
    authorize(infra,plan)
    value=original_docker(infra,plan,runtime)
    if value['State']['Running']:
        marker['stage']='stop_dispatched';save(infra,path,marker);docker(infra,'stop','--time','30',runtime['container_id'])
    actual=original_docker(infra,plan,runtime);state=actual['State']
    require(not state['Running'] and state['Pid']==0 and state['FinishedAt'] and not state['FinishedAt'].startswith('0001'),'physical original container termination unproven')
    terminal={'container_id':runtime['container_id'],'started_at':state['StartedAt'],'finished_at':state['FinishedAt'],'exit_code':state['ExitCode']}
    marker.update(stage='terminated',terminal=terminal);save(infra,path,marker)
    docker(infra,'rm',runtime['container_id'])
    require(not docker(infra,'ps','-aq','--no-trunc','--filter','id='+runtime['container_id']).strip(),'original stopped container remains')
    return terminal
def kube(infra,plan,*args,data=None):
    return infra.run(['kubectl','--namespace',plan['backend_profile']['specification']['namespace'],'--request-timeout=20s',*args],data=data,timeout=90)
def object(infra,plan,kind,name):
    raw=kube(infra,plan,'get',kind,name,'--ignore-not-found','-o','json');return None if not raw.strip() else json.loads(raw)
def actual_kube_scope(infra,plan):
    profile=plan['backend_profile']['specification'];actual=object(infra,plan,'namespace',profile['namespace'])
    require(actual and actual['metadata']['uid']==profile['namespace_uid'],'original namespace UID changed')
def uid_delete(infra,plan,kind,name,identity):
    actual=object(infra,plan,kind,name)
    if actual is None:return
    require(actual['metadata']['uid']==identity,'original Kubernetes deletion UID changed')
    plural={'pod':'pods','pvc':'persistentvolumeclaims','networkpolicy':'networkpolicies'}[kind]
    group='/apis/networking.k8s.io/v1' if kind=='networkpolicy' else '/api/v1'
    uri=group+'/namespaces/'+plan['backend_profile']['specification']['namespace']+'/'+plural+'/'+name
    # kubectl raw delete accepts actual DeleteOptions in stdin; UID+RV CAS.
    kube(infra,plan,'delete','--raw',uri,'-f','-',data=canonical({'apiVersion':'v1','kind':'DeleteOptions','preconditions':{'uid':identity,'resourceVersion':actual['metadata']['resourceVersion']},'propagationPolicy':'Foreground'}))
def create_kube(infra,plan,marker,path,directory):
    authorize(infra,plan)
    import kubernetes_lifecycle
    selected=kubernetes_lifecycle.settings(infra.config);actual_kube_scope(infra,plan);caps=limits(plan);identity=identifier(environment_id(plan));name='environment-'+identity
    workspace='env-workspace-'+(identifier(plan['instance_id']) if spec(plan)['shared_workspace'] else identity)
    refs=directory/(workspace+'.json');claim=object(infra,plan,'pvc',workspace)
    if refs.exists():
        storage=read(refs);require(claim and claim['metadata']['uid']==storage['storage_uid'] and storage['storage_gib']==caps['storage_gib'],'original/shared workspace PVC changed')
    else:
        require(claim is None,'foreign/unacknowledged environment PVC exists')
        marker['stage']='storage_dispatched';save(infra,path,marker)
        value={'apiVersion':'v1','kind':'PersistentVolumeClaim','metadata':{'name':workspace,'labels':{'cowork.instance':plan['instance_id'],'cowork.organisation':plan['organisation_id'],'cowork.environment-storage':workspace}},
            'spec':{'storageClassName':selected['storage_class'],'accessModes':['ReadWriteOnce'],'resources':{'requests':{'storage':str(caps['storage_gib'])+'Gi'}}}}
        authorize(infra,plan);claim=json.loads(kube(infra,plan,'create','-f','-','-o','json',data=canonical(value)))
        storage={'storage_uid':claim['metadata']['uid'],'volume_name':workspace,'storage_gib':caps['storage_gib'],'refs':[]};save(infra,refs,storage)
    storage['refs']=sorted(set(storage['refs']+[environment_id(plan)]));save(infra,refs,storage)
    require(object(infra,plan,'pod',name) is None and object(infra,plan,'networkpolicy',name) is None,'foreign/unacknowledged environment runtime exists')
    marker.update(stage='network_dispatched',storage=storage);save(infra,path,marker)
    policy={'apiVersion':'networking.k8s.io/v1','kind':'NetworkPolicy','metadata':{'name':name,'labels':labels(plan)},'spec':{'podSelector':{'matchLabels':{'cowork.environment':environment_id(plan)}},'policyTypes':['Ingress','Egress'],'ingress':[],'egress':[]}}
    authorize(infra,plan);network=json.loads(kube(infra,plan,'create','-f','-','-o','json',data=canonical(policy)))
    marker.update(stage='create_dispatched',network_uid=network['metadata']['uid']);save(infra,path,marker)
    pod={'apiVersion':'v1','kind':'Pod','metadata':{'name':name,'labels':labels(plan),'finalizers':[FINALIZER]},'spec':{'restartPolicy':'Never','automountServiceAccountToken':False,
        'runtimeClassName':selected['runtime_class'],'nodeSelector':selected['node_selector'],'securityContext':{'runAsNonRoot':True,'runAsUser':10001,'runAsGroup':10001,'fsGroup':10001,'seccompProfile':{'type':'RuntimeDefault'}},
        'containers':[{'name':'environment','image':plan['template']['image_reference'],'command':['/bin/sh','-c','exec sleep infinity'],'resources':{'requests':{'cpu':str(caps['cpu_millis'])+'m','memory':str(caps['memory_mib'])+'Mi'},'limits':{'cpu':str(caps['cpu_millis'])+'m','memory':str(caps['memory_mib'])+'Mi'}},
            'securityContext':{'readOnlyRootFilesystem':True,'allowPrivilegeEscalation':False,'capabilities':{'drop':['ALL']}},'volumeMounts':[{'name':'workspace','mountPath':'/workspace'},{'name':'temporary','mountPath':'/tmp'}]}],
        'volumes':[{'name':'workspace','persistentVolumeClaim':{'claimName':workspace}},{'name':'temporary','emptyDir':{'medium':'Memory','sizeLimit':'16Mi'}}]}}
    authorize(infra,plan);actual=json.loads(kube(infra,plan,'create','-f','-','-o','json',data=canonical(pod)))
    runtime={'backend':'kubernetes','instance_id':plan['instance_id'],'organisation_id':plan['organisation_id'],'image_reference':plan['template']['image_reference'],
        'cluster_uid':plan['backend_profile']['specification']['cluster_uid'],'namespace_uid':plan['backend_profile']['specification']['namespace_uid'],'pod_uid':actual['metadata']['uid'],
        'pod_name':name,'original_resource_version':actual['metadata']['resourceVersion'],'pod_spec':actual['spec'],'labels':labels(plan),'network_uid':network['metadata']['uid'],'storage':storage,'limits':caps}
    marker.update(stage='created',runtime=runtime);save(infra,path,marker)
    deadline=time.monotonic()+90
    while time.monotonic()<deadline:
        actual=original_kube(infra,plan,runtime)
        if actual.get('status',{}).get('phase')=='Running' and any(s.get('ready') for s in actual['status'].get('containerStatuses',[])):break
        time.sleep(1)
    probe_kube(infra,plan,runtime);return runtime
def original_kube(infra,plan,runtime):
    actual_kube_scope(infra,plan);actual=object(infra,plan,'pod',runtime['pod_name'])
    require(actual and actual['metadata']['uid']==runtime['pod_uid'] and actual['metadata']['labels']==runtime['labels'] and FINALIZER in actual['metadata'].get('finalizers',[]),'original environment pod ACK/UID differs')
    before=runtime['pod_spec'];current=actual['spec']
    # Admission may add defaults, but may never change original security,
    # runtime class, image, resources, mounts or scoped node selector.
    for key in ('containers','volumes','securityContext','automountServiceAccountToken','runtimeClassName','nodeSelector','restartPolicy'):
        require(current.get(key)==before.get(key),'original environment pod specification differs')
    return actual
def probe_kube(infra,plan,runtime):
    authorize(infra,plan)
    actual=original_kube(infra,plan,runtime);require(actual['status']['phase']=='Running','original environment pod not running')
    node=object(infra,plan,'node',actual['spec']['nodeName']);selected=infra.config['kubernetes_execution']
    require(node['metadata']['name'] in selected['node_names'] and node['metadata']['uid']==selected['node_uids'][node['metadata']['name']],'original assigned execution node UID changed')
    config=json.loads(kube(infra,plan,'get','--raw','/api/v1/nodes/'+node['metadata']['name']+'/proxy/configz'))
    require(type(config['kubeletconfig']['podPidsLimit']) is int and 0<config['kubeletconfig']['podPidsLimit']<=64,'actual environment node PID cap exceeds64')
    challenge=plan['challenge'];file='/workspace/.cowork-probe-'+identifier(plan['id'])
    require(kube(infra,plan,'exec',runtime['pod_name'],'--container=environment','--','/bin/sh','-c','set -eu; test "$(id -u)" = 10001; printf %s '+shlex.quote(challenge)+' > '+file+'; cat '+file).decode()==challenge,'actual environment challenge/workspace probe differs')
    after=original_kube(infra,plan,runtime);status=after['status']['containerStatuses'][0]
    require(status['ready'] and status['restartCount']==0 and status['state'].get('running') and status['imageID'].endswith(plan['template']['image_reference'].split('@')[1]),'actual original digest/container runtime changed')
    claim=object(infra,plan,'pvc',runtime['storage']['volume_name']);require(claim['metadata']['uid']==runtime['storage']['storage_uid'] and claim['status']['phase']=='Bound','original environment persistent storage unbound')
    measured=kube(infra,plan,'exec',runtime['pod_name'],'--container=environment','--','stat','-f','-c','%S:%b','/workspace').decode().strip()
    require(re.fullmatch('[0-9]+:[0-9]+',measured) and 0<int(measured.split(':')[0])*int(measured.split(':')[1])<=limits(plan)['storage_gib']*1073741824,'actual environment filesystem exceeds original physical quota')
    runtime.update(node_name=node['metadata']['name'],node_uid=node['metadata']['uid'],container_runtime_id=status['containerID'],observed_resource_version=after['metadata']['resourceVersion'],
        challenge_observed=challenge,workspace_write_verified=True,resource_limits_verified=True,started_at=status['state']['running']['startedAt'])
def stop_kube(infra,plan,runtime,marker,path):
    authorize(infra,plan)
    actual=original_kube(infra,plan,runtime);marker['stage']='stop_dispatched';save(infra,path,marker)
    uid_delete(infra,plan,'pod',runtime['pod_name'],runtime['pod_uid']);deadline=time.monotonic()+120
    while time.monotonic()<deadline:
        actual=original_kube(infra,plan,runtime);statuses=actual.get('status',{}).get('containerStatuses',[])
        if statuses and all(s.get('state',{}).get('terminated') for s in statuses):break
        time.sleep(1)
    terminal=[{'container_runtime_id':s['containerID'],'termination':s['state']['terminated']} for s in statuses]
    require(terminal and all(t['container_runtime_id']==runtime['container_runtime_id'] and t['termination']['finishedAt'] for t in terminal),'physical original environment pod termination unproven')
    marker.update(stage='terminated',terminal=terminal);save(infra,path,marker)
    patch=[{'op':'test','path':'/metadata/uid','value':runtime['pod_uid']},{'op':'test','path':'/metadata/resourceVersion','value':actual['metadata']['resourceVersion']},
        {'op':'test','path':'/metadata/finalizers','value':actual['metadata']['finalizers']},{'op':'replace','path':'/metadata/finalizers','value':[f for f in actual['metadata']['finalizers'] if f!=FINALIZER]}]
    kube(infra,plan,'patch','pod',runtime['pod_name'],'--type=json','-p',json.dumps(patch));deadline=time.monotonic()+60
    while object(infra,plan,'pod',runtime['pod_name']) is not None and time.monotonic()<deadline:time.sleep(1)
    require(object(infra,plan,'pod',runtime['pod_name']) is None,'physically terminated original pod remains')
    uid_delete(infra,plan,'networkpolicy',runtime['pod_name'],runtime['network_uid'])
    require(object(infra,plan,'networkpolicy',runtime['pod_name']) is None,'original environment network policy remains')
    return terminal
def release_kube_storage(infra,plan,runtime,directory):
    authorize(infra,plan)
    storage=runtime['storage'];path=directory/(storage['volume_name']+'.json');marker=read(path)
    require(marker['storage_uid']==storage['storage_uid'] and environment_id(plan) in marker['refs'],'original workspace reference changed')
    marker['refs'].remove(environment_id(plan))
    if marker['refs']:save(infra,path,marker);return
    actual=object(infra,plan,'pvc',storage['volume_name']);require(actual and actual['metadata']['uid']==storage['storage_uid'],'original PVC UID changed')
    pv=object(infra,plan,'pv',actual['spec']['volumeName']);require(pv['spec']['persistentVolumeReclaimPolicy']=='Delete' and pv['spec'].get('csi') and pv['spec']['claimRef']['uid']==storage['storage_uid'],'reviewed physical CSI Delete storage required')
    pods=json.loads(kube(infra,plan,'get','pods','-o','json'))['items'];require(not any(v.get('persistentVolumeClaim',{}).get('claimName')==storage['volume_name'] for p in pods for v in p['spec'].get('volumes',[])),'original workspace still mounted')
    uid_delete(infra,plan,'pvc',storage['volume_name'],storage['storage_uid']);deadline=time.monotonic()+120
    while time.monotonic()<deadline and (object(infra,plan,'pvc',storage['volume_name']) is not None or object(infra,plan,'pv',pv['metadata']['name']) is not None):time.sleep(1)
    require(object(infra,plan,'pvc',storage['volume_name']) is None and object(infra,plan,'pv',pv['metadata']['name']) is None,'original PVC/CSI allocation deletion unproven')
    path.unlink()
def released_storage(infra,plan,runtime,directory):
    storage=runtime['storage']
    if runtime['backend']=='docker':
        require(guest(infra,plan,'read_release',storage)['reference_released'],'original workspace reference reappeared');return
    marker=directory/(storage['volume_name']+'.json');claim=object(infra,plan,'pvc',storage['volume_name'])
    if marker.exists():
        value=read(marker);require(value['storage_uid']==storage['storage_uid'] and environment_id(plan) not in value['refs']
            and claim and claim['metadata']['uid']==storage['storage_uid'],'original released shared reference changed')
    else:require(claim is None,'original released PVC reappeared')
def execute(infra,job):
    if job['plan']['action'] in ('power_start','power_stop'):
        import external_vm_power
        return external_vm_power.execute_job(infra,job)
    if job['plan'].get('environment_controller') and job['plan']['environment_controller']['backend']=='open_shell':
        import openshell_environment
        return openshell_environment.execute_job(infra,job)
    if job['plan'].get('environment_controller'):
        import external_vm
        infra=external_vm.Infrastructure(infra,job['plan'])
    plan,directory,path=scope(infra,job);configuration=settings(infra.config);require(configuration is not None,'environment lifecycle not opted in')
    if path.exists():
        marker=read(path);require(marker['plan_sha256']==job['plan_sha256'] and marker['lease_token']==job['lease_token'],'original environment journal differs')
        if marker.get('receipt') is not None:return marker['receipt']
        if job.get('recovery_authorized') is True:return recover(infra,job,plan,directory,path,marker)
        raise RuntimeError('environment uncertain original action held; no create/exec replay')
    marker={'format':1,'job_id':job['id'],'lease_token':job['lease_token'],'plan_sha256':job['plan_sha256'],'stage':'prepared'};save(infra,path,marker)
    native=plan['backend_profile']['specification']['kind']=='kubernetes';action=plan['action'];runtime=plan['environment'].get('runtime') if plan['environment'] else None
    if action=='delete' and runtime is None:
        # A metadata-only environment is deletable only while both exact owned
        # runtime/storage names are physically absent. Existing names are never
        # adopted or removed without an original acknowledgement.
        compact=identifier(environment_id(plan));name='environment-'+compact
        if native:
            actual_kube_scope(infra,plan)
            require(object(infra,plan,'pod',name) is None and object(infra,plan,'networkpolicy',name) is None
                    and object(infra,plan,'pvc','env-workspace-'+compact) is None,'uncreated environment has unacknowledged infrastructure')
        else:
            name='cowork-'+identifier(plan['instance_id'])+'-environment-'+compact
            require(not docker(infra,'ps','-aq','--no-trunc','--filter','name=^/'+name+'$').strip()
                    and not docker(infra,'volume','ls','--filter','label=cowork.environment-storage='+identifier(plan['instance_id'])+'-'+compact,'--format','{{.Name}}').strip(),'uncreated environment has unacknowledged infrastructure')
        receipt={'format':'cowork-environment-completion-v1','job_id':job['id'],'plan_sha256':job['plan_sha256'],'challenge':plan['challenge'],'action':action,
            'backend_profile_sha256':plan['backend_profile']['specification_sha256'],'runtime':None,'uncreated_environment_deleted':True,'original_runtime_absent':True,'storage_release_verified':True}
        marker.update(stage='completed',receipt=receipt);save(infra,path,marker);return receipt
    if action in ('qualify','start'):
        if runtime is not None:require(runtime.get('stopped') is True,'original retained environment runtime not stopped')
        runtime=create_kube(infra,plan,marker,path,directory) if native else create_docker(infra,plan,marker,path)
    else:
        require(runtime is not None,'original environment has never been physically created; qualify/start it before stopping/deleting')
        # Probe and stop use original identities, with a fresh challenge on a
        # running runtime; stopped records retain their authenticated evidence.
        if not runtime.get('stopped'):
            if native:probe_kube(infra,plan,runtime)
            else:probe_docker(infra,plan,runtime)
        else:
            # Keep the actual original runtime challenge. Fresh namespace/VM
            # scope and original UID absence are read below, never invented as
            # a new execution on an already stopped runtime.
            runtime=dict(runtime)
    stopped=action in ('qualify','stop','delete')
    if stopped and not runtime.get('stopped'):
        terminal=stop_kube(infra,plan,runtime,marker,path) if native else stop_docker(infra,plan,runtime,marker,path)
        runtime.update(stopped=True,physical_termination=terminal);marker['runtime']=runtime;save(infra,path,marker)
    elif stopped:
        require(runtime.get('physical_termination') is not None,'original physical stop receipt missing')
        if native:require(object(infra,plan,'pod',runtime['pod_name']) is None,'stopped original pod reappeared')
        else:require(not docker(infra,'ps','-aq','--no-trunc','--filter','id='+runtime['container_id']).strip(),'stopped original container reappeared')
    released=action in ('qualify','delete') or action=='stop' and not spec(plan)['persistent']
    if released:
        marker['stage']='storage_release_dispatched';save(infra,path,marker)
        if runtime.get('storage_released'):
            released_storage(infra,plan,runtime,directory)
        elif native:release_kube_storage(infra,plan,runtime,directory)
        else:require(guest(infra,plan,'release',runtime['storage'])['reference_released'] is True,'original storage reference not physically released')
        runtime['storage_released']=True
    receipt={'format':'cowork-environment-completion-v1','job_id':job['id'],'plan_sha256':job['plan_sha256'],'challenge':plan['challenge'],'action':action,
        'backend_profile_sha256':plan['backend_profile']['specification_sha256'],'runtime':runtime,'physical_stop_verified':stopped,
        'original_runtime_absent':stopped,'storage_release_verified':released}
    marker.update(stage='completed',runtime=runtime,receipt=receipt);save(infra,path,marker);return receipt
def recover(infra,job,plan,directory,path,marker):
    require(job.get('recovery_authorized') is True and marker.get('runtime') is not None,'original create ACK missing; trusted physical fencing required')
    runtime=marker['runtime'];native=runtime['backend']=='kubernetes'
    # This read-only nonce probe is allowed only on an acknowledged running
    # original runtime. A lost Create/Start ACK never authorizes another start.
    if marker.get('terminal') is None:
        if native:probe_kube(infra,plan,runtime)
        else:probe_docker(infra,plan,runtime)
        terminal=stop_kube(infra,plan,runtime,marker,path) if native else stop_docker(infra,plan,runtime,marker,path)
        runtime.update(stopped=True,physical_termination=terminal);marker['runtime']=runtime;save(infra,path,marker)
    else:
        runtime.update(stopped=True,physical_termination=marker['terminal'])
        if native:
            actual=object(infra,plan,'pod',runtime['pod_name'])
            if actual is not None:
                require(actual['metadata']['uid']==runtime['pod_uid'] and all(s.get('state',{}).get('terminated',{}).get('finishedAt') for s in actual.get('status',{}).get('containerStatuses',[])),'original physical terminal changed')
                patch=[{'op':'test','path':'/metadata/uid','value':runtime['pod_uid']},{'op':'test','path':'/metadata/resourceVersion','value':actual['metadata']['resourceVersion']},
                    {'op':'replace','path':'/metadata/finalizers','value':[v for v in actual['metadata'].get('finalizers',[]) if v!=FINALIZER]}]
                kube(infra,plan,'patch','pod',runtime['pod_name'],'--type=json','-p',json.dumps(patch))
            require(object(infra,plan,'pod',runtime['pod_name']) is None,'terminated original pod still retained')
            uid_delete(infra,plan,'networkpolicy',runtime['pod_name'],runtime['network_uid'])
        else:
            present=docker(infra,'ps','-aq','--no-trunc','--filter','id='+runtime['container_id']).strip()
            if present:
                actual=original_docker(infra,plan,runtime)
                require(not actual['State']['Running'] and actual['State']['Pid']==0 and actual['State']['FinishedAt']==marker['terminal']['finished_at'],'original physical terminal changed')
                docker(infra,'rm',runtime['container_id'])
            require(not docker(infra,'ps','-aq','--no-trunc','--filter','id='+runtime['container_id']).strip(),'terminated original container remains')
    released=plan['action'] in ('qualify','delete') or plan['action']=='stop' and not spec(plan)['persistent']
    if released:
        if native:
            storage=directory/(runtime['storage']['volume_name']+'.json')
            if storage.exists():release_kube_storage(infra,plan,runtime,directory)
            else:require(object(infra,plan,'pvc',runtime['storage']['volume_name']) is None,'original released PVC reappeared')
        else:require(guest(infra,plan,'release',runtime['storage'])['reference_released'],'original storage release unproven')
        runtime['storage_released']=True
    receipt={'format':'cowork-environment-completion-v1','job_id':job['id'],'plan_sha256':job['plan_sha256'],'challenge':plan['challenge'],'action':plan['action'],
        'backend_profile_sha256':plan['backend_profile']['specification_sha256'],'runtime':runtime,'recovered_stopped':plan['action'] in ('start','qualify','inspect'),
        'physical_stop_verified':True,'original_runtime_absent':True,'storage_release_verified':released}
    marker.update(stage='completed',runtime=runtime,receipt=receipt);save(infra,path,marker);return receipt
def process(manager,infra):
    if settings(infra.config) is None:return
    binding=executor_host.manager_binding(infra.config);request={'executor_host_binding':binding}
    def guarded(job):
        def check(plan):
            result=manager.call('/environments/'+job['id']+'/authorize',{**request,'lease_token':job['lease_token'],'plan_sha256':job['plan_sha256'],'receipt':{}})
            require(result['id']==job['id'] and result['plan_sha256']==job['plan_sha256'] and result['authorized'] is True,'current original manager environment authorization changed')
        infra._environment_authorizer=check
    # Only completed original jobs are replayed. Every uncertain original plan
    # remains held, retaining manager pool reservations and its original lease.
    for job in manager.call('/environments/pending',request):
        path=infra.directory(job['instance_id'])/'environments'/(identifier(job['id'])+'.json')
        if path.exists() and read(path).get('receipt') is not None:
            manager.call('/environments/'+job['id']+'/complete',{**request,'lease_token':job['lease_token'],'plan_sha256':job['plan_sha256'],'receipt':read(path)['receipt']})
        elif job.get('recovery_authorized') is True:
            guarded(job)
            try:receipt=execute(infra,job)
            except Exception:continue
            manager.call('/environments/'+job['id']+'/complete',{**request,'lease_token':job['lease_token'],'plan_sha256':job['plan_sha256'],'receipt':receipt})
    job=manager.call('/environments/claim',request)
    if job:
        guarded(job)
        try:receipt=execute(infra,job)
        except Exception:
            manager.call('/environments/'+job['id']+'/hold',{**request,'lease_token':job['lease_token'],'plan_sha256':job['plan_sha256'],'receipt':{}})
            return
        manager.call('/environments/'+job['id']+'/complete',{**request,'lease_token':job['lease_token'],'plan_sha256':job['plan_sha256'],'receipt':receipt})
def docker_capacity(infra,container,instance):
    label=container['Config'].get('Labels',{});environment=label.get('cowork.environment');create=label.get('cowork.environment-create')
    path=infra.root/identifier(instance)/'environments'/(identifier(create)+'.json');marker=read(path);runtime=marker['runtime']
    require(marker.get('stage') in ('created','start_dispatched','completed') and runtime['container_id']==container['Id'] and runtime['labels']==label
        and label['cowork.managed']=='environment-lifecycle' and environment==label['cowork.environment'],'environment workload has no original create ACK')
    caps=container['HostConfig'];expected=runtime['limits']
    require(caps['NanoCpus']==expected['cpu_millis']*1000000 and caps['Memory']==expected['memory_mib']*1048576 and caps['PidsLimit']==64,'environment actual original resource caps differ')
    return True
def kubernetes_capacity(infra,pod,instance):
    label=pod['metadata'].get('labels',{});path=infra.root/identifier(instance)/'environments'/(identifier(label['cowork.environment-create'])+'.json')
    marker=read(path);runtime=marker['runtime'];require(label['cowork.managed']=='environment-lifecycle' and runtime['pod_uid']==pod['metadata']['uid'] and runtime['labels']==label
        and pod['spec'].get('runtimeClassName')==runtime['pod_spec']['runtimeClassName'] and pod['spec']['containers']==runtime['pod_spec']['containers'],'environment pod has no original create ACK')
    return True
