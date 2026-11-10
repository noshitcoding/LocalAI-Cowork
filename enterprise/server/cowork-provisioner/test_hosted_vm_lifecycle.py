from contextlib import contextmanager
import copy
import hashlib
import json
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch
import uuid

import hosted_vm_transport as transport
import qualify_hosted_vm as harness
from hosted_vm_lifecycle_checks import Context
from lifecycle_schema_delta import LifecycleSchemaDelta
from test_lifecycle_schema_delta import FixtureSql


def identity():
    return str(uuid.uuid4())


class HostedLifecycleTests(unittest.TestCase):
    def fixture(self):
        instance, organisation, host, source, target = [identity() for _ in range(5)]
        binding = {'version': 1, 'host_id': host, 'trust_domain': organisation,
                   'daemon_id': 'own-engine', 'operator_state_id': identity()}
        operation = {'id': identity(), 'action': 'restore', 'instance_id': instance,
            'organisation_id': organisation, 'instance': {'id': instance, 'organisation_id': organisation},
            'executor_host_binding': binding, 'execution_binding': {'instance_id': instance,
                'organisation_id': organisation, 'source_epoch': source, 'candidate_epoch': target},
            'host_allocation': {'binding': {'version': 1, 'host_id': host, 'organisation_id': organisation,
                'instance_id': instance, 'allocation_revision': 3, 'plan_sha256': 'a' * 64,
                'execution_epoch': source}}}
        operation['execution_binding']['operation_id'] = operation['id']
        return instance, binding, operation

    def test_generation_transport_extends_only_exact_scoped_frozen_claim(self):
        instance, binding, operation = self.fixture()
        scope = transport.GenerationScope(instance, binding)
        old = scope.databases(); scope.allow_operation(operation)
        suffix = hashlib.sha256((instance + ':' + operation['id']).encode()).hexdigest()[:32]
        self.assertEqual(scope.databases(), old | {'ci_' + suffix})
        self.assertTrue(transport.authorized('s3', 'PUT', '/cowork-' + suffix + '/own-object', generations=scope))
        self.assertFalse(transport.authorized('s3', 'PUT', '/foreign/object', generations=scope))
        self.assertFalse(transport.authorized('s3', 'GET', '/cowork-' + suffix + '/%2e%2e/foreign', generations=scope))
        self.assertFalse(transport.authorized('s3', 'GET', '/cowork-' + suffix + '/a%5cb', generations=scope))
        self.assertEqual(len(scope.evidence()), 2)

    def test_generation_scope_refuses_changed_epoch_host_and_allocation(self):
        instance, binding, operation = self.fixture()
        for mutate in [lambda row: row['executor_host_binding'].update(host_id=identity()),
                lambda row: row['execution_binding'].update(candidate_epoch=row['execution_binding']['source_epoch']),
                lambda row: row['host_allocation']['binding'].update(execution_epoch=identity()),
                lambda row: row['host_allocation']['binding'].update(allocation_revision=True),
                lambda row: row['host_allocation']['binding'].update(plan_sha256='mutable'),
                lambda row: row['instance'].update(id=identity())]:
            changed = copy.deepcopy(operation); mutate(changed)
            scope = transport.GenerationScope(instance, binding)
            with self.assertRaises(RuntimeError): scope.allow_operation(changed)
            self.assertEqual(len(scope.evidence()), 1)

    def test_generation_allowlist_is_bounded_and_nontransition_cannot_extend(self):
        instance, binding, operation = self.fixture(); scope = transport.GenerationScope(instance, binding)
        operation['action'] = 'backup'; scope.allow_operation(operation)
        self.assertEqual(len(scope.evidence()), 1)
        operation['action'] = 'upgrade'
        for _ in range(15):
            operation['id'] = identity(); operation['execution_binding']['operation_id'] = operation['id']
            scope.allow_operation(operation)
        operation['id'] = identity(); operation['execution_binding']['operation_id'] = operation['id']
        with self.assertRaisesRegex(RuntimeError, 'inventory_exceeds_bound'): scope.allow_operation(operation)

    def test_profile_requires_private_ca_capable_source_different_binary_and_explicit_pool(self):
        args = SimpleNamespace(qualification_profile='host55-lifecycle', schema_version=57, source_schema_version=54,
            source_server_image_id='sha256:' + 'a' * 64, server_image_id='sha256:' + 'b' * 64,
            source_runner_image_id='sha256:' + 'c' * 64, parent_capacity_plan_file=Path.cwd() / 'private/plan',
            retained_pilot_root=Path.cwd() / 'private/pilot', retained_pilot_vm_id=identity(), retained_pilot_known_stopped=True,
            expected_parent_capacity_revision=0)
        harness.validate_profile_arguments(args)
        for key, value in [('source_schema_version', 52), ('schema_version', 55),
                ('source_server_image_id', args.server_image_id), ('source_runner_image_id', None),
                ('retained_pilot_known_stopped', False), ('expected_parent_capacity_revision', True)]:
            changed = copy.deepcopy(args); setattr(changed, key, value)
            with self.assertRaises(RuntimeError): harness.validate_profile_arguments(changed)

    def test_schema59_normalization_is_wired_to_scoped_historical_query(self):
        ctx = object.__new__(Context); ctx.source_schema = 54; ctx.target_schema = 59
        ctx.schema_delta = LifecycleSchemaDelta(54, 59)
        sql = FixtureSql(54); ctx._sql = sql; ctx.schema = lambda: sql.current
        query = "SELECT jsonb_agg(to_jsonb(r) ORDER BY id) FROM runs r WHERE id IN ('" + identity() + "');"
        baseline = json.loads(ctx.sql(query)); sql.current = 59
        sql.rows[0].update(runner_cancel_id=None, runner_cancel_lease=None,
            runner_stop_receipt=None, server_resume_checkpoint_id=None)
        self.assertEqual(json.loads(ctx.sql(query)), baseline)
        sql.rows[0]['server_resume_checkpoint_id'] = identity()
        with self.assertRaisesRegex(RuntimeError, 'historical_runs_migration_state_changed'): ctx.sql(query)
        sql.rows[0]['server_resume_checkpoint_id'] = None
        sql.count_overrides['run_resume_requests'] = 1
        with self.assertRaisesRegex(RuntimeError, 'new_lifecycle_migration_table_not_empty'): ctx.sql(query)

    def test_epoch_fence_checks_actual_markers_before_request_and_never_reports_key(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); (root / 'secrets').mkdir()
            source, epoch, user, instance, organisation = [identity() for _ in range(5)]
            (root / 'execution.json').write_text(json.dumps({'epoch': epoch}))
            (root / 'secrets/fleet_key').write_text('c' * 64)
            manifest = {'services': {name: {'environment': {'COWORK_EXECUTION_EPOCH': epoch}} for name in ['api', 'worker']}}
            (root / 'compose.json').write_text(json.dumps(manifest))
            ctx = object.__new__(Context); ctx.active = lambda: root; ctx.directory = root
            ctx.current = lambda: {'id': instance, 'organisation_id': organisation, 'execution_epoch': epoch}
            ctx.management_opener = object(); ctx.runtime_management_origin = 'https://localhost:1234'
            ctx.request_callback = Mock(); ctx.report = {}
            receipt = {'id': identity(), 'result': {'execution': {'active_epoch': epoch, 'binding': {'source_epoch': source}}}}
            ctx.assert_epoch_fence(receipt, user)
            self.assertEqual(ctx.request_callback.call_args.kwargs['expected'], 403)
            self.assertEqual(ctx.request_callback.call_args.kwargs['body']['execution_epoch'], source)
            self.assertNotIn('c' * 64, json.dumps(ctx.report))
            ctx.request_callback.reset_mock(); manifest['services']['worker']['environment']['COWORK_EXECUTION_EPOCH'] = source
            (root / 'compose.json').write_text(json.dumps(manifest))
            with self.assertRaisesRegex(RuntimeError, 'runtime_epoch_mismatch'): ctx.assert_epoch_fence(receipt, user)
            ctx.request_callback.assert_not_called()

    @unittest.skipUnless(os.name == 'posix', 'POSIX metadata ownership')
    def test_pilot_adoption_reads_only_metadata_and_leaves_its_hash_unchanged(self):
        with tempfile.TemporaryDirectory() as directory:
            parent = Path(directory); pilot_id = identity()
            pilot = parent / ('cowork-executor-' + uuid.UUID(pilot_id).hex); pilot.mkdir(mode=0o700)
            metadata = pilot / 'vm.json'; metadata.write_text(json.dumps({'id': pilot_id})); metadata.chmod(0o600)
            secret = pilot / 'ssh-key'; secret.write_text('must not read'); secret.chmod(0o600)
            plan = {'pool_id': identity(), 'existing_vm_roots': [str(pilot)]}
            pool = Mock(); pool.data = None
            def configure(value, revision): pool.data = {'revision': 1, 'plan': value}
            pool.configure.side_effect = configure; pool.totals.return_value = {'cpu_millis': 4500, 'memory_mib': 4608}
            @contextmanager
            def locked(**kwargs): yield pool
            @contextmanager
            def inventory(value):
                self.assertEqual(value, parent); yield
            capacity = SimpleNamespace(read_private_json=Mock(return_value=plan), validate_plan=Mock(),
                canonical_path=Path, locked_pool=locked, ledger_digest=lambda value: 'a' * 64)
            vm = SimpleNamespace(capacity_pool=capacity, private_directory=Mock(), private_file=Mock(),
                load=Mock(return_value={'id': pilot_id}), inventory_lock=inventory, start=Mock(), ssh=Mock())
            args = SimpleNamespace(parent_capacity_plan_file=parent / 'plan.json', retained_pilot_root=pilot,
                retained_pilot_vm_id=pilot_id, expected_parent_capacity_revision=0)
            before = metadata.read_bytes(); report = {}
            harness.prepare_parent_pool(vm, args, report)
            self.assertEqual(metadata.read_bytes(), before)
            vm.private_file.assert_called_once_with(metadata, 'unsafe_retained_pilot_metadata')
            vm.load.assert_called_once_with(pilot, allow_preparing=False)
            pool.reserve.assert_called_once_with(pilot, {'id': pilot_id}, adopted=True)
            vm.start.assert_not_called(); vm.ssh.assert_not_called()
            self.assertFalse(report['parent_capacity']['pilot_started'])


if __name__ == '__main__': unittest.main()
