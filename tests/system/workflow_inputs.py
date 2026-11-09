"""Independent real-daemon workflow question and durable answer qualification.

The fixture owns temporary SQLite data, daemon processes and a synthetic loopback
provider. It does not open a preview, change an installed app or use a real model.
"""
from execution_plan_client import reviewed_run
import argparse
import contextlib
import csv
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

ANSWERS = {
    "ask_a": '  Antwort A: "ja" \\pfad\r\nGrüße 日本語\n  ',
    "ask_b": 'Antwort B\nzweite Zeile 🌻\r\n  ',
}


def protect_empty_fixture(root):
    """Bind a newly created, still empty test root to the current Windows user."""
    root=pathlib.Path(root).resolve()
    assert root.is_dir() and not any(root.iterdir()), 'Only a new empty fixture may receive private ACLs'
    if os.name!='nt':
        return
    options={'creationflags':subprocess.CREATE_NO_WINDOW}
    identity=subprocess.check_output(['whoami','/user','/fo','csv','/nh'],text=True,**options)
    sid=next(csv.reader(identity.strip().splitlines()))[1]
    assert sid.startswith('S-1-5-')
    subprocess.run(['icacls',str(root),'/inheritance:r','/grant:r',f'*{sid}:(OI)(CI)F',
        '*S-1-5-18:(OI)(CI)F','*S-1-5-32-544:(OI)(CI)F'],check=True,stdout=subprocess.DEVNULL,**options)
    subprocess.run(['icacls',str(root),'/setowner','*'+sid],check=True,stdout=subprocess.DEVNULL,**options)


def string_values(value):
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for item in value.values():
            yield from string_values(item)
    elif isinstance(value, list):
        for item in value:
            yield from string_values(item)


class Provider(http.server.BaseHTTPRequestHandler):
    requests = []
    errors = []
    lock = threading.Lock()

    def log_message(self, *_):
        pass

    def do_GET(self):
        assert self.path == "/models", self.path
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(b'{"data":[{"id":"workflow-input-fixture"}]}')

    def do_POST(self):
        try:
            assert self.path == "/chat/completions", self.path
            body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            with self.lock:
                self.requests.append(body)
            step = None
            for message in body["messages"]:
                content = message.get("content")
                if isinstance(content, str):
                    try:
                        value = json.loads(content)
                    except json.JSONDecodeError:
                        continue
                    if isinstance(value, dict) and "workflow_step" in value:
                        step = value["workflow_step"]
            assert step in ("ask_a", "ask_b", "independent"), body
            names = [tool["function"]["name"] for tool in body.get("tools", [])]
            assert "request_input" in names, names
            tool_messages = [m for m in body["messages"] if m.get("role") == "tool"]
            delta = {"content": "Synthetic completed " + step}
            if step == "independent":
                assert not tool_messages
            elif tool_messages:
                assert len(tool_messages) == 1, body
                assert tool_messages[0]["tool_call_id"] == "same-raw-call"
                assert tool_messages[0]["content"] == ANSWERS[step], tool_messages
                other = "ask_b" if step == "ask_a" else "ask_a"
                assert all(ANSWERS[other] not in text for text in string_values(body))
            else:
                delta["tool_calls"] = [{"index": 0, "id": "same-raw-call", "type": "function",
                    "function": {"name": "request_input", "arguments": json.dumps({
                        "question": "Question for " + step + "?"})}}]
            payload = {"choices": [{"index": 0, "delta": delta,
                "finish_reason": "tool_calls" if "tool_calls" in delta else "stop"}],
                "usage": {"prompt_tokens": 64, "completion_tokens": 16}}
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
    def count(cls):
        with cls.lock:
            return len(cls.requests)


class Fixture:
    def __init__(self, binary, data, provider):
        self.binary, self.data, self.provider = binary, data, provider
        self.process, self.token = None, ""
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            self.port = sock.getsockname()[1]
        self.base = f"http://127.0.0.1:{self.port}/api/v2"
        self.env = {**os.environ, "COWORK_DATA_DIR": str(data), "COWORK_PORT": str(self.port), "RUST_LOG": "error"}
        for key in ("COWORK_DATABASE_URL", "COWORK_WEB_DIR", "COWORK_ENABLE_TEST_PROVIDER"):
            self.env.pop(key, None)
        self.budgets = {"max_steps": 32, "max_seconds": 180, "max_output_tokens": 256, "max_total_tokens": 131072}

    def call(self, method, path, body=None):
        request = urllib.request.Request(self.base + path, method=method,
            data=None if body is None else json.dumps(body, ensure_ascii=False).encode(),
            headers={"Authorization": "Bearer " + self.token, "Content-Type": "application/json"})
        with urllib.request.urlopen(request, timeout=10) as response:
            return json.load(response)

    def error(self, method, path, body, status, code=None):
        try:
            self.call(method, path, body)
        except urllib.error.HTTPError as error:
            payload = json.load(error)
            assert error.code == status, (path, error.code, payload)
            assert payload["correlation_id"], payload
            if code is not None:
                assert payload["code"] == code, payload
            return payload
        raise AssertionError("Expected refusal: " + path)

    def lost_ack(self, method, path, body=None, status=200):
        connection = http.client.HTTPConnection("127.0.0.1", self.port, timeout=10)
        try:
            connection.request(method, "/api/v2" + path,
                None if body is None else json.dumps(body, ensure_ascii=False).encode(),
                {"Authorization": "Bearer " + self.token, "Content-Type": "application/json"})
            response = connection.getresponse()
            assert response.status == status, (path, response.status, response.read())
            response.close()  # Deliberately consume no application receipt.
        finally:
            connection.close()

    def wait(self, path, predicate, timeout=35):
        deadline = time.monotonic() + timeout
        value = None
        while time.monotonic() < deadline:
            try:
                value = self.call("GET", path)
                if predicate(value):
                    return value
            except urllib.error.HTTPError as error:
                payload = json.load(error)
                if error.code != 503 or not payload.get("retryable"):
                    raise
            time.sleep(.05)
        raise AssertionError((path, value, Provider.errors))

    def start(self, no_worker=False):
        assert self.process is None
        options = {"creationflags": subprocess.CREATE_NO_WINDOW} if os.name == "nt" else {}
        self.process = subprocess.Popen([str(self.binary)] + (["--no-worker"] if no_worker else []),
            env=self.env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, **options)
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                raise AssertionError(self.process.stderr.read().decode())
            try:
                self.token = (self.data / "daemon.token").read_text().strip()
                self.call("GET", "/health")
                return
            except (FileNotFoundError, urllib.error.URLError, TimeoutError):
                time.sleep(.05)
        raise AssertionError("Daemon startup timeout")

    def kill(self):
        if self.process is not None:
            if self.process.poll() is None:
                self.process.kill()
            self.process.wait(timeout=5)
            self.process.stderr.close()
            self.process = None

    def initialize(self):
        self.project = self.call("POST", "/projects", {"name": "Disposable workflow input fixture"})
        self.model = self.call("POST", "/providers", {"name": "Synthetic workflow input model",
            "kind": "openai_compatible", "model": "workflow-input-fixture",
            "endpoint": f"http://127.0.0.1:{self.provider.server_port}", "context_tokens": 16384})
        rights = {"tools": ["request_input"], "folders": []}
        definition = {"format_version": 1, "name": "Parallel questions", "mode": {"mode": "parallel", "max_parallel": 3},
            "failure": "continue_independent", "rights": rights, "budgets": self.budgets,
            "roles": [{"id": "reader", "instructions": "Ask the question specific to your step.",
                "provider_id": self.model["id"], "provider_revision": self.model["revision"],
                "rights": rights, "budgets": self.budgets, "delegate_to": []}],
            "steps": [{"id": step, "role_id": "reader", "goal": "Synthetic " + step,
                "dependencies": [], "rights": rights, "budgets": self.budgets}
                for step in ("ask_a", "ask_b", "independent")]}
        self.library = self.call("POST", f"/projects/{self.project['id']}/workflows", {
            "idempotency_key": "questions-once", "definition": definition})

    def run(self, title, mode="confirm_mutations"):
        order = self.call("POST", "/orders", {"project_id": self.project["id"], "title": title})
        request = {"kind": "workflow", "order_id": order["id"], "base_revision": order["revision"],
            "idempotency_key": "start-once", "prompt": "Execute the synthetic parallel questions.",
            "workflow_id": self.library["id"], "version_id": self.library["versions"][0]["id"],
            "access_mode": mode, "budgets": self.budgets}
        request["expected_binding"] = self.call("POST", "/preflight", request)["workflow"]
        return order, reviewed_run(self.call, request)["id"]

    def questions(self, run):
        view = self.wait(f"/runs/{run}/workflow", lambda v: len(v["inputs"]) == 2
            and all(q["actionable"] for q in v["inputs"]))
        assert view["run"]["state"] == "running", view
        assert len({q["request_id"] for q in view["inputs"]}) == 2
        assert len({q["fingerprint"] for q in view["inputs"]}) == 2
        return {q["node_id"]: q for q in view["inputs"]}

    @staticmethod
    def draft_path(run, question):
        return f"/runs/{run}/workflow/inputs/{question['request_id']}/draft"

    @staticmethod
    def intent_path(run, intent):
        return f"/runs/{run}/workflow/input-answer-intents/{intent['id']}"

    def save(self, run, question, answer, key, base=0):
        return self.call("PUT", self.draft_path(run, question), {"fingerprint": question["fingerprint"],
            "base_revision": base, "idempotency_key": key, "answer": answer})

    def prepare(self, run, question, revision, key):
        return self.call("POST", f"/runs/{run}/workflow/inputs/{question['request_id']}/answer-intents", {
            "fingerprint": question["fingerprint"], "base_draft_revision": revision, "idempotency_key": key})


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=pathlib.Path, default=pathlib.Path(
        "target/debug/cowork-daemon.exe" if os.name == "nt" else "target/debug/cowork-daemon"))
    binary = parser.parse_args().binary.resolve()
    assert binary.is_file(), "Build cowork-daemon first"
    provider = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Provider)
    thread = threading.Thread(target=provider.serve_forever, daemon=True)
    thread.start()
    checks = []
    with tempfile.TemporaryDirectory(prefix="cowork-v2-workflow-inputs-") as temp:
        protect_empty_fixture(temp)
        fixture = Fixture(binary, pathlib.Path(temp), provider)
        try:
            fixture.start()
            fixture.initialize()
            order, run = fixture.run("Parallel exact answers")
            questions = fixture.questions(run)
            fixture.wait(f"/runs/{run}/workflow", lambda v: any(n["id"] == "independent" and n["state"] == "completed" for n in v["nodes"]))
            before = fixture.call("GET", f"/runs/{run}/workflow")["run"]["revision"]
            # More than one normal coordinator heartbeat while both questions wait.
            time.sleep(3.3)
            waiting = fixture.call("GET", f"/runs/{run}/workflow")
            assert waiting["run"]["state"] == "running" and waiting["run"]["revision"] > before
            assert all(q["actionable"] for q in waiting["inputs"])
            assert Provider.count() == 3, Provider.requests
            checks.append("Two scoped questions with same raw call ID; independent third node and parent heartbeat continue")
            inbox = fixture.call("GET", "/activity?attention_only=true")
            assert any(item["run"]["id"] == run for item in inbox["items"])
            q_a, q_b = questions["ask_a"], questions["ask_b"]
            empty = fixture.call("GET", fixture.draft_path(run, q_a))
            assert empty["draft"]["revision"] == 0 and empty["draft"]["answer"] == "" and empty["intents"] == []
            saved_a = fixture.save(run, q_a, ANSWERS["ask_a"], "save-a")
            assert saved_a["outcome"] == "saved" and saved_a["draft"]["revision"] == 1
            assert saved_a["draft"]["answer"] == ANSWERS["ask_a"]
            assert saved_a["draft"]["answer_sha256"] == hashlib.sha256(ANSWERS["ask_a"].encode()).hexdigest()
            assert fixture.save(run, q_a, ANSWERS["ask_a"], "save-a") == saved_a
            save_b = {"fingerprint": q_b["fingerprint"], "base_revision": 0, "idempotency_key": "save-b", "answer": ANSWERS["ask_b"]}
            fixture.lost_ack("PUT", fixture.draft_path(run, q_b), save_b)
            saved_b = fixture.call("PUT", fixture.draft_path(run, q_b), save_b)
            assert saved_b["outcome"] == "saved" and saved_b["draft"]["revision"] == 1
            conflict_body = {"fingerprint": q_b["fingerprint"], "base_revision": 0, "idempotency_key": "conflict-b", "answer": "Other retained draft"}
            try:
                fixture.call("PUT", fixture.draft_path(run, q_b), conflict_body)
                raise AssertionError("Expected actual CAS conflict")
            except urllib.error.HTTPError as error:
                conflict = json.load(error)
                assert error.code == 409 and conflict["outcome"] == "conflict", conflict
            assert conflict["draft"] == saved_b["draft"] and conflict["snapshot"]["answer"] == "Other retained draft"
            lookup = fixture.call("GET", fixture.draft_path(run, q_b))
            assert len(lookup["snapshots"]) == 2
            assert fixture.call("GET", fixture.draft_path(run, q_b) + "/snapshots/" + conflict["snapshot"]["id"]) == conflict["snapshot"]
            checks.append("Committed exact UTF-8 drafts, lost Save ACK, original receipt replay and durable CAS conflict copy")
            for answer in ("bad\0answer", "🌻" * 8001):
                fixture.error("PUT", fixture.draft_path(run, q_a), {"fingerprint": q_a["fingerprint"], "base_revision": 1, "idempotency_key": "bad-" + hashlib.sha256(answer.encode()).hexdigest(), "answer": answer}, 400)
            fixture.error("PUT", fixture.draft_path(run, q_a), {"fingerprint": q_b["fingerprint"], "base_revision": 1, "idempotency_key": "wrong-question", "answer": "bad"}, 409)
            fixture.error("PUT", fixture.draft_path(run, q_a), {"fingerprint": q_a["fingerprint"], "base_revision": 1, "idempotency_key": "unknown-owner", "answer": "bad", "owner": "other"}, 400)
            fixture.error("POST", f"/runs/{run}/input", {"request_id": q_a["request_id"], "answer": ANSWERS["ask_a"]}, 403)
            checks.append("Wrong question fingerprint, foreign owner field, legacy workflow input and UTF-8/NUL bounds refused")
            prepare_path = f"/runs/{run}/workflow/inputs/{q_a['request_id']}/answer-intents"
            prepare_body = {"fingerprint": q_a["fingerprint"], "base_draft_revision": 1, "idempotency_key": "answer-a"}
            fixture.lost_ack("POST", prepare_path, prepare_body)
            lookup = fixture.call("GET", fixture.draft_path(run, q_a))
            assert len(lookup["intents"]) == 1 and lookup["intents"][0]["state"] == "pending"
            intent_a = fixture.call("GET", f"/runs/{run}/workflow/input-answer-intents/{lookup['intents'][0]['id']}")
            assert fixture.call("POST", prepare_path, prepare_body) == intent_a
            assert intent_a["submission"]["input"]["answer"] == ANSWERS["ask_a"]
            fixture.error("POST", prepare_path, {**prepare_body, "idempotency_key": "replacement-pending"}, 409)
            edited_a = fixture.save(run, q_a, "A later draft must not change the frozen answer", "edit-after-prepare", 1)
            assert edited_a["draft"]["revision"] == 2
            assert fixture.call("GET", fixture.intent_path(run, intent_a)) == intent_a
            intent_b = fixture.prepare(run, q_b, 1, "answer-b")
            checks.append("Lost Prepare ACK found through service lookup; later draft cannot mutate frozen submission")
            fixture.error("POST", fixture.intent_path(run, intent_a) + "/commit", {"answer": "replacement"}, 400)
            results, errors = {}, []
            def commit_a():
                try:
                    fixture.lost_ack("POST", fixture.intent_path(run, intent_a) + "/commit")
                    results["a"] = fixture.call("GET", fixture.intent_path(run, intent_a))
                except Exception as error:
                    errors.append(repr(error))
            def commit_b():
                try:
                    results["b"] = fixture.call("POST", fixture.intent_path(run, intent_b) + "/commit")
                except Exception as error:
                    errors.append(repr(error))
            threads = [threading.Thread(target=commit_a), threading.Thread(target=commit_b)]
            for task in threads: task.start()
            for task in threads: task.join(timeout=15)
            assert not errors and all(not task.is_alive() for task in threads), (errors, results)
            assert all(i["state"] == "accepted" for i in results.values()), results
            finished = fixture.wait(f"/runs/{run}", lambda v: v["run"]["state"] == "completed")
            assert Provider.count() == 5 and not Provider.errors, (Provider.count(), Provider.errors)
            for key, intent in (("a", intent_a), ("b", intent_b)):
                accepted = results[key]
                assert fixture.call("POST", fixture.intent_path(run, intent) + "/commit") == accepted
                assert accepted["receipt"]["answer_sha256"] == hashlib.sha256(ANSWERS["ask_" + key].encode()).hexdigest()
            events = fixture.call("GET", f"/runs/{run}/events?after=0")
            assert [e["sequence"] for e in events] == list(range(1, len(events) + 1))
            input_events = [e for e in events if e["kind"].startswith("workflow_input_")]
            assert len([e for e in input_events if e["kind"] == "workflow_input_answered"]) == 2
            assert all(answer not in text for answer in ANSWERS.values() for text in string_values(input_events))
            with contextlib.closing(sqlite3.connect(fixture.data / "cowork-v2.sqlite3")) as db:
                payload = json.loads(db.execute("SELECT payload FROM runs WHERE id=?", (run,)).fetchone()[0])
                assert all(answer not in text for answer in ANSWERS.values() for text in string_values(payload["checkpoint"]["messages"]))
            checks.append("Parallel atomic answer commits; exact per-agent answer bytes, lost Commit ACK, one event/receipt and no parent mutation")
            export = fixture.call("GET", f"/orders/{order['id']}/export")
            canonical = json.dumps(export["data"], sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
            assert export["sha256"] == hashlib.sha256(canonical).hexdigest() and export["format_version"] == 13
            assert export["data"]["workflows"][0]["human_inputs"]
            assert export["data"]["workflow_input_answer_records"]["intents"]
            fixture.kill(); fixture.start()
            assert fixture.call("GET", fixture.intent_path(run, intent_a)) == results["a"]
            assert fixture.call("POST", fixture.intent_path(run, intent_a) + "/commit") == results["a"]
            assert fixture.call("GET", f"/orders/{order['id']}/export")["sha256"] == export["sha256"]
            assert Provider.count() == 5
            checks.append("Question/draft/snapshot/intent archival hash and accepted answer receipts survive daemon restart without provider replay")
            _, protected = fixture.run("Every action requires approval", "confirm_all")
            approval_view = fixture.wait(f"/runs/{protected}/workflow", lambda v: len(v["approvals"]) == 2)
            assert approval_view["inputs"] == []
            for approval in approval_view["approvals"]:
                value = approval["approval"]
                assert value["tool"]["name"] == "request_input"
                fixture.call("POST", f"/runs/{protected}/approvals/{value['id']}", {"fingerprint": value["fingerprint"], "allow": True})
            protected_questions = fixture.questions(protected)
            fixture.error("GET", f"/runs/{protected}/workflow/input-answer-intents/{intent_a['id']}", None, 404)
            q = protected_questions["ask_a"]
            fixture.save(protected, q, ANSWERS["ask_a"], "cancel-draft")
            canceled_intent = fixture.prepare(protected, q, 1, "cancel-answer")
            fixture.wait(f"/runs/{protected}/workflow", lambda v: any(
                n["id"] == "independent" and n["state"] == "completed" for n in v["nodes"]))
            before_cancel = Provider.count()
            fixture.call("POST", f"/runs/{protected}/cancel")
            # Wait through the worker's cleanup: an immediate commit alone misses
            # the regression where that cleanup used to overwrite known cancel.
            fixture.wait(f"/runs/{protected}", lambda v: v["run"]["state"] == "canceled")
            try:
                canceled = fixture.call("POST", fixture.intent_path(protected, canceled_intent) + "/commit")
            except urllib.error.HTTPError as error:
                raise AssertionError({"cancel_commit_status": error.code, "error": json.load(error),
                    "parent": fixture.call("GET", f"/runs/{protected}"),
                    "events": fixture.call("GET", f"/runs/{protected}/events?after=0")}) from error
            assert canceled["state"] == "rejected" and canceled["receipt"] is None
            assert fixture.call("POST", fixture.intent_path(protected, canceled_intent) + "/commit") == canceled
            assert not any(q["actionable"] for q in fixture.call("GET", f"/runs/{protected}/workflow")["inputs"])
            assert fixture.call("GET", fixture.draft_path(protected, q))["draft"]["answer"] == ANSWERS["ask_a"]
            fixture.error("PUT", fixture.draft_path(protected, q), {"fingerprint": q["fingerprint"],
                "base_revision": 1, "idempotency_key": "write-after-cancel", "answer": "Must be refused"}, 403)
            assert Provider.count() == before_cancel
            checks.append("ConfirmAll approval precedes questions; actual worker cancellation rejects pending intent, denies new draft writes and preserves history without model replay")
            _, interrupted = fixture.run("Hard kill during human wait", "read_only")
            interrupted_questions = fixture.questions(interrupted)
            q = interrupted_questions["ask_a"]
            whitespace = fixture.save(interrupted, q, "  \r\n", "incomplete-whitespace")
            assert whitespace["draft"]["answer"] == "  \r\n"
            fixture.error("POST", f"/runs/{interrupted}/workflow/inputs/{q['request_id']}/answer-intents", {
                "fingerprint": q["fingerprint"], "base_draft_revision": 1, "idempotency_key": "invalid-blank-answer"}, 400)
            saved = fixture.save(interrupted, q, ANSWERS["ask_a"], "restart-draft", 1)
            pending = fixture.prepare(interrupted, q, 2, "restart-answer")
            before = Provider.count()
            fixture.kill(); fixture.start()
            fixture.wait(f"/runs/{interrupted}", lambda v: v["run"]["state"] == "interrupted")
            lookup = fixture.call("GET", fixture.draft_path(interrupted, q))
            assert lookup["draft"] == saved["draft"] and lookup["intents"][0]["id"] == pending["id"]
            assert Provider.count() == before
            recovered = fixture.call("GET", f"/runs/{interrupted}/workflow")
            assert recovered["input_resume_available"] and not any(q["actionable"] for q in recovered["inputs"])
            assert fixture.call("GET", fixture.intent_path(interrupted, pending))["submission"]["input"]["answer"] == ANSWERS["ask_a"]
            checks.append("ReadOnly human questions and durable pending answers survive hard kill; verified explicit resume preview creates no authority, answer or automatic model replay")
            assert not Provider.errors, Provider.errors
            print(json.dumps({"result": "passed", "real_model": False, "checks": checks,
                "limitations": ["Synthetic loopback provider; no hardware performance or Enterprise authorization",
                    "Human cursor resume remains blocked in this first implementation; no general or narrow resume acceptance"]}))
        finally:
            fixture.kill()
            provider.shutdown()
            provider.server_close()
            thread.join(timeout=5)


if __name__ == "__main__":
    main()
