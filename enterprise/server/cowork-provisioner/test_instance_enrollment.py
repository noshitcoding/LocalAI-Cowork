"""Crash/replay/abort boundaries. Infrastructure is mocked; Docker tests are separate."""
import copy
import json
from unittest.mock import Mock, patch
import unittest
import uuid
import os
from pathlib import Path
import tempfile

import provisioner as p
import instance_enrollment as enrollment
import execution_generation as epochs
import test_execution_generation as fixture
import local_entrypoint
from operation_journal import OperationJournal, JournalError


class DormantHistoryTests(unittest.TestCase):
    def test_dormant_rows_never_override_executor_reservation_or_quiescence_evidence(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);url=root/'database-url';url.write_text('postgres://operator:private@db/admin')
            infra=Mock();infra.config={'database_admin_url_file':str(url)}
            inventory=Mock();inventory.inventory.return_value={'scope':'cowork-'+'a'*32,'staging':[],
                'resources':[{'database':'ci_'+'a'*32,'generation':None}]};inventory.docker.return_value=[]
            dormant={key:0 for key in [*enrollment.preflight.COUNT_KEYS,'unsettled_reservations']}
            dormant.update(unfinished_runs=2,preservable_server_runs=2,legacy_assignments_requiring_review=1)
            with patch.object(enrollment,'InstanceDeletion',return_value=inventory), \
                 patch.object(enrollment.preflight,'sql',return_value='0') as sql, \
                 patch.object(enrollment.preflight,'counts',return_value=dormant) as counts:
                enrollment.stopped_inventory(infra,{},root)
                for field in ['registered_executors','external_assignment_history','executor_active_runs',
                              'unsettled_reservations','terminal_sessions','desktop_sessions']:
                    counts.return_value={**dormant,field:1}
                    with self.assertRaisesRegex(epochs.ExecutionGenerationError,'reconciliation'):
                        enrollment.stopped_inventory(infra,{},root)
                counts.return_value={**dormant,'unfinished_runs':3}
                with self.assertRaisesRegex(epochs.ExecutionGenerationError,'reconciliation'):
                    enrollment.stopped_inventory(infra,{},root)
                counts.return_value=dormant;sql.return_value='1'
                with self.assertRaisesRegex(epochs.ExecutionGenerationError,'database_still_active'):
                    enrollment.stopped_inventory(infra,{},root)


class InstanceEnrollmentTests(unittest.TestCase):
    def setUp(self):
        fixture.ExecutionGenerationTests.setUp(self)
        self.instance.update(state='stopped',execution_required=False)
        self.op.update(action='enroll_execution',execution_runtime_required=True,arguments={'acknowledge_execution_enrollment':True})
        self.infra.config['execution_enrollment_enabled']=True
        self.infra.write_compose(self.instance,self.directory)
        self.original=(self.directory/'compose.json').read_bytes()
        self.infra.ensure_secrets=Mock()
        self.inventory=Mock();self.inventory.docker.return_value=[]
        self.resources={'scope':'cowork-'+uuid.UUID(self.instance['id']).hex,'resources':[{'generation':None}], 'staging':[]}
        self.backup={'backup_id':self.op['id'],'backup_sha256':'a'*64,'backup_bytes':100,'manifest_version':1}
        backup_manager=Mock();backup_manager.root=self.root/'backups';backup_manager.root.mkdir();backup_manager.config=self.infra.config
        backup_manager.backup.return_value=self.backup
        self.backup_manager=backup_manager
        for name,value in [('stopped_inventory',(self.inventory,self.resources,[],'b'*64)),('InstanceBackups',backup_manager)]:
            context=patch.object(enrollment,name,return_value=value);setattr(self,name,context.start());self.addCleanup(context.stop)
        self.probes=[]
        for name,value in [('verify_backup',{'sha256':'a'*64,'bytes':100}),('inspect',{'blockers':[]})]:
            context=patch.object(enrollment.preflight,name,return_value=value);self.probes.append(context.start());self.addCleanup(context.stop)
        self.journal=OperationJournal(self.infra,self.op);self.journal.begin()

    def test_fresh_backup_precedes_immutable_marker_and_completion_does_not_start_work(self):
        def backup(*args):
            self.assertIsNone(epochs.read(self.instance,self.directory))
            return self.backup
        self.backup_manager.backup.side_effect=backup
        receipt=self.infra.execute(self.op)
        self.assertEqual(receipt['execution_enrollment']['outcome'],'enrolled')
        self.assertEqual(epochs.read(self.instance,self.directory),self.instance['execution_epoch'])
        result=self.journal.finish(True,receipt)
        self.infra.run.reset_mock();self.assertEqual(self.journal.recover(),result);self.infra.run.assert_not_called()
        self.assertEqual(self.backup_manager.backup.call_count,1)
        self.assertIn('runtime',receipt['execution'])
        self.assertEqual(json.loads((self.directory/'compose.json').read_text())['services']['worker']['environment']['COWORK_FLEET_EXECUTION_ADMISSION'],'true')

    def test_failed_precheck_proves_unchanged_and_keeps_original_startable_configuration(self):
        self.stopped_inventory.side_effect=epochs.ExecutionGenerationError('retained_connection')
        with self.assertRaises(p.ProvisioningError) as raised:self.infra.execute(self.op)
        error=raised.exception
        self.assertEqual(str(error),'execution_enrollment_precheck_failed')
        self.assertEqual((self.directory/'compose.json').read_bytes(),self.original)
        self.assertIsNone(epochs.read(self.instance,self.directory))
        self.backup_manager.backup.assert_not_called();self.infra.ensure_secrets.assert_not_called()
        completed=self.journal.finish(False,{**error.result,'error_code':str(error)})
        self.assertEqual(self.journal.recover(),completed)
        bad=copy.deepcopy(completed);bad['succeeded']=True
        with self.assertRaises(JournalError):self.journal.validate_completion(bad)

    def interrupt_after_marker(self):
        write=self.infra.write_compose
        self.infra.write_compose=Mock(side_effect=OSError('injected durable configuration failure'))
        with self.assertRaisesRegex(epochs.ExecutionGenerationError,'requires_reconciliation'):self.infra.execute(self.op)
        self.infra.write_compose=write
        self.assertIsNone(self.journal.recover())
        self.assertIsNotNone(epochs.read(self.instance,self.directory))

    def test_commit_point_requires_explicit_recovery_and_never_recaptures_or_starts(self):
        self.interrupt_after_marker()
        with self.assertRaisesRegex(epochs.ExecutionGenerationError,'explicit_recovery'):self.infra.execute(self.op)
        result=self.infra.execute(self.op,enrollment_resume=True)
        self.assertEqual(self.backup_manager.backup.call_count,1)
        self.assertEqual(self.journal.finish(True,result)['result']['execution_enrollment']['outcome'],'enrolled')
        self.assertFalse(any('up' in call.args[0] or 'start' in call.args[0] for call in self.infra.run.call_args_list))

    def test_committed_plan_cannot_change_scope_active_generation_or_configuration(self):
        self.interrupt_after_marker()
        self.infra.config['fleet_management_url']='http://different-control:8080'
        with self.assertRaisesRegex(epochs.ExecutionGenerationError,'plan_changed'):self.infra.execute(self.op,enrollment_resume=True)
        self.assertIsNotNone(epochs.read(self.instance,self.directory))
        self.assertIsNone(self.journal.recover())

    def test_missing_runtime_receipt_and_changed_generation_cannot_be_acknowledged(self):
        receipt=self.infra.execute(self.op)
        bad=copy.deepcopy(receipt);del bad['execution']['runtime']
        with self.assertRaises(epochs.ExecutionGenerationError):self.journal.finish(True,bad)
        bad=copy.deepcopy(receipt);bad['execution_enrollment']['binding']['instance_id']=str(uuid.uuid4())
        with self.assertRaises(epochs.ExecutionGenerationError):self.journal.finish(True,bad)

    def test_transitional_operator_permits_stopping_but_never_starts_unbound_work(self):
        op=copy.deepcopy(self.op);op['execution_binding']=None;op['execution_runtime_required']=False
        op['action']='stop';epochs.prepare(self.infra,op,self.directory)
        for action in ['start','resume','provision']:
            op['action']=action
            with self.assertRaisesRegex(epochs.ExecutionGenerationError,'binding_required'):epochs.prepare(self.infra,op,self.directory)

    def test_local_profile_requires_explicit_flag_and_separate_image_allowlist(self):
        with patch.dict(os.environ,{},clear=True):
            config={};local_entrypoint.execution_configuration(config);self.assertEqual(config,{})
            os.environ['COWORK_EXECUTION_ENROLLMENT_ENABLED']='true'
            with self.assertRaisesRegex(p.ProvisioningError,'requires_admission_allowlist'):local_entrypoint.execution_configuration({})
            path=self.root/'allowlist.json';path.write_text(json.dumps(self.infra.config['execution_admission']),encoding='utf-8')
            os.environ['COWORK_EXECUTION_ADMISSION_CONFIG_FILE']=str(path)
            config={};local_entrypoint.execution_configuration(config)
            self.assertTrue(config['execution_enrollment_enabled'])
            self.assertEqual(p.operator_contract({'backend':'compose',**config}),{'backend':'compose','execution_admission_version':1,'execution_enrollment_version':1})
            os.environ['COWORK_EXECUTION_ENROLLMENT_ENABLED']='yes'
            with self.assertRaisesRegex(p.ProvisioningError,'invalid_execution_enrollment'):local_entrypoint.execution_configuration({})


if __name__=='__main__':unittest.main()
