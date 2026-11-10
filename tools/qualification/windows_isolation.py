"""Run local Windows primitive checks; never restart/install a daemon or claim full sandbox acceptance."""
import hashlib
import json
import os
import pathlib
import platform
import re
import subprocess
import sys
import time
import uuid

if sys.platform != "win32":
    raise SystemExit("Windows qualification requires a Windows host")
root = pathlib.Path(__file__).resolve().parents[2]
destination = root / ".local" / ("windows-isolation-" + uuid.uuid4().hex)
destination.mkdir(parents=True)
env = os.environ.copy()
env["CARGO_TARGET_DIR"] = str(root / ".local/windows-supervisor-build")
sources = [root / name for name in ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml"]]
for folder in ["crates", "services", "tools/contracts", "tools/qualification"]:
    sources.extend(path for path in (root / folder).rglob("*") if path.is_file()
                   and not {"target", "__pycache__", "node_modules"} & set(path.relative_to(root).parts)
                   and path.suffix in {".rs", ".toml", ".sql", ".py", ".mjs", ".json"})
manifest = {path.relative_to(root).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in sorted(set(sources))}
(destination / "sources.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
report = {
    "scope": "Headless Windows isolation primitives and workspace regression; no qualified production sandbox or GUI/Office/shell acceptance",
    "platform": platform.platform(), "processor_count": os.cpu_count(),
    "rust": subprocess.check_output(["rustc", "--version"], text=True).strip(),
    "source_files": len(manifest),
    "source_manifest_sha256": hashlib.sha256((destination / "sources.json").read_bytes()).hexdigest(),
    "result": "running", "steps": [], "observations": {},
    "limitations": ["Disk/scratch quota and output promotion are absent",
                    "Subprocess compatibility and full network enforcement remain unqualified",
                    "No authenticated durable runner or product terminal integration",
                    "A crash before the profile ownership receipt requires manual reconciliation",
                    "Interactive GUI/Office programs are incompatible with mandatory Win32k disablement"],
}

def save():
    (destination / "result.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")

save()
print(str(destination), flush=True)
steps = [
    ("format", ["cargo", "fmt", "--all", "--", "--check"]),
    ("rust-tests", ["cargo", "test", "--locked", "--workspace"]),
    ("clippy", ["cargo", "clippy", "--locked", "--workspace", "--all-targets", "--", "-D", "warnings"]),
    ("isolation-observations", ["cargo", "test", "--locked", "-p", "cowork-executor", "--test", "windows_isolation", "--", "--nocapture"]),
]
for name, command in steps:
    start = time.monotonic()
    result = subprocess.run(command, cwd=root, env=env, capture_output=True, text=True, encoding="utf-8", errors="replace")
    log = result.stdout + result.stderr
    (destination / (name + ".log")).write_text(log, encoding="utf-8")
    report["steps"].append({"name": name, "command": command, "exit_code": result.returncode, "seconds": round(time.monotonic() - start, 2)})
    if name == "rust-tests":
        report["rust_tests_passed"] = sum(map(int, re.findall(r"test result: ok\. (\d+) passed", log)))
    if name == "isolation-observations":
        for key, value in re.findall(r"^(.*?observation): (\{.*\})$", log, re.M):
            report["observations"][key] = json.loads(value)
    print(name + ": " + str(result.returncode), flush=True)
    if result.returncode:
        print(log, flush=True)
        report["result"] = "failed"
        save()
        raise SystemExit(result.returncode)
    save()
for name in ["cowork-executor-fixture.exe", "cowork-isolation-fixture.exe"]:
    report[name + "_sha256"] = hashlib.sha256((pathlib.Path(env["CARGO_TARGET_DIR"]) / "debug" / name).read_bytes()).hexdigest()
assert all(hashlib.sha256((root / path).read_bytes()).hexdigest() == digest for path, digest in manifest.items()), "sources changed during qualification"
report["result"] = "passed"
save()
print(json.dumps(report, indent=2), flush=True)
