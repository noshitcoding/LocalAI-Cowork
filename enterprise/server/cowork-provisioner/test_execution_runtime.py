"""Opt-in, compatibility and credential boundaries independent of the Docker daemon."""
import copy
import json
import unittest
from unittest.mock import Mock
from operation_journal import OperationJournal, JournalError
import execution_generation as epochs
import execution_runtime as runtime
import test_execution_generation as fixture


class ExecutionRuntimeTests(unittest.TestCase):
    setUp = fixture.ExecutionGenerationTests.setUp
    initialize = fixture.ExecutionGenerationTests.initialize

    def test_both_execution_paths_have_credentials_but_job_services_do_not(self):
        self.initialize()
        manifest=json.loads((self.directory/'compose.json').read_text(encoding='utf-8'))
        runtime.validate_manifest(self.infra.config,manifest)
        for name in ['runner','egress-proxy']:
            self.assertNotIn('fleet_key',manifest['services'][name].get('secrets',[]))
            self.assertNotIn('services',manifest['services'][name]['networks'])
        command=self.infra.run.call_args.args[0]
        self.assertIn('none',command); self.assertNotIn('--volume',command)
        self.assertIn('--read-only',command); self.assertEqual(command[-1],'--execution-admission-contract')
        self.assertEqual(command[command.index('--cpus')+1], '0.1')
        self.assertEqual(command[command.index('--memory')+1], '128m')
        self.assertEqual(command[command.index('--memory-swap')+1], '128m')

    def test_refuses_unknown_images_old_binaries_and_missing_operator_opt_in(self):
        epochs.prepare(self.infra,self.op,self.directory)
        config=copy.deepcopy(self.infra.config)
        for images in [[],['image:latest'],['registry.example/new@sha256:'+'b'*64]]:
            self.infra.config['execution_admission']={'server_images':images}
            with self.assertRaises(epochs.ExecutionGenerationError): self.infra.write_compose(self.instance,self.directory)
            self.assertFalse((self.directory/'compose.json').exists())
        self.infra.config=config
        for output in [b'',b'{}',b'not-json',json.dumps({**runtime.CONTRACT,'external_claims':False}).encode(),
                       json.dumps({**runtime.CONTRACT,'external_claims':1}).encode()]:
            self.infra.run=Mock(return_value=output)
            with self.assertRaisesRegex(epochs.ExecutionGenerationError,'incompatible'): self.infra.write_compose(self.instance,self.directory)
        del self.infra.config['execution_admission']
        with self.assertRaisesRegex(epochs.ExecutionGenerationError,'opt_in_required'): self.infra.write_compose(self.instance,self.directory)

    def test_acknowledgment_refuses_disabled_worker_missing_credentials_and_secret_leaks(self):
        self.initialize()
        path=self.directory/'compose.json'
        original=json.loads(path.read_text(encoding='utf-8'))
        for mutate in [lambda m:m['services']['worker']['environment'].update({'COWORK_FLEET_EXECUTION_ADMISSION':'false'}),
                       lambda m:m['services']['worker']['secrets'].remove('fleet_key'),
                       lambda m:m['services']['runner']['secrets'].append('fleet_key'),
                       lambda m:m['services']['api']['environment'].update({'COWORK_FLEET_MANAGEMENT_URL':'http://foreign'})]:
            manifest=copy.deepcopy(original); mutate(manifest)
            path.write_text(json.dumps(manifest),encoding='utf-8')
            with self.assertRaises(epochs.ExecutionGenerationError): epochs.attach(self.infra,self.op,{},True)

    def test_enabled_operator_cannot_dispatch_unbound_legacy_work(self):
        unbound=copy.deepcopy(self.op); del unbound['execution_binding']
        with self.assertRaisesRegex(epochs.ExecutionGenerationError,'binding_required'): self.infra.execute(unbound)
        self.infra.run.assert_not_called()

    def test_runtime_attestation_is_generated_from_verified_manifest_and_survives_exact_replay(self):
        self.op['execution_runtime_required']=True
        self.initialize()
        journal=OperationJournal(self.infra,self.op);journal.begin()
        completion=journal.finish(True,{'receipt_id':self.op['id']})
        proof=completion['result']['execution']['runtime']
        self.assertEqual(proof['version'],1)
        self.assertEqual(proof['server_image'],self.infra.config['images']['server'])
        self.assertRegex(proof['manifest_sha256'],'^[a-f0-9]{64}$')
        self.assertEqual(OperationJournal(self.infra,self.op).recover(),completion)
        downgraded=copy.deepcopy(self.op);del downgraded['execution_runtime_required']
        with self.assertRaisesRegex(JournalError,'binding_mismatch'):OperationJournal(self.infra,downgraded).recover()

    def test_missing_or_malformed_runtime_proof_is_not_replayed_as_success(self):
        self.op['execution_runtime_required']=True
        self.initialize()
        good=epochs.attach(self.infra,self.op,{'receipt_id':self.op['id']},True)
        for mutate in [lambda r:r['execution'].pop('runtime'),
                       lambda r:r['execution']['runtime'].update(version=True),
                       lambda r:r['execution']['runtime'].update(server_image='image:latest'),
                       lambda r:r['execution']['runtime'].update(manifest_sha256='invalid')]:
            receipt=copy.deepcopy(good);mutate(receipt)
            with self.assertRaises(epochs.ExecutionGenerationError):epochs.validate_receipt(self.op,receipt)


if __name__=='__main__': unittest.main()
