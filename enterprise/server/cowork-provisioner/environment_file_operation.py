"""Bounded workspace IO executed with the sandbox's real UID, never a host path.

Existing-file CAS holds a positive Linux write lease on the original inode.
Contending opens or a changed baseline produce a new conflict file. A broken
lease or a renamed inode after mutation has no completion receipt.
"""
import base64,errno,fcntl,hashlib,json,os,re,signal,stat,sys

LIMIT=262144
def require(ok,message):
    if not ok:raise RuntimeError(message)
def digest(raw):return hashlib.sha256(raw).hexdigest()
def canonical(value):return json.dumps(value,sort_keys=True,separators=(',',':')).encode()
def execute(request):
    require(os.geteuid()==10001 and request['operation'] in ('file_read','file_write'),'original workspace UID/operation required')
    relative=request['relative_path'];parts=relative.split('/')
    require(len(relative.encode())<=1024 and 1<=len(parts)<=32 and all(re.fullmatch('[A-Za-z0-9][A-Za-z0-9._ -]{0,127}',p) and p not in ('.','..') for p in parts),'safe granted relative workspace path required')
    fds=[];parent=os.open('/workspace',os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW);fds.append(parent)
    broken=[False]
    def interrupted(signum,frame):broken[0]=True
    signal.signal(signal.SIGIO,interrupted)
    try:
        for component in parts[:-1]:
            parent=os.open(component,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW,dir_fd=parent);fds.append(parent)
        directories=list(fds)
        def scope():
            current=os.open('/workspace',os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW);opened=[current]
            try:
                for index,original in enumerate(directories):
                    a=os.fstat(current);b=os.fstat(original);require((a.st_ino,a.st_dev)==(b.st_ino,b.st_dev),'original granted directory alias changed')
                    if index<len(parts)-1:current=os.open(parts[index],os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW,dir_fd=current);opened.append(current)
            finally:
                for item in reversed(opened):os.close(item)
        scope()
        name=parts[-1];fd=None;before=None;inode=None;device=None;lease=False;conflict=False;conflict_path=None
        try:fd=os.open(name,(os.O_RDONLY if request['operation']=='file_read' else os.O_RDWR)|os.O_NOFOLLOW,dir_fd=parent)
        except FileNotFoundError:pass
        if fd is not None:
            fds.append(fd);info=os.fstat(fd);require(stat.S_ISREG(info.st_mode) and info.st_nlink==1 and info.st_uid==10001 and info.st_size<=LIMIT,'bounded single-link owned regular file required');inode=info.st_ino;device=info.st_dev
            try:
                fcntl.fcntl(fd,fcntl.F_SETLEASE,fcntl.F_RDLCK if request['operation']=='file_read' else fcntl.F_WRLCK);lease=True
            except OSError as error:
                require(request['operation']=='file_write' and error.errno in (errno.EAGAIN,errno.EACCES,errno.EINVAL,errno.ENOTSUP),'positive file read lease required');conflict=True
            raw=os.read(fd,LIMIT+1);after=os.fstat(fd);require(len(raw)<=LIMIT and (after.st_ino,after.st_size,after.st_mtime_ns)==(info.st_ino,info.st_size,info.st_mtime_ns),'bounded stable file required');before=digest(raw)
        else:require(request['operation']=='file_write','original source file absent')
        expected=request.get('expected_sha256')
        require(expected is None or re.fullmatch('[a-f0-9]{64}',expected),'original SHA baseline required')
        result={'format':1,'operation':request['operation'],'relative_path':relative,'transfer_request_id':request['transfer_request_id'],'transfer_grant_id':request['transfer_grant_id'],'expected_transfer_revision':request['expected_transfer_revision'],'destination_before_sha256':before,'destination_after_sha256':before,'conflict':False,'conflict_relative_path':None}
        if request['operation']=='file_read':
            require(expected is None or before==expected,'original read baseline changed');require(not broken[0] and fcntl.fcntl(fd,fcntl.F_GETLEASE)==fcntl.F_RDLCK,'original read lease broken')
            result.update(content_base64=base64.b64encode(raw).decode(),sha256=before,size=len(raw))
            observed=os.stat(name,dir_fd=parent,follow_symlinks=False);require((observed.st_ino,observed.st_dev,observed.st_size)==(inode,device,len(raw)),'original read path changed')
        else:
            content=base64.b64decode(request['content_base64'],validate=True);require(len(content)<=LIMIT,'bounded transfer content required');content_sha=digest(content)
            conflict=conflict or before!=expected
            if fd is not None and not conflict:
                require(lease and not broken[0] and fcntl.fcntl(fd,fcntl.F_GETLEASE)==fcntl.F_WRLCK,'positive original CAS write lease required')
                observed=os.stat(name,dir_fd=parent,follow_symlinks=False);require((observed.st_ino,observed.st_dev)==(inode,device),'original CAS path changed')
                os.lseek(fd,0,os.SEEK_SET);os.ftruncate(fd,0);view=memoryview(content)
                while view:view=view[os.write(fd,view):]
                os.fsync(fd);os.lseek(fd,0,os.SEEK_SET);require(os.read(fd,LIMIT+1)==content and not broken[0] and fcntl.fcntl(fd,fcntl.F_GETLEASE)==fcntl.F_WRLCK,'original write readback or lease changed after IO')
                observed=os.stat(name,dir_fd=parent,follow_symlinks=False);require((observed.st_ino,observed.st_dev,observed.st_nlink)==(inode,device,1),'original CAS path changed after IO');result['destination_after_sha256']=content_sha
            else:
                # O_EXCL is the actual missing-file CAS. A baseline conflict is
                # always preserved under a new original request-specific alias.
                selected=name if not conflict else name+'.cowork-conflict-'+request['request_id'].replace('-','')
                require(len(selected.encode())<=255,'conflict alias exceeds filesystem bound')
                output=os.open(selected,os.O_RDWR|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600,dir_fd=parent);fds.append(output)
                fcntl.fcntl(output,fcntl.F_SETLEASE,fcntl.F_WRLCK);out_info=os.fstat(output);view=memoryview(content)
                while view:view=view[os.write(output,view):]
                os.fsync(output);os.lseek(output,0,os.SEEK_SET);require(os.read(output,LIMIT+1)==content and not broken[0] and fcntl.fcntl(output,fcntl.F_GETLEASE)==fcntl.F_WRLCK,'new file readback or write lease changed')
                observed=os.stat(selected,dir_fd=parent,follow_symlinks=False);require((observed.st_ino,observed.st_dev,observed.st_nlink)==(out_info.st_ino,out_info.st_dev,1),'new original file path changed')
                if conflict:
                    conflict_path='/'.join([*parts[:-1],selected]);result.update(conflict=True,conflict_relative_path=conflict_path)
                    if fd is not None:
                        original=os.stat(name,dir_fd=parent,follow_symlinks=False);require((original.st_ino,original.st_dev,original.st_nlink)==(inode,device,1) and original.st_size<=LIMIT,'original conflict destination changed path')
                        os.lseek(fd,0,os.SEEK_SET);latest=os.read(fd,LIMIT+1);after=os.fstat(fd);require((after.st_size,after.st_mtime_ns)==(original.st_size,original.st_mtime_ns) and len(latest)<=LIMIT,'conflict destination readback unstable');result['destination_after_sha256']=digest(latest)
                else:result['destination_after_sha256']=content_sha
            os.fsync(parent);result.update(sha256=content_sha,size=len(content))
        scope()
        result['local_receipt']={'request_sha256':digest(canonical(request)),'original_inode':inode,'original_device':device,'observed_inode':observed.st_ino,'observed_device':observed.st_dev,'kernel_write_lease':request['operation']=='file_write' and not conflict,'content_sha256':result['sha256']}
        return result
    finally:
        for fd in reversed(fds):os.close(fd)
if __name__=='__main__':
    raw=sys.stdin.buffer.read(524289);require(len(raw)<=524288,'bounded original file operation required');print(json.dumps(execute(json.loads(raw)),sort_keys=True,separators=(',',':')))
