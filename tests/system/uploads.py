"""Binary uploads and confirmed copy imports across HTTP, faults and restarts.

Uses only disposable data and no provider. Fault triggers are installed in the fixture database.
"""
import argparse, contextlib, hashlib, http.client, json, os, pathlib, socket, sqlite3, subprocess, tempfile, time, urllib.request, urllib.error, urllib.parse
from file_action_client import prepare_file_action, recheck_file_action, commit_file_action

def main():
    parser=argparse.ArgumentParser();parser.add_argument("--binary",type=pathlib.Path,default=pathlib.Path("target/debug/cowork-daemon.exe" if os.name=="nt" else "target/debug/cowork-daemon"));args=parser.parse_args()
    binary=args.binary.resolve();assert binary.is_file()
    with socket.socket() as sock:sock.bind(("127.0.0.1",0));port=sock.getsockname()[1]
    with tempfile.TemporaryDirectory(prefix="cowork-v2-uploads-") as temp:
        data=pathlib.Path(temp);process=None;token=""
        env={**os.environ,"COWORK_DATA_DIR":str(data),"COWORK_PORT":str(port),"RUST_LOG":"error"}
        for key in ["COWORK_DATABASE_URL","COWORK_WEB_DIR","COWORK_ENABLE_TEST_PROVIDER"]:env.pop(key,None)
        def call(method,path,body=None,binary_body=False):
            content=body if binary_body else None if body is None else json.dumps(body).encode()
            request=urllib.request.Request(f"http://127.0.0.1:{port}/api/v2"+path,data=content,method=method,headers={"Authorization":"Bearer "+token,"Content-Type":"application/octet-stream" if binary_body else "application/json"})
            with urllib.request.urlopen(request,timeout=15) as response:return json.load(response)
        def rejected(method,path,body,status,binary_body=False):
            try:call(method,path,body,binary_body)
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
        def attention(run):
            deadline=time.monotonic()+20
            while time.monotonic()<deadline:
                detail=call("GET","/runs/"+run)
                if detail["run"]["state"] in ["interrupted","completed","failed","expired"] or (detail["run"]["state"]=="waiting_approval" and any(a["decision"] is None for a in detail["approvals"])):return detail
                time.sleep(.05)
            raise AssertionError(detail["run"])
        def upload(order,key,content):
            metadata={"file_name":"Prüfung 📄.bin","size":len(content),"sha256":hashlib.sha256(content).hexdigest()}
            record=call("POST",f"/orders/{order}/uploads",{"idempotency_key":key,"file":metadata})
            return call("PUT",f"/orders/{order}/uploads/{record['id']}/content",content,True)
        def request_import(order,folder,source,key,path,expected=None,mode="confirm_mutations"):
            revision=call("GET",f"/orders/{order}")["revision"]
            return {"order_id":order,"base_revision":revision,"idempotency_key":key,"prompt":"Import synthetic file","access_mode":mode,"import":{"folder_id":folder,"path":path,"upload_id":source["id"],"file":source["file"],"expected_sha256":expected}}
        def prepare_plan(kind,request):
            order=request["order_id"];before=call("GET",f"/orders/{order}/runs")
            prepared=prepare_file_action(call,kind,request)
            assert call("GET",f"/orders/{order}/runs")==before,"Preparing a plan must not start a run"
            return prepared
        def copy_file(run,prepared,path):
            binding=run["spec"]["input_workspace"];assert binding and binding["input"]["snapshot_id"]==prepared.plan["input"]["id"]
            alias=prepared.plan["input"]["sources"][0]["alias"]
            return data/"file-broker"/"run-workspaces"/binding["id"]/"files"/alias/path
        def exported(order,upload):
            archive=call("GET",f"/orders/{order}/export")
            assert archive["format_version"]==13
            assert archive["sha256"]==hashlib.sha256(json.dumps(archive["data"],sort_keys=True,separators=(",",":"),ensure_ascii=False).encode()).hexdigest()
            retained=next(item for item in archive["data"]["uploads"] if item["id"]==upload["id"])
            assert retained["file"]==upload["file"] and retained["state"]=="ready"
            return archive
        def finish_faulted_import(run):
            detail=attention(run)
            if detail["run"]["state"]=="waiting_approval":
                approval=next(a for a in detail["approvals"] if a["decision"] is None)
                call("POST",f"/runs/{run}/approvals/{approval['id']}",{"fingerprint":approval["fingerprint"],"allow":True});detail=attention(run)
            assert detail["run"]["state"]=="interrupted",detail["run"]
            operation=detail["uncertain_file_operation"];assert operation["kind"]=="import"
            rejected("POST",f"/runs/{run}/resume",{},409)
            kill();sql("DROP TRIGGER fail_import_receipt");start()
            again=call("GET",f"/runs/{run}");assert again["uncertain_file_operation"]==operation
            decision={"fingerprint":again["uncertain_file_fingerprint"],"decision":"confirm_not_applied"};rejected("POST",f"/runs/{run}/files/reconcile",decision,409)
            decision["decision"]="accept_completed";assert call("POST",f"/runs/{run}/files/reconcile",decision)["state"]=="interrupted"
            call("POST",f"/runs/{run}/resume",{});final=attention(run);assert final["run"]["state"]=="completed" and len(final["file_operations"])==1
            assert not any(e["kind"].startswith("model_") for e in call("GET",f"/runs/{run}/events?after=0"))
            return final
        try:
            start();assert call("GET","/providers")==[]
            project=call("POST","/projects",{"name":"Disposable upload test"})
            order=call("POST","/orders",{"project_id":project["id"],"title":"Binary import"})["id"]
            draft=call("PUT",f"/orders/{order}/draft",{"base_revision":1,"draft":"Preserve this independent draft"})
            content=bytes(range(256))*8192;metadata={"file_name":"Prüfung 📄.bin","size":len(content),"sha256":hashlib.sha256(content).hexdigest()}
            reservation={"idempotency_key":"upload-once","file":metadata};source=call("POST",f"/orders/{order}/uploads",reservation)
            assert call("POST",f"/orders/{order}/uploads",reservation)["id"]==source["id"]
            kill();start();assert call("GET",f"/orders/{order}/uploads")["items"][0]["state"]=="reserved"
            path=f"/orders/{order}/uploads/{source['id']}/content"
            connection=http.client.HTTPConnection("127.0.0.1",port,timeout=5);connection.putrequest("PUT","/api/v2"+path)
            for key,value in {"Authorization":"Bearer "+token,"Content-Type":"application/octet-stream","Content-Length":str(len(content))}.items():connection.putheader(key,value)
            connection.endheaders();connection.send(content[:1024]);connection.close()
            kill();start();assert call("GET",f"/orders/{order}/uploads")["items"][0]["state"]=="reserved"
            sql("CREATE TRIGGER fail_upload_confirmation BEFORE UPDATE ON file_uploads BEGIN SELECT RAISE(ABORT,'fixture upload confirmation failure'); END")
            rejected("PUT",path,content,503,True)
            retained=data/"file-broker"/"versions"/metadata["sha256"];assert retained.read_bytes()==content
            kill();sql("DROP TRIGGER fail_upload_confirmation");start()
            assert call("POST",f"/orders/{order}/uploads",reservation)["state"]=="reserved"
            source=call("PUT",path,content,True);assert source["state"]=="ready" and source["revision"]==2
            assert call("GET",f"/orders/{order}/workspace/files?path=.")["entries"]==[]
            folder=call("GET",f"/orders/{order}/workspace")["folder"]["id"]
            request=request_import(order,folder,source,"import-once","result.bin")
            rejected("POST","/file-imports",request,409)
            prepared=prepare_plan("import",request)
            assert prepared.plan["input"]["manifest"]["files"]==[]
            confirmation=f"/orders/{order}/execution-plans/v2/{prepared.plan['id']}/commit"
            rejected("POST",confirmation,{"sha256":"0"*64},409)
            first_run=commit_file_action(call,prepared);run=first_run["id"]
            assert attention(run)["run"]["state"]=="waiting_approval"
            kill();start();assert attention(run)["run"]["state"]=="waiting_approval"
            sql("CREATE TRIGGER fail_import_receipt BEFORE INSERT ON file_operations BEGIN SELECT RAISE(ABORT,'fixture import receipt failure'); END")
            first=finish_faulted_import(run);assert commit_file_action(call,recheck_file_action(call,prepared))["id"]==run
            target=copy_file(first_run,prepared,"result.bin");assert target.read_bytes()==content
            assert call("GET",f"/orders/{order}/workspace/files?path=.")["entries"]==[]
            assert call("GET",f"/orders/{order}")["draft"]==draft["draft"]
            # Explicit branching adopts a result as a new input; another import
            # still receives its own independent copy rather than changing it.
            answer=next(message for message in call("GET",f"/orders/{order}/messages") if message["run_id"]==run and message["role"]=="assistant")
            branch=call("POST",f"/orders/{order}/branches",{"title":"Replace an explicitly adopted result","through_message_id":answer["id"],"idempotency_key":"branch-imported"})["id"]
            relative=prepared.plan["input"]["sources"][0]["alias"]+"/result.bin"
            branch_folder=call("GET",f"/orders/{branch}/workspace")["folder"]["id"]
            branch_target=data/"file-broker"/"workspaces"/branch/"files"/relative
            assert branch_target.read_bytes()==content
            branch_draft=call("PUT",f"/orders/{branch}/draft",{"base_revision":call("GET",f"/orders/{branch}")["revision"],"draft":"Preserve branched replacement draft"})
            replacement=b"replacement from an explicit upload\n";second_source=upload(branch,"replace-source",replacement)
            stale=request_import(branch,branch_folder,second_source,"stale",relative,hashlib.sha256(b"wrong base").hexdigest(),"autonomous")
            before=call("GET",f"/orders/{branch}/runs")
            try:prepare_file_action(call,"import",stale)
            except urllib.error.HTTPError as error:assert error.code==409,(error.code,error.read())
            else:raise AssertionError("Expected stale copy target rejection before admission")
            assert call("GET",f"/orders/{branch}/runs")==before and branch_target.read_bytes()==content and target.read_bytes()==content
            request2=request_import(branch,branch_folder,second_source,"replace-once",relative,metadata["sha256"])
            prepared2=prepare_plan("import",request2)
            second_run=commit_file_action(call,prepared2);run2=second_run["id"];assert attention(run2)["run"]["state"]=="waiting_approval"
            sql("CREATE TRIGGER fail_import_receipt BEFORE INSERT ON file_operations BEGIN SELECT RAISE(ABORT,'fixture replacement receipt failure'); END")
            second=finish_faulted_import(run2);replacement_target=copy_file(second_run,prepared2,relative)
            assert replacement_target.read_bytes()==replacement and second["file_operations"][0]["before"]["sha256"]==metadata["sha256"]
            assert first_run["spec"]["input_workspace"]["id"]!=second_run["spec"]["input_workspace"]["id"]
            assert branch_target.read_bytes()==content and target.read_bytes()==content
            restore={"order_id":branch,"base_revision":call("GET",f"/orders/{branch}")["revision"],"idempotency_key":"restore-old","prompt":"Restore original binary into a fresh copy","access_mode":"autonomous","restore":{"folder_id":branch_folder,"path":"before.bin","source":{"run_id":run2,"tool_call_id":"import","version":"operation_before"}}}
            rejected("POST","/file-restorations",restore,409)
            restore_plan=prepare_plan("restore",restore)
            assert restore_plan.plan["preflight"]["review"]["content"]["sha256"]==metadata["sha256"]
            restored_run=commit_file_action(call,restore_plan);restored=restored_run["id"];assert attention(restored)["run"]["state"]=="completed"
            assert copy_file(restored_run,restore_plan,"before.bin").read_bytes()==content
            assert restored_run["spec"]["input_workspace"]["id"]!=second_run["spec"]["input_workspace"]["id"]
            assert not branch_target.with_name("before.bin").exists() and branch_target.read_bytes()==content
            assert replacement_target.read_bytes()==replacement and target.read_bytes()==content
            assert call("GET",f"/orders/{order}")["draft"]==draft["draft"]
            assert call("GET",f"/orders/{branch}")["draft"]==branch_draft["draft"]
            exported(order,source);exported(branch,second_source)
            kill();start();assert commit_file_action(call,recheck_file_action(call,prepared2))["id"]==run2
            assert commit_file_action(call,recheck_file_action(call,restore_plan))["id"]==restored
            assert replacement_target.read_bytes()==replacement and target.read_bytes()==content and branch_target.read_bytes()==content
            print(json.dumps({"result":"passed","model_used":False,"checks":["reservation restart","partial HTTP transfer then process kill","flushed blob with failed metadata confirmation","retry same upload","legacy direct import and restoration rejected","immutable input and confirmed ExecutionPlanV2","no run before exact confirmation","independent working copies preserve originals and prior results","explicit branch adopts imported result","approval survives restart","create and replace effects with failed receipts","exact reconciliation and separate resume","same-plan replay without new effects","stale target rejected before admission","restorable binary previous version in new copy","preserved both drafts","upload provenance and independent export hashes"]}))
        finally:kill()

if __name__=="__main__":main()
