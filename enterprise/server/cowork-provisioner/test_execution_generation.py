"""Filesystem/manifest lifecycle boundaries; actual Docker acceptance is separate."""
import copy
import json
from pathlib import Path
import tempfile
import unittest
import uuid
from unittest.mock import Mock, patch
import execution_generation as epochs
import execution_runtime as runtime
import provisioner as p
from operation_journal import OperationJournal, JournalError


class ExecutionGenerationTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory(); self.addCleanup(self.temp.cleanup)
        self.root=Path(self.temp.name)
        uid=lambda:str(uuid.uuid4())
        self.instance={'id':uid(),'organisation_id':uid(),'execution_epoch':uid(),'backend':'compose','state':'pending',
            'policy':{'minimum_isolation':'container','limits':{'cpu_millis':2000,'memory_mib':4096,'storage_gib':50,'concurrent_runs':2}}}
        self.op={'id':uid(),'organisation_id':self.instance['organisation_id'],'instance_id':self.instance['id'],'instance':self.instance,
            'action':'provision','arguments':{},'request_digest':'a'*64,'lease_token':uid()}
        self.op['execution_binding']={'operation_id':self.op['id'],'organisation_id':self.instance['organisation_id'],
            'instance_id':self.instance['id'],'source_epoch':self.instance['execution_epoch'],'candidate_epoch':self.instance['execution_epoch']}
        image='registry.example/cowork@sha256:'+'a'*64
        self.infra=p.Infrastructure({'state_directory':str(self.root),'backend':'compose',
            'images':{k:image for k in ['server','runner','sandbox_core','sandbox_gui','sandbox_crew','egress_proxy']},
            's3_endpoint':'http://storage:9000','services_network':'services','routing_network':'routing',
            'fleet_management_url':'http://management:8080','execution_admission':{'server_images':[image]}})
        self.infra.run=Mock(return_value=json.dumps(runtime.CONTRACT).encode())
        self.directory=self.infra.directory(self.instance['id'])

    def initialize(self):
        epochs.prepare(self.infra,self.op,self.directory)
        self.infra.write_compose(self.instance,self.directory)

    def test_new_instance_manifest_and_durable_journal_bind_both_services(self):
        self.initialize()
        result={'receipt_id':self.op['id']}
        journal=OperationJournal(self.infra,self.op)
        journal.begin(); receipt=journal.finish(True,result)
        manifest=json.loads((self.directory/'compose.json').read_text(encoding='utf-8'))
        for service in ['api','worker']:
            self.assertEqual(manifest['services'][service]['environment']['COWORK_EXECUTION_EPOCH'],self.instance['execution_epoch'])
            self.assertEqual(manifest['services'][service]['environment']['COWORK_FLEET_EXECUTION_ADMISSION'],'true')
            self.assertIn('fleet_key',manifest['services'][service]['secrets'])
        self.assertEqual(receipt['result']['execution']['binding'],self.op['execution_binding'])
        # A saved exact receipt is delivered without examining changed resources.
        with patch.object(self.infra,'active_directory',side_effect=AssertionError('must not execute again')):
            self.assertEqual(OperationJournal(self.infra,self.op).recover(),receipt)
        tampered=copy.deepcopy(self.op); tampered['execution_binding']['source_epoch']=str(uuid.uuid4()); tampered['execution_binding']['candidate_epoch']=tampered['execution_binding']['source_epoch']
        with self.assertRaisesRegex(JournalError,'binding_mismatch'): OperationJournal(self.infra,tampered).recover()

    def test_scope_corruption_missing_binding_and_legacy_enrollment_fail_before_infrastructure(self):
        bad=copy.deepcopy(self.op); bad['execution_binding']['instance_id']=str(uuid.uuid4())
        with self.assertRaisesRegex(epochs.ExecutionGenerationError,'scope_mismatch'): self.infra.execute(bad)
        self.assertFalse((self.directory/'execution.json').exists())
        legacy=copy.deepcopy(self.op); legacy['action']='start'
        with self.assertRaisesRegex(epochs.ExecutionGenerationError,'enrollment_required'): self.infra.execute(legacy)
        self.initialize()
        unbound=copy.deepcopy(self.op); del unbound['execution_binding']; unbound['action']='start'
        with self.assertRaisesRegex(epochs.ExecutionGenerationError,'binding_required'): self.infra.execute(unbound)
        marker=self.directory/'execution.json'; value=json.loads(marker.read_text(encoding='utf-8')); value['instance_id']=str(uuid.uuid4()); marker.write_text(json.dumps(value),encoding='utf-8')
        with self.assertRaisesRegex(epochs.ExecutionGenerationError,'scope_mismatch'): epochs.read(self.instance,self.directory)

    def test_generation_and_api_worker_manifest_mismatch_cannot_be_acknowledged(self):
        self.initialize()
        with self.assertRaisesRegex(epochs.ExecutionGenerationError,'immutable'):
            epochs.write(self.infra,self.instance,self.directory,str(uuid.uuid4()))
        manifest=self.directory/'compose.json'; contents=json.loads(manifest.read_text(encoding='utf-8'))
        contents['services']['worker']['environment']['COWORK_EXECUTION_EPOCH']=str(uuid.uuid4())
        manifest.write_text(json.dumps(contents),encoding='utf-8')
        journal=OperationJournal(self.infra,self.op); journal.begin()
        with self.assertRaisesRegex(epochs.ExecutionGenerationError,'manifest_mismatch'): journal.finish(True,{'receipt_id':self.op['id']})
        self.assertIsNone(journal.recover())
        for malformed in ['{','[]','{"services":{"api":null}}']:
            manifest.write_text(malformed,encoding='utf-8')
            with self.assertRaises(epochs.ExecutionGenerationError): journal.finish(True,{'receipt_id':self.op['id']})
            self.assertIsNone(journal.recover())

    def test_unbound_old_receipt_cannot_fill_new_completion_gap(self):
        self.initialize()
        journal=OperationJournal(self.infra,self.op); journal.begin()
        self.infra.private_write(journal.legacy_receipt,json.dumps({'receipt_id':self.op['id']}).encode())
        with self.assertRaisesRegex(epochs.ExecutionGenerationError,'receipt_mismatch'): journal.recover()

    def test_interrupted_restore_attests_actual_candidate_without_replaying(self):
        self.initialize()
        op=copy.deepcopy(self.op); op['id']=str(uuid.uuid4()); op['action']='restore'; op['instance']['state']='stopped'
        op['execution_binding']['operation_id']=op['id']; op['execution_binding']['candidate_epoch']=str(uuid.uuid4())
        op['recovery_authorized']=True
        active=self.directory/'generations'/uuid.UUID(op['id']).hex; active.mkdir(parents=True)
        (active/'generation.json').write_text(json.dumps({'generation':op['id'],'launch_not_before':1700000000}),encoding='utf-8')
        (active/'images.json').write_text(json.dumps(self.infra.config['images']),encoding='utf-8')
        epochs.stage(self.infra,op,active)
        (self.directory/'active.json').write_text(json.dumps({'generation':op['id']}),encoding='utf-8')
        self.infra.write_compose(self.instance,self.directory)
        journal=OperationJournal(self.infra,op); journal.begin()
        manager=Mock(); manager.call.return_value=[op]
        with patch.object(self.infra,'stop_compose'),patch.object(self.infra,'verify_stopped'),patch.object(self.infra,'execute',side_effect=AssertionError('restore replay')):
            p.reconcile_operations(manager,self.infra)
        completion=journal.recover()
        self.assertFalse(completion['succeeded'])
        self.assertEqual(completion['result']['execution']['active_epoch'],op['execution_binding']['candidate_epoch'])


if __name__=='__main__': unittest.main()
