"""Real extension/native-stdio/OS-IPC/approved-toolbox acceptance.

Uses fresh browser profiles and a unique temporary native-host name, removed in
finally. No production registration, application installation or user cookies.
Requires the built browser host, acceptance example and test-only Selenium.
"""
import argparse,contextlib,json,os,pathlib,shutil,subprocess,tempfile,threading,http.server,time,uuid,zipfile
from selenium import webdriver
from selenium.webdriver.chrome.options import Options as ChromeOptions
from selenium.webdriver.firefox.options import Options as FirefoxOptions
from selenium.webdriver.firefox.service import Service as FirefoxService
from smoke_managed import ROOT,Handler,runtime

@contextlib.contextmanager
def registration(name,host,directory,identity):
 manifests=[];keys=[]
 try:
  for engine in ('chrome','firefox'):
   value={'name':name,'description':'Isolated Cowork acceptance host','path':str(host),'type':'stdio'}
   value.update({'allowed_origins':['chrome-extension://'+identity['chrome_id']+'/']} if engine=='chrome' else {'allowed_extensions':[identity['firefox_id']]})
   if os.name=='nt':
    import winreg
    file=directory/(engine+'-native.json');file.write_text(json.dumps(value),encoding='utf-8')
    branches=['Google\\Chrome','Google\\ChromeForTesting'] if engine=='chrome' else ['Mozilla']
    for branch in branches:
     key='Software\\'+branch+'\\NativeMessagingHosts\\'+name
     try:
      existing=winreg.OpenKey(winreg.HKEY_CURRENT_USER,key);existing.Close();raise RuntimeError('Temporary test host already exists')
     except FileNotFoundError:pass
     with winreg.CreateKey(winreg.HKEY_CURRENT_USER,key) as handle:winreg.SetValueEx(handle,'',0,winreg.REG_SZ,str(file))
     keys.append(key)
   else:
    home=pathlib.Path.home();config=pathlib.Path(os.environ.get('XDG_CONFIG_HOME',str(home/'.config')))
    folders=[config/'google-chrome/NativeMessagingHosts',config/'google-chrome-for-testing/NativeMessagingHosts',directory/'chrome-profile/NativeMessagingHosts'] if engine=='chrome' else [home/'.mozilla/native-messaging-hosts']
    for folder in folders:
     folder.mkdir(parents=True,exist_ok=True);file=folder/(name+'.json')
     with file.open('x',encoding='utf-8') as output:json.dump(value,output)
     manifests.append(file)
  yield
 finally:
  if os.name=='nt':
   import winreg
   for key in keys:winreg.DeleteKey(winreg.HKEY_CURRENT_USER,key)
  for file in manifests:file.unlink()

def main():
 parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--peer',type=pathlib.Path,required=True);parser.add_argument('--host',type=pathlib.Path,required=True);parser.add_argument('--output',type=pathlib.Path);args=parser.parse_args()
 identity=json.loads((ROOT/'extensions/browser/identity.json').read_text());platform='windows' if os.name=='nt' else 'linux'
 model=json.loads(pathlib.Path(os.environ['COWORK_BROWSER_TEST_MODEL_ACTIONS']).read_text()) if os.environ.get('COWORK_BROWSER_TEST_MODEL_ACTIONS') else {'model':None,'expected':'Native chain passed','actions':[{'operation':'fill','selector':'#entry','text':'Native chain passed'},{'operation':'click','selector':'#save'}]}
 server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler);threading.Thread(target=server.serve_forever,daemon=True).start();report={'platform':platform,'model':model['model'],'engines':{}}
 try:
  with tempfile.TemporaryDirectory(prefix='cowork-native-chain-') as temporary:
   directory=pathlib.Path(temporary);name='dev.localai.cowork.browser.test_'+uuid.uuid4().hex
   peer=subprocess.Popen([str(args.peer),str(directory)],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,encoding='utf-8',**({'creationflags':0x08000000} if os.name=='nt' else {}))
   def peer_call(value):
    peer.stdin.write(json.dumps(value)+'\n');peer.stdin.flush();line=peer.stdout.readline()
    if not line:raise RuntimeError('Native peer exited: '+peer.stderr.read())
    result=json.loads(line)
    if 'error' in result:raise RuntimeError(result['error'])
    return result
   try:
    ready=json.loads(peer.stdout.readline());instance=ready['instance_id']
    with registration(name,args.host,directory,identity):
     for engine in ('chrome','firefox'):
      driver=None
      try:
       extension=directory/engine;shutil.copytree(ROOT/'dist/browser-extensions'/engine,extension)
       policy=extension/'policy.js';policy.write_text(policy.read_text(encoding='utf-8').replace('dev.localai.cowork.browser',name),encoding='utf-8')
       manifest=json.loads((extension/'manifest.json').read_text());manifest['host_permissions']=['<all_urls>'];(extension/'manifest.json').write_text(json.dumps(manifest))
       if engine=='chrome':
        options=ChromeOptions();options.binary_location=str(runtime(engine,platform));options.add_argument('--headless=new');options.add_argument('--user-data-dir='+str(directory/'chrome-profile'));options.add_argument('--disable-extensions-except='+str(extension));options.add_argument('--load-extension='+str(extension));driver=webdriver.Chrome(options=options);popup='chrome-extension://'+identity['chrome_id']+'/popup.html';api='chrome'
       else:
        options=FirefoxOptions();options.binary_location=str(runtime(engine,platform));options.add_argument('-headless');options.set_preference('xpinstall.signatures.required',False);driver=webdriver.Firefox(options=options,service=FirefoxService(service_args=['--allow-system-access']))
        archive=directory/'firefox.xpi'
        with zipfile.ZipFile(archive,'w') as bundle:
         for file in extension.iterdir():bundle.write(file,file.name)
        addon=driver.install_addon(str(archive),temporary=True);driver.set_context('chrome');mapping=json.loads(driver.execute_script("return Services.prefs.getStringPref('extensions.webextensions.uuids');"));driver.set_context('content');popup='moz-extension://'+mapping[addon]+'/popup.html';api='browser'
       driver.set_script_timeout(15);url=f'http://127.0.0.1:{server.server_port}/'
       driver.get(url+'shared');shared=driver.current_window_handle
       driver.switch_to.new_window('tab');driver.get(url+'unshared')
       driver.switch_to.new_window('tab');driver.get(popup)
       def extension_call(value):
        return driver.execute_async_script('const done=arguments[arguments.length-1];'+api+'.runtime.sendMessage(arguments[0]).then(done,e=>done({error:String(e)}));',value)
       extension_call({'type':'discover'});deadline=time.monotonic()+15
       while True:
        status=extension_call({'type':'status'})
        if any(v['id']==instance for v in status['instances']):break
        if time.monotonic()>deadline:raise RuntimeError('Native discovery failed: '+json.dumps(status))
        time.sleep(.1)
       selected=driver.execute_async_script('const done=arguments[arguments.length-1];'+api+'.tabs.query({}).then(t=>done(t.find(v=>v.url?.endsWith("/shared")).id));')
       extension_call({'type':'connect','instance_id':instance,'tabs':[selected]});deadline=time.monotonic()+15
       while not extension_call({'type':'status'})['connected']:
        if time.monotonic()>deadline:raise RuntimeError('Native connect timed out')
        time.sleep(.1)
       connections=peer_call({'operation':'connections'})['connections'];connection=next(v for v in connections if v['engine']==engine)
       assert len(connection['tabs'])==1 and connection['tabs'][0]['url'].endswith('/shared')
       tab=connection['tabs'][0];peer_call({'operation':'create','engine':engine,'connection_id':connection['id']})
       for action in model['actions']:peer_call({'operation':'action','command':{**action,'tab_id':tab['id'],'document_id':tab['document_id']}})
       observed=peer_call({'operation':'action','command':{'operation':'dom','tab_id':tab['id'],'document_id':tab['document_id']}})['result']
       assert model['expected'] in observed['dom'] and 'never-return-password' not in observed['dom']
       extension_call({'type':'disconnect'});peer_call({'operation':'close'})
       driver.switch_to.window(shared);assert 'retained=yes' in driver.execute_script('return document.cookie')
       report['engines'][engine]={'native_chain':True,'model_actions':True,'same_browser_cookie_context':True,'only_selected_tab':True};print(engine,'real native chain passed',flush=True)
      finally:
       if driver:driver.quit()
   finally:
    peer.stdin.close();peer.wait(timeout=15)
 finally:server.shutdown();server.server_close()
 if args.output:args.output.write_text(json.dumps(report,indent=2)+'\n')
 print(json.dumps(report,indent=2))
if __name__=='__main__':main()
