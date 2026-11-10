"""Pure real-HA-fixture regressions; no sockets, subprocesses or Docker."""
import json
import hashlib
import base64
from contextlib import nullcontext, redirect_stdout
import io
import os
from pathlib import Path
import struct
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch
import uuid

import qualify_linux_desktop_ha as ha


def binding():
    values = {name: str(uuid.uuid4()) for name in ha.BINDING_FIELDS}
    values.update(version=1, engine_id="owned-engine", scope_prefix="cowork-owned-",
        container_id="a" * 64, container_name="cowork-owned-desktop-" + uuid.UUID(values["session_id"]).hex,
        created_at="2026-10-09T00:00:00.000000000Z", started_at="2026-10-09T00:00:01.000000000Z",
        workspace_id=None, workspace_volume="cowork-owned-run-" + uuid.UUID(values["run_id"]).hex)
    return values


class MemoryWire:
    def __init__(self, data, chunk=1):
        self.data, self.chunk, self.sent = bytearray(data), chunk, []
    def recv(self, timeout):
        if not self.data:
            return None
        data = bytes(self.data[:self.chunk])
        del self.data[:self.chunk]
        return 2, data
    def binary(self, data):
        self.sent.append(data)


def rfb_bytes(*, version=b"RFB 003.008\n", auth=b"\x00" * 4, name=b"Owned", side=b"", encoding=0,
              width=4, height=4, rectangles=1, regions=None):
    native = struct.pack("!BBBBHHHBBBxxx", 32, 24, 0, 1, 255, 255, 255, 16, 8, 0)
    initial = struct.pack("!HH", 640, 480) + native + struct.pack("!I", len(name)) + name
    if regions is not None:
        rectangles = len(regions)
    frame = b"\x00\x00" + struct.pack("!H", rectangles)
    if regions is not None:
        for x, y, w, h, enc in regions:
            frame += struct.pack("!HHHHi", x, y, w, h, enc) + b"\xa5" * w * h * 4
    elif rectangles == 1:
        frame += struct.pack("!HHHHi", 0, 0, width, height, encoding) + b"\xa5" * width * height * 4
    return version + b"\x01\x01" + auth + initial + side + frame



class PortObservationBoundaryTests(unittest.TestCase):
    def fixture(self):
        case = {"run": str(uuid.uuid4()), "session": str(uuid.uuid4()), "binding": binding()}
        job = case["binding"]
        physical = {"Id": job["container_id"], "Created": job["created_at"],
                    "State": {"Running": True, "StartedAt": job["started_at"]}}
        fixture = object.__new__(ha.LinuxFixture)
        fixture.runner = SimpleNamespace(inspect=Mock(return_value=physical), relay=None)
        fixture.run_lease = Mock(return_value={"live": True})
        fixture.observer = SimpleNamespace(count=Mock(return_value=2), transport_snapshot=Mock(return_value=[]))
        fixture.parent = SimpleNamespace(run=Mock(return_value=json.dumps({
            "5900": {"connected": True, "banner_matches": True},
            "5901": {"connected": True, "banner_matches": True}})))
        return fixture, case

    def test_actual_parent_run_text_contract_and_exact_identity(self):
        fixture, case = self.fixture()
        result = fixture.observe_failed_rfb(case, 2)
        self.assertTrue(result["exact_live_docker_identity"])
        self.assertEqual(set(result["ports"]), {"5900", "5901"})
        arguments = fixture.parent.run.call_args.args
        self.assertEqual(arguments[:5], ("exec", case["binding"]["container_id"], "python3", "-I", "-c"))
        compile(arguments[5], "<owned-port-probe>", "exec")
        self.assertIn("b'RFB 003.008\\n'", arguments[5])
        self.assertEqual(fixture.parent.run.call_args.kwargs["timeout"], 10)

    def test_changed_physical_identity_never_runs_port_probe(self):
        fixture, case = self.fixture()
        fixture.runner.inspect.return_value["Id"] = "b" * 64
        with self.assertRaisesRegex(ha.QualificationFailure, "exact_owned_identity_required"):
            fixture.observe_failed_rfb(case, 2)
        fixture.parent.run.assert_not_called()

    def test_unbounded_or_untyped_metadata_rejected(self):
        for text in ["x" * 4097, json.dumps({"5900": {"connected": True, "banner_matches": True, "private_payload": "secret"},
                                             "5901": {"connected": True, "banner_matches": True}})]:
            fixture, case = self.fixture()
            fixture.parent.run.return_value = text
            with self.subTest(text_length=len(text)), self.assertRaises(ha.QualificationFailure):
                fixture.observe_failed_rfb(case, 2)


class InputsTests(unittest.TestCase):
    def args(self, **changes):
        root = Path.cwd().resolve()
        return SimpleNamespace(**dict({"server_bin": root / "candidate", "expected_sha256": "a" * 64,
            "expected_runner_sha256": "b" * 64, "expected_migration": 58,
            "runner_image_id": "sha256:" + "c" * 64, "gui_image_id": "sha256:" + "d" * 64,
            "core_image_id": "sha256:" + "e" * 64, "docker_client": root / "docker.exe",
            "runner_binary": None, "linux_docker_cli": None, "expected_linux_docker_cli_sha256": None,
            "docker_context": "desktop-linux", "baseline": root / "original39.json", "baseline_count": 39,
            "report": root / "owned-report.json"}, **changes))

    def test_accepted_exact_candidate_matrix_has_no_external_actions(self):
        with patch.object(ha.socket, "socket", side_effect=AssertionError("no sockets")), \
                patch.object(ha, "load_runner_helpers", side_effect=AssertionError("no import")):
            inputs = ha.qualification_inputs(self.args())
        self.assertEqual(inputs["migration"], 58)
        self.assertEqual(inputs["core_image"], "sha256:" + "e" * 64)
        self.assertFalse(inputs["include_api_desktop_phase"])
        self.assertTrue(ha.qualification_inputs(self.args(include_api_desktop_phase=True))["include_api_desktop_phase"])

    def test_schema_hash_and_exact_image_gates(self):
        for changes in [{"expected_sha256": "a" * 63}, {"expected_runner_sha256": None},
                {"expected_runner_sha256": "B" * 64}, {"expected_migration": 57},
                {"expected_migration": -1}, {"runner_image_id": "latest"}, {"core_image_id": None},
                {"gui_image_id": "sha256:" + "d" * 64 + "\n"}]:
            with self.subTest(changes=changes), self.assertRaises(ha.QualificationFailure):
                ha.qualification_inputs(self.args(**changes))

    def test_native_runner_matrix_requires_independent_approved_cli(self):
        root = Path.cwd().resolve()
        native = {"runner_image_id": None, "runner_binary": root / "runner-elf",
            "linux_docker_cli": root / "docker-elf", "expected_linux_docker_cli_sha256": "f" * 64}
        value = ha.qualification_inputs(self.args(**native))
        self.assertEqual(value["runner_mode"], "native")
        self.assertIsNone(value["runner_image"])
        self.assertEqual(value["linux_docker_cli_sha"], "f" * 64)
        for changed in [{"runner_image_id": "sha256:" + "c" * 64}, {"linux_docker_cli": None},
                {"expected_linux_docker_cli_sha256": None}, {"expected_linux_docker_cli_sha256": "F" * 64},
                {"runner_binary": Path("runner")}, {"linux_docker_cli": Path("docker")}]:
            with self.subTest(changed=changed), self.assertRaises(ha.QualificationFailure):
                ha.qualification_inputs(self.args(**dict(native, **changed)))
        with self.assertRaises(ha.QualificationFailure):
            ha.qualification_inputs(self.args(linux_docker_cli=root / "docker", expected_linux_docker_cli_sha256="f" * 64))

    def test_native_hashes_and_elf_gates_precede_any_parent_command(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            runner, cli = root / "runner", root / "docker"
            runner.write_bytes(b"\x7fELFowned-native-runner")
            cli.write_bytes(b"\x7fELFowned-native-docker-cli")
            runner.chmod(0o700)
            cli.chmod(0o700)
            inputs = ha.qualification_inputs(self.args(runner_image_id=None, runner_binary=runner,
                linux_docker_cli=cli, expected_sha256="a" * 64,
                expected_runner_sha256=hashlib.sha256(runner.read_bytes()).hexdigest(),
                expected_linux_docker_cli_sha256=hashlib.sha256(cli.read_bytes()).hexdigest()))
            with patch.object(ha, "load_runner_helpers", side_effect=AssertionError("no parent/helper commands")), \
                    patch.object(ha.socket, "socket", side_effect=AssertionError("no sockets")):
                ha.validate_native_inputs(inputs)
                for key, code in [("runner_sha", "exact_approved_native_runner_binary"),
                                  ("linux_docker_cli_sha", "exact_approved_linux_docker_cli")]:
                    with self.subTest(key=key), self.assertRaisesRegex(ha.QualificationFailure, code):
                        ha.validate_native_inputs(dict(inputs, **{key: "0" * 64}))
            cli.write_bytes(b"not an ELF")
            with self.assertRaisesRegex(ha.QualificationFailure, "exact_approved_linux_docker_cli"):
                ha.validate_native_inputs(dict(inputs, linux_docker_cli_sha=hashlib.sha256(cli.read_bytes()).hexdigest()))

    def test_owned_native_copy_is_independently_rehashed(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            source = root / "source"
            source.write_bytes(b"\x7fELFowned-copy")
            source.chmod(0o700)
            digest = hashlib.sha256(source.read_bytes()).hexdigest()
            copied = ha.owned_native_copy(source, root / "copy", digest, "copy_gate")
            self.assertEqual(copied.read_bytes(), source.read_bytes())
            with self.assertRaisesRegex(ha.QualificationFailure, "copy_gate"):
                ha.owned_native_copy(source, root / "wrong-copy", "0" * 64, "copy_gate")

    def test_original39_parent_context_and_absolute_paths_are_required(self):
        for changes in [{"baseline_count": 38}, {"baseline_count": 40}, {"docker_context": "default"},
                {"docker_context": "desktop-linux\n"}, {"docker_client": Path("docker")},
                {"baseline": Path("original39.json")}]:
            with self.subTest(changes=changes), self.assertRaises(ha.QualificationFailure):
                ha.qualification_inputs(self.args(**changes))


class ProtocolTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.helpers, _ = ha.load_runner_helpers()  # Module import only; no main/services.

    def test_binding_order_matches_actual_runner_producer_and_digest(self):
        value = binding()
        reversed_value = dict(reversed(list(value.items())))
        self.assertEqual(ha.binding_bytes(reversed_value), self.helpers.canonical_binding_bytes(value))
        self.assertEqual(ha.binding_digest(reversed_value), self.helpers.binding_digest(value))

    def test_missing_extra_and_changed_physical_binding_never_compare_equal(self):
        value = binding()
        for changed in [dict(value, unexpected=True), {name: item for name, item in value.items() if name != "attempt_id"}]:
            with self.assertRaises(ha.QualificationFailure):
                ha.binding_bytes(changed)
        self.assertNotEqual(ha.binding_digest(value), ha.binding_digest(dict(value, container_id="b" * 64)))

    def test_model_requests_one_normalized_relative_gui_path(self):
        scenario = ha.ModelScenario()
        self.assertFalse(scenario.ready.is_set())
        message = ha.model_tool_message(scenario.id)
        self.assertEqual(len(message["tool_calls"]), 1)
        call = message["tool_calls"][0]["function"]
        self.assertEqual(call["name"], "DesktopOpenOffice")
        path = json.loads(call["arguments"])["path"]
        self.assertFalse(path.startswith("/"))
        self.assertNotIn("..", path.split("/"))
        self.assertEqual(path, "artifacts/ha-fixture-" + uuid.UUID(scenario.id).hex + ".txt")

    def stop_reply(self):
        job = binding()
        path = "/v1/desktop-sessions/" + job["session_id"] + "?binding_sha256=" + ha.binding_digest(job)
        response = json.dumps({"version": 1, "binding": job, "execution_stopped": True}, separators=(",", ":")).encode()
        fields = ["DELETE", path, "1791504000", uuid.uuid4().hex]
        key = b"own-pure-test-signing-key"
        proof = self.helpers.response_mac(key, self.helpers.DESKTOP_STOP_DOMAIN, fields, b"", response)
        protocol = self.helpers.RunnerProtocol.__new__(self.helpers.RunnerProtocol)
        protocol.signing_key = key
        return protocol, path, {"x-cowork-timestamp": fields[2], "x-cowork-nonce": fields[3]}, response, proof

    def test_response_loss_requires_real_mac_and_exact_binding_query_first(self):
        protocol, path, headers, body, proof = self.stop_reply()
        ha.assert_real_stop_reply(protocol, "DELETE", path, headers, [("x-cowork-desktop-proof", proof)], body)
        for changed_path, changed_headers, changed_body, proofs in [
                (path + "&extra=1", headers, body, [proof]),
                (path, dict(headers, **{"x-cowork-nonce": "other"}), body, [proof]),
                (path, headers, body + b" ", [proof]), (path, headers, body, []),
                (path, headers, body, [proof, proof]), (path, headers, body, ["v1:" + "0" * 64])]:
            with self.subTest(path=changed_path), self.assertRaises(RuntimeError):
                ha.assert_real_stop_reply(protocol, "DELETE", changed_path, changed_headers,
                    [("x-cowork-desktop-proof", item) for item in proofs], changed_body)

    def test_mac_valid_but_non_stop_reply_is_not_a_confirmed_physical_stop(self):
        protocol, path, headers, body, _ = self.stop_reply()
        value = json.loads(body)
        value["execution_stopped"] = False
        body = json.dumps(value, separators=(",", ":")).encode()
        proof = self.helpers.response_mac(protocol.signing_key, self.helpers.DESKTOP_STOP_DOMAIN,
            ["DELETE", path, headers["x-cowork-timestamp"], headers["x-cowork-nonce"]], b"", body)
        with self.assertRaises(ha.QualificationFailure):
            ha.assert_real_stop_reply(protocol, "DELETE", path, headers, [("x-cowork-desktop-proof", proof)], body)


class PublicApiDesktopTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.helpers, _ = ha.load_runner_helpers()

    def creation_reply(self):
        job = binding()
        dimensions = {"width": 640, "height": 480, "scale_factor": 1.0}
        spec = {"schema_version": 2, "run_id": job["run_id"], "session_id": job["session_id"],
                "workspace_id": job["workspace_id"], "dimensions": dimensions}
        request = json.dumps(spec, separators=(",", ":")).encode()
        response = json.dumps(dict(spec, container_name=job["container_name"], workspace_volume=job["workspace_volume"]), separators=(",", ":")).encode()
        raw = json.dumps({"binding": job, "running": True, "paused": False, "restarting": False}, separators=(",", ":")).encode()
        path = "/v1/desktop-sessions?lease_token=" + job["lease_token"] + "&attempt_id=" + job["attempt_id"]
        fields = ["POST", path, "1791504000", uuid.uuid4().hex]
        key = b"private-pure-test-creation-key"
        protocol = self.helpers.RunnerProtocol.__new__(self.helpers.RunnerProtocol)
        protocol.signing_key, protocol.expected_engine = key, job["engine_id"]
        proof = self.helpers.response_mac(key, self.helpers.CREATE_DOMAIN, fields, request, response, raw)
        headers = [("x-cowork-desktop-binding", base64.urlsafe_b64encode(raw).decode().rstrip("=")),
                   ("x-cowork-desktop-proof", proof)]
        return protocol, job, path, {"x-cowork-timestamp": fields[2], "x-cowork-nonce": fields[3]}, request, headers, response

    def test_actual_creation_mac_binds_original_body_full_query_and_physical_receipt(self):
        protocol, job, path, request_headers, request, headers, response = self.creation_reply()
        self.assertEqual(ha.assert_real_creation_reply(protocol, "POST", path, request_headers, request, headers, response), job)
        for changes in [{"path": path + "&extra=1"}, {"path": path.replace(job["lease_token"], str(uuid.uuid4()))},
                {"request_headers": dict(request_headers, **{"x-cowork-nonce": "changed"})},
                {"request": request + b" "}, {"response": response + b" "},
                {"headers": headers[:1]}, {"headers": headers + [headers[1]]},
                {"headers": [headers[0], ("x-cowork-desktop-proof", "v1:" + "0" * 64)]}]:
            arguments = dict(path=path, request_headers=request_headers, request=request, response_headers=headers, response=response)
            arguments.update({"response_headers" if name == "headers" else name: value for name, value in changes.items()})
            with self.subTest(changes=changes), self.assertRaises((RuntimeError, ha.QualificationFailure)):
                ha.assert_real_creation_reply(protocol, "POST", **arguments)

    def api_fixture(self):
        fixture = ha.LinuxFixture.__new__(ha.LinuxFixture)
        old, current = binding(), binding()
        workspace = str(uuid.uuid4())
        old["workspace_id"] = workspace
        current.update(run_id=old["run_id"], lease_token=old["lease_token"], workspace_id=workspace, container_id="b" * 64)
        dimensions = {"width": 640, "height": 480, "scale_factor": 1.0}
        lease = {"state": "running", "live": True, "owner": str(uuid.uuid4()), "token": old["lease_token"],
                 "workspace": workspace, "expiry": 1000}
        fixture.run_lease = Mock(return_value=lease)
        fixture.group, fixture.node_ids = str(uuid.uuid4()), [str(uuid.uuid4()) for _ in range(3)]
        epoch = str(uuid.uuid4())
        fixture.epoch = Mock(return_value=epoch)
        fixture.desktop_owner = Mock(return_value={"group_id": fixture.group, "node_id": fixture.node_ids[0],
            "process_epoch": epoch, "executor_id": None, "connection_epoch": None, "credential_id": None})
        fixture.api = Mock(return_value={"id": current["session_id"], "run_id": old["run_id"],
            "state": "agent_controlled", "dimensions": dimensions, "stream_protocol": "rfb.binary.v1"})
        fixture.sql = Mock(return_value=json.dumps({"state": "agent_controlled", "pending": False,
            "binding": current, "lease": old["lease_token"], "attempt": current["attempt_id"]}))
        fixture.observer = SimpleNamespace(lock=ha.threading.Lock(), verified_creations={current["session_id"]: current})
        fixture.require_identity, fixture.no_replay = Mock(), Mock()
        worker_case = {"run": old["run_id"], "session": old["session_id"], "binding": old, "scenario": ha.ModelScenario()}
        return fixture, worker_case, current

    def test_public_create_posts_only_dimensions_and_requires_genuine_receipt_creator_owner(self):
        fixture, worker_case, current = self.api_fixture()
        case = fixture.create_api_desktop(worker_case)
        fixture.api.assert_called_once_with(0, f"/runs/{worker_case['run']}/desktop-sessions", method="POST",
            expected=201, data={"width": 640, "height": 480})
        self.assertEqual(case["binding"], current)
        self.assertEqual(case["scenario"], worker_case["scenario"])
        fixture.require_identity.assert_called_once_with(case)
        fixture.no_replay.assert_called_once_with(case)

    def test_missing_or_changed_upstream_receipt_or_wrong_creator_cannot_qualify(self):
        for change in ["missing_receipt", "changed_receipt", "wrong_creator", "expired_lease", "unknown_http"]:
            fixture, worker_case, current = self.api_fixture()
            if change == "missing_receipt":
                fixture.observer.verified_creations.clear()
            elif change == "changed_receipt":
                fixture.observer.verified_creations[current["session_id"]] = dict(current, engine_id="forged_engine")
            elif change == "wrong_creator":
                fixture.desktop_owner.return_value["node_id"] = fixture.node_ids[1]
            elif change == "expired_lease":
                fixture.run_lease.side_effect = [fixture.run_lease.return_value, dict(fixture.run_lease.return_value, live=False)]
            else:
                fixture.api.side_effect = ha.QualificationFailure("owned_cluster_authenticated_api_response")
            with self.subTest(change=change), self.assertRaises(ha.QualificationFailure):
                fixture.create_api_desktop(worker_case)
            self.assertEqual(fixture.api.call_count, 1)
            fixture.require_identity.assert_not_called()

    def test_control_requires_public_reauthentication_purpose_and_one_use_grant(self):
        fixture = ha.LinuxFixture.__new__(ha.LinuxFixture)
        fixture.admin_password = "private-pure-test-password"
        grant, ticket = {"token": "private-reauth-grant"}, {"token": "private-stream-ticket", "control": True}
        fixture.api = Mock(side_effect=[grant, ticket])
        fixture.websocket = Mock()
        case = {"run": str(uuid.uuid4()), "session": str(uuid.uuid4())}
        wire, actual_ticket, actual_grant = fixture.control_stream(case, 2)
        self.assertEqual(actual_ticket, ticket)
        self.assertEqual(actual_grant, grant)
        self.assertEqual(wire, fixture.websocket.return_value)
        self.assertEqual(fixture.api.call_args_list[0].args, (2, "/auth/reauthenticate"))
        self.assertEqual(fixture.api.call_args_list[0].kwargs["data"],
            {"password": fixture.admin_password, "purpose": "desktop_control"})
        self.assertEqual(fixture.api.call_args_list[1].kwargs["data"],
            {"control": True, "reauthentication_token": grant["token"]})
        fixture.websocket.assert_called_once_with(2,
            f"/api/v1/desktop-sessions/{case['session']}/stream?ticket={ticket['token']}")

    def test_control_refusal_never_opens_or_retries_physical_stream(self):
        case = {"run": str(uuid.uuid4()), "session": str(uuid.uuid4())}
        for values, expected_calls in [(ha.QualificationFailure("reauth_refused"), 1),
                ([{"token": "grant"}, ha.QualificationFailure("grant_refused")], 2),
                ([{"token": "grant"}, {"token": "ticket", "control": False}], 2)]:
            fixture = ha.LinuxFixture.__new__(ha.LinuxFixture)
            fixture.admin_password, fixture.api, fixture.websocket = "private-pure-password", Mock(side_effect=values), Mock()
            with self.subTest(expected_calls=expected_calls), self.assertRaises(ha.QualificationFailure):
                fixture.control_stream(case)
            self.assertEqual(fixture.api.call_count, expected_calls)
            fixture.websocket.assert_not_called()

    def pointer_fixture(self):
        fixture = ha.LinuxFixture.__new__(ha.LinuxFixture)
        job = binding()
        case = {"binding": job, "dimensions": {"width": 640, "height": 480}}
        fixture.runner = SimpleNamespace(inspect=Mock(return_value={"Id": job["container_id"], "Created": job["created_at"],
            "State": {"Running": True, "StartedAt": job["started_at"]}}))
        fixture.parent = SimpleNamespace(binary=Mock(return_value=b'{"x":37,"y":41}'))
        return fixture, case

    def test_pointer_observation_is_exact_owned_id_unprivileged_and_bounded(self):
        fixture, case = self.pointer_fixture()
        self.assertEqual(fixture.pointer_position(case), {"x": 37, "y": 41})
        fixture.runner.inspect.assert_called_once_with(case["binding"])
        fixture.parent.binary.assert_called_once_with("exec", "--user", "10001:10001", case["binding"]["container_id"],
            "python3", "-c", ha.X11_POINTER_QUERY, timeout=10)
        compile(ha.X11_POINTER_QUERY, "owned-x11-query", "exec")
        for body in [b'{"x":true,"y":41}', b'{"x":640,"y":41}', b'{"x":1,"y":480}',
                b'{"x":1,"y":1,"foreign":0}', b" " * 129]:
            fixture.parent.binary.return_value = body
            with self.subTest(body=body), self.assertRaises(ha.QualificationFailure):
                fixture.pointer_position(case)

    def test_physical_pointer_query_refuses_changed_id_or_started_at_before_exec(self):
        for change in ["id", "started"]:
            fixture, case = self.pointer_fixture()
            physical = fixture.runner.inspect.return_value
            if change == "id": physical["Id"] = "b" * 64
            else: physical["State"]["StartedAt"] = "changed"
            with self.subTest(change=change), self.assertRaises(ha.QualificationFailure):
                fixture.pointer_position(case)
            fixture.parent.binary.assert_not_called()

    def test_owner_failover_requires_same_live_worker_identity_and_real_renewal(self):
        before = {"state": "running", "live": True, "owner": str(uuid.uuid4()), "token": str(uuid.uuid4()),
                  "workspace": str(uuid.uuid4()), "expiry": 1000}
        ha.same_live_worker_lease(before, dict(before, expiry=1001), renewed=True)
        for changes in [{"owner": str(uuid.uuid4())}, {"token": str(uuid.uuid4())}, {"workspace": str(uuid.uuid4())},
                {"live": False}, {"state": "interrupted"}, {"expiry": 1000}, {"expiry": 999}]:
            with self.subTest(changes=changes), self.assertRaises(ha.QualificationFailure):
                ha.same_live_worker_lease(before, dict(before, **changes), renewed=True)

    def test_failed_optional_phase_restores_observer_mode_without_recording_success(self):
        fixture = SimpleNamespace(observer=SimpleNamespace(verify_creations=False),
            real_phase=Mock(return_value=nullcontext()),
            create_worker_desktop=Mock(side_effect=ha.QualificationFailure("owned_worker_fixture_failure")))
        passed = Mock()
        with self.assertRaisesRegex(ha.QualificationFailure, "owned_worker_fixture_failure"):
            ha.qualify_api_desktop_phase(fixture, passed)
        self.assertFalse(fixture.observer.verify_creations)
        passed.assert_not_called()


class RfbTests(unittest.TestCase):
    def test_fragmented_real_rfb_negotiation_and_requested_pixels(self):
        wire = MemoryWire(rfb_bytes())
        self.assertEqual(ha.real_framebuffer(wire), 64)
        self.assertEqual(wire.real_rfb_dimensions, (640, 480))
        self.assertEqual(wire.sent[:3], [b"RFB 003.008\n", b"\x01", b"\x01"])
        self.assertEqual(wire.sent[-1], struct.pack("!BBHHHH", 3, 0, 0, 0, 4, 4))

    def test_bell_and_bounded_clipboard_do_not_replace_actual_pixels(self):
        side = b"\x02\x03" + b"\x00" * 3 + struct.pack("!I", 5) + b"Owned"
        self.assertEqual(ha.real_framebuffer(MemoryWire(rfb_bytes(side=side), chunk=19)), 64)

    def test_wrong_version_security_result_encoding_and_region_refused(self):
        for changes in [{"version": b"RFB 003.003\n"}, {"auth": b"\0\0\0\1"},
                {"encoding": 1}, {"width": 641}, {"rectangles": 0}, {"rectangles": 17}]:
            with self.subTest(changes=changes), self.assertRaises(ha.QualificationFailure):
                ha.real_framebuffer(MemoryWire(rfb_bytes(**changes)))

    def test_no_pixels_truncated_transport_and_clipboard_bounds_refused(self):
        oversized_cut = b"\x03" + b"\x00" * 3 + struct.pack("!I", 65537)
        for data in [rfb_bytes()[:-1], rfb_bytes(side=oversized_cut), rfb_bytes(side=b"\x02" * 8)]:
            with self.subTest(size=len(data)), self.assertRaises(ha.QualificationFailure):
                ha.real_framebuffer(MemoryWire(data))

    def test_read_and_binary_message_bounds(self):
        with self.assertRaises(ha.QualificationFailure):
            ha.RfbReader(MemoryWire(b"")).take(65537)
        wire = Mock()
        wire.recv.return_value = (1, b"not binary")
        with self.assertRaises(ha.QualificationFailure):
            ha.RfbReader(wire).take(1)

    def test_numeric_diagnostics_preserve_exact_handshake_and_message_offsets(self):
        wire = MemoryWire(rfb_bytes(), chunk=7)
        self.assertEqual(ha.real_framebuffer(wire), 64)
        proof = ha.rfb_report([wire])[0]
        self.assertEqual(proof["handshake_step"], 8)
        self.assertEqual(proof["server_version"], [3, 8])
        self.assertEqual(proof["security_types"], [1])
        self.assertEqual(proof["security_result"], 0)
        self.assertEqual(proof["native_pixel_format"]["bits_per_pixel"], 32)
        self.assertEqual(proof["requested_pixel_format"], proof["native_pixel_format"])
        self.assertEqual(proof["client_messages"], [
            {"message_type": 0, "bytes": 20}, {"message_type": 2, "bytes": 8},
            {"message_type": 3, "bytes": 10}])
        self.assertEqual(proof["update_header"],
            {"message_type": 0, "padding": 0, "rectangle_count": 1, "byte_offset": 47})
        self.assertEqual(proof["rectangles"], [
            {"x": 0, "y": 0, "width": 4, "height": 4, "encoding": 0, "byte_offset": 51}])
        self.assertEqual(proof["bytes_consumed"], 127)
        self.assertEqual(proof["pixel_bytes_received"], 64)

    def test_failed_rectangle_retains_header_but_never_accepts_or_consumes_pixels(self):
        for changes, expected in [({"width": 641}, {"width": 641, "encoding": 0}),
                ({"encoding": -239}, {"width": 4, "encoding": -239})]:
            wire = MemoryWire(rfb_bytes(**changes), chunk=31)
            with self.subTest(changes=changes), self.assertRaisesRegex(
                    ha.QualificationFailure, "genuine_rfb_raw_requested_rectangle"):
                ha.real_framebuffer(wire)
            proof = ha.rfb_report([wire])[0]
            self.assertEqual(proof["handshake_step"], 6)
            self.assertEqual(proof["bytes_consumed"], 63)
            self.assertEqual(proof["pixel_bytes_received"], 0)
            self.assertEqual(proof["rectangles"][0]["byte_offset"], 51)
            for field, value in expected.items():
                self.assertEqual(proof["rectangles"][0][field], value)

    def test_multiple_rectangles_and_side_messages_keep_real_byte_offsets(self):
        side = b"\x02\x03" + b"\x00" * 3 + struct.pack("!I", 5) + b"Owned"
        wire = MemoryWire(rfb_bytes(side=side, regions=[(0, 0, 2, 4, 0), (2, 0, 2, 4, 0)]), chunk=3)
        self.assertEqual(ha.real_framebuffer(wire), 64)
        proof = ha.rfb_report([wire])[0]
        self.assertEqual(proof["side_message_types"], [2, 3])
        self.assertEqual(proof["update_header"]["byte_offset"], 61)
        self.assertEqual([value["byte_offset"] for value in proof["rectangles"]], [65, 109])
        self.assertEqual(proof["bytes_consumed"], 153)

    def test_report_excludes_all_desktop_clipboard_server_name_and_wire_secrets(self):
        marker = b"PRIVATE-CONTENT-MUST-NOT-APPEAR"
        side = b"\x03" + b"\x00" * 3 + struct.pack("!I", len(marker)) + marker
        wire = MemoryWire(rfb_bytes(name=marker, side=side), chunk=1)
        wire.secret = marker
        ha.real_framebuffer(wire)
        proof = ha.rfb_report([SimpleNamespace(secret=marker), wire])
        encoded = json.dumps(proof)
        self.assertNotIn(marker.decode(), encoded)
        self.assertNotIn("secret", encoded)
        self.assertEqual(proof[0]["stream_index"], 1)
        self.assertEqual(proof[0]["server_name_bytes"], len(marker))
        def numeric_leaves(value):
            if isinstance(value, dict):
                return all(numeric_leaves(child) for child in value.values())
            if isinstance(value, list):
                return all(numeric_leaves(child) for child in value)
            return type(value) is int
        self.assertTrue(numeric_leaves(proof))


    def test_measured_requested_pixels_plus_other_in_bounds_rectangles(self):
        wire = MemoryWire(rfb_bytes(regions=[(0, 0, 4, 4, 0), (4, 0, 14, 4, 0)]), chunk=7)
        self.assertEqual(ha.real_framebuffer(wire), 288)
        self.assertEqual(wire.rfb_protocol["requested_pixels_received"], 16)
        self.assertEqual(wire.rfb_protocol["pixel_bytes_received"], 288)
        self.assertEqual(wire.rfb_protocol["handshake_step"], 8)

    def test_overlapping_payload_cannot_mint_missing_requested_coverage(self):
        for regions in [[(0, 0, 2, 4, 0), (0, 0, 2, 4, 0)], [(4, 0, 4, 4, 0)]]:
            wire = MemoryWire(rfb_bytes(regions=regions), chunk=11)
            with self.subTest(regions=regions), self.assertRaisesRegex(
                    ha.QualificationFailure, "genuine_rfb_complete_requested_pixels"):
                ha.real_framebuffer(wire)
            self.assertLess(wire.rfb_protocol["requested_pixels_received"], 16)

    def test_large_valid_raw_rectangle_is_consumed_in_bounded_fragments(self):
        wire = MemoryWire(rfb_bytes(regions=[(0, 0, 640, 480, 0)]), chunk=32768)
        self.assertEqual(ha.real_framebuffer(wire), 640 * 480 * 4)
        self.assertEqual(wire.rfb_protocol["requested_pixels_received"], 16)

    def test_out_of_frame_or_empty_rectangles_refused_before_any_payload(self):
        for region in [(640, 0, 1, 4, 0), (0, 480, 4, 1, 0), (0, 0, 0, 4, 0), (0, 0, 4, 0, 0)]:
            wire = MemoryWire(rfb_bytes(regions=[region]), chunk=1)
            with self.subTest(region=region), self.assertRaisesRegex(
                    ha.QualificationFailure, "genuine_rfb_raw_requested_rectangle"):
                ha.real_framebuffer(wire)
            self.assertEqual(wire.rfb_protocol["pixel_bytes_received"], 0)

    def test_total_pixel_budget_refuses_many_valid_but_excessive_rectangles(self):
        regions = [(0, 0, 640, 480, 0)] * 7
        wire = MemoryWire(rfb_bytes(regions=regions), chunk=65536)
        with self.assertRaisesRegex(ha.QualificationFailure, "genuine_rfb_pixel_payload_bound"):
            ha.real_framebuffer(wire)
        self.assertEqual(wire.rfb_protocol["pixel_bytes_received"], 6 * 640 * 480 * 4)


class CleanupTests(unittest.TestCase):
    def fixture(self):
        fixture = ha.LinuxFixture.__new__(ha.LinuxFixture)
        fixture.parent = SimpleNamespace(assert_preserved=Mock())
        fixture.wire_sockets, fixture.streams, fixture.scenarios, fixture.owned_runs = [], [], [], []
        fixture.cleanup_failures = []
        fixture.nodes = {0: Mock()}
        fixture.nodes[0].poll.return_value = None
        fixture.api = Mock(return_value={"state": "canceled"})
        fixture.until = lambda condition, name, seconds: condition()
        return fixture

    def test_phase_cleanup_releases_barrier_and_closes_all_owned_streams_after_failure(self):
        fixture = self.fixture()
        wire, stream, scenario, run = Mock(), Mock(), ha.ModelScenario(), str(uuid.uuid4())
        with self.assertRaisesRegex(ValueError, "original failure"):
            with fixture.real_phase("owned"):
                fixture.wire_sockets.append(wire)
                fixture.streams.append(stream)
                fixture.scenarios.append(scenario)
                fixture.owned_runs.append(run)
                raise ValueError("original failure")
        wire.close.assert_called_once()
        stream.close.assert_called_once()
        self.assertTrue(scenario.ready.is_set() and scenario.release.is_set())
        fixture.api.assert_any_call(0, f"/runs/{run}/cancel", method="POST", record_failure=False)
        self.assertEqual(fixture.parent.assert_preserved.call_count, 2)

    def test_cleanup_error_is_recorded_and_does_not_hide_original_failure(self):
        fixture = self.fixture()
        wire, stream = Mock(), Mock()
        wire.close.side_effect = OSError()
        with self.assertRaisesRegex(ValueError, "first"):
            with fixture.real_phase("owned"):
                fixture.wire_sockets.append(wire)
                fixture.streams.append(stream)
                raise ValueError("first")
        stream.close.assert_called_once()
        self.assertEqual(fixture.cleanup_failures, ["owned_phase_wire_cleanup"])

    def test_cleanup_failure_prevents_successful_phase(self):
        fixture = self.fixture()
        wire = Mock()
        wire.close.side_effect = OSError()
        with self.assertRaises(ha.QualificationFailure):
            with fixture.real_phase("owned"):
                fixture.wire_sockets.append(wire)

    def test_native_rss_reads_only_the_given_live_child(self):
        child = Mock(pid=987654)
        child.poll.return_value = None
        with patch.object(ha.Path, "read_text", return_value="Name:\towned\nVmRSS:\t4096 kB\n") as read:
            self.assertEqual(ha.native_rss(child), 4096 * 1024)
        read.assert_called_once()
        child.poll.return_value = 1
        with patch.object(ha.Path, "read_text") as read, self.assertRaises(ha.QualificationFailure):
            ha.native_rss(child)
        read.assert_not_called()


class RetentionTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.runner_helpers, _ = ha.load_runner_helpers()

    def helpers(self):
        # Windows exercises the same retention/scope checks without pretending
        # to support native execution or Unix-only O_NOFOLLOW.
        def private_root(parent):
            root = parent / str(uuid.uuid4())
            root.mkdir(mode=0o700)
            return root
        def write_private(path, content):
            with path.open("xb") as stream:
                stream.write(content.encode() if isinstance(content, str) else content)
            path.chmod(0o600)
        return SimpleNamespace(private_root=private_root, write_private=write_private,
                               public_failure=self.runner_helpers.public_failure)

    def root(self, parent):
        return ha.QualificationRoot(self.helpers(), parent)

    @staticmethod
    def proofs():
        return {key: True for key in ha.ROOT_CLEANUP_KEYS}

    def test_only_known_fixture_kinds_are_validated_before_any_directory_creation(self):
        helpers = self.helpers()
        helpers.private_root = Mock()
        for kind in ("unknown", None, [], {}, 1):
            with self.subTest(kind=kind), self.assertRaisesRegex(ha.QualificationFailure, "kind_not_allowed"):
                ha.QualificationRoot(helpers, kind=kind)
        helpers.private_root.assert_not_called()

    def test_resume_identity_is_written_once_and_memory_change_does_not_authorize_deletion(self):
        with tempfile.TemporaryDirectory() as directory:
            storage = ha.QualificationRoot(self.helpers(), Path(directory).resolve(), kind="server_resume")
            marker = storage.path / "fixture.json"
            self.assertEqual(json.loads(marker.read_text())["kind"], "server_resume")
            with self.assertRaises(FileExistsError):
                storage.helpers.write_private(marker, "{}")
            storage.identity["kind"] = "linux_desktop_ha"
            with self.assertRaisesRegex(ha.QualificationFailure, "identity_mismatch"):
                storage.finish(True, self.proofs())
            self.assertTrue(storage.path.is_dir())

    def test_failed_context_and_catchable_interrupt_retain_private_identity_and_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            parent = Path(directory).resolve()
            for error in (ValueError("failed qualification"), ha.QualificationFailure("owned_linux_desktop_ha_interrupted")):
                storage = self.root(parent)
                ledger, key = storage.path / "runner-ledger.json", storage.path / "private-key.pem"
                ledger.write_bytes(b'{"operator_only":"pending_creation"}')
                key.write_bytes(b"private fixture key")
                with self.assertRaises(type(error)):
                    try:
                        raise error
                    finally:
                        self.assertEqual(storage.finish(False, self.proofs()), "retained_failed_qualification")
                self.assertEqual(json.loads((storage.path / "fixture.json").read_text()), storage.identity)
                self.assertEqual(ledger.read_bytes(), b'{"operator_only":"pending_creation"}')
                self.assertEqual(key.read_bytes(), b"private fixture key")

    def test_successful_cleanup_deletes_exact_uuid_root_only(self):
        with tempfile.TemporaryDirectory() as directory:
            parent = Path(directory).resolve()
            sibling = parent / str(uuid.uuid4())
            sibling.mkdir()
            sibling_marker = sibling / "must-remain"
            sibling_marker.write_text("another fixture")
            storage = self.root(parent)
            root = storage.path
            self.assertEqual(storage.finish(True, self.proofs()), "removed_after_confirmed_success")
            self.assertFalse(root.exists())
            self.assertEqual(sibling_marker.read_text(), "another fixture")
            self.assertTrue(parent.is_dir())

    def test_every_cleanup_proof_is_required_before_successful_root_removal(self):
        with tempfile.TemporaryDirectory() as directory:
            storage = self.root(Path(directory).resolve())
            invalid = [dict(self.proofs(), **{name: False}) for name in ha.ROOT_CLEANUP_KEYS]
            invalid += [{key: True for key in ha.ROOT_CLEANUP_KEYS if key != "archive"},
                        dict(self.proofs(), unrecognized=True), dict(self.proofs(), native=1)]
            for proofs in invalid:
                with self.subTest(proofs=proofs), self.assertRaises(ha.QualificationFailure):
                    storage.finish(True, proofs)
                self.assertTrue(storage.path.is_dir())

    def test_changed_identity_and_out_of_scope_root_refuse_recursive_deletion(self):
        with tempfile.TemporaryDirectory() as directory:
            storage = self.root(Path(directory).resolve())
            marker = storage.path / "fixture.json"
            marker.write_text(json.dumps(dict(storage.identity, scope=str(uuid.uuid4()))))
            with self.assertRaisesRegex(ha.QualificationFailure, "identity_mismatch"):
                storage.finish(True, self.proofs())
            self.assertTrue(storage.path.is_dir())
            marker.write_text(json.dumps(storage.identity))
            original = storage.path
            storage.path = storage.parent
            with self.assertRaisesRegex(ha.QualificationFailure, "scope_mismatch"):
                storage.finish(True, self.proofs())
            self.assertTrue(original.is_dir())

    @unittest.skipUnless(os.name == "posix", "Unix owner/permission and socket path constraints")
    def test_real_private_root_is_owner_only_and_short_before_mutations(self):
        with tempfile.TemporaryDirectory() as directory:
            parent = Path(directory).resolve() / "h"
            storage = ha.QualificationRoot(self.runner_helpers, parent)
            self.assertEqual(storage.path.stat().st_uid, os.geteuid())
            self.assertEqual(storage.path.stat().st_mode & 0o777, 0o700)
            self.assertEqual((storage.path / "fixture.json").stat().st_mode & 0o777, 0o600)
            self.assertLess(len(os.fsencode(storage.path / "r/docker.sock")), 104)
            with self.assertRaisesRegex(ha.QualificationFailure, "unix_socket_path_bound"):
                ha.QualificationRoot(self.runner_helpers, parent / ("long" * 30))
            self.assertFalse((parent / ("long" * 30)).exists())

    def runner(self, storage):
        root = storage.path / "r"
        root.mkdir(mode=0o700)
        return SimpleNamespace(root=root, native_binary=None, cleanup=Mock(), _stop_native=Mock(),
                               container=None, relay=None)

    def parent(self):
        return SimpleNamespace(assert_preserved=Mock(), containers={}, run=Mock(),
            owned_inventory=Mock(return_value={"containers": [], "networks": [], "volumes": [], "pending_creates": []}))

    def test_partial_runner_start_cleanup_is_signal_deferred_and_failed_root_retained(self):
        with tempfile.TemporaryDirectory() as directory:
            storage = self.root(Path(directory).resolve())
            runner, parent = self.runner(storage), self.parent()
            with patch.object(ha.signal, "signal") as signals:
                proofs, failures, inventory = ha.cleanup_owned_fixtures(None, runner, parent, storage.helpers, storage)
            runner.cleanup.assert_called_once()
            parent.assert_preserved.assert_called_once()
            parent.owned_inventory.assert_called_once_with(storage.scope)
            self.assertTrue(all(proofs.values()) and not failures and not any(inventory.values()))
            self.assertEqual(signals.call_count, len(ha.HANDLED_SIGNALS))
            self.assertTrue(all(call.args[1] == ha.signal.SIG_IGN for call in signals.call_args_list))
            self.assertEqual(storage.finish(False, proofs), "retained_failed_qualification")
            self.assertTrue(storage.path.is_dir())

    def test_failed_runner_cleanup_retains_archived_keys_and_typed_unknown_outcome(self):
        with tempfile.TemporaryDirectory() as directory:
            storage = self.root(Path(directory).resolve())
            runner, parent = self.runner(storage), self.parent()
            (runner.root / "ca-key.pem").write_bytes(b"private fixture authority")
            runner.cleanup.side_effect = self.runner_helpers.FixtureCommandError("docker.container.remove", "command_timeout", None)
            parent.owned_inventory.return_value["pending_creates"].append({"kind": "container", "scope": storage.scope})
            with patch.object(ha.signal, "signal"):
                proofs, failures, inventory = ha.cleanup_owned_fixtures(None, runner, parent, storage.helpers, storage)
            self.assertFalse(proofs["runner"] or proofs["inventory"])
            detail = next(value for value in failures if value["cleanup_action"] == "runner_fixture")
            self.assertEqual(detail["classification"], "unknown_command_outcome")
            self.assertEqual(detail["action"], "docker.container.remove")
            self.assertNotIn("private fixture authority", json.dumps(failures))
            self.assertEqual((storage.path / "evidence/ca-key.pem").read_bytes(), b"private fixture authority")
            self.assertTrue(inventory["pending_creates"])
            storage.finish(False, proofs)
            self.assertTrue((storage.path / "evidence/ca-key.pem").is_file())

    def test_archive_failure_stops_execution_without_erasing_original_runner_state(self):
        with tempfile.TemporaryDirectory() as directory:
            storage = self.root(Path(directory).resolve())
            runner, parent = self.runner(storage), self.parent()
            runner.native_binary = storage.path / "runner"
            runner.ledger = Mock(side_effect=ValueError("raw private journal content"))
            (runner.root / "server-key.pem").write_bytes(b"private fixture key")
            with patch.object(ha.signal, "signal"):
                proofs, failures, _ = ha.cleanup_owned_fixtures(None, runner, parent, storage.helpers, storage)
            self.assertFalse(proofs["archive"] or proofs["runner"])
            runner.cleanup.assert_not_called()
            runner._stop_native.assert_called_once()
            self.assertEqual((runner.root / "server-key.pem").read_bytes(), b"private fixture key")
            self.assertNotIn("raw private journal content", json.dumps(failures))

    def test_native_cleanup_failure_does_not_skip_exact_runner_cleanup(self):
        with tempfile.TemporaryDirectory() as directory:
            storage = self.root(Path(directory).resolve())
            runner, parent = self.runner(storage), self.parent()
            fixture = SimpleNamespace(cleanup=Mock(side_effect=ha.QualificationFailure("owned_proxy_cleanup")))
            with patch.object(ha.signal, "signal"):
                proofs, failures, _ = ha.cleanup_owned_fixtures(fixture, runner, parent, storage.helpers, storage)
            self.assertFalse(proofs["native"])
            self.assertTrue(proofs["runner"])
            runner.cleanup.assert_called_once()
            self.assertEqual(failures[0]["code"], "owned_proxy_cleanup")

    def run_main_without_services(self, directory, *, interrupted):
        root = Path(directory).resolve()
        source, baseline, report = root / "server", root / "baseline.json", root / "public-report.json"
        source.write_bytes(b"owned fixture candidate")
        source.chmod(0o700)
        baseline.write_text("{}")
        pg = root / "pg-bin"
        pg.mkdir()
        (pg / "initdb").touch()
        roots, runners, fixtures = [], [], []
        helpers = self.helpers()
        parent = self.parent()
        # Capture the actual constructor before the main-only patch.
        constructor = ha.QualificationRoot
        def create_root(actual_helpers):
            storage = constructor(actual_helpers, root)
            roots.append(storage)
            return storage
        def create_runner(parent_arg, runner_root, **kwargs):
            self.assertEqual(parent_arg, parent)
            self.assertEqual(kwargs["scope"], roots[0].scope)
            runner = SimpleNamespace(root=runner_root, native_binary=None, container=None, relay=None,
                _stop_native=Mock(), cleanup=Mock(), protocol=Mock(), identity_record={"scope": kwargs["scope"]})
            (runner_root / "ca-key.pem").write_bytes(b"private fixture authority")
            def start():
                early = json.loads(report.read_text())
                self.assertEqual(early["fixture_root"], str(roots[0].path))
                self.assertEqual(early["fixture_identity"]["scope"], kwargs["scope"])
                self.assertTrue((roots[0].path / "fixture.json").is_file())
                return runner
            runner.start = Mock(side_effect=start)
            runner.cleanup.side_effect = lambda: (runner_root / "ca-key.pem").unlink()
            runners.append(runner)
            return runner
        def create_fixture(fixture_root, *unused):
            fixture = SimpleNamespace(cleanup=Mock(), children=[], data=fixture_root / "pg",
                last_phase="injected_pure_case", last_api_status=None, failed_api_status=None,
                last_ws_status=None, scenarios=[], cleanup_failures=[], nodes={}, wire_sockets=[], observer=None)
            fixtures.append(fixture)
            return fixture
        def qualify(fixture, output, migration, *, include_api_desktop_phase=False, diagnostic_only=False):
            self.assertFalse(include_api_desktop_phase)
            self.assertFalse(diagnostic_only)
            if interrupted:
                ha.signal.getsignal(ha.signal.SIGTERM)(ha.signal.SIGTERM, None)
            output["checks"].append({"name": "pure_injected_workload", "passed": True})
        helpers.ParentDocker = Mock(return_value=parent)
        helpers.RunnerFixture = Mock(side_effect=create_runner)
        argv = ["qualify_linux_desktop_ha.py", "--execute", "--server-bin", str(source),
            "--expected-sha256", hashlib.sha256(source.read_bytes()).hexdigest(),
            "--expected-runner-sha256", "b" * 64, "--expected-migration", "59",
            "--runner-image-id", "sha256:" + "c" * 64, "--gui-image-id", "sha256:" + "d" * 64,
            "--core-image-id", "sha256:" + "e" * 64, "--docker-client", str(root / "docker.exe"),
            "--docker-context", "desktop-linux", "--baseline", str(baseline), "--report", str(report)]
        saved = {number: ha.signal.getsignal(number) for number in ha.HANDLED_SIGNALS}
        with patch.object(ha.sys, "argv", argv), patch.object(ha, "PG_BIN", pg), \
                patch.object(ha.shutil, "which", return_value="/unused/openssl"), \
                patch.object(ha, "load_runner_helpers", return_value=(helpers, Path(__file__))), \
                patch.object(ha, "QualificationRoot", side_effect=create_root), \
                patch.object(ha, "LinuxFixture", side_effect=create_fixture), \
                patch.object(ha, "qualify", side_effect=qualify), \
                patch.object(ha.socket, "socket", side_effect=AssertionError("no sockets")), redirect_stdout(io.StringIO()):
            result = ha.main()
        self.assertEqual(saved, {number: ha.signal.getsignal(number) for number in ha.HANDLED_SIGNALS})
        for runner in runners:
            runner.start.assert_called_once()
            runner.cleanup.assert_called_once()
        for fixture in fixtures:
            fixture.cleanup.assert_called_once()
        return result, json.loads(report.read_text()), roots[0]

    @unittest.skipUnless(os.name == "posix", "The executable main explicitly requires nonroot Linux/WSL")
    def test_main_injected_sigterm_retains_root_and_archived_keys_after_confirmed_cleanup(self):
        with tempfile.TemporaryDirectory() as directory:
            result, report, storage = self.run_main_without_services(directory, interrupted=True)
            self.assertEqual(result, 1)
            self.assertFalse(report["passed"])
            self.assertEqual(report["failure_check"], "owned_linux_desktop_ha_interrupted")
            self.assertTrue(all(report["cleanup_proofs"].values()))
            self.assertEqual(report["private_root_disposition"], "retained_failed_qualification")
            self.assertTrue((storage.path / "fixture.json").is_file())
            self.assertTrue((storage.path / "report.json").is_file())
            self.assertEqual((storage.path / "evidence/ca-key.pem").read_bytes(), b"private fixture authority")
            self.assertNotIn("private fixture authority", json.dumps(report))

    @unittest.skipUnless(os.name == "posix", "The executable main explicitly requires nonroot Linux/WSL")
    def test_main_success_removes_only_confirmed_private_root(self):
        with tempfile.TemporaryDirectory() as directory:
            result, report, storage = self.run_main_without_services(directory, interrupted=False)
            self.assertEqual(result, 0)
            self.assertTrue(report["passed"])
            self.assertTrue(all(report["cleanup_proofs"].values()))
            self.assertEqual(report["private_root_disposition"], "removed_after_confirmed_success")
            self.assertFalse(storage.path.exists())
            self.assertTrue((Path(directory) / "public-report.json").is_file())


class LeaseRefusalTests(unittest.TestCase):
    def fixture(self, status):
        fixture = ha.LinuxFixture.__new__(ha.LinuxFixture)
        fixture.api_ports = [1234]
        fixture.access_token = "own-pure-test-token"
        fixture.http = Mock(return_value=(status, "application/json", b'{"token":"own-ticket"}'))
        fixture.websocket = Mock()
        return fixture

    def test_lease_fault_is_exact_one_row_and_workspace_contract_ack_is_transaction_local(self):
        fixture = self.fixture(409)
        fixture.sql = Mock(return_value="BEGIN\nSET\n1\nCOMMIT")
        run, lease = str(uuid.uuid4()), str(uuid.uuid4())
        fixture.invalidate_owned_lease_expectation({"run": run, "binding": {"lease_token": lease}})
        query = fixture.sql.call_args.args[0]
        self.assertTrue(query.startswith("BEGIN; SET LOCAL cowork.chat_workspace_version='1';"))
        self.assertTrue(query.endswith("COMMIT;"))
        self.assertIn(f"WHERE id='{run}'::uuid AND lease_token='{lease}'::uuid RETURNING 1", query)
        self.assertNotIn("DISABLE", query)

    def test_lease_fault_refuses_zero_multiple_or_incomplete_transaction_result(self):
        fixture = self.fixture(409)
        case = {"run": str(uuid.uuid4()), "binding": {"lease_token": str(uuid.uuid4())}}
        for result in ("BEGIN\nSET\n0\nCOMMIT", "BEGIN\nSET\n2\nCOMMIT", "BEGIN\nSET\n1", "1"):
            fixture.sql = Mock(return_value=result)
            with self.subTest(result=result), self.assertRaisesRegex(ha.QualificationFailure, "exact_one_row"):
                fixture.invalidate_owned_lease_expectation(case)

    def test_lease_fault_refuses_invalid_identity_before_sql(self):
        fixture = self.fixture(409)
        fixture.sql = Mock()
        for case in ({"run": "bad", "binding": {"lease_token": str(uuid.uuid4())}},
                     {"run": str(uuid.uuid4()), "binding": {"lease_token": "bad"}}):
            with self.assertRaises(ha.QualificationFailure):
                fixture.invalidate_owned_lease_expectation(case)
        fixture.sql.assert_not_called()

    def test_fenced_ticket_refusal_needs_no_new_websocket_request(self):
        fixture = self.fixture(409)
        case = {"run": str(uuid.uuid4()), "session": str(uuid.uuid4())}
        fixture.refuse_old_lease_stream(case, 0)
        fixture.websocket.assert_not_called()
        self.assertEqual(fixture.http.call_args.kwargs["token"], fixture.access_token)
        self.assertEqual(fixture.http.call_args.kwargs["method"], "POST")

    def test_minted_ticket_must_still_refuse_wss_and_auth_failure_is_not_lease_proof(self):
        case = {"run": str(uuid.uuid4()), "session": str(uuid.uuid4())}
        fixture = self.fixture(200)
        fixture.refuse_old_lease_stream(case, 0)
        fixture.websocket.assert_called_once_with(0,
            f"/api/v1/desktop-sessions/{case['session']}/stream?ticket=own-ticket", expected=409)
        for status in [401, 403, 500]:
            fixture = self.fixture(status)
            with self.subTest(status=status), self.assertRaises(ha.QualificationFailure):
                fixture.refuse_old_lease_stream(case, 0)
            fixture.websocket.assert_not_called()



class PumpEndpoint:
    def __init__(self, reads=(), send_limit=65536):
        self.reads, self.send_limit = list(reads), send_limit
        self.sent = bytearray()
        self.blocking, self.timeouts = [], []
    def setblocking(self, value):
        self.blocking.append(value)
    def settimeout(self, value):
        self.timeouts.append(value)
    def send(self, body):
        count = min(self.send_limit, len(body))
        self.sent.extend(body[:count])
        return count
    def sendall(self, body):
        offset = 0
        while offset < len(body):
            count = self.send(body[offset:])
            if not count:
                raise BrokenPipeError
            offset += count
    def recv(self, size):
        result = self.reads.pop(0)
        if len(result) > size:
            self.reads.insert(0, result[size:])
        return result[:size]


class TransportDiagnosticTests(unittest.TestCase):
    def test_coalesced_101_prefix_and_partial_sends_preserve_exact_raw_counts(self):
        trace = ha.TransportTrace(0)
        trace.metadata(upstream_status=101)
        client = PumpEndpoint(send_limit=3)
        head, raw = b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket", b"raw-rfb-prefix"
        ha.forward_rfb_upgrade(client, head, raw, trace)
        self.assertEqual(client.sent, head + b"\r\n\r\n" + raw)
        result = trace.snapshot()
        self.assertTrue(result["upgrade_completed"])
        self.assertEqual(result["upstream_prefix_bytes"], len(raw))
        self.assertEqual(result["runner_to_client"]["read_bytes"], len(raw))
        self.assertEqual(result["runner_to_client"]["sent_bytes"], len(raw))
        self.assertEqual(result["runner_to_client"]["pending_bytes"], 0)
        self.assertNotIn(raw.decode(), json.dumps(result))

    def test_101_is_not_forwarded_upgrade_when_prefix_write_is_unknown(self):
        trace = ha.TransportTrace(0)
        trace.metadata(upstream_status=101)
        client = PumpEndpoint(send_limit=0)
        with self.assertRaisesRegex(ha.QualificationFailure, "upgrade_destination_eof"):
            ha.forward_rfb_upgrade(client, b"HTTP/1.1 101 Switching Protocols", b"prefix", trace)
        result = trace.snapshot()
        self.assertFalse(result["upgrade_completed"])
        self.assertTrue(result["runner_to_client"]["destination_eof"])
        self.assertEqual(result["runner_to_client"]["pending_bytes"], 6)

    def test_bidirectional_partial_pump_writes_and_eof_have_separate_read_sent_counts(self):
        client = PumpEndpoint([b"client", b""], send_limit=2)
        upstream = PumpEndpoint([b"upstream"], send_limit=3)
        trace = ha.TransportTrace(1)
        rounds = [([client, upstream], [], []), ([], [client, upstream], []),
                  ([], [client, upstream], []), ([], [client], []),
                  ([client], [client], [])]
        with patch.object(ha.select, "select", side_effect=rounds),              patch.object(ha.socket, "socket", side_effect=AssertionError("no socket")):
            ha.pump_tls(client, upstream, ha.threading.Event(), trace.note)
        self.assertEqual(bytes(client.sent), b"upstream")
        self.assertEqual(bytes(upstream.sent), b"client")
        result = trace.snapshot()
        self.assertEqual((result["client_to_runner"]["read_bytes"], result["client_to_runner"]["sent_bytes"]), (6, 6))
        self.assertEqual((result["runner_to_client"]["read_bytes"], result["runner_to_client"]["sent_bytes"]), (8, 8))
        self.assertTrue(result["client_to_runner"]["source_eof"])
        self.assertFalse(result["runner_to_client"]["source_eof"])

    def test_eof_keeps_unforwarded_bytes_pending_instead_of_claiming_delivery(self):
        client, upstream = PumpEndpoint([b""]), PumpEndpoint([b"waiting"])
        trace = ha.TransportTrace(0)
        with patch.object(ha.select, "select", side_effect=[([upstream], [], []), ([client], [], [])]):
            ha.pump_tls(client, upstream, ha.threading.Event(), trace.note)
        result = trace.snapshot()["runner_to_client"]
        self.assertEqual((result["read_bytes"], result["sent_bytes"], result["pending_bytes"]), (7, 0, 7))

    def test_pump_send_failure_keeps_direction_operation_and_unforwarded_count(self):
        client, upstream = PumpEndpoint(), PumpEndpoint([b"waiting"])
        client.send = Mock(side_effect=BrokenPipeError("private endpoint detail"))
        trace = ha.TransportTrace(0)
        with patch.object(ha.select, "select", side_effect=[([upstream], [], []), ([], [client], [])]):
            with self.assertRaises(BrokenPipeError):
                ha.pump_tls(client, upstream, ha.threading.Event(), trace.note)
        trace.metadata(error_class=ha.transport_error_class(BrokenPipeError()))
        row = trace.snapshot()
        self.assertEqual((row["error_direction"], row["error_operation"], row["error_class"]),
                         ("runner_to_client", "send", "broken_pipe"))
        self.assertEqual(row["runner_to_client"]["pending_bytes"], 7)
        self.assertEqual(row["runner_to_client"]["sent_bytes"], 0)
        self.assertNotIn("private", json.dumps(row))

    def test_counter_limits_fixed_metadata_and_snapshots_never_accept_payloads(self):
        trace = ha.TransportTrace(0)
        trace.value["runner_to_client"]["read_bytes"] = ha.TRACE_COUNTER_MAX - 1
        trace.note("runner_to_client", "read", 3)
        self.assertEqual(trace.snapshot()["runner_to_client"]["read_bytes"], ha.TRACE_COUNTER_MAX)
        self.assertTrue(trace.snapshot()["counter_saturated"])
        snapshot = trace.snapshot()
        snapshot["runner_to_client"]["read_bytes"] = 0
        self.assertNotEqual(trace.snapshot()["runner_to_client"]["read_bytes"], 0)
        for fields in [{"headers": "private token"}, {"upstream_status": True}, {"phase": "private name"},
                       {"error_class": "private error"}, {"upstream_prefix_bytes": ha.PROXY_BUFFER + 1}]:
            with self.assertRaises(ha.QualificationFailure):
                trace.metadata(**fields)
        for count in [True, -1, ha.PROXY_BUFFER + 1]:
            with self.assertRaises(ha.QualificationFailure):
                trace.note("runner_to_client", "read", count)
        self.assertEqual(ha.transport_error_class(TimeoutError("private nonce")), "timeout")
        self.assertEqual(ha.transport_error_class(ha.ssl.SSLError("private key path")), "tls_error")
        self.assertEqual(ha.transport_error_class(RuntimeError("private token")), "unexpected_error")

    def test_session_snapshot_is_filtered_bounded_and_contains_no_private_record_fields(self):
        observer = object.__new__(ha.RunnerObserver)
        observer.lock = ha.threading.Lock()
        own, foreign = str(uuid.uuid4()), str(uuid.uuid4())
        observer.records = [{"kind": "rfb", "session": own if index % 2 == 0 else foreign,
                             "trace": ha.TransportTrace(index)} for index in range(100)]
        self.assertEqual(observer.count("rfb", own), 50)
        selected = observer.transport_snapshot(own)
        self.assertEqual(len(selected), 16)
        self.assertTrue(all(row["stream_index"] % 2 == 0 for row in selected))
        self.assertEqual(len(observer.transport_snapshot()), 64)
        self.assertNotIn(own, json.dumps(selected))
        self.assertNotIn(foreign, json.dumps(selected))

    def test_actual_observer_handler_records_genuine_101_prefix_before_pump(self):
        session = str(uuid.uuid4())
        upstream = PumpEndpoint([b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\r\ncoalesced-rfb"])
        upstream.close = Mock()
        raw = SimpleNamespace(close=Mock())
        client = PumpEndpoint(send_limit=7)
        server = SimpleNamespace(serve_forever=Mock())
        tls_context = SimpleNamespace(wrap_socket=Mock(return_value=upstream))
        tls_server = SimpleNamespace(load_cert_chain=Mock())
        with patch.object(ha.ssl, "create_default_context", return_value=tls_context),              patch.object(ha.ssl, "SSLContext", return_value=tls_server),              patch.object(ha, "OwnedHttpServer", return_value=server) as ctor,              patch.object(ha.threading, "Thread"),              patch.object(ha.socket, "create_connection", return_value=raw),              patch.object(ha, "pump_tls") as pump:
            observer = ha.RunnerObserver(1025, "https://127.0.0.1:1026", Path("ca"), Path("cert"), Path("key"), Mock())
            handler_type = ctor.call_args.args[1]
            handler = object.__new__(handler_type)
            handler.command, handler.path = "GET", "/v1/desktop-sessions/" + session + "/stream?binding_sha256=" + "a" * 64
            handler.headers = ha.Message()
            handler.headers["Upgrade"] = "websocket"
            handler.rfile, handler.connection = io.BytesIO(), client
            handler.forward()
        snapshot = observer.transport_snapshot(session)[0]
        self.assertEqual(snapshot["upstream_status"], 101)
        self.assertTrue(snapshot["upgrade_completed"])
        self.assertTrue(snapshot["finished"])
        self.assertEqual((snapshot["runner_to_client"]["read_bytes"], snapshot["runner_to_client"]["sent_bytes"]), (13, 13))
        pump.assert_called_once()
        self.assertTrue(callable(pump.call_args.args[3]))
        self.assertEqual(observer.count("rfb", session), 1)


class DirectRunnerDiagnosticTests(unittest.TestCase):
    def test_deadline_socket_caps_each_partial_operation_and_refuses_slow_drip(self):
        endpoint = PumpEndpoint([b"a", b"b"], send_limit=2)
        stream = ha.DeadlineSocket(endpoint, 5)
        stream.settimeout(20)
        with patch.object(ha.time, "monotonic", side_effect=[1, 2, 3, 5]):
            stream.sendall(b"four")
            self.assertEqual(stream.recv(1), b"a")
            with self.assertRaises(TimeoutError):
                stream.recv(1)
        self.assertEqual(endpoint.timeouts, [4, 3, 2])
        self.assertEqual(endpoint.reads, [b"b"])

    def test_direct_probe_mac_full_binding_and_readonly_rfb_are_real_contract_and_sanitized(self):
        from contextlib import contextmanager
        helpers, _ = ha.load_runner_helpers()
        key = b"private-test-proof-key"
        protocol = helpers.RunnerProtocol.__new__(helpers.RunnerProtocol)
        protocol.origin, protocol.signing_key = "https://127.0.0.1:1026", key
        job = binding()
        requests, deadlines = [], []
        class Peer:
            def __init__(self):
                self.response = bytearray()
                self.closed = False
            def settimeout(self, seconds): pass
            def shutdown(self, how): pass
            def close(self): self.closed = True
            def remaining(self): return 1
            def recv(self, size):
                answer = bytes(self.response[:min(size, 17)])
                del self.response[:len(answer)]
                return answer
            def sendall(self, request):
                if not request.startswith((b"POST ", b"GET ")):
                    return  # Actual RFB handshake bytes, never an input event.
                head, body = request.split(b"\r\n\r\n", 1)
                lines = head.decode().split("\r\n")
                method, path, _ = lines[0].split()
                fields = dict(line.split(": ", 1) for line in lines[1:])
                requests.append((method, path, body))
                if method == "POST":
                    payload = json.loads(body)
                    reply = json.dumps({"version": 1, "challenge": payload["challenge"],
                        "job": {"binding": job, "running": True, "paused": False, "restarting": False}}, separators=(",", ":")).encode()
                    mac = helpers.response_mac(key, helpers.PROBE_DOMAIN,
                        [method, path, fields["x-cowork-timestamp"], fields["x-cowork-nonce"]], body, reply)
                    self.response.extend(b"HTTP/1.1 200 OK\r\nContent-Length: " + str(len(reply)).encode()
                        + b"\r\nx-cowork-desktop-proof: " + mac.encode() + b"\r\n\r\n" + reply)
                else:
                    rfb = rfb_bytes(name=b"private-name-not-for-report")
                    wire = b"\x82\x7e" + struct.pack("!H", len(rfb)) + rfb
                    self.response.extend(("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
                        "Sec-WebSocket-Accept: " + ha.websocket_accept(fields["Sec-WebSocket-Key"]) + "\r\n\r\n").encode() + wire)
        peers = []
        @contextmanager
        def connection(unused, deadline):
            peer = Peer(); peers.append(peer); deadlines.append(deadline)
            yield peer
            peer.close()
        with patch.object(ha, "direct_runner_tls", side_effect=connection),              patch.object(ha.socket, "socket", side_effect=AssertionError("no actual socket")):
            result = ha.direct_runner_readonly(protocol, job)
        self.assertEqual(result, {"phase": "completed", "fresh_probe_confirmed": True, "upgrade_status": 101, "framebuffer_bytes": 64})
        self.assertTrue(all(peer.closed for peer in peers))
        self.assertEqual(requests[0][:2], ("POST", "/v1/desktop-session-probe"))
        self.assertIn("control=false&binding_sha256=" + ha.binding_digest(job), requests[1][1])
        self.assertNotIn("private", json.dumps(result))
        self.assertNotIn("pixel_sha256", result)
        self.assertLessEqual(deadlines[0], deadlines[1])

    def test_current_lease_desktop_and_physical_identity_fence_direct_io(self):
        fixture = object.__new__(ha.LinuxFixture)
        job = binding()
        case = {"run": job["run_id"], "session": job["session_id"], "binding": job}
        lease = {"state": "running", "live": True, "token": job["lease_token"], "workspace": job["workspace_id"]}
        row = {"state": "agent_controlled", "binding": job, "start_pending": "false", "cleanup_state": None}
        physical = {"Id": job["container_id"], "Created": job["created_at"], "State": {"Running": True, "StartedAt": job["started_at"]}}
        fixture.runner = SimpleNamespace(inspect=Mock(return_value=physical), actual_engine_id=job["engine_id"])
        fixture.protocol, fixture.no_replay = Mock(), Mock()
        for stage in ["expired", "new_lease", "stop_pending", "changed_binding", "restarted"]:
            changed_lease, changed_row, changed_physical = dict(lease), dict(row), dict(physical)
            if stage == "expired": changed_lease["live"] = False
            if stage == "new_lease": changed_lease["token"] = str(uuid.uuid4())
            if stage == "stop_pending": changed_row["cleanup_state"] = "stop_pending"
            if stage == "changed_binding": changed_row["binding"] = {**job, "attempt_id": str(uuid.uuid4())}
            if stage == "restarted": changed_physical["State"] = {"Running": True, "StartedAt": "changed"}
            fixture.run_lease = Mock(return_value=changed_lease)
            fixture.sql = Mock(return_value=json.dumps(changed_row))
            fixture.runner.inspect = Mock(return_value=changed_physical)
            with self.subTest(stage=stage), patch.object(ha, "direct_runner_readonly") as io_call:
                with self.assertRaises(ha.QualificationFailure):
                    fixture.direct_runner_observation(case)
                io_call.assert_not_called()

    def test_diagnostic_optin_never_claims_public_api_full_phase(self):
        args = InputsTests().args(diagnostic_only=True)
        self.assertTrue(ha.qualification_inputs(args)["diagnostic_only"])
        with self.assertRaisesRegex(ha.QualificationFailure, "diagnostic_only_cannot_claim_full_api_phase"):
            ha.qualification_inputs(InputsTests().args(diagnostic_only=True, include_api_desktop_phase=True))



class DiagnosticOnlyFlowTests(unittest.TestCase):
    def test_four_check_diagnostic_stops_before_owner_kill_and_always_exits_phase(self):
        from contextlib import contextmanager
        fixture = Mock()
        fixture.api_ports, fixture.node_ids, fixture.peer_proxies = [1001, 1002, 1003], [str(uuid.uuid4()) for _ in range(3)], {(2, 0): Mock()}
        fixture.http.return_value = (201, "application/json", b'{"access_token":"pure-token"}')
        fixture.api.side_effect = [{"id": str(uuid.uuid4())}, {"id": str(uuid.uuid4())}]
        fixture.sql.return_value = "59"
        fixture.metrics.return_value = {"application": {"runner_configured": True, "object_store_configured": False},
            "runtime": {"api_routing": {"multiple_api_replicas_supported": False}}}
        fixture.reserve_port.return_value = 1004
        fixture.runtime_environment.return_value = {}
        fixture.parent = SimpleNamespace(assert_preserved=Mock())
        fixture.runner.actual_engine_id, fixture.runner.instance_id = "owned-engine", str(uuid.uuid4())
        case = {"run": str(uuid.uuid4()), "session": str(uuid.uuid4())}
        fixture.create_worker_desktop.return_value = case
        lease = {"state": "running", "live": True, "owner": str(uuid.uuid4()), "token": str(uuid.uuid4()),
                 "workspace": str(uuid.uuid4()), "expiry": 123.0}
        fixture.run_lease.return_value = lease
        fixture.desktop_owner.return_value = {"node_id": fixture.node_ids[0], "executor_id": None}
        def wire(): return SimpleNamespace(assert_closed=Mock(), close=Mock())
        fixture.live_stream.side_effect = [(wire(), {"token": "pure-ticket"}), (wire(), {}), (None, {}), (wire(), {})]
        traces = []
        for index in range(3):
            trace = ha.TransportTrace(index)
            trace.metadata(upstream_status=101, upgrade_completed=True)
            for direction in ha.TRACE_DIRECTIONS:
                trace.note(direction, "read", 12)
                trace.note(direction, "sent", 12)
            traces.append(trace.snapshot())
        fixture.observer.transport_snapshot.return_value = traces
        phase_exits = []
        @contextmanager
        def phase(name):
            try:
                yield
            finally:
                phase_exits.append(name)
        fixture.real_phase.side_effect = phase
        report = {"checks": [], "full_matrix_qualified": False}
        with patch.object(ha.Fixture, "server_environment", return_value={}), \
             patch.object(ha, "real_framebuffer", return_value=64), \
             patch.object(ha.socket, "socket", side_effect=AssertionError("no actual socket")):
            ha.qualify(fixture, report, 59, diagnostic_only=True)
        self.assertEqual(len(report["checks"]), 4)
        self.assertFalse(report["full_matrix_qualified"])
        self.assertEqual(phase_exits, ["physical_owner_kill_and_stop"])
        self.assertEqual(fixture.peer_proxies[2, 0].partition.call_args_list[0].args, (True,))
        self.assertEqual(fixture.peer_proxies[2, 0].partition.call_args_list[1].args, (False,))
        fixture.kill_group.assert_not_called()
        fixture.stop_server.assert_not_called()
        self.assertEqual(report["owner_partition_diagnostic"]["observer_transport"], traces)


if __name__ == "__main__":
    unittest.main()
