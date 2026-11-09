"""Opt-in real Docker qualification and signed runner execution smoke.

Run only against a freshly initialized local_runner.py fixture. Private keys stay
in memory; the report contains public identities and verified outcomes only.
Requires Python cryptography and Windows Docker Desktop or a local Linux Engine.
"""
import argparse
import hashlib
import json
from pathlib import Path
import ssl
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
import uuid

from cryptography.hazmat.primitives.serialization import load_der_private_key
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey, Ed25519PublicKey


def encoded(value, *, canonical=False):
    return json.dumps(value, separators=(",", ":"), ensure_ascii=False, sort_keys=canonical).encode()


def digest(value):
    return hashlib.sha256(encoded(value, canonical=True)).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runner-container", required=True)
    parser.add_argument("--guest-image", required=True)
    parser.add_argument("--endpoint", default="https://127.0.0.1:9443")
    parser.add_argument("--report", required=True, type=Path)
    args = parser.parse_args()

    def command(*argv, data=None):
        return subprocess.run(argv, input=data, capture_output=True, check=True, timeout=60).stdout

    def read(path):
        return command("docker", "exec", args.runner_container, "/bin/cat", path)

    config = json.loads(read("/var/lib/cowork/runner/runner.json"))
    assert config["local_container_profile"] is True and config["agent_sandbox"] is None
    binding = config["binding"]
    private_der = read("/var/lib/cowork/runner/control-key.pk8")
    try:
        control = load_der_private_key(private_der, password=None)
    except ValueError:
        # ring generates RFC5958 PKCS#8 v2 including the public key. OpenSSL's
        # Ed25519 loader accepts v1 only, so decode this exact ring envelope.
        assert len(private_der) in (83, 85) and private_der[:2] == bytes([0x30, len(private_der) - 2])
        assert private_der[2:16] == bytes.fromhex("020101300506032b657004220420")
        public_offset = 51 if len(private_der) == 83 else 53
        assert private_der[48:public_offset] == bytes.fromhex("812100" if len(private_der) == 83 else "a123032100")
        control = Ed25519PrivateKey.from_private_bytes(private_der[16:48])
        assert control.public_key().public_bytes_raw() == private_der[public_offset:]
    assert control.public_key().public_bytes_raw() == bytes(binding["control_public_key"])
    public = Ed25519PublicKey.from_public_bytes(bytes(binding["runner_public_key"]))
    token = read(config["token_file"]).decode()

    def signed(purpose, payload):
        message = f"localai-cowork/v2/runner/1/{purpose}\0".encode() + encoded(payload)
        return {"payload": payload, "signature": list(control.sign(message))}

    def verify(purpose, envelope):
        message = f"localai-cowork/v2/runner/1/{purpose}\0".encode() + encoded(envelope["payload"])
        public.verify(bytes(envelope["signature"]), message)
        return envelope["payload"]

    with tempfile.TemporaryDirectory(prefix="cowork-local-smoke-") as temporary:
        certificate = Path(temporary) / "runner-ca.pem"
        certificate.write_bytes(read(config["tls_certificate_file"]))
        context = ssl.create_default_context(cafile=str(certificate))
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), urllib.request.HTTPSHandler(context=context))

        def request(path, payload=None):
            req = urllib.request.Request(args.endpoint.rstrip("/") + path,
                                         data=None if payload is None else encoded(payload),
                                         headers={"Authorization": "Bearer " + token, "Content-Type": "application/json"})
            try:
                with opener.open(req, timeout=120) as response:
                    body = response.read(2 * 1024 * 1024)
                    return None if not body else json.loads(body)
            except urllib.error.HTTPError as error:
                raise RuntimeError(f"Runner rejected {path}: HTTP {error.code}: {error.read(512).decode()}") from None

        identifier = uuid.uuid4().hex
        definition = {
            "format_version": 2,
            "artifact": {"kind": "local_linux_image", "reference": args.guest_image},
            "programs": {"python": "usr/bin/python3"},
            "packages": [],
            "limits": {"cpu_millis": 500, "memory_bytes": 268435456, "processes": 64, "scratch_bytes": 268435456},
            "network": {"kind": "denied"},
        }
        install = {"environment_id": "local-smoke-" + identifier, "version_id": "v1", "definition": definition}
        qualification = request("/v2/environments/install", signed("environment_install", install))
        qualified = verify("environment_qualification", qualification)
        expected = {"filesystem_boundary", "network_denied", "credentials_isolated", "process_tree_containment", "linux_local_container", "local_container_resource_limits"}
        assert expected == set(qualified["capabilities"]), qualified["capabilities"]
        assert qualified["definition_sha256"] == digest(definition)
        assert qualified["runner_id"] == binding["runner_id"]
        assert qualified["runner_generation"] == binding["generation"]
        assert qualified["adapter_build_sha256"] == binding["adapter_build_sha256"]
        reports = command("docker", "exec", args.runner_container, "/usr/bin/find",
                          config["data_directory"] + "/attempts", "-name", "qualification-report.json").decode().splitlines()
        measured = [json.loads(read(path)) for path in reports]
        measured = [report for report in measured if digest(report) == qualified["report_sha256"]]
        assert len(measured) == 1, "Signed qualification report is missing or ambiguous"
        measured = measured[0]
        assert measured["definition"] == definition
        assert all(measured["probe"][key] is True for key in
                   ("read_denied", "write_denied", "network_denied", "credentials_isolated",
                    "child_started", "workspace_writable", "resource_limits_enforced"))
        assert measured["local_storage_policy"]["hard_quota"] is False

        base = config["workspace_state_directory"] + "/workspaces/smoke-" + identifier
        root = base + "/files"
        command("docker", "exec", args.runner_container, "/bin/mkdir", "-m", "700", "-p", root)
        identity = command("docker", "exec", args.runner_container, "/usr/bin/stat", "-c", "%d-%i", root).decode().strip()
        workspace_id = hashlib.sha256(identity.encode()).hexdigest()
        marker = encoded({"folder": {"resource_id": workspace_id}})
        command("docker", "exec", "-i", args.runner_container, "/usr/bin/tee", base + "/workspace.json", data=marker)
        command("docker", "exec", args.runner_container, "/bin/chmod", "600", base + "/workspace.json")
        environment = {"environment_id": install["environment_id"], "version_id": "v1", "definition_sha256": digest(definition), "qualification_sha256": digest(qualified)}
        now = int(time.time() * 1000)
        invocation = {"program": "python", "arguments": ["-c", "from pathlib import Path; p=Path('counter.txt'); p.write_text(str(int(p.read_text())+1 if p.exists() else 1)); Path('result.txt').write_text('signed runner smoke'); print('local-profile-ok')"], "timeout_seconds": 15}
        launch = {
            "format_version": 1, "runner_id": binding["runner_id"], "runner_generation": binding["generation"],
            "adapter_build_sha256": binding["adapter_build_sha256"], "process_attempt_id": "smoke-attempt-" + identifier,
            "run_id": "smoke-run-" + identifier, "tool_call_id": "smoke-tool-" + identifier,
            "request_sha256": digest(invocation), "worker_fence_sha256": "a" * 64, "lease_epoch": 1,
            "environment": environment, "workspace_resource_id": workspace_id, "invocation": invocation,
            "issued_at": now, "expires_at": now + 30000, "execution_deadline": now + 60000,
        }
        envelope = signed("launch", launch)
        request("/v2/launches", envelope)
        deadline = time.monotonic() + 60
        while True:
            receipt = request("/v2/receipts/" + launch["process_attempt_id"])
            observed = verify("receipt", receipt)
            assert observed["request"] == launch
            if observed["state"]["state"] == "completed":
                break
            assert time.monotonic() < deadline, "Signed execution remains unconfirmed"
            time.sleep(0.25)
        result = observed["state"]["result"]
        assert result["termination"] == "exited" and result["exit_code"] == 0 and result["output_complete"] is True, result
        assert isinstance(result["process_id"], int) and result["process_id"] > 0, "Actual diagnostic supervisor PID was erased"
        assert bytes(result["stdout"]["bytes"]) == b"local-profile-ok\n"
        assert read(root + "/counter.txt") == b"1"
        assert read(root + "/result.txt") == b"signed runner smoke"
        replay = request("/v2/launches", envelope)
        assert replay == receipt, "Replay changed retained receipt"
        assert read(root + "/counter.txt") == b"1", "Replay executed the physical command again"
        release_deadline = time.monotonic() + 10
        while command("docker", "container", "ls", "--all", "--filter", "label=localai.cowork.binding=" + digest(binding), "--format", "{{.ID}}").strip():
            assert time.monotonic() < release_deadline, "Completed physical fixture containers were not released"
            time.sleep(0.25)
        replay_observation_deadline = time.monotonic() + 2
        while time.monotonic() < replay_observation_deadline:
            assert read(root + "/counter.txt") == b"1", "Replay changed the workspace counter"
            assert not command("docker", "container", "ls", "--all", "--filter",
                               "label=localai.cowork.binding=" + digest(binding), "--format", "{{.ID}}").strip(), "Replay created a new physical container"
            time.sleep(0.25)
        report = {"format": 1, "profile": "local_linux_image", "engine_id": config["local_engine_id"],
                  "binding": binding,
                  "runner_id": binding["runner_id"], "adapter_build_sha256": binding["adapter_build_sha256"],
                  "guest_image": args.guest_image, "qualification": qualification,
                  "measured_qualification_report": measured, "receipt": receipt,
                  "checks": {"signed_qualification_verified": True, "hard_cpu_memory_pids_probe": True,
                             "storage_is_soft_monitored": True, "signed_completion_verified": True,
                             "workspace_promoted": True, "replay_did_not_execute": True,
                             "completed_physical_resources_released": True}}
        args.report.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        print(json.dumps({"passed": True, "report": str(args.report), "runner_id": binding["runner_id"]}))


if __name__ == "__main__":
    main()
