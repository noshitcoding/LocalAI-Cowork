"""A live page in one exact run-bound container; IPC remains in private /tmp."""
import json, os, pathlib, socket, sys, time
LIMIT=2*1024*1024
SOCKET='/tmp/cowork-browser.sock'
def receive(connection):
    data=bytearray()
    while not data.endswith(b'\n'):
        chunk=connection.recv(min(65536,LIMIT+1-len(data)))
        if not chunk: raise ValueError('Incomplete browser command')
        data.extend(chunk)
        if len(data)>LIMIT: raise ValueError('Oversized browser command')
    return json.loads(data)
mode=sys.argv[1]
if mode=='call':
    action=json.loads(sys.argv[2]);deadline=time.monotonic()+15
    while not pathlib.Path(SOCKET).exists():
        if time.monotonic()>deadline:raise ValueError('Browser context unavailable; create a new session')
        time.sleep(.05)
    with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as connection:
        connection.settimeout(100);connection.connect(SOCKET)
        connection.sendall(json.dumps(action).encode()+b'\n');result=receive(connection)
        if 'error' in result:raise ValueError(result['error'])
        print(json.dumps(result['result'],ensure_ascii=False))
elif mode=='serve':
    from playwright.sync_api import sync_playwright
    os.umask(0o077);session=sys.argv[2]
    with sync_playwright() as playwright:
        context=playwright.chromium.launch(headless=True,executable_path='/usr/bin/chromium',args=['--disable-dev-shm-usage'])
        page=context.new_page(viewport={'width':1440,'height':960})
        with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as listener:
            listener.bind(SOCKET);listener.listen(1);listener.settimeout(1)
            started=last_used=time.monotonic()
            try:
                while time.monotonic()-started<600 and time.monotonic()-last_used<300:
                    try:connection,_=listener.accept()
                    except socket.timeout:continue
                    with connection:
                        connection.settimeout(100);last_used=time.monotonic()
                        try:
                            action=receive(connection)
                            if action.get('session_id')!=session:raise ValueError('Session mismatch')
                            operation=action['operation']
                            if operation=='navigate':
                                target=action['url']
                                if not target.startswith(('https://','http://')):raise ValueError('Expected HTTP(S) URL')
                                page.goto(target,wait_until='domcontentloaded',timeout=45000)
                            elif operation=='close':
                                connection.sendall(json.dumps({'result':{'session_id':session,'closed':True}}).encode()+b'\n');break
                            elif page.url=='about:blank':raise ValueError('Navigate this session first')
                            elif action.get('url') and action['url']!=page.url:raise ValueError('Approved URL no longer matches live page; inspect and approve again')
                            elif operation=='fill':page.locator(action['selector']).fill(action['text'],timeout=30000)
                            elif operation=='click':page.locator(action['selector']).click(timeout=30000)
                            elif operation not in {'dom','screenshot'}:raise ValueError('Unknown browser operation')
                            shot=None
                            if operation=='screenshot':
                                shot='browser-'+session+'.png';page.screenshot(path='/workspace/'+shot,timeout=30000)
                            result={'session_id':session,'url':page.url,'title':page.title(),'backend':'team_playwright_persistent',
                                    'dom':page.evaluate('document.documentElement.outerHTML.slice(0,1048576)'), 'screenshot_path':shot}
                            encoded=json.dumps({'result':result},ensure_ascii=False).encode()
                            if len(encoded)>LIMIT:raise ValueError('Browser result exceeds its limit')
                            connection.sendall(encoded+b'\n')
                        except Exception as error:
                            connection.sendall(json.dumps({'error':str(error)[:4096]}).encode()+b'\n')
                            # A timed-out click must not remain alive or be replayed.
                            break
            finally:context.close();pathlib.Path(SOCKET).unlink(missing_ok=True)
else:raise ValueError('Unknown browser session mode')
