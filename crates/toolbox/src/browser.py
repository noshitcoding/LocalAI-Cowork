"""Headless browser in an explicitly supplied private order/session profile."""
import json, os, pathlib, subprocess, sys, tempfile
profile,workspace,executable=map(pathlib.Path,sys.argv[1:4])
encoded=sys.argv[4];action=json.loads(pathlib.Path(encoded[1:]).read_text(encoding='utf-8') if encoded.startswith('@') else encoded); operation=action['operation']; url=action['url']
assert url.startswith(('https://','http://')) and operation in {'navigate','dom','screenshot','click','fill'}
profile=profile.resolve(strict=True);workspace=workspace.resolve(strict=True)
shot=workspace/('browser-'+action['session_id']+'.png')
# The runner supplies the operator-controlled egress proxy. Chromium does not
# consume these variables itself; apply the same proxy to both browser backends.
proxy=next((os.environ[name] for name in ('HTTPS_PROXY','HTTP_PROXY','ALL_PROXY') if os.environ.get(name)),None)
try:
    from playwright.sync_api import sync_playwright
except ImportError:
    assert executable.is_file(), 'Install Playwright Python or configure a Chromium executable'
    assert operation in {'navigate','dom','screenshot'}, 'Playwright Python is required for click/fill'
    command=[str(executable),'--headless=new','--no-first-run','--no-default-browser-check',
        '--disable-breakpad','--disable-crash-reporter','--disable-background-networking',
        '--user-data-dir='+str(profile),'--timeout=45000']
    if proxy:command+=['--proxy-server='+proxy]
    if operation=='screenshot':command+=['--screenshot='+str(shot),'--window-size=1440,960']
    else:command+=['--dump-dom']
    command.append(url)
    with tempfile.TemporaryFile() as stdout,tempfile.TemporaryFile() as stderr:
        child=subprocess.Popen(command,stdout=stdout,stderr=stderr)
        try:code=child.wait(timeout=55)
        except subprocess.TimeoutExpired:
            child.terminate();child.wait(timeout=5);raise
        assert code==0, 'Chromium headless command failed'
        stdout.seek(0);html=stdout.read(1024*1024+1)
    print(json.dumps({'session_id':action['session_id'],'url':url,'backend':'chromium_cli',
        'dom':html[:1024*1024].decode('utf-8',errors='replace'),'dom_truncated':len(html)>1024*1024,
        'screenshot_path':shot.name if operation=='screenshot' else None}))
else:
    with sync_playwright() as playwright:
        settings={'headless':True,'viewport':{'width':1440,'height':960},'timeout':45000}
        if proxy:settings['proxy']={'server':proxy}
        if executable.is_file():settings['executable_path']=str(executable)
        context=playwright.chromium.launch_persistent_context(str(profile),**settings)
        try:
            page=context.pages[0] if context.pages else context.new_page()
            page.goto(url,wait_until='domcontentloaded',timeout=45000)
            if operation=='click':page.locator(action['selector']).click(timeout=15000)
            if operation=='fill':page.locator(action['selector']).fill(action['text'],timeout=15000)
            if operation=='screenshot':page.screenshot(path=str(shot),full_page=False)
            # Bound inside Chromium, before transporting a potentially huge DOM.
            dom=page.evaluate('document.documentElement.outerHTML.slice(0,1048576)')
            print(json.dumps({'session_id':action['session_id'],'url':page.url,'title':page.title(),
                'backend':'playwright','dom':dom,'screenshot_path':shot.name if operation=='screenshot' else None}))
        finally:context.close()
