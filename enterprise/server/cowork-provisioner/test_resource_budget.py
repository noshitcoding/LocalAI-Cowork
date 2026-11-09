import copy
import json
from pathlib import Path
import tempfile
import unittest
import uuid

import provisioner as p
import resource_budget as budget


class ResourceBudgetTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        image = 'registry.test/cowork@sha256:' + 'a' * 64
        self.config = {'backend': 'compose', 'state_directory': self.temp.name,
                       'images': {key: image for key in ['server', 'runner', 'sandbox_core', 'sandbox_gui', 'sandbox_crew', 'egress_proxy']},
                       's3_endpoint': 'https://storage.test', 'services_network': 'services', 'routing_network': 'routing'}
        self.instance = {'id': str(uuid.uuid4()), 'organisation_id': str(uuid.uuid4()), 'backend': 'compose',
                         'policy': {'minimum_isolation': 'container', 'limits': {'cpu_millis': 2000,
                             'memory_mib': 4096, 'storage_gib': 10, 'concurrent_runs': 2}}}
        self.infra = p.Infrastructure(self.config)
        self.directory = self.infra.directory(self.instance['id'])

    def manifest(self, instance=None, directory=None):
        return p.compose_manifest(self.config, instance or self.instance, directory or self.directory)

    def host(self, cpu=8500, memory=11008):
        self.config['fleet_host_budget'] = {'cpu_millis': cpu + 1000, 'memory_mib': memory + 512,
                                          'reserved_cpu_millis': 1000, 'reserved_memory_mib': 512}

    def test_all_services_bounded_and_worker_does_not_duplicate_sandbox_policy(self):
        manifest = self.manifest()
        for service in manifest['services'].values():
            self.assertGreater(service['cpus'], 0)
            self.assertGreater(service['pids_limit'], 0)
            self.assertRegex(service['mem_limit'], r'^[0-9]+m$')
        self.assertEqual(manifest['services']['worker']['mem_limit'], '512m')
        self.assertEqual(manifest['services']['worker']['cpus'], 0.5)
        self.assertEqual(manifest['services']['runner']['environment']['COWORK_RUNNER_MEMORY_MIB'], '4096')
        measured = budget.manifest_budget(manifest, self.instance['id'])
        self.assertEqual(measured['total'], {'cpu_millis': 4250, 'memory_mib': 5504})
        self.assertEqual(measured, budget.instance_budget(self.config, self.instance))

    def test_strict_caps_reject_unbounded_bool_missing_and_unknown_values(self):
        for overrides in [{'worker': {'cpu_millis': True, 'memory_mib': 512, 'pids': 256}},
                          {'runner': {'cpu_millis': 100, 'memory_mib': 512, 'pids': -1}},
                          {'api': {'cpu_millis': 100, 'memory_mib': 512}}, {'hidden-worker': {}}]:
            with self.subTest(overrides=overrides), self.assertRaises(budget.BudgetError):
                budget.service_caps({'instance_service_limits': overrides})

    def test_two_instances_fill_host_and_third_rejected_without_reservation(self):
        self.host()
        for _ in range(2):
            item = copy.deepcopy(self.instance)
            item['id'] = str(uuid.uuid4())
            directory = self.infra.directory(item['id'])
            value = self.manifest(item, directory)
            budget.reserve(self.infra, item, directory, value)
            p.private_write(directory / 'compose.json', json.dumps(value).encode())
        with self.assertRaisesRegex(budget.BudgetError, 'capacity_exhausted'):
            budget.reserve(self.infra, self.instance, self.directory, self.manifest())
        self.assertFalse((self.directory / 'resource-budget.json').exists())

    def test_stopped_instances_and_crash_before_manifest_keep_capacity(self):
        self.host(cpu=4250, memory=5504)
        budget.reserve(self.infra, self.instance, self.directory, self.manifest())
        # No compose file exists yet: the reservation is durable before start.
        other = copy.deepcopy(self.instance)
        other['id'] = str(uuid.uuid4())
        other['state'] = 'stopped'
        target = self.infra.directory(other['id'])
        with self.assertRaisesRegex(budget.BudgetError, 'capacity_exhausted'):
            budget.reserve(self.infra, other, target, self.manifest(other, target))

    def test_legacy_current_or_other_manifest_requires_explicit_review(self):
        self.host()
        p.private_write(self.directory / 'compose.json', json.dumps(self.manifest()).encode())
        with self.assertRaisesRegex(budget.BudgetError, 'legacy_resource_budget'):
            budget.reserve(self.infra, self.instance, self.directory, self.manifest())
        other = copy.deepcopy(self.instance)
        other['id'] = str(uuid.uuid4())
        target = self.infra.directory(other['id'])
        with self.assertRaisesRegex(budget.BudgetError, 'legacy_resource_budget'):
            budget.reserve(self.infra, other, target, self.manifest(other, target))

    def test_manifest_tampering_hidden_services_and_replicas_fail_closed(self):
        self.host()
        value = self.manifest()
        budget.reserve(self.infra, self.instance, self.directory, value)
        p.private_write(self.directory / 'compose.json', json.dumps(value).encode())
        for mutation in ['extra', 'replicas', 'unbounded']:
            changed = copy.deepcopy(value)
            if mutation == 'extra':
                changed['services']['another-worker'] = copy.deepcopy(changed['services']['worker'])
            elif mutation == 'replicas':
                changed['services']['worker']['deploy'] = {'replicas': 2}
            else:
                del changed['services']['runner']['mem_limit']
            with self.subTest(mutation=mutation), self.assertRaises(budget.BudgetError):
                budget.manifest_budget(changed, self.instance['id'])
        changed = copy.deepcopy(value)
        changed['services']['worker']['mem_limit'] = '1024m'
        p.private_write(self.directory / 'compose.json', json.dumps(changed).encode())
        with self.assertRaisesRegex(budget.BudgetError, 'manifest_changed'):
            budget.reserve(self.infra, self.instance, self.directory, value)

    def test_delete_tombstone_cannot_hide_retained_allocation(self):
        self.host()
        budget.reserve(self.infra, self.instance, self.directory, self.manifest())
        p.private_write(self.directory / 'deleted.json', b'{}')
        other = copy.deepcopy(self.instance)
        other['id'] = str(uuid.uuid4())
        target = self.infra.directory(other['id'])
        with self.assertRaisesRegex(budget.BudgetError, 'deleted_resource_budget'):
            budget.reserve(self.infra, other, target, self.manifest(other, target))

    def test_reserved_platform_overhead_and_memory_are_enforced_independently(self):
        self.host(cpu=100000, memory=5503)
        with self.assertRaisesRegex(budget.BudgetError, 'capacity_exhausted'):
            budget.reserve(self.infra, self.instance, self.directory, self.manifest())
        for value in [False, -1, '512']:
            invalid = {'cpu_millis': 9500, 'memory_mib': 11520, 'reserved_cpu_millis': value, 'reserved_memory_mib': 512}
            with self.subTest(value=value), self.assertRaises(budget.BudgetError):
                budget.host_budget({'fleet_host_budget': invalid})

    def test_enabling_budget_cannot_be_silently_disabled_later(self):
        self.host()
        self.infra.directory(self.instance['id'])
        changed = copy.deepcopy(self.config)
        del changed['fleet_host_budget']
        replacement = p.Infrastructure(changed)
        with self.assertRaisesRegex(p.ProvisioningError, 'disable_requires_review'):
            replacement.directory(str(uuid.uuid4()))

    def test_unverified_generation_cannot_hide_behind_existing_allocation(self):
        self.host()
        value = self.manifest()
        budget.reserve(self.infra, self.instance, self.directory, value)
        (self.directory / 'generations').mkdir()
        (self.directory / 'generations' / 'unverified').mkdir()
        with self.assertRaisesRegex(budget.BudgetError, 'generation_requires_review'):
            budget.reserve(self.infra, self.instance, self.directory, value)

    def test_failed_first_upgrade_can_retain_candidate_with_original_root_active(self):
        self.host()
        value = self.manifest()
        budget.reserve(self.infra, self.instance, self.directory, value)
        p.private_write(self.directory / 'compose.json', json.dumps(value).encode())
        candidate = str(uuid.uuid4())
        path = self.directory / 'generations' / candidate.replace('-', '')
        path.mkdir(parents=True)
        p.private_write(path / 'generation.json', json.dumps({'generation': candidate}).encode())
        self.assertFalse((self.directory / 'active.json').exists())
        reserved = budget.reserve(self.infra, self.instance, self.directory, value)
        self.assertEqual(reserved['total'], {'cpu_millis': 4250, 'memory_mib': 5504})


if __name__ == '__main__':
    unittest.main()
