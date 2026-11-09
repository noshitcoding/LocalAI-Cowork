"""Actual age/files/loopback object reads; Engine and DB adapters are labelled fixtures."""
import copy
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import threading
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch
from urllib.parse import quote, unquote
from urllib.request import urlopen
import uuid

import execution_generation as epochs
import executor_host
from instance_backups import BackupError, InstanceBackups, digest
import instance_imports
import instance_import_target as target
import provisioner
import remote_volume_streaming as streaming
import test_verified_backups as backup_fixtures


@unittest.skipUnless(os.name == 'posix' and shutil.which('age') and shutil.which('age-keygen'), 'actual age and POSIX file/pipe fixtures')
class TargetReadbackTests(unittest.TestCase):
    def setUp(self):
        fixture = backup_fixtures.ActualBackupVerificationTests()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        self.archive = fixture
        self.root = fixture.root
        self.images = {key: 'registry.invalid/' + key + '@sha256:' + 'a' * 64 for key in
                       ['server', 'runner', 'sandbox_core', 'sandbox_gui', 'sandbox_crew', 'egress_proxy']}
        fixture.images = self.images
        fixture.manifest['images'] = self.images
        fixture.files['secrets/storage_master_key'] = b'original-data-key-' + uuid.uuid4().hex.encode()
        for name in ['bootstrap_token', 'runner_signing_key']:
            fixture.files['secrets/' + name] = ('original-' + name + '-' + uuid.uuid4().hex).encode()
        self.run_id = str(uuid.UUID(fixture.workspace))
        fixture.manifest['import_provenance'] = {'version': 1, 'origin_epoch': fixture.source['execution_epoch'], 'run_ids': [self.run_id]}
        self.catalog = fixture.encrypt()
        self.operation = fixture.import_operation(self.catalog)
        host = {'id': self.operation['instance']['executor_host_id'], 'trust_domain': fixture.source['organisation_id'],
                'operator_state_id': str(uuid.uuid4()), 'endpoint': 'tcp://localhost:22376', 'daemon_id': 'f' * 64,
                'ca_file': str(self.root / 'fixture-ca'), 'cert_file': str(self.root / 'fixture-cert'), 'key_file': str(self.root / 'fixture-key')}
        self.config = {'backend': 'compose', 'executor_host': host, 'images': self.images,
            'fleet_host_budget': {'cpu_millis': 8000, 'memory_mib': 8192, 'reserved_cpu_millis': 1000, 'reserved_memory_mib': 1024},
            's3_endpoint': 'http://storage:9000', 'services_network': 'services', 'routing_network': 'routing',
            's3_admin_alias': 'owned-fixture', 'fleet_management_url': 'http://management:8080',
            'execution_admission': {'server_images': [self.images['server']]}}
        frozen = self.operation['arguments']['import_binding']
        frozen['target_host_binding'] = executor_host.manager_binding(self.config)
        self.operation['executor_host_binding'] = frozen['target_host_binding']
        self.operation['instance']['policy']['limits'] = {'cpu_millis': 500, 'memory_mib': 512, 'storage_gib': 1, 'concurrent_runs': 1}
        self.operation['arguments']['import_binding_sha256'] = instance_imports.canonical(frozen)
        self.operation['execution_binding'] = {'operation_id': self.operation['id'], 'instance_id': self.operation['instance_id'],
            'organisation_id': self.operation['organisation_id'], 'source_epoch': frozen['target_epoch'], 'candidate_epoch': frozen['candidate_epoch']}
        self.state = self.root / 'state'; self.state.mkdir()
        self.directory = self.state / uuid.UUID(self.operation['instance_id']).hex; self.directory.mkdir()
        self.generation = self.directory / 'generations' / uuid.UUID(self.operation['id']).hex
        self.generation.mkdir(parents=True)
        provisioner.private_write(self.generation / 'generation.json', json.dumps({'generation': self.operation['id'], 'launch_not_before': 1700000000}).encode())
        self.manager = fixture.manager()
        self.manager.config = self.config
        self.calls = []
        self.volumes = self.root / 'fixture-volumes'; self.volumes.mkdir()
        workspace = self.volumes / fixture.workspace; (workspace / 'sub').mkdir(parents=True)
        (workspace / 'sub').chmod(0o750)
        (workspace / 'sub/marker.txt').write_bytes(fixture.marker); (workspace / 'sub/marker.txt').chmod(0o640)
        self.objects = self.root / 'fixture-objects'; self.objects.mkdir()
        (self.objects / 'result.txt').write_bytes(fixture.files['objects/result.txt'])
        class ObjectHandler(BaseHTTPRequestHandler):
            def do_GET(handler):
                path = self.objects / unquote(handler.path.removeprefix('/'))
                if not path.is_file() or not path.resolve().is_relative_to(self.objects):
                    handler.send_error(404); return
                body = path.read_bytes()
                handler.send_response(200); handler.end_headers(); handler.wfile.write(body)
            def log_message(*args): pass
        self.server = ThreadingHTTPServer(('127.0.0.1', 0), ObjectHandler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True); self.thread.start()
        def close_server():
            self.server.shutdown(); self.server.server_close(); self.thread.join(timeout=5)
            self.assertFalse(self.thread.is_alive())
        self.addCleanup(close_server)
        self.infra = SimpleNamespace(root=self.state, config=self.config, host=Mock(), private_write=provisioner.private_write,
            resource_names=provisioner.resource_names, network_plan=lambda instance, directory: {},
            active_directory=self.active_directory, ensure_database=Mock(), ensure_bucket=Mock())
        self.manager.infra = self.infra
        self.manager.quiescent = Mock()  # External Engine quiescence is a fixture, never physical acceptance.
        self.manager.require_remote_operation = Mock()
        self.infra.ensure_secrets = self.write_secrets
        self.infra.write_compose = lambda instance, directory: self.write(directory / 'compose.json',
            json.dumps(target.candidate_compose(self.manager, self.operation, directory, self.active_directory(directory))).encode())
        self.manager.write = self.write
        for name in ['bootstrap_token', 'runner_signing_key', 'database_password', 's3_access_key', 's3_secret_key', 'fleet_key']:
            (self.directory / 'secrets').mkdir(exist_ok=True)
            (self.directory / 'secrets' / name).write_bytes(('previous-target-' + name).encode())
        self.write_secrets(self.operation['instance'], self.generation)
        self.write(self.generation / 'secrets/storage_master_key', fixture.files['secrets/storage_master_key'])
        self.write(self.generation / 'runtime-secrets/storage_master_key', fixture.files['secrets/storage_master_key'])
        epochs.stage(self.infra, self.operation, self.generation)
        self.write(self.generation / 'generation.json', json.dumps({'generation': self.operation['id'], 'launch_not_before': 1700000000}).encode())
        self.write(self.generation / 'images.json', json.dumps(self.images).encode())
        self.write(self.generation / 'restore.json', json.dumps({'backup_id': self.catalog['id'], 'previous_generation': None,
            'import_binding_sha256': instance_imports.canonical(frozen), 'original_manifest_sha256': instance_imports.canonical(fixture.manifest),
            'origin_epoch': fixture.source['execution_epoch'], 'restore_operation_id': self.operation['id']}).encode())
        self.history = {'runs': [{'id': self.run_id, 'state': 'interrupted', 'lease_owner': None, 'lease_token': None, 'lease_expires_at': None}],
            'origins': [{'run_id': self.run_id, 'import_id': frozen['import_id'], 'organisation_id': frozen['organisation_id'],
                'source_instance_id': frozen['source']['id'], 'target_instance_id': frozen['target']['id'], 'source_backup_id': self.catalog['id'],
                'source_manifest_sha256': instance_imports.canonical(fixture.manifest)}],
            'active_schedules': 0, 'active_sessions': 0, 'active_credentials': 0}
        self.manager.run = self.command
        self.infra.run = self.command
        self.manager.storage = self.storage
        self.manager.read_object = self.read_object
        self.manager.capture_import_volume = self.capture_volume

    def write(self, path, body):
        self.calls.append(('write', path.name))
        provisioner.private_write(path, body)

    def active_directory(self, directory):
        pointer = directory / 'active.json'
        return directory / 'generations' / uuid.UUID(json.loads(pointer.read_text())['generation']).hex if pointer.exists() else directory

    def write_secrets(self, instance, generation):
        (generation / 'secrets').mkdir(exist_ok=True); (generation / 'runtime-secrets').mkdir(exist_ok=True)
        values = {name: 'fresh-' + name + '-' + uuid.uuid4().hex for name in
                  ['bootstrap_token', 'runner_signing_key', 'database_password', 's3_access_key', 's3_secret_key', 'fleet_key']}
        resources = provisioner.resource_names(instance, generation)
        values['database_url'] = 'postgres://' + resources['database'] + ':' + values['database_password'] + '@localhost:25432/' + resources['database']
        # Preserve the supplied data key, just like production ensure_secrets.
        values['storage_master_key'] = ((generation / 'secrets/storage_master_key').read_text()
            if (generation / 'secrets/storage_master_key').exists() else self.archive.files['secrets/storage_master_key'].decode())
        for name, value in values.items():
            self.write(generation / 'secrets' / name, value.encode())
            if name != 'database_password': self.write(generation / 'runtime-secrets' / name, value.encode())
        return values

    def command(self, arguments, **options):
        self.calls.append(('command', arguments[0]))
        if arguments[0] == 'age': return self.archive.command(arguments, **options)
        if arguments[:3] == ['docker', 'volume', 'ls']:
            prefix = provisioner.resource_names(self.operation['instance'], self.generation)['volume_prefix']
            return ''.join(prefix + path.name + '\n' for path in sorted(self.volumes.iterdir())).encode()
        if arguments[0] == 'psql':
            if 'data' in options: return b''  # Actual fence SQL has a separate private PG test.
            if 'json_build_object(\'runs\'' in arguments[-1] and "'origins',(SELECT count" in arguments[-1]:
                return json.dumps({'runs': 1, 'origins': 1, 'active_runs': 0, 'schedules': 0, 'sessions': 0, 'credentials': 0}).encode()
            return json.dumps(self.history).encode()
        if arguments[0] == 'pg_restore': return b''  # File fixture, not physical restore.
        raise AssertionError('unexpected fixture command ' + arguments[0])

    def storage(self, *arguments):
        if arguments[0] == 'mirror':
            self.calls.append(('restore-object-transport', 'external-Engine-fixture'))
            return b''
        self.calls.append(('object-list', 'actual-fixture-files'))
        self.assertEqual(arguments[:3], ('ls', '--recursive', '--json'))
        return b'\n'.join(json.dumps({'status': 'success', 'type': 'file', 'key': path.relative_to(self.objects).as_posix(),
            'size': path.stat().st_size}).encode() for path in sorted(self.objects.rglob('*')) if path.is_file())

    def read_object(self, name, destination, bound):
        self.calls.append(('object-read', 'actual-loopback-HTTP'))
        key = name.split('/', 2)[2]
        with urlopen('http://127.0.0.1:' + str(self.server.server_port) + '/' + quote(key), timeout=5) as response:
            content = response.read(bound + 1)
        self.assertLessEqual(len(content), bound)
        destination.write_bytes(content)
        return {'bytes': len(content), 'sha256': digest(destination)}

    def capture_volume(self, operation, generation, run_id, destination):
        self.calls.append(('workspace-read', 'actual-owned-file-helper'))
        # Execute the actual fixed capture helper against an owned directory.
        process = subprocess.Popen([sys.executable, '-c', streaming.HELPER_SOURCE, 'capture', str(self.volumes / run_id),
            str(os.geteuid()), str(os.getegid()), '1048576', '524288', '100'], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        try:
            with destination.open('xb') as output:
                return streaming.pump(process, output, 'capture', {'max_archive_bytes': 1048576, 'timeout_seconds': 10})
        finally:
            if process.poll() is None: process.kill()
            process.wait(timeout=5); process.stdout.close()

    def verify(self, **options):
        verified = self.archive.verify(self.catalog)
        scratch = self.root / uuid.uuid4().hex; scratch.mkdir(mode=0o700)
        return target.verify_target(self.manager, self.operation, self.directory, self.generation, verified, scratch,
                                    before_activation=options.pop('before_activation', True), **options)

    def test_actual_object_and_workspace_readback_binds_original_provenance(self):
        proof = self.verify()
        self.assertTrue(proof['workspace_inventory_complete'])
        self.assertTrue(proof['credentials_rotated'])
        self.assertEqual(proof['manifest_sha256'], instance_imports.canonical(self.archive.manifest))
        self.assertIn(('object-read', 'actual-loopback-HTTP'), self.calls)
        self.assertIn(('workspace-read', 'actual-owned-file-helper'), self.calls)
        self.assertFalse((self.directory / 'active.json').exists())
        self.assertFalse(any(path.name.startswith('receipt-') for path in self.directory.rglob('*')))

    def test_missing_extra_and_same_size_corrupt_objects_refuse_success(self):
        path = self.objects / 'result.txt'; original = path.read_bytes()
        for change in ['missing', 'extra', 'corrupt', 'size']:
            with self.subTest(change=change):
                path.write_bytes(original)
                (self.objects / 'extra').unlink(missing_ok=True)
                if change == 'missing': path.unlink()
                elif change == 'extra': (self.objects / 'extra').write_bytes(b'extra')
                elif change == 'corrupt': path.write_bytes(b'X' * len(original))
                else: path.write_bytes(original[:-1])
                with self.assertRaisesRegex(BackupError, 'object_'): self.verify()

    def test_missing_extra_and_changed_workspace_contents_refuse_success(self):
        workspace = self.volumes / self.archive.workspace
        marker = workspace / 'sub/marker.txt'; original = marker.read_bytes()
        marker.write_bytes(b'X' * len(original))
        with self.assertRaisesRegex(BackupError, 'workspace_content'): self.verify()
        marker.write_bytes(original)
        extra = self.volumes / uuid.uuid4().hex; extra.mkdir()
        with self.assertRaisesRegex(BackupError, 'workspace_inventory'): self.verify()
        extra.rmdir(); workspace.rename(self.volumes / ('renamed-' + workspace.name))
        with self.assertRaisesRegex(BackupError, 'workspace_inventory'): self.verify()

    def test_wrong_epoch_host_runtime_mount_or_credential_refuses_success(self):
        marker = self.generation / 'execution.json'; original = marker.read_bytes()
        changed = json.loads(original); changed['epoch'] = str(uuid.uuid4()); marker.write_text(json.dumps(changed))
        with self.assertRaisesRegex(BackupError, 'epoch'): self.verify()
        marker.write_bytes(original)
        for name in ['bootstrap_token', 'runner_signing_key', 'storage_master_key', 's3_access_key', 's3_secret_key', 'database_password']:
            path = self.generation / 'secrets' / name; before = path.read_bytes()
            copied = (b'changed-key' if name == 'storage_master_key' else self.archive.files['secrets/' + name]
                if name in ['bootstrap_token', 'runner_signing_key'] else (self.directory / 'secrets' / name).read_bytes())
            path.write_bytes(copied)
            runtime = self.generation / 'runtime-secrets' / name
            if runtime.exists(): runtime.write_bytes(copied)
            with self.subTest(secret=name), self.assertRaisesRegex(BackupError, 'credential|data_key'): self.verify()
            path.write_bytes(before)
            if runtime.exists(): runtime.write_bytes(before)
        changed = copy.deepcopy(self.config['executor_host']); changed['daemon_id'] = 'a' * 64
        self.config['executor_host'] = changed
        with self.assertRaisesRegex(BackupError, 'host_binding'): self.verify()

    def test_exact_history_and_origins_not_counts_are_required(self):
        original = copy.deepcopy(self.history)
        for key in ['source_instance_id', 'source_backup_id', 'import_id', 'organisation_id', 'target_instance_id', 'source_manifest_sha256']:
            self.history = copy.deepcopy(original); self.history['origins'][0][key] = str(uuid.uuid4())
            with self.subTest(key=key), self.assertRaisesRegex(BackupError, 'origin_provenance'): self.verify()
        self.history = copy.deepcopy(original); self.history['runs'][0]['id'] = str(uuid.uuid4())
        with self.assertRaisesRegex(BackupError, 'history_inventory'): self.verify()
        self.history = copy.deepcopy(original); self.history['runs'][0]['lease_token'] = str(uuid.uuid4())
        with self.assertRaisesRegex(BackupError, 'historical_run_active'): self.verify()

    def test_qualify_reopens_original_catalog_and_rechecks_changed_target(self):
        self.verify()
        self.write(self.directory / 'active.json', json.dumps({'generation': self.operation['id']}).encode())
        self.write(self.directory / 'compose.json', (self.generation / 'import-compose.json').read_bytes())
        self.operation['action'] = 'import_qualify'
        first = instance_imports._target_evidence(self.infra, self.operation, self.directory, manager=self.manager, job='c' * 64)
        self.assertEqual(first['bounded_job_sha256'], 'c' * 64)
        cipher = self.archive.archive.read_bytes()
        self.archive.archive.write_bytes(b'corrupted cipher')
        with self.assertRaises(BackupError): instance_imports._target_evidence(self.infra, self.operation, self.directory, manager=self.manager)
        self.archive.archive.write_bytes(cipher)
        (self.objects / 'result.txt').write_bytes(b'changed target')
        with self.assertRaisesRegex(BackupError, 'object_'):
            instance_imports._target_evidence(self.infra, self.operation, self.directory, manager=self.manager)

    def test_import_missing_original_epoch_metadata_fails_before_target_write(self):
        verified = self.archive.verify(self.catalog)
        del verified.manifest['import_provenance']
        with self.assertRaisesRegex(BackupError, 'original_provenance_required'): target.original_provenance(verified)
        verified.manifest['import_provenance'] = {'version': 1, 'origin_epoch': str(uuid.uuid4()), 'run_ids': [self.run_id]}
        with self.assertRaisesRegex(BackupError, 'original_epoch'): target.original_provenance(verified)

    def test_restore_reads_target_before_activation_and_does_not_activate_corruption(self):
        require_owned = self.generation.resolve().is_relative_to(self.root.resolve())
        self.assertTrue(require_owned); shutil.rmtree(self.generation)
        self.manager.volume_archive = lambda *args, **kwargs: None  # Existing actual fixture data models restore IO.
        with patch('remote_lifecycle_qualification.validate_workspaces'), patch('execution_runtime.probe'):
            (self.objects / 'result.txt').write_bytes(b'corrupted restore')
            with self.assertRaisesRegex(BackupError, 'object_'):
                self.manager.restore(self.operation, self.directory, self.directory,
                                     source_import=self.operation['arguments']['import_binding'])
        self.assertFalse((self.directory / 'active.json').exists())
        self.assertFalse((self.directory / 'compose.json').exists())
        self.assertFalse(any(path.name.startswith('receipt-') for path in self.directory.rglob('*')))

    def test_successful_restore_measures_before_active_pointer_write(self):
        self.assertTrue(self.generation.resolve().is_relative_to(self.root.resolve())); shutil.rmtree(self.generation)
        self.manager.volume_archive = lambda *args, **kwargs: None  # External restore Engine IO is a fixture.
        self.calls.clear()
        with patch('remote_lifecycle_qualification.validate_workspaces'), patch('execution_runtime.probe'):
            restored = self.manager.restore(self.operation, self.directory, self.directory,
                source_import=self.operation['arguments']['import_binding'])
        activation = self.calls.index(('write', 'active.json'))
        self.assertLess(self.calls.index(('object-read', 'actual-loopback-HTTP')), activation)
        self.assertLess(self.calls.index(('workspace-read', 'actual-owned-file-helper')), activation)
        self.assertEqual(self.active_directory(self.directory), self.generation)
        self.assertTrue(restored['import_target']['workspace_inventory_complete'])
        self.assertFalse(any(path.name.startswith('receipt-') for path in self.directory.rglob('*')))

    def test_import_legacy_manifest_is_not_rewritten_and_rejects_before_generation(self):
        self.assertTrue(self.generation.resolve().is_relative_to(self.root.resolve())); shutil.rmtree(self.generation)
        del self.archive.manifest['import_provenance']
        catalog = self.archive.encrypt()
        frozen = self.operation['arguments']['import_binding']; frozen['backup'] = catalog
        self.operation['backup'] = catalog
        self.operation['arguments']['import_binding_sha256'] = instance_imports.canonical(frozen)
        self.operation['arguments']['import_export'].update(manifest_sha256=instance_imports.canonical(self.archive.manifest),
            backup_sha256=catalog['sha256'])
        with patch('execution_runtime.probe') as probe:
            with self.assertRaisesRegex(BackupError, 'original_provenance_required'):
                self.manager.restore(self.operation, self.directory, self.directory, source_import=frozen)
            probe.assert_not_called()
        self.assertFalse(self.generation.exists()); self.assertFalse((self.directory / 'active.json').exists())
        self.assertNotIn('import_provenance', self.archive.manifest)

    def test_credential_swap_during_actual_object_read_refuses_final_success(self):
        actual = self.manager.read_object
        def swap(*arguments):
            evidence = actual(*arguments)
            secret = self.archive.files['secrets/runner_signing_key']
            (self.generation / 'secrets/runner_signing_key').write_bytes(secret)
            (self.generation / 'runtime-secrets/runner_signing_key').write_bytes(secret)
            return evidence
        self.manager.read_object = swap
        with self.assertRaisesRegex(BackupError, 'source_credential_reused'): self.verify()
        self.assertFalse((self.directory / 'active.json').exists())

    def test_exact_history_change_during_actual_object_read_refuses_final_success(self):
        actual = self.manager.read_object
        original = copy.deepcopy(self.history)
        for change in ['terminal-state', 'origin', 'run-id', 'schedule', 'session', 'executor-credential']:
            with self.subTest(change=change):
                self.history = copy.deepcopy(original)
                def mutate(*arguments):
                    evidence = actual(*arguments)
                    if change == 'terminal-state': self.history['runs'][0]['state'] = 'failed'
                    elif change == 'origin': self.history['origins'][0]['source_backup_id'] = str(uuid.uuid4())
                    elif change == 'run-id': self.history['runs'][0]['id'] = str(uuid.uuid4())
                    elif change == 'schedule': self.history['active_schedules'] = 1
                    elif change == 'session': self.history['active_sessions'] = 1
                    else: self.history['active_credentials'] = 1
                    return evidence
                self.manager.read_object = mutate
                with self.assertRaisesRegex(BackupError, 'history_changed|history_inventory|origin_provenance|fences_unverified'):
                    self.verify()
                self.assertFalse((self.directory / 'active.json').exists())

    def test_workspace_read_cannot_outlive_quiescence_or_activation_guard(self):
        actual = self.manager.capture_import_volume
        self.manager.quiescent.side_effect = [None, BackupError('import_target_no_longer_quiescent')]
        with self.assertRaisesRegex(BackupError, 'no_longer_quiescent'): self.verify()
        self.assertEqual(self.manager.quiescent.call_count, 2)
        self.manager.quiescent.side_effect = None
        command = self.manager.run
        disabled = False
        def read(*arguments):
            nonlocal disabled
            evidence = actual(*arguments); disabled = True; return evidence
        def run(arguments, **options):
            if disabled and arguments[0] == 'psql' and 'data' in options:
                raise subprocess.CalledProcessError(3, ['psql'])
            return command(arguments, **options)
        self.manager.capture_import_volume = read; self.manager.run = run
        with self.assertRaises(subprocess.CalledProcessError): self.verify()
        self.assertFalse((self.directory / 'active.json').exists())

    def test_previous_target_bootstrap_runner_and_fleet_are_not_reused(self):
        for name in ['bootstrap_token', 'runner_signing_key', 'fleet_key']:
            with self.subTest(secret=name):
                path = self.generation / 'secrets' / name
                runtime = self.generation / 'runtime-secrets' / name
                original, original_runtime = path.read_bytes(), runtime.read_bytes()
                copied = ('private-previous-' + name + '-' + uuid.uuid4().hex).encode()
                (self.directory / 'secrets' / name).write_bytes(copied)
                path.write_bytes(copied); runtime.write_bytes(copied)
                with self.assertRaisesRegex(BackupError, 'previous_credential_reused'): self.verify()
                path.write_bytes(original); runtime.write_bytes(original_runtime)
        self.assertFalse((self.directory / 'active.json').exists())

    def test_previous_target_fleet_without_legacy_second_copy_is_still_compared(self):
        original = (self.directory / 'secrets/fleet_key').read_bytes()
        (self.directory / 'secrets/fleet_key').unlink()
        (self.directory / 'runtime-secrets').mkdir()
        (self.directory / 'runtime-secrets/fleet_key').write_bytes(original)
        self.verify()
        content = (self.generation / 'secrets/fleet_key').read_bytes()
        (self.directory / 'runtime-secrets/fleet_key').write_bytes(content)
        with self.assertRaisesRegex(BackupError, 'previous_credential_reused'): self.verify()

    def test_original_fleet_is_compared_only_when_bound_in_archived_manifest(self):
        self.assertNotIn('secrets/fleet_key', self.archive.manifest['files'])
        self.verify()  # Legacy archives need not contain a fleet credential.
        content = ('archived-fleet-' + uuid.uuid4().hex).encode()
        self.archive.files['secrets/fleet_key'] = content
        self.catalog = self.archive.encrypt()
        frozen = self.operation['arguments']['import_binding']; frozen['backup'] = self.catalog
        self.operation['backup'] = self.catalog
        self.operation['arguments']['import_binding_sha256'] = instance_imports.canonical(frozen)
        self.operation['arguments']['import_export'].update(manifest_sha256=instance_imports.canonical(self.archive.manifest),
            backup_sha256=self.catalog['sha256'])
        self.history['origins'][0]['source_manifest_sha256'] = instance_imports.canonical(self.archive.manifest)
        self.history['origins'][0]['source_backup_id'] = self.catalog['id']
        restore = json.loads((self.generation / 'restore.json').read_bytes())
        restore.update(import_binding_sha256=instance_imports.canonical(frozen),
            original_manifest_sha256=instance_imports.canonical(self.archive.manifest), backup_id=self.catalog['id'])
        self.write(self.generation / 'restore.json', json.dumps(restore).encode())
        self.verify()
        for parent in ['secrets', 'runtime-secrets']:
            (self.generation / parent / 'fleet_key').write_bytes(content)
        with self.assertRaisesRegex(BackupError, 'source_credential_reused'): self.verify()

    def test_readonly_volume_adapter_refuses_foreign_generation_or_owner_before_helper(self):
        operator = object.__new__(streaming.VolumeStreamer)
        operator.operation = self.operation; operator.instance = self.operation['instance']; operator.infra = self.infra
        operator.directory = self.directory; operator.backup_root = self.root
        operator.limits = {'max_archive_bytes': 1048576, 'max_file_bytes': 524288, 'max_files': 100, 'timeout_seconds': 10}
        operator.quiescent = Mock()
        operator.inspect_volume = Mock(return_value={'Labels': {'cowork.volume-stream.instance': str(uuid.uuid4()),
            'cowork.volume-stream.operation': self.operation['id']}})
        operator.transfer = Mock(side_effect=AssertionError('foreign volume must never reach helper'))
        destination = self.root / (self.archive.workspace + '.tar')
        with self.assertRaisesRegex(streaming.StreamError, 'scope'):
            operator.capture_import_target(self.root, self.archive.workspace, destination)
        with self.assertRaisesRegex(streaming.StreamError, 'owner'):
            operator.capture_import_target(self.generation, self.archive.workspace, destination)
        operator.transfer.assert_not_called(); self.assertFalse(destination.exists())

    def test_default_object_adapter_uses_bounded_owned_process_and_reaps_failure(self):
        self.manager.config['s3_admin_config_directory'] = str(self.root)
        original_popen = subprocess.Popen
        children, observed = [], []
        body = b'actual-owned-process-bytes'
        def process(arguments, **options):
            observed.append(arguments)
            child = original_popen([sys.executable, '-c', 'import sys;sys.stdout.buffer.write(' + repr(body) + ')'], **options)
            children.append(child)
            return child
        with patch.object(target.subprocess, 'Popen', side_effect=process):
            destination = self.root / (uuid.uuid4().hex + '.object')
            evidence = target.read_object(self.manager, 'owned-fixture/bucket/result.txt', destination, len(body))
            self.assertEqual(evidence, {'bytes': len(body), 'sha256': hashlib.sha256(body).hexdigest()})
            self.assertEqual(destination.read_bytes(), body)
            with self.assertRaises(streaming.StreamError):
                target.read_object(self.manager, 'owned-fixture/bucket/result.txt', self.root / (uuid.uuid4().hex + '.object'), 1)
        self.assertTrue(all(child.poll() is not None for child in children))
        self.assertEqual(observed[0], ['mc', '--config-dir', str(self.root), 'cat', 'owned-fixture/bucket/result.txt'])

    @unittest.skipUnless(os.environ.get('COWORK_TEST_DATABASE_URL'), 'requires own private PostgreSQL fixture')
    def test_actual_postgres_exact_origins_and_activation_guard_readback(self):
        from instance_backups import pg_environment
        schema = 'cowork_target_readback_' + uuid.uuid4().hex
        environment = pg_environment(os.environ['COWORK_TEST_DATABASE_URL'])
        exits = []
        def sql(text, *, scoped=True):
            env = {**environment, **({'PGOPTIONS': '-c search_path=' + schema + ',public'} if scoped else {})}
            completed = subprocess.run(['psql', '-X', '-v', 'ON_ERROR_STOP=1', '-tA'], input=text.encode(),
                capture_output=True, env=env, timeout=20)
            exits.append(completed.returncode); completed.check_returncode(); return completed.stdout
        sql('CREATE SCHEMA ' + schema, scoped=False)
        try:
            sql("CREATE TABLE runs(id UUID PRIMARY KEY,state TEXT,lease_owner UUID,lease_token UUID,lease_expires_at TIMESTAMPTZ);"
                "CREATE TABLE schedules(enabled BOOLEAN,next_run_at TIMESTAMPTZ);"
                "CREATE TABLE auth_sessions(revoked_at TIMESTAMPTZ);CREATE TABLE executor_credentials(revoked_at TIMESTAMPTZ);"
                "INSERT INTO runs VALUES('" + self.run_id + "','interrupted',NULL,NULL,NULL);")
            sql(instance_imports.origin_fence(self.operation).decode())
            command = self.manager.run
            self.manager.run = lambda args, **options: (sql(options['data'].decode() if 'data' in options else args[-1])
                if args[0] == 'psql' else command(args, **options))
            verified = self.archive.verify(self.catalog)
            provenance = target.original_provenance(verified)
            observed = target.target_history(self.manager, self.operation, self.generation, provenance)
            self.assertEqual(observed['runs'][0]['id'], self.run_id)
            actual_object_read = self.manager.read_object
            changes = {
                'terminal-state': "UPDATE runs SET state='failed'",
                'origin': "UPDATE run_instance_import_origins SET source_backup_id='" + str(uuid.uuid4()) + "'",
                'schedule': "INSERT INTO schedules VALUES(true,NULL)",
                'session': "INSERT INTO auth_sessions VALUES(NULL)",
                'executor-credential': "INSERT INTO executor_credentials VALUES(NULL)",
            }
            for name, mutation in changes.items():
                with self.subTest(late_database_change=name):
                    def late_read(*arguments):
                        evidence = actual_object_read(*arguments); sql(mutation); return evidence
                    self.manager.read_object = late_read
                    with self.assertRaisesRegex(BackupError, 'history_changed|origin_provenance|fences_unverified'):
                        self.verify()
                    self.assertFalse((self.directory / 'active.json').exists())
                    sql("UPDATE runs SET state='interrupted'; DELETE FROM schedules; DELETE FROM auth_sessions;"
                        "DELETE FROM executor_credentials; UPDATE run_instance_import_origins SET source_backup_id='" + self.catalog['id'] + "'")
            self.manager.read_object = actual_object_read
            sql("UPDATE run_instance_import_origins SET source_backup_id='" + str(uuid.uuid4()) + "'")
            with self.assertRaisesRegex(BackupError, 'origin_provenance'):
                target.target_history(self.manager, self.operation, self.generation, provenance)
            sql("UPDATE run_instance_import_origins SET source_backup_id='" + self.catalog['id'] + "'")
            actual_workspace_read = self.manager.capture_import_volume
            def removed_guard(*arguments):
                evidence = actual_workspace_read(*arguments)
                sql('DROP TRIGGER imported_run_activation_fence ON runs')
                return evidence
            self.manager.capture_import_volume = removed_guard
            with self.assertRaises(subprocess.CalledProcessError):
                self.verify()
            self.assertEqual(sql('SELECT state FROM runs').strip(), b'interrupted')
        finally:
            sql('DROP SCHEMA ' + schema + ' CASCADE', scoped=False)
        self.assertEqual(sql("SELECT count(*) FROM pg_namespace WHERE nspname='" + schema + "'", scoped=False).strip(), b'0')
        self.assertEqual(exits.count(1) + exits.count(3), 1)  # Exactly the deliberate missing-guard psql failure.


if __name__ == '__main__': unittest.main()
