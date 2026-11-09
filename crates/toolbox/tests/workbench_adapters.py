"""Real temporary-repository and binary document adapter roundtrips. No network writes."""
import hashlib,importlib.util,json,os,pathlib,sqlite3,subprocess,sys,tempfile,unittest,zipfile
from unittest.mock import patch
SOURCE=pathlib.Path(__file__).resolve().parents[1]/'src'
spec=importlib.util.spec_from_file_location('structured_git',SOURCE/'git.py');git=importlib.util.module_from_spec(spec);spec.loader.exec_module(git)
spec=importlib.util.spec_from_file_location('pinned_pr',SOURCE/'pull_request.py');pr=importlib.util.module_from_spec(spec);spec.loader.exec_module(pr)
class GitTests(unittest.TestCase):
 def setUp(self):
  self.directory=tempfile.TemporaryDirectory();self.root=pathlib.Path(self.directory.name);self.repository=git.Repository(self.root,os.environ['COWORK_TEST_GIT'],'repository')
 def tearDown(self):self.directory.cleanup()
 def run_action(self,operation,**fields):return self.repository.execute({'operation':operation,'expected_state':self.repository.state()['binding'],**fields})
 def test_review_state_staging_exact_bytes_commit_and_branches(self):
  self.assertFalse(self.run_action('status')['state']['exists']);self.run_action('init');source=self.repository.path/'hello.txt';source.write_text('hello\n',encoding='utf8');before=self.repository.state();source.write_text('changed\n',encoding='utf8')
  with self.assertRaisesRegex(AssertionError,'changed since review'):self.repository.execute({'operation':'stage','paths':['hello.txt'],'expected_state':before['binding']})
  self.run_action('stage',paths=['hello.txt']);self.assertIn('changed',self.run_action('diff',reference='staged')['text'])
  hook=self.repository.path/'.git/hooks/pre-commit';hook.write_text('#!/bin/sh\nexit 99\n',encoding='utf8');hook.chmod(0o700)
  self.run_action('commit',text=json.dumps({'message':'Fixture commit','author_name':'Test','author_email':'test@example.invalid'}));self.assertTrue(self.repository.state()['head']);self.run_action('create_branch',reference='fixture');self.run_action('switch_branch',reference='fixture');self.assertEqual(self.repository.state()['branch'],'fixture');self.run_action('switch_branch',reference='main');self.run_action('delete_branch',reference='fixture')
 def test_unstage_and_external_command_config_rejected(self):
  self.run_action('init');(self.repository.path/'a.txt').write_text('a');self.run_action('stage',paths=['a.txt']);self.run_action('unstage',paths=['a.txt']);self.assertEqual(self.repository.state()['entries'][0]['status'],'??')
  self.repository.call('config','filter.danger.clean','echo dangerous')
  with self.assertRaisesRegex(AssertionError,'external command'):self.repository.state()
 @unittest.skipUnless(os.name=='nt','Windows namespace regression')
 def test_extended_windows_root_roundtrip_and_other_repository_rejected(self):
  self.repository=git.Repository('\\\\?\\'+str(self.root),os.environ['COWORK_TEST_GIT'],'repository')
  self.run_action('init');(self.repository.path/'windows.txt').write_text('Windows namespace')
  self.run_action('stage',paths=['windows.txt']);self.run_action('commit',text=json.dumps({'message':'Windows namespace','author_name':'Test','author_email':'test@example.invalid'}))
  self.assertTrue(self.run_action('status')['state']['head'])
  other=self.root/'other';other.mkdir()
  original=self.repository.call
  def redirected(*args,**kwargs):
   if args==('rev-parse','--show-toplevel'):return subprocess.CompletedProcess([],0,str(other).encode(),b'')
   return original(*args,**kwargs)
  with patch.object(self.repository,'call',redirected):
   with self.assertRaisesRegex(AssertionError,'root does not match'):self.repository.state()
class OfficeTests(unittest.TestCase):
 def setUp(self):self.directory=tempfile.TemporaryDirectory();self.root=pathlib.Path(self.directory.name)
 def tearDown(self):self.directory.cleanup()
 def call(self,action,ok=True):
  result=subprocess.run([sys.executable,'-I',str(SOURCE/'office.py'),str(self.root),json.dumps(action)],capture_output=True,text=True)
  if not ok:self.assertNotEqual(result.returncode,0);return
  self.assertEqual(result.returncode,0,result.stderr);return json.loads(result.stdout)
 def test_docx_copy_edit_retains_original_and_unmodified_members(self):
  self.call({'operation':'create_docx','output':'original.docx','text':'Hello\nWorld'});before=(self.root/'original.docx').read_bytes();model=self.call({'operation':'inspect_docx','input':'original.docx'})['model'];self.assertEqual(model['items'][0]['text'],'Hello')
  self.call({'operation':'edit_docx','input':'original.docx','output':'edited.docx','text':json.dumps({'source_sha256':model['source_sha256'],'patches':[{'paragraph':0,'text':'Changed 🧠'}]})});self.assertEqual((self.root/'original.docx').read_bytes(),before);self.assertEqual(self.call({'operation':'inspect_docx','input':'edited.docx'})['model']['items'][0]['text'],'Changed 🧠')
  with zipfile.ZipFile(self.root/'original.docx') as original,zipfile.ZipFile(self.root/'edited.docx') as edited:self.assertEqual(original.read('[Content_Types].xml'),edited.read('[Content_Types].xml'))
  self.call({'operation':'edit_docx','input':'original.docx','output':'bad.docx','text':json.dumps({'source_sha256':'0'*64,'patches':[{'paragraph':0,'text':'Stale'}]})},False);self.assertFalse((self.root/'bad.docx').exists())
 def test_xlsx_explicit_formula_and_text_are_distinct(self):
  self.call({'operation':'create_xlsx','output':'a.xlsx','text':'value,=1+1'});model=self.call({'operation':'inspect_xlsx','input':'a.xlsx'})['model'];self.assertEqual(model['items'][1]['type'],'text')
  self.call({'operation':'edit_xlsx','input':'a.xlsx','output':'b.xlsx','text':json.dumps({'source_sha256':model['source_sha256'],'patches':[{'sheet':'Sheet','cell':'A1','value':'=2+2','type':'formula'},{'sheet':'Sheet','cell':'B1','value':'=3+3','type':'text'}]})});items=self.call({'operation':'inspect_xlsx','input':'b.xlsx'})['model']['items'];self.assertEqual([i['type'] for i in items],['formula','text'])
 def test_word_compatibility_prefixes_survive_paragraph_edit(self):
  self.call({'operation':'create_docx','output':'source.docx','text':'Compatibility'});source=self.root/'source.docx'
  with zipfile.ZipFile(source) as archive:members=[(item,archive.read(item.filename)) for item in archive.infolist()]
  with zipfile.ZipFile(source,'w') as archive:
   for item,data in members:
    if item.filename=='word/document.xml':data=data.replace(b'<w:document ',b'<w:document xmlns:testUnused="urn:word-compatibility-fixture" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" mc:Ignorable="testUnused" ',1)
    archive.writestr(item,data)
  model=self.call({'operation':'inspect_docx','input':'source.docx'})['model'];self.call({'operation':'edit_docx','input':'source.docx','output':'edited.docx','text':json.dumps({'source_sha256':model['source_sha256'],'patches':[{'paragraph':0,'text':'Edited compatibility'}]})})
  with zipfile.ZipFile(self.root/'edited.docx') as archive:xml=archive.read('word/document.xml');self.assertIn(b'xmlns:testUnused="urn:word-compatibility-fixture"',xml);self.assertIn(b'mc:Ignorable="testUnused"',xml)
 def test_pptx_text_edit_and_output_never_overwrites(self):
  self.call({'operation':'create_pptx','output':'a.pptx','text':'Hello','title':'Fixture'});model=self.call({'operation':'inspect_pptx','input':'a.pptx'})['model'];item=model['items'][-1];action={'operation':'edit_pptx','input':'a.pptx','output':'b.pptx','text':json.dumps({'source_sha256':model['source_sha256'],'patches':[{'slide':item['slide'],'shape':item['shape'],'text':'Changed slide'}]})};self.call(action);self.assertEqual(self.call({'operation':'inspect_pptx','input':'b.pptx'})['model']['items'][-1]['text'],'Changed slide');self.call(action,False)
 def test_real_binary_knowledge_extraction_for_pdf_and_all_office_formats(self):
  for extension,label in [('docx','Paragraph'),('xlsx','Sheet'),('pptx','Slide'),('pdf','Page')]:
   self.call({'operation':'create_'+extension,'output':'evidence.'+extension,'text':'Retained knowledge evidence','title':'Fixture'});source=self.root/('evidence.'+extension);digest=hashlib.sha256(source.read_bytes()).hexdigest();action={'input':source.name,'text':json.dumps({'source_sha256':digest})};result=subprocess.run([sys.executable,'-I',str(SOURCE/'knowledge_extract.py'),str(self.root),json.dumps(action)],capture_output=True,text=True);self.assertEqual(result.returncode,0,result.stderr);value=json.loads(result.stdout);self.assertEqual(value['source_sha256'],digest);self.assertEqual(value['text_sha256'],hashlib.sha256(value['text'].encode()).hexdigest());self.assertIn('Retained knowledge evidence',value['text']);self.assertIn('['+label,value['text'])
class PullRequestTests(unittest.TestCase):
 def test_server_merge_is_bound_to_exact_sha_and_changed_head_never_sends_write(self):
  calls=[];head='a'*40
  class FakeGh:
   def __init__(self,command,**options):self.command=command;self.options=options;self.returncode=0
   def communicate(self,input=None,timeout=None):
    calls.append((self.command,json.loads(input) if input else None));value={'head':{'sha':head},'base':{'sha':'b'*40},'title':'Fixture','state':'open','html_url':'https://example.invalid/pr','draft':False} if self.command[self.command.index('--method')+1]=='GET' else {'merged':True};self.options['stdout'].write(json.dumps(value).encode())
  with patch.object(pr.subprocess,'Popen',FakeGh):
   pr.execute('gh',{'repository':'owner/repo','number':1,'operation':'merge','expected_head':head,'merge_method':'squash'});self.assertEqual(calls[-1][1],{'sha':head,'merge_method':'squash'});calls.clear()
   with self.assertRaisesRegex(AssertionError,'head changed'):pr.execute('gh',{'repository':'owner/repo','number':1,'operation':'merge','expected_head':'c'*40,'merge_method':'squash'})
   self.assertEqual(len(calls),1);self.assertIsNone(calls[0][1])
class PipelineCaptureTests(unittest.TestCase):
 def test_actual_sqlite_rpc_capture_retains_args_and_zero_context_without_config(self):
  sys.path.insert(0,str(SOURCE.parents[2]/'tools/migration'))
  spec=importlib.util.spec_from_file_location('v1_capture',SOURCE/'v1_import.py');capture=importlib.util.module_from_spec(spec);spec.loader.exec_module(capture)
  with sqlite3.connect(':memory:') as connection:
   connection.execute('CREATE TABLE rpc_pipelines(id TEXT,name TEXT,description TEXT,steps_json TEXT,zero_context INTEGER)');steps=[{'tool':'web_fetch','args':'https://example.invalid/page'},{'tool':'planner','prompt':'Review evidence'}];connection.execute('INSERT INTO rpc_pipelines VALUES(?,?,?,?,?)',('rpc','Original','Description',json.dumps(steps),1));item=capture.sqlite_catalog(connection,'desktop')['objects'][0];source=json.loads(item['instructions']);self.assertTrue(source['zero_context']);self.assertEqual(json.loads(source['steps_json']),steps);self.assertIsNone(item['base_url'])
   connection.execute('UPDATE rpc_pipelines SET steps_json=?',(json.dumps([{'args':{'api_key':'fixture-secret'}}]),))
   with self.assertRaisesRegex(AssertionError,'credential fields'):capture.sqlite_catalog(connection,'desktop')
if __name__=='__main__':unittest.main()
