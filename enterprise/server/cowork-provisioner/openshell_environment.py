"""Pinned NVIDIA OpenShell v1 gRPC adapter on the original owned VM Docker.

The reviewed descriptor/binary, authenticated peer and sole-owner workspace are
enrolled explicitly. CPU/RAM/PID/storage and stop proofs come from actual Docker
UIDs and the guest filesystem, never merely from portable resource flags.
"""
import base64,hashlib,importlib.util,json,os,re,socket,ssl,time,uuid
from pathlib import Path
import environment_lifecycle as lc,external_vm

def events(raw):
    text=raw.decode();decoder=json.JSONDecoder();result=[]
    while text.strip():value,end=decoder.raw_decode(text.lstrip());result.append(value);text=text.lstrip()[end:]
    return result
def rpc(infra,c,method,body,timeout=60,stream=False):
    for key in ('grpcurl_file','descriptor_file','gateway_ca_file','gateway_client_certificate','gateway_client_key','gateway_token_file'):
        lc.require(c[key] in c['credential_pins'] and external_vm.digest(c[key])==c['credential_pins'][c[key]],'reviewed OpenShell client/descriptor/credential changed')
    token=external_vm.private(c['gateway_token_file']).read_text().strip();lc.require(32<=len(token)<=16384,'explicit authenticated OpenShell identity required')
    env=dict(os.environ);env['COWORK_OPENSHELL_AUTH']=token
    args=[c['grpcurl_file'],'-expand-headers','-H','Authorization: Bearer ${COWORK_OPENSHELL_AUTH}','-cacert',c['gateway_ca_file'],'-cert',c['gateway_client_certificate'],'-key',c['gateway_client_key'],'-protoset',c['descriptor_file'],'-max-time',str(timeout-5),'-d','@',c['gateway_endpoint'],'openshell.v1.OpenShell/'+method]
    out=infra.run(args,data=lc.canonical(body),env=env,timeout=timeout);lc.require(len(out)<=4194304,'OpenShell response exceeds original bound');values=events(out)
    lc.require(stream or len(values)==1,'one original OpenShell response required');return values if stream else values[0]
def controller_observation(infra,c):
    lc.require(c['backend']=='open_shell' and re.fullmatch('[A-Za-z0-9.-]+:[0-9]{2,5}',c['gateway_endpoint']) and re.fullmatch('[a-z0-9][a-z0-9-]{0,62}',c['workspace']),'explicit OpenShell gateway/workspace required')
    host,port=c['gateway_endpoint'].rsplit(':',1);context=ssl.create_default_context(cafile=c['gateway_ca_file']);context.load_cert_chain(c['gateway_client_certificate'],c['gateway_client_key'])
    with socket.create_connection((host,int(port)),timeout=5) as wire:
        with context.wrap_socket(wire,server_hostname=host) as tls:peer=hashlib.sha256(tls.getpeercert(binary_form=True)).hexdigest()
    lc.require(peer==c['controller_identity'],'original OpenShell TLS peer certificate changed')
    info=rpc(infra,c,'GetGatewayInfo',{});who=rpc(infra,c,'GetCurrentUser',{});space=rpc(infra,c,'GetWorkspace',{'name':c['workspace']})['workspace']
    lc.require(info['gatewayVersion']==c['gateway_version'] and who['subject']==c['gateway_subject'] and len(info['computeDrivers'])==1 and info['computeDrivers'][0]['name']=='docker','reviewed OpenShell release, authenticated subject or Docker driver changed')
    capabilities=info['computeDrivers'][0]['capabilities']['resourceCapabilities'];lc.require(capabilities['cpu']['limitSupported'] and capabilities['memory']['limitSupported'],'OpenShell driver must actually support CPU/RAM limits; VM flag-only drivers rejected')
    metadata=space['metadata'];lc.require(metadata['id']==c['workspace_uid'] and metadata['labels']['cowork.organisation']==c['organisation_id'] and metadata['labels']['cowork.instance']==c['instance_id'] and metadata['labels']['cowork.controller']==c['controller_id'],'original OpenShell workspace UID/owner changed')
    members=rpc(infra,c,'ListWorkspaceMembers',{'workspaceScope':{'workspace':c['workspace']},'pageSize':1000});lc.require(not members.get('nextPageToken') and len(members['members'])==1 and members['members'][0]['principalSubject']==c['gateway_subject'] and members['members'][0]['role']=='WORKSPACE_ROLE_ADMIN','private OpenShell workspace must have exactly its enrolled operator admin')
    producer=c['parent_profile_producer_file'];lc.require(producer in c['credential_pins'] and external_vm.digest(producer)==c['credential_pins'][producer],'original independent VM producer changed')
    definition=importlib.util.spec_from_file_location('_cowork_openshell_original_parent',producer);module=importlib.util.module_from_spec(definition);definition.loader.exec_module(module)
    configuration,sha=module.configuration(c['parent_backend_configuration_file']);lc.require(c['parent_backend_configuration_file'] in c['credential_pins'] and sha==c['credential_pins'][c['parent_backend_configuration_file']] and all(configuration[k]==c[k] for k in ('organisation_id','instance_id','host_id','operator_state_id')),'original OpenShell parent enrollment changed')
    parent=module.physical(configuration,routing=False);root=Path(configuration['vm_root']);vm=module.vm_module().load(root);lc.require(parent['vm_id']==c['vm_id'] and parent['guest_hardware_uuid']==c['guest_hardware_uuid'] and parent['daemon_id']==c['guest_daemon_id'] and vm['disk_virtual_gib']==c['parent_disk_virtual_gib'] and c['guest']=={'hostname':'127.0.0.1','port':vm['ssh_port'],'username':'cowork','ssh_key':str(root/'ssh-key'),'known_hosts':str(root/'known_hosts')},'actual OpenShell VM parent allocation/SSH endpoint changed')
    daemon=json.loads(infra.run(['docker','info','--format','{{json .}}']));lc.require(daemon['ID']==c['guest_daemon_id'] and daemon['CgroupVersion']=='2','actual OpenShell VM daemon/cgroup identity changed')
    gateway=gateway_process(infra,c)
    lc.require(c['limits']['cpu_millis']+gateway['cpu_millis']<=vm['cpus']*1000 and c['limits']['memory_mib']+gateway['memory_mib']<=vm['memory_mib'] and c['limits']['storage_gib']<=vm['disk_virtual_gib'],'advertised OpenShell pool exceeds actual original VM hardware after gateway reservation')
    return {'peer_certificate_sha256':peer,'gateway_version':info['gatewayVersion'],'descriptor_sha256':c['credential_pins'][c['descriptor_file']],'authenticated_subject':who['subject'],'workspace_uid':metadata['id'],'compute_driver':info['computeDrivers'][0],'daemon_id':daemon['ID'],'physical_parent':parent,'gateway_process':gateway}

def gateway_process(infra,c):
    unit=c['gateway_systemd_unit'];lc.require(re.fullmatch(r'[A-Za-z0-9_-]+\.service',unit),'exact enrolled OpenShell systemd unit required')
    pid=int(external_vm.guest_command(infra,c,['systemctl','show',unit,'--property=MainPID','--value']).decode().strip());lc.require(pid>1,'actual OpenShell gateway process required')
    script='import hashlib,json,os; from pathlib import Path; p=Path("/proc/'+str(pid)+'"); x=(p/"exe").resolve(strict=True); s=(p/"stat").read_text().split(); c=(p/"cgroup").read_text().strip(); assert c.startswith("0::/"); root=Path("/sys/fs/cgroup")/c[4:]; q,t=(root/"cpu.max").read_text().split(); m=(root/"memory.max").read_text().strip(); assert q!="max" and m!="max"; print(json.dumps({"pid":'+str(pid)+',"process_start_ticks":s[21],"binary_sha256":hashlib.sha256(x.read_bytes()).hexdigest(),"binary_owner":x.stat().st_uid,"cpu_millis":int(q)*1000//int(t),"memory_mib":int(m)//1048576,"cgroup":c}))'
    result=json.loads(external_vm.guest_command(infra,c,['python3','-c',script]));lc.require(result['binary_owner']==0 and result['binary_sha256']==c['gateway_binary_sha256'] and result['cpu_millis']==c['gateway_limits']['cpu_millis'] and result['memory_mib']==c['gateway_limits']['memory_mib'],'actual reviewed OpenShell binary/cgroup hard limits changed')
    sockets=external_vm.guest_command(infra,c,['ss','-ltnp']).decode();lc.require(any(re.search(r':'+str(c['gateway_guest_port'])+r'\s',line) and 'pid='+str(pid)+',' in line for line in sockets.splitlines()) and int(external_vm.guest_command(infra,c,['systemctl','show',unit,'--property=MainPID','--value']).decode().strip())==pid,'original TLS gateway listener/process changed');return result
def selected(infra,plan):
    original=plan['environment_controller'];entry=infra.config['environment_controllers']['open_shell'];c,sha=external_vm.configuration(entry['configuration_file'],entry['configuration_sha256']);s=original['specification']
    lc.require(sha==s['configuration_sha256'] and c['controller_id']==original['id'] and all(c[k]==s[k] for k in ('organisation_id','instance_id','host_id','operator_state_id','vm_id','guest_daemon_id','guest_hardware_uuid','controller_identity','limits')) and c['vm_id']==plan['backend_profile']['specification']['vm_id'] and c['guest_daemon_id']==plan['backend_profile']['specification']['daemon_id'],'OpenShell must use the original physically activated VM parent')
    controller_observation(infra,c);return c
def policy():return {'version':1,'filesystem':{'includeWorkdir':False,'readOnly':['/usr','/bin','/sbin','/lib','/lib64','/etc','/proc','/dev'],'readWrite':['/workspace','/tmp']},'landlock':{'compatibility':'hard_requirement'},'process':{'runAsUser':'10001','runAsGroup':'10001'},'networkPolicies':{},'networkMiddlewares':{}}
def normalized(value):
    if isinstance(value,dict):return {k:normalized(v) for k,v in value.items() if v not in (False,None,'',[],{})}
    if isinstance(value,list):return [normalized(v) for v in value]
    return value
def get(infra,c,runtime):
    sandbox=rpc(infra,c,'GetSandbox',{'workspaceScope':{'workspace':c['workspace']},'name':runtime['sandbox_name']})['sandbox']
    lc.require(sandbox['metadata']['id']==runtime['sandbox_id'] and sandbox['metadata']['name']==runtime['sandbox_name'] and sandbox['spec']['template']['image']==runtime['image_reference'] and normalized(sandbox['spec']['policy'])==normalized(runtime['policy']) and sandbox['spec'].get('providers',[])==[] and sandbox['spec']['restartPolicy']=='SANDBOX_RESTART_POLICY_NEVER','original OpenShell sandbox UID/policy/image changed')
    if runtime.get('host_key_fingerprint'):lc.require(sandbox['hostKeyFingerprint']==runtime['host_key_fingerprint'],'original OpenShell SSH identity changed')
    return sandbox
def original_resources(infra,runtime,running=True):
    total=[0,0,0]
    for own in runtime['resources']:
        actual=lc.docker_inspect(infra,own['container_id']);caps=actual['HostConfig'];labels=actual['Config']['Labels']
        lc.require(labels['openshell.ai/sandbox-id']==runtime['sandbox_id'] and labels['openshell.ai/isolation-role']==own['role'] and actual['Image']==own['image_id'] and actual['Config']['Labels']==own['labels'] and caps['NanoCpus']==own['cpu_millis']*1000000 and caps['Memory']==own['memory_mib']*1048576 and caps['MemorySwap']==caps['Memory'] and caps['PidsLimit']==own['pids'] and caps['Privileged'] is False and caps.get('CapDrop')==['ALL'] and not caps.get('Devices') and not caps.get('PortBindings'),'original OpenShell physical resource/caps changed')
        if running:lc.require(actual['State']['Running'] and actual['State']['Pid']>0 and actual['State']['StartedAt']==own['started_at'],'OpenShell original runtime restarted')
        if own['role']=='sandbox':
            image=json.loads(lc.docker(infra,'image','inspect',own['image_id']))[0];lc.require(runtime['image_reference'] in image.get('RepoDigests',[]) and caps['NetworkMode']=='none' and actual['Config']['User'] in ('10001','10001:10001') and any(m['Destination']=='/workspace' and m['Type']=='volume' and m['Name']==runtime['storage']['volume_name'] and m['RW'] is True for m in actual['Mounts']) and any(m.get('Target')=='/tmp' and m.get('Type')=='tmpfs' and 0<m.get('TmpfsOptions',{}).get('SizeBytes',0)<=16777216 for m in caps.get('Mounts',[])),'actual OpenShell workload digest/UID/network/workspace/tmpfs differs')
        total=[total[0]+own['cpu_millis'],total[1]+own['memory_mib'],total[2]+own['pids']]
    lc.require(total[0]<=runtime['limits']['cpu_millis'] and total[1]<=runtime['limits']['memory_mib'] and total[2]<=64,'OpenShell total physical resources exceed reserved environment pool')
def exec(infra,plan,runtime,command,data,timeout):
    c=selected(infra,plan);sandbox=get(infra,c,runtime);original_resources(infra,runtime)
    lc.require(sandbox['status']['phase']=='SANDBOX_PHASE_READY' and sandbox['status']['mainProcessInstanceId']==runtime['runtime_generation'],'original OpenShell generation not ready')
    installed=rpc(infra,c,'GetSandboxConfig',{'sandboxId':runtime['sandbox_id']})
    lc.require(installed['configurationAdmitted'] is True and installed['configurationInstanceId']==runtime['runtime_generation'] and normalized(installed['policy'])==normalized(runtime['policy']) and not installed.get('configurationError'),'actual OpenShell installed policy/generation is not admitted')
    runtime['policy_admission']={'configuration_instance_id':installed['configurationInstanceId'],'policy_sha256':lc.sha(normalized(installed['policy'])),'admitted':True}
    request={'workspaceScope':{'workspace':c['workspace']},'sandbox':runtime['sandbox_name'],'command':command,'workdir':'/workspace','stdin':base64.b64encode(data).decode(),'executionTimeout':str(timeout-5)+'s','noLoginShell':True,'requestId':str(uuid.uuid4())}
    values=rpc(infra,c,'ExecSandbox',request,timeout,True);output=bytearray();exits=[]
    for value in values:
        if 'stdout' in value:output+=base64.b64decode(value['stdout']['data'],validate=True)
        if 'exit' in value:exits.append(value['exit'].get('exitCode',0))
        lc.require(len(output)<=1048576,'OpenShell process delivery exceeds bound')
    lc.require(exits==[0] and get(infra,c,runtime)['status']['mainProcessInstanceId']==runtime['runtime_generation'],'OpenShell original execution delivery/generation unproven');original_resources(infra,runtime);return bytes(output)
def execute_job(infra,job):
    plan,directory,path=lc.scope(infra,job);c=selected(infra,plan);action=plan['action'];runtime=plan['environment'].get('runtime') if plan['environment'] else None
    if path.exists():
        marker=lc.read(path);lc.require(marker['plan_sha256']==job['plan_sha256'] and marker['lease_token']==job['lease_token'],'original OpenShell journal changed')
        if marker.get('receipt') is not None:return marker['receipt']
        lc.require(job.get('recovery_authorized') is True and marker.get('runtime') is not None,'OpenShell unknown create/exec ACK held; no launch replay');runtime=marker['runtime'];action='stop'
    else:marker={'format':1,'plan_sha256':job['plan_sha256'],'lease_token':job['lease_token'],'environment_id':lc.environment_id(plan),'stage':'prepared'};lc.save(infra,path,marker)
    if action=='delete' and runtime is None:
        name='environment-'+lc.identifier(lc.environment_id(plan));listing=rpc(infra,c,'ListSandboxes',{'workspaceScope':{'workspace':c['workspace']},'pageSize':1000})
        lc.require(not listing.get('nextPageToken') and not any(s['metadata']['name']==name for s in listing.get('sandboxes',[])) and not lc.docker(infra,'ps','-aq','--no-trunc','--filter','label=cowork.environment='+lc.environment_id(plan)).strip() and not lc.docker(infra,'volume','ls','--filter','label=cowork.environment-storage='+lc.identifier(plan['instance_id'])+'-'+lc.identifier(lc.environment_id(plan)),'--format','{{.Name}}').strip(),'uncreated SDK environment has unacknowledged resources')
        receipt={'format':'cowork-environment-completion-v1','job_id':job['id'],'plan_sha256':job['plan_sha256'],'challenge':plan['challenge'],'action':plan['action'],'backend_profile_sha256':plan['backend_profile']['specification_sha256'],'runtime':None,'uncreated_environment_deleted':True,'original_runtime_absent':True,'storage_release_verified':True}
        marker.update(stage='completed',receipt=receipt);lc.save(infra,path,marker);return receipt
    if action in ('qualify','start'):
        caps=lc.limits(plan);lc.require(caps['cpu_millis']>=500 and caps['memory_mib']>=512,'OpenShell reserves its actual supervisor and workload together')
        storage=lc.guest(infra,plan,'ensure',runtime.get('storage') if runtime and not runtime.get('storage_released') else None);name='environment-'+lc.identifier(lc.environment_id(plan));p=policy()
        request={'workspaceScope':{'workspace':c['workspace']},'requestId':plan['id'],'sandbox':{'metadata':{'name':name,'labels':lc.labels(plan)},'spec':{'template':{'image':plan['template']['image_reference'],'labels':lc.labels(plan),'resources':{'limits':{'cpu':str(caps['cpu_millis']-100)+'m','memory':str(caps['memory_mib']-128)+'Mi'}},'driverConfig':{'docker':{'mounts':[{'type':'volume','source':storage['volume_name'],'target':'/workspace','read_only':False},{'type':'tmpfs','target':'/tmp','size_bytes':16777216}]}}},'policy':p,'command':['/bin/sh','-c','exec sleep infinity'],'restartPolicy':'SANDBOX_RESTART_POLICY_NEVER'}}}
        marker['stage']='create_dispatched';lc.save(infra,path,marker);lc.authorize(infra,plan);sandbox=rpc(infra,c,'CreateSandbox',request)['sandbox'];lc.identifier(sandbox['metadata']['id'])
        runtime={'backend':'open_shell','environment_backend':'open_shell','vm_controller_id':plan['environment_controller']['id'],'controller_sha256':plan['environment_controller']['specification_sha256'],'physical_vm_id':c['vm_id'],'physical_vm_hardware_uuid':c['guest_hardware_uuid'],'instance_id':plan['instance_id'],'organisation_id':plan['organisation_id'],'daemon_id':c['guest_daemon_id'],'sandbox_id':sandbox['metadata']['id'],'sandbox_name':name,'policy':p,'image_reference':plan['template']['image_reference'],'limits':caps,'storage':storage}
        marker.update(stage='sandbox_acknowledged',runtime=runtime);lc.save(infra,path,marker);deadline=time.monotonic()+120
        while True:
            sandbox=get(infra,c,runtime)
            if sandbox['status']['phase']=='SANDBOX_PHASE_READY':break
            lc.require(time.monotonic()<deadline,'actual OpenShell readiness unproven');time.sleep(1)
        ids=lc.docker(infra,'ps','-aq','--no-trunc','--filter','label=openshell.ai/sandbox-id='+runtime['sandbox_id']).decode().splitlines();lc.require(2<=len(ids)<=4 and all(re.fullmatch('[a-f0-9]{64}',v) for v in ids),'actual original OpenShell workload/supervisor inventory required');resources=[]
        for cid in ids:
            a=lc.docker_inspect(infra,cid);role=a['Config']['Labels']['openshell.ai/isolation-role'];lc.require(role in ('sandbox','supervisor','identity'),'unknown OpenShell resource role')
            if role=='sandbox':cpu=caps['cpu_millis']-100;memory=caps['memory_mib']-128
            else:cpu=100//(len(ids)-1);memory=128//(len(ids)-1);lc.require(a['Image'] in c['trusted_auxiliary_image_ids'],'OpenShell auxiliary release image is not enrolled')
            resources.append({'container_id':cid,'role':role,'image_id':a['Image'],'labels':a['Config']['Labels'],'cpu_millis':cpu,'memory_mib':memory,'pids':64//len(ids),'started_at':a['State']['StartedAt']})
        lc.require(sum(v['role']=='sandbox' for v in resources)==1,'one original OpenShell workload required');runtime['resources']=resources;runtime['container_id']=next(v['container_id'] for v in resources if v['role']=='sandbox');runtime['labels']=lc.docker_inspect(infra,runtime['container_id'])['Config']['Labels'];runtime['container_name']=name;runtime['runtime_generation']=sandbox['status']['mainProcessInstanceId'];runtime['host_key_fingerprint']=sandbox['hostKeyFingerprint'];runtime['started_at']=sandbox['status']['mainProcessStartedTime'];marker.update(stage='created',runtime=runtime);lc.save(infra,path,marker)
        for own in resources:
            lc.authorize(infra,plan);lc.docker(infra,'update','--cpus',str(own['cpu_millis']/1000),'--memory',str(own['memory_mib'])+'m','--memory-swap',str(own['memory_mib'])+'m','--pids-limit',str(own['pids']),own['container_id'])
        lc.guest(infra,plan,'bind',storage,runtime['container_id'])
    lc.require(runtime is not None,'original OpenShell create ACK required')
    if not runtime.get('stopped') and action in ('qualify','start','inspect'):
        lc.authorize(infra,plan);challenge=plan['challenge'];out=exec(infra,plan,runtime,['/bin/sh','-c','set -eu; test "$(id -u)" = 10001; printf %s '+challenge+' > /workspace/.cowork-probe-'+lc.identifier(plan['id'])+'; cat /workspace/.cowork-probe-'+lc.identifier(plan['id'])],b'',30);lc.require(out.decode()==challenge,'actual OpenShell nonce/workspace probe differs');lc.guest(infra,plan,'inspect',runtime['storage']);runtime.update(challenge_observed=challenge,workspace_write_verified=True,resource_limits_verified=True)
    stopped=action in ('qualify','stop','delete');released=action in ('qualify','delete') or action=='stop' and not lc.spec(plan)['persistent']
    if stopped and not runtime.get('stopped'):
        get(infra,c,runtime)
        saved=marker.get('terminal',[])
        lc.require(all(t['container_id'] in {v['container_id'] for v in runtime['resources']} and t['finished_at'] for t in saved),'original partial physical terminal proof changed')
        marker['stage']='stop_dispatched';lc.save(infra,path,marker)
        # Capture terminal CRI-equivalent Docker facts before the SDK driver
        # removes auxiliary containers. SDK metadata absence is never stop proof.
        terminal=[]
        for own in runtime['resources']:
            prior=next((t for t in saved if t['container_id']==own['container_id']),None)
            if prior is not None:
                if lc.docker(infra,'ps','-aq','--no-trunc','--filter','id='+own['container_id']).strip():
                    a=lc.docker_inspect(infra,own['container_id']);lc.require(a['Config']['Labels']==own['labels'] and a['Image']==own['image_id'] and not a['State']['Running'] and a['State']['Pid']==0 and a['State']['FinishedAt']==prior['finished_at'],'original terminal SDK resource changed')
                terminal.append(prior);continue
            a=lc.docker_inspect(infra,own['container_id']);lc.require(a['Config']['Labels']==own['labels'] and a['Image']==own['image_id'],'original SDK stop resource changed')
            if a['State']['Running']:lc.authorize(infra,plan);lc.docker(infra,'stop','--time','30',own['container_id'])
            a=lc.docker_inspect(infra,own['container_id']);lc.require(not a['State']['Running'] and a['State']['Pid']==0 and a['State']['FinishedAt'] and not a['State']['FinishedAt'].startswith('0001') and a['Config']['Labels']==own['labels'],'actual original OpenShell physical termination unproven');terminal.append({'container_id':own['container_id'],'finished_at':a['State']['FinishedAt'],'exit_code':a['State']['ExitCode']})
            marker.update(stage='physical_stop_pending',terminal=terminal);lc.save(infra,path,marker)
        marker.update(stage='terminated',terminal=terminal);lc.save(infra,path,marker)
        if get(infra,c,runtime)['status']['phase']!='SANDBOX_PHASE_STOPPED':lc.authorize(infra,plan);rpc(infra,c,'StopSandbox',{'workspaceScope':{'workspace':c['workspace']},'name':runtime['sandbox_name'],'requestId':str(uuid.uuid5(uuid.UUID(plan['id']),'openshell-stop'))})
        deadline=time.monotonic()+120
        while get(infra,c,runtime)['status']['phase']!='SANDBOX_PHASE_STOPPED':lc.require(time.monotonic()<deadline,'OpenShell stopped scope barrier unproven');time.sleep(1)
        runtime.update(stopped=True,physical_termination=terminal);marker.update(stage='scope_closed',runtime=runtime);lc.save(infra,path,marker)
        lc.authorize(infra,plan);rpc(infra,c,'DeleteSandbox',{'workspaceScope':{'workspace':c['workspace']},'name':runtime['sandbox_name'],'requestId':str(uuid.uuid5(uuid.UUID(plan['id']),'openshell-delete'))});deadline=time.monotonic()+120
        while any(lc.docker(infra,'ps','-aq','--no-trunc','--filter','id='+own['container_id']).strip() for own in runtime['resources']):lc.require(time.monotonic()<deadline,'original OpenShell resource deletion unproven');time.sleep(1)
        listing=rpc(infra,c,'ListSandboxes',{'workspaceScope':{'workspace':c['workspace']},'pageSize':1000});lc.require(not listing.get('nextPageToken') and not any(s['metadata']['id']==runtime['sandbox_id'] or s['metadata']['name']==runtime['sandbox_name'] for s in listing.get('sandboxes',[])),'original OpenShell sandbox metadata deletion unproven')
    elif stopped:
        lc.require(runtime.get('physical_termination') and all(not lc.docker(infra,'ps','-aq','--no-trunc','--filter','id='+own['container_id']).strip() for own in runtime['resources']),'original stopped OpenShell resources reappeared')
    if released:
        lc.require(lc.guest(infra,plan,'read_release' if runtime.get('storage_released') else 'release',runtime['storage'])['reference_released'],'original OpenShell workspace reference release unproven');runtime['storage_released']=True
    receipt={'format':'cowork-environment-completion-v1','job_id':job['id'],'plan_sha256':job['plan_sha256'],'challenge':plan['challenge'],'action':plan['action'],'backend_profile_sha256':plan['backend_profile']['specification_sha256'],'runtime':runtime,'physical_stop_verified':stopped,'original_runtime_absent':stopped,'storage_release_verified':released,'recovered_stopped':action!=plan['action']}
    marker.update(stage='completed',runtime=runtime,receipt=receipt);lc.save(infra,path,marker);return receipt

def capacity(infra,container,instance):
    sid=container['Config']['Labels'].get('openshell.ai/sandbox-id');directory=infra.root/lc.identifier(instance)/'environments';found=[]
    for p in directory.glob('*.json'):
        value=lc.read(p);runtime=value.get('runtime',{})
        if isinstance(runtime,dict) and runtime.get('sandbox_id')==sid and runtime.get('backend')=='open_shell' and value.get('stage') in ('created','completed','physical_stop_pending','terminated','scope_closed') and p.stem==runtime['labels'].get('cowork.environment-create','').replace('-','') and any(v['container_id']==container['Id'] for v in runtime.get('resources',[])):found.append(runtime)
    lc.require(len(found)==1,'OpenShell resource lacks its unique original create ACK');runtime=found[0];own=next(v for v in runtime['resources'] if v['container_id']==container['Id']);caps=container['HostConfig'];lc.require(container['Config']['Labels']==own['labels'] and container['Image']==own['image_id'] and caps['NanoCpus']==own['cpu_millis']*1000000 and caps['Memory']==own['memory_mib']*1048576 and caps['MemorySwap']==caps['Memory'] and caps['PidsLimit']==own['pids'] and caps['Privileged'] is False and caps['CapDrop']==['ALL'] and not caps.get('CapAdd') and 'no-new-privileges:true' in caps.get('SecurityOpt',[]),'OpenShell original capacity changed')
    if own['role']=='sandbox':
        admitted=runtime.get('policy_admission',{});lc.require(admitted.get('admitted') is True and admitted['configuration_instance_id']==runtime['runtime_generation'] and admitted['policy_sha256']==lc.sha(normalized(policy())) and caps['NetworkMode']=='none','original OpenShell hard Landlock policy/generation missing')
    else:lc.require(caps['ReadonlyRootfs'] is True,'original SDK auxiliary root filesystem must be read-only')
    return own['role']=='sandbox'
