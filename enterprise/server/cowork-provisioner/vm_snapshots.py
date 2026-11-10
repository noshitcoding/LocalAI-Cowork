"""Original owned-QEMU disk snapshots with real age and QMP/SSH/mTLS readbacks.

External application PostgreSQL/S3 and parent control files are not snapshotted.
Unknown IO acknowledgements remain held; no snapshot/create/restore replay.
"""
import hashlib,importlib.util,json,os,re,sys,time,uuid
from pathlib import Path
import backend_profile,executor_host

def require(ok,message):
    if not ok:raise RuntimeError(message)
def canonical(value):return json.dumps(value,sort_keys=True,ensure_ascii=False,separators=(',',':')).encode()
def sha(value):return hashlib.sha256(value).hexdigest()
def identifier(value):require(str(uuid.UUID(value))==value,'noncanonical VM snapshot identity');return uuid.UUID(value).hex
def private(path,directory=False):
    path=Path(path);require(path.is_absolute() and not path.is_symlink() and all(not p.is_symlink() for p in path.parents)
        and (path.is_dir() if directory else path.is_file()),'original private VM snapshot path required')
    if os.name=='posix':require(path.stat().st_uid==os.getuid() and not path.stat().st_mode&0o077,'owner-only VM snapshot state required')
    return path
def read(path):
    path=private(path);require(path.stat().st_size<=2097152,'VM snapshot marker exceeds bound');return json.loads(path.read_bytes())
def digest(path):
    private(path)
    with Path(path).open('rb') as stream:return hashlib.file_digest(stream,'sha256').hexdigest()
def module(infra):
    configured=infra.config['vm_snapshot']['controller_file'];source=private(configured)
    require(digest(source)==infra.config['vm_snapshot']['controller_sha256'] and source.name=='local_executor_vm.py','reviewed original QEMU controller source changed')
    spec=importlib.util.spec_from_file_location('_cowork_bound_snapshot_vm',source);value=importlib.util.module_from_spec(spec);sys.modules[spec.name]=value;spec.loader.exec_module(value)
    return value
def settings(infra):
    value=infra.config.get('vm_snapshot')
    if value is None:return None
    require(set(value)=={'version','controller_file','controller_sha256','store_directory','store_filesystem_uuid','maximum_bytes','age_recipient','recovery_identity_file'} and value['version']==1,'invalid original VM snapshot configuration')
    profile=backend_profile.settings(infra.config);require(profile and profile['specification']['kind']=='vm_compose','VM snapshots require actual activated VM profile')
    private(value['store_directory'],True);private(value['recovery_identity_file']);require(re.fullmatch('[a-f0-9]{64}',value['controller_sha256']) and type(value['maximum_bytes']) is int and value['maximum_bytes']>=1073741824,'invalid snapshot source/storage bound')
    require(isinstance(value['age_recipient'],str) and re.fullmatch('age1[0-9a-z]{50,100}',value['age_recipient']),'explicit snapshot age recipient required')
    return value
def storage(infra):
    selected=settings(infra);root=private(selected['store_directory'],True)
    mount=json.loads(infra.run(['findmnt','--json','--mountpoint',str(root),'-o','SOURCE,TARGET,FSTYPE,UUID']))['filesystems']
    require(len(mount)==1 and mount[0]['target']==str(root) and mount[0]['fstype']=='ext4' and mount[0]['uuid']==selected['store_filesystem_uuid'],'original dedicated hard-bounded snapshot filesystem changed')
    stat=os.statvfs(root);require(0<stat.f_blocks*stat.f_frsize<=selected['maximum_bytes'],'actual snapshot filesystem exceeds configured hard bound')
    marker=root/'store.json';binding={'version':1,'filesystem_uuid':selected['store_filesystem_uuid'],'maximum_bytes':selected['maximum_bytes'],
        'recipient':selected['age_recipient'],'identity_sha256':digest(selected['recovery_identity_file'])}
    if marker.exists():require(read(marker)==binding,'original snapshot store/key binding changed')
    else:
        require(all(p.name=='lost+found' and p.is_dir() and not p.is_symlink() for p in root.iterdir()),'unregistered snapshot store contains unknown original allocations')
        infra.private_write(marker,canonical(binding))
    return root,stat
def vm_root(infra,plan):
    command=backend_profile.settings(infra.config)['probe_command'];require('--config' in command,'original VM producer configuration missing')
    config=read(Path(command[command.index('--config')+1]));root=private(config['vm_root'],True);controller=module(infra);value=controller.load(root)
    require(value['version']==2 and value['id']==plan['backend_profile']['specification']['vm_id'] and value['organisation_id']==plan['organisation_id'] and value['operator_state_id']==plan['executor_host_binding']['operator_state_id'],'original VM assignment differs')
    with controller.capacity_pool.locked_pool() as pool:
        pool.require_registered(root,value);pool.check_inventory(controller.load);allocation=pool.data['allocations'][value['id']]
        require(not allocation['adopted'] and str(root) not in pool.data['plan']['existing_vm_roots'],'protected/adopted VM snapshot forbidden')
    return controller,root,value
def parent_configuration(infra,plan,root):
    directory=infra.directory(plan['instance_id']);active=infra.active_directory(directory)
    names=('vm.json','base.qcow2','seed.iso','ca.pem','server.pem','server-key.pem','client.pem','client-key.pem','ssh-key','known_hosts')
    frozen={name:digest(root/name) for name in names}
    for file in sorted((active/'secrets').iterdir()):
        require(file.is_file() and not file.is_symlink(),'original instance secret inventory changed');frozen['instance-secret/'+file.name]=digest(file)
    frozen['instance-compose']=digest(active/'compose.json');frozen['profile']=plan['backend_profile']['specification_sha256']
    return sha(canonical(frozen))
def quiescent(infra,plan):
    raw=infra.run(['docker','ps','-aq','--no-trunc']).decode().splitlines()
    require(len(raw)<=20000 and all(re.fullmatch('[a-f0-9]{64}',v) for v in raw),'complete original VM container inventory required')
    if not raw:return []
    actual=json.loads(infra.run(['docker','inspect',*raw]));prefix='/cowork-'+identifier(plan['instance_id'])+'-'
    require(len(actual)==len(raw) and {v['Id'] for v in actual}==set(raw) and all(v['Name'].startswith(prefix) and not v['State']['Running'] and v['State']['Pid']==0 for v in actual),'VM snapshot requires all actual own instance producers stopped')
    return sorted([{'container_id':v['Id'],'image_id':v['Image'],'started_at':v['State']['StartedAt'],'finished_at':v['State']['FinishedAt']} for v in actual],key=lambda v:v['container_id'])
def quiet(controller,root,value):
    parent,entry,entries=controller.retirement_tree(root)
    record={'allocation':{'identity':controller.capacity_pool.snapshot(root,value)},'quarantine':str(root.parent/('.snapshot-unused-'+uuid.UUID(value['id']).hex)),
        'root_entry':entry,'entries':entries,'parent':parent}
    controller.retirement_quiet(record);require(controller.process(root) is None,'original VM process remains alive')
def authorize(manager,job,binding):
    value=manager.call('/vm-snapshots/'+job['id']+'/authorize',{'executor_host_binding':binding,'lease_token':job['lease_token'],'plan_sha256':job['plan_sha256'],'receipt':{}})
    require(value['authorized'] is True and value['id']==job['id'] and value['plan_sha256']==job['plan_sha256'],'current original VM snapshot authority changed')
def data_services(infra,plan,manager,job,binding,journal,marker):
    if infra.config.get('instance_disk_quotas') is None:return []
    import instance_disk_quotas
    profile=instance_disk_quotas.source_profile(infra);require(profile['instance_id']==plan['instance_id'] and profile['executor_host_binding']==plan['executor_host_binding'],'snapshot original physical disk scope differs');instance_disk_quotas.refresh(manager,infra)
    active=infra.active_directory(infra.directory(plan['instance_id']));manifest=read(active/'compose.json');facts=[]
    require(set(manifest['services'])=={'api','worker','runner','egress-proxy','postgres','minio'},'original dedicated six-service manifest required')
    for service in ('postgres','minio'):
        ids=infra.compose(plan['instance'],active,'ps','--all','--quiet',service).decode().splitlines();require(len(ids)==1 and re.fullmatch('[a-f0-9]{64}',ids[0]),'one actual original data-service UID required')
        a=json.loads(infra.run(['docker','inspect',ids[0]]))[0];labels=a['Config']['Labels'];require(a['Id']==ids[0] and labels['com.docker.compose.project']==manifest['name'] and labels['com.docker.compose.service']==service and a['Config']['Image']==manifest['services'][service]['image'],'original dedicated data service/manifest changed')
        facts.append({'container_id':a['Id'],'service':service,'image_id':a['Image'],'labels':labels,'started_at':a['State']['StartedAt']})
    marker.update(stage='data_stop_prepared',original_data_services=facts);saved(infra,journal,marker)
    for own in facts:
        authorize(manager,job,binding);a=json.loads(infra.run(['docker','inspect',own['container_id']]))[0];require(a['Image']==own['image_id'] and a['Config']['Labels']==own['labels'] and a['State']['StartedAt']==own['started_at'],'original data stop UID/generation changed')
        if a['State']['Running']:
            marker['stage']='data_stop_dispatched';saved(infra,journal,marker);infra.run(['docker','stop','--time','60',own['container_id']])
        a=json.loads(infra.run(['docker','inspect',own['container_id']]))[0];require(a['Id']==own['container_id'] and a['Config']['Labels']==own['labels'] and not a['State']['Running'] and a['State']['Pid']==0 and a['State']['FinishedAt'] and not a['State']['FinishedAt'].startswith('0001'),'actual original data-service termination unproven');own['finished_at']=a['State']['FinishedAt']
        saved(infra,journal,marker)
    return facts
def resume_data_services(infra,plan,manager,job,binding,facts,journal,marker):
    for own in facts:
        authorize(manager,job,binding);a=json.loads(infra.run(['docker','inspect',own['container_id']]))[0];require(a['Id']==own['container_id'] and a['Image']==own['image_id'] and a['Config']['Labels']==own['labels'] and not a['State']['Running'] and a['State']['Pid']==0,'original data-service resume requires its retained stopped UID')
        marker['stage']='data_resume_dispatched';saved(infra,journal,marker);require(infra.run(['docker','start',own['container_id']]).decode().strip()==own['container_id'],'original data-service start ACK changed')
        a=json.loads(infra.run(['docker','inspect',own['container_id']]))[0];require(a['Id']==own['container_id'] and a['Image']==own['image_id'] and a['Config']['Labels']==own['labels'] and a['State']['Running'] and a['State']['Pid']>0,'original data-service physical resume unproven')
def saved(infra,path,value):infra.private_write(path,canonical(value))
def execute(manager,infra,job):
    selected=settings(infra);require(selected is not None,'VM snapshot operator opt-in required');plan=job['plan']
    require(plan['format']==1 and sha(canonical(plan))==job['plan_sha256'] and plan['id']==job['id'],'original VM snapshot plan differs')
    directory=infra.directory(plan['instance_id']);journal=directory/('vm-snapshot-job-'+identifier(job['id'])+'.json')
    if journal.exists():
        original=read(journal);require(original['plan_sha256']==job['plan_sha256'] and original['lease_token']==job['lease_token'],'original snapshot journal differs')
        if original.get('receipt') is not None:return original['receipt']
        raise RuntimeError('VM snapshot original IO unproven; explicit review required')
    binding=executor_host.manager_binding(infra.config);authorize(manager,job,binding);backend_profile.validate(infra,plan,physical=True)
    controller,root,value=vm_root(infra,plan);store,filesystem=storage(infra)
    parent=parent_configuration(infra,plan,root);disk=root/'disk.qcow2'
    marker={'format':1,'plan_sha256':job['plan_sha256'],'lease_token':job['lease_token'],'vm_id':value['id'],'parent_configuration_sha256':parent,'stage':'prepared'}
    saved(infra,journal,marker)
    data=[] if plan['action']=='delete_snapshot' else data_services(infra,plan,manager,job,binding,journal,marker)
    containers=quiescent(infra,plan) if plan['action']!='delete_snapshot' else []
    marker['original_containers']=containers;saved(infra,journal,marker);snapshot_id=identifier(plan['snapshot_id']);manifest=store/(snapshot_id+'.json');archive=store/(snapshot_id+'.age')
    prior=read(manifest) if plan['action']!='snapshot' else None
    if prior is not None:
        require(prior['instance_id']==plan['instance_id'] and prior['organisation_id']==plan['organisation_id'] and prior['vm_id']==value['id']
            and prior['backend_profile_sha256']==plan['backend_profile']['specification_sha256'] and prior['parent_configuration_sha256']==parent,'snapshot cannot restore changed host/keys/configuration')
        require(digest(archive)==prior['archive_sha256'] and archive.stat().st_size==prior['archive_size_bytes'] and archive.stat().st_ino==prior['archive_inode'],'original encrypted archive changed')
    restarted=False;comparison=False;encrypted=False;unlinked=False;restored=None
    if plan['action']=='delete_snapshot':
        authorize(manager,job,binding);marker['stage']='archive_delete_dispatched';saved(infra,journal,marker)
        descriptor=os.open(archive,os.O_PATH|os.O_NOFOLLOW)
        try:
            require(os.fstat(descriptor).st_ino==prior['archive_inode'] and os.fstat(descriptor).st_nlink==1,'original archive inode changed')
            archive.unlink();require(os.fstat(descriptor).st_nlink==0 and not archive.exists(),'original archive unlink unproven');unlinked=True
        finally:os.close(descriptor)
        saved(infra,manifest,{**prior,'state':'deleted','deletion_job_id':job['id']})
    else:
        authorize(manager,job,binding);marker['stage']='vm_stop_dispatched';saved(infra,journal,marker)
        controller.stop(root);quiet(controller,root,value);marker['stage']='vm_stopped';saved(infra,journal,marker)
        # Hold the actual controller inventory lock over offline disk IO. The
        # normal controller start path acquires this same lock.
        with controller.inventory_lock(root.parent):
            quiet(controller,root,value);source_sha=digest(disk);plain=store/(snapshot_id+'.'+identifier(job['id'])+'.qcow2')
            require(not plain.exists(),'unacknowledged plaintext snapshot exists')
            info=json.loads(infra.run(['qemu-img','info','--output=json',str(disk)]));require(info['format']=='qcow2' and info['virtual-size']==value['disk_virtual_gib']*1073741824,'original VM virtual disk size changed')
            maximum=info['virtual-size']+1073741824
            # Each retained original archive remains charged at its maximum,
            # including missing/corrupt files. Scratch needs another full slot.
            rows=[read(p) for p in store.glob('*.json') if p.name!='store.json'];require(all(v.get('format')=='cowork-owned-vm-disk-snapshot-v1' for v in rows),'snapshot inventory contains unknown allocations')
            held=sum(v['maximum_archive_bytes'] for v in rows if v['state']!='deleted')
            require(held+maximum*(2 if plan['action']=='snapshot' else 1)<=selected['maximum_bytes'] and filesystem.f_bavail*filesystem.f_frsize>=maximum*2,'snapshot physical storage reserve exhausted')
            authorize(manager,job,binding)
            if plan['action']=='snapshot':
                require(not manifest.exists() and not archive.exists(),'original snapshot ID already has unacknowledged storage')
                reservation={'format':'cowork-owned-vm-disk-snapshot-v1','state':'held','snapshot_id':plan['snapshot_id'],'instance_id':plan['instance_id'],'organisation_id':plan['organisation_id'],
                    'vm_id':value['id'],'backend_profile_sha256':plan['backend_profile']['specification_sha256'],'parent_configuration_sha256':parent,'maximum_archive_bytes':maximum,'job_id':job['id']}
                saved(infra,manifest,reservation);marker['stage']='disk_copy_dispatched';saved(infra,journal,marker)
                infra.run(['qemu-img','convert','-f','qcow2','-O','qcow2','-S','0',str(disk),str(plain)]);os.chmod(plain,0o600)
                infra.run(['qemu-img','check','-f','qcow2',str(plain)]);infra.run(['qemu-img','compare','-f','qcow2','-F','qcow2',str(disk),str(plain)])
                require(digest(disk)==source_sha,'source VM disk changed during offline snapshot');plain_sha=digest(plain);comparison=True
                marker['stage']='archive_encrypt_dispatched';saved(infra,journal,marker)
                infra.run(['age','--encrypt','-r',selected['age_recipient'],'-o',str(archive),str(plain)]);os.chmod(archive,0o600);plain.unlink()
                require(archive.stat().st_size<=maximum,'actual encrypted snapshot exceeds reserved hard limit')
                infra.run(['age','--decrypt','-i',selected['recovery_identity_file'],'-o',str(plain),str(archive)]);os.chmod(plain,0o600)
                require(digest(plain)==plain_sha,'actual age recovery identity/archive integrity mismatch');infra.run(['qemu-img','check','-f','qcow2',str(plain)]);encrypted=True;plain.unlink()
                prior={**reservation,'state':'retained','archive_sha256':digest(archive),'archive_size_bytes':archive.stat().st_size,'archive_inode':archive.stat().st_ino,
                    'source_disk_sha256':plain_sha,'original_overlay_sha256':source_sha};saved(infra,manifest,prior)
            else:
                marker['stage']='archive_decrypt_dispatched';saved(infra,journal,marker)
                infra.run(['age','--decrypt','-i',selected['recovery_identity_file'],'-o',str(plain),str(archive)]);os.chmod(plain,0o600)
                require(digest(plain)==prior['source_disk_sha256'],'original snapshot disk integrity mismatch');infra.run(['qemu-img','check','-f','qcow2',str(plain)])
                authorize(manager,job,binding);require(parent_configuration(infra,plan,root)==parent,'original snapshot parent keys/configuration changed')
                # Atomic replacement is preceded by original inode/digest CAS.
                original=disk.stat();require(original.st_nlink==1 and digest(disk)==source_sha,'original restore target changed')
                candidate=root/('snapshot-restore-'+identifier(job['id'])+'.qcow2');require(not candidate.exists(),'unacknowledged restore candidate exists')
                marker['stage']='restore_copy_dispatched';saved(infra,journal,marker)
                infra.run(['cp','--reflink=never','--sparse=never','--',str(plain),str(candidate)]);os.chmod(candidate,0o600)
                require(digest(candidate)==prior['source_disk_sha256'] and disk.stat().st_ino==original.st_ino and digest(disk)==source_sha,'actual restore copy/CAS differs')
                with candidate.open('rb') as stream:os.fsync(stream.fileno())
                os.replace(candidate,disk);controller.sync_directory(root);restored=digest(disk);require(restored==prior['source_disk_sha256'],'restored original snapshot bytes differ');plain.unlink();comparison=True
            quiet(controller,root,value);require(parent_configuration(infra,plan,root)==parent,'original VM parent identity changed during offline IO')
        authorize(manager,job,binding);marker['stage']='vm_start_dispatched';saved(infra,journal,marker);controller.start(root)
        deadline=time.monotonic()+180
        while time.monotonic()<deadline:
            try:backend_profile.validate(infra,plan,physical=True);break
            except Exception:time.sleep(1)
        backend_profile.validate(infra,plan,physical=True);require(controller.qmp(root,'query-kvm')['enabled'] and controller.qmp(root,'query-status')['status']=='running','actual original VM restart unproven')
        stopped=quiescent(infra,plan)
        require(plan['action']!='snapshot' or stopped==containers,'original stopped producers changed during VM restart');resume_data_services(infra,plan,manager,job,binding,data,journal,marker);restarted=True
    authorize(manager,job,binding);backend_profile.validate(infra,plan,physical=True)
    receipt={'format':'cowork-vm-snapshot-completion-v1','job_id':job['id'],'plan_sha256':job['plan_sha256'],'snapshot_id':plan['snapshot_id'],'action':plan['action'],'vm_id':value['id'],
        'backend_profile_sha256':plan['backend_profile']['specification_sha256'],'original_assignment_verified':True,'physical_quiescence_verified':True,
        'actual_vm_running_verified':True,'actual_vm_restart_verified':restarted,'parent_configuration_sha256':parent,'disk_content_comparison_verified':comparison,
        'encrypted_archive_verified':encrypted,'original_archive_deleted':unlinked,'original_inode_unlinked':unlinked,
        'archive_sha256':prior['archive_sha256'],'archive_size_bytes':prior['archive_size_bytes'],'source_disk_sha256':prior['source_disk_sha256'],'restored_disk_sha256':restored}
    marker.update(stage='completed',receipt=receipt);saved(infra,journal,marker);return receipt
def process(manager,infra):
    if settings(infra) is None:return
    binding=executor_host.manager_binding(infra.config);request={'executor_host_binding':binding}
    for job in manager.call('/vm-snapshots/pending',request):
        path=infra.directory(job['instance_id'])/('vm-snapshot-job-'+identifier(job['id'])+'.json')
        if path.exists() and read(path).get('receipt') is not None:manager.call('/vm-snapshots/'+job['id']+'/complete',{**request,'lease_token':job['lease_token'],'plan_sha256':job['plan_sha256'],'receipt':read(path)['receipt']})
    job=manager.call('/vm-snapshots/claim',request)
    if job:
        try:receipt=execute(manager,infra,job)
        except Exception:
            manager.call('/vm-snapshots/'+job['id']+'/hold',{**request,'lease_token':job['lease_token'],'plan_sha256':job['plan_sha256'],'receipt':{}});return
        manager.call('/vm-snapshots/'+job['id']+'/complete',{**request,'lease_token':job['lease_token'],'plan_sha256':job['plan_sha256'],'receipt':receipt})
