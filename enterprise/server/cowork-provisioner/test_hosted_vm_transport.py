import hashlib
import copy
from datetime import datetime, timezone
import ipaddress
import gzip
import io
import json
from pathlib import Path
import struct
import shutil
import os
import sys
import tarfile
import tempfile
import unittest
import uuid
from types import SimpleNamespace
from unittest.mock import Mock, patch
from contextlib import ExitStack
import socket
import ssl
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.error import HTTPError
from urllib.request import build_opener, ProxyHandler, HTTPSHandler, Request

import hosted_vm_transport as transport
import qualify_hosted_vm as harness


class HostedTransportTests(unittest.TestCase):
    def test_secret_seed_accepts_typed_s3_names_and_rejects_paths_before_commands(self):
        parent = object.__new__(harness.ParentDocker)
        parent.scope = 'owned-fixture'; parent.volumes = {'owned-volume'}
        parent.preserved = Mock(); parent.run = Mock()
        parent.seed('owned-volume', 'trusted-core', {'s3_user': b'fixture', 's3_password': b'private'})
        self.assertEqual(parent.run.call_count, 1)
        self.assertEqual(json.loads(parent.run.call_args.kwargs['data']), {'s3_user': b'fixture'.hex(), 's3_password': b'private'.hex()})
        for volume, values in [('foreign-volume', {'s3_user': b'fixture'}), ('owned-volume', {'../outside': b'private'}),
                               ('owned-volume', {'/absolute': b'private'}), ('owned-volume', {'s3_password': 'not bytes'}),
                               ('owned-volume', {'s3_password': b'x' * 16385})]:
            parent.run.reset_mock()
            with self.assertRaisesRegex(RuntimeError, 'invalid_seed_scope'):
                parent.seed(volume, 'trusted-core', values)
            parent.run.assert_not_called()

    def test_control_subnet_avoids_all_overlapping_ipv4_and_reads_ipv6_inventory(self):
        scope = 'cowork-hosted-vm-' + uuid.uuid4().hex
        first = harness.control_subnet(scope, [])
        inventory = [{'IPAM': {'Config': [{'Subnet': first}, {'Subnet': 'fd00:1234::/64'}, {'Subnet': '172.16.0.0/12'}]}}]
        chosen = ipaddress.ip_network(harness.control_subnet(scope, inventory))
        self.assertFalse(chosen.overlaps(ipaddress.ip_network(first)))
        self.assertFalse(chosen.overlaps(ipaddress.ip_network('172.16.0.0/12')))
        self.assertEqual(chosen.prefixlen, 24)
        self.assertTrue(chosen.is_private)
        with self.assertRaisesRegex(RuntimeError, 'invalid_parent_network_subnet'):
            harness.control_subnet(scope, [{'IPAM': {'Config': [{'Subnet': 'malformed-v6::not-cidr'}]}}])

    def test_control_subnet_exhaustion_cannot_change_global_networks(self):
        inventory = [{'IPAM': {'Config': [{'Subnet': value} for value in ['10.0.0.0/8', '172.16.0.0/12', '192.168.0.0/16']]}}]
        with self.assertRaisesRegex(RuntimeError, 'private_control_subnets_exhausted'):
            harness.control_subnet('cowork-hosted-vm-' + uuid.uuid4().hex, inventory)
        parent = object.__new__(harness.ParentDocker); parent.scope = 'own-uuid'; parent.networks = set()
        parent.run = Mock(side_effect=['', 'a' * 64, 'network-id'])
        parent.json = Mock(return_value=[{'Id': 'a' * 64, 'IPAM': {'Config': [{'Subnet': '10.0.0.0/8'}]}}])
        parent.create_network()
        commands = [call.args for call in parent.run.call_args_list]
        self.assertTrue(all(command[:2] in [('network', 'ls'), ('network', 'create')] for command in commands))
        created = commands[-1]
        self.assertIn('--internal', created); self.assertIn('--subnet', created)
        self.assertNotIn('prune', created)
        self.assertEqual(created[-1], parent.scope + '-control')

    def test_parent_failure_code_does_not_render_argv_or_secret_exception(self):
        parent = object.__new__(harness.ParentDocker)
        parent.argv = ['explicit-parent-client', '--context', 'desktop-linux']; parent.env = {}
        with patch.object(harness, 'call', side_effect=RuntimeError('secret argv must never be retained')):
            with self.assertRaisesRegex(RuntimeError, '^parent_docker_failed:run$'):
                parent.run('run', '--env', 'PASSWORD=private-secret', 'fixture')
        self.assertEqual(parent.last_action, 'run')

    @unittest.skipUnless(os.name == 'posix', 'POSIX durable private fixture directory')
    def test_failure_report_is_outside_private_fixture_and_survives_fixture_removal(self):
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory); home.chmod(0o700)
            with patch.object(harness.Path, 'home', return_value=home):
                scope = 'cowork-hosted-vm-' + uuid.uuid4().hex
                root = harness.private_fixture_root(scope)
                self.assertEqual(root.parent, home / '.local/share/cowork/qualification')
                report = home / 'evidence/attempt.json'
                harness.durable_report(report, {'scope': scope, 'passed': False, 'failure_code': 'parent_docker_failed:run'})
                shutil.rmtree(root)
                self.assertEqual(json.loads(report.read_text())['failure_code'], 'parent_docker_failed:run')
                harness.durable_report(report, {'scope': scope, 'cleanup_requires_review': False})
                self.assertFalse(json.loads(report.read_text())['cleanup_requires_review'])

    def test_parent_client_environment_cannot_be_replaced_by_operator_config(self):
        with tempfile.TemporaryDirectory() as directory:
            client = Path(directory) / 'docker.exe'; client.touch()
            baseline = [{'ID': 'a' * 64, 'Image': 'sha256:' + 'b' * 64}]
            image = {'Id': 'a' * 64, 'Image': 'sha256:' + 'b' * 64}
            with patch.dict(os.environ, {'DOCKER_CONFIG': 'parent-config', 'PATH': 'parent-path',
                    'PGPASSWORD': 'must-not-cross', 'DOCKER_HOST': 'foreign-daemon'}, clear=True), \
                    patch.object(harness.ParentDocker, 'json', side_effect=[{'ID': 'daemon'}, [image]]), \
                    patch.object(harness.ParentDocker, 'snapshot', return_value={image['Id']: image['Image']}):
                parent = harness.ParentDocker(str(client), 'desktop-linux', 'own-fixture', baseline)
                os.environ.update({'DOCKER_CONFIG': 'linux-operator-config', 'PATH': 'operator-tools'})
                with patch.object(harness, 'call', return_value=b'[]') as invoke:
                    parent.run('ps')
                selected = invoke.call_args.kwargs['env']
                self.assertEqual(selected['DOCKER_CONFIG'], 'parent-config')
                self.assertEqual(selected['PATH'], 'parent-path')
                self.assertNotIn('PGPASSWORD', selected)
                self.assertNotIn('DOCKER_HOST', selected)
                with patch.object(transport.subprocess, 'Popen') as popen:
                    transport.PipeSocket(parent.argv + ['exec', 'own-fixture'], env=parent.env)
                self.assertEqual(popen.call_args.kwargs['env'], selected)

    def test_runtime_manager_cannot_call_admin_or_claim_endpoints(self):
        for path in transport.MANAGER_PATHS:
            self.assertTrue(transport.authorized('manager', 'POST', path))
            self.assertFalse(transport.authorized('manager', 'GET', path))
            self.assertFalse(transport.authorized('manager', 'POST', path + '?admin=true'))
        for path in ['/management/runner/v1/claim', '/management/runner/v1/pending',
                     '/management/v1/organisations', '/api/v1/auth/bootstrap',
                     'https://foreign.invalid/management/runner/v1/instance-access']:
            self.assertFalse(transport.authorized('manager', 'POST', path))

    def test_runtime_storage_is_limited_to_one_new_tenant_bucket(self):
        bucket = 'cowork-' + uuid.uuid4().hex
        self.assertTrue(transport.authorized('s3', 'PUT', '/' + bucket + '/marker.bin', bucket=bucket))
        self.assertTrue(transport.authorized('s3', 'GET', '/' + bucket + '?location', bucket=bucket))
        for path in ['/', '/minio/admin/v3/add-user', '/' + bucket + '-foreign/marker',
                     'https://foreign.invalid/' + bucket, '/otherbucket/marker']:
            self.assertFalse(transport.authorized('s3', 'GET', path, bucket=bucket))

    def test_pg_runtime_startup_requires_exact_database_and_user(self):
        database = 'ci_' + uuid.uuid4().hex
        def startup(user, db, extra=b''):
            payload = struct.pack('!I', 196608) + b'user\0' + user.encode() + b'\0database\0' + db.encode() + b'\0' + extra + b'\0'
            return struct.pack('!I', len(payload) + 4) + payload
        self.assertTrue(transport.startup_database(startup(database, database), database))
        self.assertTrue(transport.startup_database(startup(database, database,
            b'DateStyle\0ISO, MDY\0TimeZone\0UTC\0client_encoding\0UTF8\0extra_float_digits\x003\0'), database))
        for payload in [startup('fixture', database), startup(database, 'fixture'),
                        startup(database, database, b'options\0-c role=fixture\0'),
                        startup(database, database, b'user\0fixture\0'), b'not a startup packet']:
            self.assertFalse(transport.startup_database(payload, database))

    @unittest.skipUnless(os.name == 'posix' and shutil.which('openssl'), 'POSIX ephemeral TLS certificate test')
    def test_real_tls_manager_allowlist_and_pg_database_fence(self):
        with tempfile.TemporaryDirectory() as directory, ExitStack() as stack:
            root = Path(directory)
            tls = harness.certificate(root)
            context = ssl.create_default_context(cafile=root / 'ca.pem')
            opener = build_opener(ProxyHandler({}), harness.NoRedirect(), HTTPSHandler(context=context))
            accepted = []
            class Upstream(BaseHTTPRequestHandler):
                def log_message(self, *args): pass
                def do_POST(self):
                    accepted.append(self.path)
                    content = b'{"accepted":true}'
                    self.send_response(200); self.send_header('Content-Length', str(len(content))); self.end_headers()
                    self.wfile.write(content)
            upstream = HTTPServer(('127.0.0.1', 0), Upstream)
            threading.Thread(target=upstream.serve_forever, daemon=True).start()
            self.addCleanup(upstream.server_close); self.addCleanup(upstream.shutdown)
            port = stack.enter_context(transport.serving(transport.proxy(tls, upstream.server_address[1], 'manager')))
            origin = 'https://localhost:' + str(port)
            result = harness.request(opener, origin, '/management/runner/v1/instance-access', body={})
            self.assertEqual(result, {'accepted': True})
            harness.request(opener, origin, '/management/runner/v1/claim', body={}, expected=404)
            self.assertEqual(accepted, ['/management/runner/v1/instance-access'])
            database = 'ci_' + uuid.uuid4().hex
            forwarded = []
            def connector():
                client, server = socket.socketpair()
                def read():
                    with server:
                        header = server.recv(4)
                        length = struct.unpack('!I', header)[0]
                        body = b''
                        while len(body) < length - 4: body += server.recv(length - 4 - len(body))
                        forwarded.append(header + body); server.sendall(b'bounded-fixture')
                threading.Thread(target=read, daemon=True).start()
                return client
            pg = stack.enter_context(transport.serving(transport.PostgreSQLProxy(tls, 5432, connector, database=database)))
            def startup(user):
                payload = struct.pack('!I', 196608) + b'user\0' + user.encode() + b'\0database\0' + database.encode() + b'\0\0'
                return struct.pack('!I', len(payload) + 4) + payload
            for user, expected in [(database, b'bounded-fixture'), ('fixture', b'')]:
                with socket.create_connection(('127.0.0.1', pg), timeout=5) as raw:
                    raw.sendall(struct.pack('!II', 8, 80877103)); self.assertEqual(raw.recv(1), b'S')
                    with context.wrap_socket(raw, server_hostname='localhost') as secure:
                        secure.settimeout(5); secure.sendall(startup(user)); self.assertEqual(secure.recv(64), expected)
            self.assertEqual(forwarded, [startup(database)])

    def archive(self, path, *, content=b'private candidate layer', expected=None):
        difference = 'sha256:' + hashlib.sha256(expected or content).hexdigest()
        config = json.dumps({'os': 'linux', 'architecture': 'amd64', 'rootfs': {'type': 'layers', 'diff_ids': [difference]}}).encode()
        image = 'sha256:' + hashlib.sha256(config).hexdigest()
        manifest = json.dumps([{'Config': 'config.json', 'Layers': ['layer.tar'], 'RepoTags': []}]).encode()
        with tarfile.open(path, 'w') as archive:
            for name, value in [('config.json', config), ('layer.tar', content), ('manifest.json', manifest)]:
                member = tarfile.TarInfo(name); member.size = len(value)
                archive.addfile(member, io.BytesIO(value))
        return image

    def test_registry_manifest_digest_is_distinct_from_actual_image_config_id(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            image = self.archive(root / 'image.tar')
            registry = transport.Registry(root / 'registry', 'cowork-hosted-vm-' + uuid.uuid4().hex)
            repository, digest = registry.import_archive(root / 'image.tar', image, 'server')
            manifest = registry.manifests[(repository, digest)]
            self.assertEqual(digest, 'sha256:' + hashlib.sha256(manifest).hexdigest())
            self.assertNotEqual(digest, image)
            self.assertEqual(json.loads(manifest)['config']['digest'], image)
            self.assertEqual(transport.hash_file(registry.blobs[image]), image.split(':')[1])

    def test_registry_refuses_image_identity_and_layer_tampering(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            image = self.archive(root / 'image.tar', content=b'tampered', expected=b'original')
            registry = transport.Registry(root / 'registry', 'cowork-hosted-vm-' + uuid.uuid4().hex)
            with self.assertRaisesRegex(RuntimeError, 'identity_mismatch'):
                registry.import_archive(root / 'image.tar', 'sha256:' + 'a' * 64, 'server')
            with self.assertRaisesRegex(RuntimeError, 'layer_identity_mismatch'):
                registry.import_archive(root / 'image.tar', image, 'server')
            self.assertFalse(registry.manifests)

    def test_original_oci_index_compressed_blobs_and_config_identity_are_preserved(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            content = b'original uncompressed layer'
            layer = gzip.compress(content, mtime=0)
            config = json.dumps({'os': 'linux', 'architecture': 'amd64', 'rootfs': {'type': 'layers',
                                'diff_ids': ['sha256:' + hashlib.sha256(content).hexdigest()]}}).encode()
            blobs = {}
            def descriptor(value, media):
                digest = 'sha256:' + hashlib.sha256(value).hexdigest(); blobs[digest] = value
                return {'mediaType': media, 'digest': digest, 'size': len(value)}
            config_desc = descriptor(config, 'application/vnd.oci.image.config.v1+json')
            layer_desc = descriptor(layer, 'application/vnd.oci.image.layer.v1.tar+gzip')
            manifest = json.dumps({'schemaVersion': 2, 'mediaType': 'application/vnd.oci.image.manifest.v1+json',
                                  'config': config_desc, 'layers': [layer_desc]}).encode()
            manifest_desc = descriptor(manifest, 'application/vnd.oci.image.manifest.v1+json')
            manifest_desc['platform'] = {'os': 'linux', 'architecture': 'amd64'}
            index = json.dumps({'schemaVersion': 2, 'mediaType': 'application/vnd.oci.image.index.v1+json',
                               'manifests': [manifest_desc]}).encode()
            index_desc = descriptor(index, 'application/vnd.oci.image.index.v1+json')
            image = index_desc['digest']
            archive_path = root / 'oci.tar'
            members = {'index.json': json.dumps({'schemaVersion': 2, 'manifests': [index_desc]}).encode(),
                       'manifest.json': b'[]', **{'blobs/sha256/' + key.split(':')[1]: value for key, value in blobs.items()}}
            with tarfile.open(archive_path, 'w') as archive:
                for name, value in members.items():
                    member = tarfile.TarInfo(name); member.size = len(value)
                    archive.addfile(member, io.BytesIO(value))
            registry = transport.Registry(root / 'registry', 'cowork-hosted-vm-' + uuid.uuid4().hex)
            repository, digest = registry.import_archive(archive_path, image, 'server')
            self.assertEqual(digest, image)
            self.assertEqual(registry.manifests[(repository, digest)], index)
            self.assertEqual(registry.blobs[layer_desc['digest']].read_bytes(), layer)
            self.assertEqual(registry.packages['server']['config_digest'], config_desc['digest'])
            self.assertNotEqual(image, config_desc['digest'])
            self.assertTrue(registry.packages['server']['manifest_bytes_preserved'])

    def test_parent_cleanup_refuses_baseline_and_unlabelled_containers(self):
        parent = harness.ParentDocker.__new__(harness.ParentDocker)
        parent.scope = 'cowork-hosted-vm-' + uuid.uuid4().hex
        parent.baseline = {'original': 'sha256:' + 'a' * 64}
        parent.containers = {'original', 'fixture'}
        parent.run = Mock()
        parent.json = Mock(return_value=[{'Name': '/foreign', 'Config': {'Labels': {}}}])
        for identifier in ['original', 'fixture', 'unknown']:
            with self.subTest(identifier=identifier), self.assertRaises(RuntimeError):
                parent.remove_container(identifier)
        parent.run.assert_not_called()

    def test_image_argument_rejects_mutable_tags_and_repository_manifest_references(self):
        self.assertEqual(harness.image_id('sha256:' + 'a' * 64), 'sha256:' + 'a' * 64)
        for value in ['candidate:latest', 'repository@sha256:' + 'a' * 64, 'sha256:' + 'A' * 64]:
            with self.assertRaises(Exception):
                harness.image_id(value)

    def test_vm_process_limit_wrapper_changes_only_new_qemu_child_argv(self):
        vm = Mock()
        original = vm.call
        harness.bound_vm_processes(vm)
        vm.call(['qemu-img', 'create', 'own-disk'], timeout=10)
        original.assert_called_with(['qemu-img', 'create', 'own-disk'], timeout=10)
        # The independent owned-child test executes a real Python child to prove
        # cwd, pipe bounds and reaping. This check proves controller delegation.
        with patch('hosted_qemu_process.QemuChildren.launch', return_value=b'') as launch:
            requested = ['qemu-system-x86_64', '-m', '4096']
            self.assertEqual(vm.call(requested, timeout=10), b'')
            launch.assert_called_once_with(requested, timeout=10)

    @unittest.skipUnless(os.name == 'posix', 'POSIX root ownership')
    def test_vm_process_wrapper_rejects_foreign_socket_before_starting_child(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); root.chmod(0o700)
            (root / 'vm.json').write_text('{}'); (root / 'vm.json').chmod(0o600)
            vm = Mock(); harness.bound_vm_processes(vm)
            argv = ['qemu-system-x86_64', '-pidfile', str(root / 'qemu.pid'),
                    '-qmp', 'unix:' + str(root.parent / 'foreign.sock') + ',server=on,wait=off']
            with patch('hosted_qemu_process.subprocess.Popen') as invoked:
                with self.assertRaisesRegex(RuntimeError, 'foreign_qemu_socket_refused'):
                    vm.call(argv)
                invoked.assert_not_called()


class HostedPreparationTests(unittest.TestCase):
    """In-memory HTTP/Docker observations; no listener or subprocess is started."""
    @staticmethod
    def response(value, status=200):
        response = Mock(status=status)
        response.read.return_value = json.dumps(value).encode()
        response.__enter__ = Mock(return_value=response)
        response.__exit__ = Mock(return_value=False)
        return response

    def tokens(self, access_expiry=900):
        return {'access_token': 'old-access-token-private', 'refresh_token': 'old-refresh-token-private',
                'access_expires_at': datetime.fromtimestamp(access_expiry, timezone.utc).isoformat(),
                'refresh_expires_at': datetime.fromtimestamp(9000, timezone.utc).isoformat(),
                'user_id': str(uuid.uuid4()), 'session_id': str(uuid.uuid4())}

    def test_regular_refresh_rotates_same_session_before_post_without_bootstrap_retry(self):
        tokens = self.tokens()
        fresh = {**tokens, 'access_token': 'new-access-token-private', 'refresh_token': 'new-refresh-token-private',
                 'access_expires_at': datetime.fromtimestamp(4000, timezone.utc).isoformat()}
        opener = Mock()
        opener.open.side_effect = [self.response(fresh), self.response({'id': 'accepted'}), self.response({})]
        diagnostics = harness.PublicDiagnostics()
        session = harness.AdminSession(opener, 'https://manager.invalid', tokens, diagnostics, clock=lambda: 1000)
        harness.request(opener, session.origin, '/instances', session, {'name': 'private'},
                        phase_name='instance_create', diagnostics=diagnostics)
        harness.request(opener, session.origin, '/policy', session)
        requests = [call.args[0] for call in opener.open.call_args_list]
        self.assertEqual([request.full_url for request in requests],
                         ['https://manager.invalid/api/v1/auth/refresh', 'https://manager.invalid/instances',
                          'https://manager.invalid/policy'])
        self.assertEqual(json.loads(requests[0].data), {'refresh_token': tokens['refresh_token']})
        self.assertIsNone(requests[0].get_header('Authorization'))
        self.assertEqual(requests[1].get_header('Authorization'), 'Bearer ' + fresh['access_token'])
        self.assertEqual(session.tokens['session_id'], tokens['session_id'])
        self.assertEqual(tokens['refresh_token'], 'old-refresh-token-private')
        self.assertNotIn('private', json.dumps(diagnostics.snapshot()))

    def test_refused_refresh_never_submits_or_retries_mutating_request(self):
        opener = Mock()
        opener.open.side_effect = HTTPError('https://private.invalid/refresh?key=secret', 401, 'private', {},
            io.BytesIO(b'{"error":"unauthorized","message":"private refresh token"}'))
        diagnostics = harness.PublicDiagnostics()
        session = harness.AdminSession(opener, 'https://manager.invalid', self.tokens(), diagnostics, clock=lambda: 1000)
        with self.assertRaises(harness.HttpQualificationFailure) as caught:
            harness.request(opener, session.origin, '/instances', session, {'name': 'private'})
        self.assertEqual(opener.open.call_count, 1)
        self.assertEqual(caught.exception.observation,
                         {'phase': 'auth_refresh', 'expected_status': 200, 'actual_status': 401,
                          'api_error_code': 'unauthorized'})
        self.assertEqual(str(caught.exception), 'http_status_unexpected')

    def test_refresh_cannot_change_user_session_or_reuse_consumed_refresh_token(self):
        for changed in ['user_id', 'session_id', 'refresh_token', 'refresh_expires_at', 'access_expires_at']:
            with self.subTest(changed=changed):
                tokens = self.tokens()
                fresh = {**tokens, 'access_token': 'new-access-token-private', 'refresh_token': 'new-refresh-token-private',
                         'access_expires_at': datetime.fromtimestamp(4000, timezone.utc).isoformat()}
                fresh[changed] = str(uuid.uuid4()) if changed.endswith('_id') else tokens[changed] \
                    if changed in ['refresh_token', 'access_expires_at'] else '2099-01-01T00:00:00Z'
                opener = Mock(); opener.open.return_value = self.response(fresh)
                session = harness.AdminSession(opener, 'https://manager.invalid', tokens,
                    harness.PublicDiagnostics(), clock=lambda: 1000)
                with self.assertRaisesRegex(RuntimeError, 'fixture_admin_refresh_scope_changed'):
                    harness.request(opener, session.origin, '/instances', session, {})
                self.assertEqual(opener.open.call_count, 1)

    def test_unexpected_post_401_has_one_attempt_and_no_auth_recovery(self):
        opener = Mock()
        opener.open.side_effect = HTTPError('https://manager.invalid/instances', 401, 'private', {},
                                          io.BytesIO(b'{"error":"unauthorized"}'))
        session = harness.AdminSession(opener, 'https://manager.invalid', self.tokens(4000),
            harness.PublicDiagnostics(), clock=lambda: 1000)
        with self.assertRaises(harness.HttpQualificationFailure):
            harness.request(opener, session.origin, '/instances', session, {})
        self.assertEqual(opener.open.call_count, 1)

    def test_http_status_is_numeric_and_error_body_is_strictly_allowlisted(self):
        payload = {'error': 'unavailable', 'message': 'host_capacity_unqualified_or_stale',
                   'details': {'credential': 'private'}, 'path': '/private/secret'}
        opener = Mock()
        opener.open.side_effect = HTTPError('https://private.invalid/?token=private', 503, 'private', {},
                                          io.BytesIO(json.dumps(payload).encode()))
        with self.assertRaises(harness.HttpQualificationFailure) as caught:
            harness.request(opener, 'https://manager.invalid', '/private/path', 'private-token', {},
                            expected=201, phase_name='instance_create')
        self.assertEqual(caught.exception.observation,
                         {'phase': 'instance_create', 'expected_status': 201, 'actual_status': 503,
                          'api_error_code': 'unavailable', 'host_error_code': 'host_capacity_unqualified_or_stale'})
        self.assertNotIn('private', json.dumps(caught.exception.observation))
        self.assertEqual(harness.failure_code(caught.exception), 'http_status_unexpected')
        for value in ['private-token', '/private/path', 'host_capacity_unqualified_or_stale private',
                      ['unavailable'], {'error': 'unavailable'}]:
            observation = harness.http_observation('instance_create', 200, 503,
                                                  json.dumps({'error': value, 'message': value}).encode())
            self.assertEqual(set(observation), {'phase', 'expected_status', 'actual_status'})

    def test_expected_negative_response_is_not_failure_and_invalid_phase_never_opens(self):
        opener = Mock(); opener.open.side_effect = HTTPError('https://private.invalid', 403, 'private', {}, io.BytesIO(b'{}'))
        diagnostics = harness.PublicDiagnostics()
        self.assertIsNone(harness.request(opener, 'https://manager.invalid', '/claim', expected=403,
            phase_name='operator_auth_fences', diagnostics=diagnostics))
        self.assertNotIn('last_unexpected_http_status', diagnostics.snapshot())
        opener.open.reset_mock()
        with self.assertRaisesRegex(RuntimeError, 'invalid_qualification_phase'):
            harness.request(opener, 'https://manager.invalid', '/instances', phase_name='/secret/path')
        opener.open.assert_not_called()

    def test_host_and_credential_ids_are_persisted_separately_before_later_failure(self):
        binding = {'version': 1, 'host_id': str(uuid.uuid4()), 'trust_domain': str(uuid.uuid4()),
                   'daemon_id': 'a' * 64, 'operator_state_id': str(uuid.uuid4())}
        registered = {'id': binding['host_id'], 'trust_domain': binding['trust_domain'], 'revision': 1}
        credential = {'id': str(uuid.uuid4()), 'host_id': binding['host_id'], 'token': 'private-credential-token'}
        report = {'phase': 'host_register', 'passed': False}; writes = []
        with patch.object(harness, 'durable_report', side_effect=lambda path, value: writes.append(copy.deepcopy(value))):
            harness.accept_host(report, Path('unused'), registered, binding)
            self.assertNotIn('host_credential_id', writes[0])
            harness.accept_credential(report, Path('unused'), credential, binding)
            self.assertEqual(writes[0]['host_binding'], binding)
            self.assertEqual(writes[1]['host_credential_id'], credential['id'])
            self.assertNotIn('private-credential-token', json.dumps(writes))
            for changed in [{'host_id': str(uuid.uuid4())}, {'id': 'not-an-id'}]:
                with self.assertRaisesRegex(RuntimeError, 'host_credential_scope_mismatch'):
                    harness.accept_credential(report, Path('unused'), {**credential, **changed}, binding)
            self.assertEqual(len(writes), 2)

    def test_host_attestor_http_failures_preserve_operator_exception_but_report_only_codes(self):
        original = Mock()
        error = HTTPError('https://private.invalid/secret', 503, 'private', {},
                          io.BytesIO(b'{"error":"unavailable","message":"host_disabled","token":"private"}'))
        original.open.side_effect = error
        diagnostics = harness.PublicDiagnostics()
        observed = harness.ObservedManagerHttp(original, diagnostics)
        with self.assertRaises(HTTPError) as caught:
            observed.open(Request('https://manager.invalid/management/runner/v1/host-capacity/challenge', b'{}'), timeout=30)
        self.assertIs(caught.exception, error)
        self.assertEqual(diagnostics.snapshot()['last_host_http_failure'],
                         {'phase': 'host_capacity_challenge', 'expected_status': 200, 'actual_status': 503,
                          'api_error_code': 'unavailable', 'host_error_code': 'host_disabled'})
        self.assertNotIn('private', json.dumps(diagnostics.snapshot()))

    def capacity_fixture(self, root, *, unowned=False):
        import host_capacity_attestation as capacity
        import executor_host
        import provisioner as p
        import resource_budget
        org = str(uuid.uuid4())
        config = {'backend': 'compose', 'state_directory': str(root),
            'management_url': 'https://manager.invalid', 'management_ca_file': '/private/ca',
            'provisioner_token_file': '/private/host-token', 'execution_admission': {'version': 1},
            'fleet_host_budget': {'cpu_millis': 4000, 'memory_mib': 3584,
                                  'reserved_cpu_millis': 1000, 'reserved_memory_mib': 1024},
            'instance_service_limits': resource_budget.service_caps({}),
            'host_capacity_attestation': {'version': 1, 'host_revision': 1},
            'executor_host': {'id': str(uuid.uuid4()), 'trust_domain': org, 'operator_state_id': str(uuid.uuid4()),
                'endpoint': 'tcp://localhost:2376', 'daemon_id': 'a' * 64, 'ca_file': '/private/ca',
                'cert_file': '/private/cert', 'key_file': '/private/key'}}
        info = {'ID': 'a' * 64, 'NCPU': 4, 'MemTotal': 3900 * 1024**2,
                'Labels': ['cowork.role=executor', 'cowork.trust-domain=' + org]}
        containers = [{'Id': 'b' * 64, 'Name': '/unowned'}] if unowned else []
        def docker(arguments, **keywords):
            if arguments[1] == 'info': return json.dumps(info).encode()
            if arguments[1] == 'ps': return ('b' * 64 + '\n').encode() if containers else b''
            if arguments[1] == 'inspect': return json.dumps(containers).encode()
            raise AssertionError('unexpected in-memory Docker observation')
        infrastructures = []
        def infrastructure(value):
            infra = SimpleNamespace(config=value, root=root, host=Mock(), private_write=p.private_write,
                                    run=Mock(side_effect=docker))
            infrastructures.append(infra)
            return infra
        observations, challenges = [], []
        manager = Mock(http=Mock())
        def call(path, payload):
            if path == '/host-capacity/challenge':
                selected = {'version': 1, 'challenge_id': str(uuid.uuid4()), 'expires_at': '2099-01-01T00:00:00Z',
                    'executor_host_binding': executor_host.manager_binding(config), 'host_revision': 1,
                    'allocation_revision': 1, 'capacity': config['fleet_host_budget'],
                    'service_plan': config['instance_service_limits'], 'allocations': []}
                challenges.append(selected['challenge_id'])
                return selected
            self.assertEqual(path, '/host-capacity/attest')
            observations.append(copy.deepcopy(payload))
            qualified = payload['inventory_complete'] and not payload['unknown_workloads']
            return {'qualified': qualified, 'reason': None if qualified else 'host_allocation_unknown'}
        manager.call.side_effect = call
        attestors = []
        def attestor_factory(*arguments):
            instance = capacity.CapacityAttestor(*arguments)
            attestors.append(instance)
            return instance
        package = SimpleNamespace(Infrastructure=Mock(side_effect=infrastructure), Manager=Mock(return_value=manager),
            resource_budget=resource_budget, host_capacity_attestation=SimpleNamespace(
                CapacityAttestor=Mock(side_effect=attestor_factory), settings=capacity.settings))
        return package, config, manager, infrastructures, attestors, observations, challenges

    def test_real_attestor_uses_fresh_challenge_complete_info_inventory_and_one_keepalive(self):
        import host_capacity_attestation as capacity
        with tempfile.TemporaryDirectory() as temporary, ExitStack() as stack, \
                patch.object(capacity.threading, 'Thread') as thread:
            thread.return_value.is_alive.return_value = False
            root = Path(temporary)
            package, config, manager, infrastructures, attestors, observations, challenges = self.capacity_fixture(root)
            initial, observed_manager, attestor = harness.start_capacity_observation(
                package, config, stack, harness.PublicDiagnostics())
            self.assertIs(observed_manager, manager)
            self.assertTrue(attestor.qualified)
            self.assertEqual(observations[0]['observed_capacity'], {'cpu_millis': 4000, 'memory_mib': 3900})
            self.assertEqual(observations[0]['allocations'], [])
            self.assertTrue(observations[0]['inventory_complete'])
            self.assertFalse(observations[0]['unknown_workloads'])
            self.assertNotIn('instance_id', config)
            self.assertEqual([call.args[0] for call in initial.run.call_args_list],
                             [['docker', 'info', '--format', '{{json .}}'], ['docker', 'ps', '--all', '--quiet', '--no-trunc'],
                              ['docker', 'ps', '--all', '--quiet', '--no-trunc']])
            # Exercise the real renewal closure without starting any thread or
            # sleeping: preparation must collect a new challenge/inventory too.
            renew = thread.call_args.kwargs['target']
            with patch.object(attestor.stop_event, 'wait', side_effect=[False, True]):
                renew()
            self.assertEqual(len(challenges), 2)
            self.assertEqual(initial.run.call_count, 6)
            full = {**config, 'images': {'server': 'private-runtime'},
                    'guest_runtime_endpoints': {'instance_id': str(uuid.uuid4())}}
            completed, same_manager = harness.complete_capacity_context(package, full, attestor)
            self.assertIs(same_manager, manager)
            self.assertIs(attestor.infra, completed)
            self.assertIs(completed.capacity_attestor, attestor)
            self.assertIs(manager.capacity_attestor, attestor)
            attestor.refresh()
            self.assertEqual(len(challenges), 3)
            self.assertEqual(len(set(challenges)), 3)
            self.assertEqual(completed.run.call_count, 3)
            package.Manager.assert_called_once()
            package.host_capacity_attestation.CapacityAttestor.assert_called_once()
            thread.assert_called_once(); thread.return_value.start.assert_called_once()
            stack.close()
            self.assertTrue(attestor.stop_event.is_set())
            thread.return_value.join.assert_called_once_with(timeout=55)

    def test_unknown_full_inventory_cannot_qualify_and_partial_start_is_closed(self):
        import host_capacity_attestation as capacity
        with tempfile.TemporaryDirectory() as temporary, ExitStack() as stack, \
                patch.object(capacity.threading, 'Thread') as thread:
            thread.return_value.is_alive.return_value = False
            package, config, manager, infrastructures, attestors, observations, challenges = self.capacity_fixture(
                Path(temporary), unowned=True)
            with self.assertRaisesRegex(RuntimeError, 'initial_empty_host_capacity_not_qualified'):
                harness.start_capacity_observation(package, config, stack, harness.PublicDiagnostics())
            self.assertFalse(attestors[0].qualified)
            self.assertTrue(observations[0]['unknown_workloads'])
            self.assertIn(['docker', 'inspect', 'b' * 64], [call.args[0] for call in infrastructures[0].run.call_args_list])
            stack.close()
            self.assertTrue(attestors[0].stop_event.is_set())
            thread.return_value.join.assert_called_once_with(timeout=55)

    def test_start_refresh_error_also_has_registered_close_before_start(self):
        with tempfile.TemporaryDirectory() as temporary, ExitStack() as stack:
            package, config, manager, infrastructures, attestors, observations, challenges = self.capacity_fixture(Path(temporary))
            manager.call.side_effect = RuntimeError('private challenge detail')
            with self.assertRaisesRegex(Exception, 'host_capacity_attestation_failed'):
                harness.start_capacity_observation(package, config, stack, harness.PublicDiagnostics())
            self.assertIsNone(attestors[0].thread)
            stack.close()
            self.assertTrue(attestors[0].stop_event.is_set())

    def test_context_transition_cannot_change_host_authority_or_start_second_attestor(self):
        import host_capacity_attestation as capacity
        with tempfile.TemporaryDirectory() as temporary, ExitStack() as stack, \
                patch.object(capacity.threading, 'Thread') as thread:
            thread.return_value.is_alive.return_value = False
            package, config, manager, infrastructures, attestors, observations, challenges = self.capacity_fixture(Path(temporary))
            _, _, attestor = harness.start_capacity_observation(package, config, stack, harness.PublicDiagnostics())
            for key, changed in [('executor_host', {**config['executor_host'], 'id': str(uuid.uuid4())}),
                                 ('fleet_host_budget', {**config['fleet_host_budget'], 'memory_mib': 3600}),
                                 ('state_directory', str(Path(temporary) / 'foreign')),
                                 ('management_url', 'https://foreign.invalid'),
                                 ('provisioner_token_file', '/foreign/credential'),
                                 ('host_capacity_attestation', {'version': 1, 'host_revision': 2}),
                                 ('instance_service_limits', {**config['instance_service_limits'],
                                     'api': {**config['instance_service_limits']['api'], 'cpu_millis': 1100}})]:
                with self.subTest(key=key), self.assertRaisesRegex(RuntimeError, 'completed_host_context_scope_changed'):
                    harness.complete_capacity_context(package, {**config, key: changed}, attestor)
            package.Infrastructure.assert_called_once()
            thread.assert_called_once()
            self.assertIs(attestor.infra, infrastructures[0])

    def test_initial_host_context_never_invents_instance_identity(self):
        package = Mock()
        for key in ['instance_id', 'guest_runtime_endpoints', 'remote_lifecycle_qualification']:
            with ExitStack() as stack, self.subTest(key=key), \
                    self.assertRaisesRegex(RuntimeError, 'initial_host_context_contains_instance'):
                harness.start_capacity_observation(package, {key: str(uuid.uuid4())}, stack, harness.PublicDiagnostics())
        package.Infrastructure.assert_not_called(); package.Manager.assert_not_called()

    def test_signal_unwinds_cleanup_and_restores_handlers_without_delivering_real_signal(self):
        handlers = {}; original = object()
        with patch.object(harness.signal, 'getsignal', return_value=original), \
                patch.object(harness.signal, 'signal', side_effect=lambda number, handler: handlers.update({number: handler})):
            closed = Mock()
            with self.assertRaisesRegex(RuntimeError, 'qualification_interrupted'):
                with harness.QualificationSignals(), ExitStack() as stack:
                    stack.callback(closed)
                    handlers[harness.signal.SIGTERM](harness.signal.SIGTERM, None)
            closed.assert_called_once()
            self.assertIs(handlers[harness.signal.SIGTERM], original)
            self.assertIs(handlers[harness.signal.SIGINT], original)


if __name__ == '__main__':
    unittest.main()
