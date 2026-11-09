#!/usr/bin/env python3
"""Native API/worker runner-wait acceptance with disposable PG16 and fake HTTP.

Linux/WSL, non-root, Python standard library only. The fake runner verifies the
real worker's request HMAC and emits request-bound NoStart proofs; it never
starts Docker containers or executes tools. An explicitly approved binary SHA
is required before any fixture service starts.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import hmac
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import secrets
import signal
import tempfile
import threading
import time
import uuid

from qualify_runtime import Fixture, HANDLED_SIGNALS, PG_BIN, QualificationFailure, require


NO_START_REASON = "sandbox_capacity_exhausted"
NO_START_DOMAIN = b"open-cowork/runner-no-start/v1\0"


def no_start_proof(key, method, path, timestamp, nonce, body):
    mac = hmac.new(key, NO_START_DOMAIN, hashlib.sha256)
    for field in [method, path, timestamp, nonce, hashlib.sha256(body).hexdigest(),
                  "429", NO_START_REASON]:
        encoded = field.encode("utf-8")
        mac.update(len(encoded).to_bytes(8, "big"))
        mac.update(encoded)
    return "v1:" + mac.hexdigest()


def request_signature(key, method, path, timestamp, nonce, body):
    signed = "\n".join([timestamp, nonce, method, path]).encode() + b"\n" + body
    return hmac.new(key, signed, hashlib.sha256).hexdigest()


class Scenario:
    def __init__(self, mode):
        self.mode = mode
        self.key = str(uuid.uuid4())
        self.records = []
        self.success_release = threading.Event()


class PrivateFakeRunner:
    """Only an owned loopback port and in-memory requests; no external I/O."""
    def __init__(self, port, signing_key):
        self.signing_key = signing_key
        self.scenarios = {}
        self.errors = []
        self.nonces = set()
        self.lock = threading.Lock()
        self.stopping = threading.Event()
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def setup(self):
                super().setup()
                self.connection.settimeout(5)

            def log_message(self, format, *arguments):
                pass

            def do_POST(self):
                try:
                    status, body, proof = owner.handle(self)
                except QualificationFailure as error:
                    with owner.lock:
                        owner.errors.append(str(error))
                    status, body, proof = 500, {"error": "private_fixture_request_rejected"}, None
                except Exception:
                    with owner.lock:
                        owner.errors.append("private_fake_runner_handler_error")
                    status, body, proof = 500, {"error": "private_fixture_request_rejected"}, None
                encoded = json.dumps(body).encode()
                try:
                    self.send_response(status)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Content-Length", str(len(encoded)))
                    if proof is not None:
                        self.send_header("x-cowork-no-start-proof", proof)
                    self.end_headers()
                    self.wfile.write(encoded)
                except (BrokenPipeError, ConnectionResetError):
                    pass

        class Server(ThreadingHTTPServer):
            daemon_threads = False

            def handle_error(self, request, client_address):
                with owner.lock:
                    owner.errors.append("private_fake_runner_transport_error")

        self.server = Server(("127.0.0.1", port), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever,
                                       kwargs={"poll_interval": .1}, name="private-fake-runner")
        self.thread.start()

    def add(self, mode):
        scenario = Scenario(mode)
        with self.lock:
            self.scenarios[scenario.key] = scenario
        return scenario

    def records(self, scenario):
        with self.lock:
            return list(scenario.records)

    def assert_healthy(self):
        with self.lock:
            require(not self.errors, self.errors[0] if self.errors else "private_fake_runner_health")

    def handle(self, handler):
        require(handler.path == "/v1/jobs", "private_runner_expected_legacy_endpoint")
        length = handler.headers.get("Content-Length", "")
        require(length.isdecimal() and 0 < int(length) <= 1024 * 1024, "private_runner_bounded_body")
        body = handler.rfile.read(int(length))
        require(len(body) == int(length), "private_runner_complete_body")
        timestamp, nonce = (handler.headers.get(name, "")
                            for name in ["x-cowork-timestamp", "x-cowork-nonce"])
        require(timestamp.isdecimal() and abs(time.time() - int(timestamp)) < 60,
                "private_runner_current_request_timestamp")
        require(str(uuid.UUID(nonce)) == nonce, "private_runner_canonical_request_nonce")
        expected = request_signature(self.signing_key, "POST", handler.path, timestamp, nonce, body)
        require(hmac.compare_digest(handler.headers.get("x-cowork-signature", ""), expected),
                "private_runner_authentic_request_hmac")
        spec = json.loads(body)
        with self.lock:
            require(nonce not in self.nonces, "private_runner_fresh_request_nonce")
            self.nonces.add(nonce)
            scenario = self.scenarios.get(spec["argv"][-1])
            require(scenario is not None, "private_runner_owned_scenario")
            scenario.records.append({"body": body, "nonce": nonce, "timestamp": timestamp,
                                     "run_id": spec["run_id"], "path": handler.path})
            count = len(scenario.records)
        if scenario.mode == "success" and count >= 3:
            deadline = time.monotonic() + 20
            while not scenario.success_release.wait(.1):
                if self.stopping.is_set() or time.monotonic() >= deadline:
                    raise QualificationFailure("private_runner_success_gate_timeout")
            return 200, {"schema_version": spec["schema_version"], "run_id": spec["run_id"],
                         "container_name": "private-fake-container", "workspace_volume": "private-fake-volume",
                         "exit_code": 0, "timed_out": False, "stdout": "private-fake-success",
                         "stderr": "", "output_truncated": False}, None
        proof = no_start_proof(self.signing_key, "POST", handler.path, timestamp, nonce, body)
        if scenario.mode == "generic":
            proof = None
        elif scenario.mode == "false_proof":
            proof = no_start_proof(self.signing_key, "POST", handler.path, timestamp,
                                   "different-request-" + nonce, body)
        return 429, {"error": NO_START_REASON, "message": "private fixture capacity refusal"}, proof

    def close(self):
        self.stopping.set()
        with self.lock:
            for scenario in self.scenarios.values():
                scenario.success_release.set()
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)
        require(not self.thread.is_alive(), "private_fake_runner_shutdown")


class RunnerWaitFixture(Fixture):
    def __init__(self, root, binary):
        super().__init__(root, binary)
        self.fake_runner = None
        self.runner_port = self.reserve_port()
        self.runner_secret_file = root / "private-runner-secret"
        self.runner_secret = secrets.token_hex(32).encode("ascii")
        descriptor = os.open(self.runner_secret_file, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(descriptor, "wb") as secret:
            secret.write(self.runner_secret)
        self.api = None
        self.worker = None
        self.owned_runs = {}

    def start_fake_runner(self):
        self.release_port(self.runner_port)
        self.fake_runner = PrivateFakeRunner(self.runner_port, self.runner_secret)

    def server_environment(self, mode, port):
        environment = super().server_environment(mode, port)
        environment.update({"COWORK_RUNNER_URL": f"http://127.0.0.1:{self.runner_port}",
                            "COWORK_RUNNER_SIGNING_KEY_FILE": str(self.runner_secret_file),
                            "COWORK_SERVER_CAPABILITIES": "shell",
                            "COWORK_LEASE_SECONDS": "3", "COWORK_WORKER_MAX_CONCURRENCY": "1"})
        return environment

    def cleanup(self):
        saved = {number: signal.getsignal(number) for number in HANDLED_SIGNALS}
        for number in HANDLED_SIGNALS:
            signal.signal(number, signal.SIG_IGN)
        try:
            try:
                super().cleanup()
            finally:
                if self.fake_runner is not None:
                    self.fake_runner.close()
                self.runner_secret_file.unlink(missing_ok=True)
        finally:
            for number, handler in saved.items():
                signal.signal(number, handler)

    def ensure_services(self):
        require(self.api.poll() is None and self.worker.poll() is None, "owned_api_and_worker_still_running")
        self.fake_runner.assert_healthy()

    def api_json(self, path, *, expected=200, method="GET", data=None):
        status, _, body = self.http(self.api_port, "/api/v1" + path, method=method,
                                    data=data, token=self.access_token)
        require(status == expected, "authenticated_fixture_api_response")
        return json.loads(body) if body else None

    def until(self, condition, name, seconds=15):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            self.ensure_services()
            result = condition()
            if result:
                return result
            time.sleep(.05)
        raise QualificationFailure(name)

    def snapshot(self, run_id):
        require(str(uuid.UUID(run_id)) == run_id, "canonical_owned_run_id")
        return json.loads(self.sql(f"""
            SELECT jsonb_build_object('state',r.state,'error_code',r.error->>'code','owner',r.lease_owner,'token',r.lease_token,
                'expires_ms',(extract(epoch FROM r.lease_expires_at)*1000)::bigint,
                'reason',w.reason,'waiting_owner',w.lease_owner,'waiting_token',w.lease_token,
                'checkpoint',to_jsonb(c),'checkpoint_event',e.payload,
                'tool_started_count',(SELECT count(*) FROM run_events WHERE run_id=r.id AND kind='tool_started'))
            FROM runs r LEFT JOIN run_waiting_reasons w ON w.run_id=r.id
            LEFT JOIN LATERAL (SELECT * FROM run_checkpoints WHERE run_id=r.id ORDER BY sequence DESC LIMIT 1) c ON true
            LEFT JOIN LATERAL (SELECT payload FROM run_events WHERE run_id=r.id AND kind='checkpoint_created'
                AND payload->>'checkpoint_id'=c.id::text LIMIT 1) e ON true
            WHERE r.id='{run_id}'::uuid;
        """))

    def diagnostics(self):
        """Export only owned request counts and fixed state/phase codes."""
        phases = {"sandbox_dispatched", "sandbox_waiting_capacity", "sandbox_completed"}
        error_codes = {"unsafe_tool_interrupted", "executor_operation_failed", "executor_lease_expired", "worker_shutdown"}
        cases = []
        with self.fake_runner.lock:
            scenarios = list(self.fake_runner.scenarios.values())
            auth_errors = list(self.fake_runner.errors)
        for scenario in scenarios:
            run_id = self.owned_runs.get(scenario.key)
            row = self.snapshot(run_id) if run_id else {}
            checkpoint = row.get("checkpoint") or {}
            phase = checkpoint.get("executor_state", {}).get("phase")
            cases.append({"case": scenario.mode, "request_count": len(self.fake_runner.records(scenario)),
                          "run_state": row.get("state"), "waiting_reason": row.get("reason"),
                          "checkpoint_phase": phase if phase in phases else None,
                          "safe_to_resume": checkpoint.get("safe_to_resume"),
                          "error_code": row.get("error_code") if row.get("error_code") in error_codes else None})
        return {"cases": cases, "runner_auth_error_codes": auth_errors,
                "api_alive": self.api is not None and self.api.poll() is None,
                "worker_alive": self.worker is not None and self.worker.poll() is None}

    def waiting_snapshot(self, run_id):
        def waiting():
            dto = self.api_json(f"/runs/{run_id}/waiting")
            row = self.snapshot(run_id)
            if dto.get("reason") != "runner_capacity" or row["reason"] != "runner_capacity":
                return None
            require(set(dto) == {"run_id", "reason", "checked_at"} and dto["checked_at"],
                    "waiting_dto_content_free")
            checkpoint, event = row["checkpoint"], row["checkpoint_event"]
            require(row["state"] == "running" and row["owner"] and row["token"]
                    and row["waiting_owner"] == row["owner"] and row["waiting_token"] == row["token"],
                    "capacity_wait_owned_by_current_running_lease")
            require(isinstance(checkpoint, dict) and isinstance(event, dict)
                    and checkpoint["safe_to_resume"] is True
                    and checkpoint["executor_state"] == {"phase": "sandbox_waiting_capacity", "reason": "runner_capacity"}
                    and event["safe_to_resume"] is True and event["checkpoint_id"] == checkpoint["id"]
                    and event["sequence"] == checkpoint["sequence"], "capacity_wait_atomic_safe_checkpoint_and_event")
            return row
        return self.until(waiting, "authenticated_capacity_wait_timeout")

    def terminal(self, run_id):
        def done():
            row = self.api_json(f"/runs/{run_id}")
            return row if row["state"] in {"completed", "failed", "canceled", "interrupted", "expired"} else None
        return self.until(done, "owned_run_terminal_timeout")

    def assert_no_more_requests(self, scenario, expected, seconds=4):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            self.ensure_services()
            require(len(self.fake_runner.records(scenario)) == expected, "refused_or_fenced_run_was_retried")
            time.sleep(.1)

    def create_run(self, project, scenario):
        thread = self.api_json("/threads", expected=201, method="POST",
                               data={"project_id": project["id"], "title": "Private runner-wait fixture"})
        request = {"project_id": project["id"], "thread_id": thread["id"],
                   "project_revision": project["revision"], "project_privacy": "team_managed",
                   "executor_target": {"kind": "server_linux", "pool_id": None},
                   "required_capabilities": ["shell"], "idempotency_key": str(uuid.uuid4()),
                   "input": {"sandbox": {"schema_version": 2, "run_id": str(uuid.uuid4()),
                       "image": "core", "argv": ["private-fixture-never-executed", scenario.key], "network": "none",
                       "limits": {"cpu_nanos": 100000000, "memory_bytes": 128 * 1024 * 1024,
                           "pids": 64, "timeout_seconds": 20, "tmpfs_bytes": 4 * 1024 * 1024, "output_bytes": 4096}}}}
        row = self.api_json("/runs", expected=201, method="POST", data=request)
        run_id = row["spec"]["id"]
        self.owned_runs[scenario.key] = run_id
        require(row["state"] in {"queued", "running"}, "private_shell_fixture_admitted_to_worker")
        return run_id


def qualify(fixture, report, expected_migration):
    def passed(name):
        report["checks"].append({"name": name, "passed": True})

    fixture.start_cluster()
    fixture.start_fake_runner()
    fixture.api, port = fixture.start_server("api", "api")
    fixture.wait_ready(fixture.api, port)
    require(fixture.sql("SELECT max(version) FROM _sqlx_migrations WHERE success;") == str(expected_migration),
            "runner_wait_expected_database_migration")
    report["database_migration_version"] = expected_migration
    status, _, body = fixture.http(port, "/api/v1/auth/bootstrap", method="POST", token=fixture.bootstrap_secret,
        data={"email": "qualification@localhost.invalid", "display_name": "Private runner fixture",
              "password": fixture.admin_password, "device_id": str(uuid.uuid4())})
    require(status == 201, "runner_wait_bootstrap")
    fixture.access_token = json.loads(body)["access_token"]
    team = fixture.api_json("/teams", expected=201, method="POST", data={"name": "Private runner fixture"})
    project = fixture.api_json("/projects", expected=201, method="POST",
                               data={"name": "Private runner fixture", "privacy": "team_managed", "team_id": team["id"]})
    fixture.worker, _ = fixture.start_server("worker", "worker")
    metrics = fixture.wait_fresh_groups(port, {"api", "worker"})
    require(metrics["application"]["runner_configured"] and not metrics["application"]["object_store_configured"],
            "only_private_runner_configured")
    passed("fresh_pg16_authenticated_api_and_separate_worker_with_private_fake_runner")

    success = fixture.fake_runner.add("success")
    run_id = fixture.create_run(project, success)
    require(fixture.http(port, f"/api/v1/runs/{run_id}/waiting")[0] == 401, "waiting_requires_authentication")
    first = fixture.waiting_snapshot(run_id)
    fixture.until(lambda: len(fixture.fake_runner.records(success)) == 3, "same_lease_third_request_timeout")
    records = fixture.fake_runner.records(success)
    dispatched = fixture.snapshot(run_id)
    require(all(record["body"] == records[0]["body"] and record["run_id"] == run_id for record in records)
            and len({record["nonce"] for record in records}) == 3, "retry_frozen_body_and_fresh_nonces")
    require(dispatched["owner"] == first["owner"] and dispatched["token"] == first["token"]
            and dispatched["expires_ms"] > first["expires_ms"], "same_lease_heartbeat_during_capacity_wait")
    require(dispatched["state"] == "running" and dispatched["reason"] is None
            and dispatched["checkpoint"]["safe_to_resume"] is False
            and dispatched["checkpoint"]["executor_state"]["phase"] == "sandbox_dispatched"
            and dispatched["checkpoint_event"]["safe_to_resume"] is False
            and dispatched["tool_started_count"] == 1, "waiting_cleared_and_dispatch_fenced_before_retry")
    require(fixture.api_json(f"/runs/{run_id}/waiting")["reason"] is None, "waiting_clear_visible_during_start")
    success.success_release.set()
    completed = fixture.terminal(run_id)
    require(completed["state"] == "completed" and completed["result"]["sandbox"]["stdout"] == "private-fake-success",
            "successful_capacity_retry_completes_normally")
    require(fixture.snapshot(run_id)["reason"] is None
            and fixture.api_json(f"/runs/{run_id}/waiting")["reason"] is None, "completed_run_waiting_cleared")
    passed("authentic_no_start_atomic_wait_same_renewed_lease_frozen_retry_and_normal_completion")

    for mode in ["cancel", "lease_loss"]:
        scenario = fixture.fake_runner.add(mode)
        run_id = fixture.create_run(project, scenario)
        row = fixture.waiting_snapshot(run_id)
        require(len(fixture.fake_runner.records(scenario)) == 1, "capacity_fence_precedes_second_request")
        if mode == "cancel":
            canceled = fixture.api_json(f"/runs/{run_id}/cancel", method="POST")
            require(canceled["state"] == "canceled", "capacity_wait_cancellation_response")
        else:
            changed = fixture.sql(f"WITH changed AS (UPDATE runs SET lease_expires_at=clock_timestamp()-interval '1 second' "
                f"WHERE id='{run_id}'::uuid AND state='running' AND lease_owner='{row['owner']}'::uuid "
                f"AND lease_token='{row['token']}'::uuid RETURNING id) SELECT id FROM changed;")
            require(changed == run_id, "only_owned_capacity_wait_lease_expired")
        terminal = fixture.terminal(run_id)
        require(terminal["state"] == ("canceled" if mode == "cancel" else "interrupted"),
                "capacity_wait_cancellation_or_expiry_terminal")
        fixture.assert_no_more_requests(scenario, 1)
        final = fixture.snapshot(run_id)
        require(final["reason"] is None and final["owner"] is None and final["token"] is None
                and final["checkpoint"]["safe_to_resume"] is True
                and fixture.api_json(f"/runs/{run_id}/waiting")["reason"] is None,
                "fenced_capacity_wait_cleared_without_unsafe_replay")
        passed("capacity_wait_" + mode + "_prevents_any_second_post_and_clears_waiting")

    for mode in ["generic", "false_proof"]:
        scenario = fixture.fake_runner.add(mode)
        run_id = fixture.create_run(project, scenario)
        terminal = fixture.terminal(run_id)
        require(terminal["state"] == "interrupted" and terminal["error"]["code"] == "unsafe_tool_interrupted"
                and terminal["error"]["retryable"] is False
                and terminal["error"]["details"]["safe_to_resume"] is False, "unproven_runner_outcome_remains_unsafe")
        fixture.assert_no_more_requests(scenario, 1)
        row = fixture.snapshot(run_id)
        require(row["reason"] is None and row["checkpoint"]["safe_to_resume"] is False
                and fixture.api_json(f"/runs/{run_id}/waiting")["reason"] is None,
                "unproven_runner_outcome_has_no_safe_capacity_wait")
        passed(mode + "_429_is_not_retry_authority_and_retains_unsafe_outcome")

    counts = {scenario.mode: len(fixture.fake_runner.records(scenario))
              for scenario in fixture.fake_runner.scenarios.values()}
    require(counts == {"success": 3, "cancel": 1, "lease_loss": 1, "generic": 1, "false_proof": 1},
            "exact_runner_post_counts_after_all_cases")
    fixture.stop_server(fixture.worker, "worker")
    fixture.stop_server(fixture.api, "api")
    fixture.fake_runner.assert_healthy()
    passed("owned_api_and_worker_sigterm_clean_shutdown")
    report["runner_post_counts"] = counts


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server-bin", required=True, type=Path)
    parser.add_argument("--expected-sha256", required=True, help="Explicitly approved candidate binary hash")
    parser.add_argument("--expected-migration", required=True, type=int)
    parser.add_argument("--report", required=True, type=Path)
    args = parser.parse_args()

    def interrupted(signum, frame):
        for number in HANDLED_SIGNALS:
            signal.signal(number, signal.SIG_IGN)
        raise QualificationFailure("runner_wait_qualification_interrupted")

    for number in HANDLED_SIGNALS:
        signal.signal(number, interrupted)
    started = time.monotonic()
    report = {"schema_version": 1, "passed": False, "checks": [],
              "started_at": datetime.now(timezone.utc).isoformat(),
              "scope": {"native_api_worker_and_pg16": True, "authenticated_fake_runner_http": True,
                        "real_docker_capacity": False, "live_fleet": False},
              "limitations": ["The private HTTP runner supplies protocol responses; no tool or Docker workload is executed.",
                              "Native binary acceptance does not qualify container-image or production deployment.",
                              "SIGKILL or host termination cannot execute fixture cleanup handlers."]}
    try:
        require(os.name == "posix" and os.geteuid() != 0, "linux_nonroot_execution")
        require(args.expected_migration >= 54, "runner_wait_schema54_or_later_required")
        require(bool(re.fullmatch(r"[a-f0-9]{64}", args.expected_sha256)), "explicit_candidate_sha256_required")
        require((PG_BIN / "initdb").is_file(), "postgres16_binary_available")
        binary = args.server_bin.resolve(strict=True)
        require(binary.is_file() and os.access(binary, os.X_OK), "candidate_binary_executable")
        digest = hashlib.sha256()
        with binary.open("rb") as candidate:
            for chunk in iter(lambda: candidate.read(1024 * 1024), b""):
                digest.update(chunk)
        report["candidate_sha256"] = digest.hexdigest()
        require(report["candidate_sha256"] == args.expected_sha256, "candidate_matches_approved_sha256")
        report["harness_sha256"] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
        report["fixture_helper_sha256"] = hashlib.sha256(Path(__file__).with_name("qualify_runtime.py").read_bytes()).hexdigest()
        with tempfile.TemporaryDirectory(prefix="cowork-runner-wait-qualification-", dir="/tmp") as directory:
            fixture = RunnerWaitFixture(Path(directory), binary)
            try:
                try:
                    qualify(fixture, report, args.expected_migration)
                except Exception:
                    if fixture.fake_runner is not None:
                        try:
                            report["diagnostics"] = fixture.diagnostics()
                        except Exception:
                            report["diagnostics"] = {"available": False}
                    raise
            finally:
                fixture.cleanup()
        report["passed"] = True
    except QualificationFailure as error:
        report["failure_check"] = str(error)
    except Exception as error:
        report["failure_kind"] = type(error).__name__
    report["duration_seconds"] = round(time.monotonic() - started, 3)
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, separators=(",", ":")))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
