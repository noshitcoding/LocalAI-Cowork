#!/usr/bin/env python3
"""Root-only helper installed explicitly in one owned VM, never on its parent.

Each workspace has a fully allocated ext4 image and original loop/mount/volume
readbacks. Container disappearance cannot authorize storage release.
"""
import fcntl,hashlib,json,os,re,subprocess,sys,uuid
from pathlib import Path

ROOT=Path('/var/lib/cowork-environment-storage')
def require(ok,message):
    if not ok:raise RuntimeError(message)
def canonical(value):return json.dumps(value,sort_keys=True,separators=(',',':')).encode()
def run(args):return subprocess.run(args,check=True,capture_output=True,timeout=120).stdout
def write(path,value):
    require(not path.is_symlink(),'unsafe storage marker')
    temporary=path.with_suffix('.'+uuid.uuid4().hex+'.tmp')
    with temporary.open('xb') as stream:stream.write(canonical(value));stream.flush();os.fsync(stream.fileno())
    os.chmod(temporary,0o600);os.replace(temporary,path)
    fd=os.open(path.parent,os.O_RDONLY|os.O_DIRECTORY);os.fsync(fd);os.close(fd)
def uid(value):require(str(uuid.UUID(value))==value and uuid.UUID(value).int!=0,'noncanonical original identity');return uuid.UUID(value).hex
def private(path):
    require(not path.is_symlink() and all(not p.is_symlink() for p in path.parents) and path.stat().st_uid==0
            and not path.stat().st_mode&0o022,'private root-owned storage required')
def observed(directory,marker):
    private(directory);private(directory/'disk.ext4')
    require(not (directory/'workspace').is_symlink() and (directory/'workspace').stat().st_uid==10001
            and not (directory/'workspace').stat().st_mode&0o077,'original workspace owner changed')
    disk=directory/'disk.ext4';mount=directory/'workspace'
    require(disk.stat().st_size==marker['bytes'] and disk.stat().st_ino==marker['disk_inode'],'original disk changed')
    loop=json.loads(run(['losetup','--json','--list','--output','NAME,BACK-FILE']))['loopdevices']
    matches=[v for v in loop if v.get('back-file')==str(disk)]
    require(len(matches)==1 and matches[0]['name']==marker['loop_device'],'original loop allocation changed')
    mounted=json.loads(run(['findmnt','--json','--mountpoint',str(mount),'-o','SOURCE,TARGET,FSTYPE,UUID']))['filesystems']
    require(len(mounted)==1 and mounted[0]['source']==marker['loop_device'] and mounted[0]['target']==str(mount)
            and mounted[0]['fstype']=='ext4' and mounted[0]['uuid']==marker['filesystem_uuid'],'original filesystem mount changed')
    volume=json.loads(run(['docker','volume','inspect',marker['volume_name']]))[0]
    require(volume['Labels']==marker['labels'] and volume['Options']=={'type':'none','o':'bind','device':str(mount)},'original Docker workspace volume changed')
    stat=os.statvfs(mount);require(stat.f_blocks*stat.f_frsize<=marker['bytes'],'actual filesystem quota exceeds original allocation')
    return {**marker,'quota_bytes':stat.f_blocks*stat.f_frsize,'volume_mount_verified':True}
def execute(request):
    require(os.geteuid()==0 and request['format']==1,'owned guest root helper required')
    for field in ('job_id','environment_id','instance_id','organisation_id','vm_id'):uid(request[field])
    require(request['action'] in ('ensure','bind','inspect','release','read_release') and type(request['storage_gib']) is int and 1<=request['storage_gib']<=1048576,'invalid storage operation')
    require(Path('/sys/class/dmi/id/product_uuid').read_text().strip().lower()==request['vm_id'],'original guest hardware UUID changed')
    info=json.loads(run(['docker','info','--format','{{json .}}']))
    require(info['ID']==request['daemon_id'] and 'cowork.trust-domain='+request['organisation_id'] in info.get('Labels',[]),'original guest daemon changed')
    ROOT.mkdir(mode=0o700,exist_ok=True);private(ROOT)
    with (ROOT/'lock').open('a+b') as lock:
        os.chmod(ROOT/'lock',0o600);fcntl.flock(lock,fcntl.LOCK_EX)
        identity=uid(request['instance_id'])+'-'+('shared' if request['shared_workspace'] else uid(request['environment_id']))
        directory=ROOT/identity;receipt=ROOT/(uid(request['job_id'])+'.json')
        original=hashlib.sha256(canonical(request)).hexdigest()
        if receipt.exists() and request['action'] not in ('inspect','read_release'):
            prior=json.loads(receipt.read_bytes());require(prior['request_sha256']==original,'original guest storage request differs')
            if request['action']=='ensure':
                require(directory.is_dir(),'original ensured workspace disappeared')
                observed(directory,json.loads((directory/'storage.json').read_bytes()))
            elif prior['physical_allocation_released']:require(not directory.exists(),'released storage reappeared')
            return prior
        if directory.exists():
            private(directory);marker=json.loads((directory/'storage.json').read_bytes())
            require(marker['instance_id']==request['instance_id'] and marker['organisation_id']==request['organisation_id']
                    and marker['vm_id']==request['vm_id'] and marker['daemon_id']==request['daemon_id'] and marker['bytes']==request['storage_gib']*1073741824,'shared/original storage scope changed')
            actual=observed(directory,marker)
        else:
            if request['action']=='read_release':
                tombstone=json.loads((ROOT/(identity+'.released.json')).read_bytes())
                require(tombstone['storage_uid']==request['expected_storage_uid'] and not run(['docker','volume','ls','--filter','name=^cowork-env-storage-'+identity+'$','--format','{{.Name}}']).strip(),'original released allocation reappeared')
                return {'format':1,'job_id':request['job_id'],'storage_uid':tombstone['storage_uid'],'quota_bytes':request['storage_gib']*1073741824,'reference_released':True,'physical_allocation_released':True}
            require(request['action']=='ensure','original workspace allocation missing')
            directory.mkdir(mode=0o700);(directory/'workspace').mkdir(mode=0o700)
            disk=directory/'disk.ext4';disk.touch(mode=0o600,exist_ok=False)
            # Full allocation fails before exposing the workspace if the guest
            # cannot reserve the requested physical bytes.
            run(['fallocate','--length',str(request['storage_gib']*1073741824),str(disk)])
            filesystem=str(uuid.uuid4());run(['mkfs.ext4','-q','-m','0','-U',filesystem,str(disk)])
            loop=run(['losetup','--find','--show',str(disk)]).decode().strip();require(re.fullmatch('/dev/loop[0-9]+',loop),'loop ACK unknown')
            run(['mount','-t','ext4','-o','nodev,nosuid',loop,str(directory/'workspace')]);os.chown(directory/'workspace',10001,10001);os.chmod(directory/'workspace',0o700)
            labels={'cowork.environment-storage':identity,'cowork.instance':request['instance_id'],'cowork.organisation':request['organisation_id'],'cowork.storage-uid':filesystem}
            volume='cowork-env-storage-'+identity
            require(not run(['docker','volume','ls','--filter','name=^'+volume+'$','--format','{{.Name}}']).strip(),'foreign workspace volume exists')
            args=['docker','volume','create','--driver','local','--opt','type=none','--opt','o=bind','--opt','device='+str(directory/'workspace')]
            for key,value in sorted(labels.items()):args+=['--label',key+'='+value]
            require(run([*args,volume]).decode().strip()==volume,'workspace volume create ACK unknown')
            marker={'format':1,'instance_id':request['instance_id'],'organisation_id':request['organisation_id'],'vm_id':request['vm_id'],'daemon_id':request['daemon_id'],
                    'bytes':request['storage_gib']*1073741824,'disk_inode':disk.stat().st_ino,'loop_device':loop,'filesystem_uuid':filesystem,'volume_name':volume,'labels':labels,'refs':{}}
            write(directory/'storage.json',marker);actual=observed(directory,marker)
        expected=request.get('expected_storage_uid')
        if expected is not None:require(marker['filesystem_uuid']==expected,'original workspace UID changed')
        environment=request['environment_id']
        if request['action']=='ensure':
            require(environment not in marker['refs'] or expected is not None,'workspace reference already exists without original receipt')
            prior=marker['refs'].get(environment)
            if prior is not None:require(not run(['docker','ps','-aq','--no-trunc','--filter','id='+prior]).strip(),'original stopped workspace runtime still exists')
            marker['refs'][environment]=None;write(directory/'storage.json',marker)
        elif request['action']=='bind':
            container=request['container_id'];require(re.fullmatch('[a-f0-9]{64}',container) and environment in marker['refs'],'original workspace create ACK missing')
            actual_container=json.loads(run(['docker','inspect',container]))[0]
            require(actual_container['Id']==container and actual_container['Config']['Labels']['cowork.environment']==environment
                    and any(m.get('Name')==marker['volume_name'] and m['Destination']=='/workspace' for m in actual_container['Mounts']),'original runtime workspace binding changed')
            marker['refs'][environment]=container;write(directory/'storage.json',marker)
        elif request['action']=='release':
            require(environment in marker['refs'] and expected==marker['filesystem_uuid'],'original workspace reference missing')
            containers=run(['docker','ps','-aq','--no-trunc','--filter','volume='+marker['volume_name']]).decode().splitlines()
            others={key:value for key,value in marker['refs'].items() if key!=environment}
            require(set(containers)<=set(value for value in others.values() if value is not None),'original workspace still referenced by unacknowledged runtime')
            del marker['refs'][environment]
            if marker['refs']:write(directory/'storage.json',marker)
            else:
                require(run(['docker','volume','rm',marker['volume_name']]).decode().strip()==marker['volume_name'],'volume deletion ACK unknown')
                run(['umount',str(directory/'workspace')]);run(['losetup','--detach',marker['loop_device']])
                require(not any(v.get('back-file')==str(directory/'disk.ext4') for v in json.loads(run(['losetup','--json','--list','--output','BACK-FILE']))['loopdevices']),'original disk still attached')
                (directory/'disk.ext4').unlink();(directory/'storage.json').unlink();(directory/'workspace').rmdir();directory.rmdir()
                write(ROOT/(identity+'.released.json'),{'storage_uid':marker['filesystem_uuid'],'original_release_job':request['job_id']})
        elif request['action']=='read_release':require(environment not in marker['refs'],'original workspace reference reappeared')
        result={'format':1,'job_id':request['job_id'],'request_sha256':original,'storage_uid':marker['filesystem_uuid'],
                'volume_name':marker['volume_name'],'quota_bytes':actual['quota_bytes'],'reference_released':request['action'] in ('release','read_release'),
                'physical_allocation_released':request['action']=='release' and not marker['refs'],'mount_verified':request['action']!='release'}
        write(receipt,result);return result
if __name__=='__main__':
    raw=sys.stdin.buffer.read(65537);require(len(raw)<=65536,'storage request too large');print(json.dumps(execute(json.loads(raw))))
