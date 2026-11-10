"""Physical native receiver ownership proof; no task, clock or profile qualification."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import shlex
import subprocess
import uuid
import zipfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--wsl-distro", required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    spec = importlib.util.spec_from_file_location("receiver_sources", Path(__file__).with_name("qualify.py"))
    shared = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(shared)
    out = root / ".local/v2-rebuild-validation" / ("managed-receiver-claim-" + uuid.uuid4().hex)
    out.mkdir(parents=True)

    def sources():
        values = shared.source_inputs(root)
        for directory in ("crates/domain", "crates/contracts", "crates/workflow-contracts", "crates/runner-protocol", "crates/broker"):
            for path in sorted((root / directory).rglob("*")):
                if path.is_file() and not path.is_symlink() and path.suffix in (".rs", ".toml", ".json"):
                    if path.suffix == ".rs" and path.relative_to(root / directory).parts[0] == "tests":
                        continue
                    values[path.relative_to(root).as_posix()] = shared.digest(path)
        return dict(sorted(values.items()))

    before = sources()
    (out / "sources-before.json").write_text(json.dumps(before, indent=2) + "\n")
    archive = out / "review-source-inputs.zip"
    with zipfile.ZipFile(archive, "x", zipfile.ZIP_DEFLATED) as bundle:
        for name, digest in before.items():
            data = (root / name).read_bytes()
            if hashlib.sha256(data).hexdigest() != digest:
                raise RuntimeError("Input changed before archive")
            bundle.writestr(name, data)
    report = {"scope": __doc__, "result": "running", "checks": [], "workload_tasks_launched": False,
              "runtime_service_integrated": False, "runtime_qualified": False, "profile_released": False,
              "installed": False, "published": False, "complete_C": False, "complete_A_to_I": False}

    def run(command, label):
        result = subprocess.run(command, cwd=root, capture_output=True, check=False)
        (out / (label + ".stdout.log")).write_bytes(result.stdout)
        (out / (label + ".stderr.log")).write_bytes(result.stderr)
        report["checks"].append({"name": label, "exit_code": result.returncode})
        print(json.dumps({"check": label, "passed": result.returncode == 0}), flush=True)
        if result.returncode:
            raise RuntimeError("Check failed: " + label)
        return result.stdout.decode(errors="replace")

    try:
        prefix = ["wsl", "-d", args.wsl_distro, "--exec"]
        def linux(path):
            return subprocess.check_output(prefix + ["wslpath", "-u", str(path)], text=True).strip()
        lr, lo = linux(root), linux(out)
        target = "/home/noah/.cache/localai-cowork-v2/workspace-receive-target"
        protoc = lr + "/.local/managed-runtime-build-tools/33.5/linux-x86_64/bin/protoc"
        command = "cd " + shlex.quote(lr) + " && PROTOC=" + shlex.quote(protoc) + " cargo test --release --locked -p cowork-managed-runtime --features containerd --lib --no-run --message-format=json --target-dir " + shlex.quote(target)
        log = run(prefix + ["bash", "-lc", command], "native-library-build")
        artifacts = [json.loads(line) for line in log.splitlines() if line.startswith('{"reason":"compiler-artifact"')]
        binary = next(v["executable"] for v in artifacts if "lib" in v.get("target", {}).get("kind", []) and v.get("executable"))
        run(prefix + ["/usr/bin/strip", "--strip-debug", "-o", lo + "/receiver-claim-linux", binary], "bounded-test-elf")
        test_sha = shared.digest(out / "receiver-claim-linux")
        run(["wsl", "-d", args.wsl_distro, "-u", "root", "--exec", "python3", lr + "/tools/managed_runtime/volume_fixture.py",
             "--suite", "receiver-claim", "--test-binary", lo + "/receiver-claim-linux", "--expected-test-sha256", test_sha, "--output", lo], "native-receiver-claim")
        physical = json.loads((out / "physical-volume-report.json").read_text())
        if physical["result"] != "passed" or physical["workload_tasks_launched"] or not all(physical.get(k) for k in ("fixture_removed", "mounts_absent", "loops_detached")):
            raise RuntimeError("Physical proof or exact fixture retirement missing")
        text = (out / "volume-tests.stdout.log").read_text()
        if "receiver_claim=verified;" not in text or "published_reservation_retained=true;" not in text:
            raise RuntimeError("Original ownership/reservation evidence missing")
        if "same_receiver_observation=serialized;" not in text or "independent_pool_lock=rejected;" not in text:
            raise RuntimeError("Own-pool serialization or independent native lock exclusion missing")
        after = sources()
        (out / "sources-after.json").write_text(json.dumps(after, indent=2) + "\n")
        report["source_drift"] = sorted(n for n in set(before) | set(after) if before.get(n) != after.get(n))
        if report["source_drift"]:
            raise RuntimeError("Compiler inputs changed during proof")
        with zipfile.ZipFile(archive) as bundle:
            if bundle.testzip() is not None or any(hashlib.sha256(bundle.read(n)).hexdigest() != h for n, h in before.items()):
                raise RuntimeError("Source archive readback failed")
        report.update(result="passed", physical_tests=1, source_count=len(before), source_archive_sha256=shared.digest(archive), test_binary_sha256=test_sha)
    except Exception as error:
        report.update(result="failed", failure=str(error))
    (out / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"report": str(out / "report.json"), "result": report["result"]}), flush=True)
    return int(report["result"] != "passed")


if __name__ == "__main__":
    raise SystemExit(main())
