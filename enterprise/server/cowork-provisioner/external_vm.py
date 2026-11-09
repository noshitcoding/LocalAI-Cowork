"""Original Libvirt/KubeVirt/Hyper-V/attached VM controller adapters.

Only explicitly enrolled, owner-marked VMs are accessible. The controller never
adopts a VM by name, never touches baseline/protected IDs and never provisions V2.
"""
import hashlib,importlib.util,json,os,re,shlex,sys,time,uuid,xml.etree.ElementTree as ET
from pathlib import Path
import environment_lifecycle as lifecycle

def private(path):
    path=Path(path);lifecycle.require(path.is_absolute() and path.is_file() and not path.is_symlink() and all(not p.is_symlink() for p in path.parents),'private enrolled controller file required')
    if os.name=='posix':lifecycle.require(path.stat().st_uid==os.getuid() and not path.stat().st_mode&0o077,'owner-only controller configuration required')
    return path
def digest(path):return hashlib.sha256(private(path).read_bytes()).hexdigest()
def configuration(path,expected=None):
    path=private(path);raw=path.read_bytes();lifecycle.require(len(raw)<=1048576 and (expected is None or hashlib.sha256(raw).hexdigest()==expected),'original controller configuration changed')
    c=json.loads(raw);lifecycle.require(c['format']==1 and c['backend'] in ('libvirt','kubevirt','hyper_v','attached','open_shell'),'explicit supported controller required')
    for key in ('organisation_id','instance_id','host_id','operator_state_id','controller_id','vm_id','guest_hardware_uuid'):lifecycle.identifier(c[key])
    lifecycle.require(isinstance(c['protected_vm_ids'],list) and c['vm_id'] not in c['protected_vm_ids'],'protected VM is excluded from environment controllers')
    for filename,expected_digest in c['credential_pins'].items():lifecycle.require(re.fullmatch('[a-f0-9]{64}',expected_digest) and digest(filename)==expected_digest,'original controller credential/host identity changed')
    lifecycle.require(c['guest']['known_hosts'] in c['credential_pins'] and c['guest']['ssh_key'] in c['credential_pins'],'separately pinned original SSH host identity required')
    return c,hashlib.sha256(raw).hexdigest()
def ssh(c,host='guest'):
    h=c[host];lifecycle.require(re.fullmatch('[A-Za-z0-9._:-]{1,253}',h['hostname']) and type(h['port']) is int and 1<=h['port']<=65535 and re.fullmatch('[A-Za-z][A-Za-z0-9_-]{0,31}',h['username']),'exact enrolled SSH endpoint required')
    private(h['ssh_key']);private(h['known_hosts'])
    return ['ssh','-i',h['ssh_key'],'-p',str(h['port']),'-o','BatchMode=yes','-o','IdentitiesOnly=yes','-o','StrictHostKeyChecking=yes','-o','UserKnownHostsFile='+h['known_hosts'],'-o','ConnectTimeout=5',h['username']+'@'+h['hostname']]
def guest_command(infra,c,argv,**kwargs):return infra.run([*ssh(c),'sudo -n '+shlex.join(argv)],**kwargs)
def owner(c):return {k:c[k] for k in ('organisation_id','instance_id','operator_state_id','controller_id','vm_id')}
def qemu(c):
    path=private(c['qemu_controller_file']);lifecycle.require(digest(path)==c['qemu_controller_sha256'] and path.name=='local_executor_vm.py','reviewed original QEMU controller changed')
    spec=importlib.util.spec_from_file_location('_cowork_attached_environment_vm',path);m=importlib.util.module_from_spec(spec);sys.modules[spec.name]=m;spec.loader.exec_module(m);return m
def kubectl(infra,c,*args,data=None):
    private(c['kubeconfig_file']);lifecycle.require(c['kubeconfig_file'] in c['credential_pins'] and c['kubernetes_context'],'original KubeVirt credentials/context required')
    env=dict(os.environ);env.pop('KUBECONFIG',None);env.pop('KUBERNETES_MASTER',None)
    return infra.run(['kubectl','--kubeconfig',c['kubeconfig_file'],'--context',c['kubernetes_context'],'--request-timeout=20s',*args],data=data,env=env,timeout=60)
def domain(infra,c):
    b=c['backend'];v=c['vm_id']
    if b=='libvirt':
        lifecycle.require(c['libvirt_uri']=='qemu:///system','explicit local system Libvirt controller required')
        machine=Path('/etc/machine-id').read_text().strip();lifecycle.require(machine==c['controller_identity'],'original Libvirt physical host changed')
        argv=['virsh','--connect',c['libvirt_uri']];lifecycle.require(infra.run([*argv,'domuuid',v]).decode().strip()==v,'original Libvirt domain UUID changed')
        xml=ET.fromstring(infra.run([*argv,'dumpxml',v]));metadata=xml.find('./metadata/{https://open-cowork.org/enterprise/v1}owner');lifecycle.require(xml.findtext('uuid')==v and metadata is not None and json.loads(metadata.text)==owner(c),'original Libvirt owner metadata differs')
        memory=xml.find('memory');current=xml.find('currentMemory');cpu=xml.find('cputune');lifecycle.require(memory is not None and memory.attrib.get('unit')=='KiB' and current is not None and current.text==memory.text and current.attrib.get('unit')=='KiB' and cpu is not None,'fixed Libvirt memory and CPU quota required')
        quota=int(cpu.findtext('quota','-1'));period=int(cpu.findtext('period','0'));lifecycle.require(quota>0 and period>0 and not xml.findall('./devices/hostdev'),'bounded Libvirt CPU without host passthrough required')
        disks=xml.findall('./devices/disk[@device="disk"]');lifecycle.require(len(disks)==1 and disks[0].find('source') is not None,'one original bounded VM root disk required')
        source=disks[0].find('source').attrib['file'];private(source);info=json.loads(infra.run(['qemu-img','info','--output=json',source]));lifecycle.require(not info.get('backing-filename') and source==c['disk_file'],'original standalone Libvirt disk changed')
        limits={'cpu_millis':quota*1000//period,'memory_mib':int(memory.text)//1024,'storage_gib':info['virtual-size']//1073741824}
        state=infra.run([*argv,'domstate',v]).decode().strip();running=state=='running';off=state=='shut off';identity={'domain_uuid':v,'xml_sha256':hashlib.sha256(ET.tostring(xml)).hexdigest()}
    elif b=='kubevirt':
        system=json.loads(kubectl(infra,c,'get','namespace','kube-system','-o','json'));ns=json.loads(kubectl(infra,c,'get','namespace',c['namespace'],'-o','json'));vm=json.loads(kubectl(infra,c,'--namespace',c['namespace'],'get','vm',c['vm_name'],'-o','json'))
        lifecycle.require(system['metadata']['uid']==c['controller_identity'] and ns['metadata']['uid']==c['namespace_uid'] and vm['metadata']['uid']==v and vm['metadata']['annotations']['open-cowork.io/environment-owner']==json.dumps(owner(c),sort_keys=True,separators=(',',':')),'original KubeVirt cluster/namespace/VM owner changed')
        spec=vm['spec']['template']['spec'];d=spec['domain'];lifecycle.require(d['firmware']['uuid']==c['guest_hardware_uuid'] and d.get('resources',{}).get('requests')==d['resources']['limits'] and not d.get('devices',{}).get('hostDevices'),'fixed KubeVirt VM CPU/memory without host passthrough required')
        resources=d['resources']['limits'];cpu=resources['cpu'];memory=resources['memory'];lifecycle.require(re.fullmatch('[0-9]+m',cpu) and re.fullmatch('[0-9]+Mi',memory),'explicit measured KubeVirt millicpu/Mi limits required')
        claims=[x['persistentVolumeClaim']['claimName'] for x in spec['volumes'] if 'persistentVolumeClaim' in x];lifecycle.require(claims==[c['disk_claim']],'original KubeVirt disk claim changed')
        pvc=json.loads(kubectl(infra,c,'--namespace',c['namespace'],'get','pvc',c['disk_claim'],'-o','json'));lifecycle.require(pvc['metadata']['uid']==c['disk_claim_uid'] and pvc['status']['phase']=='Bound' and re.fullmatch('[0-9]+Gi',pvc['spec']['resources']['requests']['storage']),'original bounded KubeVirt root PVC changed')
        vmi_raw=kubectl(infra,c,'--namespace',c['namespace'],'get','vmi',c['vm_name'],'--ignore-not-found','-o','json');vmi=json.loads(vmi_raw) if vmi_raw.strip() else None
        node=json.loads(kubectl(infra,c,'get','node',c['node_name'],'-o','json'));lifecycle.require(node['metadata']['uid']==c['node_uid'] and spec['nodeSelector']=={'kubernetes.io/hostname':c['node_name']},'original pinned KubeVirt physical node changed')
        migrations=json.loads(kubectl(infra,c,'--namespace',c['namespace'],'get','virtualmachineinstancemigrations','-o','json'))['items'];lifecycle.require(not any(x['spec']['vmiName']==c['vm_name'] and x.get('status',{}).get('phase') not in ('Succeeded','Failed') for x in migrations),'original VM migration in progress; physical ownership uncertain')
        lifecycle.require(vmi is None or vmi['status'].get('nodeName')==c['node_name'] and not vmi['status'].get('migrationState',{}).get('targetNode'),'original KubeVirt VMI moved or is migrating')
        running=vmi is not None and vmi['status']['phase']=='Running';off=vmi is None and vm['spec'].get('runStrategy')=='Halted';lifecycle.require(vmi is None or any(o['uid']==v and o['kind']=='VirtualMachine' for o in vmi['metadata']['ownerReferences']),'original KubeVirt VMI owner differs')
        limits={'cpu_millis':int(cpu[:-1]),'memory_mib':int(memory[:-2]),'storage_gib':int(pvc['spec']['resources']['requests']['storage'][:-2])};identity={'vm_uid':v,'resource_version':vm['metadata']['resourceVersion'],'vmi_uid':None if vmi is None else vmi['metadata']['uid'],'namespace_uid':c['namespace_uid'],'node_name':c['node_name'],'node_uid':c['node_uid']}
    elif b=='hyper_v':
        import base64
        lifecycle.require(c['controller']['known_hosts'] in c['credential_pins'] and c['controller']['ssh_key'] in c['credential_pins'],'original Hyper-V parent SSH enrollment required')
        script="$ErrorActionPreference='Stop'; $v=Get-VM -Id '"+v+"'; $p=Get-VMProcessor -VM $v; $m=Get-VMMemory -VM $v; $d=@(Get-VMHardDiskDrive -VM $v); if($d.Count -ne 1 -or $m.DynamicMemoryEnabled){throw 'fixed VM allocation required'}; $disk=Get-VHD -Path $d[0].Path; [ordered]@{id=$v.Id.ToString().ToLower();host=$env:COMPUTERNAME;notes=$v.Notes;state=$v.State.ToString();cpu_millis=[int]($p.Count*$p.Maximum*10);memory_mib=[int]($m.Startup/1MB);storage_gib=[int]($disk.Size/1GB);disk_id=$disk.DiskIdentifier.ToString().ToLower();disk_path=$disk.Path;parent=$disk.ParentPath}|ConvertTo-Json -Compress"
        raw=infra.run([*ssh(c,'controller'),'powershell.exe -NoProfile -NonInteractive -EncodedCommand '+base64.b64encode(script.encode('utf-16le')).decode()],timeout=60);value=json.loads(raw)
        lifecycle.require(value['id']==v and value['host']==c['controller_identity'] and json.loads(value['notes'])==owner(c) and value['disk_id']==c['disk_id'] and value['disk_path']==c['disk_file'] and not value['parent'],'original Hyper-V VM/parent/owner/disk changed')
        running=value['state']=='Running';off=value['state']=='Off';limits={k:value[k] for k in ('cpu_millis','memory_mib','storage_gib')};identity={'vm_id':v,'disk_id':value['disk_id'],'parent_host':value['host']}
    elif b=='attached':
        m=qemu(c);root=Path(c['vm_root']);value=m.load(root)
        lifecycle.require(value['version']==2 and value['id']==v and value['organisation_id']==c['organisation_id'] and value['operator_state_id']==c['operator_state_id'],'attached original owned QEMU allocation changed')
        with m.capacity_pool.locked_pool() as pool:
            pool.require_registered(root,value);pool.check_inventory(m.load);allocation=pool.data['allocations'][v];lifecycle.require(not allocation['adopted'] and str(root) not in pool.data['plan']['existing_vm_roots'],'protected/adopted attached VM forbidden')
            identity={'vm_id':v,'allocation_sha256':lifecycle.sha(allocation),'capacity_revision':pool.data['revision']}
        lifecycle.require(Path('/etc/machine-id').read_text().strip()==c['controller_identity'],'attached VM physical parent changed')
        running=m.process(root) is not None
        if running:lifecycle.require(m.qmp(root,'query-kvm')['enabled'] and m.qmp(root,'query-status')['status']=='running','actual attached KVM runtime not running')
        else:
            parent,entry,entries=m.retirement_tree(root);m.retirement_quiet({'allocation':{'identity':m.capacity_pool.snapshot(root,value)},'quarantine':str(root.parent/('.power-unused-'+v.replace('-',''))),'root_entry':entry,'entries':entries,'parent':parent})
        off=not running
        limits={'cpu_millis':value['cpus']*1000,'memory_mib':value['memory_mib'],'storage_gib':value['disk_virtual_gib']}
    else:raise RuntimeError('OpenShell uses its dedicated gateway adapter')
    lifecycle.require(limits==c['limits'] and all(type(v) is int and v>0 for v in limits.values()),'actual VM hard allocation differs from registered controller')
    return {'running':running,'powered_off':off,'limits':limits,'identity':identity}
def no_uncertain_power(c):
    root=Path(c['power_state_directory']);lifecycle.require(root.is_absolute() and root.is_dir() and not root.is_symlink() and all(not p.is_symlink() for p in root.parents),'original private controller power journal required')
    if os.name=='posix':lifecycle.require(root.stat().st_uid==os.getuid() and not root.stat().st_mode&0o077,'owner-only original power journal required')
    for p in root.glob('*.json'):
        value=json.loads(private(p).read_bytes());lifecycle.require(value['controller_id']==c['controller_id'] and value.get('stage')=='completed' and value.get('receipt') is not None,'original VM power IO uncertain; environment dispatch held')
def observe(infra,c,allow_power_pending=False):
    if not allow_power_pending:no_uncertain_power(c)
    physical=domain(infra,c);lifecycle.require(physical['running'],'original enrolled VM must physically run')
    lifecycle.require(guest_command(infra,c,['stat','-c','%u:%a:%h','/etc/cowork/environment-owner.json']).decode().strip()=='0:600:1','guest owner enrollment must be root-owned, private and unlinked')
    value=json.loads(guest_command(infra,c,['cat','/etc/cowork/environment-owner.json']));lifecycle.require(value==owner(c),'original guest owner enrollment differs')
    hardware=guest_command(infra,c,['cat','/sys/class/dmi/id/product_uuid']).decode().strip().lower();lifecycle.require(hardware==c['guest_hardware_uuid'],'original guest hardware UUID differs')
    daemon=json.loads(guest_command(infra,c,['docker','info','--format','{{json .}}']));lifecycle.require(daemon['ID']==c['guest_daemon_id'] and daemon['OSType']=='linux' and daemon['CgroupVersion']=='2' and any('apparmor' in s for s in daemon['SecurityOptions']),'actual enrolled guest Docker identity/cgroup/AppArmor differs')
    physical['guest_daemon_id']=daemon['ID'];physical['guest_hardware_uuid']=hardware;return physical
def selected(infra,plan,require_running=True):
    controller=plan.get('environment_controller');lifecycle.require(controller is not None and controller['backend']!='open_shell','original VM controller required')
    entry=infra.config['environment_controllers'][controller['backend']];c,sha=configuration(entry['configuration_file'],entry['configuration_sha256']);spec=controller['specification']
    lifecycle.require(sha==spec['configuration_sha256'] and c['controller_id']==str(controller['id']) and all(c[key]==spec[key] for key in ('backend','organisation_id','instance_id','host_id','operator_state_id','vm_id','guest_hardware_uuid','guest_daemon_id','controller_identity','limits')),'original registered VM controller/configuration differs')
    if require_running:observe(infra,c)
    else:domain(infra,c)
    return c
class Infrastructure:
    def __init__(self,infra,plan):self.original=infra;self.plan=plan;self.config=infra.config;self.root=infra.root
    def __getattr__(self,key):return getattr(self.original,key)
    def run(self,argv,**kwargs):
        if argv and argv[0]=='docker':return guest_command(self.original,selected(self.original,self.plan),argv,**kwargs)
        return self.original.run(argv,**kwargs)
def storage(infra,plan,action,storage=None,container_id=None):
    original=infra.original if isinstance(infra,Infrastructure) else infra;c=selected(original,plan);cfg=original.config['environment_lifecycle'];helper=cfg['guest_storage_helper']
    lifecycle.require(guest_command(original,c,['sha256sum','--',helper]).decode().split()[0]==cfg['guest_storage_helper_sha256'],'reviewed external guest quota helper changed')
    request={'format':1,'job_id':str(uuid.uuid5(uuid.UUID(plan['id']),'storage-'+action)),'environment_id':lifecycle.environment_id(plan),'instance_id':plan['instance_id'],'organisation_id':plan['organisation_id'],'vm_id':c['guest_hardware_uuid'],'daemon_id':c['guest_daemon_id'],'action':action,'storage_gib':lifecycle.limits(plan)['storage_gib'],'shared_workspace':lifecycle.spec(plan)['shared_workspace'],'expected_storage_uid':None if storage is None else storage['storage_uid']}
    if container_id is not None:request['container_id']=container_id
    result=json.loads(guest_command(original,c,['python3',helper],data=lifecycle.canonical(request),timeout=180));lifecycle.require(result['job_id']==request['job_id'] and result['quota_bytes']<=request['storage_gib']*1073741824 and (storage is None or result['storage_uid']==storage['storage_uid']),'original external guest storage quota readback differs');return result
