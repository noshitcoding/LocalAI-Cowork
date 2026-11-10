"""One isolated real V2 device-agent flow; not a test suite or release qualification.

Starts only the supplied daemon and standalone agent in private owned namespaces.
Uses its own synthetic loopback model and device credentials; no V1 process is
created. Nothing runs until this script is explicitly invoked.
"""
from __future__ import annotations

import argparse
import ctypes
import http.server
import importlib.util
import json
import os
import pathlib
import secrets
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
import uuid

ROOT = pathlib.Path(__file__).resolve().parents[2]


def local_module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError("Owned demo helper unavailable")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


common = local_module("cowork_alpha5_owned_demo", pathlib.Path(__file__).with_name("alpha5_demo.py"))
configuration = local_module("cowork_v2_device_configuration", ROOT / "tools/device_agent/configure.py")
PROOF = "V2 standalone agent proof: device-owned model, team approval and local consent."


class AgentFixture(common.Fixture):
    def __init__(self):
        http.server.ThreadingHTTPServer.__init__(self, ("127.0.0.1", 0), AgentHandler)
        self.calls = []
        self.model_key = secrets.token_urlsafe(32)
        self.model_name = "standalone-v2-device-demo"
        self.thread = threading.Thread(target=self.serve_forever, name="v2-agent-owned-http-fixture")
        self.thread.start()


class AgentHandler(common.Handler):
    def do_GET(self):
        if self.path != "/v1/models":
            self.send_error(404)
            return
        self.reply({"object": "list", "data": [{"id": self.server.model_name, "object": "model"}]})

    def do_POST(self):
        size = int(self.headers.get("Content-Length", "0"))
        if self.path != "/v1/chat/completions" or not 0 < size <= 4 * 1024 * 1024:
            self.send_error(400)
            return
        authenticated = self.headers.get("Authorization") == "Bearer " + self.server.model_key
        raw = self.rfile.read(size)
        body = json.loads(raw)
        self.server.calls.append({"path": self.path, "method": "POST", "body_sha256": common.sha(raw),
                                  "device_key_accepted": authenticated, "model": body.get("model")})
        if not authenticated or body.get("model") != self.server.model_name:
            self.send_error(401)
            return
        messages = body.get("messages", [])
        if not any(message.get("role") == "tool" for message in messages):
            tools = {tool["function"]["name"] for tool in body.get("tools", [])}
            if "create_artifact" not in tools:
                self.send_error(400)
                return
            message = {"role": "assistant", "content": "Please approve the exact proof artifact.",
                       "tool_calls": [{"id": "call_" + uuid.uuid4().hex, "type": "function",
                                       "function": {"name": "create_artifact",
                                                    "arguments": json.dumps({"path": "proof.txt", "text": PROOF})}}]}
            finish = "tool_calls"
        else:
            message = {"role": "assistant", "content": "Standalone V2 device execution completed after the approved proof artifact."}
            finish = "stop"
        self.reply({"choices": [{"message": message, "finish_reason": finish}],
                    "usage": {"prompt_tokens": 12, "completion_tokens": 8, "total_tokens": 20}})


# Reuse Alpha.5's gated private console controller and retained native handle
# checks, changing only the owned child's explicit argument vector and label.
AGENT_CONTROLLER = common.WINDOWS_CONTROLLER.replace(
    "[sys.argv[1]]", "[sys.argv[1],'--config',sys.argv[3]]"
).replace('"daemon_pid"', '"agent_pid"')


class OwnedAgent(common.OwnedDaemon):
    def __init__(self, binary, config, evidence):
        self.binary = binary
        self.data = config.parent
        self.evidence = evidence
        self.closed = False
        self.job = None
        self.pidfd = None
        evidence.mkdir(parents=True, exist_ok=False)
        env = {key: value for key, value in os.environ.items() if not key.upper().startswith("COWORK_")}
        self.logs = [(evidence / name).open("wb") for name in ("agent.stdout.log", "agent.stderr.log")]
        if os.name == "nt":
            from ctypes import wintypes as w
            self.k = ctypes.WinDLL("kernel32", use_last_error=True)

            def bind(name, args, result=w.BOOL):
                function = getattr(self.k, name)
                function.argtypes = args
                function.restype = result
                return function

            bind("CreateJobObjectW", [ctypes.c_void_p, w.LPCWSTR], w.HANDLE)
            bind("SetInformationJobObject", [w.HANDLE, ctypes.c_int, ctypes.c_void_p, w.DWORD])
            bind("AssignProcessToJobObject", [w.HANDLE, w.HANDLE])
            bind("QueryInformationJobObject", [w.HANDLE, ctypes.c_int, ctypes.c_void_p, w.DWORD, ctypes.c_void_p])
            bind("TerminateJobObject", [w.HANDLE, w.UINT])
            bind("CloseHandle", [w.HANDLE])

            class Basic(ctypes.Structure):
                _fields_ = [("times", ctypes.c_int64 * 2), ("flags", w.DWORD), ("min_ws", ctypes.c_size_t),
                            ("max_ws", ctypes.c_size_t), ("limit", w.DWORD), ("affinity", ctypes.c_size_t),
                            ("priority", w.DWORD), ("scheduling", w.DWORD)]

            class Extended(ctypes.Structure):
                _fields_ = [("basic", Basic), ("io", ctypes.c_uint64 * 6), ("memory", ctypes.c_size_t * 4)]

            self.job = self.k.CreateJobObjectW(None, None)
            settings = Extended()
            settings.basic.flags = 0x2000
            if not self.job or not self.k.SetInformationJobObject(self.job, 9, ctypes.byref(settings), ctypes.sizeof(settings)):
                raise ctypes.WinError(ctypes.get_last_error())
            self.control = evidence / "controller.json"
            start = subprocess.STARTUPINFO()
            start.dwFlags |= subprocess.STARTF_USESHOWWINDOW
            start.wShowWindow = 0
            self.process = subprocess.Popen(
                [sys.executable, "-I", "-B", "-S", "-c", AGENT_CONTROLLER, str(binary), str(self.control), str(config)],
                cwd=config.parent, env=env, stdin=subprocess.PIPE, stdout=self.logs[0], stderr=self.logs[1],
                creationflags=subprocess.CREATE_NEW_CONSOLE, startupinfo=start)
            if not self.k.AssignProcessToJobObject(self.job, w.HANDLE(int(self.process._handle))):
                self.process.kill()
                self.process.wait()
                raise ctypes.WinError(ctypes.get_last_error())
            self.process.stdin.write(b"go\n")
            self.process.stdin.flush()
        else:
            self.process = subprocess.Popen([str(binary), "--config", str(config)], cwd=config.parent, env=env,
                                            stdin=subprocess.DEVNULL, stdout=self.logs[0], stderr=self.logs[1],
                                            start_new_session=True)
            self.pidfd = os.pidfd_open(self.process.pid)


def command(arguments, evidence):
    """Retain the exact short CLI process; arguments contain only selected paths/IDs."""
    environment = {key: value for key, value in os.environ.items() if not key.upper().startswith("COWORK_")}
    with (evidence / "stdout.log").open("wb") as stdout, (evidence / "stderr.log").open("wb") as stderr:
        child = subprocess.Popen(arguments, cwd=ROOT, env=environment, stdin=subprocess.DEVNULL,
                                 stdout=stdout, stderr=stderr)
        try:
            code = child.wait(timeout=30)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait(timeout=10)
            raise RuntimeError("Own bounded configuration command exceeded its limit") from None
        native_exit = True
        if os.name == "nt":
            kernel = ctypes.WinDLL("kernel32", use_last_error=True)
            value = ctypes.c_ulong()
            kernel.GetExitCodeProcess.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_ulong)]
            native_exit = bool(kernel.GetExitCodeProcess(ctypes.c_void_p(int(child._handle)), ctypes.byref(value))) and value.value == (code & 0xffffffff)
        common.write(evidence / "process-receipt.json", {"exit_code": code, "retained_wait": True,
                                                         "native_exit_confirmed": native_exit, "forced": False})
        if code != 0 or not native_exit:
            raise RuntimeError("Own configuration/consent command failed; private logs retained")


def rejection_status(server, executor, token):
    request = urllib.request.Request(server.base + f"/api/v1/agent/executors/{executor}/heartbeat",
                                     data=json.dumps({"protocol_version": 2, "active_run_ids": [], "health": {}}).encode(),
                                     method="POST", headers={"Authorization": "Bearer " + token, "Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            response.read(4096)
            return response.status
    except urllib.error.HTTPError as error:
        with error:
            error.read(4096)
            return error.code


def demonstrate(daemon_binary, agent_binary, output):
    for path in (daemon_binary, agent_binary, output):
        common.ordinary(path)
    if not daemon_binary.is_file() or not agent_binary.is_file() or output.exists():
        raise RuntimeError("Supply actual daemon/agent executables and a fresh private output directory")
    common.private_data_directory(output)
    report = {"format": "localai-cowork-v2-device-agent-isolated-demo/1", "result": "running",
              "daemon_sha256": common.sha(daemon_binary.read_bytes()), "agent_sha256": common.sha(agent_binary.read_bytes()),
              "harness_sha256": common.sha(pathlib.Path(__file__).read_bytes()), "user_credentials_used": False,
              "v1_daemon_started": False, "formal_qualification": False, "steps": [],
              "limitations": ["Own synthetic loopback model; no real LiteLLM provider qualification",
                              "One personal-device model/artifact run; no physical desktop, Android or installer qualification"]}
    started = time.monotonic()
    fixture = AgentFixture()
    processes = []

    def save():
        common.write(output / "receipt.json", report)

    def step(name, evidence):
        report["steps"].append({"name": name, "evidence": evidence})
        save()

    save()
    try:
        server_data = output / "v2-server-data"
        common.private_data_directory(server_data)
        server = common.OwnedDaemon.__new__(common.OwnedDaemon)
        processes.append(server)
        server.__init__(daemon_binary, server_data, common.free_port(), output / "server-process")
        bootstrap = server.call("POST", "/api/v2/identity/bootstrap", {
            "bootstrap_secret": (server_data / "identity-bootstrap/master.key").read_text().strip(),
            "username": "v2_device_demo_owner", "display_name": "Isolated V2 device owner",
            "password": "own-demo-" + secrets.token_hex(24), "organization_name": "Isolated V2 device team"})
        session = bootstrap["session"]["token"]
        team = bootstrap["organization"]["id"]
        project = server.call("POST", f"/api/v2/teams/{team}/projects", {
            "name": "Device-owned model project", "idempotency_key": str(uuid.uuid4())}, token=session)
        order = server.call("POST", f'/api/v2/teams/{team}/projects/{project["id"]}/orders', {
            "title": "Standalone V2 proof", "prompt": "Create the explicitly approved proof artifact.",
            "idempotency_key": str(uuid.uuid4())}, token=session)
        step("isolated_v2_team_and_project_created", {"team_id": team, "project_id": project["id"]})
        executor = str(uuid.uuid4())
        registration = {"schema_version": 2, "executor_id": executor, "kind": "personal_device", "pool_id": None,
                        "owner_user_id": None, "display_name": "Standalone V2 synthetic personal device",
                        "protocol_version": 2, "capabilities": [{"schema_version": 2, "name": "model.external", "version": "1", "attributes": {}}],
                        "labels": {}, "personal_device_remote_control": "off", "max_concurrent_runs": 1}
        server.call("POST", "/api/v1/devices/pair", {"registration": registration, "team_id": team}, token=session)
        credential = server.call("POST", f"/api/v1/executors/{executor}/credentials", {
            "label": "Own isolated V2 demonstration", "expires_at": None}, token=session)
        inputs = output / "private-import-inputs"
        configuration.create_directory(inputs)
        profile_path = inputs / "device-profile.json"
        configuration.create_file(profile_path, json.dumps({"format": 1, "protocol_version": 2, "schema_version": 2,
                                 "server_url": server.base, "executor_id": executor, "registration": registration,
                                 "token": credential["token"]}, indent=2) + "\n")
        model_key = inputs / "model-key.txt"
        configuration.create_file(model_key, fixture.model_key + "\n")
        imported = output / "private-device-config"
        import_evidence = output / "profile-import-command"
        import_evidence.mkdir()
        command([sys.executable, "-I", "-B", str(ROOT / "tools/device_agent/configure.py"),
                 "--profile", str(profile_path), "--output", str(imported), "--agent-binary", str(agent_binary),
                 "--model-url", fixture.base + "/v1", "--model-name", fixture.model_name,
                 "--model-key-file", str(model_key)], import_evidence)
        config = json.loads((imported / "agent.json").read_text())
        if config["registration"] != registration or (imported / "model-key.txt").read_text().strip() != fixture.model_key:
            raise RuntimeError("Imported registration/key does not match the explicitly selected own profile")
        check_evidence = output / "native-config-check-command"
        check_evidence.mkdir()
        command([str(agent_binary), "--config", str(imported / "agent.json"), "--check"], check_evidence)
        step("profile_import_and_native_config_check", {"executor_id": executor, "registration_exact": True,
                                                        "device_key_copied": True, "native_owner_rights": False})
        agent = OwnedAgent.__new__(OwnedAgent)
        processes.append(agent)
        agent.__init__(agent_binary, imported / "agent.json", output / "agent-process")

        def registered():
            if agent.process.poll() is not None:
                raise RuntimeError("Owned standalone agent exited before registering")
            return next((device for device in server.call("GET", "/api/v1/executors", token=session)
                         if device["registration"]["executor_id"] == executor), {})

        device = common.wait_for(registered, lambda value: value.get("online") is True, timeout=40)
        step("standalone_v2_agent_registered", {"executor_id": executor, "online": device["online"], "v1_ipc_required": False})
        project_wire = server.call("GET", f'/api/v1/projects/{project["id"]}', token=session)
        run = server.call("POST", "/api/v1/runs", {"thread_id": order["id"], "project_id": project["id"],
                          "project_revision": project_wire["revision"], "project_privacy": "team_managed", "task": None,
                          "executor_target": {"kind": "personal_device", "device_id": executor},
                          "required_capabilities": ["model.external"], "input": {"prompt": "Create proof.txt with the explicitly approved demonstration text."},
                          "model_profile_id": None, "snapshot_id": None, "idempotency_key": str(uuid.uuid4())}, token=session)
        run_id = run["spec"]["id"]
        approvals = common.wait_for(lambda: server.call("GET", f"/api/v1/runs/{run_id}/approvals", token=session),
                                    lambda values: any(value.get("state") == "pending" for value in values), timeout=60)
        pending = next(value for value in approvals if value["state"] == "pending")
        action = pending["requested_action"]
        if action.get("tool") != "create_artifact" or action.get("path") != "proof.txt" or action.get("sha256") != common.sha(PROOF.encode()):
            raise RuntimeError("The model did not request the exact synthetic proof artifact")
        server.call("POST", f'/api/v1/runs/{run_id}/approvals/{pending["id"]}/resolve', {
            "expected_revision": pending["revision"], "decision": "approved"}, token=session)
        workspace = pathlib.Path(config["workspace_root"])

        def own_request():
            if agent.process.poll() is not None:
                raise RuntimeError("Owned standalone agent exited before the local decision")
            matches = []
            for path in (workspace / "local-consent").glob("*.request.json"):
                value = json.loads(path.read_text())
                if value.get("run_id") == run_id and value.get("action") == action:
                    matches.append(value)
            return matches

        request = common.wait_for(own_request, lambda values: len(values) == 1, timeout=40)[0]
        proof_path = workspace / "tools/workspaces" / run_id / "proof.txt"
        if proof_path.exists():
            raise RuntimeError("Personal artifact was created before its required local consent")
        local_evidence = output / "local-consent-command"
        local_evidence.mkdir()
        command([str(agent_binary), "consent", "--data", str(workspace), "--request", request["request_id"],
                 "--decision", "approve"], local_evidence)
        step("exact_team_and_local_action_approved", {"run_id": run_id, "server_request_id": pending["id"],
                                                     "local_request_id": request["request_id"], "artifact_absent_before_consent": True})
        result = common.wait_for(lambda: server.call("GET", f"/api/v1/runs/{run_id}", token=session),
                                 lambda value: value["state"] in ("completed", "failed", "canceled", "interrupted"), timeout=60)
        if result["state"] != "completed" or result["result"].get("adapter") != "standalone_v2_device":
            raise RuntimeError("The actual standalone device run failed")
        artifacts = server.call("GET", f"/api/v1/runs/{run_id}/artifacts", token=session)
        artifact = next((value for value in artifacts if value["name"] == "proof.txt"), None)
        if artifact is None:
            raise RuntimeError("Completed device run did not publish the proof artifact")
        raw = server.call("GET", f'/api/v1/runs/{run_id}/artifacts/{artifact["id"]}', token=session, raw=True)
        if raw != PROOF.encode() or proof_path.read_bytes() != raw:
            raise RuntimeError("Native workspace and downloaded server artifact do not match")
        sent = workspace / "transport-outbox" / (run_id + ".sent")
        common.wait_for(lambda: sent.is_file(), bool, timeout=15)
        receipt = json.loads(sent.read_text())
        if not receipt.get("snapshot") or "proof.txt" not in receipt.get("diff", {}).get("added", []):
            raise RuntimeError("Actual agent did not export a committed result snapshot and proof-file diff")
        (output / "proof.txt").write_bytes(raw)
        common.write(output / "completed-run.json", result)
        step("device_model_artifact_snapshot_completed", {"run_id": run_id, "state": result["state"],
               "adapter": result["result"]["adapter"], "model_steps": result["result"].get("steps"),
               "actions": result["result"].get("actions"), "artifact_sha256": common.sha(raw),
               "result_snapshot_id": receipt["snapshot"], "diff": receipt["diff"],
               "model_http_rounds": len(fixture.calls), "device_owned_model_key_accepted": all(call["device_key_accepted"] for call in fixture.calls)})
        agent.stop()
        server.call("POST", f'/api/v1/executors/{executor}/credentials/{credential["credential_id"]}/revoke', {}, token=session)
        status = rejection_status(server, executor, credential["token"])
        if status != 401:
            raise RuntimeError("Revoked own device credential was still accepted")
        step("own_device_credential_revoked", {"executor_id": executor, "heartbeat_http_status": status})
        server.stop()
        report["result"] = "passed"
    except BaseException as error:
        report["result"] = "failed"
        report["error_type"] = type(error).__name__
        report["error"] = str(error)[:1000]
        raise
    finally:
        errors = []
        for process in reversed(processes):
            try:
                process.stop()
            except BaseException as error:
                errors.append(type(error).__name__ + ":" + str(error)[:250])
        fixture.close()
        report["fixture_thread_joined"] = not fixture.thread.is_alive()
        report["fixture_calls"] = fixture.calls
        report["processes"] = [{"label": process.evidence.name, "receipt": getattr(process, "receipt", None)} for process in processes]
        report["seconds"] = round(time.monotonic() - started, 2)
        if errors:
            report["result"] = "failed"
            report["cleanup_errors"] = errors
        save()
    return report


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--daemon", required=True, type=pathlib.Path)
    parser.add_argument("--agent", required=True, type=pathlib.Path)
    parser.add_argument("--output", required=True, type=pathlib.Path)
    args = parser.parse_args()
    try:
        result = demonstrate(args.daemon.absolute(), args.agent.absolute(), args.output.absolute())
        print(json.dumps({"result": result["result"], "receipt": str(args.output.absolute() / "receipt.json")}), flush=True)
    except Exception as error:
        print(json.dumps({"result": "failed", "error_type": type(error).__name__, "message": str(error)[:1000],
                          "receipt": str(args.output.absolute() / "receipt.json")}), flush=True)
        raise SystemExit(1)
    raise SystemExit(0 if result["result"] == "passed" else 1)
