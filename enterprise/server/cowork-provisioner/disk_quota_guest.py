#!/usr/bin/env python3
"""Root-only original, fully allocated ext4 disk lifecycle. No path adoption.

Input is an explicit operator request from the original management disk plan.
An interrupted mutating syscall is never repeated by observe or by a new UUID.
The root journal is separate from the data disk so ENOSPC cannot erase ownership.
"""
import fcntl, hashlib, json, os, re, stat, subprocess, sys, uuid
from pathlib import Path

ROOT = Path('/var/lib/cowork-disk-quotas')
MAX_BYTES = 1024 ** 5

def require(ok, message):
    if not ok: raise RuntimeError(message)
def canonical(v): return json.dumps(v, sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode()
def sha(v): return hashlib.sha256(canonical(v)).hexdigest()
def uid(v):
    require(type(v) is str and str(uuid.UUID(v)) == v and uuid.UUID(v).int != 0, 'canonical nonnil disk identity required')
    return uuid.UUID(v).hex
def private(p, directory=False):
    p=Path(p); s=p.lstat()
    require(p.is_absolute() and not p.is_symlink() and all(not x.is_symlink() for x in p.parents)
        and (stat.S_ISDIR(s.st_mode) if directory else stat.S_ISREG(s.st_mode)) and s.st_uid==0
        and not s.st_mode & 0o077 and (directory or s.st_nlink==1), 'root-only original disk path required')
    return p
def run(argv):
    try: return subprocess.run(argv, check=True, capture_output=True, timeout=120).stdout
    except (OSError, subprocess.SubprocessError): raise RuntimeError('disk command failed: '+Path(argv[0]).name) from None
def save(p,v):
    tmp=p.with_name(p.name+'.'+uuid.uuid4().hex+'.tmp')
    fd=os.open(tmp,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
    with os.fdopen(fd,'wb') as out: out.write(canonical(v)); out.flush(); os.fsync(out.fileno())
    os.replace(tmp,p)
    fd=os.open(p.parent,os.O_DIRECTORY|os.O_RDONLY|os.O_NOFOLLOW)
    try: os.fsync(fd)
    finally: os.close(fd)
def read(p):
    private(p); require(p.stat().st_size<=1048576,'disk journal exceeds bound')
    return json.loads(p.read_bytes())
def tuple_stat(p):
    s=private(p).stat(); return {'device':s.st_dev,'inode':s.st_ino,'bytes':s.st_size}
def filesystem(mount, binding):
    s=mount.lstat();require(mount.is_dir() and not mount.is_symlink() and s.st_uid==0 and not s.st_mode&0o022
        and all(not x.is_symlink() for x in mount.parents),'original root-owned filesystem mount required')
    values=json.loads(run(['findmnt','--json','--mountpoint',str(mount),'-o','SOURCE,TARGET,FSTYPE,UUID']))['filesystems']
    require(len(values)==1 and values[0]['target']==str(mount) and values[0]['fstype']=='ext4'
        and values[0]['uuid']==binding['filesystem_uuid'], 'original quota filesystem mount changed')
    loop=values[0]['source']; require(re.fullmatch(r'/dev/loop[0-9]+',loop),'original loop device required')
    original=json.loads(run(['losetup','--json',loop]))['loopdevices']
    require(len(original)==1 and original[0]['back-file']==binding['disk_file'],'original loop backing inode path changed')
    require(tuple_stat(Path(binding['disk_file']))==binding['disk_stat'],'original fully allocated disk inode changed')
    disk=Path(binding['disk_file']).stat()
    require(disk.st_blocks*512>=binding['size_bytes'],'quota disk is sparse or deallocated')
    s=os.statvfs(mount); capacity=s.f_blocks*s.f_frsize
    require(0<capacity<=binding['size_bytes'],'actual filesystem exceeds original hard disk bound')
    return {'filesystem_uuid':binding['filesystem_uuid'],'mount':str(mount),'mount_device':mount.stat().st_dev,
        'capacity_bytes':capacity,'available_bytes':s.f_bavail*s.f_frsize,'used_bytes':(s.f_blocks-s.f_bfree)*s.f_frsize,
        'disk_stat':binding['disk_stat'],'size_bytes':binding['size_bytes'],'loop_device':loop,'fsid':s.f_fsid}
def no_nested_mounts(mount):
    values=json.loads(run(['findmnt','--json','-R',str(mount),'-o','TARGET']))['filesystems']
    require(len(values)==1 and not values[0].get('children'),'nested mount bypasses hard quota')
def physical_scope(p):
    for k in ('id','instance_id','organisation_id','machine_id','filesystem_uuid'): uid(p[k])
    require(p['kind'] in ('guest','controller','snapshot','environment') and type(p['size_bytes']) is int
        and 1024**3<=p['size_bytes']<=MAX_BYTES,'invalid original physical disk reservation')
    actual=Path('/sys/class/dmi/id/product_uuid').read_text().strip().lower()
    require(actual==p['machine_id'],'actual original machine differs')
    if p['kind']=='guest':
        actual=json.loads(run(['docker','info','--format','{{json .}}']))
        require(actual['ID']==p['daemon_id'],'actual original Docker engine differs')
def initialise(request,path):
    p=request['placement']; marker=path/'allocation.json'
    if marker.exists():
        old=read(marker); require(old['placement']==p and old['operation_id']==request['operation_id'],'original disk allocation cannot be adopted')
        require(old['state']=='mounted','interrupted disk allocation requires original physical reconciliation; no resend')
        return observe(request,path,old)
    require(not any(path.iterdir()),'existing disk path cannot be adopted')
    disk=path/'disk.ext4'; mount=path/'mount'
    fd=os.open(disk,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
    os.close(fd)
    old={'format':'cowork-original-disk-allocation-v1','operation_id':request['operation_id'],'placement':p,
         'state':'allocation_admitted','disk_file':str(disk),'size_bytes':p['size_bytes'],
         'filesystem_uuid':p['filesystem_uuid'],'disk_stat':tuple_stat(disk)}
    save(marker,old) # durable once-dispatch before fallocate/mkfs/mount
    run(['fallocate','-l',str(p['size_bytes']),'--',str(disk)])
    old['disk_stat']=tuple_stat(disk); old['state']='allocated'; save(marker,old)
    run(['mkfs.ext4','-q','-F','-m','0','-U',p['filesystem_uuid'],str(disk)])
    mount.mkdir(mode=0o700)
    loop=run(['losetup','--find','--show','--nooverlap',str(disk)]).decode().strip()
    require(re.fullmatch(r'/dev/loop[0-9]+',loop),'invalid original loop allocation')
    old['loop_device']=loop; old['state']='mount_admitted'; save(marker,old)
    run(['mount','-t','ext4','-o','nodev,nosuid',loop,str(mount)])
    old['state']='mounted'; save(marker,old)
    return observe(request,path,old)
def observe(request,path,old=None):
    p=request['placement']; old=read(path/'allocation.json') if old is None else old
    require(old['placement']==p and old['state']=='mounted','original mounted allocation required; absence is not release')
    actual=filesystem(path/'mount',old); no_nested_mounts(path/'mount')
    return {'format':'cowork-disk-observation-v1','placement':p,'challenge':request.get('challenge'),'allocation_operation_id':old['operation_id'],
        'allocation_sha256':sha(old),'physical':actual,'source_sha256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}
def reconcile(request,path):
    """Explicitly observe one interrupted original mount; never run allocation IO."""
    old=read(path/'allocation.json'); p=request['placement']
    require(old['placement']==p and request['allocation_operation_id']==old['operation_id']
        and old['state'] in ('allocated','mount_admitted'),'original interrupted disk scope differs')
    require(tuple_stat(Path(old['disk_file']))==old['disk_stat'],'original interrupted disk inode differs')
    filesystem(path/'mount',old); no_nested_mounts(path/'mount')
    old['state']='mounted'; save(path/'allocation.json',old)
    return observe(request,path,old)
def placement_paths(mount, values, snapshot=None):
    require(type(values) is dict and set(values)=={'state','backups','scratch','snapshot_store'} ,'all original controller durable paths required')
    for name,value in values.items():
        if name=='snapshot_store' and snapshot is not None:
            require(snapshot['kind']=='snapshot' and Path(value)==ROOT/uid(snapshot['id'])/'mount','snapshot store differs from original reserved placement')
            original=read(ROOT/uid(snapshot['id'])/'allocation.json');require(original['placement']==snapshot and original['state']=='mounted','original snapshot allocation required')
            filesystem(Path(value),original);continue
        p=Path(value); require(p.is_absolute() and not p.is_symlink() and p.resolve().is_relative_to(mount)
            and all(not item.is_symlink() for item in p.parents) and p.is_dir()
            and p.stat().st_dev==mount.stat().st_dev,'controller data is outside hard filesystem')
    no_nested_mounts(mount)
def environment_workspace(source,p,mount):
    root=Path('/var/lib/cowork-environment-storage');private(root,True)
    require(source.is_relative_to(root) and source.name=='workspace','unregistered external runtime mount')
    directory=source.parent;marker=read(directory/'storage.json');disk=directory/'disk.ext4'
    require(marker['instance_id']==p['instance_id'] and marker['organisation_id']==p['organisation_id']
        and marker['vm_id']==p['machine_id'] and marker['daemon_id']==p['daemon_id'] and marker['format']==1,
        'original environment-storage owner differs')
    require(disk.stat().st_dev==mount.stat().st_dev and disk.stat().st_ino==marker['disk_inode']
        and disk.stat().st_size==marker['bytes'] and disk.stat().st_blocks*512>=marker['bytes'],
        'environment backing disk bypasses physical quota')
    matches=json.loads(run(['findmnt','--json','--mountpoint',str(source),'-o','SOURCE,TARGET,FSTYPE,UUID']))['filesystems']
    require(len(matches)==1 and matches[0]['source']==marker['loop_device'] and matches[0]['uuid']==marker['filesystem_uuid']
        and matches[0]['fstype']=='ext4' and os.statvfs(source).f_blocks*os.statvfs(source).f_frsize<=marker['bytes'],
        'original environment filesystem differs')
    loops=json.loads(run(['losetup','--json',marker['loop_device']]))['loopdevices']
    require(len(loops)==1 and loops[0]['back-file']==str(disk),'original environment loop source differs')
    return True
def service(info, instance, kind, mount, original):
    require(info['Id']==original and info['Config']['Labels'].get('cowork.disk.instance')==instance
        and info['Config']['Labels'].get('cowork.disk.service')==kind and
        (info['State']['Running'] is True or info['State']['Running'] is False and info['State']['Paused'] is False
            and info['State']['Restarting'] is False and info['State']['Pid']==0),
        'original dedicated data service differs')
    for entry in info['Mounts']:
        if entry['RW'] and entry['Type']!='tmpfs':
            p=Path(entry['Source']);require(not p.is_symlink() and p.resolve().is_relative_to(mount)
                and p.stat().st_dev==mount.stat().st_dev,'data service writable mount bypasses hard filesystem')
    dest='/var/lib/postgresql/data' if kind=='postgres' else '/data'
    matches=[entry for entry in info['Mounts'] if entry['Destination']==dest and entry['RW']]
    require(len(matches)==1,'original data-service storage mount required')
    return Path(matches[0]['Source'])
def topology(request,path):
    observation=observe(request,path); p=request['placement']; mount=path/'mount'; t=request['topology']
    if p['kind']=='controller':
        placement_paths(mount,t['paths'],t.get('snapshot_placement')); observation['topology']={'paths':t['paths']}
    elif p['kind']=='guest':
        info=json.loads(run(['docker','info','--format','{{json .}}']))
        root=Path(info['DockerRootDir'])
        require(root.resolve().is_relative_to(mount) and root.stat().st_dev==mount.stat().st_dev
            and not root.is_symlink(),'Docker volumes, layers and logs are outside hard quota')
        pgid=t['postgres_container_id']; s3id=t['minio_container_id']
        require(all(re.fullmatch('[a-f0-9]{64}',v) for v in (pgid,s3id)) and pgid!=s3id,'original data-service container IDs required')
        pg,s3=json.loads(run(['docker','inspect',pgid,s3id]))
        data=service(pg,p['instance_id'],'postgres',mount,pgid)
        service(s3,p['instance_id'],'minio',mount,s3id)
        wal=data/'pg_wal';require(wal.is_dir() and not wal.is_symlink() and wal.stat().st_dev==mount.stat().st_dev,'PostgreSQL WAL bypasses hard filesystem')
        actual=run(['docker','exec',pgid,'psql','-U','postgres','-d','postgres','-Atc','SHOW data_directory']).decode().strip() if pg['State']['Running'] else '/var/lib/postgresql/data' if 'PGDATA=/var/lib/postgresql/data' in pg['Config']['Env'] else None
        require(actual=='/var/lib/postgresql/data','actual PostgreSQL data directory differs')
        command=s3['Config']['Cmd'];require(command[:2]==['server','/data'],'MinIO object directory differs')
        identities=run(['docker','ps','-aq','--no-trunc']).decode().splitlines()
        require(len(identities)<=20000 and len(set(identities))==len(identities),'complete bounded Docker inventory required')
        for offset in range(0,len(identities),64):
            for container in json.loads(run(['docker','inspect',*identities[offset:offset+64]])):
                for entry in container['Mounts']:
                    if entry['RW'] and entry['Type']!='tmpfs' and entry['Source']!='/var/run/docker.sock':
                        source=Path(entry['Source']);require(source.resolve().is_relative_to(mount)
                            and source.stat().st_dev==mount.stat().st_dev or environment_workspace(source,p,mount),
                            'runtime writable mount bypasses total disk quota')
        require(set(run(['docker','ps','-aq','--no-trunc']).decode().splitlines())==set(identities),'Docker inventory changed during disk observation')
        observation['topology']={'docker_root':str(root),'postgres_container_id':pgid,'minio_container_id':s3id,
            'database_host':t['database_host'],'s3_endpoint':t['s3_endpoint']}
    else:
        observation['topology']={'reserved_pool_bytes':p['size_bytes']}
    return observation
def configure(request,path):
    """Explicit empty-daemon installation. Never migrate old Docker data."""
    p=request['placement'];require(p['kind']=='guest' and request.get('confirmation')=='INSTALL_EMPTY_ORIGINAL_DOCKER_DISK','explicit empty-daemon configuration required')
    observation=observe(request,path);marker=path/'docker-configuration.json'
    if marker.exists():
        old=read(marker);require(old['request_sha256']==sha(request) and old['state']=='configured','original Docker configuration unknown; no restart resend')
        return old
    require(not run(['docker','ps','-aq']).strip() and not run(['docker','volume','ls','-q']).strip()
        and not run(['docker','images','-q']).strip(),'old Docker resources require explicit migration; no adoption')
    root=path/'mount';config=Path('/etc/docker/daemon.json');old=json.loads(config.read_bytes()) if config.exists() else {}
    require(type(old) is dict and not config.is_symlink(),'original Docker configuration required')
    previous=json.loads(run(['docker','info','--format','{{json .}}']))
    identity_file=Path(previous['DockerRootDir'])/'engine-id'
    info=identity_file.lstat();require(identity_file.is_file() and not identity_file.is_symlink() and info.st_uid==0 and not info.st_mode&0o022
        and info.st_nlink==1 and all(not p.is_symlink() for p in identity_file.parents),'original root-owned engine identity required');engine_identity=identity_file.read_bytes()
    require(len(engine_identity)<=256 and engine_identity.decode().strip()==p['daemon_id'],'actual original Docker engine identity file required')
    old['data-root']=str(root/'docker');old['storage-driver']='overlay2'
    original={'request_sha256':sha(request),'observation':observation,'state':'configuration_admitted','original_configuration_sha256':sha(old)}
    save(marker,original)
    run(['systemctl','stop','docker.service','docker.socket'])
    for name,owner,mode in [('postgres',999,0o700),('minio',1000,0o700),('docker',0,0o700),('runtime-observation',0,0o755),('environments',0,0o700)]:
        folder=root/name;folder.mkdir(mode=mode);os.chown(folder,owner,owner);os.chmod(folder,mode)
    # Read-only runtime mount exposes only an empty statvfs directory, never
    # database, Docker layers, server keys or operator state.
    fd=os.open(root/'docker'/'engine-id',os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
    with os.fdopen(fd,'wb') as out:out.write(engine_identity);out.flush();os.fsync(out.fileno())
    os.chmod(root,0o711);save(config,old)
    environment=Path('/var/lib/cowork-environment-storage')
    if environment.exists():require(environment.is_dir() and not environment.is_symlink() and not any(environment.iterdir()),'old environment allocations need explicit migration')
    else:environment.mkdir(mode=0o700)
    run(['mount','--bind',str(root/'environments'),str(environment)])
    run(['systemctl','start','docker.service'])
    actual=json.loads(run(['docker','info','--format','{{json .}}']))
    require(actual['ID']==p['daemon_id'] and actual['DockerRootDir']==str(root/'docker'),'original Docker engine/root changed after explicit installation')
    original.update(state='configured',docker_root=str(root/'docker'));save(marker,original);return original
def directories(request,path):
    require(request['placement']['kind']=='controller','controller path installation only')
    observe(request,path);mount=path/'mount';paths=request['topology']['paths']
    require(set(paths)=={'state','backups','scratch','snapshot_store'},'complete bounded controller paths required')
    for name,value in paths.items():
        if name=='snapshot_store' and request['topology'].get('snapshot_placement') is not None:continue
        p=Path(value);require(p.is_absolute() and p.is_relative_to(mount) and not p.is_symlink()
            and all(not x.is_symlink() for x in p.parents),'controller path escapes original physical disk')
        p.mkdir(mode=0o700,parents=True,exist_ok=True);os.chmod(p,0o700)
    placement_paths(mount,paths,request['topology'].get('snapshot_placement'));return observe(request,path)
def release(request,path):
    original=read(path/'allocation.json'); p=request['placement']; require(original['placement']==p,'original release scope differs')
    output=path/('release-'+uid(request['operation_id'])+'.json')
    if output.exists():
        result=read(output); require(result['request_sha256']==sha(request),'original release request conflict'); return result
    require(request.get('confirmation')=='DESTROY_ORIGINAL_PHYSICAL_DISK' and original['state']=='mounted','explicit original disk release required')
    observation=observe(request,path); mount=path/'mount'; disk=Path(original['disk_file'])
    intent={'format':'cowork-disk-release-intent-v1','request_sha256':sha(request),'operation_id':request['operation_id'],
        'placement':p,'original_allocation_sha256':sha(original),'observation':observation,'state':'release_admitted'}
    save(output,intent) # durable before even the original Docker stop
    if p['kind']=='guest':
        require(not run(['docker','ps','-aq']).strip() and not run(['docker','volume','ls','-q']).strip(),
            'all original Docker resources must be positively retired first')
        run(['systemctl','stop','docker.service','docker.socket'])
        require(not any((proc/'comm').read_text().strip()=='dockerd' for proc in Path('/proc').iterdir()
            if proc.name.isdigit() and (proc/'comm').exists()),'Docker daemon remains alive')
    # Open file/cwd/exe handles and mounts, not absent archive names, decide quiescence.
    for proc in Path('/proc').iterdir():
        if not proc.name.isdigit(): continue
        try:
            handles=[proc/'cwd',proc/'root',proc/'exe',*(proc/'fd').iterdir()]
            for handle in handles:
                try: target=Path(os.readlink(handle))
                except FileNotFoundError: continue
                require(not target.is_absolute() or not target.is_relative_to(mount),'original disk has live process handles')
        except (PermissionError,OSError): raise RuntimeError('complete physical handle inventory required') from None
    run(['umount','--',str(mount)])
    run(['losetup','--detach',observation['physical']['loop_device']])
    require(not json.loads(run(['losetup','--json','-j',str(disk)]))['loopdevices'],'original disk remains attached')
    require(tuple_stat(disk)==original['disk_stat'],'original disk changed before unlink')
    disk.unlink(); fd=os.open(path,os.O_DIRECTORY|os.O_RDONLY)
    try: os.fsync(fd)
    finally: os.close(fd)
    intent.update(state='released',physical_release_verified=True,original_inode=original['disk_stat'])
    save(output,intent); original['state']='released'; save(path/'allocation.json',original)
    return intent
def main():
    require(os.name=='posix' and os.geteuid()==0,'physical disk helper requires Linux root')
    raw=sys.stdin.buffer.read(1048577); require(len(raw)<=1048576,'original disk request exceeds bound')
    request=json.loads(raw); require(request['format']=='cowork-disk-request-v1','unsupported disk request')
    uid(request['operation_id']); p=request['placement'];uid(p['id']);uid(p['machine_id'])
    require(Path('/sys/class/dmi/id/product_uuid').read_text().strip().lower()==p['machine_id'],'original machine differs')
    ROOT.mkdir(mode=0o700,exist_ok=True); private(ROOT,True)
    path=ROOT/uid(p['id']); path.mkdir(mode=0o700,exist_ok=True); private(path,True)
    fd=os.open(ROOT/'.lock',os.O_RDWR|os.O_CREAT|os.O_NOFOLLOW,0o600)
    with os.fdopen(fd,'a+') as lock:
        fcntl.flock(lock,fcntl.LOCK_EX)
        action=request['action']
        if action=='release' and (path/('release-'+uid(request['operation_id'])+'.json')).exists():
            original=read(path/('release-'+uid(request['operation_id'])+'.json'))
            require(original['request_sha256']==sha(request),'original release request conflict')
            sys.stdout.buffer.write(canonical(original)+b'\n');return
        physical_scope(p)
        require(action in ('init','observe','reconcile','topology','configure','directories','release'),'explicit disk action required')
        result={'init':initialise,'observe':observe,'reconcile':reconcile,'topology':topology,'configure':configure,'directories':directories,'release':release}[action](request,path)
        sys.stdout.buffer.write(canonical(result)+b'\n')
if __name__=='__main__':
    try: main()
    except Exception: sys.stderr.write('original disk operation failed; inspect original journal, no resend\n'); sys.exit(1)
