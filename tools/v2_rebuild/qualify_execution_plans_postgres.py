"""Run the execution-plan contract on a new private PostgreSQL cluster.

Native Linux or WSL with --cargo pointing to native Windows cargo.exe. Never
uses a system cluster, live database, Docker or existing runtime installation.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import secrets
import shutil
import socket
import subprocess
import tempfile
import uuid


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cargo", type=Path)
    parser.add_argument("--suite", choices=("execution-plans", "execution-inputs", "execution-snapshots"), default="execution-plans")
    parser.add_argument("--pg-bin", type=Path, default=Path("/usr/lib/postgresql/16/bin"))
    args = parser.parse_args()
    if os.name != "posix" or os.getuid() == 0:
        raise RuntimeError("Use an ordinary Linux user")
    repo = Path(__file__).resolve().parents[2]
    cargo = args.cargo or Path(shutil.which("cargo") or "/missing-cargo")
    if not cargo.is_file() or not all((args.pg_bin / name).is_file() for name in ("initdb", "pg_ctl", "createdb", "postgres")):
        raise RuntimeError("Installed Cargo and PostgreSQL binaries are required")
    output = repo / ".local" / (args.suite + "-postgres-" + uuid.uuid4().hex)
    output.mkdir(parents=True)
    sources = [repo / name for name in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml")]
    for folder in ("crates", "services/daemon", "tools/v2_rebuild"):
        sources += [path for path in (repo / folder).rglob("*") if path.is_file()
                    and path.suffix in (".rs", ".toml", ".sql", ".py") and "__pycache__" not in path.parts]
    manifest = {str(path.relative_to(repo)): hashlib.sha256(path.read_bytes()).hexdigest() for path in sorted(set(sources))}
    (output / "sources.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    password = secrets.token_urlsafe(36)
    report = {"result": "running", "scope": "Owner " + args.suite + " PostgreSQL custody/concurrency contract only",
              "checks": [], "cluster_stopped": False, "production_accepted": False,
              "limitations": ["No managed-runtime or cross-node qualification", "No real model dispatch or release acceptance"]}
    env = {key: value for key, value in os.environ.items() if not key.startswith(("COWORK_", "PG"))}

    def command(argv, label, timeout=60):
        result = subprocess.run([str(item) for item in argv], cwd=repo, env=env, capture_output=True,
                                text=True, errors="replace", timeout=timeout)
        log = (result.stdout + result.stderr).replace(password, "[redacted]")
        (output / (label + ".log")).write_text(log, encoding="utf-8")
        if result.returncode:
            raise RuntimeError("Private fixture command failed: " + label)
        return log

    print(output, flush=True)
    with tempfile.TemporaryDirectory(prefix="cowork-execution-plan-pg-", dir="/tmp") as temporary:
        fixture = Path(temporary).resolve(strict=True)
        if fixture.parent != Path("/tmp") or not fixture.name.startswith("cowork-execution-plan-pg-") or fixture.stat().st_uid != os.getuid():
            raise RuntimeError("Unexpected private fixture path")
        data = fixture / "data"
        pwfile = fixture / "password"
        pwfile.write_text(password + "\n", encoding="utf-8")
        pwfile.chmod(0o600)
        socketdir = fixture / "socket"
        socketdir.mkdir(mode=0o700)
        with socket.socket() as choice:
            choice.bind(("127.0.0.1", 0))
            port = choice.getsockname()[1]
        env.update(PGHOST="127.0.0.1", PGPORT=str(port), PGUSER="cowork_fixture", PGPASSWORD=password, PGDATABASE="postgres")
        try:
            command([args.pg_bin / "initdb", "-D", data, "--username=cowork_fixture", "--pwfile=" + str(pwfile),
                     "--auth-local=trust", "--auth-host=scram-sha-256", "--encoding=UTF8", "--locale=C.UTF-8"], "initdb")
            command([args.pg_bin / "pg_ctl", "-D", data, "-l", fixture / "server.log", "-o",
                     f"-h 127.0.0.1 -p {port} -k {socketdir}", "-w", "-t", "30", "start"], "start")
            database = ("cowork_execution_inputs_" if args.suite == "execution-inputs" else "cowork_execution_plan_") + uuid.uuid4().hex
            command([args.pg_bin / "createdb", database], "createdb")
            variable = "COWORK_EXECUTION_INPUT_TEST_DB" if args.suite == "execution-inputs" else "COWORK_TEST_EXECUTION_PLAN_POSTGRES"
            env[variable] = f"postgresql://cowork_fixture:{password}@127.0.0.1:{port}/{database}"
            if cargo.suffix.lower() == ".exe":
                env["WSLENV"] = variable
            if args.suite == "execution-inputs":
                log = command([cargo, "test", "--locked", "-p", "cowork-storage", "--test", "execution_inputs", "--", "--test-threads=1"], "execution-inputs", timeout=900)
                if "test result: ok. 3 passed;" not in log:
                    raise RuntimeError("The three input custody contracts did not run")
                report["checks"] = ["16 concurrent preparations keep one original input receipt",
                                    "retirement, reopen and order changes preserve that original receipt",
                                    "stale order, conflicting key and corrupted custody are rejected",
                                    "no run or process is created by input custody"]
            else:
                snapshots = args.suite == "execution-snapshots"
                case = "postgres_execution_snapshots_serialize_confirmation_and_retirement" if snapshots else "postgres_execution_plans_serialize_confirmation_and_retirement"
                target = "execution_snapshots" if snapshots else "execution_plans"
                log = command([cargo, "test", "--locked", "-p", "cowork-runtime", "--test", target, case,
                               "--", "--exact", "--ignored", "--nocapture"], args.suite, timeout=900)
                if "test result: ok. 1 passed;" not in log:
                    raise RuntimeError("The selected contract did not run")
                report["checks"] = ["16 simultaneous confirmations return the original receipt",
                                "12 confirmation/retirement races exclude double success",
                                "retired plans cannot start", "accepted receipt survives cancellation and provider removal"]
            drift = [name for name, digest in manifest.items() if hashlib.sha256((repo / name).read_bytes()).hexdigest() != digest]
            if drift:
                report["source_drift"] = drift
                raise RuntimeError("Sources changed during qualification")
            report["result"] = "passed"
        finally:
            if (data / "postmaster.pid").exists():
                command([args.pg_bin / "pg_ctl", "-D", data, "-m", "fast", "-w", "-t", "30", "stop"], "stop")
            report["cluster_stopped"] = not (data / "postmaster.pid").exists()
            if report["result"] == "running":
                report["result"] = "failed"
            report["source_manifest_sha256"] = hashlib.sha256((output / "sources.json").read_bytes()).hexdigest()
            (output / "result.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    report["cluster_fixture_removed"] = not fixture.exists()
    if not report["cluster_fixture_removed"]:
        report["result"] = "failed"
    (output / "result.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report), flush=True)


if __name__ == "__main__":
    main()
