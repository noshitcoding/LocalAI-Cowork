"""Public native workflow qualification across real daemon/process boundaries.

Only a temporary V2 dataset and a loopback synthetic provider are used. No real
model, live installation, preview process or global database is touched.
"""
from private_fixture import protect_empty_fixture
from execution_plan_client import reviewed_run
import argparse
import contextlib
import hashlib
import http.client
import http.server
import json
import os
import pathlib
import socket
import sqlite3
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request


class Provider(http.server.BaseHTTPRequestHandler):
    requests = []
    lock = threading.Lock()

    def log_message(self, *_):
        pass

    def do_GET(self):
        assert self.path == "/models", self.path
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(b'{"data":[{"id":"workflow-fixture"}]}')

    def do_POST(self):
        assert self.path == "/chat/completions", self.path
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        with self.lock:
            self.requests.append(body)
        step = None
        for message in body["messages"]:
            content = message.get("content")
            if not isinstance(content, str):
                continue
            try:
                parsed = json.loads(content)
            except json.JSONDecodeError:
                continue
            if isinstance(parsed, dict) and "workflow_step" in parsed:
                step = parsed["workflow_step"]
                if step == "review":
                    dependency = parsed["dependency_results"]["create"]
                    assert dependency["content"] == "Synthetic output-create"
                    assert dependency["sha256"] == hashlib.sha256(dependency["content"].encode()).hexdigest()
        assert step in ["create", "review"], body
        done = any(message.get("role") == "tool" for message in body["messages"])
        delta = {"content": "Synthetic output-" + step}
        if not done:
            delta["tool_calls"] = [{
                "index": 0, "id": "same-raw-call", "type": "function",
                "function": {"name": "create_artifact", "arguments": json.dumps({
                    "name": step + ".txt", "content": "Synthetic artifact-" + step,
                })},
            }]
        payload = {
            "choices": [{"index": 0, "delta": delta, "finish_reason": "stop" if done else "tool_calls"}],
            "usage": {"prompt_tokens": 100, "completion_tokens": 16},
        }
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        self.wfile.write(("data: " + json.dumps(payload) + "\n\ndata: [DONE]\n\n").encode())

    @classmethod
    def count(cls):
        with cls.lock:
            return len(cls.requests)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=pathlib.Path, default=pathlib.Path(
        "target/debug/cowork-daemon.exe" if os.name == "nt" else "target/debug/cowork-daemon"))
    binary = parser.parse_args().binary.resolve()
    assert binary.is_file(), "Build cowork-daemon first"
    provider = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Provider)
    provider_thread = threading.Thread(target=provider.serve_forever, daemon=True)
    provider_thread.start()
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    with tempfile.TemporaryDirectory(prefix="cowork-v2-workflows-") as temp:
        data = pathlib.Path(temp)
        protect_empty_fixture(data)
        process = None
        token = ""
        env = {**os.environ, "COWORK_DATA_DIR": str(data), "COWORK_PORT": str(port), "RUST_LOG": "error"}
        for key in ["COWORK_DATABASE_URL", "COWORK_WEB_DIR", "COWORK_ENABLE_TEST_PROVIDER"]:
            env.pop(key, None)
        base = f"http://127.0.0.1:{port}/api/v2"

        def call(method, path, body=None):
            request = urllib.request.Request(base + path, method=method,
                data=None if body is None else json.dumps(body).encode(),
                headers={"Authorization": "Bearer " + token, "Content-Type": "application/json"})
            with urllib.request.urlopen(request, timeout=10) as response:
                return json.load(response)

        def rejected(method, path, body, status):
            try:
                call(method, path, body)
            except urllib.error.HTTPError as error:
                payload = json.load(error)
                assert error.code == status, (path, error.code, payload)
                assert payload["correlation_id"] and not payload["retryable"]
                return payload
            raise AssertionError("Expected refusal: " + path)

        def start(no_worker=False):
            nonlocal process, token
            options = {"creationflags": subprocess.CREATE_NO_WINDOW} if os.name == "nt" else {}
            process = subprocess.Popen([str(binary)] + (["--no-worker"] if no_worker else []),
                env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, **options)
            deadline = time.monotonic() + 15
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise AssertionError(process.stderr.read().decode())
                try:
                    token = (data / "daemon.token").read_text().strip()
                    call("GET", "/health")
                    return
                except (FileNotFoundError, PermissionError, urllib.error.URLError, TimeoutError):
                    time.sleep(.05)
            raise AssertionError("Daemon startup timeout")

        def kill():
            nonlocal process
            if process is not None:
                process.kill()
                process.wait(timeout=5)
                process.stderr.close()
                process = None

        def wait_value(path, predicate, timeout=35):
            deadline = time.monotonic() + timeout
            value = None
            while time.monotonic() < deadline:
                try:
                    value = call("GET", path)
                    if predicate(value):
                        return value
                except urllib.error.HTTPError as error:
                    # Snapshot readers may race a real SQLite writer. Only the
                    # documented transient storage refusal is retried here.
                    payload = json.load(error)
                    if error.code != 503 or not payload.get("retryable"):
                        raise
                time.sleep(.05)
            raise AssertionError((path, value))

        def view(run):
            return call("GET", f"/runs/{run}/workflow")

        def approval(run):
            result = wait_value(f"/runs/{run}/workflow", lambda value: bool(value["approvals"]))
            assert result["run"]["state"] == "running"
            assert result["state"] == "running"
            assert len(result["approvals"]) == 1
            return result["approvals"][0]

        def assert_no_workspace():
            assert list((data / "file-broker" / "workspaces").iterdir()) == []
            with contextlib.closing(sqlite3.connect(data / "cowork-v2.sqlite3")) as database:
                assert database.execute("SELECT COUNT(*) FROM workspaces").fetchone()[0] == 0

        def workflow_definition(name):
            rights = {"tools": ["create_artifact", "list_artifacts", "read_artifact"], "folders": []}
            role = {"id": "writer", "instructions": "Create a text result using the selected tools.",
                "provider_id": model["id"], "provider_revision": model["revision"],
                "rights": rights, "budgets": budgets, "delegate_to": []}
            return {"format_version": 1, "name": name, "mode": {"mode": "sequential"},
                "failure": "stop", "rights": rights, "budgets": budgets, "roles": [role],
                "steps": [
                    {"id": "create", "role_id": "writer", "goal": "Create the first result.",
                        "dependencies": [], "rights": rights, "budgets": budgets},
                    {"id": "review", "role_id": "writer", "goal": "Review the first result.",
                        "dependencies": ["create"], "rights": rights, "budgets": budgets},
                ]}

        def request(order, version, key):
            return {"kind": "workflow", "order_id": order["id"], "base_revision": order["revision"],
                "idempotency_key": key, "prompt": "Run this synthetic workflow.",
                "workflow_id": library["id"], "version_id": version,
                "access_mode": "confirm_mutations", "budgets": budgets}

        def archive(order):
            exported = call("GET", f"/orders/{order}/export")
            canonical = json.dumps(exported["data"], sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
            assert exported["sha256"] == hashlib.sha256(canonical).hexdigest()
            return exported

        try:
            start(no_worker=True)
            assert_no_workspace()
            project = call("POST", "/projects", {"name": "Disposable native workflow fixture"})
            model = call("POST", "/providers", {"name": "Synthetic workflow model",
                "kind": "openai_compatible", "model": "workflow-fixture",
                "endpoint": f"http://127.0.0.1:{provider.server_port}", "context_tokens": 16_384})
            budgets = {"max_steps": 32, "max_seconds": 120, "max_output_tokens": 256, "max_total_tokens": 131_072}
            collection = f"/projects/{project['id']}/workflows"
            creation = {"idempotency_key": "library-once", "definition": workflow_definition("Frozen workflow V1")}
            library = call("POST", collection, creation)
            assert call("POST", collection, creation)["id"] == library["id"]
            assert call("GET", collection + "?limit=1")["items"][0]["id"] == library["id"]
            original_version = library["versions"][0]["id"]
            original = call("GET", f"{collection}/{library['id']}/versions/{original_version}")
            order = call("POST", "/orders", {"project_id": project["id"], "title": "Queued workflow with lost response"})
            run_request = request(order, original_version, "workflow-once")
            flight = call("POST", "/preflight", run_request)
            assert flight["workflow"]["version_id"] == original_version
            run_request["expected_binding"] = flight["workflow"]
            assert_no_workspace()
            # The application result body is discarded: the caller has no run ID
            # from this start response and must recover it using the same key.
            prepared = call("POST", "/execution-plans", run_request)
            commit_path = f"/api/v2/orders/{order['id']}/execution-plans/{prepared['plan']['id']}/commit"
            connection = http.client.HTTPConnection("127.0.0.1", port, timeout=10)
            connection.request("POST", commit_path, json.dumps({"sha256": prepared["sha256"]}),
                {"Authorization": "Bearer " + token, "Content-Type": "application/json"})
            response = connection.getresponse()
            assert response.status == 200
            response.close()
            connection.close()
            runs = call("GET", f"/orders/{order['id']}/runs")
            assert len(runs) == 1 and runs[0]["state"] == "queued"
            run = runs[0]["id"]
            frozen_spec = runs[0]["spec"]
            assert frozen_spec["folders"] == [] and frozen_spec["provider"] is None
            assert_no_workspace()
            kill()
            start(no_worker=True)
            assert reviewed_run(call, run_request)["id"] == run
            assert view(run)["run"]["state"] == "queued"
            assert view(run)["run"]["spec"] == frozen_spec
            assert Provider.count() == 0
            publication = call("POST", f"{collection}/{library['id']}/versions", {
                "base_revision": 1, "idempotency_key": "publish-v2",
                "definition": workflow_definition("New workflow V2"),
            })
            assert publication["revision"] == 2
            assert call("GET", f"{collection}/{library['id']}/versions/{original_version}") == original
            kill()
            start()
            first = approval(run)
            assert first["node_id"] == "create" and first["role_id"] == "writer"
            assert view(run)["name"] == "Frozen workflow V1"
            assert reviewed_run(call, run_request)["id"] == run
            assert Provider.count() == 1
            assert call("GET", f"/orders/{order['id']}/artifacts") == []
            inbox = call("GET", "/activity?attention_only=true")
            assert any(item["run"]["id"] == run and item["run"]["state"] == "running" for item in inbox["items"])
            action = first["approval"]
            rejected("POST", f"/runs/{run}/approvals/{action['id']}", {"fingerprint": "changed", "allow": True}, 409)
            call("POST", f"/runs/{run}/approvals/{action['id']}", {"fingerprint": action["fingerprint"], "allow": True})
            second = approval(run)
            assert second["node_id"] == "review"
            assert second["approval"]["tool"]["id"] != action["tool"]["id"]
            call("POST", f"/runs/{run}/approvals/{second['approval']['id']}", {
                "fingerprint": second["approval"]["fingerprint"], "allow": False,
            })
            wait_value(f"/runs/{run}", lambda value: value["run"]["state"] == "completed")
            artifacts = call("GET", f"/orders/{order['id']}/artifacts")
            assert len(artifacts) == 1 and artifacts[0]["name"] == "create.txt"
            assert artifacts[0]["sha256"] == hashlib.sha256(artifacts[0]["content"].encode()).hexdigest()
            assert Provider.count() == 4
            assert view(run)["approvals"] == []
            assert_no_workspace()
            exported = archive(order["id"])
            kill()
            start()
            assert reviewed_run(call, run_request)["id"] == run
            assert archive(order["id"])["sha256"] == exported["sha256"]
            assert Provider.count() == 4

            crashed_order = call("POST", "/orders", {"project_id": project["id"], "title": "Kill during Running approval"})
            crash_request = request(crashed_order, publication["version_id"], "approval-crash-once")
            crash_request["expected_binding"] = call("POST", "/preflight", crash_request)["workflow"]
            crashed = reviewed_run(call, crash_request)["id"]
            open_approval = approval(crashed)["approval"]
            before = Provider.count()
            assert before == 5
            kill()
            start()
            wait_value(f"/runs/{crashed}", lambda value: value["run"]["state"] == "interrupted")
            recovered = view(crashed)
            assert recovered["state"] == "interrupted" and recovered["approvals"] == []
            assert not recovered["completion_resume_available"]
            assert next(node for node in recovered["nodes"] if node["id"] == "create")["state"] == "interrupted"
            with contextlib.closing(sqlite3.connect(data / "cowork-v2.sqlite3")) as database:
                assert database.execute("SELECT actionable_count FROM workflow_attention WHERE run_id=?", (crashed,)).fetchone()[0] == 0
            rejected("POST", f"/runs/{crashed}/approvals/{open_approval['id']}", {
                "fingerprint": open_approval["fingerprint"], "allow": True,
            }, 409)
            assert reviewed_run(call, crash_request)["id"] == crashed
            assert call("GET", f"/orders/{crashed_order['id']}/artifacts") == []
            # Interrupted work remains a review item; there is no actionable
            # approval and no automatic replay of its confirmed model proposal.
            inbox = call("GET", "/activity?attention_only=true")
            assert any(item["run"]["id"] == crashed and item["run"]["state"] == "interrupted" for item in inbox["items"])
            assert Provider.count() == before
            assert_no_workspace()
            archive(crashed_order["id"])
            events = call("GET", f"/runs/{run}/events?after=0")
            assert [event["sequence"] for event in events] == list(range(1, len(events) + 1))
            print(json.dumps({"result": "passed", "real_model": False, "checks": [
                "public versioned library", "immutable earlier version", "exact preflight/start binding",
                "discarded start body and queued persistence", "same-key replay across new workers",
                "Running approval in inbox", "exact allow and deny", "scoped duplicate raw tool IDs",
                "dependent model receives verified result", "atomic artifact and durable export hash",
                "kill during Running approval", "expired lease recovery with no automatic replay",
                "no stale actionable approval", "Interrupted review item", "no implicit workspace or file grants",
            ]}))
        finally:
            kill()
            provider.shutdown()
            provider.server_close()
            provider_thread.join(timeout=5)


if __name__ == "__main__":
    main()
