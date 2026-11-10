"""Pure qualification-fixture regressions; no sockets, processes or services."""
from pathlib import Path
import struct
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch
import uuid

from qualify_api_cluster import (ClusterFixture, MAX_WS_FRAME, OwnedProxy,
    QualificationFailure, WireSocket, canonical_uuid, client_frame,
    qualification_inputs, sse_event, take_server_frame, websocket_accept)


def server_frame(opcode, body, final=True):
    length = len(body)
    size = bytes([length]) if length < 126 else (
        bytes([126]) + struct.pack("!H", length) if length < 65536 else
        bytes([127]) + struct.pack("!Q", length))
    return bytes([(0x80 if final else 0) | opcode]) + size + body


class MemorySocket:
    def __init__(self, chunks):
        self.chunks = list(chunks)
        self.sent = []
    def settimeout(self, value):
        pass
    def recv(self, length):
        return self.chunks.pop(0) if self.chunks else b""
    def sendall(self, body):
        self.sent.append(body)
    def shutdown(self, mode):
        pass
    def close(self):
        pass


class InputsTests(unittest.TestCase):
    def args(self, **changes):
        return SimpleNamespace(**dict({"server_bin": Path("owned-candidate"), "expected_sha256": "a" * 64,
            "expected_migration": 56, "report": Path("owned-report.json")}, **changes))
    def test_exact_hash_and_cluster_schema_gates_before_any_service(self):
        for changes in [{"expected_sha256": "a" * 63}, {"expected_sha256": "A" * 64},
                        {"expected_sha256": "a" * 64 + "\n"}, {"expected_migration": 55},
                        {"expected_migration": -1}]:
            with self.subTest(changes=changes), self.assertRaises(QualificationFailure):
                qualification_inputs(self.args(**changes))
        result = qualification_inputs(self.args())
        self.assertEqual(result["sha256"], "a" * 64)
        self.assertEqual(result["migration"], 56)
    def test_bad_ids_fail_with_fixed_identifier(self):
        valid = str(uuid.uuid4())
        self.assertEqual(canonical_uuid(valid), valid)
        for value in [valid.upper(), valid + "' OR true", "not-a-uuid", "00000000-0000-0000-0000-000000000000", None]:
            with self.subTest(value=value), self.assertRaises(QualificationFailure):
                canonical_uuid(value)
    def test_proxy_rejects_invalid_targets_before_socket_creation(self):
        for port, target in [(0, 1), (1, 0), (3, 3), (65536, 1), ("9000", 9001)]:
            with patch("qualify_api_cluster.socket.socket") as socket_factory:
                with self.assertRaises(QualificationFailure):
                    OwnedProxy(port, target)
                socket_factory.assert_not_called()


class WireTests(unittest.TestCase):
    def test_handshake_accept_matches_rfc_example(self):
        self.assertEqual(websocket_accept("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=")
    def test_incremental_parser_preserves_every_incomplete_header_and_body(self):
        for size in [3, 130, 70000]:
            body = b"z" * size
            encoded = server_frame(2, body)
            for length in [0, 1, 2, 3, 9, len(encoded) - 1]:
                if length >= len(encoded):
                    continue
                incomplete = bytearray(encoded[:length])
                self.assertIsNone(take_server_frame(incomplete))
                self.assertEqual(incomplete, encoded[:length])
            complete = bytearray(encoded)
            self.assertEqual(take_server_frame(complete), (2, True, body))
            self.assertEqual(complete, b"")
    def test_client_mask_and_lengths(self):
        for size in [0, 4, 130, 70000]:
            body, mask = b"x" * size, b"1234"
            encoded = client_frame(2, body, mask)
            self.assertTrue(encoded[1] & 0x80)
            offset = 2 if size < 126 else (4 if size < 65536 else 10)
            self.assertEqual(encoded[offset:offset + 4], mask)
            decoded = bytes(value ^ mask[index % 4] for index, value in enumerate(encoded[offset + 4:]))
            self.assertEqual(decoded, body)
    def test_oversized_masked_reserved_or_invalid_control_frames_fail(self):
        for encoded in [bytes([0x82, 0x80]), bytes([0xC2, 0]), bytes([0x83, 0]),
                        bytes([0x82, 127]) + struct.pack("!Q", MAX_WS_FRAME + 1),
                        server_frame(9, b"x" * 126)]:
            with self.subTest(encoded=encoded[:10]), self.assertRaises(QualificationFailure):
                take_server_frame(bytearray(encoded))
        with self.assertRaises(QualificationFailure):
            client_frame(9, b"x" * 126, b"1234")
    def test_fragments_and_ping_preserve_binary_bytes(self):
        encoded = server_frame(2, b"abc", final=False) + server_frame(9, b"ping") + server_frame(0, b"def")
        transport = MemorySocket([encoded[:1], encoded[1:4], encoded[4:]])
        wire = WireSocket(transport)
        self.assertEqual(wire.recv(), (2, b"abcdef"))
        self.assertEqual(len(transport.sent), 1)
        self.assertEqual(transport.sent[0][0], 0x8a)


class CleanupTests(unittest.TestCase):
    def fixture(self):
        fixture = object.__new__(ClusterFixture)
        fixture.wire_sockets, fixture.fake_executors, fixture.owned_runs, fixture.streams = [], [], [], []
        fixture.nodes, fixture.phase_counts, fixture.access_token = {}, {}, None
        return fixture
    def test_phase_closes_every_owned_resource_after_body_failure(self):
        fixture = self.fixture()
        wire, sse, fake = Mock(), Mock(), Mock()
        fake.count.return_value = 1
        with self.assertRaisesRegex(QualificationFailure, "body_failed"):
            with fixture.phase("case"):
                fixture.wire_sockets.append(wire)
                fixture.streams.append(sse)
                fixture.fake_executors.append(fake)
                raise QualificationFailure("body_failed")
        wire.close.assert_called_once()
        sse.close.assert_called_once()
        fake.close.assert_called_once()
        self.assertEqual(fixture.phase_counts, {"case": 1})
    def test_one_cleanup_failure_still_closes_remaining_owned_resources(self):
        fixture = self.fixture()
        wire, sse, fake = Mock(), Mock(), Mock()
        wire.close.side_effect = RuntimeError("private detail")
        fake.count.return_value = 2
        with self.assertRaisesRegex(QualificationFailure, "phase_wire_cleanup"):
            with fixture.phase("case"):
                fixture.wire_sockets.append(wire)
                fixture.streams.append(sse)
                fixture.fake_executors.append(fake)
        sse.close.assert_called_once()
        fake.close.assert_called_once()


class ApiDiagnosticsTests(unittest.TestCase):
    def test_best_effort_cleanup_refusal_is_not_a_qualification_failure_diagnostic(self):
        fixture = object.__new__(ClusterFixture)
        fixture.api_ports, fixture.access_token = [9001], "private-fixture-token"
        fixture.last_phase, fixture.failed_api_status = "forwarding_replay", None
        fixture.http = Mock(return_value=(409, {}, b'{"error":"conflict"}'))
        with self.assertRaises(QualificationFailure):
            fixture.api(0, "/private-own-resource/cancel", method="POST", record_failure=False)
        self.assertIsNone(fixture.failed_api_status)
    def test_cleanup_does_not_overwrite_original_api_failure_or_expose_error_text(self):
        fixture = object.__new__(ClusterFixture)
        fixture.api_ports, fixture.access_token = [9001], "private-fixture-token"
        fixture.last_phase, fixture.failed_api_status = "owner_change", None
        fixture.http = Mock(side_effect=[(500, {}, b'{"error":"database_error","message":"private detail"}'),
                                         (200, {}, b'{}')])
        with self.assertRaisesRegex(QualificationFailure, "owned_cluster_authenticated_api_response"):
            fixture.api(0, "/private-own-resource", expected=201)
        fixture.api(0, "/private-own-resource/cancel", method="POST")
        self.assertEqual(fixture.failed_api_status, {"node": 0, "expected": 201, "actual": 500,
                                                    "phase": "owner_change", "error_code": "database_error"})
        self.assertEqual(fixture.last_api_status["actual"], 200)
        self.assertNotIn("private", str(fixture.failed_api_status))


class ReplayTests(unittest.TestCase):
    def test_last_event_marker_cannot_be_silently_skipped(self):
        import io
        replay = io.BytesIO(b'id: 3\ndata: {"payload":{"fixture_marker":"already-seen"}}\n\n'
                            b'id: 4\ndata: {"payload":{"fixture_marker":"new"}}\n\n')
        with self.assertRaisesRegex(QualificationFailure, "last_event_id_excludes"):
            sse_event(replay, "new", "fixture_replay", forbidden_marker="already-seen")
    def test_new_marker_returns_actual_durable_cursor(self):
        import io
        replay = io.BytesIO(b': keepalive\n\nid: 4\ndata: {"payload":{"fixture_marker":"new"}}\n\n')
        self.assertEqual(sse_event(replay, "new", "fixture_replay", forbidden_marker="already-seen"), 4)


if __name__ == "__main__":
    unittest.main()
