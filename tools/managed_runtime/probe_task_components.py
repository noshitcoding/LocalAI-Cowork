"""Bounded private WSL component proof before integrating actual gVisor tasks.

This is not a shipped runtime, public start route or platform qualification.
"""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import re
import shlex
import subprocess
import sys
import uuid
import zipfile


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--wsl-distro",required=True)
    args=parser.parse_args()
    root=Path(__file__).resolve().parents[2]
    spec=importlib.util.spec_from_file_location("shared_managed_runtime_sources",Path(__file__).with_name("qualify.py"))
    shared=importlib.util.module_from_spec(spec);spec.loader.exec_module(shared)
    out=root/".local/v2-rebuild-validation"/("managed-task-components-"+uuid.uuid4().hex);out.mkdir(parents=True)
    def sources():
        result=shared.source_inputs(root)
        for directory in ("crates/domain","crates/contracts","crates/workflow-contracts","crates/runner-protocol","crates/broker"):
            for path in sorted((root/directory).rglob("*")):
                if path.is_file() and not path.is_symlink() and path.suffix in (".rs",".toml",".json"):
                    if path.suffix==".rs" and path.relative_to(root/directory).parts[0]=="tests":continue
                    result[path.relative_to(root).as_posix()]=shared.digest(path)
        return dict(sorted(result.items()))
    before=sources();(out/"sources-before.json").write_text(json.dumps(before,indent=2)+"\n")
    report={"scope":"Private managed-input signature, bounded FIFO, pinned tools/rootfs and cgroup configuration components",
            "result":"running","checks":[],"workload_tasks_launched":False,"profile_released":False,"runtime_service_integrated":False,
            "complete_C":False,"complete_A_to_I":False,"installed":False,"published":False,
            "limitations":["Five actual component tests; native receiver binding now covered by probe_tasks --input-only in WSL2; no gVisor task create/start or physical CPU/RAM/PID stress test in this adapter",
                           "Generated fixture controller keys and synthetic confirmed-plan metadata, not an app PlanV2 grant-minting/UI acceptance path",
                           "No transfer endpoint, all-path execution convergence, complete service resource admission or new platform qualification",
                           "Published reservations remain booked; no fabricated task stop or release witness",
                           "Native Linux hardware, shipped dedicated WSL distribution and final signed package acceptance remain open"]}
    def run(argv,label,minimum=0):
        r=subprocess.run(argv,cwd=root,capture_output=True,check=False)
        (out/(label+".stdout.log")).write_bytes(r.stdout);(out/(label+".stderr.log")).write_bytes(r.stderr)
        text=(r.stdout+r.stderr).decode(errors="replace")
        count=sum(int(n) for n in re.findall(r"test result: ok\. (\d+) passed;",text))
        passed=r.returncode==0 and count>=minimum
        report["checks"].append({"name":label,"exit_code":r.returncode,"passed":passed,"tests":count,
                                "stdout_sha256":shared.digest(out/(label+".stdout.log")),"stderr_sha256":shared.digest(out/(label+".stderr.log"))})
        print(json.dumps({"check":label,"passed":passed,"tests":count}),flush=True)
        if not passed:raise RuntimeError("Check failed: "+label)
        return text
    try:
        fixtures=json.loads(run([sys.executable,"tools/managed_runtime/gvisor_fixture.py"],"exact-toolset").splitlines()[-1])
        manifest={"format_version":1,"version":fixtures["version"],"files":fixtures["files"]}
        manifest_bytes=(json.dumps(manifest,sort_keys=True,separators=(",",":"))+"\n").encode()
        (out/"gvisor-toolset-manifest.json").write_bytes(manifest_bytes)
        prefix=["wsl","-d",args.wsl_distro,"--exec"]
        linux_root=subprocess.check_output(prefix+["wslpath","-u",str(root)],text=True).strip()
        linux_out=subprocess.check_output(prefix+["wslpath","-u",str(out)],text=True).strip()
        home=subprocess.check_output(prefix+["printenv","HOME"],text=True).strip()
        protoc=json.loads(run([sys.executable,"tools/managed_runtime/protoc.py","--platform","linux-x86_64"],"private-protoc").splitlines()[-1])
        linux_protoc=subprocess.check_output(prefix+["wslpath","-u",str(Path(protoc["directory"])/"bin/protoc")],text=True).strip()
        target=home+"/.cache/localai-cowork-v2/workspace-receive-target"
        def cargo(arguments):return prefix+["bash","-lc","cd "+shlex.quote(linux_root)+" && PROTOC="+shlex.quote(linux_protoc)+" cargo "+arguments+" --target-dir "+shlex.quote(target)]
        log=run(cargo("test --locked -p cowork-managed-runtime --features containerd --test task_components --no-run --message-format=json"),"test-build")
        entries=[json.loads(line) for line in log.splitlines() if line.startswith('{"reason":"compiler-artifact"')]
        binaries=[v["executable"] for v in entries if v.get("target",{}).get("name")=="task_components" and v.get("executable")]
        if len(binaries)!=1:raise RuntimeError("Expected exact native component test executable")
        binary=binaries[0]
        content=subprocess.run(prefix+["cat",binary],capture_output=True,check=True).stdout
        if len(content)>128*1024**2:
            run(prefix+["/usr/bin/strip","--strip-debug","-o",linux_out+"/task-components-linux",binary],"bounded-debug-strip")
            binary=linux_out+"/task-components-linux";content=subprocess.run(prefix+["cat",binary],capture_output=True,check=True).stdout
        else:(out/"task-components-linux").write_bytes(content)
        if not content.startswith(b"\x7fELF") or len(content)>128*1024**2:raise RuntimeError("Native test executable exceeded its bound")
        test_sha=hashlib.sha256(content).hexdigest()
        run(prefix+[binary],"normal-grant-tests",1)
        run(["wsl","-d",args.wsl_distro,"-u","root","--exec","python3",linux_root+"/tools/managed_runtime/volume_fixture.py",
             "--suite","task-components","--test-binary",binary,"--expected-test-sha256",test_sha,
             "--gvisor-directory",linux_root+"/.local/managed-runtime-fixtures/gvisor-release-20261026.0",
             "--gvisor-manifest",linux_out+"/gvisor-toolset-manifest.json","--gvisor-manifest-sha256",hashlib.sha256(manifest_bytes).hexdigest(),"--output",linux_out],"physical-components")
        physical=json.loads((out/"physical-volume-report.json").read_text())
        if physical["result"]!="passed" or physical["test_binary_sha256"]!=test_sha or not all(physical.get(k) for k in ("fixture_removed","mounts_absent","loops_detached","toolset_removed")):
            raise RuntimeError("Actual component fixture or positive cleanup failed")
        run(cargo("test --locked -p cowork-domain --lib environments::sandbox_tests"),"linux-environments",6)
        run(cargo("test --locked -p cowork-runner-protocol --lib"),"linux-runner-protocol",21)
        run(["cargo","test","--locked","-p","cowork-domain","--lib","environments::sandbox_tests"],"windows-environments",6)
        run(["cargo","test","--locked","-p","cowork-runner-protocol","--lib"],"windows-runner-protocol",21)
        run(prefix+["bash","-lc","cd "+shlex.quote(linux_root)+" && PROTOC="+shlex.quote(linux_protoc)+" cargo clippy --locked -p cowork-managed-runtime --features containerd --all-targets --no-deps --target-dir "+shlex.quote(target)+" -- -D warnings"],"feature-clippy")
        run(["cargo","fmt","-p","cowork-managed-runtime","-p","cowork-runner-protocol","--","--check"],"format")
        after=sources();(out/"sources-after.json").write_text(json.dumps(after,indent=2)+"\n")
        report["source_drift"]=sorted(n for n in set(before)|set(after) if before.get(n)!=after.get(n))
        if report["source_drift"]:raise RuntimeError("Project source dependencies changed during this component proof")
        archive=out/"review-source-inputs.zip"
        with zipfile.ZipFile(archive,"x",zipfile.ZIP_DEFLATED) as bundle:
            for name,digest in before.items():
                data=(root/name).read_bytes()
                if hashlib.sha256(data).hexdigest()!=digest:raise RuntimeError("Source changed before archival")
                bundle.writestr(name,data)
        with zipfile.ZipFile(archive) as bundle:
            if bundle.testzip() is not None or any(hashlib.sha256(bundle.read(n)).hexdigest()!=h for n,h in before.items()):raise RuntimeError("Source archive verification failed")
        report.update(result="passed",source_count=len(before),physical_tests=5,source_archive_sha256=shared.digest(archive),test_binary_sha256=test_sha,gvisor_manifest_sha256=hashlib.sha256(manifest_bytes).hexdigest())
    except Exception as error:report.update(result="failed",failure=str(error))
    (out/"report.json").write_text(json.dumps(report,indent=2)+"\n")
    print(json.dumps({"report":str(out/"report.json"),"result":report["result"]}),flush=True)
    return int(report["result"]!="passed")


if __name__=="__main__":raise SystemExit(main())
