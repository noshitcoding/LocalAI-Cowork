"""Native Linux WebView boot + independent daemon lifetime, under a supplied display/Xvfb.

No browser or desktop UI is driven by this process test. The real frontend must invoke
connection_info to start the daemon; HTTP then observes durable work after desktop exit.
"""
from execution_plan_client import reviewed_run
import argparse, json, os, pathlib, signal, socket, subprocess, tempfile, time, urllib.request, urllib.error

parser=argparse.ArgumentParser()
parser.add_argument("--desktop",type=pathlib.Path,required=True)
parser.add_argument("--daemon",type=pathlib.Path,required=True)
args=parser.parse_args()
assert os.name=="posix" and pathlib.Path("/proc").is_dir(),"Linux /proc is required"
desktop=args.desktop.resolve();daemon=args.daemon.resolve()
assert desktop.is_file() and daemon.is_file(),"Prepare the sidecar and build the native desktop first"
with socket.socket() as check:
    assert check.connect_ex(("127.0.0.1",43821))!=0,"Port 43821 is already owned; no existing daemon is stopped"

with tempfile.TemporaryDirectory(prefix="cowork-v2-desktop-") as temp:
    data=pathlib.Path(temp)
    env={**os.environ,"COWORK_DATA_DIR":str(data),"COWORK_ENABLE_TEST_PROVIDER":"1","GDK_BACKEND":"x11"}
    for name in ["COWORK_DATABASE_URL","COWORK_WEB_DIR","COWORK_PORT"]:env.pop(name,None)
    process=None; daemon_pid=None;token=""
    def api(method,path,body=None):
        req=urllib.request.Request("http://127.0.0.1:43821/api/v2"+path,method=method,data=None if body is None else json.dumps(body).encode(),headers={"Authorization":"Bearer "+token,"Content-Type":"application/json"})
        with urllib.request.urlopen(req,timeout=5) as response:return json.load(response)
    def own_daemon():
        # Never stop a process based only on a port or executable name.
        for entry in pathlib.Path("/proc").iterdir():
            if not entry.name.isdigit():continue
            try:
                if (entry/"exe").resolve()!=daemon:continue
                environment=(entry/"environ").read_bytes().split(b"\0")
                if ("COWORK_DATA_DIR="+str(data)).encode() in environment:return int(entry.name)
            except (FileNotFoundError,PermissionError,ProcessLookupError):pass
        return None
    def stop_own_daemon():
        owned=own_daemon()
        if owned is not None:
            os.kill(owned,signal.SIGTERM)
            for _ in range(50):
                if own_daemon() is None:return
                time.sleep(.05)
            if own_daemon()==owned:os.kill(owned,signal.SIGKILL)
    with (data/"desktop.log").open("wb") as log:
        try:
            process=subprocess.Popen([str(desktop)],env=env,stdout=log,stderr=log)
            deadline=time.monotonic()+30
            while time.monotonic()<deadline:
                if process.poll() is not None:raise AssertionError("Desktop exited: "+(data/"desktop.log").read_text(errors="replace")[-4000:])
                try:
                    token=(data/"daemon.token").read_text().strip()
                    api("GET","/health");break
                except (FileNotFoundError,urllib.error.URLError,TimeoutError):time.sleep(.1)
            else:raise AssertionError("Desktop did not start its daemon: "+(data/"desktop.log").read_text(errors="replace")[-4000:])
            daemon_pid=own_daemon();assert daemon_pid,"Expected the exact staged daemon with this test's data directory"
            project=api("POST","/projects",{"name":"Linux desktop lifetime fixture"})
            order=api("POST","/orders",{"project_id":project["id"],"title":"Independent daemon"})
            provider=api("POST","/providers",{"name":"Slow deterministic test","kind":"test","endpoint":"","model":"slow-test","context_tokens":32768})
            run=reviewed_run(api, {"order_id":order["id"],"base_revision":1,"idempotency_key":"desktop-lifetime","prompt":"Create one deterministic artifact.","provider_id":provider["id"],"provider_revision":1,"access_mode":"autonomous"})
            process.terminate();process.wait(timeout=5)
            deadline=time.monotonic()+25
            while time.monotonic()<deadline:
                detail=api("GET","/runs/"+run["id"])
                assert detail["run"]["state"] not in ["failed","canceled","expired","interrupted"],detail["run"]["state"]
                if detail["run"]["state"]=="completed":break
                time.sleep(.1)
            else:raise AssertionError("Run did not complete after desktop exit")
            assert len(detail["artifacts"])==1
            assert own_daemon()==daemon_pid
            print(json.dumps({"result":"passed","platform":"native Linux WebKit desktop under supplied display","checks":["frontend boot invokes native daemon connection","exact staged sidecar","run survives desktop process exit","exactly one persisted artifact"],"provider":"deterministic; no actual model"}))
        finally:
            if process is not None and process.poll() is None:process.terminate();process.wait(timeout=5)
            stop_own_daemon()
