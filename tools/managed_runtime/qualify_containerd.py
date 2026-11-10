"""Source-bound private containerd control and shared dispatch-handoff evidence.

Development component qualification only; never installs or launches a workload.
"""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import re
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
    output = root / ".local/v2-rebuild-validation" / ("managed-containerd-control-" + uuid.uuid4().hex)
    output.mkdir(parents=True)
    before = shared.source_inputs(root)
    (output / "sources-before.json").write_text(json.dumps(before, indent=2) + "\n")
    report = {"scope": "Private containerd control library/transport and existing shared Broker handoff only",
              "result": "running", "checks": [], "complete_A_to_I": False, "complete_C": False,
              "runtime_service_integrated": False, "workload_tasks_launched": False, "profile_released": False,
              "installed": False, "published": False, "limitations": [
                  "Actual control tests run inside WSL2 development, not native Linux hardware or dedicated shipped WSL distribution acceptance",
                  "Read-only version RPC only; no task create/start, gVisor, proxy, PlanV2/receiver grant or result-promotion integration",
                  "Daemon root exit never proves task/shim termination and never releases workspace or control capacity",
                  "Control directory must be provided from separately reserved storage; aggregate service/cache/compute admission remains open",
                  "Private development build tools and exact upstream fixture ELF, not final signed runtime/tool packages",
                  "SQLite replay/reopen regressions only; ignored PostgreSQL fixture is not claimed by this report"]}

    def run(argv, label, minimum=0):
        result = subprocess.run(argv, cwd=root, capture_output=True, check=False)
        (output / (label + ".stdout.log")).write_bytes(result.stdout)
        (output / (label + ".stderr.log")).write_bytes(result.stderr)
        text = (result.stdout + result.stderr).decode("utf-8", errors="replace")
        count = sum(map(int, re.findall(r"test result: ok\. (\d+) passed;", text)))
        passed = result.returncode == 0 and count >= minimum
        report["checks"].append({"name": label, "command": argv, "exit_code": result.returncode, "tests": count,
                                  "passed": passed, "stdout_sha256": shared.digest(output / (label + ".stdout.log")),
                                  "stderr_sha256": shared.digest(output / (label + ".stderr.log"))})
        print(json.dumps({"check": label, "passed": passed, "tests": count}), flush=True)
        if not passed:
            raise RuntimeError("Check failed: " + label)
        return text

    def archive(manifest, name):
        path = output / name
        with zipfile.ZipFile(path, "x", zipfile.ZIP_DEFLATED) as bundle:
            for relative, sha in manifest.items():
                content = (root / relative).read_bytes()
                if hashlib.sha256(content).hexdigest() != sha:
                    raise RuntimeError("Source changed before archival: " + relative)
                bundle.writestr(relative, content)
        with zipfile.ZipFile(path) as bundle:
            if bundle.testzip() is not None or any(hashlib.sha256(bundle.read(n)).hexdigest() != h for n, h in manifest.items()):
                raise RuntimeError("Review source archive did not verify")
        return shared.digest(path)

    def handoff_sources():
        paths = [root / n for n in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml")]
        for directory in ("crates/runner-protocol", "crates/contracts", "crates/workflow-contracts", "crates/domain",
                          "crates/executor", "crates/agent-sandbox", "services/process-runner"):
            paths += [p for p in (root / directory).rglob("*") if p.is_file() and not p.is_symlink()
                      and p.suffix in (".rs", ".toml") and not (p.suffix==".rs" and p.relative_to(root/directory).parts[0]=="tests")]
        return {p.relative_to(root).as_posix(): shared.digest(p) for p in sorted(paths)}

    try:
        pkg = run([sys.executable, "tools/managed_runtime/qualify.py", "--wsl-distro", args.wsl_distro], "package-regressions")
        package_report = Path(json.loads(pkg.splitlines()[-1])["report"])
        if json.loads(package_report.read_text())["result"] != "passed":
            raise RuntimeError("Package report did not pass")
        shutil.copytree(package_report.parent, output / "package")
        report["package_report_sha256"] = shared.digest(package_report)
        protoc = json.loads(run([sys.executable, "tools/managed_runtime/protoc.py", "--platform", "linux-x86_64"], "private-protoc").splitlines()[-1])
        fixture = json.loads(run([sys.executable, "tools/managed_runtime/containerd_fixture.py"], "private-upstream-fixture").splitlines()[-1])
        prefix = ["wsl", "-d", args.wsl_distro, "--exec"]
        linux_root = subprocess.check_output(prefix + ["wslpath", "-u", str(root)], text=True).strip()
        linux_output = subprocess.check_output(prefix + ["wslpath", "-u", str(output)], text=True).strip()
        linux_protoc = subprocess.check_output(prefix + ["wslpath", "-u", str(Path(protoc["directory"]) / "bin/protoc")], text=True).strip()
        linux_fixture = subprocess.check_output(prefix + ["wslpath", "-u", fixture["binary"]], text=True).strip()
        linux_home = subprocess.check_output(prefix + ["printenv", "HOME"], text=True).strip()
        target = linux_home + "/.cache/localai-cowork-v2/workspace-receive-target"
        def cargo(arguments):
            return prefix + ["bash", "-lc", "cd " + shlex.quote(linux_root) + " && PROTOC=" + shlex.quote(linux_protoc)
                             + " cargo " + arguments + " --target-dir " + shlex.quote(target)]
        log = run(cargo("test --locked -p cowork-managed-runtime --features containerd --test containerd --no-run --message-format=json"), "control-test-build")
        binaries = [json.loads(line)["executable"] for line in log.splitlines() if line.startswith('{"reason":"compiler-artifact"')
                    and json.loads(line).get("target", {}).get("name") == "containerd" and json.loads(line).get("executable")]
        if len(binaries) != 1:
            raise RuntimeError("Expected exact actual control test ELF")
        binary = binaries[0]
        content = subprocess.run(prefix + ["cat", binary], capture_output=True, check=True).stdout
        if len(content)>128*1024**2:
            run(prefix+["/usr/bin/strip","--strip-debug","-o",linux_output+"/control-tests-linux",binary],"bounded-debug-strip")
            binary=linux_output+"/control-tests-linux"
            content=subprocess.run(prefix+["cat",binary],capture_output=True,check=True).stdout
        if not content.startswith(b"\x7fELF") or len(content) > 128 * 1024**2:
            raise RuntimeError("Control test ELF exceeded its declared bound")
        (output / "control-tests-linux").write_bytes(content)
        test_sha = hashlib.sha256(content).hexdigest()
        run(["wsl", "-d", args.wsl_distro, "-u", "root", "--exec", "python3", linux_root + "/tools/managed_runtime/rpc_fixture.py",
             "--test-binary", binary, "--test-sha256", test_sha, "--containerd-binary", linux_fixture,
             "--containerd-sha256", fixture["sha256"], "--output", linux_output], "physical-control")
        physical = json.loads((output / "physical-rpc-report.json").read_text())
        text = (output / "physical-rpc.stdout.log").read_text()
        if physical["result"] != "passed" or physical["test_binary_sha256"] != test_sha or not all(physical.get(k) for k in
                ("own_test_directory_removed", "private_run_unmounted", "own_daemons_reaped")) or "3 passed; 0 failed" not in text:
            raise RuntimeError("Physical controls or positive fixture retirement did not complete")
        run(prefix + ["bash", "-lc", "cd " + shlex.quote(linux_root) + " && PROTOC=" + shlex.quote(linux_protoc)
                      + " cargo clippy --locked -p cowork-managed-runtime --features containerd --all-targets --target-dir "
                      + shlex.quote(target) + " -- -D warnings"], "linux-feature-clippy")
        # Separate source window for the existing shared broker/service, whose
        # project contracts also receive coordinated work in other chats.
        handoff_before = handoff_sources()
        (output / "handoff-sources-before.json").write_text(json.dumps(handoff_before, indent=2) + "\n")
        run(["cargo", "test", "--locked", "-p", "cowork-runner-protocol", "--lib"], "windows-handoff", 21)
        run(["cargo", "test", "--locked", "-p", "cowork-process-runner", "--bin", "cowork-process-runner"], "windows-process-runner", 11)
        run(cargo("test --locked -p cowork-runner-protocol --lib"), "linux-handoff", 21)
        run(cargo("test --locked -p cowork-process-runner --bin cowork-process-runner"), "linux-process-runner", 6)
        handoff_after = handoff_sources()
        report["handoff_source_drift"] = sorted(n for n in set(handoff_before) | set(handoff_after) if handoff_before.get(n) != handoff_after.get(n))
        if report["handoff_source_drift"]:
            raise RuntimeError("Shared source inputs changed during the separate handoff regression window")
        report["handoff_source_archive_sha256"] = archive(handoff_before, "handoff-source-inputs.zip")
        after = shared.source_inputs(root)
        (output / "sources-after.json").write_text(json.dumps(after, indent=2) + "\n")
        report["source_drift"] = sorted(n for n in set(before) | set(after) if before.get(n) != after.get(n))
        if report["source_drift"]:
            raise RuntimeError("Private control sources changed during qualification")
        report.update(result="passed", physical_tests=3, test_binary_sha256=test_sha, upstream_fixture=fixture,
                      source_count=len(before), handoff_source_count=len(handoff_before),
                      source_manifest_sha256=shared.digest(output / "sources-before.json"),
                      source_archive_sha256=archive(before, "review-source-inputs.zip"))
    except Exception as error:
        report.update(result="failed", failure=str(error))
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"report": str(output / "report.json"), "result": report["result"]}), flush=True)
    return int(report["result"] != "passed")


if __name__ == "__main__":
    raise SystemExit(main())
