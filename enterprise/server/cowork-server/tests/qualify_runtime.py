#!/usr/bin/env python3
"""Qualify one native candidate against a disposable PostgreSQL 16 cluster.

Linux/WSL only; Python standard library only. The cluster, socket directory,
child process groups, and loopback ports belong exclusively to this invocation.
No existing database, Docker service, environment file, or vault is used.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import signal
import socket
import subprocess
import tempfile
import time
from datetime import datetime, timezone
from http.client import IncompleteRead
from urllib.error import HTTPError, URLError
from urllib.request import ProxyHandler, Request, build_opener
import uuid


PG_BIN = Path("/usr/lib/postgresql/16/bin")
API_LOCK_KEY = 0x43574B4150490001
HANDLED_SIGNALS = tuple(getattr(signal, name) for name in ("SIGINT", "SIGTERM", "SIGHUP", "SIGQUIT")
                       if hasattr(signal, name))


class QualificationFailure(Exception):
    """Contains a fixed check identifier, never credentials or child output."""


def require(condition, check):
    if not condition:
        raise QualificationFailure(check)


def clean_environment():
    keep = {"PATH", "HOME", "USER", "LOGNAME", "LANG", "LC_ALL", "LD_LIBRARY_PATH",
            "CARGO_HOME", "RUSTUP_HOME", "CARGO_TARGET_DIR", "RUSTFLAGS"}
    result = {key: value for key, value in os.environ.items() if key in keep}
    result.update({"NO_PROXY": "127.0.0.1,localhost", "RUST_LOG": "cowork_server=info"})
    return result


class Fixture:
    def __init__(self, root, binary):
        self.root = root
        self.binary = binary
        self.data = root / "database"
        self.sockets = root / "sockets"
        self.sockets.mkdir(mode=0o700)
        self.children = []
        self.logs = []
        self.streams = []
        self.last_response_headers = {}
        self.port_sockets = []
        self.allocated_ports = set()
        self.cluster_started = False
        self.environment = clean_environment()
        self.pg_port = self.reserve_port()
        self.api_port = self.reserve_port()
        self.database = "cowork_qualification"
        self.database_url = f"postgres://qualification@127.0.0.1:{self.pg_port}/{self.database}"
        self.bootstrap_secret = secrets.token_urlsafe(48)
        self.admin_password = secrets.token_urlsafe(32)
        self.access_token = None
        self.opener = build_opener(ProxyHandler({}))

    def reserve_port(self):
        while True:
            reservation = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            reservation.bind(("127.0.0.1", 0))
            port = reservation.getsockname()[1]
            if port not in self.allocated_ports:
                self.allocated_ports.add(port)
                self.port_sockets.append((port, reservation))
                return port
            reservation.close()

    def release_port(self, port):
        for candidate, reservation in self.port_sockets:
            if candidate == port:
                reservation.close()

    def command(self, arguments, name, timeout=30, environment=None, cwd=None):
        child = self.spawn(arguments, name, environment=environment, cwd=cwd)
        try:
            result = child.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            self.kill_group(child)
            raise QualificationFailure(f"{name}_timeout") from None
        require(result == 0, f"{name}_exit")
        return child

    def spawn(self, arguments, name, environment=None, cwd=None):
        log = (self.root / f"{name}.log").open("wb")
        self.logs.append(log)
        child = subprocess.Popen(arguments, cwd=cwd or self.root,
                                 env=environment or self.environment,
                                 stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT,
                                 start_new_session=True)
        self.children.append(child)
        return child

    def start_cluster(self):
        self.command([str(PG_BIN / "initdb"), "-D", str(self.data), "-U", "qualification",
                      "-A", "trust", "--no-locale", "--encoding=UTF8"], "initdb")
        self.release_port(self.pg_port)
        self.command([str(PG_BIN / "pg_ctl"), "-D", str(self.data), "-l", str(self.root / "postgres.log"),
                      "-o", f"-h 127.0.0.1 -p {self.pg_port} -k {self.sockets} -c max_connections=50",
                      "-w", "start"], "postgres_start")
        self.cluster_started = True
        self.command([str(PG_BIN / "createdb"), "-h", "127.0.0.1", "-p", str(self.pg_port),
                      "-U", "qualification", self.database], "createdb")

    def sql(self, query):
        result = subprocess.run([str(PG_BIN / "psql"), "-h", "127.0.0.1", "-p", str(self.pg_port),
                                 "-U", "qualification", "-d", self.database, "-X", "-A", "-t",
                                 "-v", "ON_ERROR_STOP=1"], input=query, text=True,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                env=self.environment, timeout=10, check=False)
        require(result.returncode == 0, "isolated_sql_exit")
        return result.stdout.strip()

    def server_environment(self, mode, port):
        environment = self.environment.copy()
        environment.update({
            "DATABASE_URL": self.database_url,
            "COWORK_BOOTSTRAP_TOKEN": self.bootstrap_secret,
            "COWORK_MODE": mode,
            "COWORK_LISTEN_ADDR": f"127.0.0.1:{port}",
            "COWORK_DATABASE_MAX_CONNECTIONS": "8",
            "COWORK_WORKER_ID": str(uuid.uuid4()),
            "COWORK_WORKER_MAX_CONCURRENCY": "2",
            "COWORK_WORKER_POLL_MS": "200",
            "COWORK_MAINTENANCE_EVERY_POLLS": "1",
            "COWORK_LEASE_SECONDS": "30",
            "COWORK_WORKER_SHUTDOWN_GRACE_SECONDS": "3",
            "COWORK_WEB_PUSH_ENABLED": "false",
            "COWORK_CHAT_WORKSPACES_ENABLED": "false",
            "COWORK_FLEET_EXECUTION_ADMISSION": "false",
        })
        return environment

    def start_server(self, mode, name, port=None):
        if port is None:
            port = self.api_port if mode != "worker" else self.reserve_port()
        self.release_port(port)
        child = self.spawn([str(self.binary)], name, self.server_environment(mode, port))
        return child, port

    def http(self, port, path, method="GET", data=None, token=None, accept="application/json", request_id=None):
        headers = {"Accept": accept}
        if request_id is not None:
            headers["X-Request-ID"] = request_id
        if token:
            headers["Authorization"] = f"Bearer {token}"
        if data is not None:
            headers["Content-Type"] = "application/json"
            data = json.dumps(data).encode("utf-8")
        request = Request(f"http://127.0.0.1:{port}{path}", data=data, headers=headers, method=method)
        try:
            response = self.opener.open(request, timeout=3)
        except HTTPError as error:
            response = error
        with response:
            self.last_response_headers = {key.lower(): value for key, value in response.headers.items()}
            return response.status, response.headers.get("Content-Type", ""), response.read()

    def open_sync_sse(self, port, token=None):
        request = Request(f"http://127.0.0.1:{port}/api/v1/sync/events", headers={
            "Authorization": f"Bearer {token or self.access_token}",
            "Accept": "text/event-stream", "Last-Event-ID": "0",
        })
        response = self.opener.open(request, timeout=3)
        self.streams.append(response)
        require(response.status == 200 and response.headers.get("Content-Type", "").startswith("text/event-stream"),
                "authenticated_sync_sse_response")
        return response

    def assert_sse_closed(self, response, name, allow_abrupt=False):
        try:
            response.read()
        except (IncompleteRead, ConnectionResetError):
            require(allow_abrupt, name)
        except (TimeoutError, OSError):
            raise QualificationFailure(name) from None
        finally:
            response.close()

    def new_admin_session(self, port):
        status, _, body = self.http(port, "/api/v1/auth/login", method="POST", data={
            "email": "qualification@localhost.invalid", "password": self.admin_password,
            "device_id": str(uuid.uuid4()), "second_factor": None,
        })
        require(status == 200, "independent_test_session_login")
        return json.loads(body)

    def wait_ready(self, child, port):
        deadline = time.monotonic() + 45
        while time.monotonic() < deadline:
            require(child.poll() is None, "api_exited_before_ready")
            try:
                if self.http(port, "/readyz")[0] == 200:
                    return
            except (URLError, TimeoutError, OSError):
                pass
            time.sleep(0.15)
        raise QualificationFailure("api_ready_timeout")

    def metrics(self, port):
        status, content_type, body = self.http(port, "/api/v1/operations/metrics", token=self.access_token)
        require(status == 200 and content_type.startswith("application/json"), "metrics_json_response")
        return json.loads(body)

    def wait_fresh_groups(self, port, modes):
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            metrics = self.metrics(port)
            groups = metrics["runtime"]["process_groups"]
            found = {group["mode"] for group in groups
                     if group["lifecycle"] == "running" and group["fresh_processes"] >= 1
                     and (group["mode"] not in {"worker", "all"}
                          or all(group[key] is not None and group[key] <= 20 for key in
                                 ("scheduler_age_seconds", "maintenance_age_seconds", "dispatch_age_seconds")))}
            if set(modes) <= found:
                return metrics
            time.sleep(0.2)
        raise QualificationFailure("fresh_runtime_pulses_timeout")

    def stop_server(self, child, name, timeout=35):
        require(child.poll() is None, f"{name}_unexpected_exit")
        child.send_signal(signal.SIGTERM)
        try:
            result = child.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            self.kill_group(child)
            raise QualificationFailure(f"{name}_shutdown_timeout") from None
        require(result == 0, f"{name}_shutdown_exit")

    @staticmethod
    def kill_group(child):
        if child.poll() is None:
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            child.wait(timeout=10)

    def cleanup(self):
        errors = []
        # Once cleanup starts, further catchable termination signals must not
        # interrupt cluster shutdown or leave its files behind.
        saved_handlers = {number: signal.getsignal(number) for number in HANDLED_SIGNALS}
        for number in HANDLED_SIGNALS:
            signal.signal(number, signal.SIG_IGN)
        try:
            for response in self.streams:
                response.close()
            for child in reversed(self.children):
                try:
                    self.kill_group(child)
                except Exception:
                    errors.append("child_process_cleanup")
            # pg_ctl may have started the cluster before returning an error.
            # The exact fixture data directory is the only shutdown target.
            if self.data.exists() and (self.data / "postmaster.pid").exists():
                try:
                    result = subprocess.run([str(PG_BIN / "pg_ctl"), "-D", str(self.data), "-m", "immediate",
                                             "-w", "stop"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                            env=self.environment, timeout=20, check=False)
                    if result.returncode != 0:
                        errors.append("postgres_cleanup_exit")
                except Exception:
                    errors.append("postgres_cleanup_timeout")
        finally:
            for _, reservation in self.port_sockets:
                reservation.close()
            for log in self.logs:
                log.close()
            for number, handler in saved_handlers.items():
                signal.signal(number, handler)
        require(not errors, errors[0] if errors else "cleanup")


def qualify(fixture, cargo_root, report, expected_migration):
    def passed(name):
        report["checks"].append({"name": name, "passed": True})

    fixture.start_cluster()
    passed("fresh_isolated_postgresql16_cluster")
    api, port = fixture.start_server("api", "api_initial")
    fixture.wait_ready(api, port)
    require(fixture.http(port, "/healthz")[0] == 200, "api_health_response")
    generated_request_id = fixture.last_response_headers.get("x-request-id", "")
    require(bool(re.fullmatch(r"[0-9a-fA-F]{8}(?:-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}", generated_request_id)),
            "generated_request_id_response")
    request_id = str(uuid.uuid4())
    require(fixture.http(port, "/healthz", request_id=request_id)[0] == 200
            and fixture.last_response_headers.get("x-request-id") == request_id, "propagated_request_id_response")
    passed("generated_and_propagated_request_id_response_headers")
    require(fixture.sql("SELECT max(version) FROM _sqlx_migrations WHERE success;") == str(expected_migration), "expected_database_migration")
    report["database_migration_version"] = expected_migration
    passed(f"fresh_database_migrations_through_{expected_migration}_and_api_readiness")
    require(fixture.http(port, "/api/v1/operations/metrics")[0] == 401, "metrics_requires_authentication")
    status, _, body = fixture.http(port, "/api/v1/auth/bootstrap/status")
    require(status == 200 and json.loads(body) == {"bootstrap_available": True},
            "public_bootstrap_status_available_before_first_admin")
    require(fixture.last_response_headers.get("cache-control") == "no-store",
            "bootstrap_status_before_first_admin_is_not_cached")
    status, _, body = fixture.http(port, "/api/v1/auth/bootstrap", method="POST", token=fixture.bootstrap_secret,
                                  data={"email": "qualification@localhost.invalid", "display_name": "Qualification",
                                        "password": fixture.admin_password, "device_id": str(uuid.uuid4())})
    require(status == 201, "bootstrap_admin_response")
    fixture.access_token = json.loads(body)["access_token"]
    status, _, body = fixture.http(port, "/api/v1/auth/bootstrap/status")
    require(status == 200 and json.loads(body) == {"bootstrap_available": False},
            "public_bootstrap_status_unavailable_after_first_admin")
    require(fixture.last_response_headers.get("cache-control") == "no-store",
            "bootstrap_status_after_first_admin_is_not_cached")
    passed("public_bootstrap_status_tracks_first_admin_without_caching")
    passed("bootstrap_admin_and_authenticated_metrics")
    logout_session = fixture.new_admin_session(port)
    logout_stream = fixture.open_sync_sse(port, logout_session["access_token"])
    require(fixture.http(port, "/api/v1/auth/logout", method="POST", token=logout_session["access_token"])[0] == 204,
            "secondary_session_logout")
    fixture.assert_sse_closed(logout_stream, "sync_sse_stops_after_logout")
    revoked_session = fixture.new_admin_session(port)
    revoked_stream = fixture.open_sync_sse(port, revoked_session["access_token"])
    require(fixture.http(port, f"/api/v1/auth/sessions/{revoked_session['session_id']}", method="DELETE",
                         token=fixture.access_token)[0] == 204, "secondary_session_revocation")
    fixture.assert_sse_closed(revoked_stream, "sync_sse_stops_after_session_revocation")
    passed("authenticated_last_event_id_sync_sse_closes_after_logout_and_revocation")
    worker, _ = fixture.start_server("worker", "worker_separate")
    metrics = fixture.wait_fresh_groups(port, {"api", "worker"})
    require(metrics["application"]["database_migration_version"] == expected_migration, "metrics_expected_migration")
    require(metrics["runtime"]["api_routing"]["multiple_api_replicas_supported"] is False,
            "single_api_policy_status")
    require(metrics["runtime"]["event_delivery"]["auth_recheck_ms"] == 750, "auth_recheck_status")
    require(metrics["runtime"]["event_delivery"]["replay_fallback_ms"] == 10000, "replay_fallback_status")
    require(not metrics["application"]["runner_configured"] and not metrics["application"]["object_store_configured"],
            "no_runner_or_object_store_credentials_loaded")
    api_groups = [group for group in metrics["runtime"]["process_groups"]
                  if group["mode"] == "api" and group["lifecycle"] == "running"]
    require(all(group["scheduler_age_seconds"] is None for group in api_groups), "api_does_not_advertise_scheduler")
    passed("authenticated_metrics_fresh_api_and_separate_worker_pulses")
    peer_id = str(uuid.uuid4())
    try:
        # A fresh pulse with missing work-loop ticks must remain unhealthy even
        # in the exact same aggregate group as a healthy real worker.
        fixture.sql(f"""
            INSERT INTO server_process_health
                (process_id, process_mode, build_version, migration_version, lifecycle,
                 started_at, last_seen_at, observations)
            SELECT '{peer_id}'::uuid, process_mode, build_version, migration_version, lifecycle,
                now(), now(), jsonb_build_object(
                    'scheduler_tick_ms', 0,
                    'maintenance_tick_ms', (extract(epoch FROM now())*1000)::bigint,
                    'dispatch_tick_ms', (extract(epoch FROM now())*1000)::bigint,
                    'active_runs', 0, 'loop_freshness_ms', 20000)
            FROM server_process_health
            WHERE process_mode='worker' AND lifecycle='running'
            ORDER BY last_seen_at DESC LIMIT 1;
        """)
        require(fixture.sql(f"SELECT count(*) FROM server_process_health WHERE process_id='{peer_id}'::uuid;") == "1",
                "isolated_peer_health_row_created")
        for scheduler_tick in ("0", "(extract(epoch FROM now())*1000)::bigint-60000"):
            fixture.sql(f"""
                UPDATE server_process_health SET last_seen_at=now(), observations=jsonb_build_object(
                    'scheduler_tick_ms', {scheduler_tick},
                    'maintenance_tick_ms', (extract(epoch FROM now())*1000)::bigint,
                    'dispatch_tick_ms', (extract(epoch FROM now())*1000)::bigint,
                    'active_runs', 0, 'loop_freshness_ms', 20000)
                WHERE process_id='{peer_id}'::uuid;
            """)
            peer_metrics = fixture.metrics(port)
            worker_groups = [group for group in peer_metrics["runtime"]["process_groups"]
                             if group["mode"] == "worker" and group["lifecycle"] == "running"]
            require(len(worker_groups) == 1 and worker_groups[0]["fresh_processes"] >= 2,
                    "healthy_and_missing_tick_peer_share_fresh_group")
            group = worker_groups[0]
            require(group["scheduler_age_seconds"] is not None and group["scheduler_age_seconds"] <= 20,
                    "healthy_real_worker_still_ticks")
            require(group["scheduler_unhealthy_processes"] >= 1
                    and group["maintenance_unhealthy_processes"] == 0
                    and group["dispatch_unhealthy_processes"] == 0,
                    "healthy_worker_cannot_mask_missing_or_stalled_peer_scheduler")
    finally:
        fixture.sql(f"DELETE FROM server_process_health WHERE process_id='{peer_id}'::uuid;")
    passed("healthy_worker_cannot_mask_fresh_peer_missing_or_stale_scheduler_ticks")
    status, content_type, body = fixture.http(port, "/api/v1/operations/metrics", token=fixture.access_token,
                                            accept="text/plain; version=0.0.4")
    prometheus = body.decode("utf-8")
    require(status == 200 and content_type.startswith("text/plain"), "prometheus_content_type")
    require(f"cowork_database_migration_version {expected_migration}\n" in prometheus and "cowork_fresh_processes{" in prometheus
            and 'cowork_scheduler_age_seconds{mode="worker"' in prometheus,
            "prometheus_runtime_gauges")
    require(fixture.access_token not in prometheus and fixture.bootstrap_secret not in prometheus,
            "metrics_excludes_fixture_credentials")
    passed("prometheus_negotiation_and_content_free_runtime_gauges")
    owner_stream = fixture.open_sync_sse(port)
    second, _ = fixture.start_server("api", "api_second", fixture.reserve_port())
    try:
        exit_code = second.wait(timeout=15)
    except subprocess.TimeoutExpired:
        raise QualificationFailure("second_api_refusal_timeout") from None
    require(exit_code != 0, "second_api_must_refuse_startup")
    second_log = (fixture.root / "api_second.log").read_text(errors="replace")
    require("another API process" in second_log, "second_api_refusal_reason")
    passed("second_api_process_refuses_shared_database_ownership")
    high, low = API_LOCK_KEY >> 32, API_LOCK_KEY & 0xFFFFFFFF
    owner = fixture.sql(f"SELECT pid FROM pg_locks WHERE locktype='advisory' AND granted AND objsubid=1 "
                        f"AND classid::bigint={high} AND objid::bigint={low} "
                        "AND database=(SELECT oid FROM pg_database WHERE datname=current_database());")
    require(owner.isdecimal(), "exact_isolated_api_lock_backend")
    require(fixture.sql(f"SELECT pg_terminate_backend({int(owner)});") == "t", "api_owner_backend_terminated")
    try:
        exit_code = api.wait(timeout=15)
    except subprocess.TimeoutExpired:
        raise QualificationFailure("lost_api_owner_exit_timeout") from None
    require(exit_code != 0, "lost_api_owner_must_fail_closed")
    fixture.assert_sse_closed(owner_stream, "sync_sse_stops_after_api_ownership_loss", allow_abrupt=True)
    require(worker.poll() is None, "api_owner_loss_keeps_worker_independent")
    passed("database_ownership_loss_stops_api_and_preserves_independent_worker")
    replacement, _ = fixture.start_server("api", "api_replacement")
    fixture.wait_ready(replacement, port)
    fixture.wait_fresh_groups(port, {"api", "worker"})
    passed("replacement_api_acquires_released_ownership_and_reuses_authentication")
    fixture.stop_server(worker, "worker")
    shutdown_stream = fixture.open_sync_sse(port)
    fixture.stop_server(replacement, "api")
    fixture.assert_sse_closed(shutdown_stream, "sync_sse_gracefully_stops_during_api_sigterm")
    require(fixture.sql("SELECT count(*) FROM server_process_health WHERE lifecycle!='stopped';") == "0",
            "separate_processes_persist_stopped_pulses")
    passed("separate_worker_and_api_sigterm_clean_shutdown")
    passed("held_sync_sse_ends_gracefully_on_sigterm_and_fences_on_owner_loss")
    combined, _ = fixture.start_server("all", "all_combined")
    fixture.wait_ready(combined, port)
    fixture.wait_fresh_groups(port, {"all"})
    combined_stream = fixture.open_sync_sse(port)
    fixture.stop_server(combined, "all")
    fixture.assert_sse_closed(combined_stream, "sync_sse_gracefully_stops_during_all_sigterm")
    require(fixture.sql("SELECT count(*) FROM server_process_health WHERE process_mode='all' AND lifecycle='stopped';") == "1",
            "all_mode_persists_stopped_pulse")
    passed("all_mode_sigterm_drains_both_services_without_runs")
    if cargo_root is not None:
        environment = fixture.environment.copy()
        environment["COWORK_TEST_DATABASE_URL"] = fixture.database_url
        fixture.command(["cargo", "test", "--offline", "--locked", "-p", "cowork-server", "postgres_", "--",
                         "--ignored", "--test-threads=1"], "postgres_integration_tests", timeout=300,
                        environment=environment, cwd=cargo_root)
        output = (fixture.root / "postgres_integration_tests.log").read_text(errors="replace")
        results = re.findall(r"test result: ok\. (\d+) passed; (\d+) failed;", output)
        require(bool(results) and all(int(failed) == 0 for _, failed in results)
                and sum(int(count) for count, _ in results) >= 4, "postgres_integration_tests_executed")
        report["postgresql_tests_passed"] = sum(int(count) for count, _ in results)
        passed("real_postgres_notification_replay_refusal_and_lease_fencing_tests")
    else:
        report["limitations"].append("Ignored PostgreSQL integration tests were not requested; pass --cargo-root to run them.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server-bin", required=True, type=Path, help="Already-built Linux cowork-server candidate")
    parser.add_argument("--expected-migration", required=True, type=int, help="Explicit expected schema version for this candidate")
    parser.add_argument("--cargo-root", type=Path, help="Run ignored PostgreSQL Rust tests from this workspace")
    parser.add_argument("--report", required=True, type=Path, help="Write a secret-free JSON evidence report")
    args = parser.parse_args()
    def interrupted(signum, frame):
        # Turn terminal interruption into normal stack unwinding so the owned
        # servers and PostgreSQL cluster are still stopped in finally blocks.
        for number in HANDLED_SIGNALS:
            signal.signal(number, signal.SIG_IGN)
        raise QualificationFailure("qualification_interrupted")
    for number in HANDLED_SIGNALS:
        signal.signal(number, interrupted)
    started = time.monotonic()
    report = {"schema_version": 1, "passed": False, "started_at": datetime.now(timezone.utc).isoformat(),
              "checks": [], "limitations": [
                  "Native Linux candidate qualification; container images and production deployment remain unqualified.",
                  "No real model, tool, runner, or workflow runs were executed; long-run isolation and drain behavior are covered by the Rust supervisor tests.",
                  "Single API ownership is qualified; multiple API replica routing remains unsupported.",
                  "Cleanup handles SIGINT, SIGTERM, SIGHUP and SIGQUIT; SIGKILL or host termination cannot run cleanup handlers.",
              ]}
    try:
        require(os.name == "posix" and os.geteuid() != 0, "linux_nonroot_execution")
        require(args.expected_migration > 0, "positive_expected_migration")
        require((PG_BIN / "initdb").is_file(), "postgres16_binary_available")
        binary = args.server_bin.resolve(strict=True)
        require(binary.is_file() and os.access(binary, os.X_OK), "candidate_binary_executable")
        digest = hashlib.sha256()
        with binary.open("rb") as candidate:
            for chunk in iter(lambda: candidate.read(1024 * 1024), b""):
                digest.update(chunk)
        report["candidate_sha256"] = digest.hexdigest()
        with tempfile.TemporaryDirectory(prefix="cowork-runtime-qualification-", dir="/tmp") as directory:
            fixture = Fixture(Path(directory), binary)
            try:
                qualify(fixture, args.cargo_root.resolve() if args.cargo_root else None, report, args.expected_migration)
            finally:
                fixture.cleanup()
        report["passed"] = True
    except QualificationFailure as error:
        report["failure_check"] = str(error)
    except Exception as error:
        # Exceptions may contain URLs or response bodies. Only their class is
        # exported; all credentials and raw child output stay in disposable files.
        report["failure_kind"] = type(error).__name__
    report["duration_seconds"] = round(time.monotonic() - started, 3)
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, separators=(",", ":")))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
