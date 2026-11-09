"""Native workflow clock denial across actual daemon restart and queue scans.

Only this fixture's disposable SQLite database is edited, while its own daemon
is stopped. Production UTC is unchanged. This proves durable missing/future-HWM
queue guards, not an OS clock rollback, human resume, desktop or Enterprise.
"""
from execution_plan_client import reviewed_run
import argparse
import contextlib
import hashlib
import http.server
import json
import os
import pathlib
import shutil
import sqlite3
import tempfile
import threading
import time

from workflow_inputs import Fixture as InputFixture, protect_empty_fixture


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


class Provider(http.server.BaseHTTPRequestHandler):
    calls = []
    errors = []
    lock = threading.Lock()

    def log_message(self, *_):
        pass

    def do_GET(self):
        try:
            assert self.path == "/models", self.path
            with self.lock:
                self.calls.append({"method": "GET", "path": self.path})
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(b'{"data":[{"id":"workflow-clock-fixture"}]}')
        except Exception as error:
            with self.lock:
                self.errors.append(repr(error))
            self.send_response(500)
            self.end_headers()

    def do_POST(self):
        try:
            assert self.path == "/chat/completions", self.path
            body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            with self.lock:
                self.calls.append({"method": "POST", "path": self.path, "body": body})
            assert body["model"] == "workflow-clock-fixture", body
            # Any accidental dispatch of a denied entry remains visible even
            # if the runtime later failed or discarded the provider response.
            assert "[CLOCK_BLOCKED:" not in json.dumps(body), body
            payload = {"choices": [{"index": 0,
                "delta": {"content": "Synthetic clock fixture completed"},
                "finish_reason": "stop"}], "usage": {"prompt_tokens": 32, "completion_tokens": 8}}
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.end_headers()
            self.wfile.write(("data: " + json.dumps(payload) + "\n\ndata: [DONE]\n\n").encode())
        except Exception as error:
            with self.lock:
                self.errors.append(repr(error))
            self.send_response(500)
            self.end_headers()

    @classmethod
    def sample(cls):
        with cls.lock:
            return json.loads(json.dumps(cls.calls))


class Fixture(InputFixture):
    def __init__(self, binary, data, provider):
        super().__init__(binary, data, provider)
        self.data.mkdir(parents=True)
        self.owned_data = self.data.resolve()
        self.pids = []
        self.budgets = {"max_steps": 8, "max_seconds": 180,
            "max_output_tokens": 256, "max_total_tokens": 32768}

    def start(self, no_worker=False):
        super().start(no_worker)
        self.pids.append(self.process.pid)

    def initialize(self):
        self.project = self.call("POST", "/projects", {"name": "Disposable workflow clocks"})
        self.model = self.call("POST", "/providers", {"name": "Synthetic clock provider",
            "kind": "openai_compatible", "model": "workflow-clock-fixture",
            "endpoint": f"http://127.0.0.1:{self.provider.server_port}", "context_tokens": 16384})
        rights = {"tools": [], "folders": []}
        definition = {"format_version": 1, "name": "One synthetic clock step",
            "mode": {"mode": "sequential"}, "failure": "stop", "rights": rights,
            "budgets": self.budgets, "roles": [{"id": "reader",
                "instructions": "Return the synthetic result without invoking any tool.",
                "provider_id": self.model["id"], "provider_revision": self.model["revision"],
                "rights": rights, "budgets": self.budgets, "delegate_to": []}],
            "steps": [{"id": "clock_step", "role_id": "reader", "goal": "Return a synthetic result.",
                "dependencies": [], "rights": rights, "budgets": self.budgets}]}
        self.library = self.call("POST", f"/projects/{self.project['id']}/workflows", {
            "idempotency_key": "clock-definition", "definition": definition})
        self.ops = f"/projects/{self.project['id']}/workflow-operations"

    def workflow(self, label, blocked=False):
        order = self.call("POST", "/orders", {"project_id": self.project["id"], "title": label})
        request = {"kind": "workflow", "order_id": order["id"], "base_revision": order["revision"],
            "idempotency_key": label + "-start", "prompt": (
                "[CLOCK_BLOCKED:" + label + "]" if blocked else "Synthetic healthy workflow " + label),
            "workflow_id": self.library["id"], "version_id": self.library["versions"][0]["id"],
            "access_mode": "read_only", "budgets": self.budgets}
        request["expected_binding"] = self.call("POST", "/preflight", request)["workflow"]
        operation = self.call("POST", self.ops, {"intent_id": label + "-intent",
            "request": {"kind": "start", "request": request}})
        path = self.ops + "/" + operation["id"]
        accepted = self.call("POST", path + "/commit")
        assert accepted["state"] == "accepted" and accepted["receipt"]["kind"] == "started", accepted
        return {"order": order, "request": request, "operation": accepted,
            "operation_path": path, "run": accepted["receipt"]["run_id"]}

    def single(self):
        order = self.call("POST", "/orders", {"project_id": self.project["id"], "title": "Healthy single agent"})
        return reviewed_run(self.call, {"order_id": order["id"], "base_revision": order["revision"],
            "idempotency_key": "single-start", "prompt": "Synthetic healthy single agent",
            "provider_id": self.model["id"], "provider_revision": self.model["revision"],
            "access_mode": "read_only", "budgets": self.budgets})["id"]

    @contextlib.contextmanager
    def database(self, write=False):
        database = (self.data / "cowork-v2.sqlite3").resolve()
        assert database.parent == self.owned_data and database.is_file()
        if write:
            assert self.process is None, "Database mutation is allowed only after our daemon has stopped"
        with contextlib.closing(sqlite3.connect(database)) as connection:
            connection.execute("BEGIN" if not write else "BEGIN IMMEDIATE")
            try:
                yield connection
                connection.commit()
            except BaseException:
                connection.rollback()
                raise

    def change_clock(self, run, mode):
        with self.database(write=True) as database:
            row = database.execute("SELECT revision,last_run_revision,highwater_ms,payload FROM workflow_execution_clocks WHERE run_id=?", (run,)).fetchone()
            assert row is not None
            projection = json.loads(row[3])
            assert projection["run_id"] == run and projection["project_id"] == self.project["id"]
            assert projection["revision"] == row[0] and projection["last_run_revision"] == row[1]
            assert projection["highwater_ms"] == row[2] and projection["creation_confirmed"] is True
            parent = json.loads(database.execute("SELECT payload FROM runs WHERE id=?", (run,)).fetchone()[0])
            assert parent["record"]["state"] == "queued" and parent["lease"] is None
            assert projection["last_run_revision"] == parent["record"]["revision"]
            if mode == "missing":
                assert database.execute("DELETE FROM workflow_execution_clocks WHERE run_id=?", (run,)).rowcount == 1
                return {"mode": mode, "original_highwater_ms": row[2], "new_highwater_ms": None}
            assert mode == "future"
            highwater = max(row[2], time.time_ns() // 1_000_000) + 3_600_000
            projection["highwater_ms"] = highwater
            assert database.execute("UPDATE workflow_execution_clocks SET highwater_ms=?,payload=? WHERE run_id=?",
                (highwater, json.dumps(projection, ensure_ascii=False, separators=(",", ":")), run)).rowcount == 1
            return {"mode": mode, "original_highwater_ms": row[2], "new_highwater_ms": highwater}

    def blocked_snapshot(self, run, code):
        view = self.wait(f"/runs/{run}/workflow", lambda value: value["run"]["error_code"] == code)
        assert view["run"]["state"] == "queued" and view["run"]["started_at"] is None, view
        assert view["run"]["steps"] == 0 and not view["run"].get("model_attempts", []), view
        assert view["charged_tokens"] == 0 and view["steps_used"] == 0, view
        assert not view["completion_resume_available"] and not view["input_resume_available"], view
        detail = self.call("GET", f"/runs/{run}")
        assert not detail["artifacts"] and not detail["approvals"], detail
        with self.database() as database:
            parent = json.loads(database.execute("SELECT payload FROM runs WHERE id=?", (run,)).fetchone()[0])
            graph = json.loads(database.execute("SELECT payload FROM workflow_runs WHERE run_id=?", (run,)).fetchone()[0])
            events = [json.loads(row[0]) for row in database.execute("SELECT payload FROM events WHERE run_id=? ORDER BY sequence", (run,))]
            clock_row = database.execute("SELECT payload FROM workflow_execution_clocks WHERE run_id=?", (run,)).fetchone()
        assert parent["record"] == view["run"] and parent["lease"] is None
        assert graph["progress"]["revision"] == 0 and graph["progress"]["started_at"] is None, graph
        assert all(node["state"] == {"state": "pending"} and node["started_at"] is None
            for node in graph["progress"]["nodes"].values()), graph
        assert not graph["progress"]["attempts"] and not graph["progress"]["history"], graph
        assert not graph.get("agents", {}) and not graph.get("inputs", {}), graph
        blocks = [event for event in events if event["kind"] == "workflow_activation_blocked"]
        assert len(blocks) == 1 and blocks[0]["payload"]["error_code"] == code, events
        assert blocks[0]["payload"]["no_new_action_started"] is True, blocks
        assert all(event["kind"] not in ("workflow_started", "model_started", "run_started") for event in events), events
        assert [event["sequence"] for event in events] == list(range(1, len(events) + 1)), events
        return {"parent": parent, "graph": graph, "events": events,
            "detail": detail, "clock": None if clock_row is None else json.loads(clock_row[0])}


def scenario(binary, data, provider, mode):
    fixture = Fixture(binary, data, provider)
    try:
        fixture.start(no_worker=True)
        fixture.initialize()
        blocked = fixture.workflow(mode, blocked=True)
        accepted = blocked["operation"]
        initial = fixture.call("GET", f"/runs/{blocked['run']}")
        assert initial["run"]["state"] == "queued" and initial["run"]["steps"] == 0, initial
        fixture.kill()
        change = fixture.change_clock(blocked["run"], mode)
        before_calls = Provider.sample()
        fixture.start()
        code = "workflow_clock_unverified" if mode == "missing" else "workflow_clock_rollback"
        first = fixture.blocked_snapshot(blocked["run"], code)
        assert Provider.sample() == before_calls, "Denied queue activation must perform no provider GET/POST IO"
        assert fixture.call("GET", blocked["operation_path"]) == accepted
        assert fixture.call("POST", blocked["operation_path"] + "/commit") == accepted
        # The accepted workflow operation owns this start receipt. A distinct
        # execution-plan protocol must not create authority from its old request.
        fixture.error("POST", "/execution-plans", blocked["request"], 409)
        assert fixture.call("GET", f"/orders/{blocked['order']['id']}/runs")[0]["id"] == blocked["run"]
        assert Provider.sample() == before_calls, "Accepted replay must not re-preflight or dispatch"
        # More than ten 100ms queue scans without changing the denied entry.
        time.sleep(2.2)
        assert fixture.blocked_snapshot(blocked["run"], code) == first
        assert Provider.sample() == before_calls
        healthy = fixture.workflow("healthy-" + mode)
        healthy_view = fixture.wait(f"/runs/{healthy['run']}/workflow", lambda value: value["run"]["state"] == "completed")
        single = fixture.single()
        fixture.wait(f"/runs/{single}", lambda value: value["run"]["state"] == "completed")
        assert healthy_view["run"]["steps"] > 0
        assert fixture.blocked_snapshot(blocked["run"], code) == first
        after_calls = Provider.sample()
        dispatched = [call for call in after_calls[len(before_calls):] if call["method"] == "POST"]
        assert len(dispatched) == 2, dispatched
        assert all("[CLOCK_BLOCKED:" not in json.dumps(call) for call in dispatched), dispatched
        fixture.kill()
        fixture.start()
        assert fixture.blocked_snapshot(blocked["run"], code) == first
        assert fixture.call("GET", blocked["operation_path"]) == accepted
        assert fixture.call("POST", blocked["operation_path"] + "/commit") == accepted
        assert Provider.sample() == after_calls, "Another restart must neither activate denied work nor replay completed provider IO"
        return {"mode": mode, "run_id": blocked["run"], "error_code": code,
            "clock_change": change, "block_event_id": first["events"][-1]["id"],
            "block_events": 1, "steps": 0, "lease": None, "started_at": None,
            "accepted_start_receipt_preserved": True, "provider_io_during_denial": 0,
            "healthy_workflow_completed": True, "single_agent_completed": True,
            "provider_model_requests_for_healthy_work": len(dispatched),
            "scanner_and_second_restart_idempotent": True}
    finally:
        fixture.kill()
        assert fixture.process is None
        scenario.pids.extend(fixture.pids)


scenario.pids = []


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=pathlib.Path, default=pathlib.Path(
        "target/debug/cowork-daemon.exe" if os.name == "nt" else "target/debug/cowork-daemon"))
    parser.add_argument("--expected-daemon-sha256")
    args = parser.parse_args()
    source = args.binary.resolve()
    assert source.is_file(), "Build cowork-daemon first"
    binary_sha256 = digest(source)
    if args.expected_daemon_sha256:
        assert args.expected_daemon_sha256 == binary_sha256, "Daemon does not match the qualified binary"
    source_manifest = {path.name: digest(path) for path in [pathlib.Path(__file__).resolve(),
        pathlib.Path(__file__).with_name("workflow_inputs.py").resolve()]}
    provider = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Provider)
    thread = threading.Thread(target=provider.serve_forever, daemon=True)
    thread.start()
    scenarios = []
    try:
        with tempfile.TemporaryDirectory(prefix="cowork-v2-workflow-clocks-") as temp:
            protect_empty_fixture(temp)
            owned = pathlib.Path(temp)
            binary = owned / "bin" / source.name
            binary.parent.mkdir()
            shutil.copy2(source, binary)
            assert digest(binary) == binary_sha256
            for mode in ("missing", "future"):
                scenarios.append(scenario(binary, owned / mode, provider, mode))
            assert digest(binary) == binary_sha256
            assert not Provider.errors, Provider.errors
    finally:
        provider.shutdown()
        provider.server_close()
        thread.join(timeout=5)
    assert not thread.is_alive(), "Owned provider must be stopped before pass"
    assert digest(source) == binary_sha256, "Daemon build changed during qualification"
    assert source_manifest == {path.name: digest(path) for path in [pathlib.Path(__file__).resolve(),
        pathlib.Path(__file__).with_name("workflow_inputs.py").resolve()]}, "Fixture sources changed"
    print(json.dumps({"result": "passed", "daemon_sha256": binary_sha256,
        "source_manifest": source_manifest, "private_daemon_pids": scenario.pids,
        "ownDaemonStopped": True, "provider_stopped": True, "real_model": False,
        "scenarios": scenarios, "limitations": [
            "Synthetic loopback provider and temporary native daemon data only",
            "Missing projection and durable future high-water fixture, not an OS clock rollback",
            "No human resume, desktop GUI, native dialogs, WCAG, Enterprise, load or release qualification"]}))


if __name__ == "__main__":
    main()
