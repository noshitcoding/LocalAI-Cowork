"""Source-bound development qualification of the process-free snapshot adapter.

Never installs an app, starts a workload runtime, publishes or alters a live DB.
Platform/runtime/signing/M10 acceptance is deliberately outside this scope.
"""
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
    excluded = {".git", ".local", ".reference", "archiv", "node_modules", "target", "dist", "__pycache__"}
    extensions = {".rs", ".sql", ".toml", ".py", ".sh", ".ts", ".tsx", ".css", ".js", ".mjs", ".json", ".html", ".svg"}
    for folder in ("crates", "services/daemon", "tools/contracts", "tools/v2_rebuild", "packages/client", "packages/ui", "apps/web", "contracts"):
        for base, dirs, files in os.walk(root / folder, followlinks=False):
            dirs[:] = sorted(d for d in dirs if d not in excluded and not (Path(base) / d).is_symlink())
            paths.extend(Path(base) / name for name in files if Path(name).suffix in extensions)
    return {p.relative_to(root).as_posix(): sha(p) for p in sorted(set(paths)) if not p.is_symlink()}


def main() -> int:
    parser = argparse.ArgumentParser(__doc__)
    parser.add_argument("--wsl-distro")
    parser.add_argument("--postgres-report", type=Path)
    parser.add_argument("--local-ollama", action="store_true", help="Opt into synthetic-file qualification using already installed loopback llama3.1:8b; never pulls models or manages the service")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    output = root / ".local/v2-rebuild-validation" / ("execution-snapshots-" + uuid.uuid4().hex)
    output.mkdir(parents=True, exist_ok=False)
    before = source_inputs(root)
    (output / "sources-before.json").write_text(json.dumps(before, indent=2) + "\n", encoding="utf-8")
    env = {k: v for k, v in os.environ.items() if not k.startswith(("COWORK_", "PG"))}
    checks, artifacts = [], []
    report = {"scope": "Confirmed owner snapshots and process-free private working copies only", "result": "running", "checks": checks,
              "production_accepted": False, "runtime_qualified": False, "m10_accepted": False,
              "limitations": ["No managed WSL2/containerd/gVisor program dispatch", "No team/device/remote input transfer admission",
                              "No automatic adoption into original files", "No hard runtime filesystem volume qualification",
                              "Synthetic HTTP fixtures, plus only the installed local llama3.1:8b when explicitly selected; no general external provider qualification", "No installer, signing, NVDA/WCAG or full roadmap acceptance"]}

    def run(argv: list[str], name: str, minimum: int = 0, extra_env: dict[str, str] | None = None) -> str:
        print(json.dumps({"check": name, "state": "running"}), flush=True)
        # Distinct pipes avoid WSL inherited regular-file stdout/stderr offsets.
        result = subprocess.run(argv, cwd=root, env=env | (extra_env or {}), stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
        (output / (name + ".stdout.log")).write_bytes(result.stdout)
        (output / (name + ".stderr.log")).write_bytes(result.stderr)
        log = output / (name + ".log")
        log.write_bytes(result.stdout + result.stderr)
        content = log.read_text(encoding="utf-8", errors="replace")
        tests = sum(map(int, re.findall(r"test result: ok\. (\d+) passed;", content)))
        passed = result.returncode == 0 and tests >= minimum
        checks.append({"name": name, "command": argv, "exit_code": result.returncode, "tests": tests, "passed": passed, "log": log.name, "sha256": sha(log)})
        print(json.dumps({"check": name, "passed": passed, "tests": tests}), flush=True)
        if not passed:
            raise RuntimeError("qualification failed: " + name)
        return content

    def artifact(path: Path, relative: str, kind: str) -> None:
        destination = output / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(path, destination)
        if sha(path) != sha(destination):
            raise RuntimeError("artifact changed while being copied")
        artifacts.append({"path": relative, "sha256": sha(destination), "kind": kind, "signed": False})

    failure = None
    try:
        run(["cargo", "test", "--locked", "-p", "cowork-workflow-contracts", "--lib", "public_plan_bytes"], "decimal-codec", 1)
        cases = run(["cargo", "test", "--locked", "--message-format=json", "-p", "cowork-broker", "--test", "run_workspaces"], "native-broker", 10)
        cases += run(["cargo", "test", "--locked", "--message-format=json", "-p", "cowork-runtime", "-p", "cowork-daemon", "--test", "execution_snapshots", "--", "--test-threads=2"], "native-admission", 12)
        if args.local_ollama:
            local = run(["cargo", "test", "--locked", "-p", "cowork-runtime", "--test", "execution_snapshots",
                         "ollama_execution_snapshot_reads_frozen_input_and_retains_its_output", "--", "--exact", "--ignored", "--nocapture"],
                        "local-ollama", 1, {"COWORK_TEST_LOCAL_OLLAMA": "1"})
            receipts = [json.loads(line) for line in local.splitlines() if line.startswith('{"digest":')]
            if len(receipts) != 1 or receipts[0].get("provider") != "local_ollama":
                raise RuntimeError("one source-bound local model receipt is required")
            report["local_ollama"] = receipts[0]
        binaries = set()
        for line in cases.splitlines():
            if line.startswith("{"):
                item = json.loads(line)
                if item.get("reason") == "compiler-artifact" and item.get("executable") and item.get("target", {}).get("name") in ("run_workspaces", "execution_snapshots"):
                    binaries.add(item["executable"])
        if len(binaries) != 3:
            raise RuntimeError("three exact native test executables are required")
        for binary in sorted(binaries):
            artifact(Path(binary), "tests/" + Path(binary).name, "development_test_binary")
        node = shutil.which("node")
        if not node:
            raise RuntimeError("Node.js is required")
        run([node, "tools/contracts/check.mjs"], "contracts")
        run([node, "node_modules/typescript/bin/tsc", "--noEmit"], "typecheck")
        ui = run([node, "node_modules/vitest/vitest.mjs", "run", "--reporter=dot"], "frontend")
        if not re.search(r"Tests\s+(\d+) passed", re.sub(r"\x1b\[[0-9;]*m", "", ui)):
            raise RuntimeError("frontend test count is missing")
        run(["cmd", "/c", "npm", "run", "build"] if os.name == "nt" else ["npm", "run", "build"], "web-build")
        if args.wsl_distro:
            prefix = ["wsl", "-d", args.wsl_distro, "--exec"]
            linux_root = subprocess.check_output(prefix + ["wslpath", "-u", str(root)], text=True).strip()
            linux_home = subprocess.check_output(prefix + ["printenv", "HOME"], text=True).strip()
            command = "cd " + shlex.quote(linux_root) + " && cargo test --locked -p cowork-broker --target-dir " + shlex.quote(linux_home + "/.cache/localai-cowork-v2/workspace-receive-target") + " --test run_workspaces --test execution_inputs --test workspace_receive --test workspaces"
            run(prefix + ["bash", "-lc", command], "linux-broker", 26)
        if args.postgres_report:
            path = args.postgres_report.resolve(strict=True)
            pg = json.loads(path.read_text(encoding="utf-8"))
            pg_sources = json.loads(path.with_name("sources.json").read_text(encoding="utf-8"))
            drift = [name for name, digest in pg_sources.items() if not (root / name).is_file() or sha(root / name) != digest]
            report["postgres"] = {"report": str(path), "sha256": sha(path), "source_drift": drift,
                                  "passed": pg.get("result") == "passed" and pg.get("cluster_stopped") and pg.get("cluster_fixture_removed") and not drift}
            if not report["postgres"]["passed"]:
                raise RuntimeError("PostgreSQL source binding is no longer current")
        # The same current checks build the native sidecar; no installed app is replaced.
        executable = root / "target/debug" / ("cowork-daemon.exe" if os.name == "nt" else "cowork-daemon")
        artifact(executable, "bin/" + executable.name, "unsigned_development_daemon")
        for path in sorted((root / "apps/web/dist").rglob("*")):
            if path.is_file():
                artifact(path, "web/" + path.relative_to(root / "apps/web/dist").as_posix(), "built_frontend")
        after = source_inputs(root)
        (output / "sources-after.json").write_text(json.dumps(after, indent=2) + "\n", encoding="utf-8")
        drift = sorted(name for name in set(before) | set(after) if before.get(name) != after.get(name))
        report["source_drift"] = drift
        report["source_count"] = len(before)
        if drift:
            raise RuntimeError("sources changed during qualification")
        archive = output / "review-source-inputs.zip"
        with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as bundle:
            for name, digest in before.items():
                content=(root / name).read_bytes()
                if hashlib.sha256(content).hexdigest()!=digest:
                    raise RuntimeError("source changed while archiving: "+name)
                bundle.writestr(name,content)
        with zipfile.ZipFile(archive) as bundle:
            if bundle.testzip() is not None or any(hashlib.sha256(bundle.read(name)).hexdigest()!=digest for name,digest in before.items()):
                raise RuntimeError("source archive verification failed")
        artifacts.append({"path": archive.name, "sha256": sha(archive), "kind": "review_source_inputs_requires_full_checkout", "signed": False})
        report["result"] = "passed"
    except Exception as error:
        failure = str(error)
        report["result"] = "failed"
        report["failure"] = failure
    finally:
        report["artifacts"] = artifacts
        report["source_manifest_sha256"] = sha(output / "sources-before.json")
        (output / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        print(json.dumps({"report": str(output / "report.json"), "result": report["result"], "failure": failure}), flush=True)
    return int(failure is not None)


if __name__ == "__main__":
    raise SystemExit(main())
