"""Owned Chrome/Firefox context. Private one-shot IPC; no debugging TCP port.

The Rust supervisor retains the profile lock and owns this entire process tree.
Frames are read-only and never navigate, focus a tab, or execute page input.
"""
import base64, collections, hashlib, json, os, pathlib, sys, time, urllib.parse, uuid

root = pathlib.Path(sys.argv[1]).resolve(strict=True)
config = json.loads((root / 'config.json').read_text(encoding='utf-8'))
LIMIT = 2 * 1024 * 1024

def publish(path, value):
    raw = json.dumps(value, ensure_ascii=False).encode('utf-8')
    if len(raw) > LIMIT:
        raw = json.dumps({'result': {'ok': False, 'context_epoch': config['epoch'], 'error': 'browser_output_limit'}}).encode()
    temporary = path.with_suffix('.tmp')
    with temporary.open('xb') as output:
        output.write(raw); output.flush(); os.fsync(output.fileno())
    temporary.replace(path)

def http_url(value):
    if not isinstance(value, str) or len(value) > 8192: raise ValueError('browser_url')
    url = urllib.parse.urlsplit(value)
    if url.scheme not in ('http', 'https') or not url.hostname or url.username or url.password:
        raise ValueError('browser_http_url_required')
    return value

def public_url(value):
    url = urllib.parse.urlsplit(value)
    return urllib.parse.urlunsplit((url.scheme, url.netloc.split('@')[-1], url.path, '', ''))

SAFE_DOM = """() => {
 const root=document.documentElement.cloneNode(true);
 root.querySelectorAll('input').forEach(e=>{
   if(e.type==='password'||e.type==='hidden'||/token|secret|credential|api.?key/i.test(e.name||'')){
     e.removeAttribute('value');e.value='';
   }
 });
 root.querySelectorAll('script').forEach(e=>e.remove());
 return root.outerHTML.slice(0,500000);
}"""

def main():
    executable = pathlib.Path(config['executable'])
    if executable.is_symlink() or hashlib.sha256(executable.read_bytes()).hexdigest() != config['executable_sha256']:
        raise ValueError('browser_original_executable_changed')
    from playwright.sync_api import sync_playwright
    with sync_playwright() as pw:
        engine = pw.firefox if config['engine'] == 'firefox' else pw.chromium
        context = engine.launch_persistent_context(config['profile'], executable_path=str(executable),
            headless=True, viewport={'width':1440,'height':960}, accept_downloads=False, timeout=45000)
        pages = {}; documents = {}; console = collections.deque(maxlen=200)
        network = collections.deque(maxlen=200); responses = {}; annotations = {}
        current = None; poisoned = False

        def register(page):
            nonlocal current
            identity = uuid.uuid4().hex; pages[identity] = page; documents[identity] = uuid.uuid4().hex
            if current is None: current = identity
            def navigation(frame):
                if frame == page.main_frame: documents[identity] = uuid.uuid4().hex
            page.on('framenavigated', navigation)
            page.on('console', lambda message: console.append({'type':message.type,'text':message.text[:4096]}))
            page.on('pageerror', lambda _: console.append({'type':'pageerror','text':'Page script error'}))
            def response_received(response):
                key = uuid.uuid4().hex
                network.append({'id':key,'method':response.request.method,'url':public_url(response.url),'status':response.status,'resource_type':response.request.resource_type})
                responses[key] = response
                if len(responses)>200: responses.pop(next(iter(responses)))
            page.on('response', response_received)
            return identity
        for page in context.pages: register(page)
        context.on('page', register)
        if not pages: context.new_page()

        def tab_list():
            return [{'id':identity,'document_id':documents[identity],'url':page.url,'title':page.title()[:500],
                     'active':identity==current} for identity,page in pages.items() if not page.is_closed()]

        def dispatch(a, timeout):
            nonlocal current, poisoned
            operation = a['operation']; bound = min(timeout*1000,45000)
            if poisoned and operation not in ('frame','tabs','dom','screenshot'):
                raise ValueError('browser_previous_effect_uncertain_close_session')
            active = [identity for identity,page in pages.items() if not page.is_closed()]
            if not active: context.new_page(); active = list(pages)
            if current not in active: current = active[0]
            identity = a.get('tab_id') or current
            if identity not in active: raise ValueError('browser_unknown_tab')
            page = pages[identity]
            if a.get('document_id') and a['document_id'] != documents[identity]: raise ValueError('browser_document_changed')
            details = None; text = None; screenshot = None; frame = None
            if operation in ('navigate','fetch_text','search'):
                url = ('https://html.duckduckgo.com/html/?'+urllib.parse.urlencode({'q':a.get('text','')[:8000]})) if operation=='search' else (a.get('url') or (page.url if operation=='fetch_text' else None))
                response = page.goto(http_url(url), wait_until='domcontentloaded', timeout=bound)
                if operation=='fetch_text': text='URL: '+page.url+'\nStatus: '+str(response.status if response else 0)+'\nTitle: '+page.title()+'\n\n'+page.locator('body').inner_text(timeout=bound)[:12000]
                if operation=='search':
                    items=page.locator('.result').evaluate_all("items=>items.slice(0,5).map(e=>({title:e.querySelector('.result__a')?.textContent,url:e.querySelector('.result__a')?.href,snippet:e.querySelector('.result__snippet')?.textContent}))")
                    if not items: raise ValueError('browser_search_challenge')
                    text=json.dumps(items,ensure_ascii=False)[:12000]
            elif operation=='back': page.go_back(wait_until='domcontentloaded',timeout=bound)
            elif operation=='forward': page.go_forward(wait_until='domcontentloaded',timeout=bound)
            elif operation=='reload': page.reload(wait_until='domcontentloaded',timeout=bound)
            elif operation=='new_tab':
                before=set(pages);new_page=context.new_page()
                current=next(key for key in pages if key not in before);page=new_page;identity=current
                if a.get('url'): page.goto(http_url(a['url']),wait_until='domcontentloaded',timeout=bound)
            elif operation=='close_tab':
                page.close();active=[key for key,p in pages.items() if not p.is_closed()]
                if not active: context.new_page();active=[key for key,p in pages.items() if not p.is_closed()]
                current=active[0];identity=current;page=pages[current]
            elif operation=='activate_tab': current=identity
            elif operation=='click': page.locator(a['selector']).click(timeout=bound)
            elif operation=='fill': page.locator(a['selector']).fill(a.get('text',''),timeout=bound)
            elif operation=='select': page.locator(a['selector']).select_option(a.get('text',''),timeout=bound)
            elif operation=='scroll': page.mouse.wheel(a.get('x') or 0,a.get('y') or 0)
            elif operation=='pointer': page.mouse.click(a['x'],a['y'])
            elif operation=='key':
                if a.get('selector')=='insert': page.keyboard.insert_text(a.get('text',''))
                else: page.keyboard.press(a.get('text',''))
            elif operation=='resize': page.set_viewport_size({'width':a['width'],'height':a['height']})
            elif operation=='annotate':
                note=uuid.uuid4().hex;label=(a.get('text') or '')[:2000]
                target=page.locator(a['selector'])
                if target.count()!=1:raise ValueError('browser_exact_element_required')
                target.evaluate("""(e,n)=>{const b=document.createElement('span');b.textContent=n.text;b.dataset.coworkAnnotation=n.id;const r=e.getBoundingClientRect();b.style.cssText=`position:absolute;z-index:2147483647;background:#fff3a3;color:#171717;padding:4px;pointer-events:none;left:${r.left+scrollX}px;top:${r.top+scrollY}px`;document.documentElement.appendChild(b)}""",{'id':note,'text':label})
                annotations[note]={'id':note,'selector':a['selector'],'text':label,'url':public_url(page.url),'recorded_at':int(time.time()*1000)}
            elif operation=='clear_annotations':
                page.locator('[data-cowork-annotation]').evaluate_all('items=>items.forEach(e=>e.remove())');annotations.clear()
            elif operation=='network_details':
                response=responses.get(a.get('selector'))
                if response is None:raise ValueError('browser_network_entry_expired')
                def headers(values):return {key:('[redacted]' if any(s in key.lower() for s in ('authorization','cookie','token','secret','key')) else value[:4096]) for key,value in values.items()}
                details={'id':a['selector'],'url':public_url(response.url),'status':response.status,'request_headers':headers(response.request.all_headers()),'response_headers':headers(response.all_headers())}
                length=response.headers.get('content-length','');kind=response.headers.get('content-type','')
                if length.isdigit() and int(length)<=1024*1024 and any(t in kind for t in ('text/','json','javascript','xml')):
                    raw=response.body();details.update(body=raw[:65536].decode('utf-8','replace'),body_truncated=len(raw)>65536)
            elif operation=='screenshot':
                screenshot='browser-'+config['session_id']+'.png'
                page.screenshot(path=str(pathlib.Path(config['workspace'])/screenshot),full_page=False,timeout=bound)
            elif operation=='frame':
                raw=page.screenshot(type='jpeg',quality=65,full_page=False,timeout=bound)
                if len(raw)>750000:raise ValueError('browser_frame_limit')
                frame={'image':'data:image/jpeg;base64,'+base64.b64encode(raw).decode(),'width':page.viewport_size['width'],'height':page.viewport_size['height']}
            elif operation not in ('tabs','dom','console','network','annotations'):raise ValueError('browser_unknown_operation')
            result={'ok':True,'session_id':config['session_id'],'context_epoch':config['epoch'],'url':page.url,'title':page.title()[:500],
                'tab_id':identity,'document_id':documents[identity],'tabs':tab_list(), 'backend':'playwright_'+config['engine']}
            if operation in ('navigate','dom','click','fill'):result['dom']=page.evaluate(SAFE_DOM)
            if frame:result['frame']=frame
            if screenshot:result['screenshot_path']=screenshot
            if text:result['text']=text[:14000]
            if operation=='console':result['console']=list(console)
            if operation=='network':result['network']=list(network)
            if details:result['network_details']=details
            if operation in ('annotations','annotate','clear_annotations'):result['annotations']=list(annotations.values())
            return result
        publish(root/'ready.json',{'ready':True})
        last=time.monotonic()
        try:
            while time.monotonic()-last<900 and not (root/'stop').exists():
                pending=sorted(root.glob('*.request.json'))
                if not pending:time.sleep(.02);continue
                for path in pending:
                    last=time.monotonic();working=path.with_suffix('.working');path.rename(working)
                    try:
                        if working.stat().st_size>LIMIT:raise ValueError('browser_request_limit')
                        request=json.loads(working.read_text(encoding='utf-8'))
                        value=dispatch(request['body'],request['timeout_seconds'])
                    except ValueError as error:
                        value={'ok':False,'context_epoch':config['epoch'],'error':str(error)[:100]}
                    except Exception:
                        poisoned=True;value={'ok':False,'context_epoch':config['epoch'],'error':'browser_effect_uncertain_close_session'}
                    publish(root/path.name.replace('.request.json','.response.json'),{'result':value});working.unlink(missing_ok=True)
        finally:context.close()

if __name__=='__main__':
    try:main()
    except Exception:
        sys.stderr.write('browser_runtime_start_failed\n');sys.exit(1)
