import copy
import json
from pathlib import Path
import tempfile
import unittest
import uuid
from unittest.mock import Mock, patch

import instance_imports as imports
from execution_generation import ExecutionGenerationError
from instance_backups import BackupError, validate_manifest
from operation_journal import OperationJournal, JournalError
from provisioner import Infrastructure


class ImportTests(unittest.TestCase):
    def setUp(self):
        uid=lambda:str(uuid.uuid4())
        org,host=uid(),uid()
        source={'id':uid(),'organisation_id':org,'executor_host_id':None,'execution_required':False,
                'execution_epoch':uid(),'backend':'compose','state':'stopped','policy':{'minimum_isolation':'container'}}
        target={**source,'id':uid(),'executor_host_id':host,'execution_required':True,'execution_epoch':uid(),
                'policy':{'minimum_isolation':'container','limits':{'cpu_millis':500,'memory_mib':512}}}
        self.frozen={'version':1,'import_id':uid(),'organisation_id':org,'source':source,'target':target,
            'source_host_binding':None,'target_host_binding':{'version':1,'host_id':host,'trust_domain':org,'daemon_id':'daemon','operator_state_id':uid()},
            'source_allocation':None,'target_allocation':{},'source_epoch':source['execution_epoch'],'target_epoch':target['execution_epoch'],
            'candidate_epoch':uid(),'backup':{'id':uid(),'organisation_id':org,'instance_id':source['id'],'sha256':'a'*64,'size_bytes':123},'reservation_ids':[]}
        self.export={'backup_id':self.frozen['backup']['id'],'backup_sha256':'a'*64,'backup_bytes':123,'manifest_sha256':'b'*64,
            'images':{'server':'server@sha256:'+'c'*64},'workspace_count':0,'inventory_sha256':'d'*64}

    def operation(self,action):
        frozen=copy.deepcopy(self.frozen)
        instance=frozen['target'] if action in ['import_restore','import_qualify'] else frozen['source']
        return {'id':str(uuid.uuid4()),'organisation_id':frozen['organisation_id'],'instance_id':instance['id'],'instance':instance,
            'action':action,'lease_token':str(uuid.uuid4()),'request_digest':'e'*64,
            'executor_host_binding':frozen['target_host_binding'] if instance is frozen['target'] else None,
            'arguments':{'import_binding':frozen,'import_binding_sha256':imports.canonical(frozen),'import_export':copy.deepcopy(self.export),
                'import_source_stop':{'source_inventory_sha256':'f'*64}}}

    def receipt(self,op):
        target=op['action'] in ['import_restore','import_qualify']
        value={'version':1,'import_id':self.frozen['import_id'],'operation_id':op['id'],'step':imports.ACTIONS[op['action']],
            'binding_sha256':imports.canonical(self.frozen),'source_inventory_sha256':'f'*64,'source_stopped':True,
            'target_stopped':target,'export':copy.deepcopy(self.export) if op['action']=='import_export' else None,'target':None}
        if target:
            value['target']={'instance_id':self.frozen['target']['id'],'execution_epoch':self.frozen['candidate_epoch'],
                'manifest_sha256':'b'*64,'inventory_sha256':'a'*64,'credentials_rotated':True,'schedules_paused':True,
                'unfinished_runs_interrupted':True,'historical_resume_blocked':True,'workspace_inventory_complete':True,
                'bounded_job_sha256':'c'*64 if op['action']=='import_qualify' else None}
        return {'receipt_id':op['id'],'instance_state':'stopped','instance_import':value}

    def test_separate_import_preserves_unbound_legacy_source(self):
        for action in imports.ACTIONS:
            op=self.operation(action)
            self.assertEqual(imports.binding(op),self.frozen)
            imports.validate_receipt(op,self.receipt(op))
        self.assertIsNone(self.frozen['source']['executor_host_id'])

    def test_cross_org_catalog_epoch_and_same_target_refused(self):
        for mutate in [lambda b:b['target'].update(organisation_id=str(uuid.uuid4())),
                       lambda b:b['target'].update(id=b['source']['id']),
                       lambda b:b.update(candidate_epoch=b['source_epoch']),
                       lambda b:b['backup'].update(instance_id=b['target']['id']),
                       lambda b:b.update(version=True),lambda b:b.update(caller_credentials='bad')]:
            op=self.operation('import_restore');mutate(op['arguments']['import_binding'])
            op['arguments']['import_binding_sha256']=imports.canonical(op['arguments']['import_binding'])
            with self.subTest(mutate=mutate),self.assertRaises(ExecutionGenerationError):imports.binding(op)

    def test_operator_and_binding_digest_cannot_retarget(self):
        for field in ['instance_id','organisation_id','executor_host_binding']:
            op=self.operation('import_restore');op[field]=str(uuid.uuid4())
            with self.subTest(field=field),self.assertRaises(ExecutionGenerationError):imports.binding(op)
        op=self.operation('import_export');op['arguments']['import_binding']['candidate_epoch']=str(uuid.uuid4())
        with self.assertRaisesRegex(ExecutionGenerationError,'digest'):imports.binding(op)

    def test_source_operator_cannot_attest_target_and_changed_stop_denied(self):
        op=self.operation('import_export');proof=self.receipt(op);proof['instance_import']['target_stopped']=True
        with self.assertRaises(ExecutionGenerationError):imports.validate_receipt(op,proof)
        proof=self.receipt(op);proof['instance_import']['source_inventory_sha256']='e'*64
        with self.assertRaisesRegex(ExecutionGenerationError,'stop_changed'):imports.validate_receipt(op,proof)

    def test_original_catalog_and_complete_export_required(self):
        op=self.operation('import_export')
        for key,value in [('backup_id',str(uuid.uuid4())),('backup_sha256','e'*64),('backup_bytes',124),('workspace_count',True),('inventory_sha256','missing')]:
            proof=self.receipt(op);proof['instance_import']['export'][key]=value
            with self.subTest(key=key),self.assertRaises(ExecutionGenerationError):imports.validate_receipt(op,proof)

    def test_target_missing_fences_and_old_authority_refused(self):
        op=self.operation('import_restore')
        for key,value in [('credentials_rotated',False),('schedules_paused',False),('unfinished_runs_interrupted',False),
                          ('historical_resume_blocked',False),('workspace_inventory_complete',False),
                          ('execution_epoch',self.frozen['source_epoch']),('manifest_sha256','c'*64),('inventory_sha256','bad')]:
            proof=self.receipt(op);proof['instance_import']['target'][key]=value
            with self.subTest(key=key),self.assertRaises(ExecutionGenerationError):imports.validate_receipt(op,proof)

    def test_qualification_requires_actual_job_receipt(self):
        op=self.operation('import_qualify');proof=self.receipt(op);proof['instance_import']['target']['bounded_job_sha256']=None
        with self.assertRaisesRegex(ExecutionGenerationError,'actual_job'):imports.validate_receipt(op,proof)

    def test_original_manifest_scope_never_rewritten_for_ordinary_restore(self):
        manifest={'schema_version':1,'instance_id':self.frozen['source']['id'],'organisation_id':self.frozen['organisation_id'],
            'backup_id':self.frozen['backup']['id'],'images':self.export['images'],'workspace_runs':[],
            'files':{name:{'bytes':1,'sha256':'a'*64} for name in ['database.dump','instance.json','secrets/storage_master_key','secrets/bootstrap_token','secrets/runner_signing_key']}}
        original=copy.deepcopy(manifest);op=self.operation('import_restore')
        op['arguments']['import_export']['manifest_sha256']=imports.canonical(manifest)
        imports.verify_export(op,manifest)
        self.assertEqual(manifest,original)
        with self.assertRaisesRegex(BackupError,'scope'):validate_manifest(manifest,self.frozen['target'],self.frozen['backup']['id'],self.export['images'])
        manifest['workspace_runs']=['a'*32]
        with self.assertRaisesRegex(ExecutionGenerationError,'original_export'):imports.verify_export(op,manifest)

    def test_missing_target_fences_never_produce_success(self):
        op=self.operation('import_restore')
        with tempfile.TemporaryDirectory() as tmp:
            active=Path(tmp);(active/'secrets').mkdir();(active/'secrets'/'database_url').write_text('postgres://u:p@db/x')
            infra=Mock(config={},active_directory=Mock(return_value=active))
            counts={'runs':2,'origins':2,'active_runs':0,'schedules':0,'sessions':0,'credentials':0}
            for field in ['origins','active_runs','schedules','sessions','credentials']:
                failed={**counts,field:1};infra.run.return_value=json.dumps(failed).encode()
                with self.subTest(field=field),self.assertRaisesRegex(ExecutionGenerationError,'fences'):imports._target_evidence(infra,op,active)

    def test_durable_receipt_replays_without_import_io_and_unknown_phase_does_not(self):
        op=self.operation('import_quiesce')
        with tempfile.TemporaryDirectory() as tmp:
            infra=Infrastructure({'state_directory':tmp,'backend':'compose'})
            journal=OperationJournal(infra,op);journal.begin()
            self.assertIsNone(journal.recover())
            with self.assertRaisesRegex(JournalError,'started_without_completion'):journal.begin()
            accepted=journal.finish(True,self.receipt(op))
            self.assertEqual(OperationJournal(infra,op).recover(),accepted)
            changed=copy.deepcopy(op);changed['arguments']['import_binding']['candidate_epoch']=str(uuid.uuid4())
            changed['arguments']['import_binding_sha256']=imports.canonical(changed['arguments']['import_binding'])
            with self.assertRaisesRegex(JournalError,'binding_mismatch'):OperationJournal(infra,changed).recover()

    def test_qualify_refuses_job_beyond_current_profile_before_start(self):
        op=self.operation('import_qualify');op['instance']['policy']['limits']['memory_mib']=64
        op['arguments']['import_binding_sha256']=imports.canonical(op['arguments']['import_binding'])
        with patch('instance_imports.InstanceBackups'),patch('instance_updates.InstanceUpdates') as updates:
            infra=Mock(config={'images':{}},active_directory=Mock(return_value=Path('unused')))
            with self.assertRaisesRegex(ExecutionGenerationError,'exceeds_profile'):imports.execute(infra,op,Path('unused'))
            updates.return_value.probe.assert_not_called()


if __name__=='__main__':unittest.main()
