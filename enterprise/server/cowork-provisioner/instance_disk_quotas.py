"""Explicit total physical disk profile; no shared-service/legacy adoption.

The operator installs the original hard filesystems and dedicated data services
before qualification. Observations use the pinned original VM SSH identity and
local root helper. Renewal never allocates, recreates, frees or adopts a disk.
"""
import copy,hashlib,json,os,re,shlex,threading,time,uuid
from pathlib import Path
import backend_profile,executor_host,resource_budget

def require(ok,message):
    if not ok:raise RuntimeError(message)
def canonical(v):return json.dumps(v,sort_keys=True,separators=(',',':'),ensure_ascii=False).encode()
def digest(v):return hashlib.sha256(canonical(v)).hexdigest()
def private(p):
    p=Path(p);require(p.is_absolute() and p.is_file() and not p.is_symlink() and all(not x.is_symlink() for x in p.parents)
        and p.stat().st_nlink==1 and p.stat().st_size<=2097152,'original private disk file required')
    if os.name=='posix':require(p.stat().st_uid==os.geteuid() and not p.stat().st_mode&0o077,'private original disk file required')
    return p
def read(p):return json.loads(private(p).read_bytes())
def helper_source(p):
    p=Path(p);s=p.lstat();require(p.is_absolute() and p.is_file() and not p.is_symlink() and all(not x.is_symlink() for x in p.parents)
        and s.st_nlink==1 and s.st_size<=1048576 and (os.name!='posix' or s.st_uid in (0,os.geteuid()) and not s.st_mode&0o022),'immutable root/operator helper source required')
    return p
def settings(config):
    c=config.get('instance_disk_quotas')
    if c is None:return None
    require(type(c) is dict and set(c)=={'version','profile_file','guest_helper','controller_helper','controller_request_file',
        'guest_request_file','postgres_image','minio_image','postgres_root_password_file','minio_root_user_file','minio_root_password_file'}
        and c['version']==1,'invalid total disk operator configuration')
    p=read(c['profile_file']);spec=p['specification'];b=backend_profile.settings(config)
    require(b and b['specification']['kind']=='vm_compose' and spec['format']=='cowork-total-disk-specification-v1'
        and spec['instance_id']==b['specification']['instance_id'] and spec['executor_host_binding']==executor_host.manager_binding(config)
        and spec['backend_specification_sha256']==digest(b['specification']),'original physical quota backend differs')
    require(p['state'] in ('active','disabled') and p['revision']>0 and p['specification_sha256']==digest(spec),'committed original disk profile required')
    for field,expected in [('controller_helper',spec['controller_helper_sha256']),('guest_helper',spec['guest_helper_sha256'])]:
        path=Path(c[field]);require(path.is_absolute() and re.fullmatch('/[a-zA-Z0-9_./-]+',str(path)) and '..' not in path.parts,'fixed installed disk helper path required')
        if field=='controller_helper':require(hashlib.sha256(helper_source(path).read_bytes()).hexdigest()==expected,'original controller helper source changed')
    for name in ['postgres_image','minio_image']:require(re.fullmatch('[a-zA-Z0-9][a-zA-Z0-9._:/-]*@sha256:[a-f0-9]{64}',c[name]),'digest-pinned data image required')
    for key in ['postgres_root_password_file','minio_root_user_file','minio_root_password_file']:private(c[key])
    return c,p
def source_profile(infra):
    selected=settings(infra.config);return None if selected is None else selected[1]
def bind(infra):
    marker=infra.root/'total-disk-binding.json';selected=settings(infra.config)
    require(not marker.is_symlink(),'unsafe original disk binding')
    if selected is None:
        require(not marker.exists(),'physical disk qualification cannot be disabled by removing config');return
    c,p=selected;spec=p['specification'];paths=spec['topology']['controller_paths']
    require(str(infra.root)==paths['state'] and infra.config['backup_directory']==paths['backups'],
        'provisioner generations, state and backups must use original bounded placement')
    snapshot=infra.config.get('vm_snapshot')
    if snapshot:
        require(snapshot['store_directory']==paths['snapshot_store'],'snapshot store escapes total disk placement')
        row=next((v for v in spec['placements'] if v['kind']=='snapshot'),None)
        require(row and row['filesystem_uuid']==snapshot['store_filesystem_uuid'] and row['size_bytes']==snapshot['maximum_bytes'],
            'whole snapshot maximum must be reserved even when empty')
    binding={'format':'cowork-total-disk-operator-binding-v1','profile_id':p['instance_id'],'specification_sha256':p['specification_sha256'],
        'executor_host_binding':spec['executor_host_binding'],'guest_helper_sha256':spec['guest_helper_sha256'],'controller_helper_sha256':spec['controller_helper_sha256']}
    if marker.exists():require(read(marker)==binding,'original physical disk placement retarget requires explicit new installation')
    else:
        for v in infra.root.iterdir():
            if v.is_dir() and re.fullmatch('[a-f0-9]{32}',v.name):
                require(v.name==uuid.UUID(p['instance_id']).hex and {x.name for x in v.iterdir()}<={'quota-runtime-secrets'},
                    'existing unqualified data requires explicit migration; no automatic adoption')
        infra.private_write(marker,canonical(binding))
def guest_command(infra,c):
    profile=backend_profile.settings(infra.config);argv=profile['probe_command'];require('--config' in argv,'original VM controller configuration required')
    original=read(argv[argv.index('--config')+1]);root=Path(original['vm_root']);vm=read(root/'vm.json')
    require(vm['id']==profile['specification']['vm_id'] and vm['organisation_id']==profile['specification']['organisation_id'],'original VM SSH scope differs')
    ssh=['ssh','-i',str(private(root/'ssh-key')),'-p',str(vm['ssh_port']),'-o','BatchMode=yes','-o','IdentitiesOnly=yes',
        '-o','StrictHostKeyChecking=yes','-o','UserKnownHostsFile='+str(private(root/'known_hosts')),'-o','ConnectTimeout=5','cowork@127.0.0.1']
    actual=infra.run([*ssh,'sudo -n sha256sum -- '+shlex.quote(c['guest_helper'])],timeout=10).decode().split()[0]
    require(actual==source_profile(infra)['specification']['guest_helper_sha256'],'installed original guest helper changed')
    return ssh
def observation(infra,challenge):
    c,p=settings(infra.config);spec=p['specification'];rows=[]
    for placement in spec['placements']:
        file=c['guest_request_file'] if placement['kind']=='guest' else c['controller_request_file']
        requests=read(file);request=copy.deepcopy(next(v for v in requests if v['placement']==placement))
        request.update(action='topology',challenge=challenge)
        if placement['kind']=='guest':
            raw=infra.run([*guest_command(infra,c),'sudo -n python3 '+shlex.quote(c['guest_helper'])],data=canonical(request),timeout=12)
        else:
            raw=infra.run(['sudo','-n','python3',c['controller_helper']],data=canonical(request),timeout=12)
        require(len(raw)<=1048576,'original disk observation exceeds bound');row=json.loads(raw)
        require(row['placement']==placement and row['challenge']==challenge,'original physical disk observation scope differs')
        rows.append(row)
    return {'format':'cowork-total-disk-observation-v1','challenge':challenge,'specification_sha256':p['specification_sha256'],
        'executor_host_binding':spec['executor_host_binding'],'placements':rows}
def refresh(manager,infra):
    selected=settings(infra.config)
    if selected is None:return None
    c,p=selected;start=time.monotonic();binding=executor_host.manager_binding(infra.config)
    request={'executor_host_binding':binding,'expected_revision':p['revision'],'challenge':None,'observation':None}
    initial=manager.call('/disk-quota/instances/'+p['instance_id']+'/challenge',request)
    require(initial['profile']['specification_sha256']==p['specification_sha256'] and initial['profile']['revision']==p['revision'],'current original disk profile changed')
    actual=observation(infra,initial['challenge']);require(time.monotonic()-start<18,'physical disk observation deadline expired')
    result=manager.call('/disk-quota/instances/'+p['instance_id']+'/refresh',{**request,'challenge':initial['challenge'],'observation':actual})
    require(result['qualified'] is True and result['profile_revision']==p['revision'] and result['challenge']==initial['challenge'],'current disk qualification failed')
    return actual
def authorize_operation(infra,operation):
    selected=settings(infra.config)
    if selected is None:return
    _,p=selected;require(operation['instance_id']==p['instance_id'] and operation['executor_host_binding']==p['executor_host_binding'],'original operation belongs to another physical disk')
    manager=getattr(getattr(infra,'capacity_attestor',None),'manager',None);require(manager is not None,'current quota manager connection required')
    infra._disk_authorizing=True
    try:
        if operation['action'] not in ('stop','suspend','delete'):refresh(manager,infra)
        response=manager.call('/disk-quota/operations/'+operation['id']+'/authorize',{'executor_host_binding':operation['executor_host_binding'],'lease_token':operation['lease_token']})
        require(response['operation_id']==operation['id'] and response['lease_token']==operation['lease_token'],'original current IO authority changed')
        infra._disk_current_operation=operation
    finally:infra._disk_authorizing=False
def controller_pg_environment(config,url,environment):
    selected=settings(config)
    if selected is None:return None
    from urllib.parse import urlsplit
    c,p=selected;original=urlsplit(private(config['database_admin_url_file']).read_text().strip());runtime=urlsplit(url)
    admin_url=private(config['database_admin_url_file']).read_text().strip()
    require(url==admin_url or runtime.hostname=='postgres' and runtime.port==5432 and runtime.username==environment['PGUSER'] and runtime.path=='/'+environment['PGDATABASE'],
        'qualified runtime database source differs')
    require(original.scheme in ('postgres','postgresql') and original.hostname in ('127.0.0.1','localhost') and original.port
        and original.query=='sslmode=verify-full' and not original.fragment,'actual dedicated data-service TLS transport required')
    result={k:v for k,v in environment.items() if not k.startswith('PG')};result.update({k:environment[k] for k in ('PGUSER','PGPASSWORD','PGDATABASE')})
    result.update(PGHOST=original.hostname,PGPORT=str(original.port),PGSSLMODE='verify-full',PGSSLROOTCERT=config['guest_runtime_endpoints']['public_ca_file'],PGCONNECT_TIMEOUT='10',PGGSSENCMODE='disable')
    return result
def runtime_secrets(infra,instance,directory):
    selected=settings(infra.config)
    if selected is None:return
    from urllib.parse import urlsplit,urlunsplit
    c,p=selected;require(instance['id']==p['instance_id'],'original quota instance differs')
    path=directory/'secrets'/'database_url';runtime=directory/'runtime-secrets'/'database_url'
    value=urlsplit(private(path).read_text().strip())
    require(value.scheme in ('postgres','postgresql') and value.username==infra.resource_names(instance,directory)['database']
        and value.path=='/'+value.username,'actual original runtime database credentials required')
    updated=urlunsplit((value.scheme,value.username+':'+value.password+'@postgres:5432',value.path,'',''))
    # New-qualified directory was admitted before secrets existed; no live legacy
    # endpoints are changed and no credential bytes are exported in evidence.
    infra.private_write(path,updated.encode());infra.private_write(runtime,updated.encode());os.chmod(runtime,0o444)
def apply_manifest(infra,instance,directory,manifest):
    selected=settings(infra.config)
    if selected is None:return manifest
    c,p=selected;actual=observation(infra,str(uuid.uuid4()));guest=next(v for v in actual['placements'] if v['placement']['kind']=='guest')
    root=Path(guest['physical']['mount']);caps=resource_budget.service_caps(infra.config)
    def service(kind,image,command,destination,secret_keys):
        return {'image':image,'restart':'unless-stopped','read_only':True,'cap_drop':['ALL'],'security_opt':['no-new-privileges:true'],
            'command':command,'tmpfs':['/tmp','/run'],'networks':['services'],
            'labels':{'cowork.disk.instance':instance['id'],'cowork.disk.service':kind},'secrets':secret_keys,
            'volumes':[{'type':'bind','source':str(root/kind),'target':destination}],
            'memswap_limit':str(caps[kind]['memory_mib'])+'m',**resource_budget.service_compose(caps[kind])}
    pg=service('postgres',c['postgres_image'],['postgres','-c','max_connections=100'],'/var/lib/postgresql/data',['quota_pg_root'])
    pg['user']='999:999';pg['environment']={'POSTGRES_PASSWORD_FILE':'/run/secrets/quota_pg_root','PGDATA':'/var/lib/postgresql/data'}
    s3=service('minio',c['minio_image'],['server','/data','--address',':9000'],'/data',['quota_s3_user','quota_s3_root'])
    s3['user']='1000:1000';s3['environment']={'MINIO_ROOT_USER_FILE':'/run/secrets/quota_s3_user','MINIO_ROOT_PASSWORD_FILE':'/run/secrets/quota_s3_root'}
    manifest=copy.deepcopy(manifest);manifest['services'].update(postgres=pg,minio=s3)
    secrets=directory/'quota-runtime-secrets';secrets.mkdir(mode=0o700,exist_ok=True)
    require(not secrets.is_symlink() and secrets.resolve().is_relative_to(infra.root),'private original data-service secret directory required')
    for name,key in [('quota_pg_root','postgres_root_password_file'),('quota_s3_user','minio_root_user_file'),('quota_s3_root','minio_root_password_file')]:
        original=private(c[key]).read_bytes();target=secrets/name
        if target.exists():require(not target.is_symlink() and target.read_bytes()==original,'original data-service key changed; no implicit rotation')
        else:infra.private_write(target,original);os.chmod(target,0o444)
        manifest['secrets'][name]={'file':str(target)}
    fs=guest['physical'];policy={'version':1,'organisation_id':instance['organisation_id'],'instance_id':instance['id'],'profile_revision':p['revision'],
        'specification_sha256':p['specification_sha256'],'filesystem':{'path':'/run/cowork-disk','filesystem_uuid':fs['filesystem_uuid'],
        'device':fs['mount_device'],'fsid':fs['fsid'],'maximum_bytes':fs['size_bytes']}}
    for name in ('api','worker'):
        service=manifest['services'][name];service['environment'].update(COWORK_S3_ENDPOINT='http://minio:9000',COWORK_INSTANCE_DISK_POLICY=canonical(policy).decode())
        service['environment'].pop('COWORK_S3_CA_CERTIFICATE_FILE',None) # isolated same-instance data network, not the old TLS parent endpoint
        service.setdefault('volumes',[]).append({'type':'bind','source':str(root/'runtime-observation'),'target':'/run/cowork-disk','read_only':True})
    manifest['networks']['services']={'internal':True,'name':'cowork-'+uuid.UUID(instance['id']).hex+'-data'}
    return manifest
class Renewer:
    def __init__(self,manager,infra):self.manager,self.infra=manager,infra;self.stop=threading.Event();self.thread=None
    def start(self):
        if settings(self.infra.config) is None:return
        try:refresh(self.manager,self.infra)
        except Exception:pass # cleanup can still run; new IO needs current manager TTL
        def loop():
            while not self.stop.wait(10):
                try:refresh(self.manager,self.infra)
                except Exception:pass # manager TTL closes all new IO; no adoption/reallocation
        self.thread=threading.Thread(target=loop,name='cowork-disk-observer',daemon=True);self.thread.start()
    def close(self):
        self.stop.set()
        if self.thread:self.thread.join(55);require(not self.thread.is_alive(),'original disk observer did not drain')

def data_services_stopped(infra):
    c,p=settings(infra.config);requests=read(c['guest_request_file'])
    request=next(v for v in requests if v['placement']['kind']=='guest');t=request['topology']
    original=[t['postgres_container_id'],t['minio_container_id']]
    values=verified_data_services(infra)
    require(len(values)==2 and {v['Id'] for v in values}==set(original),'original dedicated data service inventory changed')
    return all(v['State']['Running'] is False and v['State']['Paused'] is False and v['State']['Restarting'] is False and v['State']['Pid']==0 for v in values)
def verified_data_services(infra):
    c,p=settings(infra.config);requests=read(c['guest_request_file']);request=next(v for v in requests if v['placement']['kind']=='guest')
    t=request['topology'];values=json.loads(infra.run(['docker','inspect',t['postgres_container_id'],t['minio_container_id']],timeout=10))
    for kind in ['postgres','minio']:
        identifier=t[kind+'_container_id'];v=next(v for v in values if v['Id']==identifier)
        require(v['Config']['Labels'].get('cowork.disk.instance')==p['instance_id'] and v['Config']['Labels'].get('cowork.disk.service')==kind
            and v['Config']['Image']==c[kind+'_image'],'original dedicated data service changed')
    return values
def data_service_ids(infra):
    c,p=settings(infra.config);request=next(v for v in read(c['guest_request_file']) if v['placement']['kind']=='guest')
    ids={request['topology'][k+'_container_id'] for k in ('postgres','minio')}
    require(len(ids)==2 and all(re.fullmatch('[a-f0-9]{64}',v) for v in ids),'original data service IDs required');return ids
def command_gate(infra,arguments):
    operation=getattr(infra,'_disk_current_operation',None)
    if operation is None or getattr(infra,'_disk_authorizing',False) or settings(infra.config) is None:return
    # An original result/journal can still be captured after authority is lost;
    # every subsequent external command receives fresh DB/lease/session checks.
    manager=getattr(getattr(infra,'capacity_attestor',None),'manager',None);require(manager is not None,'current original operation manager required')
    infra._disk_authorizing=True
    try:
        result=manager.call('/disk-quota/operations/'+operation['id']+'/authorize',{'executor_host_binding':operation['executor_host_binding'],'lease_token':operation['lease_token']})
        require(result['operation_id']==operation['id'] and result['lease_token']==operation['lease_token'],'original operation authority expired after IO wait')
    finally:infra._disk_authorizing=False
def pending_data_services(infra,instance,owned,services):
    selected=settings(infra.config)
    if selected is None:return not owned
    c,p=selected;require(p['instance_id']==instance and set(services)==set(resource_budget.DEFAULT_SERVICES)|set(resource_budget.DATA_SERVICES),'original six-service reservation required')
    original=verified_data_services(infra);require({v['Id'] for _,v in owned}=={v['Id'] for v in original},'pending instance has unregistered workloads')
    for suffix,v in owned:
        kind=next(k for k in ('postgres','minio') if suffix==k+'-1');caps=v['HostConfig'];allocation=services[kind]
        require(caps['NanoCpus']==allocation['cpu_millis']*1000000 and caps['Memory']==allocation['memory_mib']*1048576
            and caps['PidsLimit']==allocation['pids'] and caps['ReadonlyRootfs'] is True and caps['Privileged'] is False
            and set(caps['CapDrop'])=={'ALL'} and 'no-new-privileges:true' in caps['SecurityOpt'],'original data service resources changed')
    return True
def apply_configuration(config,instance,directory,manifest):
    from types import SimpleNamespace
    view=SimpleNamespace(config=config,root=Path(config['state_directory']).resolve(),run=__import__('provisioner').run,
        private_write=__import__('provisioner').private_write,resource_names=__import__('provisioner').resource_names)
    return apply_manifest(view,instance,directory,manifest)
def compose_arguments(config,arguments):
    if settings(config) is None:return arguments
    if arguments and arguments[0] in ('up','start','restart') and not any(v in ('postgres','minio','api','worker','runner','egress-proxy') for v in arguments[1:]):
        return [*arguments,'api','worker','runner','egress-proxy']
    return arguments
