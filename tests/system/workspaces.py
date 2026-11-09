"""Managed file snapshots through real HTTP, filesystem and process restart boundaries.

Only disposable fixtures are modified; the SQLite fault trigger is test data, not a product hook.
"""
from execution_plan_client import reviewed_run, selected_folder_capabilities
import argparse, contextlib, hashlib, http.server, json, os, pathlib, socket, sqlite3, subprocess, tempfile, threading, time, urllib.request, urllib.error

class Provider(http.server.BaseHTTPRequestHandler):
    calls = 0
    def log_message(self, *_): pass
    def do_GET(self):
        self.send_response(200); self.send_header("Content-Type", "application/json"); self.end_headers()
        self.wfile.write(b'{"data":[{"id":"workspace-fixture"}]}')
    def do_POST(self):
        type(self).calls += 1
        data = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        prompt = next(m["content"] for m in reversed(data["messages"]) if m["role"] == "user")
        call = "workspace-" + prompt
        done = any(m.get("tool_call_id") == call for m in data["messages"] if m["role"] == "tool")
        grants = selected_folder_capabilities(data["messages"])
        folder = next(g for g in grants if g.get("order_id"))
        delta = {"content": "Fixture finished: " + prompt}
        if not done:
            args = {"folder_id":folder["id"],"path":"note.txt","content":prompt,"expected_sha256":None if prompt=="first" else hashlib.sha256(b"first").hexdigest()}
            delta["tool_calls"] = [{"index":0,"id":call,"function":{"name":"write_text_file","arguments":json.dumps(args)}}]
        self.send_response(200); self.send_header("Content-Type","text/event-stream"); self.end_headers()
        self.wfile.write(("data: "+json.dumps({"choices":[{"delta":delta,"finish_reason":"stop" if done else "tool_calls"}]})+"\n\ndata: [DONE]\n\n").encode())

def main():
    parser=argparse.ArgumentParser();parser.add_argument("--binary",type=pathlib.Path,default=pathlib.Path("target/debug/cowork-daemon.exe" if os.name=="nt" else "target/debug/cowork-daemon"));args=parser.parse_args()
    binary=args.binary.resolve();assert binary.is_file()
    provider=http.server.ThreadingHTTPServer(("127.0.0.1",0),Provider)
    threading.Thread(target=provider.serve_forever,daemon=True).start()
    with socket.socket() as sock: sock.bind(("127.0.0.1",0));port=sock.getsockname()[1]
    with tempfile.TemporaryDirectory(prefix="cowork-v2-workspaces-") as temp:
        data=pathlib.Path(temp);process=None;token=""
        env={**os.environ,"COWORK_DATA_DIR":str(data),"COWORK_PORT":str(port),"RUST_LOG":"error"}
        for key in ["COWORK_DATABASE_URL","COWORK_WEB_DIR","COWORK_ENABLE_TEST_PROVIDER"]:env.pop(key,None)
        def call(method,path,body=None):
            req=urllib.request.Request(f"http://127.0.0.1:{port}/api/v2"+path,data=None if body is None else json.dumps(body).encode(),method=method,headers={"Authorization":"Bearer "+token,"Content-Type":"application/json"})
            with urllib.request.urlopen(req,timeout=10) as response:return json.load(response)
        def download(order,path,run=None,expected=None):
            query=urllib.parse.urlencode({"path":path,**({"expected_sha256":expected} if expected else {})})
            route=f"/orders/{order}"+(f"/runs/{run}" if run else "")+"/workspace/download?"+query
            req=urllib.request.Request(f"http://127.0.0.1:{port}/api/v2"+route,headers={"Authorization":"Bearer "+token})
            with urllib.request.urlopen(req,timeout=10) as response:
                content=response.read()
                assert response.headers["Content-Type"]=="application/octet-stream"
                assert int(response.headers["Content-Length"])==len(content)
                assert response.headers["X-Cowork-Sha256"]==hashlib.sha256(content).hexdigest()
                assert urllib.parse.unquote(response.headers["X-Cowork-File-Name"])==path.split("/")[-1]
                assert response.headers["Content-Disposition"].startswith("attachment;")
                return content
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
        def wait(run,state):
            deadline=time.monotonic()+20
            while time.monotonic()<deadline:
                detail=call("GET","/runs/"+run)
                if detail["run"]["state"]==state:return detail
                time.sleep(.05)
            raise AssertionError((state,detail["run"]))
        def create_run(order,prompt):
            revision=call("GET","/orders/"+order)["revision"]
            return reviewed_run(call, {"order_id":order,"base_revision":revision,"idempotency_key":"run-"+prompt,"prompt":prompt,"provider_id":model["id"],"provider_revision":1,"access_mode":"autonomous","folders":[]})["id"]
        def text(order):return call("GET",f"/orders/{order}/workspace/text?path=note.txt")["content"]
        try:
            start();project=call("POST","/projects",{"name":"Workspace process fixture"})
            order=call("POST","/orders",{"project_id":project["id"],"title":"Historical files"})["id"]
            model=call("POST","/providers",{"name":"Workspace HTTP fixture","kind":"openai_compatible","endpoint":f"http://127.0.0.1:{provider.server_port}","model":"workspace-fixture","context_tokens":32768})
            workspace=call("GET",f"/orders/{order}/workspace")
            assert workspace["folder"]["order_id"]==order
            assert call("GET","/folders?project_id="+project["id"])==[]
            # Binary fixture belongs only to this disposable managed directory.
            working=data/"file-broker"/"workspaces"/order/"files"
            (working/"nested").mkdir();binary_content=bytes(range(256))*8192
            (working/"nested"/"binary.bin").write_bytes(binary_content)
            with contextlib.closing(sqlite3.connect(data/"cowork-v2.sqlite3")) as db,db:
                db.execute("CREATE TRIGGER test_fail_snapshot BEFORE INSERT ON workspace_snapshots BEGIN SELECT RAISE(ABORT,'snapshot commit fault'); END")
            first=create_run(order,"first");interrupted=wait(first,"interrupted")
            assert interrupted["workspace_snapshot"] is None
            assert not any(m["role"]=="assistant" for m in call("GET",f"/orders/{order}/messages"))
            assert text(order)=="first";mtime=(working/"note.txt").stat().st_mtime_ns;calls=Provider.calls
            kill()
            with contextlib.closing(sqlite3.connect(data/"cowork-v2.sqlite3")) as db,db:db.execute("DROP TRIGGER test_fail_snapshot")
            start();assert call("GET",f"/orders/{order}/workspace")==workspace
            call("POST",f"/runs/{first}/resume",{});done=wait(first,"completed")
            assert Provider.calls==calls,"Snapshot resume repeated the model request"
            assert (working/"note.txt").stat().st_mtime_ns==mtime,"Snapshot resume repeated the write"
            snapshot=done["workspace_snapshot"];assert len(snapshot["entries"])==3
            manifest={k:v for k,v in snapshot.items() if k!="sha256"}
            assert snapshot["sha256"]==hashlib.sha256(json.dumps(manifest,sort_keys=True,separators=(",",":"),ensure_ascii=False).encode()).hexdigest()
            boundary=next(m for m in call("GET",f"/orders/{order}/messages") if m["role"]=="assistant")
            second=create_run(order,"second");wait(second,"completed");assert text(order)=="second"
            assert download(order,"note.txt")==b"second"
            assert download(order,"note.txt",first)==b"first"
            assert download(order,"nested/binary.bin",first)==binary_content
            try:download(order,"note.txt",expected=hashlib.sha256(b"first").hexdigest())
            except urllib.error.HTTPError as e:assert e.code==409
            else:raise AssertionError("Changed current file downloaded against a stale preview")
            request={"title":"Independent older copy","through_message_id":boundary["id"],"idempotency_key":"branch-once"}
            branch=call("POST",f"/orders/{order}/branches",request)["id"]
            assert text(branch)=="first"
            branch_files=data/"file-broker"/"workspaces"/branch/"files"
            assert (branch_files/"nested"/"binary.bin").read_bytes()==binary_content
            third=create_run(branch,"third");wait(third,"completed");assert text(branch)=="third" and text(order)=="second"
            assert call("POST",f"/orders/{order}/branches",request)["id"]==branch
            assert text(branch)=="third","Idempotent branch replay replaced its later files"
            kill();start();assert text(order)=="second" and text(branch)=="third"
            assert download(order,"note.txt",first)==b"first"
            assert download(order,"nested/binary.bin",first)==binary_content
            try:download(branch,"note.txt",first)
            except urllib.error.HTTPError as e:assert e.code==403
            else:raise AssertionError("A run from another order was accepted")
            try:call("GET",f"/orders/{branch}/workspace/text?path=../workspace.json")
            except urllib.error.HTTPError as e:assert e.code==403
            else:raise AssertionError("Traversal was allowed")
            archive=call("GET",f"/orders/{branch}/export")
            assert archive["format_version"]==13 and archive["data"]["history"]["inherited"]["workspace_snapshots"][0]==snapshot
            blob=data/"file-broker"/"versions"/hashlib.sha256(binary_content).hexdigest()
            blob.write_bytes(b"corrupted retained content")
            try:download(order,"nested/binary.bin",first)
            except urllib.error.HTTPError as e:assert e.code==400
            else:raise AssertionError("Corrupt retained bytes downloaded successfully")
            assert download(order,"nested/binary.bin")==binary_content
            print(json.dumps({"result":"passed","real_model":False,"checks":["persistent managed directory","explicit external grants unchanged","snapshot commit failure rolls back result","restart and resume without model or write replay","binary content snapshot","independent manifest digest","older result branch","branch isolation","idempotent branch replay","restart persistence","path traversal denied","exported snapshot origin","verified current and historical binary downloads across restart","foreign-order snapshot rejected","stale preview rejected","corrupt retained bytes rejected"]}))
        finally:kill();provider.shutdown();provider.server_close()

if __name__=="__main__":main()
