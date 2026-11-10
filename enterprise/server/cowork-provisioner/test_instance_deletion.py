import copy
import json
from pathlib import Path
import re
import tempfile
import unittest
import uuid

import provisioner as p
from instance_deletion import InstanceDeletion, DeletionError


class DeletionSecurityTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        root = Path(self.temp.name)
        (root / 'database-url').write_text('postgres://operator:private@database:5432/postgres')
        self.config = {'state_directory': str(root / 'instances'), 'backup_directory': str(root / 'backups'),
                       'database_admin_url_file': str(root / 'database-url'), 'database_endpoint': 'database:5432',
                       's3_endpoint': 'http://storage:9000', 's3_admin_alias': 'fleet', 's3_admin_config_directory': str(root / 'mc'), 'deletion_enabled': True}
        self.infra = p.Infrastructure(self.config)
        self.instance = {'id': str(uuid.uuid4()), 'organisation_id': str(uuid.uuid4()), 'name': 'Disposable', 'backend': 'compose', 'state': 'stopped', 'policy': {'minimum_isolation': 'container'}}
        self.directory = self.infra.directory(self.instance['id'])
        self.scope = 'cowork-' + uuid.UUID(self.instance['id']).hex
        self.backup = str(uuid.uuid4())
        self.operation = {'id': str(uuid.uuid4()), 'instance': self.instance, 'arguments': {'confirmation_name': 'Disposable', 'purge_backups': True, 'delete_backup_ids': [self.backup]}}
        self.databases, self.roles, self.users = {}, set(), {}
        self.buckets, self.policies, self.volumes = set(), set(), set()
        self.container = {'Id': 'container-id', 'Name': '/' + self.scope + '-api-1', 'Config': {'Labels': {'com.docker.compose.project': self.scope}}}
        self.containers = [self.container]
        self.networks = [{'Id': 'network-id', 'Name': self.scope + '-sandbox', 'Labels': {'com.docker.compose.project': self.scope}}]
        self.calls = []
        self.fail_bucket_removal = False
        self.infra.run = self.run_command
        self.infra.stop_compose = lambda *args: None
        self.manager = InstanceDeletion(self.infra)
        for index, generation in enumerate([None, str(uuid.uuid4())]):
            directory = self.directory if generation is None else self.directory / 'generations' / uuid.UUID(generation).hex
            directory.mkdir(parents=True, exist_ok=True)
            if generation:
                (directory / 'generation.json').write_text(json.dumps({'generation': generation}))
            (directory / 'secrets').mkdir()
            key = str(index + 1) * 64
            (directory / 'secrets' / 's3_access_key').write_text(key)
            (directory / 'secrets' / 'storage_master_key').write_text('instance-encryption-key')
            resource = self.infra.resource_names(self.instance, directory)
            self.databases[resource['database']] = resource['database']
            self.roles.add(resource['database'])
            self.users[key] = resource['bucket']
            self.buckets.add(resource['bucket'])
            self.policies.add(resource['bucket'])
            self.volumes.add(resource['volume_prefix'] + uuid.uuid4().hex)
        (self.directory / 'compose.json').write_text(json.dumps({'name': self.scope, 'services': {'api': {'environment': {'COWORK_INSTANCE_ID': self.instance['id'], 'COWORK_ORGANISATION_ID': self.instance['organisation_id']}}}}))
        (self.manager.root / (uuid.UUID(self.backup).hex + '.age')).write_bytes(b'encrypted fixture')
        (self.manager.root / (uuid.UUID(self.backup).hex + '.json')).write_text(json.dumps({'backup_id': self.backup, 'instance_id': self.instance['id'], 'organisation_id': self.instance['organisation_id']}))
        self.foreign = self.manager.root / (uuid.uuid4().hex + '.age')
        self.foreign.write_bytes(b'foreign encrypted fixture')

    def run_command(self, args, **kwargs):
        self.calls.append(args)
        if args[0] == 'docker':
            kind = args[1]
            if kind == 'ps': return '\n'.join(c['Name'].lstrip('/') for c in self.containers).encode()
            if kind == 'rm': self.containers = []; return b''
            if args[2] == 'ls': return '\n'.join(self.volumes if kind == 'volume' else [n['Name'] for n in self.networks]).encode()
            if args[2] == 'inspect':
                return json.dumps(self.containers if kind == 'container' else [{'Name': n, 'Labels': {}} for n in self.volumes] if kind == 'volume' else self.networks).encode()
            if args[2] == 'rm':
                if kind == 'volume': self.volumes.clear()
                else: self.networks.clear()
                return b''
        if args[0] == 'psql':
            query = args[-1]
            db = re.search(r'ci_[a-f0-9]{32}', query).group()
            if query.startswith('SELECT pg_get_userbyid'): return self.databases.get(db, '').encode()
            if query.startswith('SELECT 1'):
                exists = db in self.roles if 'UNION' not in query else db in self.roles or db in self.databases
                return b'1' if exists else b''
            if query.startswith('DROP DATABASE'): self.databases.pop(db, None)
            if query.startswith('DROP ROLE'): self.roles.discard(db)
            return b''
        if args[0] == 'mc':
            command = args[4:]
            records = []
            if command[:3] == ['admin', 'user', 'list']: records = [{'accessKey': user, 'policyName': policy} for user, policy in self.users.items()]
            elif command[:3] == ['admin', 'policy', 'list']: records = [{'policy': policy} for policy in self.policies]
            elif command[0] == 'ls': records = [{'key': bucket + '/'} for bucket in self.buckets]
            elif command[:3] == ['admin', 'user', 'remove']: self.users.pop(command[-1])
            elif command[:3] == ['admin', 'user', 'disable']: pass
            elif command[:3] == ['admin', 'policy', 'remove']: self.policies.remove(command[-1])
            elif command[0] == 'rb':
                if self.fail_bucket_removal: raise p.ProvisioningError('fixture_storage_unavailable')
                self.buckets.remove(command[-1].split('/')[-1])
            else: raise AssertionError('unexpected storage command')
            return '\n'.join(json.dumps({'status': 'success', **row}) for row in records).encode()
        raise AssertionError('unexpected infrastructure command')

    def test_permanent_deletion_covers_all_generations_and_leaves_foreign_archive(self):
        receipt = self.manager.execute(self.operation, self.directory)
        self.assertTrue(receipt['deletion_verified'])
        self.assertEqual(receipt['deleted_backup_count'], 1)
        self.assertEqual(self.databases, {})
        self.assertFalse(self.roles or self.users or self.buckets or self.policies or self.volumes or self.containers or self.networks)
        self.assertFalse((self.directory / 'secrets').exists())
        self.assertFalse((self.directory / 'generations').exists())
        self.assertEqual(self.foreign.read_bytes(), b'foreign encrypted fixture')
        self.assertEqual({p.name for p in self.directory.iterdir()}, {'deleted.json', 'deletion-' + uuid.UUID(self.operation['id']).hex + '.json'})

    def test_partial_deletion_requires_explicit_recovery_and_preserves_inventory(self):
        self.fail_bucket_removal = True
        with self.assertRaises(p.ProvisioningError): self.manager.execute(self.operation, self.directory)
        self.assertFalse((self.directory / 'deleted.json').exists())
        self.assertEqual(self.databases, {})
        self.assertTrue(self.buckets)
        with self.assertRaisesRegex(DeletionError, 'explicit_recovery'): self.manager.execute(self.operation, self.directory)
        self.fail_bucket_removal = False
        receipt = self.manager.execute(self.operation, self.directory, resume=True)
        self.assertTrue(receipt['deletion_verified'])

    def test_foreign_database_owner_blocks_before_any_removal(self):
        self.databases[next(iter(self.databases))] = 'different_organisation'
        with self.assertRaisesRegex(DeletionError, 'foreign_deletion_database_owner'): self.manager.execute(self.operation, self.directory)
        self.assertFalse(any(command[0] == 'psql' and command[-1].startswith(('DROP', 'ALTER')) for command in self.calls))
        self.assertEqual(len(self.containers), 1)

    def test_foreign_docker_project_blocks_before_any_removal(self):
        self.container['Config']['Labels']['com.docker.compose.project'] = 'another-instance'
        with self.assertRaisesRegex(DeletionError, 'foreign_deletion_container'): self.manager.execute(self.operation, self.directory)
        self.assertEqual(len(self.databases), 2)

    def test_unattributed_backup_staging_blocks_secret_cleanup(self):
        (self.manager.root / 'capture-unattributed').mkdir()
        with self.assertRaisesRegex(DeletionError, 'unattributed_backup_staging'): self.manager.execute(self.operation, self.directory)
        self.assertTrue((self.directory / 'secrets' / 'storage_master_key').exists())

    def test_modified_scope_and_missing_purge_acknowledgement_are_rejected(self):
        for extra in [{'confirmation_name': 'another'}, {'purge_backups': False}]:
            operation = copy.deepcopy(self.operation)
            operation['arguments'].update(extra)
            with self.assertRaisesRegex(DeletionError, 'acknowledgement'): self.manager.execute(operation, self.directory)
        self.assertEqual(self.calls, [])

    def test_frozen_inventory_cannot_delete_a_foreign_database_on_recovery(self):
        self.fail_bucket_removal = True
        with self.assertRaises(p.ProvisioningError): self.manager.execute(self.operation, self.directory)
        path = self.directory / ('deletion-' + uuid.UUID(self.operation['id']).hex + '.json')
        value = json.loads(path.read_text())
        value['resources'][0]['database'] = 'postgres'
        path.write_text(json.dumps(value))
        self.calls.clear()
        with self.assertRaisesRegex(DeletionError, 'resource_changed'): self.manager.execute(self.operation, self.directory, resume=True)
        self.assertEqual(self.calls, [])

    def test_recovery_revalidates_backup_ownership_before_more_removal(self):
        self.fail_bucket_removal = True
        with self.assertRaises(p.ProvisioningError): self.manager.execute(self.operation, self.directory)
        metadata = self.manager.root / (uuid.UUID(self.backup).hex + '.json')
        value = json.loads(metadata.read_text())
        value['organisation_id'] = str(uuid.uuid4())
        metadata.write_text(json.dumps(value))
        self.calls.clear()
        with self.assertRaisesRegex(DeletionError, 'backup_scope_mismatch'): self.manager.execute(self.operation, self.directory, resume=True)
        self.assertEqual(self.calls, [])

    def test_backup_root_cannot_overlap_instance_state(self):
        self.infra.config['backup_directory'] = str(self.infra.root / 'backups')
        with self.assertRaisesRegex(DeletionError, 'overlapping_deletion_roots'): InstanceDeletion(self.infra)


if __name__ == '__main__':
    unittest.main()
