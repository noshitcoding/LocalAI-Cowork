"""Qualify local input custody/metadata only, without a model or a workload."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import uuid
import zipfile


def sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def source_inputs(root: Path) -> dict[str, str]:
    paths = [root / name for name in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "package.json", "package-lock.json", "tsconfig.json")]
    for folder in ("crates", "services/daemon", "tools/contracts", "tools/v2_rebuild", "packages/client/src"):
        paths.extend(p for p in (root / folder).rglob("*") if p.is_file() and p.suffix in (".rs", ".sql", ".toml", ".py", ".sh", ".ts", ".mjs", ".json") and "__pycache__" not in p.parts)
    return {p.relative_to(root).as_posix(): sha(p) for p in sorted(set(paths))}


def main() -> int:
    parser = argparse.ArgumentParser(__doc__)
    parser.add_argument("--wsl-distro")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    output = root / ".local/v2-rebuild-validation" / ("execution-inputs-" + uuid.uuid4().hex)
    output.mkdir(parents=True, exist_ok=False)
    before = source_inputs(root)
    checks = []

    def run(command: list[str], name: str, minimum: int = 0) -> None:
        path = output / (name + ".log")
        # WSL's stdout/stderr forwarding can use distinct host file positions
        # when both inherited handles name the same regular file. Capture the
        # two pipes independently, then persist them without overwritten bytes.
        result = subprocess.run(command, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
        (output / (name + ".stdout.log")).write_bytes(result.stdout)
        (output / (name + ".stderr.log")).write_bytes(result.stderr)
        path.write_bytes(result.stdout + result.stderr)
        content = path.read_text(encoding="utf-8", errors="replace")
        count = sum(map(int, re.findall(r"test result: ok\. (\d+) passed;", content)))
        passed = result.returncode == 0 and count >= minimum
        checks.append({"name": name, "command": command, "exit_code": result.returncode, "tests_passed": count, "minimum_tests": minimum, "passed": passed, "log": path.name, "sha256": sha(path)})
        print(json.dumps({"check": name, "passed": passed, "tests": count}), flush=True)
        if not passed:
            raise RuntimeError("qualification failed: " + name)

    failed = None
    artifacts = []
    try:
        run(["cargo", "test", "--locked", "--message-format=json", "-p", "cowork-broker", "-p", "cowork-storage", "-p", "cowork-daemon", "--test", "execution_inputs"], "native-inputs", 10 if os.name == "nt" else 11)
        seen = set()
        for line in (output / "native-inputs.log").read_text(encoding="utf-8", errors="replace").splitlines():
            if not line.startswith("{"):
                continue
            item = json.loads(line)
            if item.get("reason") == "compiler-artifact" and item.get("target", {}).get("name") == "execution_inputs" and item.get("executable"):
                seen.add(item["executable"])
        if len(seen) != 3:
            raise RuntimeError("expected exactly three current input test artifacts")
        for name in sorted(seen):
            source = Path(name)
            target = output / source.name
            shutil.copyfile(source, target)
            artifacts.append({"path": target.name, "sha256": sha(target), "kind": "unsigned_development_test_binary"})
        node = shutil.which("node")
        if not node:
            raise RuntimeError("installed Node.js is required")
        run([node, "tools/contracts/check.mjs"], "contracts")
        run([node, "node_modules/typescript/bin/tsc", "--noEmit"], "typecheck")
        run([node, "node_modules/vitest/vitest.mjs", "run", "packages/client/src/execution-inputs.test.ts", "packages/client/src/workspace-transfer.test.ts", "packages/client/src/execution-plans.test.ts"], "client-inputs")
        client_log = (output / "client-inputs.log").read_text(encoding="utf-8", errors="replace")
        if not re.search(r"Tests\s+11 passed", re.sub(r"\x1b\[[0-9;]*m", "", client_log)):
            raise RuntimeError("the eleven client contracts did not run")
        if args.wsl_distro:
            if os.name != "nt":
                raise RuntimeError("--wsl-distro is a Windows development option")
            prefix = ["wsl", "-d", args.wsl_distro, "--exec"]
            linux_root = subprocess.check_output(prefix + ["wslpath", "-u", str(root)], text=True).strip()
            linux_home = subprocess.check_output(prefix + ["printenv", "HOME"], text=True).strip()
            target = linux_home + "/.cache/localai-cowork-v2/workspace-receive-target"
            command = "cd " + shlex.quote(linux_root) + " && cargo test --locked -p cowork-broker --target-dir " + shlex.quote(target) + " --test execution_inputs --test workspace_receive --test workspaces"
            run(prefix + ["bash", "-lc", command], "linux-broker", 16)
    except Exception as error:
        failed = str(error)
    after = source_inputs(root)
    changed = sorted(p for p in before.keys() | after.keys() if before.get(p) != after.get(p))
    if not changed:
        archive = output / "review-source-inputs.zip"
        with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as bundle:
            for name, expected in before.items():
                data = (root / name).read_bytes()
                if hashlib.sha256(data).hexdigest() != expected:
                    raise RuntimeError("source changed while archiving: " + name)
                bundle.writestr(name, data)
        artifacts.append({"path": archive.name, "sha256": sha(archive), "kind": "review_inputs_requires_full_checkout"})
    report = {"format_version": 1, "scope": "local_execution_input_preparation_custody_and_metadata", "production_accepted": False,
              "input_snapshot_bound": False, "remote_transfer_authorized": False, "model_or_workload_dispatched": False,
              "sources": before, "source_window_stable": not changed, "changed_sources": changed,
              "checks": checks, "artifacts": artifacts, "error": failed, "passed": failed is None and not changed,
              "remaining": ["confirmed_plan_v2_binding", "recipient_transfer_grants", "reserved_runtime_volumes", "queue_snapshot_gate", "working_copy_publication", "reviewed_original_promotion", "product_qualification_and_signing"]}
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"report": str(output / "report.json"), "passed": report["passed"], "changed_sources": changed}), flush=True)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
