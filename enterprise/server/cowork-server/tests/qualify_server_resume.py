#!/usr/bin/env python3
"""Explicit, isolated Server59 crash/resume qualification with a real Bash job.

Nothing starts on import or without --execute. Uses the approved native Server
and Runner, the existing private Unix Docker relay, an isolated PostgreSQL16,
and the original39 parent baseline. A private model selects one real Bash tool;
it is not an executor. No run, lease or checkpoint is fabricated or updated by
the fixture. Only an exact BEFORE-INSERT fault trigger/advisory lock is installed.
Failures retain private evidence; successful removal requires every cleanup proof.
Central fleet admission is disabled here and is explicitly outside this proof.
"""
import argparse
from contextlib import closing
from datetime import datetime, timezone
from email.message import Message
import hashlib
import http.client
import json
import os
from pathlib import Path
import re
import shutil
import signal
import ssl
import stat
import subprocess
import threading
import time
from urllib.parse import urlsplit
import uuid

from qualify_api_cluster import canonical_uuid, file_sha256
import qualify_linux_desktop_ha as shared
from qualify_runtime import Fixture, HANDLED_SIGNALS, PG_BIN, QualificationFailure, require


MAX_BODY = 2 * 1024 * 1024
SAFE_ORIGIN = "server_agent_v1"
# The approved Server59 debug ELF is 328772104 bytes (313.542 MiB). Keep
# descriptor reads bounded at 512 MiB, separately from the existing 120 MiB
# Runner/Docker-CLI gate; a debug server includes considerably more symbols.
MAX_SERVER_BINARY_BYTES = 512 * 1024 * 1024


def server_stat_stamp(info):
    return (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns, info.st_ctime_ns,
            info.st_mode, info.st_uid)


def server_descriptor_matches(path_info, descriptor_info):
    if os.name == "posix":
        return server_stat_stamp(path_info) == server_stat_stamp(descriptor_info)
    # Windows path and descriptor ctime values can differ even for the same
    # unchanged file. Compare identity here, then each full stamp with its own
    # original source after reading; POSIX keeps the exact ctime comparison.
    return ((path_info.st_dev, path_info.st_ino, path_info.st_size, path_info.st_mtime_ns,
             path_info.st_mode, path_info.st_uid)
            == (descriptor_info.st_dev, descriptor_info.st_ino, descriptor_info.st_size,
                descriptor_info.st_mtime_ns, descriptor_info.st_mode, descriptor_info.st_uid))


def validate_server_stat(info, check):
    require(stat.S_ISREG(info.st_mode) and 4 <= info.st_size <= MAX_SERVER_BINARY_BYTES
            and (os.name != "posix" or info.st_uid == os.geteuid()), check)


def checked_server_elf(path, expected, check):
    """Hash one unchanged owned descriptor; do not loosen Runner/CLI limits."""
    path = Path(path)
    require(path.is_absolute() and re.fullmatch(r"[0-9a-f]{64}", expected) is not None
            and not any(parent.is_symlink() for parent in [path, *path.parents])
            and os.access(path, os.X_OK), check)
    initial = path.lstat()
    validate_server_stat(initial, check)
    with os.fdopen(os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)), "rb") as source:
        opened = os.fstat(source.fileno())
        validate_server_stat(opened, check)
        require(server_descriptor_matches(initial, opened), check)
        magic = source.read(4)
        require(magic == b"\x7fELF", check)
        digest, size = hashlib.sha256(magic), len(magic)
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            size += len(chunk)
            require(size <= MAX_SERVER_BINARY_BYTES, check)
            digest.update(chunk)
        require(size == opened.st_size and digest.hexdigest() == expected
                and server_stat_stamp(os.fstat(source.fileno())) == server_stat_stamp(opened)
                and server_stat_stamp(path.lstat()) == server_stat_stamp(initial), check)
    return path


def owned_server_copy(source, destination, expected, check):
    """Exclusive own private copy, fsynced then executable/read-only at 0500."""
    source = checked_server_elf(source, expected, check)
    destination = Path(destination)
    parent = destination.parent
    require(destination.is_absolute() and parent.is_dir()
            and not any(path.is_symlink() for path in [parent, *parent.parents])
            and (os.name != "posix" or (parent.stat().st_uid == os.geteuid()
                 and parent.stat().st_mode & 0o077 == 0)), check)
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0)
    with os.fdopen(os.open(source, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)), "rb") as input_file:
        initial = source.lstat()
        opened = os.fstat(input_file.fileno())
        validate_server_stat(opened, check)
        require(server_descriptor_matches(initial, opened), check)
        with os.fdopen(os.open(destination, flags, 0o600), "wb") as output:
            size = 0
            for chunk in iter(lambda: input_file.read(1024 * 1024), b""):
                size += len(chunk)
                require(size <= MAX_SERVER_BINARY_BYTES, check)
                output.write(chunk)
            output.flush()
            os.fsync(output.fileno())
            require(size == opened.st_size
                    and server_stat_stamp(os.fstat(input_file.fileno())) == server_stat_stamp(opened)
                    and server_stat_stamp(source.lstat()) == server_stat_stamp(initial), check)
            os.fchmod(output.fileno(), 0o500) if os.name == "posix" else destination.chmod(0o500)
    checked_server_elf(destination, expected, check)
    return destination


def scenario_values(identifier):
    identifier = canonical_uuid(identifier)
    return {"id": identifier, "call_id": "resume_bash_" + uuid.UUID(identifier).hex,
            "marker": "resume-" + uuid.UUID(identifier).hex + ".txt"}


def bash_message(values):
    expected = scenario_values(values["id"])
    require(values == expected, "exact_owned_model_scenario")
    command = "printf '%s\\n' '" + values["id"] + "' >> /workspace/" + values["marker"] + "; printf '%s\\n' 'resume-fixture-ok'"
    return {"role": "assistant", "content": None, "tool_calls": [{"id": values["call_id"],
        "type": "function", "function": {"name": "Bash", "arguments": json.dumps({
            "command": command, "timeout_seconds": 10}, separators=(",", ":"))}}]}


def assert_resumed_context(request, values):
    messages = request.get("messages", [])
    require(sum(message.get("role") == "tool" and message.get("tool_call_id") == values["call_id"]
                and isinstance(message.get("content"), str) and "resume-fixture-ok" in message["content"]
                for message in messages) == 1, "resumed_model_receives_completed_bash_result_once")


def assert_completed_cursor(checkpoint, run, lease, values):
    require(checkpoint["origin"] == SAFE_ORIGIN and checkpoint["safe"] is True
            and checkpoint["worker"] == lease["owner"] and checkpoint["lease"] == lease["token"]
            and checkpoint["attempt"] is not None, "trusted_safe_checkpoint_provenance")
    state = checkpoint["state"]
    require(state.get("version") == 1 and state.get("run_id") == run
            and state.get("safe_to_resume") is True and state.get("next_step") == 2
            and state.get("pending") is None and state.get("finished") is None,
            "safe_cursor_after_one_completed_bash")
    tools = [message for message in state.get("messages", [])
             if message.get("role") == "tool" and message.get("tool_call_id") == values["call_id"]]
    require(len(tools) == 1 and isinstance(tools[0].get("content"), str)
            and "resume-fixture-ok" in tools[0]["content"],
            "safe_cursor_contains_actual_bash_result_once")
    return checkpoint


def fault_sql(run, lease, values, key, name):
    """Only this run/worker/lease's next unsafe cursor can block; no data writes."""
    run = canonical_uuid(run)
    owner, token = (canonical_uuid(lease[field]) for field in ("owner", "token"))
    require(values == scenario_values(values["id"]), "fault_scenario_binding")
    require(re.fullmatch(r"resume_fault_[0-9a-f]{32}", name) is not None
            and len(key) == 2 and all(type(item) is int and 1 <= item < 2 ** 31 for item in key),
            "fault_identifier_and_advisory_key_bound")
    call = values["call_id"]
    return f"""CREATE FUNCTION {name}() RETURNS trigger LANGUAGE plpgsql AS $fault$
DECLARE latest record;
BEGIN
  IF NEW.run_id='{run}'::uuid AND NEW.origin='server_agent_v1'
    AND NEW.worker_id='{owner}'::uuid AND NEW.worker_lease_token='{token}'::uuid
    AND NEW.server_execution_attempt_id IS NOT NULL AND NEW.safe_to_resume=false
    AND NEW.executor_state->>'next_step'='2' AND NEW.executor_state->'pending'='null'::jsonb THEN
    SELECT * INTO latest FROM run_checkpoints WHERE run_id=NEW.run_id ORDER BY sequence DESC LIMIT 1;
    IF latest.origin='server_agent_v1' AND latest.safe_to_resume=true
      AND latest.worker_id=NEW.worker_id AND latest.worker_lease_token=NEW.worker_lease_token
      AND latest.server_execution_attempt_id=NEW.server_execution_attempt_id
      AND latest.executor_state->>'next_step'='2' AND latest.executor_state->'pending'='null'::jsonb
      AND EXISTS(SELECT 1 FROM jsonb_array_elements(latest.executor_state->'messages') m
        WHERE m->>'role'='tool' AND m->>'tool_call_id'='{call}'
          AND strpos(m->>'content','resume-fixture-ok')>0)
      AND EXISTS(SELECT 1 FROM run_events WHERE run_id=NEW.run_id AND kind='tool_completed'
        AND payload->>'tool'='Bash' AND payload->>'tool_call_id'='{call}') THEN
      PERFORM pg_advisory_xact_lock({key[0]},{key[1]});
    END IF;
  END IF;
  RETURN NEW;
END $fault$;
CREATE TRIGGER {name} BEFORE INSERT ON run_checkpoints FOR EACH ROW EXECUTE FUNCTION {name}();"""


class BashScenario:
    def __init__(self):
        self.values = scenario_values(str(uuid.uuid4()))
        self.id = self.values["id"]
        self.ready, self.release, self.resumed = (threading.Event() for _ in range(3))
        self.requests, self.tool_deliveries = 0, 0
        self.resumed_has_tool_result = False


class BashModel:
    def __init__(self, port):
        self.scenario = BashScenario()
        self.lock, self.stopping = threading.Lock(), threading.Event()
        self.errors = []
        model = self
        class Handler(shared.http.server.BaseHTTPRequestHandler):
            def log_message(self, *unused):
                pass
            def do_POST(self):
                try:
                    length = self.headers.get("Content-Length", "")
                    require(self.path == "/v1/chat/completions" and length.isdecimal()
                            and 0 < int(length) <= MAX_BODY, "owned_resume_model_request_bound")
                    body = self.rfile.read(int(length))
                    require(len(body) == int(length), "owned_resume_model_complete_request")
                    request = json.loads(body)
                    scenario = model.scenario
                    identifiers = {item for message in request.get("messages", [])
                        if isinstance(message.get("content"), str)
                        for item in re.findall(r"resume-fixture:([0-9a-f-]{36})", message["content"])}
                    require(identifiers == {scenario.id}, "owned_resume_model_exact_scenario")
                    with model.lock:
                        scenario.requests += 1
                        count = scenario.requests
                    require(count in {1, 2}, "resume_never_restarts_model_step_one")
                    if count == 1:
                        require(scenario.ready.wait(60) and not model.stopping.is_set(), "resume_initial_model_barrier")
                        require(any(tool.get("function", {}).get("name") == "Bash"
                                    for tool in request.get("tools", [])), "real_worker_bash_tool_available")
                        with model.lock:
                            scenario.tool_deliveries += 1
                        message = bash_message(scenario.values)
                    else:
                        assert_resumed_context(request, scenario.values)
                        scenario.resumed_has_tool_result = True
                        scenario.resumed.set()
                        require(scenario.release.wait(120) and not model.stopping.is_set(), "resume_final_model_barrier")
                        message = {"role": "assistant", "content": "Owned resume qualification complete."}
                    response = json.dumps({"choices": [{"message": message}],
                                           "usage": {"prompt_tokens": 1, "completion_tokens": 1}}).encode()
                    self.send_response(200)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Content-Length", str(len(response)))
                    self.end_headers()
                    self.wfile.write(response)
                except (BrokenPipeError, ConnectionResetError, ssl.SSLError):
                    pass
                except Exception as error:
                    with model.lock:
                        model.errors.append(str(error) if isinstance(error, QualificationFailure) else "owned_resume_model_internal_failure")
                    self.close_connection = True
        self.server = shared.OwnedHttpServer(("127.0.0.1", port), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, name="owned-resume-model")
        self.thread.start()
        self.origin = f"http://127.0.0.1:{port}/v1"
    def close(self):
        self.stopping.set()
        self.scenario.ready.set()
        self.scenario.release.set()
        self.server.close_owned(self.thread)


class RunnerObserver:
    """Fixed CA-verified upstream; verifies genuine Stop MAC, never signs it."""
    def __init__(self, port, runner, certificate, key, helpers):
        upstream = urlsplit(runner.origin)
        require(upstream.scheme == "https" and upstream.hostname in {"localhost", "127.0.0.1"}
                and not upstream.username and not upstream.password and not upstream.query
                and not upstream.fragment and upstream.path in {"", "/"}, "resume_fixed_private_runner_origin")
        self.lock = threading.Lock()
        self.run, self.jobs, self.stops, self.errors, self.connections = None, [], [], [], set()
        self.stopping = threading.Event()
        context = ssl.create_default_context(cafile=str(runner.ca_file))
        observer = self
        class Handler(shared.http.server.BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"
            def log_message(self, *unused):
                pass
            def do_GET(self):
                self.forward()
            def do_POST(self):
                self.forward()
            def forward(self):
                connection = None
                try:
                    route = urlsplit(self.path)
                    require(not route.scheme and not route.netloc and self.path.startswith(("/v1/", "/v2/")), "resume_observer_allowlisted_target")
                    length = self.headers.get("Content-Length", "0")
                    require(length.isdecimal() and 0 <= int(length) <= MAX_BODY and not self.headers.get("Transfer-Encoding"), "resume_observer_request_bound")
                    body = self.rfile.read(int(length))
                    require(len(body) == int(length), "resume_observer_complete_request")
                    request = json.loads(body) if body else {}
                    owned = observer.run is not None and request.get("run_id") == observer.run
                    if owned and route.path in {"/v1/jobs", "/v2/jobs"}:
                        with observer.lock:
                            observer.jobs.append({"lease": request["_execution"]["lease_token"],
                                "attempt": request["_execution"]["attempt_id"], "workspace": request.get("workspace_id"),
                                "sha256": hashlib.sha256(body).hexdigest()})
                    connection = http.client.HTTPSConnection(upstream.hostname, upstream.port or 443, context=context, timeout=40)
                    with observer.lock:
                        observer.connections.add(connection)
                    fields = {name: value for name, value in self.headers.items()
                              if name.lower() not in {"host", "connection", "content-length"}}
                    fields.update({"Content-Length": str(len(body)), "Connection": "close"})
                    connection.request(self.command, self.path, body, fields)
                    with closing(connection.getresponse()) as response:
                        data = response.read(MAX_BODY + 1)
                        require(len(data) <= MAX_BODY, "resume_observer_response_bound")
                        if owned and route.path == "/v1/execution-stop" and response.status == 200:
                            headers = Message()
                            for name, value in response.getheaders():
                                headers[name] = value
                            runner.protocol.verify({"status": response.status, "headers": headers, "body": data,
                                "request": body, "fields": [self.command, self.path,
                                    self.headers["x-cowork-timestamp"], self.headers["x-cowork-nonce"]]}, helpers.EXECUTION_STOP_DOMAIN)
                            result = json.loads(data)
                            require(result.get("execution_stopped") is True and all(result.get(field) == value for field, value in request.items()), "resume_observed_exact_stop_receipt")
                            with observer.lock:
                                observer.stops.append(result)
                        self.send_response(response.status)
                        for name, value in response.getheaders():
                            if name.lower() not in {"connection", "content-length", "transfer-encoding"}:
                                self.send_header(name, value)
                        self.send_header("Content-Length", str(len(data)))
                        self.send_header("Connection", "close")
                        self.end_headers()
                        self.wfile.write(data)
                    self.close_connection = True
                except (BrokenPipeError, ConnectionResetError, ssl.SSLError):
                    self.close_connection = True
                except Exception as error:
                    with observer.lock:
                        observer.errors.append(str(error) if isinstance(error, QualificationFailure) else "resume_observer_internal_failure")
                    self.close_connection = True
                finally:
                    if connection:
                        connection.close()
                        with observer.lock:
                            observer.connections.discard(connection)
        tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        tls.load_cert_chain(certificate, key)
        self.server = shared.OwnedHttpServer(("127.0.0.1", port), Handler, tls=tls)
        self.thread = threading.Thread(target=self.server.serve_forever, name="owned-resume-runner-observer")
        self.thread.start()
        self.origin = f"https://localhost:{port}"
    def close(self):
        self.stopping.set()
        with self.lock:
            connections = list(self.connections)
        for connection in connections:
            connection.close()
        self.server.close_owned(self.thread)


class AdvisoryBarrier:
    def __init__(self, fixture, run, lease, values):
        self.fixture, self.run = fixture, canonical_uuid(run)
        self.key = tuple((part & 0x7fffffff) or 1 for part in [uuid.uuid4().int >> 96, uuid.uuid4().int >> 96])
        self.name = "resume_fault_" + uuid.uuid4().hex
        self.application = self.name + "_holder"
        self.worker_application = fixture.worker_application
        self.holder, self.armed, self.blocked_pid = None, False, None
        self.sql = fault_sql(run, lease, values, self.key, self.name)
    def start(self):
        fixture = self.fixture
        log = (fixture.root / "resume-advisory-holder.log").open("wb")
        fixture.logs.append(log)
        environment = dict(fixture.environment, PGAPPNAME=self.application)
        self.holder = subprocess.Popen([str(PG_BIN / "psql"), "-h", "127.0.0.1", "-p", str(fixture.pg_port),
            "-U", "qualification", "-d", fixture.database, "-X", "-q", "-A", "-t", "-v", "ON_ERROR_STOP=1"],
            stdin=subprocess.PIPE, stdout=log, stderr=log, env=environment, start_new_session=True)
        fixture.children.append(self.holder)
        self.holder.stdin.write(f"SELECT pg_advisory_lock({self.key[0]},{self.key[1]});\n".encode())
        self.holder.stdin.flush()
        fixture.until(lambda: fixture.sql(f"SELECT count(*) FROM pg_locks l JOIN pg_stat_activity a ON a.pid=l.pid WHERE l.locktype='advisory' AND l.granted AND l.classid={self.key[0]}::oid AND l.objid={self.key[1]}::oid AND l.objsubid=2 AND a.datname=current_database() AND a.application_name='{self.application}';") == "1", "owned_advisory_holder_acquired")
        fixture.sql(self.sql)
        self.armed = True
    def blocked(self):
        answer = self.fixture.sql(f"SELECT coalesce(json_agg(a.pid),'[]') FROM pg_locks l JOIN pg_stat_activity a ON a.pid=l.pid WHERE l.locktype='advisory' AND NOT l.granted AND l.classid={self.key[0]}::oid AND l.objid={self.key[1]}::oid AND l.objsubid=2 AND a.datname=current_database() AND a.application_name='{self.worker_application}' AND a.xact_start IS NOT NULL;")
        pids = json.loads(answer)
        require(len(pids) <= 1, "single_exact_worker_checkpoint_transaction_blocked")
        if pids:
            self.blocked_pid = int(pids[0])
            return self.blocked_pid
        return None
    def unlock(self):
        if self.holder is not None and self.holder.poll() is None:
            self.holder.stdin.write(f"SELECT pg_advisory_unlock({self.key[0]},{self.key[1]});\n\\q\n".encode())
            self.holder.stdin.flush()
            self.holder.stdin.close()
            require(self.holder.wait(timeout=10) == 0, "owned_advisory_holder_clean_exit")
    def disarm(self):
        self.unlock()
        if self.armed:
            self.fixture.sql(f"DROP TRIGGER {self.name} ON run_checkpoints; DROP FUNCTION {self.name}();")
            self.armed = False


class ResumeFixture(shared.LinuxFixture):
    def __init__(self, *arguments, helpers):
        super().__init__(*arguments)
        self.helpers, self.barrier, self.worker_application = helpers, None, None
        self.worker_generation = 0
    def prepare_private_services(self):
        # Existing code creates bounded own observer certificates and key file.
        super().prepare_private_services()
        self.model.close()
        self.observer.close()
        self.model = self.observer = None
        self.model = BashModel(self.model_port)
        self.observer = RunnerObserver(self.observer_port, self.runner,
            self.root / "runner-observer.pem", self.root / "runner-observer.key", self.helpers)
        self.scenarios.append(self.model.scenario)
    def runtime_environment(self):
        environment = super().runtime_environment()
        environment.update({"COWORK_LEASE_SECONDS": "60", "COWORK_WORKER_POLL_MS": "200",
                            "COWORK_MAINTENANCE_EVERY_POLLS": "1"})
        return environment
    def start_worker(self):
        port = self.reserve_port()
        self.release_port(port)
        environment = Fixture.server_environment(self, "worker", port)
        environment.update(self.runtime_environment())
        self.worker_application = self.identity + "_resume_worker" + str(self.worker_generation)
        environment["DATABASE_URL"] = self.database_url + "?application_name=" + self.worker_application
        self.worker_generation += 1
        self.worker = self.spawn([str(self.binary)], "resume_worker_" + str(self.worker_generation), environment)
        return self.worker
    def checkpoint(self, run):
        raw = self.sql(f"SELECT jsonb_build_object('id',id,'sequence',sequence,'safe',safe_to_resume,'origin',origin,'worker',worker_id,'lease',worker_lease_token,'attempt',server_execution_attempt_id,'state',executor_state) FROM run_checkpoints WHERE run_id='{canonical_uuid(run)}'::uuid ORDER BY sequence DESC LIMIT 1;")
        return json.loads(raw) if raw else None
    def marker_count(self, volume, values):
        require(values == scenario_values(values["id"]), "marker_reader_binding")
        program = "import pathlib,json;p=pathlib.Path('/in/" + values["marker"] + "');b=p.read_bytes();assert len(b)<=4096;print(json.dumps({'count':b.splitlines().count(b'" + values["id"] + "'),'lines':len(b.splitlines())}))"
        result = json.loads(self.runner.read_volume(volume, program))
        require(result == {"count": 1, "lines": 1}, "actual_workspace_marker_written_exactly_once")
        return result
    def cleanup(self):
        if self.barrier:
            # Close the own holder first. Failure retains all private evidence;
            # immediate PG shutdown in the inherited cleanup releases locks.
            try:
                self.barrier.unlock()
            except Exception:
                self.cleanup_failures.append("resume_advisory_cleanup_unconfirmed")
        super().cleanup()
        require(not self.cleanup_failures, "resume_cleanup_unconfirmed")


def assert_resume_receipt(receipt, run, request, checkpoint, revision):
    require(receipt == {"request_id": request, "run_id": run,
                        "checkpoint_id": checkpoint, "queued_revision": revision + 1}, "exact_current_creator_resume_receipt")


def qualify(fixture, report, migration):
    def passed(name):
        report["checks"].append({"name": name, "passed": True})
    fixture.last_phase = "private_fixture_start"
    fixture.start_cluster()
    fixture.prepare_certificates()
    fixture.prepare_private_services()
    fixture.start_node(0)
    require(fixture.sql("SELECT max(version) FROM _sqlx_migrations WHERE success;") == str(migration), "exact_resume_migration")
    status, _, body = fixture.http(fixture.api_ports[0], "/api/v1/auth/bootstrap", method="POST", token=fixture.bootstrap_secret,
        data={"email": "resume@localhost.invalid", "display_name": "Owned resume fixture",
              "password": fixture.admin_password, "device_id": str(uuid.uuid4())})
    require(status == 201, "resume_current_creator_bootstrap")
    fixture.access_token = json.loads(body)["access_token"]
    team = fixture.api(0, "/teams", expected=201, method="POST", data={"name": "Owned resume fixture"})
    fixture.project = fixture.api(0, "/projects", expected=201, method="POST", data={
        "name": "Owned resume fixture", "privacy": "team_managed", "team_id": team["id"]})
    fixture.start_worker()
    fixture.wait_fresh_groups(fixture.api_ports[0], {"api", "worker"})
    fixture.parent.assert_preserved()
    passed("fresh_pg_authenticated_native_api_and_real_worker_private_https_runner_original39")

    scenario = fixture.model.scenario
    fixture.last_phase = "true_bash_then_unsafe_barrier"
    thread = fixture.api(0, "/threads", method="POST", expected=201, data={"project_id": fixture.project["id"], "title": "Owned resume fixture"})
    record = fixture.api(0, "/runs", method="POST", expected=201, data={
        "project_id": fixture.project["id"], "project_revision": fixture.project["revision"], "thread_id": thread["id"],
        "project_privacy": "team_managed", "executor_target": {"kind": "server_linux", "pool_id": None},
        "required_capabilities": ["shell"], "idempotency_key": str(uuid.uuid4()),
        "input": {"prompt": "resume-fixture:" + scenario.id, "tool_policy": "autonomous"}})
    run = canonical_uuid(record["spec"]["id"])
    fixture.owned_runs.append(run)
    fixture.observer.run = run
    lease = fixture.until(lambda: (value if value["state"] == "running" and value["live"] and value["workspace"] else None)
                          if (value := fixture.run_lease(run)) else None, "resume_initial_worker_live_lease", seconds=30)
    volume = fixture.runner.register_run(run, canonical_uuid(lease["workspace"]))
    fixture.barrier = AdvisoryBarrier(fixture, run, lease, scenario.values)
    fixture.barrier.start()
    scenario.ready.set()
    blocked_pid = fixture.until(fixture.barrier.blocked, "actual_next_unsafe_checkpoint_blocked_after_bash", seconds=35)
    safe = assert_completed_cursor(fixture.checkpoint(run), run, lease, scenario.values)
    spec = fixture.api(0, f"/runs/{run}")["spec"]
    require(scenario.requests == scenario.tool_deliveries == 1
            and fixture.sql(f"SELECT count(*) FROM run_events WHERE run_id='{run}'::uuid AND kind='tool_completed' AND payload->>'tool'='Bash';") == "1"
            and len(fixture.observer.jobs) == 1, "one_actual_bash_before_checkpoint_fault")
    passed("one_real_bash_job_and_durable_trusted_safe_cursor_next_unsafe_insert_held")

    fixture.last_phase = "worker_sigkill_transaction_rollback"
    current = fixture.run_lease(run)
    require(fixture.worker.poll() is None and current["live"] and current["token"] == lease["token"]
            and fixture.barrier.blocked() == blocked_pid, "own_live_worker_before_sigkill")
    Fixture.kill_group(fixture.worker)
    require(fixture.worker.returncode == -signal.SIGKILL, "own_worker_was_sigkilled")
    fixture.barrier.unlock()
    fixture.until(lambda: fixture.sql(f"SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND pid={blocked_pid} AND xact_start IS NOT NULL;") == "0", "killed_worker_checkpoint_backend_transaction_ended")
    require(fixture.checkpoint(run) == safe, "blocked_unsafe_insert_rolled_back_latest_safe_unchanged")
    fixture.barrier.disarm()
    require(scenario.requests == 1, "no_model_step_two_before_resume")
    # Inspect the actual workspace after the held transaction has ended. A
    # reader-container start cannot consume the worker's live lease kill window.
    fixture.marker_count(volume, scenario.values)
    passed("sigkill_unlock_actual_backend_transaction_ends_without_unsafe_checkpoint_commit")

    fixture.last_phase = "genuine_lease_expiry_maintenance"
    fixture.start_worker()
    interrupted = fixture.until(lambda: (row if row["state"] == "interrupted" else None)
                                if (row := fixture.api(0, f"/runs/{run}")) else None,
                                "replacement_worker_real_expiry_maintenance_interrupts", seconds=90)
    current = fixture.run_lease(run)
    require(current["owner"] is None and current["token"] is None and current["expiry"] is None
            and interrupted["spec"] == spec and fixture.checkpoint(run) == safe,
            "actual_expiry_keeps_spec_workspace_and_safe_cursor")
    require(interrupted.get("error", {}).get("code") == "executor_lease_expired", "real_safe_expiry_error_classification")
    require(fixture.sql(f"SELECT count(*) FROM instance_execution_reservations WHERE run_id='{run}'::uuid;") == "0", "central_admission_explicitly_disabled_fixture")
    fixture.marker_count(volume, scenario.values)
    passed("replacement_worker_db_time_expiry_maintenance_interrupted_without_auto_requeue_or_replay")

    fixture.last_phase = "current_creator_real_stop_then_resume"
    revision, request = interrupted["revision"], str(uuid.uuid4())
    payload = {"request_id": request, "expected_revision": revision}
    receipt = fixture.api(0, f"/runs/{run}/resume", method="POST", data=payload)
    assert_resume_receipt(receipt, run, request, safe["id"], revision)
    fixture.until(lambda: scenario.resumed.is_set(), "resumed_step_two_model_request_with_tool_result", seconds=30)
    resumed_lease = fixture.run_lease(run)
    require(resumed_lease["state"] == "running" and resumed_lease["live"]
            and resumed_lease["owner"] != lease["owner"] and resumed_lease["token"] != lease["token"]
            and resumed_lease["workspace"] == lease["workspace"], "resume_new_worker_lease_same_workspace")
    old_attempts = json.loads(fixture.sql(f"SELECT coalesce(json_agg(jsonb_build_object('attempt_id',id,'lease_token',lease_token,'request_sha256',request_sha256,'workspace_id',workspace_id) ORDER BY id),'[]') FROM run_runner_attempts WHERE run_id='{run}'::uuid AND lease_token='{lease['token']}'::uuid;"))
    with fixture.observer.lock:
        stops = list(fixture.observer.stops)
    require(len(old_attempts) == len(fixture.observer.jobs) == len(stops) == 1
            and stops[0]["run_id"] == run and stops[0]["lease_token"] == lease["token"]
            and stops[0]["cancel_lease"] is None and stops[0]["attempts"] == old_attempts,
            "real_request_bound_prior_lease_all_attempts_stop_verified")
    attempts = json.loads(fixture.sql(f"SELECT json_agg(jsonb_build_object('id',id,'lease',lease_token,'resume',resume_checkpoint_id,'stop',stop_receipt) ORDER BY created_at,id) FROM run_server_execution_attempts WHERE run_id='{run}'::uuid;"))
    require(len(attempts) == 2 and attempts[0]["id"] == safe["attempt"] and attempts[0]["stop"] == stops[0]
            and attempts[1]["lease"] == resumed_lease["token"] and attempts[1]["resume"] == safe["id"],
            "new_server_attempt_adopts_exact_safe_checkpoint_after_physical_stop")
    require(scenario.requests == 2 and scenario.tool_deliveries == 1 and scenario.resumed_has_tool_result
            and fixture.api(0, f"/runs/{run}")["spec"] == spec, "resume_cursor_no_tool_or_create_replay")
    fixture.marker_count(volume, scenario.values)
    passed("current_creator_signed_prior_execution_stop_new_attempt_same_spec_workspace_no_bash_replay")

    fixture.last_phase = "receipt_idempotence_and_old_revision_fence"
    require(fixture.api(0, f"/runs/{run}/resume", method="POST", data=payload) == receipt, "same_resume_request_receipt_idempotent")
    fixture.api(0, f"/runs/{run}/resume", method="POST", expected=409,
                data={"request_id": str(uuid.uuid4()), "expected_revision": revision})
    require(len(fixture.observer.stops) == 1 and len(fixture.observer.jobs) == 1, "idempotence_and_revision_refusal_issue_no_physical_commands")
    passed("immutable_same_request_receipt_and_stale_new_request_revision409_no_commands")

    fixture.last_phase = "resumed_completion"
    scenario.release.set()
    final = fixture.until(lambda: (row if row["state"] == "completed" else None)
                          if (row := fixture.api(0, f"/runs/{run}")) else None, "actual_resumed_agent_completion", seconds=30)
    require(final["spec"] == spec and fixture.run_lease(run)["workspace"] == lease["workspace"]
            and len(fixture.observer.jobs) == 1 and scenario.tool_deliveries == 1
            and fixture.sql(f"SELECT count(*) FROM run_events WHERE run_id='{run}'::uuid AND kind='tool_started' AND payload->>'tool'='Bash';") == "1"
            and not fixture.model.errors and not fixture.observer.errors, "final_complete_no_tool_replay_or_fixture_errors")
    fixture.marker_count(volume, scenario.values)
    require(fixture.api(0, f"/runs/{run}/resume", method="POST", data=payload) == receipt, "completed_run_same_resume_receipt_stable")
    passed("normal_resumed_agent_complete_marker_once_receipt_stable_after_completion")
    fixture.stop_server(fixture.worker, "owned_resume_replacement_worker")
    fixture.stop_server(fixture.nodes[0], "owned_resume_api")
    fixture.parent.assert_preserved()
    passed("owned_replacement_worker_api_clean_sigterm_original39_preserved")


def parser():
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("--execute", action="store_true")
    result.add_argument("--server-bin", type=Path, required=True)
    result.add_argument("--expected-sha256", required=True)
    result.add_argument("--runner-binary", type=Path, required=True)
    result.add_argument("--expected-runner-sha256", required=True)
    result.add_argument("--linux-docker-cli", type=Path, required=True)
    result.add_argument("--expected-linux-docker-cli-sha256", required=True)
    result.add_argument("--expected-migration", type=int, required=True)
    result.add_argument("--gui-image-id", required=True)
    result.add_argument("--core-image-id", required=True)
    result.add_argument("--docker-client", type=Path, required=True)
    result.add_argument("--docker-context", required=True)
    result.add_argument("--baseline", type=Path, required=True)
    result.add_argument("--baseline-count", type=int, default=39)
    result.add_argument("--report", type=Path, required=True)
    result.set_defaults(runner_image_id=None, include_api_desktop_phase=False)
    return result


def main():
    args = parser().parse_args()
    report = {"schema_version": 1, "passed": False, "checks": [], "started_at": datetime.now(timezone.utc).isoformat(),
        "scope": {"native_api_processes": 1, "separate_native_worker": True, "fresh_postgresql16": True,
            "real_docker_runner_https": True, "real_bash_tool": True, "fake_model_fixture_only": True,
            "fake_executor": False, "checkpoint_or_state_fabrication": False, "central_admission_qualified": False,
            "original_containers": 39, "production_ha_release": False, "live_fleet": False},
        "limitations": ["Central fleet admission is disabled; this run does not qualify central reservation retention.",
            "Private model selects one actual Bash tool and holds step two; no live provider acceptance is claimed.",
            "SIGKILL before the next unsafe commit is the only injected crash point; failures retain private UUID evidence."]}
    started = time.monotonic()
    helpers = parent = storage = runner = fixture = None
    workload_passed = False
    def write_report():
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    def interrupted(signum, frame):
        for number in HANDLED_SIGNALS:
            signal.signal(number, signal.SIG_IGN)
        raise QualificationFailure("owned_resume_qualification_interrupted")
    saved = {number: signal.getsignal(number) for number in HANDLED_SIGNALS}
    for number in HANDLED_SIGNALS:
        signal.signal(number, interrupted)
    try:
        inputs = shared.qualification_inputs(args)
        require(inputs["migration"] >= 59, "resume_schema59_or_later_required")
        require(args.execute, "explicit_real_resume_fixture_execution_required")
        require(os.name == "posix" and os.geteuid() != 0, "linux_nonroot_execution")
        require((PG_BIN / "initdb").is_file() and shutil.which("openssl"), "pg16_and_openssl_available")
        source = checked_server_elf(inputs["server_bin"], inputs["server_sha"], "exact_approved_native_server_binary")
        shared.validate_native_inputs(inputs)  # Before any ParentDocker command.
        helpers, helper_path = shared.load_runner_helpers()
        storage = shared.QualificationRoot(helpers, kind="server_resume")
        report.update({"fixture_root": str(storage.path), "fixture_identity": storage.identity,
            "private_root_disposition": "retained_qualification_in_progress", "candidate_sha256": inputs["server_sha"],
            "runner_binary_sha256": inputs["runner_sha"], "linux_docker_cli_sha256": inputs["linux_docker_cli_sha"],
            "gui_image_id": inputs["gui_image"], "core_image_id": inputs["core_image"],
            "harness_sha256": file_sha256(Path(__file__)), "runner_fixture_sha256": file_sha256(helper_path),
            "ha_fixture_sha256": file_sha256(Path(shared.__file__)), "baseline_sha256": file_sha256(inputs["baseline"])})
        write_report()
        parent = helpers.ParentDocker(client=inputs["docker_client"], context=inputs["docker_context"],
            baseline_file=inputs["baseline"], required_baseline_count=39)
        binary = owned_server_copy(source, storage.path / "approved-server", inputs["server_sha"], "owned_server_copy_matches_approved_sha")
        runner_binary = shared.owned_native_copy(inputs["runner_binary"], storage.path / "approved-runner", inputs["runner_sha"], "owned_runner_copy_matches_approved_sha")
        docker_cli = shared.owned_native_copy(inputs["linux_docker_cli"], storage.path / "approved-docker-cli", inputs["linux_docker_cli_sha"], "owned_docker_cli_copy_matches_approved_sha")
        runner_root = storage.path / "r"
        runner_root.mkdir(mode=0o700)
        runner = helpers.RunnerFixture(parent, runner_root, runner_image_id=None, gui_image_id=inputs["gui_image"],
            core_image_id=inputs["core_image"], expected_binary_sha256=inputs["runner_sha"], native_binary=runner_binary,
            linux_docker_cli=docker_cli, scope=storage.scope, memory_mib=8192)
        report["runner_fixture_identity"] = runner.identity_record
        write_report()
        runner.start()
        fixture = ResumeFixture(storage.path, binary, inputs["server_sha"], parent, runner, runner.protocol, helpers=helpers)
        qualify(fixture, report, inputs["migration"])
        workload_passed = True
    except QualificationFailure as error:
        report["failure_check"] = str(error)
        report["command_failure_detail"] = shared.public_failure(error, helpers)
        report["failure_source_positions"] = shared.failure_source_positions(error)
    except Exception as error:
        report["failure_kind"] = type(error).__name__
        report["command_failure_detail"] = shared.public_failure(error, helpers)
        report["failure_source_positions"] = shared.failure_source_positions(error)
    finally:
        for number in HANDLED_SIGNALS:
            signal.signal(number, signal.SIG_IGN)
        report["last_action"] = getattr(helpers, "LAST_ACTION", None)
        if fixture is not None:
            report["diagnostics"] = {"last_phase": fixture.last_phase, "last_api_status": fixture.last_api_status,
                "failed_api_status": fixture.failed_api_status, "model_request_count": fixture.model.scenario.requests if fixture.model else 0,
                "bash_delivery_count": fixture.model.scenario.tool_deliveries if fixture.model else 0,
                "runner_job_request_count": len(fixture.observer.jobs) if fixture.observer else 0,
                "verified_runner_stop_count": len(fixture.observer.stops) if fixture.observer else 0,
                "checkpoint_backend_pid": fixture.barrier.blocked_pid if fixture.barrier else None}
        if storage is not None:
            try:
                proofs, failures, inventory = shared.cleanup_owned_fixtures(fixture, runner, parent, helpers, storage)
            except Exception as error:
                proofs, inventory = {key: False for key in shared.ROOT_CLEANUP_KEYS}, None
                failures = [{"cleanup_action": "owned_cleanup", **shared.public_failure(error, helpers)}]
            report.update({"cleanup_proofs": proofs, "cleanup_failures": failures, "retained_owned_inventory": inventory})
            complete = workload_passed and not failures and all(proofs.values())
            try:
                report["private_root_disposition"] = storage.finish(complete, proofs)
                report["passed"] = complete
            except Exception as error:
                report["private_root_disposition"] = "retained_root_removal_unconfirmed"
                failures.append({"cleanup_action": "private_root", **shared.public_failure(error, helpers)})
            if not report["passed"] and storage.path.is_dir():
                try:
                    helpers.write_private(storage.path / "report.json", json.dumps(report, indent=2) + "\n")
                except Exception as error:
                    failures.append({"cleanup_action": "retained_private_report", **shared.public_failure(error, helpers)})
        for number, handler in saved.items():
            signal.signal(number, handler)
    report["duration_seconds"] = round(time.monotonic() - started, 3)
    write_report()
    print(json.dumps(report, separators=(",", ":")))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
