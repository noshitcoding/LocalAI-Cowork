import copy
import json
from pathlib import Path
import tempfile
import unittest
import uuid
from unittest.mock import patch

import provisioner as p
import provisioning_release as releases
from release_catalog import IMAGE_KEYS, ReleaseCatalog, ReleaseError, image_set_id
from execution_generation import ExecutionGenerationError
from operation_journal import OperationJournal, JournalError


class ProvisioningReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        root = Path(self.temp.name)
        images = lambda c: {key: f'registry.test/{key}@sha256:' + c * 64 for key in IMAGE_KEYS}
        self.old, self.new = images('a'), images('b')
        self.config = dict(state_directory=str(root / 'instances'), release_directory=str(root / 'releases'),
                           backend='compose', release_selection_enabled=True, images=self.old,
                           s3_endpoint='https://storage.test', services_network='services', routing_network='routing')
        self.infra = p.Infrastructure(self.config)
        self.catalog = ReleaseCatalog(self.config, p.private_write)
        self.release = self.catalog.publish('Selected version', self.new, ['c' * 64])
        self.instance = dict(id=str(uuid.uuid4()), organisation_id=str(uuid.uuid4()), backend='compose',
                             provisioning_release=self.release['id'], policy=dict(minimum_isolation='container',
                             limits=dict(cpu_millis=2000, memory_mib=4096, storage_gib=50, concurrent_runs=2)))
        self.operation = dict(id=str(uuid.uuid4()), instance_id=self.instance['id'], organisation_id=self.instance['organisation_id'],
                              action='provision', instance=self.instance, lease_token=str(uuid.uuid4()), request_digest='d'*64,
                              arguments=dict(release=self.release['id'], provisioning_release_version=1))
        self.directory = self.infra.directory(self.instance['id'])

    def test_provision_pins_selection_before_credentials_and_ignores_defaults(self):
        def secrets(instance, active):
            self.assertEqual(json.loads((active / 'images.json').read_text()), self.new)
            self.config['images'] = {key: image.replace('a'*64, 'e'*64) for key,image in self.old.items()}
            return {}
        with patch.object(p, 'verify_security_host'), patch.object(p, 'retain_fleet_images') as retain, \
                patch.object(p.execution_runtime, 'probe'), patch.object(self.infra, 'ensure_secrets', side_effect=secrets), \
                patch.object(self.infra, 'ensure_database'), patch.object(self.infra, 'ensure_bucket'), patch.object(self.infra, 'compose'):
            receipt = self.infra.execute(self.operation)
            retain.assert_called_once_with(self.new, self.infra.run, p.immutable_image)
        self.assertEqual(self.infra.deployed_images(self.directory), self.new)
        self.assertEqual(receipt, dict(receipt_id=self.operation['id'], instance_id=self.instance['id'], release=self.release['id']))
        # A lost completion response can be delivered after withdrawal, without replay.
        self.catalog.withdraw(self.release['id'])
        with patch.object(self.infra, 'ensure_database') as database:
            self.assertEqual(self.infra.execute(self.operation), receipt)
            self.assertEqual(OperationJournal(self.infra, self.operation).recover()['result'], receipt)
        database.assert_not_called()

    def test_unknown_withdrawn_or_disabled_selection_creates_no_credentials(self):
        for case in ['unknown', 'withdrawn', 'disabled']:
            with self.subTest(case=case):
                operation = copy.deepcopy(self.operation)
                if case == 'unknown':
                    operation['arguments']['release'] = operation['instance']['provisioning_release'] = 'f'*64
                elif case == 'withdrawn':
                    self.catalog.withdraw(self.release['id'])
                else:
                    self.config['release_selection_enabled'] = False
                with patch.object(p, 'verify_security_host'), patch.object(self.infra, 'ensure_secrets') as secrets:
                    with self.assertRaises(p.ProvisioningError): self.infra.execute(operation)
                    secrets.assert_not_called()
                self.assertFalse((self.directory / 'images.json').exists())

    def test_binding_requires_both_instance_and_operation_and_exact_version(self):
        for mutate in [lambda o: o['arguments'].pop('release'), lambda o: o['instance'].pop('provisioning_release'),
                       lambda o: o['arguments'].update(provisioning_release_version=True),
                       lambda o: o['arguments'].update(release='e'*64)]:
            operation = copy.deepcopy(self.operation); mutate(operation)
            with self.assertRaises(ExecutionGenerationError): OperationJournal(self.infra,operation)

    def test_preexisting_pins_or_manifest_cannot_be_replaced(self):
        for name in ['images.json', 'compose.json']:
            with self.subTest(file=name):
                value = self.old if name=='images.json' else p.compose_manifest(self.config,self.instance,self.directory)
                path = self.directory / name; p.private_write(path,json.dumps(value).encode())
                with self.assertRaises(ReleaseError): releases.prepare(self.infra,self.operation,self.directory,self.directory)
                self.assertEqual(json.loads(path.read_text()),value)
                path.unlink()

    def test_durable_pin_survives_operator_restart_and_defaults_change(self):
        releases.prepare(self.infra,self.operation,self.directory,self.directory)
        restarted = p.Infrastructure({**self.config,'images':{key:value.replace('a'*64,'d'*64) for key,value in self.old.items()}})
        self.assertEqual(releases.prepare(restarted,self.operation,self.directory,self.directory),self.new)
        with patch.object(p.execution_runtime,'probe'):
            restarted.write_compose(self.instance,self.directory)
        self.assertEqual(image_set_id(restarted.deployed_images(self.directory)),self.release['id'])

    def test_completed_receipt_must_prove_instance_operation_and_all_images(self):
        good = dict(receipt_id=self.operation['id'],instance_id=self.instance['id'],release=self.release['id'])
        for key in good:
            receipt = {**good,key:'wrong'}
            with self.assertRaises((ExecutionGenerationError,JournalError)):
                OperationJournal(self.infra,self.operation).finish(True,receipt)
        releases.prepare(self.infra,self.operation,self.directory,self.directory)
        p.private_write(self.directory / 'compose.json',json.dumps(p.compose_manifest(self.config,self.instance,self.directory)).encode())
        with self.assertRaises(ExecutionGenerationError): releases.attach(self.infra,self.operation,self.directory,good)

    def test_unproven_stopping_recovery_can_finish_without_claiming_selected_release(self):
        self.catalog.withdraw(self.release['id'])
        result = {'error_code':'operation_interrupted_resources_stopped','instance_state':'stopped'}
        self.assertFalse(OperationJournal(self.infra,self.operation).finish(False,result)['succeeded'])

    def test_stop_recovery_requires_matching_pins_even_after_catalog_withdrawal(self):
        releases.prepare(self.infra,self.operation,self.directory,self.directory)
        with patch.object(p.execution_runtime,'probe'):
            self.infra.write_compose(self.instance,self.directory)
        self.catalog.withdraw(self.release['id'])
        releases.verify_installed(self.infra,self.operation,self.directory)
        p.private_write(self.directory / 'images.json',json.dumps(self.old).encode())
        with self.assertRaises(ExecutionGenerationError):
            releases.verify_installed(self.infra,self.operation,self.directory)
        self.assertEqual(self.infra.deployed_images(self.directory),self.new)

    def test_old_configuration_does_not_advertise_new_protocol(self):
        self.assertEqual(p.operator_contract({'backend':'compose'}),{'backend':'compose'})
        self.assertEqual(p.operator_contract(self.config),{'backend':'compose','provisioning_release_version':1})


if __name__ == '__main__': unittest.main()
