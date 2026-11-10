"""Actual, bounded private containerd/gVisor task proof; no product release."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import shlex
import subprocess
import sys
import uuid
import zipfile


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--wsl-distro",required=True)
    parser.add_argument("--controller-binary",type=Path)
    parser.add_argument("--controller-sha256")
    parser.add_argument("--controller-source-manifest",type=Path)
    parser.add_argument("--controller-case",choices=("copy","cancel","stop_read","delivery"),default="copy")
    parser.add_argument("--service-only",action="store_true",help="Actual pinned HTTPS service bridge with copy, rejected output and disconnected-start cancellation guests")
    parser.add_argument("--reject-output",action="store_true",help="Actual unsupported-output rejection after physical completion")
    parser.add_argument("--backend-only",action="store_true",help="One actual native backend copy/result path; no app qualification")
    parser.add_argument("--input-only",action="store_true",help="Verify actual native snapshot boundary without starting a workload")
    args=parser.parse_args()
    controller=bool(args.controller_binary)
    if args.controller_case!="copy" and not controller: parser.error("Controller cancellation requires the actual Controller ELF")
    if any((args.controller_binary,args.controller_sha256,args.controller_source_manifest)) and not all((args.controller_binary,args.controller_sha256,args.controller_source_manifest,args.service_only)): parser.error("Controller integration requires exact ELF/hash/source manifest and service-only")
    if controller:
        if not args.controller_binary.is_file() or args.controller_binary.is_symlink() or args.controller_binary.stat().st_size>128*1024**2 or hashlib.sha256(args.controller_binary.read_bytes()).hexdigest()!=args.controller_sha256: parser.error("Controller artifact differs from its captured ELF")
    if args.service_only and (args.backend_only or args.input_only): parser.error("Service proof uses its own focused suite")
    if args.reject_output and not args.backend_only: parser.error("Rejected output requires backend-only")
    if args.input_only and args.backend_only: parser.error("Select one focused suite")
    root=Path(__file__).resolve().parents[2]
    spec=importlib.util.spec_from_file_location("managed_sources",Path(__file__).with_name("qualify.py"))
    shared=importlib.util.module_from_spec(spec);spec.loader.exec_module(shared)
    out=root/".local/v2-rebuild-validation"/(("managed-https-controller-" if controller else "managed-https-service-" if args.service_only else (("managed-native-backend-rejected-" if args.reject_output else "managed-native-backend-") if args.backend_only else ("managed-native-input-" if args.input_only else "managed-actual-tasks-")))+uuid.uuid4().hex);out.mkdir(parents=True)
    def sources():
        result=shared.source_inputs(root)
        for directory in ("crates/domain","crates/contracts","crates/workflow-contracts","crates/runner-protocol","crates/broker")+(("services/process-runner","crates/agent-sandbox") if args.service_only else ()):
            for path in sorted((root/directory).rglob("*")):
                if path.is_file() and not path.is_symlink() and path.suffix in (".rs",".toml",".json"):
                    # Dependency integration executables are not compiled by
                    # this package's test/clippy targets. Keep dependency src,
                    # in-source unit tests, manifests and literal fixtures.
                    if path.suffix==".rs" and path.relative_to(root/directory).parts[0]=="tests":continue
                    result[path.relative_to(root).as_posix()]=shared.digest(path)
        if controller:
            manifest=json.loads(args.controller_source_manifest.read_bytes())
            if not isinstance(manifest,dict) or not 10<=len(manifest)<=4096: raise RuntimeError("Controller compiled-source closure manifest required")
            mandatory=("Cargo.toml","Cargo.lock","services/daemon/src/process_runner_config.rs","crates/runtime/src/lib.rs","crates/storage/src/lib.rs","crates/broker/src/lib.rs")
            if any(name not in manifest for name in mandatory): raise RuntimeError("Controller closure omits required production code")
            for name,expected in manifest.items():
                relative=Path(name)
                if relative.is_absolute() or any(p in ("..","archiv",".reference") for p in relative.parts) or not isinstance(expected,str) or len(expected)!=64: raise RuntimeError("Invalid controller source binding")
                actual=shared.digest(root/relative)
                if actual!=expected: raise RuntimeError("Controller source changed since its ELF capture: "+name)
                if name in result and result[name]!=expected: raise RuntimeError("Controller/native source closure disagrees: "+name)
                result[name]=actual
        return dict(sorted(result.items()))
    before=sources();(out/"sources-before.json").write_text(json.dumps(before,indent=2)+"\n")
    report={"scope":"Actual private gVisor task execution with fixture grant/qualification keys", "result":"running","checks":[],"profile_released":False,"service_integrated":False,"installed":False,"published":False,"complete_C":False,"complete_A_to_I":False,
            "limitations":["Private WSL2 proof using synthetic confirmed-plan and qualification metadata, no app grant-mint/transfer/UI acceptance path", "Published product reservations remain booked; fixture generation retirement is not a product release receipt", "Native Linux hardware, signed install/update/rollback and complete platform stress qualification remain open"]}
    report["limitations"].extend(["Physical fence conservatively ends at the initial signed execution lease; renewed longer-running task authority is not yet supported by the independent helper", "Completed output is durably saved; crash-time recovery of partially captured console output is still open", "No physical forkbomb, full RAM/CPU stress matrix, application result promotion, published-volume release or whole-generation product retirement API"])
    report["input_only"]=args.input_only
    report["backend_only"]=args.backend_only
    report["rejected_output"]=args.reject_output
    if controller:
        report["controller_source_manifest_sha256"]=shared.digest(args.controller_source_manifest)
        report["controller_case"]=args.controller_case
        report["limitations"][0]="Actual production Controller/Store/FileBroker plan and snapshot path, declared model proposal and fixture qualification keys; no Windows app or installed distribution acceptance"
        report["limitations"][-1]="No complete physical stress matrix, explicit original-file adoption, published-volume release or whole-generation product retirement API"
    if args.service_only: report["scope"]="Actual pinned private HTTPS runtime service -> common Broker -> gVisor -> result reads; synthetic fixture keys/admission metadata, no app or platform qualification"
    if controller: report["scope"]="Actual pinned private production Controller/Store/FileBroker -> HTTPS -> native gVisor -> retained result version; fixture qualification keys, no Windows UI or platform release"
    if args.backend_only: report["scope"]="One actual native backend copy/result path with fixture keys; no HTTP/App/platform qualification"
    if args.input_only: report["scope"]="Actual native incoming snapshot boundary only; no guest tasks or platform qualification"
    report["source_scope"]="Own crate/executor/tools and dependency manifests/src/literal fixtures; unrelated dependency integration test executables excluded because these targets do not compile them"
    def run(argv,label):
        r=subprocess.run(argv,cwd=root,capture_output=True,check=False)
        (out/(label+".stdout.log")).write_bytes(r.stdout);(out/(label+".stderr.log")).write_bytes(r.stderr)
        report["checks"].append({"name":label,"exit_code":r.returncode,"passed":r.returncode==0,"stdout_sha256":shared.digest(out/(label+".stdout.log")),"stderr_sha256":shared.digest(out/(label+".stderr.log"))})
        print(json.dumps({"check":label,"passed":r.returncode==0}),flush=True)
        if r.returncode:raise RuntimeError("Check failed: "+label)
        return (r.stdout+r.stderr).decode(errors="replace")
    archive=out/"review-source-inputs.zip"
    with zipfile.ZipFile(archive,"x",zipfile.ZIP_DEFLATED) as bundle:
        for name,digest in before.items():
            data=(root/name).read_bytes()
            if hashlib.sha256(data).hexdigest()!=digest:raise RuntimeError("Source changed before archival")
            bundle.writestr(name,data)
    try:
        tools=json.loads(run([sys.executable,"tools/managed_runtime/gvisor_fixture.py"],"exact-gvisor").splitlines()[-1])
        containerd=json.loads(run([sys.executable,"tools/managed_runtime/containerd_fixture.py"],"exact-containerd").splitlines()[-1])
        manifest_bytes=(json.dumps({"format_version":1,"version":tools["version"],"files":tools["files"]},sort_keys=True,separators=(",",":"))+"\n").encode();(out/"gvisor-manifest.json").write_bytes(manifest_bytes)
        prefix=["wsl","-d",args.wsl_distro,"--exec"]
        def linux(path):return subprocess.check_output(prefix+["wslpath","-u",str(path)],text=True).strip()
        lr=linux(root);lo=linux(out)
        target="/home/noah/.cache/localai-cowork-v2/workspace-receive-target"
        protoc=lr+"/.local/managed-runtime-build-tools/33.5/linux-x86_64/bin/protoc"
        def cargo(arguments):return prefix+["bash","-lc","cd "+shlex.quote(lr)+" && PROTOC="+shlex.quote(protoc)+" cargo "+arguments+" --target-dir "+shlex.quote(target)]
        report["physical_build_profile"]="release"
        test_package="cowork-process-runner --features managed-runtime-service --bin cowork-managed-runtime-service" if args.service_only else "cowork-managed-runtime --features containerd --test tasks"
        test_name="cowork-managed-runtime-service" if args.service_only else "tasks"
        log=run(cargo("test --release --locked -p "+test_package+" --no-run --message-format=json"),"task-build")
        artifacts=[json.loads(line) for line in log.splitlines() if line.startswith('{"reason":"compiler-artifact"')]
        binary=next(v["executable"] for v in artifacts if v.get("target",{}).get("name")==test_name and v.get("executable"))
        run(prefix+["/usr/bin/strip","--strip-debug","-o",lo+"/tasks-linux",binary],"bounded-test-elf")
        log=run(cargo("build --release --locked -p cowork-managed-runtime --features containerd --bin cowork-runtime-fence --message-format=json"),"fence-build")
        artifacts=[json.loads(line) for line in log.splitlines() if line.startswith('{"reason":"compiler-artifact"')]
        binary=next(v["executable"] for v in artifacts if v.get("target",{}).get("name")=="cowork-runtime-fence" and v.get("executable"))
        run(prefix+["/usr/bin/strip","--strip-debug","-o",lo+"/fence-linux",binary],"bounded-fence-elf")
        log=run(cargo("build --release --locked -p cowork-managed-runtime --features containerd --bin cowork-runsc-driver --message-format=json"),"driver-build")
        artifacts=[json.loads(line) for line in log.splitlines() if line.startswith('{"reason":"compiler-artifact"')]
        binary=next(v["executable"] for v in artifacts if v.get("target",{}).get("name")=="cowork-runsc-driver" and v.get("executable"))
        run(prefix+["/usr/bin/strip","--strip-debug","-o",lo+"/driver-linux",binary],"bounded-driver-elf")
        log=run(cargo("test --release --locked -p cowork-managed-runtime --features containerd --lib --no-run --message-format=json"),"reaper-test-build")
        artifacts=[json.loads(line) for line in log.splitlines() if line.startswith('{"reason":"compiler-artifact"')]
        binary=next(v["executable"] for v in artifacts if "lib" in v.get("target",{}).get("kind",[]) and v.get("executable"))
        run(prefix+["/usr/bin/strip","--strip-debug","-o",lo+"/reaper-linux",binary],"bounded-reaper-test-elf")
        if args.service_only:
            log=run(cargo("build --release --locked -p cowork-process-runner --features managed-runtime-service --bin cowork-managed-runtime-service --message-format=json"),"service-build")
            artifacts=[json.loads(line) for line in log.splitlines() if line.startswith('{"reason":"compiler-artifact"')]
            binary=next(v["executable"] for v in artifacts if v.get("target",{}).get("name")=="cowork-managed-runtime-service" and v.get("executable"))
            run(prefix+["/usr/bin/strip","--strip-debug","-o",lo+"/service-linux",binary],"bounded-service-elf")
        if args.service_only:
            for package,label in (("cowork-process-runner --features managed-runtime-service --bin cowork-managed-runtime-service","retention-service"),("cowork-managed-runtime --features containerd --lib","retention-native")):
                text=run(cargo("test --release --locked -p "+package+" retention_tests"),label)
                if "test result: ok. 1 passed;" not in text: raise RuntimeError("Missing targeted retention case: "+label)
        run(prefix+["cc","-static","-O2","-Wall","-Wextra","-Werror",lr+"/tools/managed_runtime/workload_probe.c","-o",lo+"/probe-linux"],"static-probe")
        run(prefix+["cc","-static","-O2","-Wall","-Wextra","-Werror",lr+"/tools/managed_runtime/sandbox_pause.c","-o",lo+"/pause-linux"],"static-pause")
        controller_arguments=[]
        if controller:
            controller_arguments=["--controller-binary",linux(args.controller_binary.resolve(strict=True)),"--controller-sha256",args.controller_sha256,"--controller-case",args.controller_case]
        run(["wsl","-d",args.wsl_distro,"-u","root","--exec","python3",lr+"/tools/managed_runtime/volume_fixture.py","--suite","actual-tasks","--test-binary",lo+"/tasks-linux","--expected-test-sha256",shared.digest(out/"tasks-linux"),"--gvisor-directory",lr+"/.local/managed-runtime-fixtures/gvisor-release-20261026.0","--gvisor-manifest",lo+"/gvisor-manifest.json","--gvisor-manifest-sha256",hashlib.sha256(manifest_bytes).hexdigest(),"--fence-binary",lo+"/fence-linux","--fence-sha256",shared.digest(out/"fence-linux"),"--driver-binary",lo+"/driver-linux","--driver-sha256",shared.digest(out/"driver-linux"),"--reaper-binary",lo+"/reaper-linux","--reaper-sha256",shared.digest(out/"reaper-linux"),"--containerd-binary",lr+"/.local/managed-runtime-fixtures/containerd-2.2.1/containerd","--containerd-sha256",containerd["sha256"],"--probe-binary",lo+"/probe-linux","--probe-sha256",shared.digest(out/"probe-linux"),"--pause-binary",lo+"/pause-linux","--pause-sha256",shared.digest(out/"pause-linux"),"--output",lo]+controller_arguments+(["--service-binary",lo+"/service-linux","--service-sha256",shared.digest(out/"service-linux")] if args.service_only else [])+(["--input-only"] if args.input_only else (["--backend-only"]+(["--reject-output"] if args.reject_output else []) if args.backend_only else [])),"native-input" if args.input_only else ("native-backend" if args.backend_only else "actual-tasks"))
        physical=json.loads((out/"physical-volume-report.json").read_text())
        if physical["result"]!="passed" or not all(physical.get(k) for k in ("fixture_removed","mounts_absent","loops_detached","toolset_removed","fixture_generation_retired")):raise RuntimeError("Positive fixture retirement missing")
        text=(out/"volume-tests.stdout.log").read_text()
        if controller and args.controller_case == "delivery":
            from delivery_evidence import validate_delivery
            validate_delivery(out, physical)
            if text.count("managed_https_controller=verified;") != 1:
                raise RuntimeError("Missing actual production controller callback evidence")
            report.update(actual_guest_jobs=2,actual_controller_cases=1,workload_tasks_launched=True,
                          controller_elf_sha256=args.controller_sha256,windows_ui_acceptance=False,
                          original_published_volume_delivery=True,local_results_readable_after_gc=True)
            report["limitations"] = [
                "Private WSL2 fixture with actual production Controller/Store/FileBroker and declared model proposal; no installed Windows App or owned distribution qualification",
                "Only same-process original completed delivery custody; general crash-time native adoption and controlled generation retirement remain separate",
                "Native Linux hardware, signed install/update/rollback, complete stress matrix and explicit original-file adoption remain open",
                "Physical lease extensions beyond the original signed deadline remain unsupported",
            ]
        elif controller:
            if text.count("managed_https_controller=verified;")!=1: raise RuntimeError("Missing actual production controller callback evidence")
            diagnostic=physical.get("native_failure_slot",{})
            diagnostic_bytes=(out/"native-failure-slot.bin").read_bytes()
            if diagnostic.get("diagnostic_only") is not True or diagnostic.get("readback_verified") is not True or len(diagnostic_bytes)!=16384 or hashlib.sha256(diagnostic_bytes).hexdigest()!=diagnostic.get("sha256"):raise RuntimeError("Original native diagnostic slot readback differs")
            diagnostic_length=int.from_bytes(diagnostic_bytes[:4],"little")
            if diagnostic_length!=diagnostic.get("declared_payload_bytes") or diagnostic_length>16384-36:raise RuntimeError("Native diagnostic slot length differs")
            if diagnostic_length:
                if hashlib.sha256(diagnostic_bytes[36:36+diagnostic_length]).digest()!=diagnostic_bytes[4:36] or any(diagnostic_bytes[36+diagnostic_length:]) or json.loads(diagnostic_bytes[36:36+diagnostic_length]).get("diagnostic_only") is not True:raise RuntimeError("Native diagnostic slot framing differs")
            elif any(diagnostic_bytes):raise RuntimeError("Empty native diagnostic slot changed")
            report["native_failure_slot_sha256"]=diagnostic["sha256"]
            report["native_failure_slot_readback_verified"]=True
            records=physical.get("task_records",[])
            if len(records)!=1 or not records[0]["physical_stop_confirmed"] or not records[0]["control_process_reaped"] or not records[0]["output_complete"] or len(records[0]["mutations"])!=12 or any(not m["acknowledged"] for m in records[0]["mutations"]): raise RuntimeError("Missing exact physical controller job completion")
            observed=json.loads((out/"managed-service-observation.json").read_text())
            control=observed["controller"];receipt=observed["receipt"]["payload"]
            if control["status"]!="passed" or receipt["state"]["state"]!="completed":raise RuntimeError("Controller callback lacks the original completed receipt")
            result=receipt["state"]["result"]
            if args.controller_case in ("cancel","stop_read"):
                expected_business="interrupted" if args.controller_case=="stop_read" else "canceled"
                if control.get("scenario")!=args.controller_case or control.get("business_state")!=expected_business or control.get("process_state",{}).get("state")!="reconciled_completed" or result["termination"]!="canceled":raise RuntimeError("Missing actual Controller cancellation/reconciliation separation")
                if args.controller_case=="cancel" and (control.get("result_version_id") is not None or control.get("manifest_sha256") is not None):raise RuntimeError("Business cancellation unexpectedly acquired result content")
                if args.controller_case=="stop_read" and (not control.get("result_version_id") or not control.get("manifest_sha256") or control["manifest_sha256"]!=observed["native_recovery"]["manifest"]["sha256"]):raise RuntimeError("Stopped original lacks its explicit verified local Ready result")
                if control["process_state"]["result"]!=result:raise RuntimeError("Reconciled Controller result differs from the original signed physical outcome")
                for phase in ("start","completed"):
                    gate=json.loads((out/("controller-native-"+phase+".json")).read_text())
                    record_bytes=(out/("controller-native-"+phase+"-record.json")).read_bytes()
                    if gate.get("diagnostic_only") is not True or gate["run_id"]!=control["run_id"] or gate["process_attempt_id"]!=control["process_attempt_id"] or gate["request_sha256"]!=receipt["request"]["request_sha256"] or gate["physical_record_sha256"]!=hashlib.sha256(record_bytes).hexdigest():raise RuntimeError("Missing original native cancellation gate binding")
                    native=json.loads(record_bytes)
                    if native["task_id"]!=records[0]["task_id"] or native["request_sha256"]!=records[0]["request_sha256"]:raise RuntimeError("Cancellation gate refers to another physical job")
                    if phase=="start" and not any(m["operation"]=="task_start" and m["container_id"]==native["task_id"] and m["acknowledged"] and m["response"]=="succeeded" for m in native["mutations"]):raise RuntimeError("Cancel preceded original native workload start acknowledgement")
                    if phase=="completed" and gate["execution_receipt_sha256"]!=control["execution_receipt_sha256"]:raise RuntimeError("Completion gate is not the reconciled original signed receipt")
                if not records[0]["guest_exit_confirmed"] and result["exit_code"] is not None:raise RuntimeError("Unconfirmed canceled guest wait became a numeric exit")
            elif not control["result_version_id"] or not records[0]["guest_exit_confirmed"] or result["termination"]!="exited" or result["exit_code"]!=0:raise RuntimeError("Controller callback did not retain the real successful result version")
            outcome_bytes=(out/"task-outcome-0.json").read_bytes()
            if hashlib.sha256(outcome_bytes).hexdigest()!=records[0]["outcome_sha256"] or json.loads(outcome_bytes)!=receipt["state"]["result"]:raise RuntimeError("Signed controller job receipt differs from original physical outcome")
            report.update(actual_guest_jobs=1,actual_controller_cases=1,workload_tasks_launched=True,controller_elf_sha256=args.controller_sha256,windows_ui_acceptance=False)
        elif args.service_only:
            if text.count("managed_https_environment=verified;")!=1 or text.count("managed_https_copy=verified;")!=1 or text.count("managed_https_export_rejected=verified;")!=1 or text.count("managed_https_cancel=verified;")!=1: raise RuntimeError("Missing actual HTTPS service evidence")
            records=physical.get("task_records",[])
            if len(records)!=3 or any(not r["physical_stop_confirmed"] or not r["control_process_reaped"] or not r["output_complete"] or len(r["mutations"])!=12 or any(not m["acknowledged"] for m in r["mutations"]) for r in records): raise RuntimeError("Missing three original physical completion/control/EOF records")
            observations=json.loads((out/"managed-service-observation.json").read_text())
            if {o["mode"] for o in observations}!={"copy","symlink_result","block"}: raise RuntimeError("Missing original service receipts")
            for index,record in enumerate(records):
                matches=[o for o in observations if o["receipt"]["payload"]["request"]["request_sha256"]==record["request_sha256"]]
                if len(matches)!=1: raise RuntimeError("Physical result lacks its exact original receipt")
                observed=matches[0];state=observed["receipt"]["payload"]["state"]
                if state["state"]!="completed": raise RuntimeError("Non-completed service receipt")
                outcome_bytes=(out/("task-outcome-"+str(index)+".json")).read_bytes()
                if hashlib.sha256(outcome_bytes).hexdigest()!=record["outcome_sha256"] or json.loads(outcome_bytes)!=state["result"]: raise RuntimeError("Original signed service result differs from durable physical outcome")
                result=state["result"]
                if observed["mode"]=="block":
                    if result["termination"]!="canceled" or observed["start_receipt_body_read"] is not False or observed["actual_guest_started_before_stop"] is not True: raise RuntimeError("Missing actual accepted-start receipt-loss/cancel proof")
                    if not record["guest_exit_confirmed"] and result["exit_code"] is not None: raise RuntimeError("Unconfirmed wait status became a fabricated numeric exit")
                elif not record["guest_exit_confirmed"] or result["termination"]!="exited" or result["exit_code"]!=0: raise RuntimeError("Normal guest lacks original confirmed exit zero")
            report.update(actual_guest_jobs=3,actual_https_service_cases=1,workload_tasks_launched=True,numeric_cancel_exit_required=False,unknown_wait_never_becomes_numeric_exit=True)
        elif args.input_only:
            if text.count("managed_snapshot_transfer=verified;")!=2 or text.count("managed_snapshot_native_tree_substitution=rejected;")!=1: raise RuntimeError("Missing native transfer/custody/substitution evidence")
            if physical.get("workload_tasks_launched") is not False or physical.get("task_records"): raise RuntimeError("Input-only unexpectedly started a task")
            report.update(native_input_cases=3,actual_guest_jobs=0,workload_tasks_launched=False)
        elif args.backend_only:
            if text.count("managed_backend_export_rejected=verified;" if args.reject_output else "managed_backend_copy=verified;")!=1: raise RuntimeError("Missing native backend/result evidence")
            records=physical.get("task_records",[])
            if len(records)!=1 or not records[0]["physical_stop_confirmed"] or not records[0]["guest_exit_confirmed"] or len(records[0]["mutations"])!=12 or any(not m["acknowledged"] for m in records[0]["mutations"]): raise RuntimeError("Missing physical task completion for backend")
            report.update(actual_guest_jobs=1,native_backend_cases=1,workload_tasks_launched=True)
        else:
            if any("actual_gvisor_mode="+mode+";" not in text for mode in ("copy","background","cancel","deadline","output","disk","exit128","wait_fault","input_deadline")):raise RuntimeError("Missing actual workload evidence")
            observations=physical.get("owner_observations",{})
            reaper=physical.get("reaper_observation",{})
            if not all(reaper.get(k) for k in ("paused_past_drop_window","resumed_and_fenced","original_child_reaped_while_runner_alive")) or reaper.get("reservation_released") is not False:raise RuntimeError("Missing delayed original-fence reaping evidence")
            paths=physical.get("package-path_observation",{})
            if len(paths)!=4 or not all(paths.values()):raise RuntimeError("Missing replaceable package ancestry rejections")
            if set(observations)!={"owner","lease"} or not observations["lease"]["frozen_while_owner_stopped"] or not all(o["owner_sigkill"] and o["workload_frozen"] and o["workload_population_zero"] and o["unknown_reservation_retained"] for o in observations.values()):raise RuntimeError("Missing independent owner-death/lease enforcement evidence")
            records=physical.get("task_records",[])
            if any(r["format_version"]!=2 for r in records):raise RuntimeError("Unrecognized physical journal version")
            completed=[r for r in records if r["physical_stop_confirmed"]]
            uncertain=[r for r in records if not r["physical_stop_confirmed"]]
            if len(completed)!=8 or len(uncertain)!=3 or any(len(r["mutations"])!=12 or not r["output_complete"] or not r["outcome_sha256"] or not r["control_process_reaped"] or any(not m["acknowledged"] for m in r["mutations"]) for r in completed):raise RuntimeError("Missing separate exact task/root/control/output receipts")
            fault=[r for r in uncertain if r["sandbox_live_at_guest_wait"] and r["reported_exit_status"]==128 and not r["guest_exit_confirmed"] and not r["guest_wait_evidence_sha256"]]
            actual128=[r for r in completed if r["reported_exit_status"]==128 and r["guest_exit_confirmed"] and r["guest_wait_evidence_sha256"]]
            if len(fault)!=1 or len(actual128)!=1:raise RuntimeError("Internal wait failure not distinguished from genuine guest exit128")
        run(prefix+["bash","-lc","cd "+shlex.quote(lr)+" && PROTOC="+shlex.quote(protoc)+" cargo clippy --locked -p cowork-managed-runtime --features containerd --all-targets --no-deps --target-dir "+shlex.quote(target)+" -- -D warnings"],"feature-clippy")
        if args.service_only:
            run(prefix+["bash","-lc","cd "+shlex.quote(lr)+" && PROTOC="+shlex.quote(protoc)+" cargo clippy --locked -p cowork-process-runner --features managed-runtime-service --bin cowork-managed-runtime-service --no-deps --target-dir "+shlex.quote(target)+" -- -D warnings"],"service-clippy")
            run(["rustfmt","--edition","2024","--check","services/process-runner/src/managed_main.rs"],"service-format")
        run(["cargo","fmt","-p","cowork-managed-runtime","--","--check"],"format")
        after=sources();(out/"sources-after.json").write_text(json.dumps(after,indent=2)+"\n")
        report["source_drift"]=sorted(n for n in set(before)|set(after) if before.get(n)!=after.get(n))
        if report["source_drift"]:raise RuntimeError("Task source dependencies changed during proof")
        with zipfile.ZipFile(archive) as bundle:
            if bundle.testzip() is not None or any(hashlib.sha256(bundle.read(n)).hexdigest()!=h for n,h in before.items()):raise RuntimeError("Source archive failed readback")
        report.update(result="passed",source_count=len(before),source_archive_sha256=shared.digest(archive))
        if not args.input_only and not args.backend_only and not args.service_only: report.update(actual_modes=9,independent_owner_probes=2,actual_guest_jobs=11)
    except Exception as error:report.update(result="failed",failure=str(error))
    (out/"report.json").write_text(json.dumps(report,indent=2)+"\n")
    print(json.dumps({"report":str(out/"report.json"),"result":report["result"]}),flush=True)
    return int(report["result"]!="passed")


if __name__=="__main__":raise SystemExit(main())
