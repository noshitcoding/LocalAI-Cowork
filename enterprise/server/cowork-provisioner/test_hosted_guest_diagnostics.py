"""Pure private-file/Fake-Popen tests; no commands, VM, sockets or services."""
import copy
import hashlib
import io
import json
import os
from pathlib import Path
import ssl
import subprocess
import tempfile
import unittest
import uuid
from unittest.mock import Mock, patch

import hosted_vm_transport as transport
import qualify_hosted_vm as harness


class FakeChild:
    def __init__(self, stdout=b'', stderr=b'', code=0, waits=()):
        self.stdout, self.stderr = io.BytesIO(stdout), io.BytesIO(stderr)
        self.returncode, self.code = None, code
        self.waits = list(waits)
        self.calls = []
        self.signals = []

    def wait(self, timeout=None):
        self.calls.append(timeout)
        if self.waits:
            result = self.waits.pop(0)
            if isinstance(result, BaseException): raise result
        self.returncode = self.code if self.returncode is None else self.returncode
        return self.returncode

    def terminate(self): self.signals.append('terminate'); self.returncode = -15
    def kill(self): self.signals.append('kill'); self.returncode = -9


class GuestCommandTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(); self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.registry = type('Registry', (), {'diagnostics': transport.RegistryDiagnostics()})()
        self.rows = []

    def execute(self, child, action='pull', publish=None):
        def directory(root):
            self.assertEqual(root, self.root)
            path = root / 'diagnostics'; path.mkdir(exist_ok=True)
            return path
        command = ['docker', '--host', 'tcp://private.invalid:2376', '--tlscert', '/private/client.pem',
                   *(['pull'] if action == 'pull' else ['image', 'inspect']),
                   'localhost:443/owned/runner@sha256:' + 'a' * 64]
        callback = publish or (lambda row: self.rows.append(copy.deepcopy(row)))
        with patch.object(harness, 'guest_diagnostic_directory', directory), \
                patch.object(harness.os, 'O_NOFOLLOW', getattr(os, 'O_NOFOLLOW', 0), create=True), \
                patch.object(harness.subprocess, 'Popen', return_value=child) as popen:
            self.last_popen = popen
            result = harness.guest_image_command(self.root, 'runner', action, command, self.registry, callback)
        self.assertEqual(popen.call_args.args, (command,))
        self.assertEqual(popen.call_args.kwargs, {'stdin': subprocess.DEVNULL, 'stdout': subprocess.PIPE,
                                                 'stderr': subprocess.PIPE, 'start_new_session': True})
        return result

    def test_prepare_is_durable_before_child_creation_and_completed_child_is_reaped(self):
        child = FakeChild(stdout=b'private progress')
        with patch.object(child, 'wait', wraps=child.wait) as wait:
            result = self.execute(child)
        self.assertEqual(result, b'')
        self.assertEqual([row['status'] for row in self.rows], ['prepared', 'running', 'completed'])
        self.assertFalse(self.rows[0]['owned_child_started'])
        self.assertTrue(self.rows[-1]['owned_child_reaped'])
        self.assertEqual(self.rows[-1]['exit_code'], 0)
        self.assertGreaterEqual(wait.call_count, 3)
        self.assertTrue(child.stdout.closed and child.stderr.closed)
        self.assertEqual(child.signals, [])

    def test_private_stderr_is_capped_and_public_observation_never_contains_text_or_argv(self):
        content = b'x509: private password https://user:secret@private.invalid/private/client.pem\n' + b'x' * 70000
        with self.assertRaisesRegex(RuntimeError, '^guest_image_pull_failed$'):
            self.execute(FakeChild(stderr=content, code=42))
        row = self.rows[-1]; public = json.dumps(row)
        for secret in ['password', 'secret@', 'private.invalid', 'client.pem', 'x509:']:
            self.assertNotIn(secret, public)
        self.assertEqual(row['observed_error_classification'], 'tls_certificate_marker')
        self.assertEqual(row['stderr']['observed_bytes'], len(content))
        self.assertEqual(row['stderr']['retained_bytes'], 65536)
        self.assertTrue(row['stderr']['truncated'])
        private = (self.root / 'diagnostics/runner-pull.stderr').read_bytes()
        self.assertEqual(private, content[:65536])
        self.assertEqual(row['stderr']['retained_sha256'], hashlib.sha256(private).hexdigest())
        self.assertEqual(row['exit_code'], 42)
        self.assertTrue(row['owned_child_reaped'])

    def test_inspect_success_returns_bounded_private_json_and_distinguishes_action(self):
        payload = b'[{"Id":"sha256:private-config"}]'
        self.assertEqual(self.execute(FakeChild(stdout=payload), 'inspect'), payload)
        row = self.rows[-1]
        self.assertEqual(row['action'], 'inspect'); self.assertEqual(row['timeout_seconds'], 120)
        self.assertEqual(row['stdout']['retained_bytes'], len(payload))
        self.assertNotIn('private-config', json.dumps(row))

    def test_inspect_nonzero_has_different_failure_code_from_pull(self):
        with self.assertRaisesRegex(RuntimeError, '^guest_image_inspect_failed$'):
            self.execute(FakeChild(stderr=b'unknown private failure', code=1), 'inspect')
        self.assertEqual(self.rows[-1]['observed_error_classification'], 'unclassified_command_failure')

    def test_inspect_overflow_never_returns_truncated_json_as_valid_evidence(self):
        with self.assertRaisesRegex(RuntimeError, '^guest_image_inspect_output_limit$'):
            self.execute(FakeChild(stdout=b'x' * (harness.GUEST_INSPECT_LIMIT + 1)), 'inspect')
        self.assertEqual(self.rows[-1]['stdout']['retained_bytes'], harness.GUEST_INSPECT_LIMIT)
        self.assertTrue(self.rows[-1]['stdout']['truncated'])
        self.assertTrue(self.rows[-1]['owned_child_reaped'])

    def test_absolute_pull_deadline_terminates_and_reaps_child_without_retry(self):
        child = FakeChild(waits=[subprocess.TimeoutExpired('private argv', 0)])
        with patch.object(harness.time, 'monotonic', side_effect=[0, 0, 0, 901, 901, 901]):
            with self.assertRaisesRegex(RuntimeError, '^guest_image_pull_timed_out$'): self.execute(child)
        self.assertTrue(self.rows[-1]['timed_out']); self.assertTrue(self.rows[-1]['owned_child_reaped'])
        self.assertEqual(child.signals, ['terminate'])
        self.assertEqual(self.rows[-1]['termination_signal'], 15)
        self.assertEqual(self.rows[-1]['timeout_seconds'], 900)

    def test_absolute_inspect_deadline_preserves_existing_120_seconds(self):
        child = FakeChild(waits=[subprocess.TimeoutExpired('private argv', 0)])
        with patch.object(harness.time, 'monotonic', side_effect=[0, 0, 0, 121, 121, 121]):
            with self.assertRaisesRegex(RuntimeError, '^guest_image_inspect_timed_out$'): self.execute(child, 'inspect')
        self.assertEqual(self.rows[-1]['timeout_seconds'], 120)
        self.assertTrue(self.rows[-1]['timed_out']); self.assertTrue(self.rows[-1]['owned_child_reaped'])

    def test_interrupt_during_wait_cannot_leave_owned_child_running(self):
        child = FakeChild(waits=[KeyboardInterrupt(), subprocess.TimeoutExpired('owned', 0)])
        with self.assertRaisesRegex(RuntimeError, '^guest_image_pull_interrupted$'): self.execute(child)
        self.assertTrue(self.rows[-1]['interrupted']); self.assertTrue(self.rows[-1]['owned_child_reaped'])
        self.assertEqual(child.signals, ['terminate'])

    def test_qualification_signal_handler_interruption_is_typed(self):
        child = FakeChild(waits=[RuntimeError('qualification_interrupted'), subprocess.TimeoutExpired('owned', 0)])
        with self.assertRaisesRegex(RuntimeError, '^guest_image_pull_interrupted$'): self.execute(child)
        self.assertTrue(self.rows[-1]['interrupted']); self.assertTrue(self.rows[-1]['owned_child_reaped'])

    def test_term_timeout_kills_only_owned_popen_then_reaps(self):
        child = FakeChild(waits=[KeyboardInterrupt(), subprocess.TimeoutExpired('owned', 0),
                                subprocess.TimeoutExpired('owned', 2)])
        with self.assertRaisesRegex(RuntimeError, '^guest_image_pull_interrupted$'): self.execute(child)
        self.assertEqual(child.signals, ['terminate', 'kill'])
        self.assertEqual(self.rows[-1]['exit_code'], -9)
        self.assertTrue(self.rows[-1]['owned_child_reaped'])

    def test_unreadable_signal_or_wait_state_is_pending_until_child_is_reaped(self):
        child = FakeChild(waits=[KeyboardInterrupt(), subprocess.TimeoutExpired('owned', 0),
                                subprocess.TimeoutExpired('owned', .25)])
        child.terminate = Mock(side_effect=PermissionError('private metadata'))
        with self.assertRaisesRegex(RuntimeError, '^guest_image_child_supervision_pending$'): self.execute(child)
        self.assertIn('supervision_pending', [row['status'] for row in self.rows])
        self.assertTrue(self.rows[-1]['owned_child_reaped'])
        self.assertIsNotNone(child.returncode)
        self.assertNotIn('private metadata', json.dumps(self.rows))

    def test_diagnostic_write_failure_remains_failure_even_when_child_exits_zero(self):
        child = FakeChild(stderr=b'private')
        with patch.object(harness.os, 'write', side_effect=OSError('private diagnostic path')):
            with self.assertRaisesRegex(RuntimeError, '^guest_image_diagnostic_io_failed$'): self.execute(child)
        self.assertTrue(self.rows[-1]['stderr']['io_failed'])
        self.assertTrue(self.rows[-1]['owned_child_reaped'])

    def test_publisher_failure_after_spawn_still_reaps_child_and_does_not_export_exception(self):
        child = FakeChild(waits=[subprocess.TimeoutExpired('owned', 0)])
        def publish(row):
            self.rows.append(copy.deepcopy(row))
            if row['status'] == 'running': raise OSError('private report failure')
        with self.assertRaisesRegex(RuntimeError, '^guest_image_command_supervision_failed$'):
            self.execute(child, publish=publish)
        self.assertTrue(self.rows[-1]['owned_child_reaped']); self.assertEqual(child.signals, ['terminate'])
        self.assertNotIn('private report failure', json.dumps(self.rows))

    def test_prepare_write_failure_refuses_before_spawn(self):
        def publish(row):
            if row['status'] == 'prepared': raise OSError('private destination')
            self.rows.append(copy.deepcopy(row))
        with self.assertRaisesRegex(RuntimeError, '^guest_image_command_supervision_failed$'):
            self.execute(FakeChild(), publish=publish)
        self.last_popen.assert_not_called()
        self.assertFalse(self.rows[-1]['owned_child_started'])

    def test_persistent_report_write_failure_is_typed_and_still_cannot_leave_child_live(self):
        child = FakeChild(waits=[subprocess.TimeoutExpired('owned', 0)])
        def publish(row):
            self.rows.append(copy.deepcopy(row))
            if row['status'] != 'prepared': raise OSError('secret report destination')
        with self.assertRaisesRegex(RuntimeError, '^guest_image_diagnostic_report_failed$'):
            self.execute(child, publish=publish)
        self.assertTrue(self.rows[-1]['owned_child_reaped'])
        self.assertIsNotNone(child.returncode)

    def test_unknown_role_action_and_mismatched_argv_refuse_before_private_files_or_commands(self):
        for role, action, argv in [('foreign', 'pull', ['docker', 'pull', 'ref']),
                                  ('runner', 'delete', ['docker', 'pull', 'ref']),
                                  ('runner', 'inspect', ['docker', 'pull', 'ref'])]:
            with patch.object(harness.subprocess, 'Popen') as popen, self.subTest(role=role, action=action):
                with self.assertRaisesRegex(RuntimeError, '^invalid_guest_image_command'):
                    harness.guest_image_command(self.root, role, action, argv, self.registry, Mock())
                popen.assert_not_called()
        self.assertEqual(list(self.root.iterdir()), [])

    def test_existing_private_diagnostic_is_never_reused_or_overwritten(self):
        directory = self.root / 'diagnostics'; directory.mkdir()
        path = directory / 'runner-pull.stderr'; path.write_bytes(b'original evidence')
        with self.assertRaises(FileExistsError): self.execute(FakeChild())
        self.assertEqual(path.read_bytes(), b'original evidence')

    def test_command_registry_delta_is_per_step_and_contains_no_request_paths(self):
        self.registry.diagnostics.response('registry', 'GET', 'v2', 200, written=2)
        def publish(row):
            self.rows.append(copy.deepcopy(row))
            if row['status'] == 'prepared': self.registry.diagnostics.response('runner', 'GET', 'blob', 200, written=42)
        self.execute(FakeChild(), publish=publish)
        self.assertEqual(self.rows[-1]['registry_delta']['responses'], [{'image_role': 'runner', 'method': 'GET',
            'category': 'blob', 'status': 200, 'responses': 1, 'completed_write_bytes': 42}])

    def test_public_command_inventory_replaces_inflight_record_and_keeps_finite_roles(self):
        diagnostic = harness.PublicDiagnostics()
        self.execute(FakeChild(), publish=diagnostic.guest_command)
        rows = diagnostic.snapshot()['guest_image_commands']
        self.assertEqual(len(rows), 1); self.assertEqual(rows[0]['status'], 'completed')

    def test_public_command_inventory_refuses_raw_or_usercontrolled_fields(self):
        diagnostic = harness.PublicDiagnostics()
        for observation in [dict(image_role='secret-path', action='pull', status='prepared'),
            dict(image_role='runner', action='pull', status='prepared', argv=['secret']),
            dict(image_role='runner', action='pull', status='private-text')]:
            with self.assertRaisesRegex(RuntimeError, '^invalid_public_guest_command$'):
                diagnostic.guest_command(observation)
        self.assertEqual(diagnostic.snapshot(), {})

    def test_failed_child_creation_is_typed_without_private_os_error(self):
        with patch.object(harness, 'guest_diagnostic_directory', return_value=self.root), \
                patch.object(harness.os, 'O_NOFOLLOW', getattr(os, 'O_NOFOLLOW', 0), create=True), \
                patch.object(harness.subprocess, 'Popen', side_effect=OSError('private executable path')) as popen:
            with self.assertRaisesRegex(RuntimeError, '^guest_image_command_launch_failed$'):
                harness.guest_image_command(self.root, 'runner', 'pull', ['docker', 'pull', 'private reference'],
                    self.registry, lambda row: self.rows.append(copy.deepcopy(row)))
        self.assertEqual(popen.call_count, 1)
        self.assertFalse(self.rows[-1]['owned_child_started'])
        self.assertNotIn('private executable path', json.dumps(self.rows))

    def test_wait_failure_after_natural_exit_is_still_reaped_and_success_is_refused(self):
        child = FakeChild(waits=[None, OSError('private wait failure'), subprocess.TimeoutExpired('own', .25)])
        with self.assertRaisesRegex(RuntimeError, '^guest_image_child_supervision_pending$'): self.execute(child)
        self.assertTrue(self.rows[-1]['owned_child_reaped'])
        self.assertEqual(child.signals, [])

    def test_stderr_fsync_failure_remains_typed_without_diagnostic_contents(self):
        with patch.object(harness.os, 'fsync', side_effect=OSError('private path')):
            with self.assertRaisesRegex(RuntimeError, '^guest_image_diagnostic_io_failed$'):
                self.execute(FakeChild(stderr=b'private marker'))
        self.assertTrue(self.rows[-1]['owned_child_reaped'])
        self.assertTrue(self.rows[-1]['stderr']['io_failed'])

    def test_thread_start_failure_is_reaped_and_unowned_pipes_are_closed(self):
        child = FakeChild(waits=[subprocess.TimeoutExpired('own', 0)])
        with patch.object(harness.threading.Thread, 'start', side_effect=RuntimeError('private thread failure')):
            with self.assertRaisesRegex(RuntimeError, '^guest_image_command_supervision_failed$'): self.execute(child)
        self.assertTrue(self.rows[-1]['owned_child_reaped'])
        self.assertTrue(child.stdout.closed and child.stderr.closed)

    def test_stream_read_failure_is_typed_and_no_exception_text_is_exported(self):
        child = FakeChild()
        child.stderr = Mock(); child.stderr.read.side_effect = OSError('private source pathname')
        with self.assertRaisesRegex(RuntimeError, '^guest_image_diagnostic_io_failed$'): self.execute(child)
        self.assertTrue(self.rows[-1]['owned_child_reaped'])
        self.assertTrue(self.rows[-1]['stderr']['io_failed'])
        self.assertNotIn('private source pathname', json.dumps(self.rows))

    @unittest.skipUnless(os.name == 'posix', 'POSIX private diagnostic mode and symlink fence')
    def test_real_private_directory_permissions_and_symlinks_fail_closed(self):
        self.root.chmod(0o700)
        directory = harness.guest_diagnostic_directory(self.root)
        self.assertEqual(directory.stat().st_mode & 0o777, 0o700)
        directory.chmod(0o755)
        with self.assertRaisesRegex(RuntimeError, '^unsafe_guest_diagnostic_directory$'):
            harness.guest_diagnostic_directory(self.root)
        directory.chmod(0o700); directory.rmdir(); directory.symlink_to(self.root, target_is_directory=True)
        with self.assertRaisesRegex(RuntimeError, '^unsafe_guest_diagnostic_directory$'):
            harness.guest_diagnostic_directory(self.root)


class RegistryMetadataTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(); self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.registry = transport.Registry(self.root / 'registry', 'cowork-hosted-vm-' + uuid.uuid4().hex)
        with patch.object(transport, 'TLSHTTPServer') as server:
            self.registry.server(Mock())
            self.handler = server.call_args.args[0]
            self.assertEqual(server.call_args.kwargs['maximum'], 4)
            self.assertIs(server.call_args.kwargs['diagnostics'], self.registry.diagnostics)
        self.digest = 'sha256:' + 'a' * 64
        self.repository = self.registry.scope + '/runner'
        self.registry.manifests[(self.repository, self.digest)] = b'private manifest bytes'
        self.registry.manifest_media[(self.repository, self.digest)] = 'application/json'
        self.blob = self.root / 'blob'; self.blob.write_bytes(b'private blob bytes')
        self.registry.blobs[self.digest] = self.blob

    def dispatch(self, path, method='GET', writer=None):
        handler = object.__new__(self.handler); handler.path = path; handler.command = method
        handler.wfile = writer if writer is not None else io.BytesIO()
        for name in ['send_response', 'send_header', 'end_headers', 'send_error']: setattr(handler, name, Mock())
        handler.dispatch()
        return handler

    def test_manifest_blob_and_v2_are_typed_without_urls_headers_or_body(self):
        self.dispatch('/v2/')
        self.dispatch('/v2/' + self.repository + '/manifests/' + self.digest)
        self.dispatch('/v2/' + self.repository + '/blobs/' + self.digest)
        result = self.registry.diagnostics.snapshot(); encoded = json.dumps(result)
        self.assertEqual(sum(row['responses'] for row in result['responses']), 3)
        self.assertEqual(sum(row['completed_write_bytes'] for row in result['responses']),
                         2 + len(b'private manifest bytes') + len(b'private blob bytes'))
        for private in [self.repository, self.digest, 'manifest bytes', 'blob bytes', str(self.blob)]:
            self.assertNotIn(private, encoded)

    def test_head_has_response_but_zero_payload_and_missing_manifest_is_404(self):
        row = self.dispatch('/v2/' + self.repository + '/manifests/' + self.digest, 'HEAD')
        self.assertEqual(row.wfile.getvalue(), b'')
        self.dispatch('/v2/' + self.repository + '/manifests/sha256:' + 'b' * 64)
        result = self.registry.diagnostics.snapshot()['responses']
        self.assertEqual({(r['method'], r['status'], r['completed_write_bytes']) for r in result},
                         {('GET', 404, 0), ('HEAD', 200, 0)})

    def test_unmatched_usercontrolled_paths_cannot_expand_counter_keyspace(self):
        for number in range(200): self.dispatch('/private/user:password@url/' + str(number))
        result = self.registry.diagnostics.snapshot()
        self.assertEqual(len(result['responses']), 1)
        self.assertEqual(result['responses'][0]['responses'], 200)
        self.assertEqual(result['responses'][0]['image_role'], 'unmatched')
        self.assertNotIn('password', json.dumps(result))

    def test_broken_pipe_counts_failure_and_does_not_claim_completed_bytes(self):
        writer = Mock(); writer.write.side_effect = BrokenPipeError('private socket path')
        with self.assertRaises(BrokenPipeError):
            self.dispatch('/v2/' + self.repository + '/blobs/' + self.digest, writer=writer)
        result = self.registry.diagnostics.snapshot()
        self.assertEqual(result['errors']['response_write_error'], 1)
        self.assertEqual(result['responses'][0]['completed_write_bytes'], 0)
        self.assertNotIn('private socket path', json.dumps(result))

    def test_tls_handshake_failure_is_only_fixed_counter_no_exception_contents(self):
        server = object.__new__(transport.TLSHTTPServer)
        server.context = Mock(); server.context.wrap_socket.side_effect = ssl.SSLError('private certificate URL')
        server.diagnostics = self.registry.diagnostics
        server.finish_request(Mock(), ('private-address', 1))
        result = self.registry.diagnostics.snapshot()
        self.assertEqual(result['errors']['tls_error'], 1)
        self.assertNotIn('private', json.dumps(result))

    def test_counters_saturate_without_wrap_or_unbounded_values(self):
        diagnostic = transport.RegistryDiagnostics()
        with patch.object(transport, 'DIAGNOSTIC_COUNTER_MAX', 3):
            for _ in range(5):
                diagnostic.response('runner', 'GET', 'blob', 200, written=2)
                diagnostic.error('socket_error')
        row = diagnostic.snapshot()
        self.assertTrue(row['counter_saturated'])
        self.assertEqual(row['responses'][0]['responses'], 3)
        self.assertEqual(row['responses'][0]['completed_write_bytes'], 3)
        self.assertEqual(row['errors']['socket_error'], 3)

    def test_registry_counter_updates_are_atomic_across_worker_threads(self):
        import threading
        diagnostic = transport.RegistryDiagnostics()
        def work():
            for _ in range(100): diagnostic.response('runner', 'GET', 'blob', 200, written=42)
        workers = [threading.Thread(target=work) for _ in range(8)]
        for worker in workers: worker.start()
        for worker in workers: worker.join()
        row = diagnostic.snapshot()['responses'][0]
        self.assertEqual(row['responses'], 800); self.assertEqual(row['completed_write_bytes'], 33600)

    def test_invalid_metadata_roles_verbs_categories_status_and_lengths_refuse(self):
        for role, method, category, status, written in [('secret-url', 'GET', 'blob', 200, 0),
            ('runner', 'POST', 'blob', 200, 0), ('runner', 'GET', 'private-path', 200, 0),
            ('runner', 'GET', 'blob', 500, 0), ('runner', 'GET', 'blob', 200, True)]:
            with self.assertRaisesRegex(RuntimeError, '^invalid_registry_diagnostic$'):
                self.registry.diagnostics.response(role, method, category, status, written=written)


if __name__ == '__main__': unittest.main()
