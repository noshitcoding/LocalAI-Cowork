"""Windows/WSL development evidence for the actual reserved-volume component."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import uuid
import zipfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--wsl-distro", required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    spec = importlib.util.spec_from_file_location("managed_runtime_package_qualification", Path(__file__).with_name("qualify.py"))
    shared = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(shared)
    output = root / ".local/v2-rebuild-validation" / ("managed-volume-core-" + uuid.uuid4().hex)
    output.mkdir(parents=True)
    before = shared.source_inputs(root)
    (output / "sources-before.json").write_text(json.dumps(before, indent=2) + "\n", encoding="utf-8")
    report = {"scope": "Reserved ext4 volume implementation and runtime package verifier only", "result": "running", "checks": [],
              "complete_A_to_I": False, "complete_C": False, "runtime_service_integrated": False, "profile_released": False,
              "installed": False, "published": False, "limitations": ["Linux filesystem checks run inside the named WSL2 development distribution, not native hardware acceptance",
              "No common PlanV2/process/receiver adapter yet; no workload dispatch or gVisor profile qualification",
              "Published volumes deliberately remain booked pending an integrated exact physical stop witness",
              "Pool data reservations exclude the separately bounded runtime control/log/cache storage which remains to be integrated",
              "Unsigned development test/inspection binaries; no final signed runtime package or installer"]}

    def run(argv, label):
        r = subprocess.run(argv, cwd=root, capture_output=True, check=False)
        (output / (label + ".stdout.log")).write_bytes(r.stdout)
        (output / (label + ".stderr.log")).write_bytes(r.stderr)
        report["checks"].append({"name": label, "command": argv, "passed": r.returncode == 0,
                                  "stdout_sha256": shared.digest(output / (label + ".stdout.log")), "stderr_sha256": shared.digest(output / (label + ".stderr.log"))})
        print(json.dumps({"check": label, "passed": r.returncode == 0}), flush=True)
        if r.returncode:
            raise RuntimeError("Check failed: " + label)
        return (r.stdout + r.stderr).decode("utf-8", errors="replace")

    try:
        pkg = run([sys.executable, "tools/managed_runtime/qualify.py", "--wsl-distro", args.wsl_distro], "package-regressions")
        info = json.loads(pkg.splitlines()[-1])
        package_report = Path(info["report"])
        package = json.loads(package_report.read_text())
        if package["result"] != "passed":
            raise RuntimeError("Package regression report did not pass")
        shutil.copytree(package_report.parent, output / "package")
        report["package_report_sha256"] = shared.digest(package_report)
        prefix = ["wsl", "-d", args.wsl_distro, "--exec"]
        linux_root = subprocess.check_output(prefix + ["wslpath", "-u", str(root)], text=True).strip()
        linux_output = subprocess.check_output(prefix + ["wslpath", "-u", str(output)], text=True).strip()
        linux_home = subprocess.check_output(prefix + ["printenv", "HOME"], text=True).strip()
        target = linux_home + "/.cache/localai-cowork-v2/workspace-receive-target"
        log = run(prefix + ["bash", "-lc", "cd " + shlex.quote(linux_root) + " && cargo test --locked -p cowork-managed-runtime --test volumes --no-run --message-format=json --target-dir " + shlex.quote(target)], "volume-test-build")
        binaries=[]
        for line in log.splitlines():
            if line.startswith('{"reason":"compiler-artifact"'):
                entry=json.loads(line)
                if entry.get("target",{}).get("name")=="volumes" and entry.get("executable"):
                    binaries.append(entry["executable"])
        if len(binaries)!=1:
            raise RuntimeError("Expected the exact Linux volume test artifact")
        binary=binaries[0]
        content=subprocess.run(prefix+["cat",binary],capture_output=True,check=True).stdout
        if not content.startswith(b"\x7fELF") or len(content)>128*1024*1024:
            raise RuntimeError("Invalid volume test ELF")
        (output/"volume-tests-linux").write_bytes(content)
        binary_sha=hashlib.sha256(content).hexdigest()
        run(["wsl","-d",args.wsl_distro,"-u","root","--exec","python3",linux_root+"/tools/managed_runtime/volume_fixture.py",
             "--test-binary",binary,"--expected-test-sha256",binary_sha,"--output",linux_output],"physical-volumes")
        physical=json.loads((output/"physical-volume-report.json").read_text())
        if physical["result"]!="passed" or physical["test_binary_sha256"]!=binary_sha or not all(physical.get(k) for k in ("fixture_removed","mounts_absent","loops_detached")):
            raise RuntimeError("Physical fixture did not positively complete and clean up")
        run(prefix+["bash","-lc","cd "+shlex.quote(linux_root)+" && cargo clippy --locked -p cowork-managed-runtime --all-targets --target-dir "+shlex.quote(target)+" -- -D warnings"],"linux-clippy")
        after=shared.source_inputs(root)
        (output/"sources-after.json").write_text(json.dumps(after,indent=2)+"\n",encoding="utf-8")
        report["source_drift"]=[n for n in set(before)|set(after) if before.get(n)!=after.get(n)]
        if report["source_drift"]:
            raise RuntimeError("Volume component sources changed during qualification")
        archive=output/"review-source-inputs.zip"
        with zipfile.ZipFile(archive,"x",zipfile.ZIP_DEFLATED) as z:
            for name,digest in before.items():
                content=(root/name).read_bytes()
                if hashlib.sha256(content).hexdigest()!=digest:
                    raise RuntimeError("Source changed during archival")
                z.writestr(name,content)
        with zipfile.ZipFile(archive) as z:
            if z.testzip() is not None or any(hashlib.sha256(z.read(n)).hexdigest()!=h for n,h in before.items()):
                raise RuntimeError("Review archive did not verify")
        report.update(result="passed",physical_tests=13,source_count=len(before),source_manifest_sha256=shared.digest(output/"sources-before.json"),
                      test_binary_sha256=binary_sha,physical_report_sha256=shared.digest(output/"physical-volume-report.json"),
                      source_archive_sha256=shared.digest(archive))
    except Exception as error:
        report.update(result="failed",failure=str(error))
    (output/"report.json").write_text(json.dumps(report,indent=2)+"\n",encoding="utf-8")
    print(json.dumps({"report":str(output/"report.json"),"result":report["result"]}),flush=True)
    return int(report["result"]!="passed")


if __name__=="__main__":
    raise SystemExit(main())
