"""Actual two-engine adapter acceptance on local pages and owned test profiles."""
import argparse,hashlib,http.server,json,os,pathlib,subprocess,sys,tempfile,threading,time,uuid,zipfile
ROOT=pathlib.Path(__file__).resolve().parents[2]

class Handler(http.server.BaseHTTPRequestHandler):
 def log_message(self,*args):pass
 def do_GET(self):
  body=b'''<title>Cowork fixture</title><style>body{min-height:2400px}</style><h1 id="result">Ready</h1><input id="entry"><input type="password" value="never-return-password"><select id="choice"><option value="a">A</option><option value="b">B</option></select><button id="save" onclick="document.querySelector('#result').textContent=document.querySelector('#entry').value;document.cookie='retained=yes;path=/'">Save</button><script>console.log('browser fixture');document.querySelector('#result').dataset.cookie=document.cookie</script>'''
  self.send_response(200);self.send_header('content-type','text/html');self.send_header('content-length',str(len(body)));self.end_headers();self.wfile.write(body)

def runtime(engine,platform):
 recipe=json.loads((ROOT/'tools/toolbox/browser-runtimes.lock.json').read_text(encoding='utf-8'))['platforms'][platform][engine]
 archive=ROOT/'.local/browser-runtime-archives'/f'{engine}-{platform}-{recipe["version"]}.zip'
 if hashlib.file_digest(archive.open('rb'),'sha256').hexdigest()!=recipe['sha256']:raise RuntimeError('Runtime archive hash mismatch')
 destination=ROOT/'.local/browser-test-runtimes'/platform/engine
 executable=destination/recipe['executable']
 if not executable.exists():
  destination.mkdir(parents=True,exist_ok=True)
  with zipfile.ZipFile(archive) as package:
   for member in package.infolist():
    if pathlib.PurePosixPath(member.filename).is_absolute() or '..' in pathlib.PurePosixPath(member.filename).parts:raise RuntimeError('Unsafe runtime entry')
   package.extractall(destination)
   if os.name!='nt':
    for member in package.infolist():
     path=destination/member.filename
     if path.is_file():path.chmod(0o700 if (member.external_attr>>16)&0o111 else 0o600)
 return executable

def exercise(engine,executable,url):
 checks=[]
 with tempfile.TemporaryDirectory(prefix='cowork-browser-smoke-') as temporary:
  root=pathlib.Path(temporary);profile=root/'profile'
  def start(number):
   directory=root/str(number);directory.mkdir()
   config={'session_id':str(uuid.uuid4()),'epoch':str(uuid.uuid4()),'engine':engine,'executable':str(executable),'executable_sha256':hashlib.file_digest(executable.open('rb'),'sha256').hexdigest(),'profile':str(profile),'workspace':str(root)}
   (directory/'config.json').write_text(json.dumps(config),encoding='utf-8')
   child=subprocess.Popen([sys.executable,'-I',str(ROOT/'crates/toolbox/src/managed_browser.py'),str(directory)],stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.PIPE,**({'creationflags':0x08000000} if os.name=='nt' else {'start_new_session':True}))
   deadline=time.monotonic()+60
   while not (directory/'ready.json').exists():
    if child.poll() is not None:raise RuntimeError('Browser startup: '+child.stderr.read().decode(errors='replace'))
    if time.monotonic()>deadline:raise RuntimeError('Browser startup timed out')
    time.sleep(.05)
   def call(**body):
    identity=str(uuid.uuid4());request=directory/(identity+'.request.json');reply=directory/(identity+'.response.json')
    request.write_text(json.dumps({'body':body,'timeout_seconds':20}),encoding='utf-8');deadline=time.monotonic()+30
    while not reply.exists():
     if child.poll() is not None or time.monotonic()>deadline:raise RuntimeError('Browser IPC timed out')
     time.sleep(.02)
    result=json.loads(reply.read_text(encoding='utf-8'))['result'];reply.unlink()
    assert result['context_epoch']==config['epoch']
    return result
   return directory,child,call
  directory,child,call=start(1)
  try:
   initial=call(operation='navigate',url=url);assert initial['ok'];assert 'never-return-password' not in initial['dom'];checks.append('navigation_and_password_redaction')
   original=initial['tab_id'];stale=initial['document_id']
   assert call(operation='fill',selector='#entry',text='Retained value')['ok']
   assert call(operation='select',selector='#choice',text='b')['ok']
   result=call(operation='click',selector='#save');assert 'Retained value' in result['dom'];checks.append('fill_select_click')
   frame=call(operation='frame');assert frame['frame']['image'].startswith('data:image/jpeg;base64,');checks.append('live_frame')
   assert call(operation='resize',width=960,height=720)['ok'];assert call(operation='frame')['frame']['width']==960
   assert call(operation='pointer',x=20,y=20)['ok'];assert call(operation='key',text='End')['ok'];assert call(operation='scroll',x=0,y=400)['ok'];checks.append('pointer_keyboard_scroll_resize')
   assert call(operation='annotate',selector='#result',text='Note')['ok'];assert len(call(operation='annotations')['annotations'])==1
   assert call(operation='clear_annotations')['annotations']==[];assert call(operation='console')['console'];assert call(operation='network')['network'];checks.append('developer_tools')
   new=call(operation='new_tab',url=url+'second');assert len(new['tabs'])==2
   assert call(operation='activate_tab',tab_id=original)['ok'];assert call(operation='reload')['ok']
   rejected=call(operation='click',tab_id=original,document_id=stale,selector='#save');assert not rejected['ok'] and rejected['error']=='browser_document_changed';checks.append('stale_document_rejected')
   assert call(operation='back')['ok'];assert call(operation='forward')['ok'];assert call(operation='close_tab')['ok'];checks.append('tabs_history_reload')
   assert call(operation='screenshot')['screenshot_path'];checks.append('screenshot')
  finally:
   (directory/'stop').touch();child.wait(timeout=15)
  directory,child,call=start(2)
  try:
   page=call(operation='navigate',url=url);assert 'retained=yes' in page['dom'];checks.append('login_cookie_persists_after_restart')
  finally:(directory/'stop').touch();child.wait(timeout=15)
 return checks

def main():
 parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--output',type=pathlib.Path);args=parser.parse_args()
 platform='windows' if os.name=='nt' else 'linux';server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler);threading.Thread(target=server.serve_forever,daemon=True).start()
 report={'platform':platform,'engines':{}}
 try:
  for engine in ('chrome','firefox'):
   print('Testing',platform,engine,flush=True);report['engines'][engine]=exercise(engine,runtime(engine,platform),f'http://127.0.0.1:{server.server_port}/');print(engine,'passed',flush=True)
 finally:server.shutdown();server.server_close()
 if args.output:args.output.write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8')
 print(json.dumps(report,indent=2))
if __name__=='__main__':main()
