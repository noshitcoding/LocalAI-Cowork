"""Private file IPC. Its supervisor owns and joins this entire process tree.

Every request filename is consumed once. A lost reply never dispatches it again.
Neither the browser nor stdio MCP process is exposed on a debugging TCP port.
"""
import collections,json, os, pathlib, queue, subprocess, sys, threading, time,urllib.parse,uuid

root = pathlib.Path(sys.argv[1]).resolve(strict=True)
config = json.loads((root / 'config.json').read_text(encoding='utf-8'))
LIMIT = 2 * 1024 * 1024

def publish(path, value):
    raw = json.dumps(value, ensure_ascii=False).encode('utf-8')
    if len(raw) > LIMIT:
        raw = json.dumps({'error': 'Persistent tool output exceeds its limit'}).encode()
    tmp = path.with_suffix('.tmp')
    with tmp.open('xb') as output:
        output.write(raw); output.flush()
    tmp.replace(path)

def serve(dispatch):
    publish(root / 'ready.json', {'ready': True})
    last_used = time.monotonic()
    while True:
        if time.monotonic() - last_used > 900: return
        pending = sorted(root.glob('*.request.json'))
        if not pending:
            time.sleep(0.025); continue
        for path in pending:
            last_used = time.monotonic()
            working = path.with_suffix('.working')
            path.rename(working)
            try:
                if working.stat().st_size > LIMIT: raise ValueError('Oversized request')
                request = json.loads(working.read_text(encoding='utf-8'))
                response = {'result': dispatch(request['body'], request['timeout_seconds'])}
            except Exception as error:
                response = {'error': str(error)[:4096]}
            publish(root / (path.name.replace('.request.json', '.response.json')), response)
            working.unlink(missing_ok=True)

if config['mode'] == 'browser':
    from playwright.sync_api import sync_playwright
    with sync_playwright() as playwright:
        settings = {'headless': True, 'viewport': {'width': 1440, 'height': 960}, 'timeout': 45000}
        if config.get('executable'): settings['executable_path'] = config['executable']
        context = playwright.chromium.launch_persistent_context(config['profile'], **settings)
        page = context.pages[0] if context.pages else context.new_page()
        console=collections.deque(maxlen=200);network=collections.deque(maxlen=200);responses={};annotations={}
        def public_url(url):
            parsed=urllib.parse.urlsplit(url);host=parsed.hostname or '';host=('['+host+']' if ':' in host else host)+(':'+str(parsed.port) if parsed.port else '');return urllib.parse.urlunsplit((parsed.scheme,host,parsed.path,'',''))
        def console_message(message):console.append({'type':message.type,'text':message.text[:4096],'url':public_url(message.location.get('url','')),'line':message.location.get('lineNumber'),'recorded_at':int(time.time()*1000)})
        def response_received(response):
            identity=uuid.uuid4().hex;network.append({'id':identity,'method':response.request.method,'url':public_url(response.url),'status':response.status,'resource_type':response.request.resource_type,'recorded_at':int(time.time()*1000)});responses[identity]=response
            while len(responses)>200:responses.pop(next(iter(responses)))
        page.on('console',console_message);page.on('pageerror',lambda error:console.append({'type':'pageerror','text':str(error)[:4096],'recorded_at':int(time.time()*1000)}));page.on('response',response_received)
        page.on('requestfailed',lambda request:network.append({'id':uuid.uuid4().hex,'method':request.method,'url':public_url(request.url),'failure':str(request.failure)[:1000],'recorded_at':int(time.time()*1000)}))
        def dispatch(action, timeout):
            operation = action['operation']; bound = min(timeout * 1000, 45000);result_text=None
            if operation=='search':
                query=action.get('text');assert isinstance(query,str) and 0<len(query)<=8000
                action={**action,'url':'https://html.duckduckgo.com/html/?'+urllib.parse.urlencode({'q':query})}
            if operation in {'navigate','fetch_text','search'}:
                url = action['url']
                parsed=urllib.parse.urlsplit(url)
                if parsed.scheme not in {'https','http'} or not parsed.hostname or parsed.username or parsed.password: raise ValueError('Expected HTTP(S) URL without credentials')
                response=page.goto(url, wait_until='domcontentloaded', timeout=bound)
                if operation=='fetch_text':result_text='URL: '+page.url+'\nStatus: '+str(response.status if response else 0)+'\nTitle: '+page.title()+'\n\n'+page.locator('body').inner_text(timeout=bound)[:4000]
                elif operation=='search':
                    if not response or not response.ok:raise ValueError('Search returned an unsuccessful HTTP status')
                    items=page.evaluate("""()=>Array.from(document.querySelectorAll('.result')).slice(0,5).map(item=>({title:item.querySelector('.result__a')?.textContent??'',url:item.querySelector('.result__a')?.href??'',snippet:item.querySelector('.result__snippet')?.textContent??''}))""")
                    rendered=[]
                    for index,item in enumerate(items,1):
                        parsed=urllib.parse.urlsplit(item['url']);url=urllib.parse.parse_qs(parsed.query).get('uddg',[item['url']])[0]
                        rendered.append(str(index)+'. '+item['title'][:1000]+'\n'+url[:8192]+'\n'+item['snippet'][:1000])
                    if not rendered:raise ValueError('Search returned no recognizable results; provider may require a challenge')
                    result_text='\n\n'.join(rendered)
            elif page.url == 'about:blank': raise ValueError('Navigate this session first')
            elif operation == 'click': page.locator(action['selector']).click(timeout=bound)
            elif operation == 'fill': page.locator(action['selector']).fill(action['text'], timeout=bound)
            elif operation == 'annotate':
                selector=action['selector'];label=action.get('text') or '';assert len(selector)<=1000 and 0<len(label)<=2000
                identity=uuid.uuid4().hex;target=page.locator(selector);assert target.count()==1,'Annotation requires one exact element'
                target.evaluate("""(element,note)=>{const box=document.createElement('span');box.textContent=note.text;box.dataset.coworkAnnotation=note.id;box.style.cssText='position:absolute;z-index:2147483647;background:#fff3a3;color:#171717;padding:4px;border:2px solid #a74a00;max-width:400px;pointer-events:none;white-space:pre-wrap';const r=element.getBoundingClientRect();box.style.left=(r.left+scrollX)+'px';box.style.top=(r.top+scrollY)+'px';document.documentElement.appendChild(box)}""",{'id':identity,'text':label})
                annotations[identity]={'id':identity,'selector':selector,'text':label,'url':public_url(page.url),'recorded_at':int(time.time()*1000)}
            elif operation=='clear_annotations':
                page.locator('[data-cowork-annotation]').evaluate_all('(elements)=>elements.forEach(element=>element.remove())');annotations.clear()
            elif operation=='network_details':
                response=responses.get(action.get('selector'));assert response,'Network entry is no longer retained'
                redacted={'authorization','cookie','set-cookie','proxy-authorization','x-api-key'}
                headers=lambda values:{key:('[redacted]' if key.lower() in redacted else value[:4096]) for key,value in values.items()}
                details={'id':action['selector'],'url':public_url(response.url),'status':response.status,'request_headers':headers(response.request.all_headers()),'response_headers':headers(response.all_headers())}
                content_type=response.headers.get('content-type','')
                length=response.headers.get('content-length','')
                if any(kind in content_type for kind in ['text/','json','javascript','xml']) and length.isdigit() and int(length)<=1024*1024:
                    raw=response.body();details.update(body=raw[:65536].decode('utf-8','replace'),body_truncated=len(raw)>65536)
            elif operation not in {'dom', 'screenshot','console','network','annotations'}: raise ValueError('Unknown browser operation')
            shot = None
            if operation == 'screenshot':
                shot = 'browser-' + config['session_id'] + '.png'
                page.screenshot(path=str(pathlib.Path(config['workspace']) / shot), full_page=False, timeout=bound)
            if operation in {'fetch_text','search'}:
                raw=(result_text or '').encode('utf8');result={'session_id':config['session_id'],'url':page.url[:2048],'title':page.title()[:500],'backend':'playwright_persistent','context_epoch':config['epoch'],'text':raw[:14000].decode('utf8','ignore'),'text_truncated':len(raw)>14000}
                while len(json.dumps(result,ensure_ascii=False).encode('utf8'))>23000:result['text']=result['text'][:len(result['text'])*3//4];result['text_truncated']=True
                return result
            return {'session_id': config['session_id'], 'url': page.url, 'title': page.title(),
                    'backend': 'playwright_persistent', 'context_epoch': config['epoch'],
                    'dom': page.evaluate('document.documentElement.outerHTML.slice(0,1048576)') if operation in {'navigate','dom','click','fill'} else None, 'screenshot_path': shot,
                    'text':result_text,'console':list(console) if operation=='console' else None,'network':list(network) if operation=='network' else None,
                    'network_details':details if operation=='network_details' else None,'annotations':list(annotations.values()) if operation in {'annotations','annotate','clear_annotations'} else None}
        try: serve(dispatch)
        finally: context.close()
elif config['mode'] == 'mcp':
    child_environment = dict(config['environment'])
    for name, slot in config.get('environment_slots', {}).items():
        child_environment[name] = os.environ.pop(slot)
    child = subprocess.Popen([config['program'], *config['arguments']], stdin=subprocess.PIPE,
                             stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, cwd=config['workspace'], env=child_environment)
    messages = queue.Queue(maxsize=64)
    def reader():
        try:
            while True:
                line = child.stdout.readline(LIMIT + 1)
                if not line: break
                if len(line) > LIMIT or not line.endswith(b'\n'): raise ValueError('Oversized stdio response')
                messages.put(json.loads(line), timeout=5)
        except Exception as error: messages.put({'transport_error': str(error)}, timeout=5)
        finally:
            try: messages.put({'transport_error': 'Stdio process closed'}, timeout=5)
            except queue.Full: pass
    threading.Thread(target=reader, daemon=True).start()
    def send(body):
        encoded = json.dumps(body, separators=(',', ':')).encode() + b'\n'
        if len(encoded) > LIMIT: raise ValueError('Oversized MCP request')
        child.stdin.write(encoded); child.stdin.flush()
    def dispatch(body, timeout):
        if child.poll() is not None: raise ValueError('Stdio process closed; reconnect explicitly')
        send(body)
        if 'id' not in body: return None
        deadline = time.monotonic() + timeout
        while True:
            response = messages.get(timeout=max(0.001, deadline - time.monotonic()))
            if 'transport_error' in response: raise ValueError(response['transport_error'])
            if 'method' in response:
                if 'id' in response:
                    send({'jsonrpc': '2.0', 'id': response['id'], 'error': {'code': -32601, 'message': 'Client capability not authorized'}})
                continue
            if response.get('id') != body['id']: raise ValueError('Unbound stdio response')
            return response
    try: serve(dispatch)
    finally:
        child.terminate()
        try: child.wait(timeout=2)
        except subprocess.TimeoutExpired: child.kill(); child.wait()
else:
    raise ValueError('Unknown persistent tool mode')
