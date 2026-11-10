"""Explicit original-controller VM power plans with physical stop readbacks."""
import base64,json,os,time
from pathlib import Path
import external_vm as vm,environment_lifecycle as lc
FINALIZER='open-cowork.io/environment-vm-physical-stop'
def inventory(infra,c):
    if c['backend']!='kubevirt':return None
    raw=vm.kubectl(infra,c,'--namespace',c['namespace'],'get','vmi',c['vm_name'],'--ignore-not-found','-o','json')
    if not raw.strip():return None
    vmi=json.loads(raw);pods=json.loads(vm.kubectl(infra,c,'--namespace',c['namespace'],'get','pods','-o','json'))['items']
    own=[p for p in pods if any(o['kind']=='VirtualMachineInstance' and o['uid']==vmi['metadata']['uid'] for o in p['metadata'].get('ownerReferences',[]))]
    lc.require(len(own)==1 and all(p['spec']['nodeName']==c['node_name'] and p['status']['phase']=='Running' and p['status'].get('containerStatuses') and all(s.get('containerID') and s['state'].get('running') for s in p['status']['containerStatuses']) for p in own),'one actual original nonmigrating KubeVirt launcher runtime required')
    return {'vmi_uid':vmi['metadata']['uid'],'pods':[{'name':p['metadata']['name'],'uid':p['metadata']['uid'],'resource_version':p['metadata']['resourceVersion'],'node':p['spec']['nodeName'],'container_ids':[s['containerID'] for s in p['status']['containerStatuses']]} for p in own]}
def power(infra,c,action,original,save,authorize):
    lc.require(c['backend']!='open_shell' and action in ('start','stop'),'actual VM power action required')
    actual=vm.domain(infra,c);lc.require(actual==original['physical'] and (actual['running'] if action=='stop' else actual['powered_off']),'original power state/UID CAS changed')
    authorize();b=c['backend'];v=c['vm_id'];terminal=None
    if b=='kubevirt':
        if action=='stop':
            lc.require(inventory(infra,c)==original['runtime_inventory'],'original launcher UID/CRI inventory changed')
            for p in original['runtime_inventory']['pods']:
                current=json.loads(vm.kubectl(infra,c,'--namespace',c['namespace'],'get','pod',p['name'],'-o','json'));f=current['metadata'].get('finalizers',[])
                patch=[{'op':'test','path':'/metadata/uid','value':p['uid']},{'op':'test','path':'/metadata/resourceVersion','value':p['resource_version']},{'op':'add','path':'/metadata/finalizers','value':[*f,FINALIZER]}]
                vm.kubectl(infra,c,'--namespace',c['namespace'],'patch','pod',p['name'],'--type=json','-p',json.dumps(patch))
        current=json.loads(vm.kubectl(infra,c,'--namespace',c['namespace'],'get','vm',c['vm_name'],'-o','json'));lc.require(current['metadata']['uid']==v and current['metadata']['resourceVersion']==original['physical']['identity']['resource_version'] and 'running' not in current['spec'] and current['spec'].get('runStrategy') in ('Always','Halted'),'original KubeVirt VM strategy/RV changed')
        save('power_dispatched');authorize();patch=[{'op':'test','path':'/metadata/uid','value':v},{'op':'test','path':'/metadata/resourceVersion','value':current['metadata']['resourceVersion']},{'op':'replace','path':'/spec/runStrategy','value':'Always' if action=='start' else 'Halted'}]
        vm.kubectl(infra,c,'--namespace',c['namespace'],'patch','vm',c['vm_name'],'--type=json','-p',json.dumps(patch))
        if action=='stop':
            terminal=[]
            for p in original['runtime_inventory']['pods']:
                deadline=time.monotonic()+180
                while True:
                    current=json.loads(vm.kubectl(infra,c,'--namespace',c['namespace'],'get','pod',p['name'],'-o','json'));statuses=current['status'].get('containerStatuses',[])
                    lc.require(current['metadata']['uid']==p['uid'] and current['spec']['nodeName']==p['node'],'original physical launcher identity changed')
                    if statuses and all(s['state'].get('terminated',{}).get('finishedAt') for s in statuses):break
                    lc.require(time.monotonic()<deadline,'original physical launcher stop unproven');time.sleep(1)
                lc.require({s['containerID'] for s in statuses}==set(p['container_ids']),'original launcher CRI terminal identities changed');terminal.append({'pod_uid':p['uid'],'containers':statuses});save('physical_launcher_stopped',terminal)
                f=current['metadata']['finalizers'];authorize();patch=[{'op':'test','path':'/metadata/uid','value':p['uid']},{'op':'test','path':'/metadata/resourceVersion','value':current['metadata']['resourceVersion']},{'op':'test','path':'/metadata/finalizers','value':f},{'op':'replace','path':'/metadata/finalizers','value':[x for x in f if x!=FINALIZER]}]
                vm.kubectl(infra,c,'--namespace',c['namespace'],'patch','pod',p['name'],'--type=json','-p',json.dumps(patch))
                deadline=time.monotonic()+60
                while vm.kubectl(infra,c,'--namespace',c['namespace'],'get','pod',p['name'],'--ignore-not-found','-o','json').strip():lc.require(time.monotonic()<deadline,'original stopped launcher retained');time.sleep(1)
    else:
        save('power_dispatched');authorize()
        if b=='libvirt':infra.run(['virsh','--connect',c['libvirt_uri'],'start' if action=='start' else 'shutdown',v])
        elif b=='hyper_v':
            script="$ErrorActionPreference='Stop'; $v=Get-VM -Id '"+v+"'; "+('Start-VM -VM $v' if action=='start' else 'Stop-VM -VM $v')
            infra.run([*vm.ssh(c,'controller'),'powershell.exe -NoProfile -NonInteractive -EncodedCommand '+base64.b64encode(script.encode('utf-16le')).decode()],timeout=60)
        else:
            from pathlib import Path
            m=vm.qemu(c);root=Path(c['vm_root']);m.start(root) if action=='start' else m.stop(root)
            if action=='stop':
                parent,entry,entries=m.retirement_tree(root);m.retirement_quiet({'allocation':{'identity':m.capacity_pool.snapshot(root,m.load(root))},'quarantine':str(root.parent/('.power-unused-'+v.replace('-',''))),'root_entry':entry,'entries':entries,'parent':parent})
    deadline=time.monotonic()+180
    while True:
        actual=vm.domain(infra,c)
        if actual['running'] if action=='start' else actual['powered_off']:break
        lc.require(time.monotonic()<deadline,'actual original VM power readback timed out');time.sleep(1)
    if action=='start':vm.observe(infra,c,allow_power_pending=True)
    lc.require(action!='stop' or b!='kubevirt' or actual['identity']['vmi_uid'] is None,'original stopped KubeVirt VMI remains')
    return {'format':'cowork-environment-vm-power-v1','controller_id':c['controller_id'],'vm_id':v,'action':action,'original_physical_identity':original['physical']['identity'],'actual_physical_identity':actual['identity'],'actual_running':actual['running'],'physical_launcher_termination':terminal,'physical_stop_verified':action=='stop'}

def native_stop_receipt(infra,c,record,authorize=None):
    terminal=record.get('terminal') or record.get('receipt',{}).get('vm_power',{}).get('physical_launcher_termination')
    own=record.get('runtime_inventory');lc.require(terminal and own and {p['uid'] for p in own['pods']}=={t['pod_uid'] for t in terminal},'original positive launcher terminal receipt required')
    for p in own['pods']:
        t=next(t for t in terminal if t['pod_uid']==p['uid']);lc.require({s['containerID'] for s in t['containers']}==set(p['container_ids']) and all(s['state'].get('terminated',{}).get('finishedAt') for s in t['containers']),'original physical CRI terminal differs')
        raw=vm.kubectl(infra,c,'--namespace',c['namespace'],'get','pod',p['name'],'--ignore-not-found','-o','json')
        if raw.strip():
            current=json.loads(raw);statuses=current['status'].get('containerStatuses',[]);lc.require(authorize is not None and current['metadata']['uid']==p['uid'] and current['spec']['nodeName']==p['node'] and statuses==t['containers'] and FINALIZER in current['metadata'].get('finalizers',[]),'original terminal launcher cleanup authority differs')
            authorize();f=current['metadata']['finalizers'];patch=[{'op':'test','path':'/metadata/uid','value':p['uid']},{'op':'test','path':'/metadata/resourceVersion','value':current['metadata']['resourceVersion']},{'op':'test','path':'/metadata/finalizers','value':f},{'op':'replace','path':'/metadata/finalizers','value':[x for x in f if x!=FINALIZER]}];vm.kubectl(infra,c,'--namespace',c['namespace'],'patch','pod',p['name'],'--type=json','-p',json.dumps(patch));deadline=time.monotonic()+60
            while vm.kubectl(infra,c,'--namespace',c['namespace'],'get','pod',p['name'],'--ignore-not-found','-o','json').strip():lc.require(time.monotonic()<deadline,'original terminal launcher cleanup unproven');time.sleep(1)
    lc.require(vm.domain(infra,c)['powered_off'],'original halted KubeVirt VM changed');return terminal

def execute_job(infra,job):
    plan,directory,path=lc.scope(infra,job);c=vm.selected(infra,plan,require_running=False);registry=Path(c['power_state_directory']);lc.require(registry.is_absolute() and registry.is_dir() and not registry.is_symlink(),'original controller power registry required');shadow=registry/(lc.identifier(job['id'])+'.json')
    lc.require(all(not p.is_symlink() for p in registry.parents) and (os.name!='posix' or registry.stat().st_uid==os.getuid() and not registry.stat().st_mode&0o077),'owner-only original power registry required')
    if path.exists():
        marker=lc.read(path);lc.require(marker['plan_sha256']==job['plan_sha256'] and marker['lease_token']==job['lease_token'] and marker['controller_id']==c['controller_id'],'original VM power journal differs')
        if marker.get('receipt') is not None:return marker['receipt']
        lc.require(job.get('recovery_authorized') is True,'unknown VM power IO held; no automatic start replay')
        recovery=True
    else:
        vm.no_uncertain_power(c);marker={'format':1,'job_id':job['id'],'controller_id':c['controller_id'],'vm_id':c['vm_id'],'lease_token':job['lease_token'],'plan_sha256':job['plan_sha256'],'stage':'prepared'};recovery=False
    def save(stage,terminal=None):
        marker['stage']=stage
        if terminal is not None:marker['terminal']=terminal
        lc.save(infra,path,marker);lc.save(infra,shadow,marker)
    authorize=lambda:lc.authorize(infra,plan)
    if recovery and c['backend']=='kubevirt' and marker.get('terminal'):
        native_stop_receipt(infra,c,marker,authorize)
    authorize();actual=vm.domain(infra,c);action='stop' if recovery else plan['action'].removeprefix('power_')
    if action=='start' and actual['running']:
        vm.observe(infra,c,allow_power_pending=True);receipt={'format':'cowork-environment-vm-power-v1','controller_id':c['controller_id'],'vm_id':c['vm_id'],'action':action,'actual_running':True,'actual_physical_identity':actual['identity'],'physical_stop_verified':False}
    elif action=='stop' and actual['powered_off']:
        terminal=native_stop_receipt(infra,c,marker) if c['backend']=='kubevirt' else None
        receipt={'format':'cowork-environment-vm-power-v1','controller_id':c['controller_id'],'vm_id':c['vm_id'],'action':action,'actual_running':False,'actual_physical_identity':actual['identity'],'physical_stop_verified':True,'physical_launcher_termination':terminal}
    else:
        lc.require(actual['running'] if action=='stop' else actual['powered_off'],'original physical VM state remains uncertain')
        if action=='start' and c['backend']=='kubevirt':
            prior=[lc.read(p) for p in registry.glob('*.json') if p!=shadow];valid=[p for p in prior if p.get('stage')=='completed' and p.get('vm_id')==c['vm_id'] and p.get('receipt',{}).get('vm_power',{}).get('physical_stop_verified') is True]
            lc.require(valid,'native VM start requires an original positive previous stop, not VMI/Node absence');native_stop_receipt(infra,c,valid[-1])
        marker['physical']=actual;marker['runtime_inventory']=inventory(infra,c) if c['backend']=='kubevirt' and action=='stop' else None;save('prepared' if not recovery else 'recovery_prepared')
        receipt=power(infra,c,action,marker,save,authorize)
    authorize();outer={'format':'cowork-environment-completion-v1','job_id':job['id'],'plan_sha256':job['plan_sha256'],'challenge':plan['challenge'],'action':plan['action'],'backend_profile_sha256':plan['backend_profile']['specification_sha256'],'runtime':None,'vm_power':receipt,'physical_stop_verified':action=='stop','original_runtime_absent':action=='stop','storage_release_verified':False,'recovered_stopped':recovery}
    marker['receipt']=outer;save('completed');return outer
