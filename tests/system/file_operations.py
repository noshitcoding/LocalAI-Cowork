"""Real file operations with injected receipt failures, exact approval and daemon restarts."""
from execution_plan_client import reviewed_run, selected_folder_capabilities
from file_action_client import prepare_file_action, recheck_file_action, commit_file_action
from urllib.parse import urlencode
import argparse, contextlib, hashlib, http.server, json, os, pathlib, socket, sqlite3, subprocess, tempfile, threading, time, urllib.request, urllib.error

class Provider(http.server.BaseHTTPRequestHandler):
    requests=[]
    def log_message(self,*_):pass
    def do_GET(self):
        self.send_response(200);self.send_header("Content-Type","application/json");self.end_headers();self.wfile.write(b'{"data":[{"id":"file-operations-fixture"}]}')
    def do_POST(self):
        data=json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        results={m["tool_call_id"]:json.loads(m["content"]) for m in data["messages"] if m["role"]=="tool"}
        grants = selected_folder_capabilities(data["messages"])
        folder=next(g for g in grants if g.get("order_id"))["id"]
        stages=[("mkdir","create_directory",{"path":"documents"}),
            ("write","write_text_file",{"path":"documents/note.txt","content":"retained fixture","expected_sha256":None}),
            ("stat-before","stat_file",{"path":"documents"}),
            ("rename","rename_file",{"path":"documents","destination":"renamed","expected_sha256":results.get("fixture-stat-before",{}).get("node",{}).get("sha256")}),
            ("stat-after","stat_file",{"path":"renamed"}),
            ("remove","remove_file",{"path":"renamed","expected_sha256":results.get("fixture-stat-after",{}).get("node",{}).get("sha256")})]
        pending=next((("fixture-"+key,name,args) for key,name,args in stages if "fixture-"+key not in results),None)
        delta={"content":"Local deterministic file-operation fixture; no actual model."}
        if pending:
            call,name,arguments=pending;arguments={"folder_id":folder,**arguments};type(self).requests.append(call)
            delta["tool_calls"]=[{"index":0,"id":call,"function":{"name":name,"arguments":json.dumps(arguments)}}]
        self.send_response(200);self.send_header("Content-Type","text/event-stream");self.end_headers()
        self.wfile.write(("data: "+json.dumps({"choices":[{"delta":delta,"finish_reason":"tool_calls" if pending else "stop"}]})+"\n\ndata: [DONE]\n\n").encode())

def main():
    parser=argparse.ArgumentParser();parser.add_argument("--binary",type=pathlib.Path,default=pathlib.Path("target/debug/cowork-daemon.exe" if os.name=="nt" else "target/debug/cowork-daemon"));parser.add_argument("--serve-provider",action="store_true");parser.add_argument("--port",type=int,default=43831);args=parser.parse_args()
    if args.serve_provider:http.server.ThreadingHTTPServer(("127.0.0.1",args.port),Provider).serve_forever();return
    binary=args.binary.resolve();assert binary.is_file()
    server=http.server.ThreadingHTTPServer(("127.0.0.1",0),Provider);threading.Thread(target=server.serve_forever,daemon=True).start()
    with socket.socket() as sock:sock.bind(("127.0.0.1",0));port=sock.getsockname()[1]
    with tempfile.TemporaryDirectory(prefix="cowork-v2-file-operations-") as temp:
        data=pathlib.Path(temp);process=None;token=""
        env={**os.environ,"COWORK_DATA_DIR":str(data),"COWORK_PORT":str(port),"RUST_LOG":"error"}
        for key in ["COWORK_DATABASE_URL","COWORK_WEB_DIR","COWORK_ENABLE_TEST_PROVIDER"]:env.pop(key,None)
        def call(method,path,body=None):
            request=urllib.request.Request(f"http://127.0.0.1:{port}/api/v2"+path,data=None if body is None else json.dumps(body).encode(),method=method,headers={"Authorization":"Bearer "+token,"Content-Type":"application/json"})
            with urllib.request.urlopen(request,timeout=10) as response:return json.load(response)
        def rejected(method,path,body,status):
            try:call(method,path,body)
            except urllib.error.HTTPError as error:assert error.code==status,(path,error.code,error.read())
            else:raise AssertionError("Expected rejection: "+path)
        def start():
            nonlocal process,token
            options={"creationflags":subprocess.CREATE_NO_WINDOW} if os.name=="nt" else {}
            process=subprocess.Popen([str(binary)],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE,**options)
            deadline=time.monotonic()+15
            while time.monotonic()<deadline:
                if process.poll() is not None:raise AssertionError(process.stderr.read().decode())
                try:token=(data/"daemon.token").read_text().strip();call("GET","/health");return
                except (FileNotFoundError,urllib.error.URLError,TimeoutError):time.sleep(.05)
            raise AssertionError("Daemon start timeout")
        def kill():
            nonlocal process
            if process:process.kill();process.wait(timeout=5);process.stderr.close();process=None
        def sql(statement):
            with contextlib.closing(sqlite3.connect(data/"cowork-v2.sqlite3")) as db,db:db.execute(statement)
        def wait_attention(run):
            deadline=time.monotonic()+20
            while time.monotonic()<deadline:
                detail=call("GET","/runs/"+run)
                if detail["run"]["state"] in ["interrupted","completed","failed","expired"] or (detail["run"]["state"]=="waiting_approval" and any(a["decision"] is None for a in detail["approvals"])):return detail
                time.sleep(.05)
            raise AssertionError(detail["run"])
        try:
            start();project=call("POST","/projects",{"name":"Native file-operation fixture"})
            order=call("POST","/orders",{"project_id":project["id"],"title":"Retained directory operations"})["id"]
            profile=call("POST","/providers",{"name":"Local operation fixture","kind":"openai_compatible","endpoint":f"http://127.0.0.1:{server.server_port}","model":"file-operations-fixture","context_tokens":32768})
            request={"order_id":order,"base_revision":1,"idempotency_key":"operations-once","prompt":"Create, write, inspect, rename and remove only the fixture directory.","provider_id":profile["id"],"provider_revision":1,"access_mode":"confirm_all","folders":[]}
            run=reviewed_run(call, request)["id"];faults=set();approvals=[]
            while True:
                detail=wait_attention(run);state=detail["run"]["state"]
                if state=="completed":break
                assert state in ["waiting_approval","interrupted"],detail["run"]
                if state=="waiting_approval":
                    approval=next(a for a in detail["approvals"] if a["decision"] is None)
                    name=approval["tool"]["name"];approvals.append(name)
                    if name in ["create_directory","rename_file","remove_file"]:
                        sql("CREATE TRIGGER fail_operation_receipt BEFORE INSERT ON file_operations BEGIN SELECT RAISE(ABORT,'fixture commit failure'); END")
                    rejected("POST",f"/runs/{run}/approvals/{approval['id']}",{"fingerprint":"changed","allow":True},409)
                    call("POST",f"/runs/{run}/approvals/{approval['id']}",{"fingerprint":approval["fingerprint"],"allow":True})
                else:
                    operation=detail["uncertain_file_operation"];assert operation and operation["kind"] not in faults
                    faults.add(operation["kind"]);rejected("POST",f"/runs/{run}/resume",{},409);rejected("POST",f"/runs/{run}/cancel",{},409)
                    kill();sql("DROP TRIGGER fail_operation_receipt");start()
                    again=call("GET","/runs/"+run);assert again["uncertain_file_operation"]==operation
                    decision={"fingerprint":again["uncertain_file_fingerprint"],"decision":"confirm_not_applied"}
                    rejected("POST",f"/runs/{run}/files/reconcile",decision,409)
                    decision["decision"]="accept_completed";assert call("POST",f"/runs/{run}/files/reconcile",decision)["state"]=="interrupted"
                    call("POST",f"/runs/{run}/resume",{})
            assert faults=={"create_directory","rename","remove"}
            assert approvals==["create_directory","write_text_file","stat_file","rename_file","stat_file","remove_file"]
            assert len(Provider.requests)==len(set(Provider.requests))==6,"A completed operation was requested again"
            assert len(detail["file_operations"])==3 and not detail["uncertain_file_operation"]
            assert call("GET",f"/orders/{order}/workspace/files?path=.")["entries"]==[]
            original=data/"file-broker"/"versions"/hashlib.sha256(b"retained fixture").hexdigest();assert original.read_bytes()==b"retained fixture"
            export=call("GET",f"/orders/{order}/export");assert export["format_version"]==13
            assert export["sha256"]==hashlib.sha256(json.dumps(export["data"],sort_keys=True,separators=(",",":"),ensure_ascii=False).encode()).hexdigest()
            kill();start();assert len(call("GET","/runs/"+run)["file_operations"])==3
            assert reviewed_run(call, request)["id"]==run
            # A native restoration must work with every provider removed, and preserve drafts.
            current=call("GET",f"/orders/{order}")
            draft=call("PUT",f"/orders/{order}/draft",{"base_revision":current["revision"],"draft":"Keep my unrelated draft"})
            folder=call("GET",f"/orders/{order}/workspace")["folder"]["id"]
            restore={"order_id":order,"base_revision":draft["revision"],"idempotency_key":"restore-once","prompt":"Restore retained directory","access_mode":"confirm_mutations","restore":{"folder_id":folder,"path":"recovered","source":{"run_id":run,"tool_call_id":"fixture-remove","version":"operation_before"}}}
            kill();sql("DELETE FROM providers");(data/"providers.json").write_text("[]",encoding="utf-8");start()
            assert call("GET","/providers")==[]
            rejected("POST","/file-restorations",restore,409)
            restore_review=prepare_file_action(call,"restore",restore)
            restored=commit_file_action(call,restore_review)["id"]
            assert commit_file_action(call,recheck_file_action(call,restore_review))["id"]==restored
            approval=next(a for a in wait_attention(restored)["approvals"] if a["decision"] is None)
            assert call("GET",f"/orders/{order}")["draft"]=="Keep my unrelated draft"
            sql("CREATE TRIGGER fail_restore_receipt BEFORE INSERT ON file_operations BEGIN SELECT RAISE(ABORT,'restoration commit fault'); END")
            call("POST",f"/runs/{restored}/approvals/{approval['id']}",{"fingerprint":approval["fingerprint"],"allow":True})
            uncertain=wait_attention(restored);assert uncertain["run"]["state"]=="interrupted" and uncertain["run"]["spec"]["provider"] is None
            assert uncertain["uncertain_file_operation"]["restored_from"]==restore["restore"]["source"]
            kill();sql("DROP TRIGGER fail_restore_receipt");start()
            uncertain=call("GET","/runs/"+restored)
            decision={"fingerprint":uncertain["uncertain_file_fingerprint"],"decision":"confirm_not_applied"}
            rejected("POST",f"/runs/{restored}/files/reconcile",decision,409)
            decision["decision"]="accept_completed";call("POST",f"/runs/{restored}/files/reconcile",decision)
            call("POST",f"/runs/{restored}/resume",{})
            final=wait_attention(restored);assert final["run"]["state"]=="completed" and len(final["file_operations"])==1
            assert len(final["run"]["spec"]["folders"])==1
            restored_folder=final["run"]["spec"]["folders"][0]["id"]
            restored_location=urlencode({"folder_id":restored_folder,"path":"recovered/note.txt"})
            assert call("GET",f"/orders/{order}/runs/{restored}/working-copy/text?"+restored_location)["content"]=="retained fixture"
            assert call("GET",f"/orders/{order}/workspace/files?path=.")["entries"]==[]
            assert not any(e["kind"].startswith("model_") for e in call("GET",f"/runs/{restored}/events?after=0"))
            assert len(Provider.requests)==6
            assert call("GET",f"/orders/{order}")["draft"]=="Keep my unrelated draft"
            kill();start();assert commit_file_action(call,recheck_file_action(call,restore_review))["id"]==restored
            archive=call("GET",f"/orders/{order}/export");assert archive["format_version"]==13
            assert archive["sha256"]==hashlib.sha256(json.dumps(archive["data"],sort_keys=True,separators=(",",":"),ensure_ascii=False).encode()).hexdigest()
            print(json.dumps({"result":"passed","real_model":False,"checks":["exact approvals including stat","native directory creation/rename/removal","three actual effects followed by failed DB receipts","restart persistence","wrong reconciliation rejected","no resume/cancel before reconciliation","separate explicit resume","no repeated operation","retained content","receipt export hash","idempotent run replay","snapshot-bound restoration with all providers removed", "legacy direct restoration rejected", "restoration affects its independent copy only","restoration receipt failure and restart","no duplicate restore","preserved draft","restoration provenance in export"]}))
        finally:kill();server.shutdown();server.server_close()

if __name__=="__main__":main()
