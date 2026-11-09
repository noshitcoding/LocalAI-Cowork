import copy
import json
import os
from pathlib import Path
import tempfile
import threading
import time
from types import SimpleNamespace
import unittest
import uuid
from unittest.mock import Mock, patch
from urllib.error import HTTPError

import execution_generation
import executor_host
import host_capacity_attestation as capacity
import provisioner as p
import resource_budget
from operation_journal import OperationJournal, JournalError


class HostCapacityTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(); self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.org, self.instance_id, self.epoch = [str(uuid.uuid4()) for _ in range(3)]
        self.config = {'backend': 'compose', 'state_directory': str(self.root),
            'host_capacity_attestation': {'version': 1, 'host_revision': 3},
            'executor_host': {'id': str(uuid.uuid4()), 'trust_domain': self.org, 'operator_state_id': str(uuid.uuid4()),
                'endpoint': 'tcp://localhost:2376', 'daemon_id': 'a' * 64, 'ca_file': '/operator/ca',
                'cert_file': '/operator/cert', 'key_file': '/operator/key'},
            'fleet_host_budget': {'cpu_millis': 4000, 'memory_mib': 3584, 'reserved_cpu_millis': 1000,
                                  'reserved_memory_mib': 1024},
            'execution_admission': {'version': 1},
            'images': {name: 'registry.invalid/' + name + '@sha256:' + 'b' * 64 for name in
                       ['server', 'runner', 'sandbox_core', 'sandbox_gui', 'sandbox_crew', 'egress_proxy']},
            's3_endpoint': 'https://storage.invalid', 'fleet_management_url': 'https://manager.invalid',
            'control_network': 'control', 'routing_network': 'routing', 'services_network': 'services'}
        self.instance = {'id': self.instance_id, 'organisation_id': self.org, 'backend': 'compose',
            'executor_host_id': self.config['executor_host']['id'], 'execution_epoch': self.epoch,
            'policy': {'minimum_isolation': 'container', 'limits': {'cpu_millis': 500, 'memory_mib': 512,
                          'storage_gib': 1, 'concurrent_runs': 1}, 'allowed_network_origins': []}}
        self.directory = self.root / uuid.UUID(self.instance_id).hex
        self.infra = SimpleNamespace(root=self.root, config=self.config, private_write=p.private_write,
            active_directory=lambda directory: directory, host=Mock(), directory=lambda key: self.directory,
            validate_host_operation=lambda operation, directory: None)
        initial = resource_budget.instance_budget(self.config, self.instance)
        self.selected = {'version': 1, 'challenge_id': str(uuid.uuid4()), 'expires_at': '2020-01-01T00:00:00Z',
            'executor_host_binding': executor_host.manager_binding(self.config), 'host_revision': 3, 'allocation_revision': 1,
            'capacity': copy.deepcopy(self.config['fleet_host_budget']), 'service_plan': resource_budget.service_caps(self.config),
            'allocations': [{'instance_id': self.instance_id, 'allocation_revision': 1, 'execution_epoch': self.epoch,
                'instance_state': 'pending', 'plan': initial, 'plan_sha256': capacity.digest(initial),
                'expected_resource_budget': initial, 'expected_manifest_sha256': None}]}
        self.info = {'ID': 'a' * 64, 'NCPU': 4, 'MemTotal': 4096 * 1024 * 1024,
            'Labels': ['cowork.role=executor', 'cowork.trust-domain=' + self.org]}

    def applied(self):
        self.directory.mkdir()
        execution_generation.write(self.infra, self.instance, self.directory, self.epoch)
        # Avoid probing runtime image labels; this unit fixture only exercises
        # evidence and actual daemon inventory classification.
        with patch.object(p.execution_runtime, 'environment', return_value={}):
            manifest = p.compose_manifest(self.config, self.instance, self.directory)
        p.private_write(self.directory / 'compose.json', json.dumps(manifest).encode())
        budget = resource_budget.manifest_budget(manifest, self.instance_id)
        p.private_write(self.directory / 'resource-budget.json', json.dumps(budget).encode())
        self.selected['allocations'][0].update(instance_state='running', expected_manifest_sha256=capacity.digest(manifest))
        containers = []
        for service, entry in manifest['services'].items():
            caps = budget['services'][service]
            containers.append({'Id': uuid.uuid4().hex * 2, 'Name': '/cowork-' + uuid.UUID(self.instance_id).hex + '-' + service + '-1',
                'Config': {'Image': entry['image'], 'Env': [key + '=' + str(value) for key, value in entry.get('environment', {}).items()],
                           'Labels': {'com.docker.compose.project': manifest['name'], 'com.docker.compose.service': service}},
                'HostConfig': {'NanoCpus': caps['cpu_millis'] * 1000000, 'Memory': caps['memory_mib'] * 1024 * 1024,
                    'PidsLimit': caps['pids'], 'Privileged': False, 'ReadonlyRootfs': True, 'CapDrop': ['ALL'],
                    'CapAdd': None, 'SecurityOpt': ['no-new-privileges:true']}})
        return manifest, containers

    def operation(self):
        row = self.selected['allocations'][0]
        return {'id': str(uuid.uuid4()), 'organisation_id': self.org, 'instance_id': self.instance_id, 'instance': self.instance,
            'action': 'provision', 'arguments': {}, 'request_digest': 'c' * 64, 'lease_token': str(uuid.uuid4()),
            'executor_host_binding': self.selected['executor_host_binding'],
            'host_allocation': {'binding': {'version': 1, 'host_id': self.config['executor_host']['id'],
                'organisation_id': self.org, 'instance_id': self.instance_id, 'allocation_revision': 1,
                'plan_sha256': row['plan_sha256'], 'execution_epoch': self.epoch}, 'plan': row['plan']}}

    def attestor(self, *, clock=time.monotonic):
        manager = Mock()
        manager.call.side_effect = lambda path, body: copy.deepcopy(self.selected) if path.endswith('challenge') \
            else {'qualified': not body['unknown_workloads'] and body['inventory_complete'],
                  'reason': None if not body['unknown_workloads'] and body['inventory_complete'] else 'host_allocation_unknown'}
        self.infra.run = Mock(side_effect=lambda argv, **kwargs: json.dumps(self.info).encode() if argv[1] == 'info' else b'')
        return capacity.CapacityAttestor(manager, self.infra, clock=clock)

    def test_opt_in_requires_host_execution_and_exact_typed_revision(self):
        self.assertEqual(capacity.settings({}), None)
        for changed in [{'host_capacity_attestation': False}, {'host_capacity_attestation': {'version': 1, 'host_revision': True}},
                        {'executor_host': None}, {'execution_admission': None}]:
            with self.subTest(changed=changed), self.assertRaises((capacity.CapacityError, executor_host.HostError)):
                capacity.settings({**self.config, **changed})

    def test_challenge_requires_exact_host_plan_revision_and_unique_ledger(self):
        self.assertEqual(capacity.challenge(self.selected, self.config), self.selected)
        altered = []
        for field, value in [('host_revision', 4), ('service_plan', {}), ('capacity', {}), ('allocation_revision', True)]:
            changed = copy.deepcopy(self.selected); changed[field] = value; altered.append(changed)
        changed = copy.deepcopy(self.selected); changed['allocations'] *= 2; altered.append(changed)
        changed = copy.deepcopy(self.selected); changed['executor_host_binding']['host_id'] = str(uuid.uuid4()); altered.append(changed)
        changed = copy.deepcopy(self.selected); changed['allocations'][0]['plan']['total']['cpu_millis'] -= 1; altered.append(changed)
        for changed in altered:
            with self.assertRaises(capacity.CapacityError): capacity.challenge(changed, self.config)

    def test_reserved_is_observed_absence_and_uncommitted_manifest_is_unknown(self):
        observed = capacity.inventory(self.infra, self.selected, self.info, [])
        self.assertEqual(observed['allocations'][0]['state'], 'reserved')
        self.assertFalse(observed['unknown_workloads'])
        self.directory.mkdir(); (self.directory / 'compose.json').write_text('{}')
        observed = capacity.inventory(self.infra, self.selected, self.info, [])
        self.assertEqual(observed['allocations'][0]['state'], 'unknown')
        self.assertTrue(observed['unknown_workloads'])

    def test_applied_requires_actual_service_limits_environment_epoch_and_manifest(self):
        _, containers = self.applied()
        observed = capacity.inventory(self.infra, self.selected, self.info, containers)
        self.assertFalse(observed['unknown_workloads'])
        self.assertEqual(observed['allocations'][0]['state'], 'applied')
        for field, value in [('NanoCpus', 0), ('Memory', 4096 * 1024 * 1024), ('PidsLimit', 0), ('Privileged', True)]:
            changed = copy.deepcopy(containers); changed[0]['HostConfig'][field] = value
            self.assertTrue(capacity.inventory(self.infra, self.selected, self.info, changed)['unknown_workloads'])
        changed = copy.deepcopy(containers); changed[0]['Config']['Env'].append('COWORK_EXECUTION_EPOCH=' + str(uuid.uuid4()))
        self.assertTrue(capacity.inventory(self.infra, self.selected, self.info, changed)['unknown_workloads'])
        marker = json.loads((self.directory / 'execution.json').read_text()); marker['epoch'] = str(uuid.uuid4())
        p.private_write(self.directory / 'execution.json', json.dumps(marker).encode())
        self.assertTrue(capacity.inventory(self.infra, self.selected, self.info, containers)['unknown_workloads'])

    def test_unknown_workload_other_operator_and_hidden_service_fail_closed(self):
        _, containers = self.applied()
        for suffix in ['api-2', 'hidden-1', 'snapshot-' + uuid.uuid4().hex]:
            changed = copy.deepcopy(containers)
            changed[0]['Name'] = '/cowork-' + uuid.UUID(self.instance_id).hex + '-' + suffix
            self.assertTrue(capacity.inventory(self.infra, self.selected, self.info, changed)['unknown_workloads'])
        foreign = {'Id': 'd' * 64, 'Name': '/unrelated-container'}
        self.assertTrue(capacity.inventory(self.infra, self.selected, self.info, containers + [foreign])['unknown_workloads'])
        (self.root / uuid.uuid4().hex).mkdir()
        self.assertTrue(capacity.inventory(self.infra, self.selected, self.info, containers)['unknown_workloads'])

    def test_runtime_pool_is_shared_and_stopped_allocations_remain_held(self):
        _, containers = self.applied()
        runtime = copy.deepcopy(containers[0]); runtime['Id'] = 'e' * 64
        runtime['Name'] = '/cowork-' + uuid.UUID(self.instance_id).hex + '-job-' + uuid.uuid4().hex
        runtime['Config']['Labels'] = {'dev.opencowork.admission': str(uuid.uuid4()), 'dev.opencowork.run_id': str(uuid.uuid4())}
        runtime['HostConfig'].update(NanoCpus=500000000, Memory=512 * 1024 * 1024, MemorySwap=512 * 1024 * 1024)
        self.assertFalse(capacity.inventory(self.infra, self.selected, self.info, containers + [runtime])['unknown_workloads'])
        second = copy.deepcopy(runtime); second['Id'] = 'f' * 64; second['Name'] = runtime['Name'][:-32] + uuid.uuid4().hex
        self.assertTrue(capacity.inventory(self.infra, self.selected, self.info, containers + [runtime, second])['unknown_workloads'])
        self.selected['allocations'][0]['instance_state'] = 'stopped'
        observed = capacity.inventory(self.infra, self.selected, self.info, [])
        self.assertEqual(len(observed['allocations']), 1)
        self.assertEqual(observed['allocations'][0]['state'], 'applied')

    def test_daemon_identity_and_measured_memory_cannot_be_reflected_from_registry(self):
        changed = copy.deepcopy(self.info); changed['ID'] = 'd' * 64
        with self.assertRaises(capacity.CapacityError): capacity.inventory(self.infra, self.selected, changed, [])
        observed = capacity.inventory(self.infra, self.selected, {**self.info, 'NCPU': 2, 'MemTotal': 3333 * 1024 * 1024 + 999}, [])
        self.assertEqual(observed['observed_capacity'], {'cpu_millis': 2000, 'memory_mib': 3333})

    def test_renewal_uses_new_challenge_and_db_clock_even_under_wall_clock_skew(self):
        attestor = self.attestor()
        with patch.object(capacity, 'datetime', wraps=capacity.datetime), patch.object(time, 'time', return_value=1e15):
            self.assertTrue(attestor.refresh())
            first = attestor.manager.call.call_args.args[1]
            self.selected['challenge_id'] = str(uuid.uuid4())
            self.assertTrue(attestor.refresh())
        second = attestor.manager.call.call_args.args[1]
        self.assertNotEqual(first['challenge_id'], second['challenge_id'])
        self.assertEqual(second['allocations'][0]['state'], 'reserved')
        status = json.loads(attestor.path.read_text())
        self.assertTrue(status['qualified']); self.assertNotIn('expires_at', status)

    def test_http404_or_collection_deadline_never_falls_back_to_legacy(self):
        attestor = self.attestor(); attestor.manager.call.side_effect = HTTPError('https://own.invalid', 404, 'missing', {}, None)
        with self.assertRaisesRegex(capacity.CapacityError, 'attestation_failed'): attestor.refresh()
        self.assertFalse(attestor.qualified)
        self.assertFalse(json.loads(attestor.path.read_text())['qualified'])
        attestor = self.attestor(clock=Mock(side_effect=[0, 26]))
        with self.assertRaises(capacity.CapacityError): attestor.refresh()
        self.assertEqual(attestor.manager.call.call_count, 1)

    def test_inventory_change_reports_uncertainty_without_releasing_any_allocation(self):
        attestor = self.attestor()
        attestor.infra.run = Mock(side_effect=[json.dumps(self.info).encode(), b'', ('d' * 64).encode()])
        self.assertFalse(attestor.refresh())
        reported = attestor.manager.call.call_args.args[1]
        self.assertFalse(reported['inventory_complete']); self.assertTrue(reported['unknown_workloads'])
        self.assertEqual(len(reported['allocations']), 1)
        self.assertNotIn('released', json.dumps(reported))

    def test_background_renewal_continues_while_operation_is_blocked(self):
        attestor = self.attestor(); attestor.refresh_seconds = .01
        try:
            attestor.start()
            deadline = time.monotonic() + 1
            while attestor.manager.call.call_count < 6 and time.monotonic() < deadline:
                threading.Event().wait(.01)
            self.assertGreaterEqual(attestor.manager.call.call_count, 6)
        finally:
            attestor.close()
        self.assertFalse(attestor.thread.is_alive())

    def test_disable_or_revision_change_requires_operator_review(self):
        attestor = self.attestor(); attestor.bind()
        changed = copy.deepcopy(self.config); changed['host_capacity_attestation'] = None
        with self.assertRaisesRegex(capacity.CapacityError, 'disable_requires_review'):
            capacity.CapacityAttestor(Mock(), SimpleNamespace(**{**vars(self.infra), 'config': changed})).bind()
        changed['host_capacity_attestation'] = {'version': 1, 'host_revision': 4}
        with self.assertRaisesRegex(capacity.CapacityError, 'configuration_changed'):
            capacity.CapacityAttestor(Mock(), SimpleNamespace(**{**vars(self.infra), 'config': changed})).bind()

    def test_journal_receipt_allocation_fence_preserves_exact_completion(self):
        self.directory.mkdir()
        operation = self.operation()
        journal = OperationJournal(self.infra, operation)
        complete = journal.finish(True, {'receipt_id': operation['id']})
        self.assertEqual(complete['result']['host_allocation'], operation['host_allocation']['binding'])
        self.assertEqual(OperationJournal(self.infra, operation).recover(), complete)
        changed = copy.deepcopy(operation); changed['host_allocation']['binding']['allocation_revision'] += 1
        with self.assertRaisesRegex(JournalError, 'binding_mismatch'): OperationJournal(self.infra, changed).recover()
        for key, value in [('host_id', str(uuid.uuid4())), ('execution_epoch', str(uuid.uuid4())), ('plan_sha256', 'd' * 64)]:
            changed = copy.deepcopy(operation); changed['host_allocation']['binding'][key] = value
            with self.assertRaises(capacity.CapacityError): capacity.operation_allocation(changed)

    def test_unqualified_cleanup_completion_allowed_but_activation_rejected(self):
        attestor = self.attestor()
        operation = self.operation()
        self.directory.mkdir(); (self.directory / 'compose.json').write_text('{}')
        attestor.validate_operation(operation)  # Receipt recovery is structural.
        with self.assertRaisesRegex(capacity.CapacityError, 'unqualified'):
            attestor.validate_operation(operation, activating=True)
        operation['action'] = 'delete'
        attestor.validate_operation(operation, activating=True)


if __name__ == '__main__':
    unittest.main()
