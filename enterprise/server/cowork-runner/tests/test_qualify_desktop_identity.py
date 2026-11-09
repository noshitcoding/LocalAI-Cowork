"""Pure fixture regressions: no Docker, process launch, TLS listener or network."""
import contextlib
import hashlib
from http.client import HTTPMessage
import importlib.util
import io
import ipaddress
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch
import uuid

import qualify_desktop_identity as q


class RelayHalfCloseTests(unittest.TestCase):
    """Replay Docker attach closing its write half before delayed output."""

    def test_container_relay_drains_daemon_after_client_stdin_eof(self):
        source = object()
        output = io.BytesIO()
        class Target:
            def __init__(self):
                self.received = iter([b'HTTP/1.1 101 Switching Protocols\r\n\r\n', b'resume-fixture-ok\n', b''])
                self.sent, self.shutdowns = [], []
            def __enter__(self): return self
            def __exit__(self, *unused): pass
            def connect(self, path): self.path = path
            def sendall(self, data): self.sent.append(data)
            def recv(self, maximum): return next(self.received)
            def shutdown(self, how): self.shutdowns.append(how)
        target = Target()
        ready = iter([[source], [source], [target], [target], [target]])
        def select(readers, *unused):
            values = next(ready)
            self.assertTrue(all(value in readers for value in values))
            return values, [], []
        with patch.object(q.socket, 'socket', return_value=target), \
             patch.object(q.socket, 'AF_UNIX', 1, create=True), \
             patch.object(q.select, 'select', side_effect=select), \
             patch.object(q.os, 'read', side_effect=[b'attach-request', b'']), \
             patch.object(sys, 'stdin', SimpleNamespace(buffer=source)), \
             patch.object(sys, 'stdout', SimpleNamespace(buffer=output)):
            try:
                exec(compile(q.RELAY_PROGRAM, '<owned-docker-relay>', 'exec'), {})
            except SystemExit as error:
                self.assertEqual(error.code, 0)
        self.assertEqual(target.sent, [b'attach-request'])
        self.assertEqual(target.shutdowns, [q.socket.SHUT_WR])
        self.assertEqual(output.getvalue(), b'HTTP/1.1 101 Switching Protocols\r\n\r\nresume-fixture-ok\n')

    def test_native_relay_closes_only_stdin_and_retains_output_pump(self):
        relay = object.__new__(q.NativeDockerRelay)
        relay.parent = SimpleNamespace(argv=['owned-docker'], env={}, inspect_owned=Mock())
        relay.container, relay.closed = 'a' * 64, threading.Event()
        relay.lock, relay.slots, relay.processes = threading.Lock(), Mock(), set()
        pipe = SimpleNamespace(write=Mock(), flush=Mock(), close=Mock())
        stdout = SimpleNamespace(fileno=lambda: 123)
        process = SimpleNamespace(stdin=pipe, stdout=stdout, terminate=Mock(), wait=Mock(return_value=0))
        # Popen objects are hashable; SimpleNamespace deliberately is not.
        process = type('FakeRelayProcess', (), process.__dict__)()
        stream = Mock()
        stream.recv.side_effect = [b'attach-request', b'']
        ready = iter([[stream], [stream], [stdout], [stdout]])
        def select(readers, *unused):
            values = next(ready)
            self.assertTrue(all(value in readers for value in values))
            return values, [], []
        with patch.object(q.subprocess, 'Popen', return_value=process), \
             patch.object(q.select, 'select', side_effect=select), \
             patch.object(q.os, 'read', side_effect=[b'resume-fixture-ok\n', b'']):
            relay.relay(stream)
        pipe.write.assert_called_once_with(b'attach-request')
        pipe.close.assert_called_once()
        stream.sendall.assert_called_once_with(b'resume-fixture-ok\n')
        process.wait.assert_called_once_with(timeout=2)
        self.assertEqual(relay.processes, set())
        relay.slots.release.assert_called_once()


def binding():
    uid = lambda number: str(uuid.UUID(int=number))
    return {'version': 1, 'runner_id': uid(1), 'engine_id': 'private-fixture-engine',
        'scope_prefix': 'owned-fixture-', 'reservation_id': uid(2), 'attempt_id': uid(3),
        'container_id': 'a' * 64, 'container_name': 'owned-fixture-desktop-' + uuid.UUID(int=4).hex,
        'created_at': '2026-10-09T00:00:00Z', 'started_at': '2026-10-09T00:00:01Z',
        'run_id': uid(5), 'session_id': uid(4), 'lease_token': uid(6), 'workspace_id': uid(7),
        'workspace_volume': 'owned-fixture-run-' + uuid.UUID(int=7).hex}


def protocol():
    value = object.__new__(q.RunnerProtocol)
    value.signing_key = b'k' * 32
    value.expected_engine = 'private-fixture-engine'
    return value


def reply(method, path, request, result, domain, raw_binding=None, started=None):
    body = q.compact(result)
    fields = [method, path, '1700000000', '00000000-0000-4000-8000-000000000001']
    headers = HTTPMessage()
    name = 'x-cowork-stop-proof' if domain == q.EXECUTION_STOP_DOMAIN else 'x-cowork-desktop-proof'
    headers[name] = q.response_mac(b'k' * 32, domain, fields, request, body, raw_binding)
    if raw_binding is not None:
        headers['x-cowork-desktop-binding'] = q.base64.urlsafe_b64encode(raw_binding).rstrip(b'=').decode()
    return {'status': 200, 'headers': headers, 'body': body, 'fields': fields, 'request': request,
            'started': time.monotonic() if started is None else started}


class ProtocolTests(unittest.TestCase):
    def test_import_has_no_process_or_service_actions(self):
        spec = importlib.util.spec_from_file_location('private_runner_fixture_import', Path(q.__file__))
        module = importlib.util.module_from_spec(spec)
        with patch('subprocess.run') as run, patch('subprocess.Popen') as popen, patch('socket.socket') as socket:
            spec.loader.exec_module(module)
        run.assert_not_called(); popen.assert_not_called(); socket.assert_not_called()

    def test_stop_mac_matches_independent_frozen_rust_vector(self):
        request = b'{"version":1,"run_id":"00000000-0000-4000-8000-000000000002","lease_token":null,"cancel_lease":null,"attempts":[]}'
        response = b'{"version":1,"run_id":"00000000-0000-4000-8000-000000000002","lease_token":null,"cancel_lease":null,"attempts":[],"containers":[],"execution_stopped":true}'
        fields = ['POST', '/v1/execution-stop', '1700000000', '00000000-0000-4000-8000-000000000001']
        self.assertEqual(q.response_mac(b'k' * 32, q.EXECUTION_STOP_DOMAIN, fields, request, response),
            'v1:4d5ee56fc94d759dd2261b87b79cf41dbbfa08b262b4fa2e7f5c88ccd520393a')

    def test_binding_hash_uses_rust_field_order_regardless_of_header_object_order(self):
        value = binding()
        self.assertEqual(list(json.loads(q.canonical_binding_bytes(value))), list(q.BINDING_FIELDS))
        self.assertEqual(q.binding_digest(value), q.binding_digest(dict(reversed(list(value.items())))))
        self.assertEqual(list(value).index('attempt_id'), list(value).index('reservation_id') + 1)
        q.validate_binding(value)
        for mutation in [dict(value, unexpected=1), dict(value, runner_id=str(uuid.UUID(int=0))),
                         dict(value, container_id='A' * 64), dict(value, container_name='foreign-job'),
                         dict(value, started_at='not-a-time')]:
            with self.assertRaises((RuntimeError, ValueError)): q.validate_binding(mutation)

    def test_valid_proof_rejects_changed_request_domain_body_and_duplicate_header(self):
        value = reply('POST', '/v1/desktop-session-probe', b'actual-request', {'actual': True}, q.PROBE_DOMAIN)
        client = protocol(); client.verify(value, q.PROBE_DOMAIN)
        with self.assertRaises(RuntimeError): client.verify(value, q.DESKTOP_STOP_DOMAIN)
        for name, changed in [('request', b'changed'), ('body', b'changed')]:
            with self.assertRaises(RuntimeError): client.verify(dict(value, **{name: changed}), q.PROBE_DOMAIN)
        value['headers']['x-cowork-desktop-proof'] = value['headers']['x-cowork-desktop-proof']
        with self.assertRaises(RuntimeError): client.verify(value, q.PROBE_DOMAIN)

    def test_creation_verifies_original_header_bytes_and_exact_attempt_engine(self):
        client = protocol(); job = binding()
        spec = {'schema_version': 1, 'run_id': job['run_id'], 'session_id': job['session_id'],
                'workspace_id': job['workspace_id'], 'dimensions': {'width': 640, 'height': 480, 'scale_factor': 1.0}}
        result = {**{name: spec[name] for name in ['schema_version', 'run_id', 'session_id', 'dimensions']},
                  'container_name': job['container_name'], 'workspace_volume': job['workspace_volume']}
        raw = q.compact({'binding': job, 'running': True, 'paused': False, 'restarting': False})
        def request(method, path, body, **_kwargs): return reply(method, path, body, result, q.CREATE_DOMAIN, raw)
        client.signed_request = request
        self.assertEqual(client.create(spec, job['lease_token'], job['attempt_id']), (result, job))
        with self.assertRaises(RuntimeError): client.create(spec, job['lease_token'], str(uuid.uuid4()))
        client.expected_engine = 'other-engine'
        with self.assertRaises(RuntimeError): client.create(spec, job['lease_token'], job['attempt_id'])

    def test_delayed_probe_and_wrong_challenge_are_rejected_after_valid_mac(self):
        client = protocol(); job = binding()
        def request(method, path, body, **_kwargs):
            data = json.loads(body)
            return reply(method, path, body, {'version': 1, 'challenge': data['challenge'],
                'job': {'binding': job, 'running': True, 'paused': False, 'restarting': False}},
                q.PROBE_DOMAIN, started=time.monotonic() - 6)
        client.signed_request = request
        with self.assertRaises(RuntimeError): client.probe(job)
        def wrong(method, path, body, **_kwargs): return reply(method, path, body,
            {'version': 1, 'challenge': str(uuid.uuid4()), 'job': {'binding': job, 'running': True,
             'paused': False, 'restarting': False}}, q.PROBE_DOMAIN)
        client.signed_request = wrong
        with self.assertRaises(RuntimeError): client.probe(job)

    def test_exact_stop_response_and_signed_query_cannot_be_changed(self):
        client = protocol(); job = binding()
        def request(method, path, body=b'', **_kwargs):
            self.assertEqual(method, 'DELETE')
            self.assertEqual(path, '/v1/desktop-sessions/' + job['session_id'] + '?binding_sha256=' + q.binding_digest(job))
            return reply(method, path, body, {'version': 1, 'binding': job, 'execution_stopped': True}, q.DESKTOP_STOP_DOMAIN)
        client.signed_request = request
        self.assertTrue(client.stop(job)['execution_stopped'])
        altered = dict(job, started_at='2026-10-09T00:00:02Z')
        client.signed_request = lambda method, path, body=b'', **_kw: reply(method, path, body,
            {'version': 1, 'binding': altered, 'execution_stopped': True}, q.DESKTOP_STOP_DOMAIN)
        with self.assertRaises(RuntimeError): client.stop(job)

    def test_source_plan_does_not_contact_docker_or_create_fixture(self):
        argv = ['qualify', '--docker-client', '/not-executed/docker.exe', '--baseline-file', '/not-read/baseline.json',
            '--runner-image-id', 'sha256:' + 'a' * 64, '--gui-image-id', 'sha256:' + 'b' * 64,
            '--core-image-id', 'sha256:' + 'c' * 64, '--expected-runner-sha256', 'd' * 64]
        with (patch('sys.argv', argv), patch.object(q, 'ParentDocker') as parent,
                patch.object(q, 'private_root') as root, contextlib.redirect_stdout(io.StringIO()) as output):
            self.assertEqual(q.main(), 0)
        parent.assert_not_called(); root.assert_not_called()
        self.assertFalse(json.loads(output.getvalue())['actions'])


class ResponseLossProxyTests(unittest.TestCase):
    def forward(self, value, error_at=None):
        proxy = object.__new__(q.ResponseLossProxy)
        proxy.protocol = protocol(); proxy.protocol.origin = 'https://localhost:19443'; proxy.protocol.context = object()
        proxy.method, proxy.path = value['fields'][:2]
        proxy.reached, proxy.dropped = threading.Event(), threading.Event()
        headers = HTTPMessage()
        headers['Host'] = 'faultproxy.example'; headers['Connection'] = 'close'
        headers['Content-Length'] = str(len(value['request']))
        headers['x-cowork-timestamp'], headers['x-cowork-nonce'] = value['fields'][2:]
        headers['x-cowork-signature'] = 'actual-request-signature-forwarded-unchanged'
        handler = SimpleNamespace(command=proxy.method, path=proxy.path, headers=headers,
            rfile=io.BytesIO(value['request']), wfile=io.BytesIO(), close_connection=False,
            send_response=Mock(), send_header=Mock(), end_headers=Mock(), connection=SimpleNamespace(shutdown=Mock()))
        events = []
        def event(name):
            events.append(name)
            if name == error_at: raise q.http.client.HTTPException('private-upstream-failure')
        class Response:
            status = value['status']
            def read(self, size):
                event('read'); self.size = size; return value['body']
            def getheaders(self): return list(value['headers'].items())
        class Connection:
            # Deliberately match stdlib HTTPSConnection: no __enter__/__exit__.
            def request(self, *arguments): event('request'); self.arguments = arguments
            def getresponse(self): event('getresponse'); return Response()
            def close(self): event('close')
        upstream = Connection()
        self.assertFalse(hasattr(upstream, '__enter__'))
        with patch.object(q.http.client, 'HTTPSConnection', return_value=upstream) as factory:
            proxy.forward(handler)
        factory.assert_called_once_with('localhost', 19443, timeout=40, context=proxy.protocol.context)
        self.assertEqual(events[-1], 'close')
        return proxy, handler, upstream, events

    def stop_reply(self):
        job = binding(); path = '/v1/desktop-sessions/' + job['session_id'] + '?binding_sha256=' + q.binding_digest(job)
        return reply('DELETE', path, b'', {'version': 1, 'binding': job, 'execution_stopped': True}, q.DESKTOP_STOP_DOMAIN)

    def test_real_stop_reply_is_forwarded_verified_dropped_and_connection_closed(self):
        value = self.stop_reply(); proxy, handler, upstream, events = self.forward(value)
        self.assertEqual(events, ['request', 'getresponse', 'read', 'close'])
        self.assertTrue(proxy.reached.is_set()); self.assertTrue(proxy.dropped.is_set())
        self.assertEqual(upstream.arguments[:3], ('DELETE', value['fields'][1], b''))
        self.assertEqual(upstream.arguments[3]['x-cowork-signature'], 'actual-request-signature-forwarded-unchanged')
        self.assertNotIn('Host', upstream.arguments[3]); self.assertNotIn('Connection', upstream.arguments[3])
        handler.connection.shutdown.assert_called_once_with(q.socket.SHUT_RDWR)
        self.assertTrue(handler.close_connection); handler.send_response.assert_not_called()
        self.assertEqual(handler.wfile.getvalue(), b'')

    def test_non200_is_forwarded_unchanged_and_never_recorded_as_lost_stop(self):
        value = self.stop_reply(); value['status'], value['body'] = 429, b'actual-upstream-rejection'
        value['headers']['Content-Length'] = '999999'; value['headers']['Connection'] = 'keep-alive'
        proxy, handler, _, events = self.forward(value)
        self.assertEqual(events, ['request', 'getresponse', 'read', 'close'])
        self.assertTrue(proxy.reached.is_set()); self.assertFalse(proxy.dropped.is_set())
        handler.send_response.assert_called_once_with(429)
        handler.send_header.assert_any_call('Content-Length', str(len(value['body'])))
        self.assertNotIn(unittest.mock.call('Content-Length', '999999'), handler.send_header.call_args_list)
        self.assertEqual(handler.wfile.getvalue(), value['body']); handler.connection.shutdown.assert_not_called()

    def test_request_response_and_read_failure_all_close_without_dropped_success(self):
        for stage in ['request', 'getresponse', 'read']:
            with self.subTest(stage=stage):
                proxy, handler, _, _ = self.forward(self.stop_reply(), error_at=stage)
                self.assertEqual(proxy.reached.is_set(), stage != 'request')
                self.assertFalse(proxy.dropped.is_set()); self.assertTrue(handler.close_connection)
                handler.send_response.assert_not_called(); handler.connection.shutdown.assert_not_called()

    def test_unsigned_or_wrong_binding_200_cannot_be_used_as_lost_stop_evidence(self):
        invalid_mac = self.stop_reply(); invalid_mac['headers'].replace_header('x-cowork-desktop-proof', 'v1:' + '0' * 64)
        changed = self.stop_reply(); changed['body'] = q.compact({'version': 1,
            'binding': dict(binding(), started_at='2026-10-09T00:00:02Z'), 'execution_stopped': True})
        changed['headers'].replace_header('x-cowork-desktop-proof', q.response_mac(b'k' * 32, q.DESKTOP_STOP_DOMAIN,
            changed['fields'], changed['request'], changed['body']))
        for value in [invalid_mac, changed]:
            with self.subTest(body=value['body']):
                proxy, handler, _, _ = self.forward(value)
                self.assertTrue(proxy.reached.is_set()); self.assertFalse(proxy.dropped.is_set())
                handler.send_response.assert_not_called(); handler.connection.shutdown.assert_not_called()

    def test_creation_drop_verifies_original_body_header_and_exact_attempt(self):
        job = binding(); spec = {'schema_version': 1, 'run_id': job['run_id'], 'session_id': job['session_id'],
            'workspace_id': job['workspace_id'], 'dimensions': {'width': 640, 'height': 480, 'scale_factor': 1.0}}
        path = '/v2/desktop-sessions?lease_token=' + job['lease_token'] + '&attempt_id=' + job['attempt_id']
        result = {**{name: spec[name] for name in ['schema_version', 'run_id', 'session_id', 'dimensions']},
            'container_name': job['container_name'], 'workspace_volume': job['workspace_volume']}
        raw = q.compact({'binding': job, 'running': True, 'paused': False, 'restarting': False})
        value = reply('POST', path, q.compact(spec), result, q.CREATE_DOMAIN, raw)
        proxy, handler, upstream, _ = self.forward(value)
        self.assertTrue(proxy.dropped.is_set()); self.assertEqual(upstream.arguments[:3], ('POST', path, q.compact(spec)))
        handler.connection.shutdown.assert_called_once()
        value['fields'][1] = path.replace(job['attempt_id'], str(uuid.UUID(int=8)))
        value['headers'].replace_header('x-cowork-desktop-proof', q.response_mac(b'k' * 32, q.CREATE_DOMAIN,
            value['fields'], value['request'], value['body'], raw))
        proxy, handler, _, _ = self.forward(value)
        self.assertFalse(proxy.dropped.is_set()); handler.connection.shutdown.assert_not_called()


class InventoryTests(unittest.TestCase):
    def seed_fixture(self, exit_code=0):
        parent = SimpleNamespace(volumes={'own-volume': 'own-scope'},
            create_container=Mock(return_value='a' * 64), run=Mock(), binary=Mock(return_value=b'own-read'),
            inspect_owned=Mock(return_value={'State': {'ExitCode': exit_code, 'Status': 'exited', 'Running': False}}),
            remove_container=Mock())
        fixture = object.__new__(q.RunnerFixture)
        fixture.parent, fixture.scope, fixture.name, fixture.core_image_id, fixture.evidence = parent, 'own-scope', 'own-helper', 'sha256:' + 'b' * 64, {}
        fixture.state_volume = 'own-volume'
        return fixture, parent

    def test_v1_and_v2_register_the_exact_runner_volume_before_creation(self):
        for workspace in [None, binding()['workspace_id']]:
            with self.subTest(workspace=workspace):
                fixture, parent = self.seed_fixture()
                fixture.resource_prefix = binding()['scope_prefix']
                fixture.runs, fixture.workspace_volumes, fixture.sandbox_ids = set(), {}, set()
                fixture.seed = Mock(); parent.register_sandbox = Mock()
                def create_volume(name, scope): parent.volumes[name] = scope; return name
                parent.create_volume = Mock(side_effect=create_volume)
                run = binding()['run_id']; volume = fixture.register_run(run, workspace)
                self.assertEqual(volume, fixture.resource_prefix + 'run-' + uuid.UUID(workspace or run).hex)
                parent.create_volume.assert_called_once_with(volume, fixture.scope)
                fixture.seed.assert_called_once_with(volume, {}, owner=10001)
                self.assertEqual(fixture.workspace_volumes[volume], (run, workspace))
                job = dict(binding(), workspace_id=workspace, workspace_volume=volume)
                fixture.inspect(job); parent.register_sandbox.assert_called_once_with(job)
                fixture.register_run(run, workspace)
                self.assertEqual(parent.create_volume.call_count, 1); self.assertEqual(fixture.seed.call_count, 1)
                with self.assertRaisesRegex(RuntimeError, 'unregistered_fixture_workspace_binding'):
                    fixture.inspect(dict(job, workspace_volume=fixture.resource_prefix + 'workspace-' + uuid.UUID(workspace or run).hex))
                self.assertEqual(parent.register_sandbox.call_count, 1)

    def test_registered_workspace_reader_uses_10001_without_caps_and_state_tls_keep_root(self):
        fixture, parent = self.seed_fixture(); fixture.resource_prefix = binding()['scope_prefix']
        run, workspace = binding()['run_id'], binding()['workspace_id']
        volume = fixture.resource_prefix + 'run-' + uuid.UUID(workspace).hex
        fixture.runs, fixture.workspace_volumes = {run}, {volume: (run, workspace)}
        fixture.cert_volume = 'own-tls'; parent.volumes.update({volume: fixture.scope, 'own-tls': fixture.scope})
        for name, user in [(volume, '10001'), (fixture.state_volume, '0'), (fixture.cert_volume, '0')]:
            with self.subTest(volume=name):
                self.assertEqual(fixture.read_volume(name, "print('own-read')"), b'own-read')
                arguments = parent.create_container.call_args.args[3]
                self.assertEqual(arguments[arguments.index('--user') + 1], user)
                self.assertIn('--read-only', arguments); self.assertIn('--network', arguments)
                self.assertEqual(arguments[arguments.index('--volume') + 1], name + ':/in:ro')
                self.assertNotIn('--cap-add', arguments)
        parent.create_container.reset_mock()
        parent.volumes.update({'unregistered-same-scope': fixture.scope, 'foreign-volume': 'other-scope'})
        for name in ['unregistered-same-scope', 'foreign-volume']:
            with self.assertRaises(RuntimeError): fixture.read_volume(name, 'must not execute')
        fixture.workspace_volumes['unregistered-same-scope'] = (run, workspace)
        with self.assertRaisesRegex(RuntimeError, 'workspace_volume_binding_mismatch'):
            fixture.read_volume('unregistered-same-scope', 'must not execute')
        fixture.runs.clear()
        with self.assertRaisesRegex(RuntimeError, 'workspace_volume_binding_mismatch'):
            fixture.read_volume(volume, 'must not execute')
        parent.create_container.assert_not_called()

    def test_seed_only_adds_chown_and_preserves_exact_stdin_without_permission_fallback(self):
        fixture, parent = self.seed_fixture()
        payload = {'owned.txt': b'private fixture data'}
        fixture.seed('own-volume', payload, owner=10001)
        arguments = parent.create_container.call_args.args[3]
        self.assertEqual(arguments[arguments.index('--cap-add'):arguments.index('--cap-add') + 2], ['--cap-add', 'CHOWN'])
        self.assertNotIn('FOWNER', arguments); self.assertNotIn('DAC_OVERRIDE', arguments)
        self.assertIn('--read-only', arguments)
        self.assertEqual(parent.run.call_args.args, ('start', '--attach', '--interactive', 'a' * 64))
        self.assertEqual(json.loads(parent.run.call_args.kwargs['data']), {'owned.txt': payload['owned.txt'].hex()})
        self.assertEqual(parent.run.call_count, 1)
        parent.remove_container.assert_called_once_with('a' * 64)
        fixture.read_volume('own-volume', "print('own-read')")
        reader_arguments = parent.create_container.call_args.args[3]
        self.assertNotIn('--cap-add', reader_arguments)
        # The common create wrapper still removes all capabilities; the seed
        # command is the single explicit, purpose-scoped override above.
        common = object.__new__(q.ParentDocker)
        common.containers, common.pending_creates = {}, {}
        common.assert_preserved = Mock(); common.run = Mock(side_effect=['', 'c' * 64])
        common.json = Mock(return_value=[{'Image': 'sha256:' + 'd' * 64}])
        common.create_container('own-default-helper', 'own-scope', 'sha256:' + 'd' * 64, [])
        create_arguments = common.run.call_args.args
        self.assertEqual(create_arguments[create_arguments.index('--cap-drop') + 1], 'ALL')
        self.assertNotIn('--cap-add', create_arguments)

    def test_seed_writes_and_chmods_before_any_ownership_transfer(self):
        fixture, parent = self.seed_fixture()
        fixture.seed('own-volume', {'owned.txt': b'actual tempfile contents'}, owner=10001)
        program = parent.create_container.call_args.args[4][1]
        with tempfile.TemporaryDirectory() as directory:
            # Run the actual initializer against a private temp directory.
            # Simulate CHOWN without requiring privilege; disallow every
            # read/write/chmod after ownership of that inode has changed.
            program = program.replace("directory='/out'", 'directory=' + repr(directory))
            original_open, original_chmod = open, os.chmod
            transferred, actions = set(), []
            def allowed(path):
                self.assertNotIn(directory, transferred)
                self.assertNotIn(str(path), transferred)
            def tracked_open(path, *args, **kwargs):
                allowed(path); actions.append(('open', str(path)))
                return original_open(path, *args, **kwargs)
            def tracked_chmod(path, mode):
                allowed(path); actions.append(('chmod', str(path), mode))
                return original_chmod(path, mode)
            def tracked_chown(path, uid, gid):
                self.assertEqual((uid, gid), (10001, 10001))
                transferred.add(str(path)); actions.append(('chown', str(path)))
            data = parent.run.call_args.kwargs['data'].decode()
            with patch('sys.stdin', io.StringIO(data)), patch('builtins.open', side_effect=tracked_open), \
                    patch('os.chmod', side_effect=tracked_chmod), patch('os.chown', side_effect=tracked_chown, create=True):
                exec(program, {})
            self.assertEqual((Path(directory) / 'owned.txt').read_bytes(), b'actual tempfile contents')
            self.assertEqual(actions[-1], ('chown', directory))
            first_transfer = next(index for index, value in enumerate(actions) if value[0] == 'chown')
            self.assertTrue(all(value[0] == 'chown' for value in actions[first_transfer:]))
            self.assertEqual(len(transferred), 2)

    def test_seed_failure_records_observed_exit_before_cleanup_without_secret_output(self):
        fixture, parent = self.seed_fixture(exit_code=1)
        parent.run.side_effect = q.FixtureCommandError('docker.start', 'permission_denied', 1)
        with self.assertRaises(q.FixtureSeedError) as failed:
            fixture.seed('own-volume', {'owned.txt': b'private seed secret'}, owner=10001)
        detail = q.public_failure(failed.exception)
        self.assertEqual(detail['stage'], 'workspace_initialization')
        self.assertEqual(detail['container_exit_code'], 1)
        self.assertEqual(detail['container_status'], 'exited')
        self.assertEqual(detail['cause']['code'], 'permission_denied')
        self.assertEqual(fixture.evidence['seed_failures'][0]['container_id'], 'a' * 64)
        self.assertNotIn('private', json.dumps(detail))
        self.assertEqual(parent.run.call_count, 1)
        parent.remove_container.assert_called_once_with('a' * 64)

    def test_explicit_uuid_subnet_rotates_around_full_and_partial_ipam(self):
        scope = str(uuid.UUID(int=123))
        first = q.control_subnet(scope, [])
        occupied = ipaddress.ip_network(first)
        next_value = q.control_subnet(scope, [{'IPAM': {'Config': [{'Subnet': str(next(occupied.subnets(prefixlen_diff=1)))},
            {'Subnet': 'fd12:3456::/64'}]}}])
        self.assertNotEqual(first, next_value)
        self.assertFalse(ipaddress.ip_network(next_value).overlaps(occupied))
        self.assertEqual(q.control_subnet(scope, []), first)
        value = q.control_subnet(scope, [{'IPAM': {'Config': [{'Subnet': '10.0.0.0/8'}, {'Subnet': '172.16.0.0/12'}]}}])
        self.assertTrue(ipaddress.ip_network(value).subnet_of(ipaddress.ip_network('192.168.0.0/16')))

    def test_subnet_exhaustion_and_malformed_inventory_fail_closed(self):
        scope = str(uuid.uuid4())
        with self.assertRaisesRegex(RuntimeError, 'private_control_subnets_exhausted'):
            q.control_subnet(scope, [{'IPAM': {'Config': [{'Subnet': block} for block in
                ['10.0.0.0/8', '172.16.0.0/12', '192.168.0.0/16']]}}])
        for networks in [None, [{}], [{'IPAM': {'Config': [{'Subnet': '10.0.0.1/24'}]}}],
                         [{'IPAM': {'Config': [None]}}], [{'IPAM': {'Config': []}}] * 10001]:
            with self.assertRaises(RuntimeError): q.control_subnet(scope, networks)

    def test_network_create_uses_complete_actual_inventory_and_explicit_subnet(self):
        parent = object.__new__(q.ParentDocker)
        parent.networks, parent.network_subnets, parent.pending_creates = {}, {}, {}
        parent.run = Mock(side_effect=['', 'a' * 64, 'b' * 64])
        parent.json = Mock(return_value=[{'Id': 'a' * 64, 'IPAM': {'Config': [{'Subnet': '10.0.0.0/8'}]}}])
        scope = str(uuid.uuid4())
        self.assertEqual(parent.create_network('own-network', scope), 'own-network')
        call = parent.run.call_args.args
        self.assertEqual(call[:3], ('network', 'create', '--internal'))
        self.assertEqual(call[3:5], ('--subnet', parent.network_subnets['own-network']))
        self.assertTrue(ipaddress.ip_network(call[4]).subnet_of(ipaddress.ip_network('172.16.0.0/12')))
        self.assertEqual(parent.networks, {'b' * 64: ('own-network', scope)})
        self.assertFalse(parent.pending_creates)
        parent.json.assert_called_once_with('network', 'inspect', 'a' * 64)

    def test_changed_network_inventory_is_rejected_before_mutation(self):
        parent = object.__new__(q.ParentDocker)
        parent.networks, parent.network_subnets, parent.pending_creates = {}, {}, {}
        parent.run = Mock(side_effect=['', 'a' * 64])
        parent.json = Mock(return_value=[{'Id': 'b' * 64, 'IPAM': {'Config': []}}])
        with self.assertRaisesRegex(RuntimeError, 'parent_network_inventory_changed'):
            parent.create_network('own-network', str(uuid.uuid4()))
        self.assertEqual(parent.run.call_count, 2)
        self.assertFalse(parent.networks or parent.pending_creates)

    def test_command_failure_is_typed_and_never_reports_argv_or_stderr_secrets(self):
        completed = SimpleNamespace(returncode=1, stdout=b'', stderr=b'private-command-secret: all predefined address pools have been fully subnetted')
        with patch('subprocess.run', return_value=completed):
            with self.assertRaises(q.FixtureCommandError) as failed:
                q.command(['/not-executed/docker.exe', '--env', 'private-argv-secret'], action='docker.network.create')
        public = q.public_failure(failed.exception)
        self.assertEqual(public, {'code': 'docker_address_pool_exhausted', 'classification': 'command',
            'action': 'docker.network.create', 'exit_code': 1})
        self.assertNotIn('private', json.dumps(public))
        with patch('subprocess.run', side_effect=subprocess.TimeoutExpired(['private-secret'], 1)):
            with self.assertRaises(q.FixtureCommandError) as failed:
                q.command(['/not-executed/docker.exe'], action='docker.create')
        self.assertEqual(q.public_failure(failed.exception)['classification'], 'unknown_command_outcome')
        self.assertNotIn('private', str(failed.exception))

    def test_report_inventory_is_exact_scope_filtered_and_readonly(self):
        parent = object.__new__(q.ParentDocker)
        parent.containers = {'a' * 64: {'scope': 'own', 'name': 'own-helper'},
            'b' * 64: {'scope': 'foreign', 'name': 'foreign-helper'}, 'c' * 64: {'run_id': 'own-run', 'reservation_id': 'own-reservation'}}
        parent.networks = {'d' * 64: ('own-network', 'own')}
        parent.network_subnets = {'own-network': '10.20.30.0/24'}
        parent.volumes = {'own-state': 'own', 'foreign-state': 'foreign'}
        parent.pending_creates = {('network', 'own-network'): 'own', ('container', 'foreign-helper'): 'foreign'}
        parent._scope_runs = {'own': {'own-run'}}; parent.run = Mock()
        value = parent.owned_inventory('own')
        self.assertEqual([row['container_id'] for row in value['containers']], ['a' * 64, 'c' * 64])
        self.assertEqual(value['pending_creates'], [{'kind': 'network', 'name': 'own-network', 'scope': 'own', 'subnet': '10.20.30.0/24'}])
        self.assertNotIn('foreign', json.dumps(value)); parent.run.assert_not_called()

    @unittest.skipUnless(os.name == 'posix', 'Runner fixture requires POSIX ownership and permission modes')
    def test_fixture_identity_is_private_and_durable_before_first_docker_read(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); root.chmod(0o700)
            parent = SimpleNamespace(engine_id='own-engine', inspect_image=Mock(side_effect=RuntimeError('stop-before-docker')))
            fixture = q.RunnerFixture(parent, root, 'sha256:' + 'a' * 64, 'sha256:' + 'b' * 64,
                'sha256:' + 'c' * 64, 'd' * 64)
            with self.assertRaisesRegex(RuntimeError, 'stop-before-docker'): fixture.start()
            path = root / 'fixture.json'; value = json.loads(path.read_text())
            self.assertEqual(value['scope'], fixture.scope); self.assertEqual(value['resource_prefix'], fixture.resource_prefix)
            self.assertEqual(value['parent_engine_id'], 'own-engine')
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            self.assertNotIn(fixture.signing_key.decode(), path.read_text())

    def test_prior_passed_checks_survive_later_actual_matrix_failure(self):
        job = binding(); client = Mock()
        client.create.return_value = ({}, job)
        client.signed_request.side_effect = q.FixtureCommandError('docker.inspect', 'command_failed', 1)
        fixture = SimpleNamespace(protocol=client, parent=SimpleNamespace(assert_preserved=Mock()), desktop_spec=lambda: {}, ledger=lambda: [],
            inspect=lambda _binding: {'Id': job['container_id'], 'Created': job['created_at'],
                                     'State': {'StartedAt': job['started_at'], 'Running': True}})
        report = {}
        with self.assertRaises(q.FixtureCommandError): q.qualify(fixture, report)
        self.assertEqual(report['checks'], ['actual_engine_container_created_started_binding'])
        self.assertEqual(report['phase'], 'attempt_replay_denied')
        self.assertEqual(report['job_binding'], job)

    def test_baseline_requires_original_count_unique_ids_and_immutable_images(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / 'baseline.json'
            values = [{'ID': format(index, '064x'), 'Image': 'sha256:' + 'a' * 64} for index in range(39)]
            path.write_text(json.dumps(values)); self.assertEqual(len(q.baseline_records(path)), 39)
            path.write_text(json.dumps(values[:-1]))
            with self.assertRaises(RuntimeError): q.baseline_records(path)
            values[-1] = values[0]; path.write_text(json.dumps(values))
            with self.assertRaises(RuntimeError): q.baseline_records(path)

    def test_original_and_changed_container_ownership_cannot_be_removed(self):
        parent = object.__new__(q.ParentDocker)
        original = 'f' * 64; owned = 'a' * 64
        parent.baseline = {original: 'sha256:' + 'b' * 64}
        parent.containers = {owned: {'run_id': binding()['run_id'], 'reservation_id': binding()['reservation_id']}}
        parent.run = Mock(); parent.exists = Mock(return_value=True)
        parent.json = Mock(return_value=[{'Id': owned, 'Config': {'Labels': {'dev.opencowork.run_id': str(uuid.uuid4()),
            'dev.opencowork.admission': binding()['reservation_id']}}}])
        with self.assertRaises(RuntimeError): parent.register_sandbox(dict(binding(), container_id=original))
        with self.assertRaises(RuntimeError): parent.remove_container(original)
        with self.assertRaises(RuntimeError): parent.remove_container(owned)
        parent.run.assert_not_called()

    def test_same_id_cannot_overwrite_an_existing_different_owner(self):
        parent = object.__new__(q.ParentDocker); parent.baseline = {}; parent.containers = {}
        parent.register_sandbox(binding())
        with self.assertRaises(RuntimeError): parent.register_sandbox(dict(binding(), run_id=str(uuid.uuid4())))

    def test_unknown_create_retains_state_and_workspace_in_cleanup(self):
        fixture = object.__new__(q.RunnerFixture)
        fixture._stop_native = Mock(); fixture.parent = Mock(); fixture.parent.containers = {}
        fixture.container, fixture.relay, fixture.state_volume = None, None, 'held-state'
        fixture.discover = Mock(); fixture.ledger = Mock(return_value=[{'container_id': None}])
        with self.assertRaisesRegex(RuntimeError, 'unresolved_create'): fixture.cleanup()
        fixture.parent.cleanup_scope.assert_not_called()

    def test_timed_out_fixture_create_reconciles_exact_owned_id_without_replaying_create(self):
        parent = object.__new__(q.ParentDocker)
        parent.baseline, parent.containers, parent.pending_creates = {}, {}, {}
        parent.assert_preserved = Mock()
        parent.run = Mock(side_effect=['', subprocess.TimeoutExpired('own create', 1)])
        with self.assertRaises(subprocess.TimeoutExpired):
            parent.create_container('owned-helper', 'own-uuid-scope', 'sha256:' + 'a' * 64, [])
        self.assertEqual(parent.pending_creates, {('container', 'owned-helper'): 'own-uuid-scope'})
        parent.run.side_effect = None; parent.run.return_value = 'b' * 64
        parent.json = Mock(return_value=[{'Id': 'b' * 64, 'Name': '/owned-helper',
            'Config': {'Labels': {q.LABEL: 'own-uuid-scope'}}}])
        parent.reconcile_fixture_creates('own-uuid-scope')
        self.assertEqual(parent.containers, {'b' * 64: {'scope': 'own-uuid-scope', 'name': 'owned-helper'}})
        self.assertFalse(parent.pending_creates)
        self.assertEqual(sum(call.args[0] == 'create' for call in parent.run.call_args_list), 1)

    def test_unknown_fixture_create_absence_holds_state_before_any_cleanup(self):
        parent = object.__new__(q.ParentDocker)
        parent.baseline, parent.containers, parent.networks = {}, {}, {}
        parent.volumes = {'own-state': 'scope'}
        parent.pending_creates = {('container', 'late-helper'): 'scope'}
        parent.assert_preserved = Mock(); parent.run = Mock(return_value='')
        with self.assertRaisesRegex(RuntimeError, 'fixture_create_outcome_unknown'):
            parent.cleanup_scope('scope')
        self.assertEqual(parent.volumes, {'own-state': 'scope'})
        self.assertEqual(parent.pending_creates, {('container', 'late-helper'): 'scope'})
        self.assertEqual(parent.run.call_count, 1)
        self.assertEqual(parent.run.call_args.args[0], 'ps')

    def test_unknown_fixture_create_never_adopts_wrong_owner(self):
        parent = object.__new__(q.ParentDocker)
        parent.baseline, parent.containers = {}, {}
        parent.pending_creates = {('container', 'own-helper'): 'scope'}
        parent.run = Mock(return_value='a' * 64)
        parent.json = Mock(return_value=[{'Id': 'a' * 64, 'Name': '/own-helper',
            'Config': {'Labels': {q.LABEL: 'foreign-scope'}}}])
        with self.assertRaisesRegex(RuntimeError, 'fixture_create_owner_unknown'):
            parent.reconcile_fixture_creates('scope')
        self.assertFalse(parent.containers)
        self.assertTrue(parent.pending_creates)

    def test_native_binary_and_archive_checks_are_readonly_and_exact(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / 'runner'; content = b'\x7fELFowned-fixture-only'; path.write_bytes(content)
            digest = hashlib.sha256(content).hexdigest()
            self.assertEqual(q.checked_native_hash(path, digest), digest)
            with self.assertRaises(RuntimeError): q.checked_native_hash(path, 'f' * 64)
            payload = io.BytesIO()
            with tarfile.open(fileobj=payload, mode='w') as archive:
                entry = tarfile.TarInfo('cowork-runner'); entry.size = len(content)
                archive.addfile(entry, io.BytesIO(content))
            self.assertEqual(q.binary_from_tar(payload.getvalue()), digest)
            payload = io.BytesIO()
            with tarfile.open(fileobj=payload, mode='w') as archive:
                entry = tarfile.TarInfo('unsafe'); entry.type = tarfile.SYMTYPE; entry.linkname = '/foreign'
                archive.addfile(entry)
            with self.assertRaises(RuntimeError): q.binary_from_tar(payload.getvalue())


class WebSocketTests(unittest.TestCase):
    def test_real_wire_adapter_reads_binary_and_masks_pong_and_client_bytes(self):
        stream = Mock(); stream.recv.side_effect = AssertionError('frame is already buffered')
        wire = q.WebSocketBytes(stream, b'\x89\x01x\x82\x0cRFB 003.008\n')
        self.assertEqual(wire.read(12), b'RFB 003.008\n')
        pong = stream.sendall.call_args.args[0]
        self.assertEqual(pong[0], 0x8a); self.assertTrue(pong[1] & 0x80)
        self.assertEqual(pong[6] ^ pong[2], ord('x'))
        wire.send(b'abc')
        raw = stream.sendall.call_args.args[0]
        self.assertEqual(bytes(value ^ raw[2 + index % 4] for index, value in enumerate(raw[6:])), b'abc')

    def test_oversized_masked_and_closed_server_frames_fail_before_allocation(self):
        for frame in [b'\x82\x7f' + struct.pack('>Q', 65537), b'\x82\x80', b'\x88\x00']:
            with self.assertRaises(RuntimeError): q.WebSocketBytes(Mock(), frame).read(1)
        stream = Mock(); stream.recv.return_value = b''
        with self.assertRaises(RuntimeError): q.WebSocketBytes(stream).read(12)


if __name__ == '__main__':
    unittest.main()
