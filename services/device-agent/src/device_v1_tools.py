"""One immutable, supervised native operation; never a V1 process or ambient credential."""
import fnmatch,hashlib,http.client,ipaddress,json,os,pathlib,re,shutil,socket,ssl,subprocess,sys,urllib.parse,threading,time
root=pathlib.Path(sys.argv[1]).resolve(strict=True);request=json.loads(pathlib.Path(sys.argv[2]).read_text(encoding='utf-8'));op=request['tool'];a=request['arguments'];LIMIT=32*1024*1024
def bounded_program(cmd,cwd,env,seconds):
    child=subprocess.Popen(cmd,cwd=cwd,env=env,stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
    chunks=[bytearray(),bytearray()];overflow=threading.Event();failed=threading.Event();guard=threading.Lock();total=0
    def drain(stream,index):
        nonlocal total
        try:
            while True:
                part=stream.read(32768)
                if not part:break
                with guard:
                    total+=len(part)
                    if total>2*1024*1024:overflow.set();break
                    chunks[index].extend(part)
        except OSError:failed.set()
        finally:stream.close()
    readers=[threading.Thread(target=drain,args=(stream,i),daemon=True) for i,stream in enumerate((child.stdout,child.stderr))]
    for thread in readers:thread.start()
    deadline=time.monotonic()+seconds
    try:
        while child.poll() is None:
            if overflow.is_set() or failed.is_set() or time.monotonic()>=deadline:raise ValueError('program_output_or_time_bound')
            time.sleep(.025)
        for thread in readers:thread.join(max(0,deadline-time.monotonic()))
        if overflow.is_set() or failed.is_set() or any(thread.is_alive() for thread in readers):raise ValueError('program_capture_incomplete')
        if child.returncode:raise ValueError('program_exit')
        return bytes(chunks[0]),bytes(chunks[1])
    finally:
        if child.poll() is None:child.kill()
        child.wait(timeout=5)

def scope_allowed(value):
    policy=a.get('_crew_policy') or {};roots=policy.get('roots')
    if roots is not None and not any(path=='.' or value==path or kind=='folder' and value.startswith(path+'/') for path,kind in roots):return False
    for rule in policy.get('deny_rules',[]):
        tool,separator,target=rule.partition(':');target=target if separator else '*'
        if fnmatch.fnmatchcase(op.lower(),tool.lower()) and fnmatch.fnmatchcase(value.lower(),target.lower()):return False
    return True
def checked(value,existing=False):
    if not isinstance(value,str) or not value or len(value)>1000 or any(c in value for c in '\\:\x00\r\n') or any(p in ('','.','..') for p in value.split('/')):raise ValueError('workspace_path')
    if not scope_allowed(value):raise ValueError('original_crew_path_policy')
    p=root/pathlib.PurePosixPath(value)
    for parent in [p,*p.parents]:
        if parent==root.parent:break
        if parent.exists() and (parent.is_symlink() or getattr(parent.lstat(),'st_file_attributes',0)&0x400):raise ValueError('workspace_alias')
    resolved=p.resolve(strict=existing)
    if not resolved.is_relative_to(root):raise ValueError('workspace_escape')
    return p
def fingerprint(p):
    if not p.is_file() or p.stat().st_nlink!=1 or p.stat().st_size>LIMIT:raise ValueError('workspace_file')
    with p.open('rb') as f:raw=f.read(LIMIT+1)
    if len(raw)>LIMIT:raise ValueError('workspace_size')
    return {'path':p.relative_to(root).as_posix(),'bytes':len(raw),'sha256':hashlib.sha256(raw).hexdigest()}
def inventory():
    files=[];stack=[root];total=0
    while stack:
        for p in sorted(stack.pop().iterdir()):
            if p.is_symlink() or getattr(p.lstat(),'st_file_attributes',0)&0x400:raise ValueError('workspace_alias')
            if p.is_dir():stack.append(p)
            elif p.is_file():
                if not scope_allowed(p.relative_to(root).as_posix()):continue
                pin=fingerprint(p);files.append(pin);total+=pin['bytes']
                if len(files)>2000 or total>256*1024*1024:raise ValueError('workspace_inventory_bound')
            else:raise ValueError('workspace_special')
    return files
def url(value):
    u=urllib.parse.urlsplit(value)
    if u.scheme!='https' or not u.hostname or u.username or u.password or u.fragment:raise ValueError('web_url')
    ips=sorted({row[4][0] for row in socket.getaddrinfo(u.hostname,u.port or 443,type=socket.SOCK_STREAM)})
    if u.hostname not in request.get('allowed_hosts',[]) and any(not ipaddress.ip_address(n).is_global for n in ips):raise ValueError('web_private_address')
    return u,ips
def destination(value):
    p=checked(value)
    if p.exists():raise ValueError('destination_exists')
    return {'path':value,'absent':True}
def git_policy():
    for p in root.rglob('.git'):
        if not p.is_dir() or p.is_symlink():raise ValueError('git_foreign_worktree_pointer')
        config=p/'config'
        if config.exists():
            raw=config.read_bytes()
            if len(raw)>1024*1024:raise ValueError('git_config_bound')
            text=raw.decode('utf-8')
            if re.search(r'(?im)^\s*\[\s*(include|includeif|filter|gpg|diff)\b',text) or re.search(r'(?im)^\s*(worktree|sshcommand|fsmonitor)\s*=',text):raise ValueError('git_ambient_program_or_foreign_worktree')
    args=a.get('arguments',[])
    if not isinstance(args,list) or any(not isinstance(v,str) or any(v==flag or v.startswith(flag+'=') for flag in ['--exec','--ext-diff','--textconv','--recurse-submodules','--config','--config-env','--upload-pack','--receive-pack','--separate-git-dir']) for v in args):raise ValueError('git_indirect_program_forbidden')
if request['phase']=='prepare':
    pins=[];outputs=[]
    if op in {'read_file','edit_file','delete_file'}:pins.append(fingerprint(checked(a['path'],True)))
    if op in {'copy_path','move_path'}:pins.append(fingerprint(checked(a['source'],True)));outputs.append(destination(a['destination']))
    if op=='create_directory':outputs.append(destination(a['path']))
    if op=='git' and a.get('arguments',[None])[0]=='clone':outputs.append(destination(a['destination']))
    if op in {'glob','grep','bash','git'}:pins=inventory()
    if op=='git':git_policy()
    if op in {'web_fetch','web_search'}:
        target=a['url'] if op=='web_fetch' else request['web_search_url'].replace('{query}',urllib.parse.quote(a['query'],safe=''))
        u,ips=url(target);a['resolved_url']=target;a['resolved_ips']=ips
    if op in {'bash','git'}:
        program=request['program'];p=pathlib.Path(program)
        if not p.is_absolute() or not p.is_file():raise ValueError('program_missing')
        pins.append({'program':program,'bytes':p.stat().st_size,'sha256':hashlib.sha256(p.read_bytes()).hexdigest()})
    print(json.dumps({'ok':True,'tool':op,'arguments':a,'inputs':pins,'outputs':outputs},ensure_ascii=False));sys.exit(0)
for pin in request['pins']:
    if 'program' in pin:
        p=pathlib.Path(pin['program']);actual={'program':str(p),'bytes':p.stat().st_size,'sha256':hashlib.sha256(p.read_bytes()).hexdigest()}
    else:actual=fingerprint(checked(pin['path'],True))
    if actual!=pin:raise ValueError('original_input_changed')
for pin in request.get('outputs',[]):destination(pin['path'])
result={}
if op=='read_file':
    raw=checked(a['path'],True).read_bytes();lines=raw.decode('utf-8').splitlines();start=a.get('start_line',1);maximum=a.get('max_lines',400)
    if type(start) is not int or type(maximum) is not int or start<1 or not 1<=maximum<=2000:raise ValueError('read_line_bounds')
    selected=lines[start-1:start-1+maximum];text='\n'.join(selected)
    if len(text.encode())>2*1024*1024:raise ValueError('read_response_bound')
    result={'text':text,'start_line':start,'returned_lines':len(selected),'total_lines':len(lines),'has_more':start-1+len(selected)<len(lines),'file':fingerprint(checked(a['path'],True))}
elif op=='edit_file':
    p=checked(a['path'],True);old=a['old_text'];new=a['new_text'];text=p.read_text(encoding='utf-8')
    if not old or old not in text or not a.get('replace_all',False) and text.count(old)!=1:raise ValueError('edit_original_match')
    updated=text.replace(old,new,-1 if a.get('replace_all',False) else 1).encode('utf-8')
    if len(updated)>LIMIT:raise ValueError('edit_size')
    # O_TRUNC only after matching exact original pins. Current workspace lease is held.
    with p.open('r+b') as f:f.write(updated);f.truncate();f.flush();os.fsync(f.fileno())
    result={'file':fingerprint(p)}
elif op=='create_directory':checked(a['path']).mkdir(parents=False,exist_ok=False);result={'path':a['path']}
elif op in {'copy_path','move_path'}:
    src=checked(a['source'],True);dst=checked(a['destination']);dst.parent.mkdir(parents=True,exist_ok=True)
    with src.open('rb') as f,dst.open('xb') as out:shutil.copyfileobj(f,out,1024*1024);out.flush();os.fsync(out.fileno())
    original=request['pins'][0]
    if fingerprint(src)!=original or fingerprint(dst)['sha256']!=original['sha256']:raise ValueError('copy_original_changed')
    if op=='move_path':src.unlink()
    result={'file':fingerprint(dst)}
elif op=='delete_file':checked(a['path'],True).unlink();result={'path':a['path'],'deleted':True}
elif op in {'glob','grep'}:
    files=inventory();selected=[]
    for pin in files:
        if not fnmatch.fnmatchcase(pin['path'],a.get('pattern','*')):continue
        if op=='glob':selected.append(pin)
        else:
            try:text=checked(pin['path'],True).read_bytes().decode('utf-8',errors='strict')
            except UnicodeDecodeError:continue
            for i,line in enumerate(text.splitlines(),1):
                if a['query'] in line:selected.append({'path':pin['path'],'line':i,'text':line})
        if len(selected)>10000 or len(json.dumps(selected).encode())>2*1024*1024:raise ValueError('search_result_bound')
    result={'items':selected}
elif op in {'bash','git'}:
    cwd=root if not a.get('cwd') else checked(a['cwd'],True)
    env=dict(os.environ);env.update({'GIT_CONFIG_NOSYSTEM':'1','GIT_CONFIG_GLOBAL':os.devnull,'GIT_TERMINAL_PROMPT':'0','GIT_ASKPASS':'','SSH_ASKPASS':'','GIT_SSH_COMMAND':'false'})
    if op=='bash':
        shell=pathlib.Path(request['program']).name.lower();cmd=[request['program'],*(['-NoLogo','-NoProfile','-NonInteractive','-Command',a['command']] if 'powershell' in shell or 'pwsh' in shell else ['-c',a['command']])]
    else:
        git_policy()
        args=a['arguments'];allow={'status','diff','add','reset','commit','branch','checkout','clone','fetch','pull','push','log','show'}
        if not isinstance(args,list) or not args or args[0] not in allow or len(args)>64 or any(not isinstance(v,str) or '\x00' in v for v in args):raise ValueError('git_arguments')
        if args[0] in {'clone','fetch','pull','push'}:
            remote=a['remote_url'];url(remote)
            if args[0]=='clone':args=['clone','--',remote,a['destination']];destination(a['destination'])
            elif args[0] in {'fetch','pull'}:args=[args[0],'--',remote,*args[1:]]
            else:args=['push','--',remote,*args[1:]]
        if args[0] in {'diff','show','log'}:args=[args[0],'--no-ext-diff','--no-textconv',*args[1:]]
        cmd=[request['program'],'--no-pager','-c','core.fsmonitor=false','-c','commit.gpgsign=false','-c','tag.gpgsign=false','-c','core.pager=','-c','credential.helper=','-c','core.hooksPath='+request['empty_hooks'],'-c','protocol.ext.allow=never','-c','protocol.file.allow=never',*args]
    out,err=bounded_program(cmd,cwd,env,request['timeout'])
    result={'exit_code':0,'stdout':out.decode('utf-8',errors='replace'),'stderr':err.decode('utf-8',errors='replace')}
elif op in {'web_fetch','web_search'}:
    u,ips=url(a['resolved_url'])
    if ips!=a['resolved_ips']:raise ValueError('web_original_dns_changed')
    sock=socket.create_connection((ips[0],u.port or 443),timeout=15);sock=ssl.create_default_context().wrap_socket(sock,server_hostname=u.hostname)
    if sock.getpeername()[0] not in ips:raise ValueError('web_peer_changed')
    conn=http.client.HTTPSConnection(u.hostname,u.port or 443,timeout=15);conn.sock=sock
    try:
        conn.request('GET',urllib.parse.urlunsplit(('', '',u.path or '/',u.query,'')),headers={'User-Agent':'LocalAI-Cowork-V2-device/2','Accept':'text/*,application/json'})
        response=conn.getresponse()
        if not 200<=response.status<300:raise ValueError('web_http_status_or_redirect')
        raw=response.read(2*1024*1024+1)
        if len(raw)>2*1024*1024:raise ValueError('web_response_bound')
        result={'url':a['resolved_url'],'status':response.status,'content':raw.decode('utf-8',errors='replace'),'bytes':len(raw),'sha256':hashlib.sha256(raw).hexdigest()}
    finally:conn.close()
else:raise ValueError('unknown_tool')
print(json.dumps({'ok':True,'tool':op,'result':result},ensure_ascii=False))
