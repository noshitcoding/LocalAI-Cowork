"""Real daemon/provider process qualification of model reservations; disposable data only."""
from execution_plan_client import reviewed_run
import argparse,contextlib,hashlib,http.server,json,os,pathlib,socket,sqlite3,subprocess,tempfile,threading,time,urllib.request,urllib.error

class Provider(http.server.BaseHTTPRequestHandler):
    requests=[]
    gate=threading.Event()
    def log_message(self,*_):pass
    def do_GET(self):
        self.send_response(200);self.send_header('Content-Type','application/json');self.end_headers()
        self.wfile.write(json.dumps({'data':[{'id':m} for m in ['hold','measured','partial','overrun']]}).encode())
    def do_POST(self):
        body=json.loads(self.rfile.read(int(self.headers['Content-Length'])));self.requests.append(body)
        if body['model']=='hold':self.gate.wait(30)
        delta={'content':'Synthetic budget fixture completed. No real model was used.'}
        usage={'prompt_tokens':100,'completion_tokens':10}
        if body['model']=='partial':usage={'prompt_tokens':100}
        if body['model']=='overrun':
            usage['completion_tokens']=body['max_tokens']+1
            delta={'tool_calls':[{'index':0,'id':'blocked-effect','function':{'name':'create_artifact','arguments':json.dumps({'name':'must-not-exist.txt','content':'unsafe fixture'})}}]}
        payload=('data: '+json.dumps({'choices':[{'delta':delta,'finish_reason':'stop'}],'usage':usage})+'\n\ndata: [DONE]\n\n').encode()
        try:self.send_response(200);self.send_header('Content-Type','text/event-stream');self.end_headers();self.wfile.write(payload)
        except (BrokenPipeError,ConnectionResetError,ConnectionAbortedError):pass

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--binary',type=pathlib.Path,default=pathlib.Path('target/debug/cowork-daemon.exe' if os.name=='nt' else 'target/debug/cowork-daemon'));parser.add_argument('--serve-provider',action='store_true');parser.add_argument('--port',type=int,default=0);args=parser.parse_args()
    provider=http.server.ThreadingHTTPServer(('127.0.0.1',args.port),Provider)
    if args.serve_provider:Provider.gate.set();print(f'Synthetic budget provider: http://127.0.0.1:{provider.server_port}',flush=True);provider.serve_forever();return
    thread=threading.Thread(target=provider.serve_forever,daemon=True);thread.start();binary=args.binary.resolve();assert binary.is_file()
    with socket.socket() as sock:sock.bind(('127.0.0.1',0));port=sock.getsockname()[1]
    with tempfile.TemporaryDirectory(prefix='cowork-v2-budget-') as temp:
        data=pathlib.Path(temp);process=None;token='';env={**os.environ,'COWORK_DATA_DIR':str(data),'COWORK_PORT':str(port),'RUST_LOG':'error'}
        for key in ['COWORK_DATABASE_URL','COWORK_WEB_DIR','COWORK_ENABLE_TEST_PROVIDER']:env.pop(key,None)
        def call(method,path,body=None):
            request=urllib.request.Request(f'http://127.0.0.1:{port}/api/v2'+path,data=None if body is None else json.dumps(body).encode(),method=method,headers={'Authorization':'Bearer '+token,'Content-Type':'application/json'})
            with urllib.request.urlopen(request,timeout=5) as response:return json.load(response)
        def start():
            nonlocal process,token
            options={'creationflags':subprocess.CREATE_NO_WINDOW} if os.name=='nt' else {}
            process=subprocess.Popen([str(binary)],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE,**options)
            deadline=time.monotonic()+15
            while time.monotonic()<deadline:
                if process.poll() is not None:raise AssertionError(process.stderr.read().decode())
                try:token=(data/'daemon.token').read_text().strip();call('GET','/health');return
                except (FileNotFoundError,urllib.error.URLError,TimeoutError):time.sleep(.05)
            raise AssertionError('Daemon startup timeout')
        def kill():
            nonlocal process
            if process:process.kill();process.wait(timeout=5);process.stderr.close();process=None
        def wait(run,state):
            deadline=time.monotonic()+25
            while time.monotonic()<deadline:
                detail=call('GET','/runs/'+run)
                if detail['run']['state']==state:return detail
                time.sleep(.05)
            raise AssertionError(detail['run'])
        def new(model):
            order=call('POST','/orders',{'project_id':project['id'],'title':'Budget '+model})
            profile=call('POST','/providers',{'name':'Synthetic '+model,'kind':'openai_compatible','endpoint':f'http://127.0.0.1:{provider.server_port}','model':model,'context_tokens':32768})
            request={'order_id':order['id'],'base_revision':1,'idempotency_key':'once','prompt':'Synthetic budget qualification.','provider_id':profile['id'],'provider_revision':1,'access_mode':'autonomous','budgets':{'max_steps':64,'max_seconds':120,'max_output_tokens':4096,'max_total_tokens':32768}}
            return request,reviewed_run(call, request)['id']
        def sql(statement):
            with contextlib.closing(sqlite3.connect(data/'cowork-v2.sqlite3')) as db,db:db.execute(statement)
        def export(request):
            archive=call('GET',f"/orders/{request['order_id']}/export");assert archive['format_version']==13
            encoded=json.dumps(archive['data'],sort_keys=True,separators=(',',':'),ensure_ascii=False).encode();assert archive['sha256']==hashlib.sha256(encoded).hexdigest();return archive
        try:
            start();project=call('POST','/projects',{'name':'Model budget process fixture'})
            request,run=new('hold')
            deadline=time.monotonic()+10
            while not Provider.requests and time.monotonic()<deadline:time.sleep(.02)
            assert len(Provider.requests)==1;before=call('GET','/runs/'+run);assert before['token_budget']['charged_tokens']==32768 and before['run']['model_attempts'][0]['state']=='in_flight'
            kill();start();pending=wait(run,'interrupted');assert pending['token_budget']['charged_tokens']==32768;assert pending['run']['model_attempts'][0]['state']=='interrupted'
            Provider.gate.set();call('POST',f'/runs/{run}/resume',{});done=wait(run,'expired');assert len(Provider.requests)==1 and done['token_budget']['remaining_tokens']==0
            assert reviewed_run(call, request)['id']==run;assert export(request)['data']['runs'][0]['run']['model_attempts'][0]['request_sha256']
            request,run=new('measured');done=wait(run,'completed');assert done['token_budget']['charged_tokens']==110 and done['token_budget']['unconfirmed_tokens']==0,done['token_budget']
            assert len(done['run']['model_attempts'])==1,done['run']['model_attempts']
            attempt=done['run']['model_attempts'][0]
            assert attempt['state']=='measured' and attempt['usage']['input_tokens']==100 and attempt['usage']['output_tokens']==10,attempt
            archive=export(request);kill();start();assert export(request)['sha256']==archive['sha256'];assert reviewed_run(call, request)['id']==run
            request,run=new('partial');done=wait(run,'completed');assert done['token_budget']['charged_tokens']==32768 and done['run']['usage']['incomplete'];assert done['run']['usage']['input_tokens']==100
            request,run=new('overrun');done=wait(run,'expired');assert done['token_budget']['limit_exceeded'] and done['artifacts']==[] and done['approvals']==[]
            # Install before run admission so even an immediate provider cannot outrun this fault.
            sql("CREATE TRIGGER fail_model_receipt BEFORE INSERT ON events WHEN NEW.payload LIKE '%model_completed%' BEGIN SELECT RAISE(ABORT,'fixture accounting fault'); END")
            request,run=new('measured');done=wait(run,'failed');assert done['token_budget']['charged_tokens']==32768 and done['run']['model_attempts'][0]['state']=='interrupted'
            count=len(Provider.requests);kill();sql('DROP TRIGGER fail_model_receipt');start();assert call('GET','/runs/'+run)['token_budget']['charged_tokens']==32768
            assert reviewed_run(call, request)['id']==run and len(Provider.requests)==count==5
            export(request)
            print(json.dumps({'result':'passed','real_model':False,'checks':['reservation precedes HTTP','hard kill during request','lease recovery retains capacity','explicit resume cannot spend held capacity','measured and partial usage','overrun blocks tools','failed measurement transaction retains reservation','idempotent start','exact export digest after restart','no automatic provider replay']}))
        finally:Provider.gate.set();kill();provider.shutdown();provider.server_close();thread.join(timeout=5)
if __name__=='__main__':main()
