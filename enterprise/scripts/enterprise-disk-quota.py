#!/usr/bin/env python3
"""Explicit original disk plan/install/observe/commit, never live automatic migration.

Each physical Create is dispatched once into a private journal before IO. An
unknown ACK cannot be reconciled by finding a matching container name. Observe
and API response recovery perform no Create, allocation or resource release.
"""
import argparse,copy,hashlib,json,os,re,shlex,ssl,sys,uuid
from pathlib import Path
from urllib.request import Request,build_opener,HTTPSHandler,HTTPRedirectHandler,ProxyHandler
from urllib.parse import urlsplit

ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'server/cowork-provisioner'))
import backend_profile,executor_host,provisioner,resource_budget
from instance_disk_quotas import helper_source

def require(ok,why):
    if not ok:raise RuntimeError(why)
def canonical(v):return json.dumps(v,sort_keys=True,separators=(',',':'),ensure_ascii=False).encode()
def sha(v):return hashlib.sha256(canonical(v)).hexdigest()
def private(p):
    p=Path(p);s=p.lstat();require(p.is_absolute() and p.is_file() and not p.is_symlink() and all(not v.is_symlink() for v in p.parents)
        and s.st_nlink==1 and s.st_size<=2097152 and (os.name!='posix' or s.st_uid==os.geteuid() and not s.st_mode&0o077),'private operator file required')
    return p
def read(p):return json.loads(private(p).read_bytes())
class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self,*args,**kwargs):raise RuntimeError('management redirect forbidden')
def http(args,path,body=None,operator=False):
    base=args.manager_url.rstrip('/');url=urlsplit(base);require(url.scheme=='https' and not url.username and not url.password and not url.query and not url.fragment,'pinned HTTPS management origin required')
    token=private(args.operator_token_file if operator else args.session_file).read_text().strip();require(len(token)>=32,'current original credential required')
    client=build_opener(ProxyHandler({}),NoRedirect(),HTTPSHandler(context=ssl.create_default_context(cafile=str(private(args.manager_ca)))))
    headers={'Content-Type':'application/json',**({'x-cowork-provisioner-token':token} if operator else {'Authorization':'Bearer '+token})}
    with client.open(Request(base+path,None if body is None else canonical(body),headers),timeout=30) as response:
        raw=response.read(2097153);require(len(raw)<=2097152,'management original response exceeds bound');return json.loads(raw)
def save_new(p,v):
    p=Path(p);require(p.is_absolute() and not p.is_symlink() and all(not x.is_symlink() for x in p.parents),'new private receipt path required')
    fd=os.open(p,os.O_WRONLY|os.O_CREAT|os.O_EXCL|getattr(os,'O_NOFOLLOW',0),0o600)
    with os.fdopen(fd,'wb') as out:out.write(canonical(v));out.flush();os.fsync(out.fileno())
def run(args,**kwargs):return provisioner.run(args,**kwargs)
def original(args,id):return http(args,'/management/runner/v1/disk-quota/'+id,operator=True)
def guest(cfg,helper,source,request):
    profile=backend_profile.settings(cfg);argv=profile['probe_command'];require('--config' in argv,'original controller profile required')
    value=read(argv[argv.index('--config')+1]);root=Path(value['vm_root']);vm=read(root/'vm.json')
    require(vm['id']==profile['specification']['vm_id'],'original guest hardware scope changed')
    ssh=['ssh','-i',str(private(root/'ssh-key')),'-p',str(vm['ssh_port']),'-o','BatchMode=yes','-o','IdentitiesOnly=yes','-o','StrictHostKeyChecking=yes',
        '-o','UserKnownHostsFile='+str(private(root/'known_hosts')),'-o','ConnectTimeout=5','cowork@127.0.0.1']
    actual=run([*ssh,'sudo -n sha256sum -- '+shlex.quote(helper)]).decode().split()[0];require(actual==source,'installed physical helper source differs')
    return json.loads(run([*ssh,'sudo -n python3 '+shlex.quote(helper)],data=canonical(request)))
def helper(args,cfg,plan,request):
    spec=plan['specification'];kind=request['placement']['kind']
    if kind=='guest':return guest(cfg,args.guest_helper,spec['guest_helper_sha256'],request)
    require(hashlib.sha256(helper_source(args.controller_helper).read_bytes()).hexdigest()==spec['controller_helper_sha256'],'original controller helper source differs')
    return json.loads(run(['sudo','-n','python3',args.controller_helper],data=canonical(request)))
def install(args,cfg,plan):
    require(plan['purpose']=='activate' and plan['state']=='prepared','only an original prepared installation plan may allocate')
    spec=plan['specification'];journal=Path(args.journal_directory);require(journal.is_absolute() and not journal.is_symlink(),'private operator installation journal required')
    journal.mkdir(mode=0o700,exist_ok=True);require(not journal.stat().st_mode&0o077,'private installation journal required')
    source=journal/'original-plan.json'
    if source.exists():require(read(source)==plan,'original installation plan changed')
    else:save_new(source,plan)
    requests=[]
    for placement in spec['placements']:
        fresh=original(args,plan['id']);require(fresh['specification_sha256']==plan['specification_sha256'] and fresh['state']=='prepared','fresh original install authority changed')
        request={'format':'cowork-disk-request-v1','operation_id':str(uuid.uuid5(uuid.UUID(plan['id']),'allocate:'+placement['id'])),
            'placement':placement,'action':'init','challenge':plan['challenge'],'topology':{}}
        receipt=helper(args,cfg,plan,request);save=journal/(placement['id']+'.json')
        if save.exists():require(read(save)['allocation_sha256']==receipt['allocation_sha256'],'original allocation readback changed')
        else:save_new(save,receipt)
        request['action']='topology';requests.append(request)
    for request in requests:
        if request['placement']['kind']=='controller':
            request['action']='directories';request['topology']={'paths':spec['topology']['controller_paths'],
                'snapshot_placement':next((v for v in spec['placements'] if v['kind']=='snapshot'),None)};helper(args,cfg,plan,request)
        if request['placement']['kind']=='guest':
            request['action']='configure';request['confirmation']='INSTALL_EMPTY_ORIGINAL_DOCKER_DISK';helper(args,cfg,plan,request)
        request['action']='topology'
    save_new(Path(args.output),{'format':'cowork-disk-installation-v1','plan_id':plan['id'],'specification_sha256':plan['specification_sha256'],
        'requests':requests,'next':'bootstrap original dedicated data services, capture IDs, then observe and commit'})
def bootstrap(args,cfg,plan):
    spec=plan['specification'];require(spec['topology']['database']['host']=='postgres:5432' and spec['topology']['objects']['endpoint']=='http://minio:9000','dedicated instance-network endpoints required')
    installed=read(args.installation_file);require(installed['plan_id']==plan['id'] and installed['specification_sha256']==plan['specification_sha256'],'original installation output differs')
    request=next(v for v in installed['requests'] if v['placement']['kind']=='guest');request=copy.deepcopy(request);request['action']='observe'
    observation=helper(args,cfg,plan,request);mount=Path(observation['physical']['mount'])
    config=copy.deepcopy(cfg);config.pop('instance_disk_quotas',None);infra=provisioner.Infrastructure(config);infra.host.bind();infra.host.verify()
    scope='cowork-'+uuid.UUID(spec['instance_id']).hex;network=scope+'-data';journal=Path(args.journal_directory)
    original_path=journal/'data-bootstrap.json'
    if original_path.exists():
        old=read(original_path);require(old['specification_sha256']==plan['specification_sha256'] and old['state']=='received','original service Create ACK unknown; no resend or name adoption')
        save_new(Path(args.output),old);return
    intent={'format':'cowork-disk-data-bootstrap-v1','plan_id':plan['id'],'specification_sha256':plan['specification_sha256'],'state':'admitted','network':network,'services':{}}
    require(not infra.run(['docker','ps','-aq']).strip() and not infra.run(['docker','network','ls','--filter','name=^'+network+'$','-q']).strip(),'old/foreign data services or network cannot be adopted')
    save_new(original_path,intent)
    # Network and both creates belong to this one durable operation. A partial
    # or lost acknowledgement is held, never regenerated under another UUID.
    original(args,plan['id'])
    nid=infra.run(['docker','network','create','--internal','--label','com.docker.compose.project='+scope,'--label','com.docker.compose.network=services',network]).decode().strip()
    require(re.fullmatch('[a-f0-9]{64}',nid),'original network Create ACK missing');intent['network_id']=nid;infra.private_write(original_path,canonical(intent))
    quota=cfg['disk_quota_installation'];caps=resource_budget.service_caps({'instance_disk_quotas':{},'instance_service_limits':cfg.get('instance_service_limits',{})})
    for kind,uid,destination in [('postgres','999:999','/var/lib/postgresql/data'),('minio','1000:1000','/data')]:
        image=quota[kind+'_image'];provisioner.immutable_image(image);infra.run(['docker','pull',image]);original(args,plan['id'])
        name=scope+'-'+kind+'-1';cap=caps[kind]
        argv=['docker','create','--name',name,'--network',network,'--network-alias',kind,'--read-only','--user',uid,
            '--cap-drop','ALL','--security-opt','no-new-privileges:true','--cpus',str(cap['cpu_millis']/1000),'--memory',str(cap['memory_mib'])+'m',
            '--memory-swap',str(cap['memory_mib'])+'m','--pids-limit',str(cap['pids']),'--tmpfs','/tmp','--tmpfs','/run','--label','com.docker.compose.project='+scope,
            '--label','com.docker.compose.service='+kind,'--label','cowork.disk.instance='+spec['instance_id'],'--label','cowork.disk.service='+kind,
            '--mount','type=bind,source='+str(mount/kind)+',target='+destination]
        secret_root=Path(spec['topology']['controller_paths']['state'])/uuid.UUID(spec['instance_id']).hex/'quota-runtime-secrets';secret_root.mkdir(mode=0o700,parents=True,exist_ok=True)
        require(not secret_root.is_symlink() and all(not p.is_symlink() for p in secret_root.parents),'original secret placement required')
        secrets=[('quota_pg_root','postgres_root_password_file')] if kind=='postgres' else [('quota_s3_user','minio_root_user_file'),('quota_s3_root','minio_root_password_file')]
        for target,key in secrets:
            source=private(quota[key]);data=source.read_bytes();private_target=secret_root/target
            if private_target.exists():require(private_target.read_bytes()==data,'original data-service credential changed')
            infra.private_write(private_target,data);os.chmod(private_target,0o444)
            argv+=['--mount','type=bind,source='+str(private_target)+',target=/run/secrets/'+target+',readonly']
        if kind=='postgres':argv+=['--env','PGDATA=/var/lib/postgresql/data','--env','POSTGRES_PASSWORD_FILE=/run/secrets/quota_pg_root',image,'postgres','-c','max_connections=100']
        else:argv+=['--env','MINIO_ROOT_USER_FILE=/run/secrets/quota_s3_user','--env','MINIO_ROOT_PASSWORD_FILE=/run/secrets/quota_s3_root',image,'server','/data','--address',':9000']
        cid=infra.run(argv).decode().strip();require(re.fullmatch('[a-f0-9]{64}',cid),'original data-service Create ACK missing')
        intent['services'][kind]={'container_id':cid,'image':image};infra.private_write(original_path,canonical(intent));original(args,plan['id'])
        infra.run(['docker','start',cid])
    request['action']='topology';request['topology']={'postgres_container_id':intent['services']['postgres']['container_id'],
        'minio_container_id':intent['services']['minio']['container_id'],'database_host':'postgres:5432','s3_endpoint':'http://minio:9000'}
    actual=helper(args,cfg,plan,request);intent.update(state='received',original_observation=actual,guest_request=request)
    infra.private_write(original_path,canonical(intent));save_new(Path(args.output),intent)
def main():
    a=argparse.ArgumentParser();a.add_argument('action',choices=['prepare','original','install','bootstrap','observe','retire','commit','inspect'])
    for field in ['manager-url','manager-ca','session-file','operator-token-file','config','plan-id','organisation-id','instance-id','specification-file','request-id','journal-directory','guest-helper','controller-helper','output','installation-file','bootstrap-file']:
        a.add_argument('--'+field)
    a.add_argument('--instance-revision',type=int);a.add_argument('--profile-revision',type=int,default=0);a.add_argument('--purpose',choices=['activate','disable','release'],default='activate')
    args=a.parse_args();base='/management/v1/organisations/'+str(args.organisation_id)+'/instances/'+str(args.instance_id)+'/disk-quota'
    if args.action=='prepare':
        request={'request_id':args.request_id,'expected_instance_revision':args.instance_revision,'expected_profile_revision':args.profile_revision,'purpose':args.purpose,
            'specification':read(args.specification_file),'confirmation':'RESERVE_ORIGINAL_TOTAL_PHYSICAL_DISK'}
        save_new(Path(args.output),http(args,base,request));return
    if args.action=='original':save_new(Path(args.output),http(args,base+'/operations/'+args.request_id));return
    if args.action=='inspect':save_new(Path(args.output),http(args,base));return
    if args.action=='commit':save_new(Path(args.output),http(args,base+'/commit/'+args.plan_id,{}));return
    cfg=read(args.config);plan=original(args,args.plan_id)
    if args.action=='install':install(args,cfg,plan);return
    if args.action=='bootstrap':bootstrap(args,cfg,plan);return
    installed=read(args.installation_file);boot=read(args.bootstrap_file);rows=[]
    require(installed['specification_sha256']==plan['specification_sha256'] and boot['specification_sha256']==plan['specification_sha256'],'original installation/boot scope differs')
    requests=sorted(installed['requests'],key=lambda r: {'guest':0,'environment':1,'snapshot':2,'controller':3}[r['placement']['kind']])
    for request in requests:
        request=copy.deepcopy(boot['guest_request'] if request['placement']['kind']=='guest' else request);request.update(action='topology',challenge=plan['challenge'])
        if args.action=='retire':
            require(plan['purpose']=='release','original release plan required')
            request.update(action='release',operation_id=str(uuid.uuid5(uuid.UUID(plan['id']),'release:'+request['placement']['id'])),confirmation='DESTROY_ORIGINAL_PHYSICAL_DISK')
        rows.append(helper(args,cfg,plan,request))
    observation={'format':'cowork-total-disk-observation-v1','challenge':plan['challenge'],'specification_sha256':plan['specification_sha256'],
        'executor_host_binding':plan['executor_host_binding'],'placements':rows}
    receipt=http(args,'/management/runner/v1/disk-quota/'+plan['id']+'/observe',{'executor_host_binding':plan['executor_host_binding'],
        'challenge':plan['challenge'],'specification_sha256':plan['specification_sha256'],'observation':observation},operator=True)
    save_new(Path(args.output),{'original_observation':observation,'receipt':receipt})
if __name__=='__main__':
    try:main()
    except Exception:sys.stderr.write('original disk action failed; recover its original receipt, never replay unknown physical IO\n');sys.exit(1)
