"""Real daemon + HTTP provider fixture + actual files, including a failed commit after an effect.

The SQLite fault trigger is installed only in this test's temporary database. No production
fault hook or provider is added. --serve-provider exposes just the fixture for manual UI QA.
"""
from private_fixture import protect_empty_fixture
from execution_plan_client import reviewed_run
import argparse, contextlib, hashlib, http.server, json, os, pathlib, socket, sqlite3, subprocess, tempfile, threading, time, urllib.request, urllib.error

CONTENT = "Updated by the explicit file fixture.\nNo actual model was called.\n"

class Provider(http.server.BaseHTTPRequestHandler):
    folder_id = "fixture-root"
    def log_message(self, *_): pass
    def do_GET(self):
        self.send_response(200); self.send_header("Content-Type", "application/json"); self.end_headers()
        self.wfile.write(json.dumps({"data": [{"id": "file-fixture"}]}).encode())
    def do_POST(self):
        data = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        replies = {m.get("tool_call_id"): m for m in data["messages"] if m["role"] == "tool"}
        if "fixture-read" not in replies:
            name, args, call = "read_text_file", {"folder_id":self.folder_id, "path":"note.txt"}, "fixture-read"
        elif "fixture-write" not in replies:
            before = json.loads(replies["fixture-read"]["content"])
            name, args, call = "write_text_file", {"folder_id":self.folder_id, "path":"note.txt", "content":CONTENT, "expected_sha256":before.get("sha256")}, "fixture-write"
        else:
            name = None
        delta = {"content": "Deterministic file fixture completed; no real model was used."}
        if name:
            delta = {"content":"Explicit file fixture action.", "tool_calls":[{"index":0,"id":call,"function":{"name":name,"arguments":json.dumps(args)}}]}
        self.send_response(200); self.send_header("Content-Type", "text/event-stream"); self.end_headers()
        self.wfile.write(("data: "+json.dumps({"choices":[{"delta":delta}]})+"\n\ndata: [DONE]\n\n").encode())

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary",type=pathlib.Path,default=pathlib.Path("target/debug/cowork-daemon.exe" if os.name=="nt" else "target/debug/cowork-daemon"))
    parser.add_argument("--serve-provider",action="store_true")
    parser.add_argument("--port",type=int,default=0)
    args=parser.parse_args()
    provider=http.server.ThreadingHTTPServer(("127.0.0.1",args.port),Provider)
    if args.serve_provider:
        print(f"Disposable file fixture provider: http://127.0.0.1:{provider.server_port}",flush=True)
        provider.serve_forever(); return
    thread=threading.Thread(target=provider.serve_forever,daemon=True);thread.start()
    binary=args.binary.resolve();assert binary.is_file(), "Build the daemon first"
    with socket.socket() as s: s.bind(("127.0.0.1",0));port=s.getsockname()[1]
    with tempfile.TemporaryDirectory(prefix="cowork-v2-files-") as temp:
        root=pathlib.Path(temp);data=root/"private";work=root/"work";data.mkdir();protect_empty_fixture(data);work.mkdir()
        (work/"note.txt").write_text("original fixture",encoding="utf-8")
        env={**os.environ,"COWORK_DATA_DIR":str(data),"COWORK_PORT":str(port),"RUST_LOG":"error"}
        for name in ["COWORK_DATABASE_URL","COWORK_WEB_DIR","COWORK_ENABLE_TEST_PROVIDER"]:env.pop(name,None)
        process=None;token=""
        def call(method,path,body=None):
            req=urllib.request.Request(f"http://127.0.0.1:{port}/api/v2"+path,data=None if body is None else json.dumps(body).encode(),method=method,headers={"Authorization":"Bearer "+token,"Content-Type":"application/json"})
            with urllib.request.urlopen(req,timeout=5) as response:return json.load(response)
        def rejected(method,path,body,status):
            try: call(method,path,body)
            except urllib.error.HTTPError as error: assert error.code==status,(path,error.code,error.read())
            else: raise AssertionError("Expected rejection: "+path)
        def start():
            nonlocal process,token
            flags={"creationflags":subprocess.CREATE_NO_WINDOW} if os.name=="nt" else {}
            process=subprocess.Popen([str(binary)],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE,**flags)
            deadline=time.monotonic()+15
            while time.monotonic()<deadline:
                if process.poll() is not None: raise AssertionError(process.stderr.read().decode())
                try:
                    token=(data/"daemon.token").read_text().strip();call("GET","/health");return
                except (FileNotFoundError,PermissionError,urllib.error.URLError,TimeoutError):time.sleep(.05)
            raise AssertionError("Daemon start timeout")
        def kill():
            nonlocal process
            if process:
                process.kill();process.wait(timeout=5);process.stderr.close();process=None
        def wait(run,state):
            deadline=time.monotonic()+15
            while time.monotonic()<deadline:
                detail=call("GET","/runs/"+run)
                if detail["run"]["state"]==state:return detail
                time.sleep(.04)
            raise AssertionError((state,detail["run"]))
        def approve(run,detail):
            a=next(a for a in detail["approvals"] if a["decision"] is None)
            rejected("POST",f"/runs/{run}/approvals/{a['id']}",{"fingerprint":"different","allow":True},409)
            call("POST",f"/runs/{run}/approvals/{a['id']}",{"fingerprint":a["fingerprint"],"allow":True})
        try:
            start()
            project=call("POST","/projects",{"name":"File system qualification"})
            order=call("POST","/orders",{"project_id":project["id"],"title":"Write with an ambiguous completion"})
            model=call("POST","/providers",{"name":"Local deterministic HTTP fixture","kind":"openai_compatible","endpoint":f"http://127.0.0.1:{provider.server_port}","model":"file-fixture","context_tokens":32768})
            kill()
            (data/"folders.json").write_text(json.dumps([{"id":"fixture-root","project_id":project["id"],"name":"Explicit disposable folder","path":str(work),"writable":True}]),encoding="utf-8")
            start()
            grants=call("GET","/folders?project_id="+project["id"]);assert len(grants)==1 and "path" not in grants[0]
            request={"order_id":order["id"],"base_revision":1,"idempotency_key":"file-once","prompt":"Read and update only the fixture note.","provider_id":model["id"],"provider_revision":1,"access_mode":"confirm_all","folders":grants}
            no_folder=call("POST","/preflight",{**request,"folders":[]});assert len(no_folder["folders"])==1 and no_folder["folders"][0]["order_id"]==order["id"]
            rejected("POST","/preflight",{**request,"folders":[{**grants[0],"revision":"stale"}]},403)
            run=reviewed_run(call, request)["id"]
            pending=wait(run,"waiting_approval");assert pending["approvals"][0]["tool"]["name"]=="read_text_file"
            approve(run,pending)
            pending=wait(run,"waiting_approval")
            # Wait for the second approval rather than the still-observed first response.
            while not any(a["tool"]["name"]=="write_text_file" for a in pending["approvals"]):
                time.sleep(.05);pending=wait(run,"waiting_approval")
            assert (work/"note.txt").read_text()=="original fixture"
            kill();start();assert call("GET","/runs/"+run)["approvals"]==pending["approvals"]
            with contextlib.closing(sqlite3.connect(data/"cowork-v2.sqlite3")) as db, db:
                db.execute("CREATE TRIGGER test_fail_receipt BEFORE INSERT ON file_changes BEGIN SELECT RAISE(ABORT, 'test completion failure'); END")
            approve(run,pending)
            interrupted=wait(run,"interrupted")
            assert (work/"note.txt").read_text()==CONTENT
            assert interrupted["file_changes"]==[] and interrupted["uncertain_file_change"]
            rejected("POST",f"/runs/{run}/resume",{},409);rejected("POST",f"/runs/{run}/cancel",{},409)
            kill()
            with contextlib.closing(sqlite3.connect(data/"cowork-v2.sqlite3")) as db, db:db.execute("DROP TRIGGER test_fail_receipt")
            start()
            restored=call("GET","/runs/"+run);assert restored["uncertain_file_change"]==interrupted["uncertain_file_change"]
            resolution={"fingerprint":restored["uncertain_file_fingerprint"],"decision":"confirm_not_applied"}
            rejected("POST",f"/runs/{run}/files/reconcile",resolution,409)
            resolution["decision"]="accept_completed"
            assert call("POST",f"/runs/{run}/files/reconcile",resolution)["state"]=="interrupted"
            timestamp=(work/"note.txt").stat().st_mtime_ns
            call("POST",f"/runs/{run}/resume",{});done=wait(run,"completed")
            assert len(done["file_changes"])==1 and not done["uncertain_file_change"]
            assert (work/"note.txt").stat().st_mtime_ns==timestamp,"A reconciled write was repeated"
            assert call("GET",f"/runs/{run}/file-changes/fixture-write/previous")["content"]=="original fixture"
            assert call("GET",f"/runs/{run}/file-changes/fixture-write/written")["content"]==CONTENT
            events=call("GET",f"/runs/{run}/events?after=0");assert sum(e["kind"]=="file_write_started" for e in events)==1
            export=call("GET",f"/orders/{order['id']}/export")
            canonical=json.dumps(export["data"],sort_keys=True,separators=(",",":"),ensure_ascii=False).encode()
            assert export["format_version"]==13 and export["sha256"]==hashlib.sha256(canonical).hexdigest()
            kill();start();assert len(call("GET","/runs/"+run)["file_changes"])==1
            assert reviewed_run(call, request)["id"]==run
            print(json.dumps({"result":"passed","provider":"local deterministic HTTP fixture; no real model", "checks":["explicit native grant", "no implicit external grants", "stale grant rejected", "confirm-all read approval", "exact write approval after process restart", "real write followed by failed DB completion", "uncertainty survives restart", "no resume or cancel before reconciliation", "hash-bound reconciliation", "no repeated write", "retained versions", "durable receipt", "independent export hash", "idempotent restart request"]}))
        finally:kill();provider.shutdown();provider.server_close()

if __name__=="__main__":main()
