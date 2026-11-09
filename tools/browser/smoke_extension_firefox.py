"""Actual temporary Firefox add-on, real tabs/DOM, deterministic native peer.

Requires Selenium only in the test environment. Does not register a native host
or touch any installed browser profile. The production extension is unmodified
except that the fixture grants its optional site permission at installation.
"""
import json,os,pathlib,shutil,tempfile,time,uuid,zipfile,threading,http.server
from selenium import webdriver
from selenium.webdriver.firefox.options import Options
from selenium.webdriver.firefox.service import Service
from smoke_managed import Handler,ROOT,runtime

def main():
 platform='windows' if os.name=='nt' else 'linux';server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler);threading.Thread(target=server.serve_forever,daemon=True).start()
 driver=None
 try:
  with tempfile.TemporaryDirectory(prefix='cowork-extension-firefox-') as directory:
   directory=pathlib.Path(directory);extension=directory/'extension';shutil.copytree(ROOT/'dist/browser-extensions/firefox',extension)
   manifest=json.loads((extension/'manifest.json').read_text());manifest['host_permissions']=['<all_urls>'];(extension/'manifest.json').write_text(json.dumps(manifest))
   archive=directory/'extension.xpi'
   with zipfile.ZipFile(archive,'w') as bundle:
    for file in extension.iterdir():bundle.write(file,file.name)
   options=Options();options.binary_location=str(runtime('firefox',platform));options.add_argument('-headless');options.set_preference('xpinstall.signatures.required',False)
   driver=webdriver.Firefox(options=options,service=Service(service_args=["--allow-system-access"]));driver.set_script_timeout(20)
   addon=driver.install_addon(str(archive),temporary=True)
   driver.set_context('chrome');mapping=json.loads(driver.execute_script("return Services.prefs.getStringPref('extensions.webextensions.uuids');"));driver.set_context('content')
   addon_uuid=mapping[addon];url=f'http://127.0.0.1:{server.server_port}/'
   driver.get(url+'shared');shared_handle=driver.current_window_handle
   driver.switch_to.new_window('tab');driver.get(url+'unshared')
   driver.switch_to.new_window('tab');driver.get(f'moz-extension://{addon_uuid}/popup.html')
   def extension_script(code,*args):
    return driver.execute_async_script("const done=arguments[arguments.length-1];(async()=>{const bg=await browser.runtime.getBackgroundPage();"+code+"})().then(done,e=>done({fixture_error:String(e)}));",*args)
   extension_script("bg.__messages=[];const port={postMessage:v=>bg.__messages.push(v),disconnect:()=>{},onDisconnect:{addListener:()=>{}},onMessage:{addListener:fn=>bg.__emit=fn}};bg.browser.runtime.connectNative=()=>port;return true;")
   result=extension_script("const tabs=await browser.tabs.query({});const tab=tabs.find(t=>t.url?.endsWith('/shared'));await browser.runtime.sendMessage({type:'connect',instance_id:'fixture',tabs:[tab.id]});return bg.__messages.find(v=>v.type==='connect');")
   assert 'fixture_error' not in result,result
   assert len(result['tabs'])==1;tab=result['tabs'][0]
   extension_script("bg.__emit({type:'connected',version:1,connection_id:'fixture'});return true;")
   def call(**command):
    request=str(uuid.uuid4());command={'tab_id':tab['id'],'document_id':tab['document_id'],**command}
    extension_script("bg.__emit({type:'command',version:1,request_id:arguments[0],epoch:'fixture',command:arguments[1]});return true;",request,command)
    deadline=time.monotonic()+10
    while time.monotonic()<deadline:
     result=extension_script("return bg.__messages.find(v=>v.type==='result'&&v.request_id===arguments[0])??null;",request)
     if result:return result
     time.sleep(.025)
    raise RuntimeError('Firefox extension action timed out')
   model=json.loads(pathlib.Path(os.environ['COWORK_BROWSER_TEST_MODEL_ACTIONS']).read_text()) if os.environ.get('COWORK_BROWSER_TEST_MODEL_ACTIONS') else None
   for action in model['actions'] if model else [{'operation':'fill','selector':'#entry','text':'Firefox extension works'},{'operation':'click','selector':'#save'}]:assert call(**action)['ok']
   expected=model['expected'] if model else 'Firefox extension works'
   dom=call(operation='dom');assert dom['ok'] and expected in dom['result']['dom'] and 'never-return-password' not in dom['result']['dom']
   assert not call(operation='dom',tab_id='999999')['ok'];assert not call(operation='click',selector='#save',document_id='stale')['ok']
   state=extension_script("await browser.runtime.sendMessage({type:'disconnect'});return browser.runtime.sendMessage({type:'status'});");assert not state['connected'] and not state['grants']
   driver.switch_to.window(shared_handle);assert 'retained=yes' in driver.execute_script('return document.cookie')
   print(json.dumps({'browser':'Firefox','platform':platform,'native_peer':'fixture','model':model['model'] if model else None,'checks':['real_temporary_addon','shared_tab_fill_click_dom','existing_cookie_context','password_redaction','unshared_tab_rejected','stale_document_rejected','revoke_clears_grants']},indent=2))
 finally:
  if driver:driver.quit()
  server.shutdown();server.server_close()
if __name__=='__main__':main()
