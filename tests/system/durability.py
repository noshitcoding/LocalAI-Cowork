"""Real-process qualification using disposable data and a deterministic provider, never V1 data."""
from execution_plan_client import reviewed_run
import argparse, hashlib, json, os, pathlib, socket, subprocess, tempfile, time, urllib.request, urllib.error, urllib.parse
parser=argparse.ArgumentParser()
parser.add_argument("--binary",type=pathlib.Path,default=pathlib.Path("target/debug/cowork-daemon.exe" if os.name=="nt" else "target/debug/cowork-daemon"))
args=parser.parse_args()
binary=args.binary.resolve()
assert binary.is_file(), "Build cowork-daemon first"
with socket.socket() as s:
    s.bind(("127.0.0.1",0)); port=s.getsockname()[1]
with tempfile.TemporaryDirectory(prefix="cowork-v2-system-") as temp:
    data=pathlib.Path(temp)
    env={**os.environ,"COWORK_DATA_DIR":str(data),"COWORK_ENABLE_TEST_PROVIDER":"1","COWORK_PORT":str(port),"RUST_LOG":"error"}
    env.pop("COWORK_DATABASE_URL",None)
    env.pop("COWORK_WEB_DIR",None)
    process=None
    token=""
    base=f"http://127.0.0.1:{port}/api/v2"
    def call(method,path,body=None):
        payload=None if body is None else json.dumps(body).encode()
        req=urllib.request.Request(base+path,data=payload,method=method,headers={"Authorization":"Bearer "+token,"Content-Type":"application/json"})
        with urllib.request.urlopen(req,timeout=5) as response:
            return json.load(response)
    def start():
        global process,token
        options={"creationflags":subprocess.CREATE_NO_WINDOW} if os.name=="nt" else {}
        process=subprocess.Popen([str(binary)],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE,**options)
        deadline=time.monotonic()+10
        while time.monotonic()<deadline:
            if process.poll() is not None: raise AssertionError(process.stderr.read().decode())
            try:
                token=(data/"daemon.token").read_text().strip()
                call("GET","/health");return
            except (FileNotFoundError,urllib.error.URLError,TimeoutError):time.sleep(.05)
        raise AssertionError("Daemon did not start")
    def kill():
        global process
        process.kill();process.wait(timeout=5);process.stderr.close();process=None
    def wait(run,wanted):
        deadline=time.monotonic()+25
        while time.monotonic()<deadline:
            detail=call("GET","/runs/"+run)
            if detail["run"]["state"]==wanted:return detail
            time.sleep(.1)
        raise AssertionError(f"Expected {wanted}, got {detail['run']['state']}")
    def new_run(order,profile="deterministic-test",mode="confirm_mutations"):
        current=call("GET","/orders/"+order)
        return {"order_id":order,"base_revision":current["revision"],"idempotency_key":str(time.monotonic_ns()),
          "prompt":"Generate a deterministic artifact for durability verification.","provider_id":profile,"provider_revision":1,"access_mode":mode}
    try:
        start()
        project=call("POST","/projects",{"name":"Disposable system qualification"})
        order=call("POST","/orders",{"project_id":project["id"],"title":"Crash and approval recovery"})
        draft=call("PUT",f"/orders/{order['id']}/draft",{"base_revision":1,"draft":"Confirmed draft"})
        kill();start()
        assert call("GET","/orders/"+order["id"])["draft"]=="Confirmed draft"
        request=new_run(order["id"])
        run=reviewed_run(call, request)
        approval=wait(run["id"],"waiting_approval")["approvals"][0]
        assert call("GET",f"/orders/{order['id']}/artifacts")==[]
        kill();start()
        pending=call("GET","/runs/"+run["id"])
        assert pending["run"]["state"]=="waiting_approval"
        assert pending["approvals"][0]["fingerprint"]==approval["fingerprint"]
        resolution={"fingerprint":approval["fingerprint"],"allow":True}
        call("POST",f"/runs/{run['id']}/approvals/{approval['id']}",resolution)
        wait(run["id"],"completed")
        assert reviewed_run(call, request)["id"]==run["id"]
        artifacts=call("GET",f"/orders/{order['id']}/artifacts")
        assert len(artifacts)==1
        transcript=call("GET",f"/orders/{order['id']}/messages")
        branch_request={"title":"Durable separate branch","through_message_id":transcript[-1]["id"],"idempotency_key":"system-branch-once"}
        branch=call("POST",f"/orders/{order['id']}/branches",branch_request)
        assert call("POST",f"/orders/{order['id']}/branches",branch_request)["id"]==branch["id"]
        snapshot=call("GET",f"/orders/{branch['id']}/history")
        assert snapshot["inherited"]["messages"]==transcript
        assert call("GET",f"/orders/{branch['id']}/runs")==[]
        exported=call("GET",f"/orders/{branch['id']}/export")
        canonical=json.dumps(exported["data"],sort_keys=True,separators=(",",":"),ensure_ascii=False).encode()
        assert exported["sha256"]==hashlib.sha256(canonical).hexdigest()
        kill();start()
        assert call("GET",f"/orders/{order['id']}/artifacts")==artifacts
        assert call("GET",f"/orders/{branch['id']}/history")==snapshot
        assert call("GET",f"/orders/{branch['id']}/export")["sha256"]==exported["sha256"]
        search=call("GET","/search?"+urllib.parse.urlencode({"q":"Durable separate branch"}))
        assert any(hit["order_id"]==branch["id"] for hit in search["items"])
        slow=call("POST","/providers",{"name":"Slow deterministic test","kind":"test","model":"slow-test","endpoint":"","context_tokens":32768})
        cancel=reviewed_run(call, new_run(order["id"],slow["id"]))
        wait(cancel["id"],"running")
        started=time.monotonic()
        call("POST",f"/runs/{cancel['id']}/cancel",{})
        wait(cancel["id"],"canceled")
        assert time.monotonic()-started<2,"Cancellation acknowledgement exceeded 2s test ceiling"
        interrupted=reviewed_run(call, new_run(order["id"],slow["id"]))
        wait(interrupted["id"],"running")
        kill();start()
        wait(interrupted["id"],"interrupted")
        inbox=call("GET","/activity?attention_only=true")
        assert any(item["run"]["id"]==interrupted["id"] for item in inbox["items"])
        assert len(call("GET",f"/orders/{order['id']}/artifacts"))==1
        events=call("GET",f"/runs/{run['id']}/events?after=0")
        assert [e["sequence"] for e in events]==list(range(1,len(events)+1))
        print(json.dumps({"result":"passed","provider":"deterministic (no real model)",
          "checks":["confirmed draft after process kill","pending exact approval after restart","artifact after restart",
          "idempotent start after completion","cancel during model request","expired lease interrupts without replay","monotonic persisted events",
          "idempotent branch","history snapshot after restart","independently verified export hash","search after restart","interrupted run in inbox"]}))
    finally:
        if process is not None:kill()
