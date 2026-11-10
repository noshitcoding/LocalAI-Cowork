"""Small real HTTP Alpha.5 demonstration with isolated data and owned processes.

This is deliberately not M0-M10 qualification. It uses an actual supplied daemon
binary and synthetic loopback OpenAI/MCP services, never user credentials.
"""
from __future__ import annotations
import argparse, ctypes, hashlib, http.server, json, os, pathlib, secrets, select
import signal, socket, struct, subprocess, sys, threading, time, urllib.error, urllib.request, uuid

ROOT = pathlib.Path(__file__).resolve().parents[2]

def sha(raw): return hashlib.sha256(raw).hexdigest()
def write(path, value): path.write_text(json.dumps(value, indent=2, ensure_ascii=False)+"\n", encoding="utf-8")
def free_port():
    with socket.socket() as selected:
        selected.bind(("127.0.0.1",0)); return selected.getsockname()[1]
def ordinary(path):
    for ancestor in [path,*path.parents]:
        if ancestor.exists():
            stat=ancestor.lstat()
            if ancestor.is_symlink() or getattr(stat,"st_file_attributes",0)&0x400:
                raise RuntimeError("Demo path contains an alias")

def private_data_directory(path):
    """Create only our new data namespace with the daemon's private OS ACL."""
    if os.name!="nt":path.mkdir(mode=0o700);return
    from ctypes import wintypes as w
    kernel=ctypes.WinDLL("kernel32",use_last_error=True);security=ctypes.WinDLL("advapi32",use_last_error=True)
    def bind(library,name,args,result=w.BOOL):
        function=getattr(library,name);function.argtypes=args;function.restype=result;return function
    bind(kernel,"GetCurrentProcess",[],w.HANDLE);bind(kernel,"CloseHandle",[w.HANDLE]);bind(kernel,"LocalFree",[ctypes.c_void_p],ctypes.c_void_p)
    bind(security,"OpenProcessToken",[w.HANDLE,w.DWORD,ctypes.POINTER(w.HANDLE)])
    bind(security,"GetTokenInformation",[w.HANDLE,ctypes.c_int,ctypes.c_void_p,w.DWORD,ctypes.POINTER(w.DWORD)])
    bind(security,"ConvertSidToStringSidW",[ctypes.c_void_p,ctypes.POINTER(w.LPWSTR)])
    bind(security,"ConvertStringSecurityDescriptorToSecurityDescriptorW",[w.LPCWSTR,w.DWORD,ctypes.POINTER(ctypes.c_void_p),ctypes.c_void_p])
    class Attributes(ctypes.Structure):
        _fields_=[("length",w.DWORD),("descriptor",ctypes.c_void_p),("inherit",w.BOOL)]
    bind(kernel,"CreateDirectoryW",[w.LPCWSTR,ctypes.POINTER(Attributes)])
    token=w.HANDLE();descriptor=ctypes.c_void_p();sid_text=w.LPWSTR()
    try:
        if not security.OpenProcessToken(kernel.GetCurrentProcess(),8,ctypes.byref(token)):raise ctypes.WinError(ctypes.get_last_error())
        size=w.DWORD();security.GetTokenInformation(token,1,None,0,ctypes.byref(size))
        if not 8<=size.value<=65536:raise RuntimeError("Own process SID is unavailable")
        user=ctypes.create_string_buffer(size.value)
        if not security.GetTokenInformation(token,1,user,size,ctypes.byref(size)):raise ctypes.WinError(ctypes.get_last_error())
        sid=ctypes.cast(user,ctypes.POINTER(ctypes.c_void_p))[0]
        if not security.ConvertSidToStringSidW(sid,ctypes.byref(sid_text)):raise ctypes.WinError(ctypes.get_last_error())
        sddl=f"O:{sid_text.value}D:P(A;OICI;FA;;;{sid_text.value})(A;OICI;FA;;;SY)"
        if not security.ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl,1,ctypes.byref(descriptor),None):raise ctypes.WinError(ctypes.get_last_error())
        attributes=Attributes(ctypes.sizeof(Attributes),descriptor,False)
        if not kernel.CreateDirectoryW(str(path),ctypes.byref(attributes)):raise ctypes.WinError(ctypes.get_last_error())
    finally:
        if descriptor:kernel.LocalFree(descriptor)
        if sid_text:kernel.LocalFree(ctypes.cast(sid_text,ctypes.c_void_p))
        if token:kernel.CloseHandle(token)

# A separate hidden console belongs exclusively to this gated controller and
# its daemon descendants. CTRL_C is broadcast only within that private console.
WINDOWS_CONTROLLER = r'''
import ctypes,json,pathlib,subprocess,sys,time
kernel=ctypes.WinDLL("kernel32",use_last_error=True)
handler_type=ctypes.WINFUNCTYPE(ctypes.c_bool,ctypes.c_ulong)
handler=handler_type(lambda _:True)
kernel.SetConsoleCtrlHandler.argtypes=[handler_type,ctypes.c_bool]
kernel.SetConsoleCtrlHandler(handler,True)
assert sys.stdin.readline()=="go\n"
p=subprocess.Popen([sys.argv[1]],stdin=subprocess.DEVNULL,stdout=sys.stdout,stderr=sys.stderr)
receipt=pathlib.Path(sys.argv[2]); receipt.write_text(json.dumps({"daemon_pid":p.pid,"running":True}),encoding="utf-8")
command=sys.stdin.readline()
forced=False
if p.poll() is None:
    kernel.GenerateConsoleCtrlEvent.argtypes=[ctypes.c_ulong,ctypes.c_ulong]
    if not kernel.GenerateConsoleCtrlEvent(0,0):
        forced=True;p.kill()
try:code=p.wait(timeout=30)
except subprocess.TimeoutExpired:forced=True;p.kill();code=p.wait(timeout=10)
native=ctypes.c_ulong();kernel.GetExitCodeProcess.argtypes=[ctypes.c_void_p,ctypes.POINTER(ctypes.c_ulong)]
confirmed=bool(kernel.GetExitCodeProcess(ctypes.c_void_p(int(p._handle)),ctypes.byref(native))) and native.value==(code&0xffffffff)
receipt.write_text(json.dumps({"daemon_pid":p.pid,"running":False,"exit_code":code,"retained_wait":True,"native_exit_confirmed":confirmed,"forced":forced}),encoding="utf-8")
sys.exit(0 if confirmed and not forced and code==0 else 1)
'''

class OwnedDaemon:
    def __init__(self,binary,data,port,evidence):
        self.binary=binary;self.data=data;self.port=port;self.evidence=evidence;self.closed=False
        self.base=f"http://127.0.0.1:{port}";self.job=None;self.pidfd=None
        evidence.mkdir(parents=True,exist_ok=False)
        env={k:v for k,v in os.environ.items() if not k.startswith("COWORK_")}
        env.update(COWORK_DATA_DIR=str(data),COWORK_PORT=str(port),COWORK_BIND_ADDRESS="127.0.0.1")
        self.logs=[(evidence/name).open("wb") for name in ("daemon.stdout.log","daemon.stderr.log")]
        if os.name=="nt":
            from ctypes import wintypes as w
            self.k=ctypes.WinDLL("kernel32",use_last_error=True)
            def bind(name,args,result=w.BOOL):
                fn=getattr(self.k,name);fn.argtypes=args;fn.restype=result;return fn
            bind("CreateJobObjectW",[ctypes.c_void_p,w.LPCWSTR],w.HANDLE)
            bind("SetInformationJobObject",[w.HANDLE,ctypes.c_int,ctypes.c_void_p,w.DWORD])
            bind("AssignProcessToJobObject",[w.HANDLE,w.HANDLE])
            bind("QueryInformationJobObject",[w.HANDLE,ctypes.c_int,ctypes.c_void_p,w.DWORD,ctypes.c_void_p])
            bind("TerminateJobObject",[w.HANDLE,w.UINT]);bind("CloseHandle",[w.HANDLE])
            class Basic(ctypes.Structure):
                _fields_=[("times",ctypes.c_int64*2),("flags",w.DWORD),("min_ws",ctypes.c_size_t),("max_ws",ctypes.c_size_t),("limit",w.DWORD),("affinity",ctypes.c_size_t),("priority",w.DWORD),("scheduling",w.DWORD)]
            class Extended(ctypes.Structure):
                _fields_=[("basic",Basic),("io",ctypes.c_uint64*6),("memory",ctypes.c_size_t*4)]
            self.job=self.k.CreateJobObjectW(None,None);settings=Extended();settings.basic.flags=0x2000
            if not self.job or not self.k.SetInformationJobObject(self.job,9,ctypes.byref(settings),ctypes.sizeof(settings)):
                raise ctypes.WinError(ctypes.get_last_error())
            self.control=evidence/"controller.json"
            start=subprocess.STARTUPINFO();start.dwFlags|=subprocess.STARTF_USESHOWWINDOW;start.wShowWindow=0
            self.process=subprocess.Popen([sys.executable,"-I","-B","-S","-c",WINDOWS_CONTROLLER,str(binary),str(self.control)],cwd=ROOT,env=env,stdin=subprocess.PIPE,stdout=self.logs[0],stderr=self.logs[1],creationflags=subprocess.CREATE_NEW_CONSOLE,startupinfo=start)
            if not self.k.AssignProcessToJobObject(self.job,w.HANDLE(int(self.process._handle))):
                self.process.kill();self.process.wait();raise ctypes.WinError(ctypes.get_last_error())
            self.process.stdin.write(b"go\n");self.process.stdin.flush()
        else:
            self.process=subprocess.Popen([str(binary)],cwd=ROOT,env=env,stdin=subprocess.DEVNULL,stdout=self.logs[0],stderr=self.logs[1],start_new_session=True)
            self.pidfd=os.pidfd_open(self.process.pid)
        deadline=time.monotonic()+40
        while time.monotonic()<deadline:
            if self.process.poll() is not None:raise RuntimeError("Own demo daemon exited during startup")
            if (data/"daemon.token").is_file() and (data/"native.token").is_file():
                self.owner=(data/"daemon.token").read_text().strip();self.native=(data/"native.token").read_text().strip()
                try:self.call("GET","/api/v2/projects");return
                except (OSError,urllib.error.URLError):pass
            time.sleep(.1)
        raise RuntimeError("Own demo daemon did not become ready")
    def call(self,method,path,body=None,token=None,native=False,raw=False):
        headers={"Authorization":"Bearer "+(token or self.owner)}
        if native:headers["x-cowork-native"]=self.native
        data=None if body is None else json.dumps(body).encode()
        if data is not None:headers["Content-Type"]="application/json"
        request=urllib.request.Request(self.base+path,data=data,headers=headers,method=method)
        try:
            with urllib.request.urlopen(request,timeout=15) as response:
                value=response.read(64*1024*1024+1)
                if len(value)>64*1024*1024:raise RuntimeError("Demo API response exceeds bound")
                return value if raw else json.loads(value)
        except urllib.error.HTTPError as error:
            raise RuntimeError(f"Own API {method} {path}: HTTP {error.code}: "+error.read(1024).decode(errors="replace")) from error
    def stop(self):
        if not hasattr(self,"process"):
            if self.job is not None:self.k.CloseHandle(self.job);self.job=None
            for stream in getattr(self,"logs",[]):stream.close()
            self.receipt={"process_not_started":True,"forced":False};self.closed=True
            return self.receipt
        if self.closed:return self.receipt
        forced=False
        if self.process.poll() is None:
            if os.name=="nt":self.process.stdin.write(b"stop\n");self.process.stdin.flush();self.process.stdin.close()
            else:signal.pidfd_send_signal(self.pidfd,signal.SIGINT)
        try:code=self.process.wait(timeout=40)
        except subprocess.TimeoutExpired:
            forced=True
            if os.name=="nt":self.k.TerminateJobObject(self.job,125)
            else:signal.pidfd_send_signal(self.pidfd,signal.SIGKILL)
            code=self.process.wait(timeout=10)
        self.receipt={"retained_wait":True,"exit_code":code,"forced":forced}
        if os.name=="nt":
            evidence=json.loads(self.control.read_text()) if self.control.exists() else {}
            block=ctypes.create_string_buffer(48)
            if not self.k.QueryInformationJobObject(self.job,1,block,len(block),None):raise ctypes.WinError(ctypes.get_last_error())
            active=struct.unpack_from("<I",block.raw,40)[0]
            deadline=time.monotonic()+10
            while active and time.monotonic()<deadline:
                time.sleep(.05)
                if not self.k.QueryInformationJobObject(self.job,1,block,len(block),None):raise ctypes.WinError(ctypes.get_last_error())
                active=struct.unpack_from("<I",block.raw,40)[0]
            if active:
                if not self.k.TerminateJobObject(self.job,125):raise ctypes.WinError(ctypes.get_last_error())
                forced=True;deadline=time.monotonic()+10
                while active and time.monotonic()<deadline:
                    time.sleep(.05)
                    if not self.k.QueryInformationJobObject(self.job,1,block,len(block),None):raise ctypes.WinError(ctypes.get_last_error())
                    active=struct.unpack_from("<I",block.raw,40)[0]
            self.receipt.update(controller=evidence,own_job_empty=active==0,forced=forced or evidence.get("forced",False))
            if active:raise RuntimeError("Own demo Job did not become empty")
            self.k.CloseHandle(self.job);self.job=None
        else:
            poll=select.poll();poll.register(self.pidfd,select.POLLIN);self.receipt["retained_pidfd_completion"]=bool(poll.poll(0));os.close(self.pidfd);self.pidfd=None
            sys.path.insert(0,str(ROOT/"tools/desktop"));from package import ProcessScope
            members=ProcessScope.linux_session_members(self.process.pid)
            if members:ProcessScope.stop_linux_session(self.process.pid);self.receipt["forced"]=True
            self.receipt["own_session_empty"]=not ProcessScope.linux_session_members(self.process.pid)
        for stream in self.logs:stream.close()
        write(self.evidence/"process-receipt.json",self.receipt);self.closed=True
        if self.receipt["forced"] or code!=0:raise RuntimeError("Own demo daemon needed forced or nonzero cleanup")
        return self.receipt

class Fixture(http.server.ThreadingHTTPServer):
    daemon_threads=True
    def __init__(self):
        super().__init__(("127.0.0.1",0),Handler);self.calls=[];self.source_id="";self.team_key=secrets.token_urlsafe(32)
        self.thread=threading.Thread(target=self.serve_forever,name="alpha5-own-http-fixture");self.thread.start()
    @property
    def base(self):return f"http://127.0.0.1:{self.server_port}"
    def close(self):self.shutdown();self.thread.join(timeout=10);self.server_close()

class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version="HTTP/1.1"
    def log_message(self,*_):pass
    def reply(self,value):
        body=json.dumps(value).encode();self.send_response(200);self.send_header("Content-Type","application/json");self.send_header("Content-Length",str(len(body)));self.end_headers();self.wfile.write(body)
    def do_GET(self):
        self.server.calls.append({"path":self.path,"method":"GET","authorization_present":bool(self.headers.get("Authorization"))})
        if self.path!="/v1/models":self.send_error(404);return
        self.reply({"object":"list","data":[{"id":name,"object":"model"} for name in ("alpha5-personal-demo","alpha5-team-demo","alpha5-offline-demo")]})
    def do_POST(self):
        size=int(self.headers.get("Content-Length","0"))
        if size>8*1024*1024:self.send_error(413);return
        raw=self.rfile.read(size);body=json.loads(raw);self.server.calls.append({"path":self.path,"body_sha256":sha(raw),"authorization_present":bool(self.headers.get("Authorization")),"model":body.get("model"),"method":body.get("method")})
        if self.path=="/mcp":
            method=body.get("method");identity=body.get("id")
            if identity is None:self.send_response(202);self.send_header("Content-Length","0");self.end_headers();return
            if method=="initialize":result={"protocolVersion":"2025-11-25","capabilities":{"tools":{}},"serverInfo":{"name":"Alpha5 owned fixture","version":"1"}}
            elif method=="tools/list":result={"tools":[{"name":"echo","description":"Return this own synthetic text","inputSchema":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"],"additionalProperties":False}}]}
            elif method=="tools/call":result={"content":[{"type":"text","text":"Alpha5MCP:"+body["params"]["arguments"]["text"]}],"isError":False}
            else:self.send_error(400);return
            self.reply({"jsonrpc":"2.0","id":identity,"result":result});return
        if self.path!="/v1/chat/completions":self.send_error(404);return
        model=body.get("model");messages=body.get("messages",[])
        if model=="alpha5-team-demo" and self.headers.get("Authorization")!="Bearer "+self.server.team_key:self.send_error(401);return
        content="Alpha5 personal HTTP model demo completed" if body.get("stream") else "Alpha5 team HTTP model demo completed"
        if model=="alpha5-offline-demo":
            content="Alpha5 offline execution completed after teamserver disconnect"
            if not any(message.get("role")=="tool" for message in messages):
                selected={tool["function"]["name"] for tool in body.get("tools",[])}
                proposals=[("list_sources",{}),("read_source",{"id":self.server.source_id}),("execute_program",{"program":"python","arguments":["-c","print('Alpha5 offline terminal completed')"],"timeout_seconds":15}),("office",{"operation":"create_docx","output":"alpha5-offline.docx","text":"Created without Cowork teamserver contact."}),("mcp_call",{"connection_id":"own_fixture","tool_name":"echo","arguments":{"text":"offline confirmed"}}),("create_artifact",{"name":"alpha5-offline.txt","content":"Offline artifact created after teamserver disconnect."})]
                calls=[{"id":"call_"+uuid.uuid4().hex,"type":"function","function":{"name":name,"arguments":json.dumps(arguments)}} for name,arguments in proposals if name in selected]
                self.reply({"choices":[{"message":{"role":"assistant","content":"Please confirm the explicit offline actions.","tool_calls":calls},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":20,"completion_tokens":20,"total_tokens":40}});return
        if body.get("stream"):
            events=[{"choices":[{"index":0,"delta":{"role":"assistant","content":content},"finish_reason":None}]},{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":10,"total_tokens":20}}]
            raw=("".join("data: "+json.dumps(item)+"\n\n" for item in events)+"data: [DONE]\n\n").encode()
            self.send_response(200);self.send_header("Content-Type","text/event-stream");self.send_header("Content-Length",str(len(raw)));self.end_headers();self.wfile.write(raw)
        else:self.reply({"choices":[{"message":{"role":"assistant","content":content},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":10,"total_tokens":20}})

def wait_for(getter,terminal,timeout=40):
    end=time.monotonic()+timeout
    while time.monotonic()<end:
        value=getter()
        if terminal(value):return value
        time.sleep(.15)
    raise RuntimeError("Actual demo workflow did not reach its expected state")

def demonstrate(binary,output):
    ordinary(binary);ordinary(output)
    if not binary.is_file() or output.exists():raise RuntimeError("Supply an actual daemon and a fresh output directory")
    output.mkdir(parents=True);started=time.time();fixture=Fixture();daemons=[]
    report={"format":"localai-cowork-alpha5-isolated-demo/1","result":"running","daemon_sha256":sha(binary.read_bytes()),"harness_sha256":sha(pathlib.Path(__file__).read_bytes()),"formal_qualification":False,"user_credentials_used":False,"steps":[],"limitations":["Synthetic own loopback OpenAI/MCP services; no real LiteLLM provider acceptance","No physical passkey authenticator, Android phone, FCM delivery, GitHub account, container or VM acceptance","API workflows against the actual supplied executable; this does not prove all UI/installer/platform behaviors"]}
    def save():write(output/"receipt.json",report)
    def step(name,evidence):report["steps"].append({"name":name,"evidence":evidence});save()
    def start(label,data,port):
        daemon=OwnedDaemon.__new__(OwnedDaemon);daemons.append(daemon)
        daemon.__init__(binary,data,port,output/label);return daemon
    save()
    try:
        local_data=output/"local-device-data";remote_data=output/"remote-team-data"
        for data in (local_data,remote_data):private_data_directory(data);write(data/"toolbox.json",{"programs":{"python":str(pathlib.Path(sys.executable).resolve())}})
        local_port=free_port();remote_port=free_port()
        while remote_port==local_port:remote_port=free_port()
        local=start("local-owner-process",local_data,local_port);remote=start("remote-team-process",remote_data,remote_port)
        provider=local.call("POST","/api/v2/providers",{"name":"Own loopback OpenAI fixture","kind":"openai_compatible","endpoint":fixture.base+"/v1","model":"alpha5-personal-demo","context_tokens":32768})
        project=local.call("POST","/api/v2/projects",{"name":"Alpha5 personal demo"});order=local.call("POST","/api/v2/orders",{"project_id":project["id"],"title":"Personal actual HTTP run"})
        run=local.call("POST","/api/v2/runs",{"order_id":order["id"],"base_revision":order["revision"],"idempotency_key":str(uuid.uuid4()),"prompt":"Complete this own personal HTTP demo.","provider_id":provider["id"],"provider_revision":provider["revision"],"access_mode":"read_only"})
        detail=wait_for(lambda:local.call("GET",f'/api/v2/runs/{run["id"]}'),lambda d:d["run"]["state"] in ("completed","failed","interrupted","expired"))
        if detail["run"]["state"]!="completed":raise RuntimeError("Personal actual HTTP model run failed")
        write(output/"personal-run.json",detail);step("personal_owner_http_model_run",{"run_id":run["id"],"state":"completed"})
        bootstrap=remote.call("POST","/api/v2/identity/bootstrap",{"bootstrap_secret":(remote_data/"identity-bootstrap"/"master.key").read_text().strip(),"username":"alpha5_demo_owner","display_name":"Isolated demo owner","password":"alpha5-"+secrets.token_hex(24),"organization_name":"Alpha5 isolated team"})
        team=bootstrap["organization"]["id"];session=bootstrap["session"]["token"]
        remote.call("PUT",f"/api/v2/teams/{team}/provider",{"expected_revision":0,"base_url":fixture.base+"/v1","model":"alpha5-team-demo","api_key":fixture.team_key},token=session)
        project=remote.call("POST",f"/api/v2/teams/{team}/projects",{"name":"Isolated shared project","idempotency_key":str(uuid.uuid4())},token=session)
        order=remote.call("POST",f'/api/v2/teams/{team}/projects/{project["id"]}/orders',{"title":"Online plus offline demo","prompt":"Use the explicitly granted local tools after disconnect.","idempotency_key":str(uuid.uuid4())},token=session)
        document=remote.call("POST",f'/api/v2/teams/{team}/projects/{project["id"]}/documents',{"id":None,"expected_revision":0,"kind":"knowledge","name":"Own demo source","content":"Alpha5 frozen source from an isolated team.","archived":False},token=session)
        fixture.source_id="document:"+document["id"]
        run=remote.call("POST",f"/api/v2/teams/{team}/runs",{"order_id":order["id"],"prompt":"Complete this team HTTP demo.","idempotency_key":str(uuid.uuid4()),"plan":{"tools":[],"knowledge":[document["id"]]}},token=session)
        result=wait_for(lambda:remote.call("GET",f'/api/v2/teams/{team}/runs/{run["id"]}',token=session),lambda r:r["status"] in ("completed","failed","unknown","cancelled"))
        if result["status"]!="completed":raise RuntimeError("Team HTTP/LiteLLM-compatible model run failed")
        write(output/"team-run.json",result);step("team_session_encrypted_provider_http_run",{"team_id":team,"run_id":run["id"],"status":"completed"})
        current=local.call("GET","/api/v2/operator/devices")
        configured=local.call("PUT","/api/v2/operator/devices",{"expected_revision":current["revision"],"origin":f"http://localhost:{local_port}","rp_id":"localhost","fcm_account":None,"clear_fcm":False,"fleet_key":None,"clear_fleet":False,"organisation_id":None,"instance_id":None})
        checked=local.call("POST","/api/v2/operator/devices/check",{})
        write(output/"operator-configuration.json",{"configured":configured,"checked":checked});step("operator_passkey_configuration_apply",{"revision":configured["revision"],"origin":configured["origin"],"rp_id":configured["rp_id"]})
        device=local.call("POST","/api/v2/offline/device",{},native=True)
        proof_request={"issuer_origin":remote.base,"team_id":team,"project_id":project["id"],"order_id":order["id"],"idempotency_key":str(uuid.uuid4())}
        proof=local.call("POST","/api/v2/offline/device/proof",proof_request,native=True)
        tools=["list_sources","read_source","create_artifact","execute_program","office","mcp_call","read_workspace_file","write_workspace_file"]
        ticket=remote.call("POST",f'/api/v2/team-offline/{team}/{project["id"]}/grants',{"proof":proof,"prompt":"Perform the own offline demonstration after the teamserver disconnect.","offline_base_url":fixture.base+"/v1","offline_model":"alpha5-offline-demo","allowed_tools":tools,"expires_in_seconds":3600,"maximum_runs":2,"maximum_duration_seconds":300},token=session)
        actual_source=next((s for s in ticket["grant"]["sources"] if s["name"]==document["name"]),None)
        if actual_source is None:raise RuntimeError("Issued grant omitted the frozen own source")
        fixture.source_id=actual_source["id"];grant=ticket["grant"]["id"]
        local.call("POST","/api/v2/offline/grants/install",{"ticket":ticket,"confirmed_issuer_origin":remote.base,"confirmed_issuer_public_key":ticket["grant"]["issuer_public_key"]},native=True)
        local.call("POST","/api/v2/offline/model-key",{"grant_id":grant,"api_key":None,"explicitly_allow_no_key":True},native=True)
        local.call("POST","/api/v2/offline/tool-config",{"grant_id":grant,"allowed_tools":tools,"programs":["python"],"github_token":None,"mcp_connections":[{"id":"own_fixture","endpoint":fixture.base+"/mcp","allowed_tools":["echo"],"bearer":None,"explicitly_allow_no_key":True}],"explicitly_allow_native_owner_rights":True},native=True)
        write(output/"signed-offline-grant.json",ticket)
        remote_receipt=remote.stop()
        with socket.socket() as connection:
            if connection.connect_ex(("127.0.0.1",remote_port))==0:raise RuntimeError("Teamserver is still reachable; cannot claim disconnected offline demonstration")
        step("teamserver_disconnect_before_offline",remote_receipt)
        offline=local.call("POST","/api/v2/offline/runs",{"grant_id":grant,"idempotency_key":str(uuid.uuid4())},native=True)
        offline=wait_for(lambda:local.call("GET",f'/api/v2/offline/runs/{offline["id"]}',native=True),lambda r:r["state"] in ("awaiting_approval","unknown","failed","completed"))
        if offline["state"]!="awaiting_approval" or len(offline["pending_tools"])!=6:raise RuntimeError("Offline model did not propose all expected concrete actions")
        local.call("POST",f'/api/v2/offline/runs/{offline["id"]}/approve',{"expected_revision":offline["revision"],"request_sha256":[t["request_sha256"] for t in offline["pending_tools"]],"approve":True},native=True)
        offline=wait_for(lambda:local.call("GET",f'/api/v2/offline/runs/{offline["id"]}',native=True),lambda r:r["state"] in ("completed","failed","unknown","cancelled"),timeout=60)
        write(output/"offline-run.json",offline)
        if offline["state"]!="completed" or not offline["artifacts"]:raise RuntimeError("Actual disconnected offline tool run failed")
        confirmed=[j for j in offline["journal"] if j["kind"]=="tool_confirmed"]
        if len(confirmed)!=6 or any(isinstance(j["payload"]["result"],dict) and j["payload"]["result"].get("isError",False) for j in confirmed):raise RuntimeError("At least one offline tool did not return confirmed success")
        docx=local.call("GET",f'/api/v2/offline/runs/{offline["id"]}/workspace-file?path=alpha5-offline.docx',native=True,raw=True)
        if not docx.startswith(b"PK"):raise RuntimeError("Actual native Office output is missing")
        (output/"alpha5-offline.docx").write_bytes(docx)
        export=local.call("GET",f'/api/v2/offline/runs/{offline["id"]}/export',native=True);write(output/"signed-offline-run.json",export)
        step("disconnected_offline_sources_terminal_office_mcp_artifact",{"run_id":offline["id"],"state":offline["state"],"confirmed_actions":6,"office_sha256":sha(docx),"teamserver_process_terminal":True,"signed_export":bool(export["device_signature"])})
        local.stop();local=start("local-owner-restart",local_data,local_port)
        persisted=local.call("GET","/api/v2/operator/devices")
        if persisted["revision"]!=configured["revision"] or persisted["origin"]!=configured["origin"] or persisted["rp_id"]!=configured["rp_id"]:raise RuntimeError("Live operator configuration did not survive daemon restart")
        step("operator_configuration_survives_restart",{"revision":persisted["revision"],"origin":persisted["origin"],"rp_id":persisted["rp_id"]})
        local.stop();report["result"]="passed"
    except BaseException as error:
        report["result"]="failed";report["error_type"]=type(error).__name__;report["error"]=str(error)[:1000];raise
    finally:
        errors=[]
        for daemon in reversed(daemons):
            try:daemon.stop()
            except BaseException as error:errors.append(type(error).__name__+":"+str(error))
        fixture.close();report["fixture_thread_joined"]=not fixture.thread.is_alive();report["fixture_calls"]=fixture.calls;report["processes"]=[{"label":d.evidence.name,"receipt":getattr(d,"receipt",None)} for d in daemons];report["seconds"]=round(time.time()-started,2)
        if errors:report["result"]="failed";report["cleanup_errors"]=errors
        save()
    return report

if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument("--daemon",type=pathlib.Path,required=True);parser.add_argument("--output",type=pathlib.Path,required=True);args=parser.parse_args()
    try:
        result=demonstrate(args.daemon.absolute(),args.output.absolute());print(json.dumps({"result":result["result"],"receipt":str(args.output.absolute()/"receipt.json")}),flush=True)
    except Exception as error:
        print(json.dumps({"result":"failed","error_type":type(error).__name__,"message":str(error)[:1000],"receipt":str(args.output.absolute()/"receipt.json")}),flush=True);sys.exit(1)
    sys.exit(0 if result["result"]=="passed" else 1)
