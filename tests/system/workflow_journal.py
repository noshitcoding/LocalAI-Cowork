"""Public native workflow qualification across real daemon/process boundaries.

Only a temporary V2 dataset and a loopback synthetic provider are used. No real
model, live installation, preview process or global database is touched.
"""
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
    probe_entered = threading.Event()
    probe_release = threading.Event()
    hold_probe = False

    def log_message(self, *_):
        pass

    def do_GET(self):
        assert self.path == "/models", self.path
        with self.lock:
            held = self.hold_probe
        if held:
            self.probe_entered.set()
            assert self.probe_release.wait(20), "Controlled probe was not released"
        try:
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(b'{"data":[{"id":"workflow-fixture"}]}')
        except (BrokenPipeError, ConnectionResetError):
            pass

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
    started = time.monotonic()
    scenarios = []
    with tempfile.TemporaryDirectory(prefix="cowork-v2-workflow-journal-") as temp:
        data = pathlib.Path(temp)
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

        def expect_error(method, path, body, status):
            try:
                call(method, path, body)
            except urllib.error.HTTPError as error:
                payload = json.load(error)
                assert error.code == status, (path, error.code, payload)
                return payload
            raise AssertionError("Expected refusal: " + path)

        def wait_value(path, predicate, timeout=30):
            deadline = time.monotonic() + timeout
            value = None
            while time.monotonic() < deadline:
                try:
                    value = call("GET", path)
                    if predicate(value):
                        return value
                except urllib.error.HTTPError as error:
                    payload = json.load(error)
                    if error.code != 503 or not payload.get("retryable"):
                        raise
                time.sleep(.05)
            raise AssertionError((path, value))

        def start(no_worker=True):
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
                except (FileNotFoundError, urllib.error.URLError, TimeoutError):
                    time.sleep(.05)
            raise AssertionError("Daemon startup timeout")

        def kill():
            nonlocal process
            if process is not None:
                if process.poll() is None:
                    process.kill()
                process.wait(timeout=5)
                process.stderr.close()
                process = None

        def lost_ack(path, body=None):
            connection = http.client.HTTPConnection("127.0.0.1", port, timeout=10)
            connection.request("POST", "/api/v2" + path, None if body is None else json.dumps(body),
                {"Authorization": "Bearer " + token, "Content-Type": "application/json"})
            response = connection.getresponse()
            assert response.status == 200
            # Ignore the entire receipt: recovery must discover the accepted result.
            response.close()
            connection.close()

        def hold_probe():
            Provider.probe_entered.clear()
            Provider.probe_release.clear()
            with Provider.lock:
                Provider.hold_probe = True

        def release_probe():
            with Provider.lock:
                Provider.hold_probe = False
            Provider.probe_release.set()

        def dispatch_commit(path):
            outcomes = []
            def run():
                try:
                    outcomes.append(call("POST", path + "/commit"))
                except Exception as error:
                    outcomes.append(error)
            thread = threading.Thread(target=run, daemon=True)
            thread.start()
            assert Provider.probe_entered.wait(5), ("No live probe", outcomes)
            return thread, outcomes

        def make_order(title):
            return call("POST", "/orders", {"project_id": project["id"], "title": title})

        def start_operation(order, intent):
            request = {"kind": "workflow", "order_id": order["id"], "base_revision": order["revision"],
                "idempotency_key": intent + "-key", "prompt": "Run this synthetic workflow.",
                "workflow_id": workflow, "version_id": version, "access_mode": "confirm_mutations", "budgets": budgets}
            operation = call("POST", ops, {"intent_id": intent, "request": {"kind": "start", "request": request}, "draft_snapshot": snapshot_ref})
            return operation, ops + "/" + operation["id"]

        def run_count(order):
            return len(call("GET", f"/orders/{order['id']}/runs"))

        try:
            start()
            project = call("POST", "/projects", {"name": "Disposable durable workflow journal"})
            model = call("POST", "/providers", {"name": "Synthetic journal model", "kind": "openai_compatible",
                "model": "workflow-fixture", "endpoint": f"http://127.0.0.1:{provider.server_port}", "context_tokens": 16_384})
            budgets = {"max_steps": 32, "max_seconds": 120, "max_output_tokens": 256, "max_total_tokens": 131_072}
            root = f"/projects/{project['id']}"
            ops = root + "/workflow-operations"
            drafts = root + "/workflow-drafts"
            draft = {"id": "editor", "base_revision": 0, "idempotency_key": "draft-once",
                "content": {"editor_format": "workflow-v1", "data": {"unfinished": "goal"}, "archived": False}}
            saved = call("PUT", drafts, draft)
            assert saved["outcome"] == "saved"
            snapshot_ref = {"draft_id": "editor", "snapshot_id": saved["snapshot"]["id"], "sha256": saved["snapshot"]["sha256"]}
            kill()
            start()
            assert call("PUT", drafts, draft) == saved
            assert call("GET", drafts + "/editor")["content"] == saved["draft"]["content"]
            scenarios.append("invalid draft and immutable snapshot survive daemon restart")

            rights = {"tools": ["create_artifact", "list_artifacts", "read_artifact"], "folders": []}
            definition = {"format_version": 1, "name": "Journal version one", "mode": {"mode": "sequential"},
                "failure": "stop", "rights": rights, "budgets": budgets,
                "roles": [{"id": "writer", "instructions": "Create a text result using selected tools.",
                    "provider_id": model["id"], "provider_revision": model["revision"], "rights": rights,
                    "budgets": budgets, "delegate_to": []}],
                "steps": [{"id": "create", "role_id": "writer", "goal": "Create a text result.",
                    "dependencies": [], "rights": rights, "budgets": budgets}]}
            command = {"intent_id": "create-intent", "request": {"kind": "create", "request": {
                "idempotency_key": "create-key", "definition": definition}}, "draft_snapshot": snapshot_ref}
            # Prepare acknowledgement is ignored and found again by scoped journal list.
            lost_ack(ops, command)
            creation = call("GET", ops)["items"][0]
            assert creation["request"] == command["request"] and creation["state"] == "pending"
            creation_path = ops + "/" + creation["id"]
            kill()
            start()
            assert call("GET", creation_path) == creation
            lost_ack(creation_path + "/commit")
            accepted = call("GET", creation_path)
            assert accepted["state"] == "accepted"
            original_receipt = accepted["receipt"]
            workflow = original_receipt["publication"]["workflow_id"]
            version = original_receipt["publication"]["version_id"]
            kill()
            start()
            assert call("POST", creation_path + "/commit")["receipt"] == original_receipt
            assert call("GET", root + "/workflows")["total"] == 1
            scenarios.append("lost prepare/commit acknowledgement and restart create exactly one workflow")

            definition["name"] = "Journal version two"
            publication = call("POST", ops, {"intent_id": "publish-intent", "request": {"kind": "publish", "workflow_id": workflow,
                "request": {"base_revision": 1, "idempotency_key": "publish-key", "definition": definition}}, "draft_snapshot": snapshot_ref})
            publication_path = ops + "/" + publication["id"]
            lost_ack(publication_path + "/commit")
            publication_receipt = call("GET", publication_path)["receipt"]
            version = publication_receipt["publication"]["version_id"]
            kill()
            start()
            assert call("POST", publication_path + "/commit")["receipt"] == publication_receipt
            assert len(call("GET", root + "/workflows/" + workflow)["versions"]) == 2
            assert call("POST", creation_path + "/commit")["receipt"] == original_receipt
            scenarios.append("publication lost acknowledgement preserves original frozen receipt")

            discarded_order = make_order("Discard during provider probe")
            _, discarded_path = start_operation(discarded_order, "discard-intent")
            hold_probe()
            thread, outcomes = dispatch_commit(discarded_path)
            discarded = call("POST", discarded_path + "/discard")
            assert discarded["state"] == "discarded"
            release_probe()
            thread.join(10)
            assert not thread.is_alive() and len(outcomes) == 1
            assert isinstance(outcomes[0], dict) and outcomes[0]["state"] == "discarded", outcomes
            assert run_count(discarded_order) == 0
            scenarios.append("discard wins held provider probe with no accepted run")

            interrupted_order = make_order("Probe interrupted by daemon kill")
            operation, operation_path = start_operation(interrupted_order, "start-intent")
            hold_probe()
            thread, outcomes = dispatch_commit(operation_path)
            kill()
            release_probe()
            thread.join(10)
            assert not thread.is_alive()
            assert len(outcomes) == 1 and isinstance(outcomes[0], Exception), outcomes
            start()
            restored = call("GET", operation_path)
            assert restored["state"] == "pending" and restored["request"] == operation["request"]
            assert run_count(interrupted_order) == 0
            expect_error("POST", operation_path + "/commit", {"spec": {"access_mode": "autonomous"}}, 400)
            lost_ack(operation_path + "/commit")
            accepted = call("GET", operation_path)
            receipt = accepted["receipt"]
            assert accepted["state"] == "accepted" and receipt["kind"] == "started"
            run = receipt["run_id"]
            assert run_count(interrupted_order) == 1
            assert call("GET", f"/runs/{run}/workflow")["run"]["state"] == "queued"
            kill()
            start()
            assert call("POST", operation_path + "/commit")["receipt"] == receipt
            assert call("POST", operation_path + "/discard")["state"] == "accepted"
            assert Provider.count() == 0 and run_count(interrupted_order) == 1
            scenarios.append("kill during preflight stays pending; lost start acknowledgement recovers one queued run")

            foreign = call("POST", "/projects", {"name": "Other disposable project"})
            foreign_ops = f"/projects/{foreign['id']}/workflow-operations"
            expect_error("POST", foreign_ops + "/" + operation["id"] + "/commit", None, 404)
            expect_error("GET", f"/projects/{foreign['id']}/workflow-drafts/editor", None, 404)
            assert call("GET", foreign_ops)["total"] == 0
            scenarios.append("foreign project journal and draft are inaccessible")

            kill()
            start(no_worker=False)
            view = wait_value(f"/runs/{run}/workflow", lambda value: bool(value["approvals"]))
            assert view["run"]["state"] == "running" and len(view["approvals"]) == 1
            approval = view["approvals"][0]["approval"]
            call("POST", f"/runs/{run}/approvals/{approval['id']}", {"fingerprint": approval["fingerprint"], "allow": True})
            completed = wait_value(f"/runs/{run}/workflow", lambda value: value["run"]["state"] == "completed")
            assert completed["approvals"] == []
            assert Provider.count() == 2
            artifacts = call("GET", f"/orders/{interrupted_order['id']}/artifacts")
            assert len(artifacts) == 1
            assert call("POST", operation_path + "/commit")["receipt"] == receipt
            assert call("POST", operation_path + "/acknowledge")["acknowledged_at"] is not None
            assert operation["id"] not in [item["id"] for item in call("GET", ops)["items"]]
            assert operation["id"] in [item["id"] for item in call("GET", ops + "?include_acknowledged=true")["items"]]
            assert run_count(interrupted_order) == 1
            with contextlib.closing(sqlite3.connect(data / "cowork-v2.sqlite3")) as database:
                assert database.execute("SELECT COUNT(*) FROM workspaces").fetchone()[0] == 0
            scenarios.append("accepted queued run uses existing worker/exact approval and retains immutable journal receipt")
        finally:
            release_probe()
            kill()
            provider.shutdown()
            provider.server_close()
            provider_thread.join(timeout=5)
    print(json.dumps({"status": "passed", "scenarios": scenarios, "duration_seconds": round(time.monotonic() - started, 3)}))


if __name__ == "__main__":
    main()
