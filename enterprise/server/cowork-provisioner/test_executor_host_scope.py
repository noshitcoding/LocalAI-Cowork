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
import remote_entrypoint
from operation_journal import OperationJournal


class HostScopeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.org, self.id, self.state = (str(uuid.uuid4()) for _ in range(3))
        selection = {'id': self.id, 'trust_domain': self.org, 'operator_state_id': self.state,
                     'daemon_id': 'a' * 64, 'endpoint': 'tcp://localhost:22376'}
        for key in ['ca_file', 'cert_file', 'key_file']:
            path = self.root / key
            path.write_text('fixture-only')
            path.chmod(0o600)
            selection[key] = str(path)
        self.config = {'backend': 'compose', 'state_directory': str(self.root / 'state'),
                       'executor_host': selection, 'fleet_host_budget': {'cpu_millis': 4000,
                       'memory_mib': 4096, 'reserved_cpu_millis': 1000, 'reserved_memory_mib': 1024}}
        self.binding = host.manager_binding(self.config)
        self.operation = {'id': str(uuid.uuid4()), 'organisation_id': self.org,
                          'instance_id': str(uuid.uuid4()), 'lease_token': str(uuid.uuid4()),
                          'action': 'stop', 'arguments': {}, 'request_digest': 'a' * 64,
                          'executor_host_binding': self.binding}
        self.operation['instance'] = {'id': self.operation['instance_id'], 'organisation_id': self.org,
                                      'executor_host_id': self.id, 'backend': 'compose',
                                      'policy': {'minimum_isolation': 'container'}}
        self.key = 'c' * 64  # Fixture-only per-instance key, never a live credential.
        self.operation['operator_secrets'] = {'version': 1, 'operation_id': self.operation['id'],
                    'lease_token': self.operation['lease_token'], 'organisation_id': self.org,
                    'instance_id': self.operation['instance_id'], 'executor_host_binding': self.binding,
                    'fleet_key': self.key}
        tls = patch.object(host.ssl, 'create_default_context', return_value=Mock())
        tls.start()
        self.addCleanup(tls.stop)
        self.infra = p.Infrastructure(self.config)
        info = {'ID': 'a' * 64, 'Labels': ['cowork.role=executor', 'cowork.trust-domain=' + self.org],
                'NCPU': 4, 'MemTotal': 4096 * 1024 * 1024}
        self.command = patch.object(p, 'run', return_value=json.dumps(info).encode())
        self.run = self.command.start()
        self.addCleanup(self.command.stop)

    def test_claim_contract_exposes_only_frozen_nonsecret_identity(self):
        self.assertEqual(p.operator_contract(self.config), {'backend': 'compose', 'executor_host_binding': self.binding})
        self.assertEqual(set(self.binding), {'version', 'host_id', 'trust_domain', 'daemon_id', 'operator_state_id'})
        self.assertEqual(p.operator_contract({'backend': 'compose'}), {'backend': 'compose'})
        self.assertNotIn('endpoint', json.dumps(self.binding))

    def test_unassigned_remote_and_scoped_local_operations_fail_without_docker(self):
        missing = copy.deepcopy(self.operation)
        del missing['executor_host_binding']
        local = p.Infrastructure({'backend': 'compose', 'state_directory': str(self.root / 'local')})
        for infra, operation in [(self.infra, missing), (local, self.operation)]:
            with self.subTest(remote=infra.host.value is not None), self.assertRaises(p.ProvisioningError):
                infra.validate_host_operation(operation, infra.directory(operation['instance_id']))
        self.run.assert_not_called()

    def test_every_identity_change_and_foreign_assignment_fails_without_docker(self):
        changes = {key: str(uuid.uuid4()) for key in ['host_id', 'trust_domain', 'operator_state_id']}
        changes['daemon_id'] = 'b' * 64
        changes['version'] = 2
        for key, value in changes.items():
            altered = copy.deepcopy(self.operation)
            altered['executor_host_binding'][key] = value
            with self.subTest(key=key), self.assertRaises(p.ProvisioningError):
                self.infra.validate_host_operation(altered, self.infra.directory(altered['instance_id']))
        self.run.assert_not_called()

    def test_success_and_failure_journals_keep_exact_host_binding_on_restart(self):
        for succeeded in [True, False]:
            journal = OperationJournal(self.infra, self.operation)
            result = {'receipt_id': self.operation['id']} if succeeded else {'error_code': 'fixture_only'}
            completed = journal.finish(succeeded, result)
            self.assertEqual(completed['result']['executor_host_binding'], self.binding)
            self.assertEqual(journal.state()['binding']['executor_host_binding'], self.binding)
            self.assertNotIn('operator_secrets', journal.state()['binding'])
            self.assertNotIn(self.key, journal.path.read_text())
            restarted = p.Infrastructure(copy.deepcopy(self.config))
            self.assertEqual(OperationJournal(restarted, self.operation).recover(), completed)

    def test_cached_receipts_cannot_cross_host_even_with_same_operation_uuid(self):
        journal = OperationJournal(self.infra, self.operation)
        receipt = host.attach_receipt(self.operation, {'receipt_id': self.operation['id']})
        receipt['executor_host_binding']['daemon_id'] = 'b' * 64
        journal.legacy_receipt.write_text(json.dumps(receipt))
        with self.assertRaisesRegex(host.HostError, 'receipt_binding_mismatch'):
            journal.recover()
        with self.assertRaisesRegex(host.HostError, 'receipt_binding_mismatch'):
            self.infra.execute(self.operation)
        self.assertTrue(all('info' in call.args[0] for call in self.run.call_args_list))

    def test_operator_state_identity_changes_cannot_retarget_existing_inventory(self):
        self.infra.directory(self.operation['instance_id'])
        changed = copy.deepcopy(self.config)
        changed['executor_host']['operator_state_id'] = str(uuid.uuid4())
        with self.assertRaisesRegex(p.ProvisioningError, 'migration_requires_review'):
            p.Infrastructure(changed).directory(self.operation['instance_id'])
        self.run.assert_not_called()

    def test_new_scoped_binding_does_not_qualify_remote_lifecycle(self):
        for action in ['backup', 'restore', 'upgrade', 'rollback', 'enroll_execution']:
            altered = {**self.operation, 'action': action}
            with self.subTest(action=action), self.assertRaisesRegex(p.ProvisioningError, 'not_qualified'):
                self.infra.validate_host_operation(altered, self.infra.directory(altered['instance_id']))
        self.run.assert_not_called()

    def test_missing_nil_or_extra_operator_state_identity_fails_closed(self):
        for identity in [None, str(uuid.UUID(int=0)), 'AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA']:
            config = copy.deepcopy(self.config)
            if identity is None:
                del config['executor_host']['operator_state_id']
            else:
                config['executor_host']['operator_state_id'] = identity
            with self.subTest(identity=identity), self.assertRaises(host.HostError):
                host.settings(config)

    def test_private_key_envelope_rejects_missing_invalid_or_changed_scope(self):
        self.assertEqual(host.operator_secrets(self.config, self.operation)['fleet_key'], self.key)
        for mutate in [
            lambda op: op.pop('operator_secrets'),
            lambda op: op['operator_secrets'].update(fleet_key='x' * 64),
            lambda op: op['operator_secrets'].update(fleet_key='c' * 63),
            lambda op: op['operator_secrets'].update(global_key='forbidden'),
            lambda op: op['operator_secrets'].update(instance_id=str(uuid.uuid4())),
            lambda op: op['operator_secrets'].update(organisation_id=str(uuid.uuid4())),
            lambda op: op['operator_secrets'].update(operation_id=str(uuid.uuid4())),
            lambda op: op['operator_secrets'].update(lease_token=str(uuid.uuid4())),
            lambda op: op['operator_secrets']['executor_host_binding'].update(operator_state_id=str(uuid.uuid4())),
        ]:
            changed = copy.deepcopy(self.operation)
            mutate(changed)
            with self.subTest(mutate=mutate), self.assertRaises(host.HostError):
                host.operator_secrets(self.config, changed)
        self.run.assert_not_called()

    def test_hosted_runtime_key_uses_envelope_and_never_derives_from_scoped_token(self):
        self.config.update(fleet_management_url='https://manager.example.test', database_endpoint='database.test:5432')
        # No provisioner_token_file exists: a hosted runtime must not read one.
        self.infra._private_operator_secrets = host.operator_secrets(self.config, self.operation)
        directory = self.infra.directory(self.operation['instance_id'])
        values = self.infra.ensure_secrets(self.operation['instance'], directory)
        self.assertEqual(values['fleet_key'], self.key)
        self.assertEqual((directory / 'runtime-secrets' / 'fleet_key').read_text(), self.key)
        self.assertEqual((directory / 'secrets' / 'fleet_key').read_text(), self.key)
        self.assertNotIn('operator_secrets', (directory / 'executor-host.json').read_text())
        self.infra._private_operator_secrets = None
        with self.assertRaisesRegex(p.ProvisioningError, 'operator_key_required'):
            self.infra.ensure_secrets(self.operation['instance'], directory)
        self.run.assert_not_called()

    def test_private_context_is_cleared_after_success_and_failure(self):
        for error in [None, p.ProvisioningError('fixture_only')]:
            with patch.object(self.infra, '_execute_operation', return_value={}, side_effect=error):
                if error is None:
                    self.assertEqual(self.infra.execute(self.operation), {})
                else:
                    with self.assertRaises(p.ProvisioningError):
                        self.infra.execute(self.operation)
            self.assertIsNone(self.infra._private_operator_secrets)
        self.run.assert_not_called()

    def test_enrolled_operator_envelope_is_exactly_bound_to_candidate_epoch(self):
        import execution_generation as epochs
        operation = copy.deepcopy(self.operation)
        source, candidate = str(uuid.uuid4()), str(uuid.uuid4())
        operation['action'] = 'restore'; operation['instance']['execution_required'] = True
        operation['execution_binding'] = {'operation_id':operation['id'], 'organisation_id':self.org,
            'instance_id':operation['instance_id'], 'source_epoch':source, 'candidate_epoch':candidate}
        with self.assertRaises(host.HostError): host.operator_secrets(self.config, operation)
        operation['operator_secrets'].update(version=2,execution_epoch=candidate)
        self.assertEqual(host.operator_secrets(self.config, operation)['execution_epoch'], candidate)
        for mutate in [lambda op:op['operator_secrets'].update(execution_epoch=source),
                       lambda op:op['operator_secrets'].update(version=1),
                       lambda op:op.pop('execution_binding')]:
            changed = copy.deepcopy(operation); mutate(changed)
            with self.assertRaises(host.HostError): host.operator_secrets(self.config, changed)
        self.config.update(fleet_management_url='https://manager.example.test',database_endpoint='database.test:5432')
        self.infra._private_operator_secrets = host.operator_secrets(self.config, operation)
        directory = self.infra.directory(operation['instance_id'])
        with self.assertRaises(p.ProvisioningError): self.infra.ensure_secrets(operation['instance'],directory)
        epochs.write(self.infra, operation['instance'], directory, candidate)
        self.assertEqual(self.infra.ensure_secrets(operation['instance'],directory)['fleet_key'],self.key)
        wrong = copy.deepcopy(self.infra._private_operator_secrets); wrong['execution_epoch'] = source
        self.infra._private_operator_secrets = wrong
        with self.assertRaises(p.ProvisioningError): self.infra.ensure_secrets(operation['instance'],directory)
        self.run.assert_not_called()

    def test_local_fleet_key_rotates_with_actual_execution_marker_and_matches_rust_domain(self):
        import hashlib, hmac
        import execution_generation as epochs
        token = self.root / 'manager-key'; token.write_text('fixture-manager-root-' + 'd' * 64)
        config = {'backend':'compose','state_directory':str(self.root/'local-epoch'),
            'fleet_management_url':'https://manager.example.test','database_endpoint':'database.test:5432',
            'provisioner_token_file':str(token)}
        infra = p.Infrastructure(config)
        instance = {**self.operation['instance'],'executor_host_id':None}
        directory = infra.directory(instance['id'])
        legacy = infra.ensure_secrets(instance,directory)['fleet_key']
        epoch = str(uuid.uuid4()); epochs.write(infra,instance,directory,epoch)
        enrolled = infra.ensure_secrets(instance,directory)['fleet_key']
        expected = hmac.new(token.read_bytes(), ('cowork-federation-v2\0' + self.org + '\0' + instance['id'] + '\0' + epoch).encode(), hashlib.sha256).hexdigest()
        self.assertEqual(enrolled,expected); self.assertNotEqual(enrolled,legacy)
        stage = directory/'generations'/uuid.uuid4().hex; stage.mkdir(parents=True)
        epochs.write(infra,instance,stage,str(uuid.uuid4()))
        self.assertNotEqual(infra.ensure_secrets(instance,stage)['fleet_key'],enrolled)

    @unittest.skipUnless(os.name == 'posix', 'POSIX ownership and private mode required')
    def test_remote_entrypoint_rejects_public_or_local_config(self):
        config = self.root / 'operator.json'
        config.write_text(json.dumps({'backend': 'compose'}))
        config.chmod(0o600)
        with self.assertRaisesRegex(p.ProvisioningError, 'executor_host_required'):
            remote_entrypoint.configuration(config)
        config.chmod(0o644)
        with self.assertRaisesRegex(p.ProvisioningError, 'unsafe_remote_operator_file'):
            remote_entrypoint.configuration(config)


if __name__ == '__main__':
    unittest.main()
