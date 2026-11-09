"""Private Chromium context. Its only network transport is the original supervisor pipe."""
import base64, http.cookies, json, pathlib, sys, urllib.parse, uuid

ROOT = pathlib.Path('/workspace')
def emit(value):
    print(json.dumps(value, ensure_ascii=False, separators=(',', ':')), flush=True)
def receive(limit=6*1024*1024):
    raw=sys.stdin.buffer.readline(limit+1)
    if not raw.endswith(b'\n') or len(raw)>limit: raise ValueError('private_browser_frame_invalid')
    return json.loads(raw)
def pipeline_text(operation,url,status,title,body='',results=None):
    if operation=='fetch_text':text='URL: '+url[:8192]+'\nStatus: '+str(status)+'\nTitle: '+title[:500]+'\n\n'+body[:4000]
    else:
        if not 200<=status<300 or not results:raise ValueError('private_search_no_results_or_challenge')
        rendered=[]
        for index,item in enumerate(results[:5],1):
            parsed=urllib.parse.urlsplit(item['url']);target=urllib.parse.parse_qs(parsed.query).get('uddg',[item['url']])[0]
            rendered.append(str(index)+'. '+item['title'][:1000]+'\n'+target[:8192]+'\n'+item['snippet'][:1000])
        text='\n\n'.join(rendered)
    raw=text.encode('utf8');return raw[:48000].decode('utf8','ignore'),len(raw)>48000

def main():
    from playwright.sync_api import sync_playwright
    with sync_playwright() as p:
        browser=p.chromium.launch(executable_path='/usr/bin/chromium',headless=True,args=[
            '--disable-background-networking','--disable-component-update','--disable-sync',
            '--disable-default-apps','--disable-extensions','--disable-quic','--disable-breakpad',
            '--no-first-run','--no-default-browser-check','--no-sandbox'])
        context=browser.new_context(viewport={'width':1440,'height':960},service_workers='block',accept_downloads=True,permissions=[])
        context.set_default_timeout(15000);context.set_default_navigation_timeout(45000)
        # Alternate transports have no broker, and the guest kernel has network=none.
        context.route_web_socket('**/*',lambda ws:ws.close())
        page=context.new_page();active=None;downloads=[]
        def download(item):
            name='browser-'+active['session_id']+'-download-'+str(uuid.uuid4())+'.bin'
            target=ROOT/name
            item.save_as(str(target))
            if target.stat().st_size>4*1024*1024: target.unlink();raise ValueError('private_download_budget')
            downloads.append({'path':name,'suggested_filename':item.suggested_filename[:512]})
        page.on('download',download)
        def route(request_route):
            request=request_route.request
            # The trusted daemon enforces the original action policy, never page-supplied authority.
            ident=str(uuid.uuid4());body=request.post_data_buffer or b''
            if active is None or len(body)>512*1024: request_route.abort();return
            headers=request.all_headers();parsed=urllib.parse.urlsplit(request.url)
            # Never forward inherited parent-domain cookies to a different selected origin.
            headers.pop('cookie',None)
            if parsed.scheme=='https':
                cookies=[c for c in context.cookies([request.url]) if c['domain'].lstrip('.')==parsed.hostname]
                if cookies:headers['cookie']='; '.join(c['name']+'='+c['value'] for c in cookies)
            emit({'kind':'network','request_id':ident,'network':{'url':request.url,'method':request.method,'headers':headers,'body_base64':base64.b64encode(body).decode()}})
            reply=receive()
            if reply.get('kind')!='network' or reply.get('request_id')!=ident: raise ValueError('private_network_reply_mismatch')
            if reply.get('denied'):request_route.abort();return
            response=reply['response'];raw=base64.b64decode(response['body_base64'],validate=True)
            if parsed.scheme=='https':
                for encoded in response.get('set_cookie',[]):
                    jar=http.cookies.SimpleCookie()
                    try:jar.load(encoded)
                    except http.cookies.CookieError:continue
                    for item in jar.values():
                        if item['domain'] and item['domain'].lstrip('.').lower()!=parsed.hostname:continue
                        path=item['path'] or '/'
                        if not path.startswith('/') or len(path)>2048:continue
                        cookie={'name':item.key,'value':item.value,'domain':parsed.hostname,'path':path,'secure':True,'httpOnly':bool(item['httponly'])}
                        if item['samesite'].lower() in {'strict','lax','none'}:cookie['sameSite']=item['samesite'].capitalize()
                        if item['max-age']=='0':cookie['expires']=1
                        context.add_cookies([cookie])
            request_route.fulfill(status=response['status'],headers=response['headers'],body=raw)
        context.route('**/*',route)
        while True:
            value=receive(128*1024)
            if value.get('kind')!='action':raise ValueError('private_browser_action_invalid')
            active=value['action'];op=active['operation'];downloads.clear()
            error=None
            try:
                response=None;text=None;text_truncated=False
                if op in {'navigate','fetch_text','search'}:
                    target=active['url']
                    if op=='search':target='https://html.duckduckgo.com/html/?'+urllib.parse.urlencode({'q':active['text']})
                    response=page.goto(target,wait_until='domcontentloaded')
                    if op=='fetch_text':text,text_truncated=pipeline_text(op,page.url,response.status if response else 0,page.title(),page.locator('body').inner_text())
                    elif op=='search':
                        results=page.evaluate("Array.from(document.querySelectorAll('.result')).slice(0,5).map(item=>({title:item.querySelector('.result__a')?.textContent??'',url:item.querySelector('.result__a')?.href??'',snippet:item.querySelector('.result__snippet')?.textContent??''}))")
                        text,text_truncated=pipeline_text(op,page.url,response.status if response else 0,page.title(),results=results)
                elif op=='click':page.locator(active['selector']).click()
                elif op=='fill':page.locator(active['selector']).fill(active['text'])
                elif op=='screenshot':page.screenshot(path=str(ROOT/('browser-'+active['session_id']+'.png')),full_page=False)
                elif op not in {'dom','close'}:raise ValueError('unknown_private_browser_action')
                dom=page.evaluate('document.documentElement ? document.documentElement.outerHTML.slice(0,1048576) : ""') if op not in {'fetch_text','search'} else ''
                result={'operation':op,'url':page.url,'title':page.title()[:2000],'dom':dom,'dom_truncated':len(dom)>=1048576,'text':text,'text_truncated':text_truncated,'downloads':list(downloads),'screenshot_path':'browser-'+active['session_id']+'.png' if op=='screenshot' else None}
            except Exception as exc:
                # The exact received failure is evidence; it never authorizes a retry.
                result={'operation':op,'url':page.url,'dom':'','error_type':type(exc).__name__,'downloads':list(downloads)}
            emit({'kind':'result','result':result})
            active=None
if __name__=='__main__':main()
