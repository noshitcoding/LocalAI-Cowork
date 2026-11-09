import copy
import json
import os
from pathlib import Path
import tempfile
import unittest
import uuid
from unittest.mock import Mock, patch

import executor_host as host
import provisioner as p


class ExecutorHostTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.org = str(uuid.uuid4())
        self.selection = {'id': str(uuid.uuid4()), 'trust_domain': self.org, 'endpoint': 'tcp://executor.example.test:2376',
                          'daemon_id': 'a' * 64, 'operator_state_id': str(uuid.uuid4())}
        for key in ['ca_file', 'cert_file', 'key_file']:
            path = self.root / key
            path.write_text('test-only-secret')
            path.chmod(0o600)
            self.selection[key] = str(path)
        self.config = {'backend': 'compose', 'state_directory': str(self.root / 'instances'), 'executor_host': self.selection,
                       'fleet_host_budget': {'cpu_millis': 8000, 'memory_mib': 16000, 'reserved_cpu_millis': 1000, 'reserved_memory_mib': 2048}}
        self.instance = {'id': str(uuid.uuid4()), 'organisation_id': self.org, 'backend': 'compose',
                         'policy': {'minimum_isolation': 'container'}, 'executor_host_id': self.selection['id']}
        self.tls = patch.object(host.ssl, 'create_default_context', return_value=Mock())
        self.tls.start()
        self.addCleanup(self.tls.stop)
        self.infra = p.Infrastructure(self.config)
        self.info = {'ID': self.selection['daemon_id'], 'Labels': ['cowork.role=executor', 'cowork.trust-domain=' + self.org],
                     'NCPU': 16, 'MemTotal': 32 * 1024 * 1024 * 1024}

    def test_all_docker_operations_use_explicit_tls_and_sanitize_ambient_context(self):
        commands = [['docker', 'compose', 'stop'], ['docker', 'volume', 'ls'], ['docker', 'rm', '--force', 'owned'],
                    ['docker', 'network', 'inspect', 'owned'], ['docker', 'image', 'tag', 'source', 'target']]
        with patch.dict(os.environ, {'DOCKER_HOST': 'tcp://attacker:2375', 'DOCKER_CONTEXT': 'foreign', 'DOCKER_TLS_VERIFY': '0'}):
            with patch.object(p, 'run', side_effect=lambda args, **kwargs: json.dumps(self.info).encode() if 'info' in args else b'OK') as run:
                for command in commands:
                    self.assertEqual(self.infra.run(command), b'OK')
                self.assertEqual(run.call_count, len(commands) * 2)
                for call in run.call_args_list:
                    args = call.args[0]
                    self.assertEqual(args[:4], ['docker', '--host', self.selection['endpoint'], '--tlsverify'])
                    self.assertNotIn('DOCKER_HOST', call.kwargs['env'])
                    self.assertNotIn('DOCKER_CONTEXT', call.kwargs['env'])
                    self.assertNotIn('DOCKER_TLS_VERIFY', call.kwargs['env'])

    def test_daemon_identity_or_domain_change_prevents_mutation(self):
        for info in [{'ID': 'b' * 64, 'Labels': self.info['Labels']}, {'ID': self.info['ID'], 'Labels': ['cowork.role=executor']}]:
            with self.subTest(info=info), patch.object(p, 'run', return_value=json.dumps(info).encode()) as run:
                with self.assertRaises(p.ProvisioningError):
                    self.infra.run(['docker', 'rm', '--force', 'owned'])
                self.assertEqual(run.call_count, 1)
                self.assertIn('info', run.call_args.args[0])

    def test_migration_and_removing_remote_selection_cannot_retarget_journals(self):
        directory = self.infra.directory(self.instance['id'])
        p.private_write(directory / 'operation-test.json', b'{}')
        changed = copy.deepcopy(self.config)
        changed['executor_host']['endpoint'] = 'tcp://other.example.test:2376'
        for config in [changed, {'backend': 'compose', 'state_directory': self.config['state_directory']}]:
            infra = p.Infrastructure(config)
            with self.assertRaisesRegex(p.ProvisioningError, 'migration_requires_review'):
                infra.directory(self.instance['id'])

    def test_existing_local_state_cannot_be_assumed_remote(self):
        root = self.infra.root
        (root / uuid.uuid4().hex).mkdir()
        with self.assertRaisesRegex(p.ProvisioningError, 'legacy_state_requires_review'):
            self.infra.directory(self.instance['id'])

    def test_other_organisation_vm_and_unqualified_lifecycle_fail_before_command(self):
        directory = self.infra.directory(self.instance['id'])
        other = copy.deepcopy(self.instance)
        other['organisation_id'] = str(uuid.uuid4())
        vm = copy.deepcopy(self.instance)
        vm['policy']['minimum_isolation'] = 'virtual_machine'
        with patch.object(p, 'run') as run:
            for instance, action in [(other, 'provision'), (vm, 'provision'), *[(self.instance, action)
                for action in ['backup', 'restore', 'upgrade', 'rollback', 'enroll_execution']]]:
                with self.subTest(action=action), self.assertRaises(host.HostError):
                    self.infra.host.prepare(instance, directory, action)
            run.assert_not_called()

    def test_shared_state_requires_reading_exact_nonce_from_remote_bind(self):
        def remote(args, **kwargs):
            if 'info' in args:
                return json.dumps(self.info).encode()
            marker = args[-1].split('/')[-1]
            return (self.infra.root / marker).read_bytes()
        with patch.object(p, 'run', side_effect=remote) as run:
            self.infra.host.verify_shared_state('registry.test/sandbox@sha256:' + 'a' * 64)
            args = run.call_args.args[0]
            self.assertIn('type=bind,src=' + str(self.infra.root) + ',dst=/qualification,readonly', args)
            self.assertIn('--network', args)
            self.assertEqual(args[args.index('--user') + 1], str(os.geteuid()) if os.name == 'posix' else '0')
            self.assertIn('--read-only', args)
            self.assertEqual(args[args.index('--cap-drop') + 1], 'ALL')
            self.assertFalse(list(self.infra.root.glob('qualification-*')))
        with patch.object(p, 'run', side_effect=[json.dumps(self.info).encode(), b'wrong-filesystem']):
            with self.assertRaisesRegex(host.HostError, 'shared_state_unproven'):
                self.infra.host.verify_shared_state('registry.test/sandbox@sha256:' + 'a' * 64)
        self.assertFalse(list(self.infra.root.glob('qualification-*')))

    def test_tls_paths_and_endpoint_overrides_are_rejected(self):
        for endpoint in ['tcp://executor.example.test:1023', 'tcp://user@executor.example.test:2376',
                         'unix:///var/run/docker.sock', 'tcp://executor.example.test:2376/path']:
            config = copy.deepcopy(self.config)
            config['executor_host']['endpoint'] = endpoint
            with self.subTest(endpoint=endpoint), self.assertRaises(host.HostError):
                host.settings(config)
        with self.assertRaisesRegex(host.HostError, 'override_forbidden'):
            self.infra.host.arguments(['docker', '--host=tcp://foreign:2375', 'ps'])
        with self.assertRaises(p.ProvisioningError):
            p.Infrastructure({'backend': 'compose', 'state_directory': str(self.root / 'other'), 'docker_host': 'tcp://foreign:2375'})

    def test_non_docker_commands_keep_their_database_environment(self):
        with patch.object(p, 'run', return_value=b'') as run:
            self.infra.run(['psql', '-X'], env={'PGHOST': 'database.test'})
            run.assert_called_once_with(['psql', '-X'], env={'PGHOST': 'database.test'})

    def test_remote_requires_capacity_and_rejects_overstated_physical_resources(self):
        missing = copy.deepcopy(self.config)
        del missing['fleet_host_budget']
        with self.assertRaisesRegex(host.HostError, 'resource_budget_required'):
            host.settings(missing)
        info = copy.deepcopy(self.info)
        info['MemTotal'] = 4 * 1024 * 1024 * 1024
        with patch.object(p, 'run', return_value=json.dumps(info).encode()) as run:
            with self.assertRaisesRegex(p.ProvisioningError, 'capacity_mismatch'):
                self.infra.run(['docker', 'rm', '--force', 'owned'])
            self.assertEqual(run.call_count, 1)

    def test_multiple_trust_domain_labels_are_not_dedicated(self):
        info = copy.deepcopy(self.info)
        info['Labels'].append('cowork.trust-domain=' + str(uuid.uuid4()))
        with patch.object(p, 'run', return_value=json.dumps(info).encode()) as run:
            with self.assertRaisesRegex(p.ProvisioningError, 'trust_domain_mismatch'):
                self.infra.run(['docker', 'rm', '--force', 'owned'])
            self.assertEqual(run.call_count, 1)

    def test_unsupported_helper_paths_cannot_bypass_the_operation_guard(self):
        from instance_backups import InstanceBackups, BackupError
        from instance_updates import InstanceUpdates
        from instance_rollbacks import InstanceRollbacks
        from release_catalog import ReleaseError
        import execution_enrollment
        with patch.object(p, 'run') as run:
            with self.assertRaisesRegex(BackupError, 'not_qualified'):
                InstanceBackups(self.infra)
            for engine in [InstanceUpdates, InstanceRollbacks]:
                with self.assertRaisesRegex(ReleaseError, 'not_qualified'):
                    engine(self.infra)
            with self.assertRaisesRegex(execution_enrollment.EnrollmentPreflightError, 'not_qualified'):
                execution_enrollment.inspect(self.infra, self.instance, str(uuid.uuid4()))
            run.assert_not_called()


if __name__ == '__main__':
    unittest.main()
