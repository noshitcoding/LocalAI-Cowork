"""Reproduce scoped filesystem checks; never labels a runtime profile released."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shlex
import shutil
import subprocess
import sys
import time
import uuid
import zipfile


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def sources(root: Path) -> dict[str, str]:
    paths = [root / "Cargo.toml", root / "Cargo.lock"]
    for name in ("contracts", "domain", "broker"):
        folder = root / "crates" / name
        paths.extend(p for p in folder.rglob("*") if p.is_file() and (p.suffix == ".rs" or p.name == "Cargo.toml"))
    paths.extend(root / "tools/v2_rebuild" / p for p in ("qualify_workspace_receiver.py", "qualify_receiver_volume.sh"))
    return {p.relative_to(root).as_posix(): digest(p) for p in sorted(paths)}


def run(command: list[str], cwd: Path, output: Path, minimum_tests: int = 0, cleanup_required: bool = False) -> dict:
    started = time.monotonic()
    with output.open("wb") as log:
        result = subprocess.run(command, cwd=cwd, stdout=log, stderr=subprocess.STDOUT, check=False)
    content = output.read_text(encoding="utf-8", errors="replace")
    passed = sum(int(value) for value in re.findall(r"test result: ok\. (\d+) passed;", content))
    cleanup_confirmed = "fixture_removed=true; mount_absent=true; loop_detached=true" in content
    accepted = result.returncode == 0 and passed >= minimum_tests and (not cleanup_required or cleanup_confirmed)
    print(f"{output.name}: exit={result.returncode}; passed_tests={passed}; accepted={accepted}", flush=True)
    return {"command": command, "exit_code": result.returncode, "accepted": accepted, "passed_tests": passed,
            "minimum_tests": minimum_tests, "fixture_cleanup_confirmed": cleanup_confirmed if cleanup_required else None,
            "seconds": round(time.monotonic() - started, 3), "log": output.name, "log_sha256": digest(output)}


def main() -> int:
    parser = argparse.ArgumentParser(__doc__)
    parser.add_argument("--wsl-distro", help="Explicit existing development distribution; never imported or reconfigured")
    parser.add_argument("--volume-proof", action="store_true", help="Create/remove an isolated 16-MiB ext4 loop fixture inside that WSL distro")
    args = parser.parse_args()
    if args.volume_proof and not args.wsl_distro:
        parser.error("--volume-proof requires --wsl-distro")
    root = Path(__file__).resolve().parents[2]
    output = root / ".local/v2-rebuild-validation" / ("workspace-receiver-" + uuid.uuid4().hex)
    output.mkdir(parents=True, exist_ok=False)
    before = sources(root)
    checks: list[dict] = []
    target = root / ".local/workspace-receive-target"
    checks.append(run(["cargo", "test", "--locked", "-p", "cowork-broker", "--target-dir", str(target), "--test", "workspace_receive", "--test", "workspace_transfer"], root, output / "native-tests.log", 9 if os.name == "nt" else 10))
    artifacts = []
    if checks[-1]["accepted"]:
        suffix = ".exe" if os.name == "nt" else ""
        for name in ("workspace_receive", "workspace_transfer"):
            candidates = [p for p in (target / "debug/deps").glob(name + "-*" + suffix) if p.is_file() and (p.suffix == ".exe" if os.name == "nt" else p.suffix == "")]
            # Never silently pick an unrelated old test artifact.
            if len(candidates) != 1:
                raise RuntimeError(f"ambiguous artifact for {name}: {len(candidates)}")
            source = candidates[0]
            destination = output / source.name
            shutil.copyfile(source, destination)
            artifacts.append({"path": destination.name, "sha256": digest(destination), "signed": False, "kind": "development_test_binary"})
    if args.wsl_distro:
        distro = args.wsl_distro
        converted = subprocess.check_output(["wsl", "-d", distro, "--exec", "wslpath", "-u", str(root)], text=True).strip()
        linux_home = subprocess.check_output(["wsl", "-d", distro, "--exec", "printenv", "HOME"], text=True).strip()
        if not linux_home.startswith("/") or "\n" in linux_home:
            raise RuntimeError("invalid development home directory")
        linux_target = linux_home + "/.cache/localai-cowork-v2/workspace-receive-target"
        base = ["wsl", "-d", distro, "--exec", "bash", "-lc"]
        cargo = "cargo test --locked -p cowork-broker --target-dir " + shlex.quote(linux_target)
        checks.append(run(base + ["cd " + shlex.quote(converted) + " && " + cargo + " --test workspace_receive --test workspace_transfer"], root, output / "wsl-linux-tests.log", 10))
        if args.volume_proof and checks[-1]["accepted"]:
            checks.append(run(base + ["cd " + shlex.quote(converted) + " && " + cargo + " --test workspace_receive_volume --no-run --message-format=json"], root, output / "wsl-volume-build.log"))
            if checks[-1]["accepted"]:
                lines = (output / "wsl-volume-build.log").read_text(encoding="utf-8", errors="replace").splitlines()
                candidates = []
                for line in lines:
                    if not line.startswith("{"):
                        continue
                    item = json.loads(line)
                    if item.get("reason") == "compiler-artifact" and item.get("target", {}).get("name") == "workspace_receive_volume" and item.get("executable"):
                        candidates.append(item["executable"])
                if len(set(candidates)) != 1:
                    raise RuntimeError("volume test artifact is missing or ambiguous")
                proof_script = converted + "/tools/v2_rebuild/qualify_receiver_volume.sh"
                username = subprocess.check_output(["wsl", "-d", distro, "--exec", "id", "-un"], text=True).strip()
                checks.append(run(["wsl", "-d", distro, "-u", "root", "--exec", "bash", proof_script, username, candidates[0]], root, output / "wsl-bounded-volume.log", 1, True))
    after = sources(root)
    if before == after:
        archive = output / "review-source-inputs.zip"
        with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as bundle:
            for name, expected in before.items():
                data = (root / name).read_bytes()
                if hashlib.sha256(data).hexdigest() != expected:
                    raise RuntimeError(f"source changed while archiving: {name}")
                bundle.writestr(name, data)
        artifacts.append({"path": archive.name, "sha256": digest(archive), "signed": False, "kind": "review_source_inputs_requires_full_checkout"})
    report = {
        "format_version": 1, "scope": "private_workspace_receiver_development_checks",
        "production_accepted": False, "execution_authority": False, "input_snapshot_bound": False,
        "native_platform": sys.platform, "native_os": platform.platform(), "wsl_distro": args.wsl_distro,
        "sources_sha256": before, "sources_stable": before == after,
        "changed_sources": sorted(p for p in before.keys() | after.keys() if before.get(p) != after.get(p)),
        "checks": checks, "artifacts": artifacts,
        "passed": before == after and all(c["accepted"] for c in checks),
        "remaining": ["authenticated_plan_and_recipient_resolution", "input_selection_and_queue_gate", "aggregate_transfer_capacity", "managed_runtime_reserved_volumes_and_lifecycle", "atomic_working_version_publication", "signed_product_packages", "native_linux_hardware_qualification"],
    }
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"report": str(output / "report.json"), "passed": report["passed"]}), flush=True)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
