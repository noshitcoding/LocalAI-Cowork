"""Exercise sandbox ownership on a private PostgreSQL16 in WSL, using native Windows Cargo.

The database binds only loopback; no existing cluster/configuration is touched.
Only a generated test credential crosses WSLENV, and reports redact it.
"""
from pathlib import Path
import json
import os
import re
import secrets
import socket
import subprocess
import tempfile
import time
import uuid


def main():
    if os.name != "posix" or os.getuid() == 0:
        raise RuntimeError("Run as the ordinary Ubuntu-24.04 WSL user")
    root = Path(__file__).resolve().parents[2]
    pg = Path("/usr/lib/postgresql/16/bin")
    cargo = Path("/mnt/c/Users/Riege/.cargo/bin/cargo.exe")
    if not all((pg / binary).is_file() for binary in ("postgres", "initdb", "pg_ctl", "createdb")) or not cargo.is_file():
        raise RuntimeError("PostgreSQL16 and native Windows Cargo must be installed")
    out = root / ".local" / ("sandbox-postgres-" + uuid.uuid4().hex)
    out.mkdir(parents=True)
    password = secrets.token_urlsafe(36)
    report = {"checks": [], "cluster_stopped": False, "result": "running"}

    def run(command, env, name, timeout=900):
        start = time.monotonic()
        result = subprocess.run(command, cwd=root, env=env, capture_output=True, text=True, errors="replace", timeout=timeout)
        output = (result.stdout + result.stderr).replace(password, "[redacted]")
        (out / (name + ".log")).write_text(output, encoding="utf-8")
        return result.returncode, output, round(time.monotonic() - start, 2)

    with tempfile.TemporaryDirectory(prefix="cowork-sandbox-pg-", dir="/tmp") as directory:
        fixture = Path(directory)
        data = fixture / "data"
        pwfile = fixture / "password"
        pwfile.write_text(password + "\n", encoding="utf-8")
        pwfile.chmod(0o600)
        socket_dir = fixture / "socket"
        socket_dir.mkdir(mode=0o700)
        with socket.socket() as choice:
            choice.bind(("127.0.0.1", 0))
            port = choice.getsockname()[1]
        env = os.environ.copy()
        for key in ("PGSERVICE", "PGSERVICEFILE", "PGPASSFILE", "PGOPTIONS"):
            env.pop(key, None)
        env.update(PGHOST="127.0.0.1", PGPORT=str(port), PGUSER="cowork_fixture", PGPASSWORD=password, PGDATABASE="postgres")
        try:
            code, _, _ = run([str(pg / "initdb"), "-D", str(data), "--username=cowork_fixture", "--pwfile=" + str(pwfile),
                              "--auth-local=trust", "--auth-host=scram-sha-256", "--encoding=UTF8", "--locale=C.UTF-8"], env, "initdb", 45)
            if code:
                raise RuntimeError("Private PostgreSQL initialization failed")
            code, _, _ = run([str(pg / "pg_ctl"), "-D", str(data), "-l", str(fixture / "postgres.log"),
                              "-o", f"-h 127.0.0.1 -p {port} -k {socket_dir}", "-w", "start"], env, "start", 45)
            if code:
                raise RuntimeError("Private PostgreSQL start failed")
            env["WSLENV"] = ":".join(filter(None, [env.get("WSLENV", ""), "COWORK_TEST_POSTGRES"]))
            for label, selection, case in [
                ("sandbox-ownership", ["--lib"], "sandbox_operations::tests::postgres_one_dispatch_and_workspace_ownership"),
                ("environment-contract", ["--test", "environments"], "postgres_environment_contract"),
            ]:
                database = "cowork_" + uuid.uuid4().hex
                code, _, _ = run([str(pg / "createdb"), database], env, "createdb-" + label, 45)
                if code:
                    raise RuntimeError("Private test database creation failed")
                env["COWORK_TEST_POSTGRES"] = f"postgresql://cowork_fixture:{password}@127.0.0.1:{port}/{database}"
                code, output, elapsed = run([str(cargo), "test", "--locked", "-p", "cowork-storage", *selection, case,
                                             "--", "--exact", "--ignored", "--nocapture"], env, label)
                passed = sum(map(int, re.findall(r"test result: ok\. (\d+) passed", output)))
                report["checks"].append({"name": label, "exit_code": code, "passed": passed, "seconds": elapsed})
                print(label + ": " + str(code), flush=True)
                if code or passed != 1:
                    raise RuntimeError("Contract failed: " + label)
            report["result"] = "passed"
        finally:
            if (data / "postmaster.pid").exists():
                run([str(pg / "pg_ctl"), "-D", str(data), "-m", "fast", "-w", "stop"], env, "stop", 45)
            report["cluster_stopped"] = not (data / "postmaster.pid").exists()
            if report["result"] == "running":
                report["result"] = "failed"
            (out / "result.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(str(out), flush=True)


if __name__ == "__main__":
    main()
