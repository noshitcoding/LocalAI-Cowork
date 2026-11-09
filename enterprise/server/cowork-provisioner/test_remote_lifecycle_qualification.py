"""Real archive validation and typed lifecycle integration without Docker starts."""
import copy
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
from types import SimpleNamespace
import unittest
import uuid
from unittest.mock import Mock, patch

import execution_generation
import executor_host
import guest_endpoints
import host_capacity_attestation
import instance_backups
from instance_backups import BackupError, InstanceBackups, pg_environment
import provisioner as p
import remote_lifecycle_qualification as lifecycle
import remote_volume_streaming as streaming
import resource_budget


class RemoteLifecycleTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(); self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.state = self.root / 'state'; self.state.mkdir(mode=0o700)
        self.backups = self.root / 'backups'; self.backups.mkdir(mode=0o700)
        self.ca = self.root / 'ca.pem'
        self.ca.write_text('-----BEGIN CERTIFICATE-----\nZml4dHVyZS1vbmx5\n-----END CERTIFICATE-----')
        self.ca.chmod(0o600)
        self.admin = self.root / 'admin-url'
        self.admin.write_text('postgres://operator:private-admin@localhost:23450/postgres'); self.admin.chmod(0o600)
        self.mc = self.root / 'mc'; self.mc.mkdir(mode=0o700)
        self.mc_file = self.mc / 'config.json'
        self.mc_file.write_text(json.dumps({'version': '10', 'aliases': {'fixture': {
            'url': 'https://localhost:23451', 'accessKey': 'own-operator', 'secretKey': 'own-private-fixture'}}}))
        self.mc_file.chmod(0o600)
        selection = {'id': str(uuid.uuid4()), 'trust_domain': str(uuid.uuid4()), 'operator_state_id': str(uuid.uuid4()),
            'endpoint': 'tcp://localhost:22376', 'daemon_id': 'a' * 64,
            **{key: str(self.ca) for key in ['ca_file', 'cert_file', 'key_file']}}
        self.instance = {'id': str(uuid.uuid4()), 'organisation_id': selection['trust_domain'], 'state': 'stopped',
            'backend': 'compose', 'executor_host_id': selection['id'], 'execution_epoch': str(uuid.uuid4()),
            'policy': {'minimum_isolation': 'container', 'limits': {'cpu_millis': 500, 'memory_mib': 512,
                       'storage_gib': 1, 'concurrent_runs': 1}}}
        images = {key: 'registry.invalid/' + key + '@sha256:' + 'b' * 64 for key in
                  ['server', 'runner', 'egress_proxy', 'sandbox_core', 'sandbox_gui', 'sandbox_crew']}
        self.config = {'backend': 'compose', 'state_directory': str(self.state), 'backup_directory': str(self.backups),
            'executor_host': selection, 'images': images, 'sandbox_security_profile': 'linux_apparmor',
            'fleet_host_budget': {'cpu_millis': 4000, 'memory_mib': 3584, 'reserved_cpu_millis': 1000, 'reserved_memory_mib': 1024},
            'host_capacity_attestation': {'version': 1, 'host_revision': 2}, 'execution_admission': {'server_images': [images['server']]},
            'database_admin_url_file': str(self.admin), 'database_endpoint': '10.0.2.2:23456',
            's3_admin_config_directory': str(self.mc), 's3_admin_alias': 'fixture',
            's3_endpoint': 'https://10.0.2.2:23457', 'fleet_management_url': 'https://10.0.2.2:23458',
            'remote_volume_streaming_preparation': {'version': 1, 'max_archive_bytes': 1048576,
                'max_file_bytes': 524288, 'max_files': 100, 'timeout_seconds': 10}}
        binding = executor_host.manager_binding(self.config)
        self.config['guest_runtime_endpoints'] = {'version': 1, 'instance_id': self.instance['id'],
            'executor_host_binding': binding, 'public_ca_file': str(self.ca), 'api_loopback_port': 38080,
            **{key: self.config[key] for key in ['database_endpoint', 's3_endpoint', 'fleet_management_url']}}
        self.config['remote_lifecycle_qualification'] = {'version': 1, 'mode': 'fixture_only', 'fixture_id': str(uuid.uuid4()),
            'instance_id': self.instance['id'], 'executor_host_binding': binding,
            'max_workspace_archives': 3, 'max_workspace_archive_bytes': 1048576}
        context = patch.object(guest_endpoints.ssl, 'create_default_context', return_value=Mock())
        context.start(); self.addCleanup(context.stop)
        self.infra = SimpleNamespace(config=self.config, root=self.state, private_write=p.private_write,
            host=Mock(), capacity_attestor=Mock(), resource_names=p.resource_names,
            active_directory=lambda base: base, run=Mock(), ensure_database=Mock(), ensure_bucket=Mock(), ensure_secrets=Mock())
        lifecycle.bind(self.infra)
        self.directory = self.state / uuid.UUID(self.instance['id']).hex; self.directory.mkdir(mode=0o700)
        execution_generation.write(self.infra, self.instance, self.directory, self.instance['execution_epoch'])

    def operation(self, action='restore'):
        operation_id = str(uuid.uuid4()); plan = resource_budget.instance_budget(self.config, self.instance)
        return {'id': operation_id, 'instance_id': self.instance['id'], 'organisation_id': self.instance['organisation_id'],
            'action': action, 'instance': copy.deepcopy(self.instance), 'arguments': {},
            'executor_host_binding': executor_host.manager_binding(self.config),
            'execution_binding': {'operation_id': operation_id, 'organisation_id': self.instance['organisation_id'],
                'instance_id': self.instance['id'], 'source_epoch': self.instance['execution_epoch'],
                'candidate_epoch': str(uuid.uuid4()) if action in ['restore', 'upgrade', 'rollback'] else self.instance['execution_epoch']},
            'host_allocation': {'binding': {'version': 1, 'host_id': self.config['executor_host']['id'],
                'organisation_id': self.instance['organisation_id'], 'instance_id': self.instance['id'],
                'allocation_revision': 1, 'plan_sha256': host_capacity_attestation.digest(plan),
                'execution_epoch': self.instance['execution_epoch']}, 'plan': plan}}

    def test_default_and_stream_preparation_do_not_open_lifecycle(self):
        config = copy.deepcopy(self.config); config.pop('remote_lifecycle_qualification')
        self.assertIsNone(lifecycle.settings(config))
        with self.assertRaisesRegex(lifecycle.LifecycleError, 'not_qualified'):
            lifecycle.enabled(config)
        for action in ['enroll_execution', 'start', 'backup']:
            with self.assertRaises(lifecycle.LifecycleError):
                lifecycle.instance(config, self.instance, action)

    def test_host_instance_mode_capacity_and_bounds_cannot_be_bypassed(self):
        for mutate in [lambda c: c['remote_lifecycle_qualification'].update(mode='production'),
                       lambda c: c['remote_lifecycle_qualification'].update(max_workspace_archives=True),
                       lambda c: c['remote_lifecycle_qualification']['executor_host_binding'].update(host_id=str(uuid.uuid4())),
                       lambda c: c.pop('host_capacity_attestation'), lambda c: c.pop('guest_runtime_endpoints')]:
            changed = copy.deepcopy(self.config); mutate(changed)
            with self.assertRaises(lifecycle.LifecycleError): lifecycle.settings(changed)
        for key in ['host_allocation', 'execution_binding']:
            operation = self.operation(); operation.pop(key)
            with self.assertRaises((lifecycle.LifecycleError, execution_generation.ExecutionGenerationError)):
                lifecycle.operation(self.infra, operation)
        operation = self.operation(); operation['host_allocation']['binding']['execution_epoch'] = str(uuid.uuid4())
        with self.assertRaisesRegex(lifecycle.LifecycleError, 'allocation'):
            lifecycle.operation(self.infra, operation)

    def test_retained_root_cannot_disable_retarget_or_adopt_qualification(self):
        changed = copy.deepcopy(self.config); changed['remote_lifecycle_qualification']['fixture_id'] = str(uuid.uuid4())
        with self.assertRaisesRegex(lifecycle.LifecycleError, 'retarget'):
            lifecycle.bind(SimpleNamespace(root=self.state, config=changed))
        changed.pop('remote_lifecycle_qualification')
        with self.assertRaisesRegex(lifecycle.LifecycleError, 'disable'):
            lifecycle.bind(SimpleNamespace(root=self.state, config=changed))
        (self.state / 'remote-lifecycle-qualification.json').unlink()
        with self.assertRaisesRegex(lifecycle.LifecycleError, 'new_fixture'):
            lifecycle.bind(self.infra)

    def test_unknown_container_or_running_workload_blocks_entire_guest(self):
        self.infra.run.return_value = b'foreign-container\n'
        with self.assertRaisesRegex(lifecycle.LifecycleError, 'unknown_daemon'):
            lifecycle.quiescent(self.infra, self.instance)
        self.infra.run = Mock(side_effect=[b'', b'actual-running-id'])
        with self.assertRaisesRegex(lifecycle.LifecycleError, 'quiescent_daemon'):
            lifecycle.quiescent(self.infra, self.instance)
        self.infra.run = Mock(side_effect=[b'', b''])
        lifecycle.quiescent(self.infra, self.instance)

    def test_controller_tls_mapping_preserves_instance_role_and_removes_ambient_pg_options(self):
        name = p.resource_names(self.instance, self.directory)['database']
        url = guest_endpoints.database_url(self.config, 'postgres://' + name + ':runtime-password@10.0.2.2:23456/' + name)
        with patch.dict(os.environ, {'PGSSLMODE': 'disable', 'PGOPTIONS': '-c role=operator', 'PGHOSTADDR': 'foreign'}):
            environment = pg_environment(url, self.config)
        self.assertEqual(environment['PGHOST'], 'localhost'); self.assertEqual(environment['PGPORT'], '23450')
        self.assertEqual(environment['PGUSER'], name); self.assertEqual(environment['PGPASSWORD'], 'runtime-password')
        self.assertEqual(environment['PGSSLMODE'], 'verify-full'); self.assertEqual(environment['PGSSLROOTCERT'], str(self.ca))
        self.assertNotIn('PGOPTIONS', environment); self.assertNotIn('PGHOSTADDR', environment)
        for altered in [url.replace('sslmode=verify-full', 'sslmode=disable'), url.replace('10.0.2.2', 'foreign'),
                        url.replace(name, 'postgres'), url + '&sslmode=disable']:
            with self.assertRaises(BackupError): pg_environment(altered, self.config)

    def test_stopped_service_name_without_exact_compose_image_and_labels_is_unknown(self):
        prefix = 'cowork-' + uuid.UUID(self.instance['id']).hex + '-'
        p.private_write(self.directory / 'compose.json', json.dumps({'services': {
            'api': {'image': self.config['images']['server']}}}).encode())
        container = {'Name': '/' + prefix + 'api-1', 'State': {'Running': False},
            'Config': {'Image': self.config['images']['server'], 'Labels': {
                'com.docker.compose.project': prefix[:-1], 'com.docker.compose.service': 'api'}}}
        for mutate in [lambda item: item['Config'].update(Image='foreign'),
                       lambda item: item['Config']['Labels'].update({'com.docker.compose.project': 'foreign'})]:
            changed = copy.deepcopy(container); mutate(changed)
            self.infra.run = Mock(side_effect=[(prefix + 'api-1').encode(), b'', json.dumps([changed]).encode()])
            with self.assertRaisesRegex(lifecycle.LifecycleError, 'unknown_daemon'):
                lifecycle.quiescent(self.infra, self.instance)
        self.infra.run = Mock(side_effect=[(prefix + 'api-1').encode(), b'', json.dumps([container]).encode()])
        lifecycle.quiescent(self.infra, self.instance)

    @unittest.skipUnless(shutil.which('age') and shutil.which('age-keygen'), 'requires actual age encryption/decryption')
    def test_real_restore_rejects_unsafe_inner_tar_before_generation_database_or_activation(self):
        operation = self.operation(); backup_id = str(uuid.uuid4()); suffix = uuid.uuid4().hex
        workspace = io.BytesIO()
        with tarfile.open(fileobj=workspace, mode='w') as output:
            member = tarfile.TarInfo('marker'); member.type = tarfile.SYMTYPE; member.linkname = '/etc/passwd'; output.addfile(member)
        files = {name: b'fixture' for name in ['database.dump', 'instance.json', 'secrets/storage_master_key',
                                              'secrets/bootstrap_token', 'secrets/runner_signing_key']}
        files['instance.json'] = json.dumps(self.instance).encode()
        files['workspaces/' + suffix + '.tar'] = workspace.getvalue()
        manifest = {'schema_version': 1, 'backup_id': backup_id, 'instance_id': self.instance['id'],
            'organisation_id': self.instance['organisation_id'], 'images': self.config['images'], 'workspace_runs': [suffix],
            'executor_host_binding': operation['executor_host_binding'],
            'remote_fixture_id': self.config['remote_lifecycle_qualification']['fixture_id'],
            'files': {name: {'bytes': len(content), 'sha256': hashlib.sha256(content).hexdigest()} for name, content in files.items()}}
        plain = self.root / 'valid-outer.tar'
        with tarfile.open(plain, 'w') as output:
            for name, content in [('manifest.json', json.dumps(manifest).encode()), *files.items()]:
                entry = tarfile.TarInfo(name); entry.size = len(content); output.addfile(entry, io.BytesIO(content))
        identity = self.root / 'actual-private-age-identity'
        subprocess.run(['age-keygen', '-o', str(identity)], check=True, capture_output=True, timeout=15)
        recipient = subprocess.run(['age-keygen', '-y', str(identity)], check=True, capture_output=True, timeout=15).stdout.decode().strip()
        encrypted = self.backups / (uuid.UUID(backup_id).hex + '.age')
        subprocess.run(['age', '--encrypt', '-r', recipient, '-o', str(encrypted), str(plain)], check=True, capture_output=True, timeout=15)
        operation.update(arguments={'backup_id': backup_id}, backup={'id': backup_id, 'instance_id': self.instance['id'],
            'organisation_id': self.instance['organisation_id'], 'sha256': hashlib.sha256(encrypted.read_bytes()).hexdigest(),
            'size_bytes': encrypted.stat().st_size})
        engine = object.__new__(InstanceBackups)
        engine.infra, engine.config, engine.root, engine.write, engine.identity = self.infra, self.config, self.backups, p.private_write, str(identity)
        engine.run = lambda argv, **kwargs: subprocess.run(argv, check=True, capture_output=True, timeout=15).stdout
        with patch.object(p.execution_runtime, 'probe') as probe:
            with self.assertRaisesRegex(BackupError, 'unsafe_backup_workspace_entry'):
                engine.restore(operation, self.directory, self.directory)
            probe.assert_not_called()
        self.infra.ensure_database.assert_not_called(); self.infra.ensure_secrets.assert_not_called()
        self.assertFalse((self.directory / 'generations').exists()); self.assertFalse((self.directory / 'active.json').exists())

    def test_controller_storage_ignores_ambient_alias_insecure_and_ca_overrides(self):
        with patch.dict(os.environ, {'MC_HOST_fixture': 'http://foreign', 'MC_INSECURE': 'true',
                                    'SSL_CERT_FILE': 'foreign', 'SSL_CERT_DIR': 'foreign'}):
            environment = guest_endpoints.controller_storage_environment(self.config)
        self.assertEqual(environment['SSL_CERT_FILE'], str(self.ca))
        for key in ['MC_HOST_fixture', 'MC_INSECURE', 'SSL_CERT_DIR']:
            self.assertNotIn(key, environment)
        engine = object.__new__(InstanceBackups); engine.config, engine.run = self.config, Mock(return_value=b'own')
        self.assertEqual(engine.storage('mirror', 'fixture/bucket', 'own-target'), b'own')
        self.assertEqual(engine.run.call_args.kwargs['env']['SSL_CERT_FILE'], str(self.ca))

    def test_controller_storage_rejects_http_foreign_alias_and_shared_private_config(self):
        source = json.loads(self.mc_file.read_text())
        for endpoint in ['http://localhost:23451', 'https://foreign:23451', 'https://user:secret@localhost:23451',
                         'https://localhost:23451?insecure=true']:
            altered = copy.deepcopy(source); altered['aliases']['fixture']['url'] = endpoint
            self.mc_file.write_text(json.dumps(altered))
            with self.assertRaises(guest_endpoints.GuestEndpointError):
                guest_endpoints.controller_storage_environment(self.config)
        self.mc_file.write_text(json.dumps(source))
        inside = self.state / 'mc'; inside.mkdir()
        changed = {**self.config, 's3_admin_config_directory': str(inside)}
        with self.assertRaises(guest_endpoints.GuestEndpointError):
            guest_endpoints.controller_storage_environment(changed)

    def test_volume_adapter_never_accepts_foreign_target_or_controller_bind_mount(self):
        operation = self.operation('backup'); suffix = uuid.uuid4().hex
        engine = object.__new__(InstanceBackups); engine.infra, engine.config = self.infra, self.config
        operator = Mock(); operator.scope.return_value = p.resource_names(self.instance, self.directory)['volume_prefix'] + suffix
        with patch.object(streaming, 'VolumeStreamer', return_value=operator):
            with self.assertRaisesRegex(BackupError, 'workspace_scope'):
                engine.volume_archive('foreign-volume', self.backups, suffix + '.tar', operation=operation, directory=self.directory)
            operator.capture.assert_not_called(); operator.restore.assert_not_called()
            engine.volume_archive(operator.scope.return_value, self.backups, suffix + '.tar', operation=operation, directory=self.directory)
            operator.capture.assert_called_once_with(self.directory, suffix, self.backups / (suffix + '.tar'))
            self.infra.run.assert_not_called()

    def test_aggregate_workspace_archive_and_count_are_finite(self):
        suffix = uuid.uuid4().hex
        manifest = {'workspace_runs': [suffix], 'files': {'workspaces/' + suffix + '.tar': {'bytes': 1048577}}}
        with self.assertRaisesRegex(lifecycle.LifecycleError, 'total_exceeds'):
            lifecycle.workspace_inventory(self.config, manifest)
        manifest['workspace_runs'] *= 4
        with self.assertRaisesRegex(lifecycle.LifecycleError, 'inventory_exceeds'):
            lifecycle.workspace_inventory(self.config, manifest)

    def test_unlisted_or_missing_workspace_tar_is_not_silently_skipped(self):
        suffix, extra = uuid.uuid4().hex, uuid.uuid4().hex
        for files in [{}, {'workspaces/' + suffix + '.tar': {'bytes': 10240},
                          'workspaces/' + extra + '.tar': {'bytes': 10240}}]:
            with self.assertRaisesRegex(lifecycle.LifecycleError, 'archive_inventory_mismatch'):
                lifecycle.workspace_inventory(self.config, {'workspace_runs': [suffix], 'files': files})


if __name__ == '__main__':
    unittest.main()
