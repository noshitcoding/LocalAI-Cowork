"""Native-authority HTTP and real runtime revocation, using only disposable filesystem data.

This intentionally calls the privileged API as a native-client fixture; it is not an
interactive OS picker qualification. No native credential is returned or logged.
"""
from execution_plan_client import reviewed_run
import argparse,contextlib,hashlib,http.server,json,os,pathlib,socket,sqlite3,subprocess,tempfile,threading,time,urllib.request,urllib.error,urllib.parse
from files import Provider,CONTENT

def resource_id(path):
    if os.name!="nt":
        stat=path.stat();identity=f"{stat.st_dev}-{stat.st_ino}"
    else:
        import ctypes
        from ctypes import wintypes
        class FileInfo(ctypes.Structure):
            _fields_=[("attributes",wintypes.DWORD),("creation",wintypes.FILETIME),("access",wintypes.FILETIME),("write",wintypes.FILETIME),("volume",wintypes.DWORD),("size_high",wintypes.DWORD),("size_low",wintypes.DWORD),("links",wintypes.DWORD),("index_high",wintypes.DWORD),("index_low",wintypes.DWORD)]
        kernel=ctypes.WinDLL("kernel32",use_last_error=True)
        kernel.CreateFileW.argtypes=[wintypes.LPCWSTR,wintypes.DWORD,wintypes.DWORD,ctypes.c_void_p,wintypes.DWORD,wintypes.DWORD,wintypes.HANDLE];kernel.CreateFileW.restype=wintypes.HANDLE
        kernel.GetFileInformationByHandle.argtypes=[wintypes.HANDLE,ctypes.POINTER(FileInfo)];kernel.CloseHandle.argtypes=[wintypes.HANDLE]
        handle=kernel.CreateFileW(str(path),0x80,7,None,3,0x02000000,None)
        if handle==ctypes.c_void_p(-1).value:raise ctypes.WinError(ctypes.get_last_error())
        try:
            info=FileInfo()
            if not kernel.GetFileInformationByHandle(handle,ctypes.byref(info)):raise ctypes.WinError(ctypes.get_last_error())
            identity=f"{info.volume}-{(info.index_high<<32)|info.index_low}"
        finally:kernel.CloseHandle(handle)
    return hashlib.sha256(identity.encode()).hexdigest()

def main():
    parser=argparse.ArgumentParser();parser.add_argument("--binary",type=pathlib.Path,default=pathlib.Path("target/debug/cowork-daemon.exe" if os.name=="nt" else "target/debug/cowork-daemon"));args=parser.parse_args();binary=args.binary.resolve();assert binary.is_file()
    provider=http.server.ThreadingHTTPServer(("127.0.0.1",0),Provider);thread=threading.Thread(target=provider.serve_forever,daemon=True);thread.start()
    with socket.socket() as sock:sock.bind(("127.0.0.1",0));port=sock.getsockname()[1]
    with tempfile.TemporaryDirectory(prefix="cowork-v2-native-folders-") as temp:
        root=pathlib.Path(temp);data=root/"private";work=root/"chosen";data.mkdir();work.mkdir();(work/"note.txt").write_text("original fixture",encoding="utf-8")
        process=None;token="";native="";env={**os.environ,"COWORK_DATA_DIR":str(data),"COWORK_PORT":str(port),"RUST_LOG":"error"}
        for key in ["COWORK_DATABASE_URL","COWORK_WEB_DIR","COWORK_ENABLE_TEST_PROVIDER"]:env.pop(key,None)
        def call(method,path,body=None,privileged=False,origin=None):
            headers={"Authorization":"Bearer "+token,"Content-Type":"application/json"}
            if privileged:headers["x-cowork-native"]=native
            if origin:headers["Origin"]=origin
            request=urllib.request.Request(f"http://127.0.0.1:{port}/api/v2"+path,data=None if body is None else json.dumps(body).encode(),method=method,headers=headers)
            with urllib.request.urlopen(request,timeout=10) as response:return json.load(response)
        def rejected(method,path,body,status,privileged=False,origin=None):
            try:call(method,path,body,privileged,origin)
            except urllib.error.HTTPError as error:assert error.code==status,(error.code,error.read())
            else:raise AssertionError("Expected request rejection")
        def start():
            nonlocal process,token,native
            flags={"creationflags":subprocess.CREATE_NO_WINDOW} if os.name=="nt" else {}
            process=subprocess.Popen([str(binary)],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE,**flags)
            deadline=time.monotonic()+15
            while time.monotonic()<deadline:
                if process.poll() is not None:raise AssertionError(process.stderr.read().decode())
                try:
                    token=(data/"daemon.token").read_text().strip();native=(data/"native.token").read_text().strip();call("GET","/health");assert token!=native;return
                except (FileNotFoundError,urllib.error.URLError,TimeoutError):time.sleep(.05)
            raise AssertionError("Daemon start timeout")
        def kill():
            nonlocal process
            if process:process.kill();process.wait(timeout=5);process.stderr.close();process=None
        def wait(run,wanted):
            deadline=time.monotonic()+20
            while time.monotonic()<deadline:
                detail=call("GET","/runs/"+run)
                if detail["run"]["state"]==wanted:return detail
                time.sleep(.05)
            raise AssertionError(detail["run"])
        def approve(run):
            detail=wait(run,"waiting_approval");a=next(a for a in detail["approvals"] if a["decision"] is None)
            assert a["tool"]["name"]=="write_text_file"
            call("POST",f"/runs/{run}/approvals/{a['id']}",{"fingerprint":a["fingerprint"],"allow":True})
        def revoke(grant):
            return call("POST","/native/folder-revocations",{"project_id":grant["project_id"],"folder_id":grant["id"],"expected_revision":grant["revision"]},True)
        def sql(statement):
            with contextlib.closing(sqlite3.connect(data/"cowork-v2.sqlite3")) as db,db:db.execute(statement)
        try:
            start();project=call("POST","/projects",{"name":"Native-folder qualification"});order=call("POST","/orders",{"project_id":project["id"],"title":"Revocation qualification"})
            model=call("POST","/providers",{"name":"Synthetic native-folder provider","kind":"openai_compatible","endpoint":f"http://127.0.0.1:{provider.server_port}","model":"file-fixture","context_tokens":32768})
            selection={"idempotency_key":"pick-once","project_id":project["id"],"name":"Chosen fixture","path":str(work),"expected_resource_id":resource_id(work),"writable":True}
            rejected("POST","/native/folders",selection,403)
            rejected("POST","/native/folders",selection,403,True,f"http://127.0.0.1:{port}")
            record=call("POST","/native/folders",selection,True);grant=record["grant"];Provider.folder_id=grant["id"]
            assert call("POST","/native/folders",selection,True)["grant"]==grant
            kill();start();assert call("GET","/folders?project_id="+project["id"])==[grant]
            assert not (data/"folders.json").exists() and (work/"note.txt").read_text()=="original fixture"
            request={"order_id":order["id"],"base_revision":1,"idempotency_key":"before-revoke","prompt":"Read and change the fixture note.","provider_id":model["id"],"provider_revision":1,"access_mode":"confirm_mutations","folders":[grant]}
            run=reviewed_run(call, request)["id"];wait(run,"waiting_approval");revoke(grant);approve(run);done=wait(run,"completed")
            assert done["file_changes"]==[] and done["workspace_snapshot"] and (work/"note.txt").read_text()=="original fixture"
            assert any(e["kind"]=="tool_completed" and e["payload"]["result"].get("error")=="forbidden" for e in call("GET",f"/runs/{run}/events?after=0"))
            assert call("POST","/native/folders",selection,True)["revoked"]
            selection["idempotency_key"]="new-human-choice";record=call("POST","/native/folders",selection,True);grant=record["grant"];Provider.folder_id=grant["id"]
            revision=call("GET",f"/orders/{order['id']}")["revision"];request.update(base_revision=revision,idempotency_key="uncertain-before-revoke",folders=[grant])
            run=reviewed_run(call, request)["id"];wait(run,"waiting_approval")
            sql("CREATE TRIGGER fail_native_folder_receipt BEFORE INSERT ON file_changes BEGIN SELECT RAISE(ABORT,'fixture receipt failure'); END")
            approve(run);pending=wait(run,"interrupted");assert (work/"note.txt").read_text()==CONTENT
            revoke(grant);kill();sql("DROP TRIGGER fail_native_folder_receipt");start();assert call("GET","/folders?project_id="+project["id"])==[]
            pending=call("GET",f"/runs/{run}");decision={"fingerprint":pending["uncertain_file_fingerprint"],"decision":"confirm_not_applied"};rejected("POST",f"/runs/{run}/files/reconcile",decision,409)
            decision["decision"]="accept_completed";assert call("POST",f"/runs/{run}/files/reconcile",decision)["state"]=="interrupted"
            before=(work/"note.txt").stat().st_mtime_ns;call("POST",f"/runs/{run}/resume",{});done=wait(run,"completed");assert len(done["file_changes"])==1 and (work/"note.txt").stat().st_mtime_ns==before
            kill();start();assert call("GET","/folders?project_id="+project["id"])==[];assert call("POST","/native/folders",selection,True)["revoked"]
            assert reviewed_run(call, request)["id"]==run
            print(json.dumps({"result":"passed","real_model":False,"interactive_picker":False,"checks":["native-only authority and Origin rejection","durable registration without config/restart","idempotent replay","revoke during approval denies effect","own workspace snapshot completes","revocation survives restart","old request does not revive grant","new explicit selection gets new grant","receipt failure then revocation","exact human reconciliation after revocation","separate resume without replay"]}))
        finally:kill();provider.shutdown();provider.server_close();thread.join(timeout=5)

if __name__=="__main__":main()
