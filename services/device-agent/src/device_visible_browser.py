"""An owned visible Chromium context with one-shot private request files."""
import hashlib,json,os,pathlib,sys,time
directory=pathlib.Path(sys.argv[1]).resolve(strict=True);config=json.loads((directory/'config.json').read_text(encoding='utf-8'))
def publish(name,value):
    raw=json.dumps(value,ensure_ascii=False).encode('utf-8')
    if len(raw)>4*1024*1024:raise ValueError('browser_response_bound')
    tmp=directory/(name+'.tmp')
    with tmp.open('xb') as f:f.write(raw);f.flush();os.fsync(f.fileno())
    os.link(tmp,directory/name);tmp.unlink()
def unlocked():
    if os.name=='nt':
        import ctypes
        from ctypes import wintypes
        u=ctypes.WinDLL('user32',use_last_error=True);k=ctypes.WinDLL('kernel32',use_last_error=True)
        k.GetCurrentProcessId.restype=wintypes.DWORD;k.ProcessIdToSessionId.argtypes=[wintypes.DWORD,ctypes.POINTER(wintypes.DWORD)]
        session=wintypes.DWORD()
        if not k.ProcessIdToSessionId(k.GetCurrentProcessId(),ctypes.byref(session)) or session.value==0:raise ValueError('interactive_session_unavailable')
        w=ctypes.WinDLL('wtsapi32',use_last_error=True);w.WTSQuerySessionInformationW.argtypes=[wintypes.HANDLE,wintypes.DWORD,ctypes.c_int,ctypes.POINTER(ctypes.c_void_p),ctypes.POINTER(wintypes.DWORD)];w.WTSFreeMemory.argtypes=[ctypes.c_void_p]
        buffer=ctypes.c_void_p();size=wintypes.DWORD()
        if not w.WTSQuerySessionInformationW(None,session.value,8,ctypes.byref(buffer),ctypes.byref(size)):raise ValueError('interactive_session_unavailable')
        try:
            if size.value<4 or ctypes.cast(buffer,ctypes.POINTER(ctypes.c_int)).contents.value!=0:raise ValueError('interactive_session_disconnected')
        finally:w.WTSFreeMemory(buffer)
        u.OpenInputDesktop.argtypes=[wintypes.DWORD,wintypes.BOOL,wintypes.DWORD];u.OpenInputDesktop.restype=wintypes.HANDLE
        u.GetUserObjectInformationW.argtypes=[wintypes.HANDLE,ctypes.c_int,ctypes.c_void_p,wintypes.DWORD,ctypes.POINTER(wintypes.DWORD)]
        u.CloseDesktop.argtypes=[wintypes.HANDLE]
        handle=u.OpenInputDesktop(0,False,1)
        if not handle:raise ValueError('interactive_desktop_unavailable')
        try:
            name=ctypes.create_unicode_buffer(128);needed=wintypes.DWORD()
            if not u.GetUserObjectInformationW(handle,2,name,ctypes.sizeof(name),ctypes.byref(needed)) or name.value.lower()!='default':raise ValueError('interactive_desktop_locked')
        finally:u.CloseDesktop(handle)
    else:
        import re,stat,subprocess
        session=os.environ.get('XDG_SESSION_ID','');wayland=os.environ.get('WAYLAND_DISPLAY');display=os.environ.get('DISPLAY','')
        if not re.fullmatch('[A-Za-z0-9_-]{1,100}',session):raise ValueError('interactive_session_unavailable')
        if wayland:
            runtime=pathlib.Path(os.environ.get('XDG_RUNTIME_DIR',''))
            if not runtime.is_absolute() or runtime.is_symlink() or runtime.stat().st_uid!=os.geteuid() or runtime.stat().st_mode&0o077:raise ValueError('interactive_runtime_scope')
            endpoint=runtime/wayland
            if '/' in wayland or endpoint.is_symlink() or endpoint.lstat().st_uid!=os.geteuid() or not stat.S_ISSOCK(endpoint.lstat().st_mode):raise ValueError('interactive_wayland_scope')
        elif not re.fullmatch(r':[0-9]{1,5}(\.[0-9]{1,3})?',display):raise ValueError('interactive_display_not_local')
        observed=subprocess.run(['/usr/bin/loginctl','show-session',session,'--property=Active','--property=LockedHint','--property=User','--property=Type'],stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,timeout=2,check=True).stdout
        if len(observed)>4096:raise ValueError('interactive_session_bound')
        fields=dict(line.split('=',1) for line in observed.decode().splitlines() if '=' in line)
        if fields.get('Active')!='yes' or fields.get('LockedHint')!='no' or fields.get('User')!=str(os.geteuid()) or fields.get('Type')!=('wayland' if wayland else 'x11'):raise ValueError('interactive_session_locked_or_foreign')

unlocked()
program=pathlib.Path(config['chromium'])
if program.is_symlink() or not program.is_file() or hashlib.sha256(program.read_bytes()).hexdigest()!=config['chromium_sha256']:raise ValueError('original_browser_program_changed')
from playwright.sync_api import sync_playwright
with sync_playwright() as pw:
    context=pw.chromium.launch_persistent_context(str(directory/'profile'),executable_path=config['chromium'],headless=False,accept_downloads=False,timeout=45000,args=['--no-first-run','--no-default-browser-check','--disable-background-networking'])
    try:
        page=context.pages[0] if context.pages else context.new_page();publish('ready.json',{'ok':True,'session_id':config['session_id'],'epoch':config['epoch'],'mode':'visible','owned':True})
        idle=time.monotonic()
        while time.monotonic()-idle<900:
            pending=sorted(directory.glob('*.request.json'))
            if not pending:unlocked();time.sleep(.3);continue
            for path in pending:
                idle=time.monotonic();working=path.with_suffix('.working');path.rename(working)
                raw=working.read_bytes()
                if len(raw)>2*1024*1024:raise ValueError('browser_request_bound')
                request=json.loads(raw);a=request['arguments'];unlocked()
                try:
                    if request['epoch']!=config['epoch']:raise ValueError('browser_epoch_changed')
                    op=a['operation'];bound=min(45000,request['timeout']*1000)
                    if op=='navigate':
                        import urllib.parse
                        u=urllib.parse.urlsplit(a['url'])
                        if u.scheme not in ('http','https') or not u.hostname or u.username or u.password:raise ValueError('browser_url')
                        page.goto(a['url'],wait_until='domcontentloaded',timeout=bound)
                    elif op=='click':page.locator(a['selector']).click(timeout=bound)
                    elif op=='fill':page.locator(a['selector']).fill(a['text'],timeout=bound)
                    elif op not in ('dom','screenshot'):raise ValueError('browser_operation')
                    unlocked()
                    screenshot=None
                    if op=='screenshot':
                        target=directory/(request['source']+'.png');page.screenshot(path=str(target),full_page=False,timeout=bound)
                        raw_shot=target.read_bytes()
                        if len(raw_shot)>8*1024*1024:raise ValueError('browser_screenshot_bound')
                        screenshot={'file':target.name,'bytes':len(raw_shot),'sha256':hashlib.sha256(raw_shot).hexdigest()}
                    dom=page.evaluate('document.documentElement.outerHTML')
                    if len(dom.encode())>1024*1024:raise ValueError('browser_dom_bound')
                    publish(path.name.replace('.request.json','.response.json'),{'ok':True,'request_sha256':hashlib.sha256(raw).hexdigest(),'source':request['source'],'epoch':config['epoch'],'url':page.url,'dom':dom,'screenshot':screenshot})
                except Exception:
                    publish(path.name.replace('.request.json','.response.json'),{'ok':False,'error':'visible_browser_original_effect_uncertain','request_sha256':hashlib.sha256(raw).hexdigest(),'source':request['source'],'epoch':config['epoch']})
    finally:context.close()
