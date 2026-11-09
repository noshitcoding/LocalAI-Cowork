"""Negative preflight boundaries; real infrastructure/age checks are separate."""
import copy
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch
import uuid

import execution_enrollment as enrollment
import execution_generation as epochs
import test_execution_generation as fixture


class EnrollmentPreflightTests(unittest.TestCase):
    def setUp(self):
        fixture.ExecutionGenerationTests.setUp(self)
        self.instance['state']='stopped'
        self.infra.write_compose(self.instance,self.directory)
        backup_root=self.root/'backups';backup_root.mkdir()
        admin=self.root/'db-url';admin.write_text('postgres://operator:private@db:5432/admin',encoding='utf-8')
        self.infra.config.update(backup_directory=str(backup_root),database_admin_url_file=str(admin))
        self.backup=str(uuid.uuid4())
        self.inventory=Mock()
        self.inventory.inventory.return_value={'scope':'cowork-'+uuid.UUID(self.instance['id']).hex,
            'staging':[],'resources':[{'database':'ci_'+'a'*32,'generation':None},{'database':'ci_'+'b'*32,'generation':str(uuid.uuid4())}]}
        self.inventory.docker.return_value=[{'Id':'a'*64,'State':{'Running':False,'Paused':False,'Restarting':False}}]
        self.counts={key:0 for key in [*enrollment.COUNT_KEYS,'unsettled_reservations']}
        for name,value in [('InstanceDeletion',self.inventory),('counts',self.counts),('sql','0'),('verify_backup',{'backup_id':self.backup})]:
            patched=patch.object(enrollment,name,return_value=value)
            setattr(self,name,patched.start());self.addCleanup(patched.stop)

    def inspect(self): return enrollment.inspect(self.infra,self.instance,self.backup)

    def test_complete_observation_never_becomes_an_enrollment_capability(self):
        before={p:p.read_bytes() for p in self.directory.rglob('*') if p.is_file()}
        result=self.inspect()
        self.assertTrue(result['checks_passed']);self.assertFalse(result['eligible_for_enrollment'])
        self.assertEqual(result['verified_generations'],2)
        self.assertEqual(self.counts.call_count,2)
        self.assertEqual(before,{p:p.read_bytes() for p in self.directory.rglob('*') if p.is_file()})
        self.assertFalse((self.directory/'execution.json').exists())
        self.assertNotIn('private',json.dumps(result))
        for call in self.infra.run.call_args_list:
            self.assertEqual(call.args[0][0:3],['docker','run','--rm'])
            self.assertEqual(call.args[0][-1],'--execution-admission-contract')

    def test_all_generations_open_work_and_ambiguous_external_history_block(self):
        for key in self.counts.return_value:
            counts={k:0 for k in self.counts.return_value};counts[key]=1
            self.counts.side_effect=[{k:0 for k in counts},counts]
            result=self.inspect()
            self.assertFalse(result['checks_passed']);self.assertIn(key,result['blockers'])
        self.counts.side_effect=None
        self.sql.return_value='1'
        self.assertIn('database_connections_present',self.inspect()['blockers'])

    def test_partial_or_changed_inventory_never_reports_success(self):
        first=copy.deepcopy(self.inventory.inventory.return_value)
        self.inventory.inventory.side_effect=[first,{**first,'resources':[]}]
        with self.assertRaisesRegex(enrollment.EnrollmentPreflightError,'inventory_changed'):self.inspect()
        self.inventory.inventory.side_effect=None
        self.inventory.inventory.return_value['staging']=['capture-unfinished']
        with self.assertRaisesRegex(enrollment.EnrollmentPreflightError,'staging_unsettled'):self.inspect()

    def test_unknown_or_restarting_container_state_cannot_prove_stop(self):
        for state in [{'Running':False}, {'Running':False,'Paused':False,'Restarting':True}]:
            self.inventory.docker.return_value=[{'Id':'a'*64,'State':state}]
            self.assertIn('containers_active',self.inspect()['blockers'])
        stopped={'State':{'Running':False,'Paused':False,'Restarting':False}}
        self.inventory.docker.side_effect=[[{'Id':'a',**stopped}],[{'Id':'b',**stopped}]]
        self.assertIn('containers_changed_during_check',self.inspect()['blockers'])

    def test_pending_journal_and_foreign_scope_fail_before_backup_decryption(self):
        op_id=str(uuid.uuid4())
        path=self.directory/('operation-'+uuid.UUID(op_id).hex+'.json')
        path.write_text(json.dumps({'binding':{'id':op_id,'instance_id':self.instance['id'],
            'organisation_id':self.instance['organisation_id']},'completion':None}),encoding='utf-8')
        with self.assertRaisesRegex(enrollment.EnrollmentPreflightError,'operation_unsettled'):self.inspect()
        self.verify_backup.assert_not_called()
        path.unlink()
        self.instance['organisation_id']=str(uuid.uuid4())
        with self.assertRaisesRegex(enrollment.EnrollmentPreflightError,'manifest_scope'):self.inspect()
        self.verify_backup.assert_not_called()

    def test_enrolled_instances_and_unqualified_images_are_not_silently_modified(self):
        self.infra.config['execution_admission']['server_images']=['unknown.example/server@sha256:'+'b'*64]
        with self.assertRaisesRegex(epochs.ExecutionGenerationError,'image_not_allowed'):self.inspect()
        self.assertFalse((self.directory/'execution.json').exists())
        epochs.write(self.infra,self.instance,self.directory,self.instance['execution_epoch'])
        with self.assertRaisesRegex(enrollment.EnrollmentPreflightError,'already_has_execution_epoch'):self.inspect()

    def test_api_worker_or_pinned_image_disagreement_is_not_qualified(self):
        manifest_path=self.directory/'compose.json'
        original=json.loads(manifest_path.read_text(encoding='utf-8'))
        manifest=copy.deepcopy(original);manifest['services']['worker']['image']='old.example/image@sha256:'+'b'*64
        manifest_path.write_text(json.dumps(manifest),encoding='utf-8')
        with self.assertRaisesRegex(enrollment.EnrollmentPreflightError,'worker_image_mismatch'):self.inspect()
        manifest_path.write_text(json.dumps(original),encoding='utf-8')
        images=json.loads((self.directory/'images.json').read_text(encoding='utf-8'));images['server']=manifest['services']['worker']['image']
        (self.directory/'images.json').write_text(json.dumps(images),encoding='utf-8')
        with self.assertRaisesRegex(enrollment.EnrollmentPreflightError,'pinned_images_mismatch'):self.inspect()


class EnrollmentQueryTests(unittest.TestCase):
    def test_cli_does_not_create_missing_state_or_echo_invalid_input(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);config=root/'config.json';state=root/'missing-state'
            config.write_text(json.dumps({'state_directory':str(state)}),encoding='utf-8')
            for instance in [str(uuid.uuid4()),'private-invalid-value']:
                result=subprocess.run([sys.executable,enrollment.__file__,'--config',str(config),'--instance-id',instance,
                    '--organisation-id',str(uuid.uuid4()),'--backup-id',str(uuid.uuid4())],capture_output=True,text=True)
                self.assertEqual(result.returncode,2);self.assertEqual(result.stderr,'')
                self.assertEqual(json.loads(result.stdout)['error_code'],'enrollment_preflight_unproven')
                self.assertFalse(state.exists());self.assertNotIn('private-invalid-value',result.stdout)

    def test_readonly_transactions_have_bounded_timeouts_and_no_password_in_arguments(self):
        infra=Mock();infra.run.return_value=b'0'
        self.assertEqual(enrollment.sql(infra,{'PGPASSWORD':'private'},'SELECT 0;'),'0')
        args,kwargs=infra.run.call_args
        self.assertNotIn('private',' '.join(args[0]));self.assertIn('default_transaction_read_only=on',kwargs['env']['PGOPTIONS'])
        self.assertIn('REPEATABLE READ READ ONLY',kwargs['data'].decode());self.assertEqual(kwargs['timeout'],15)

    def test_unknown_schema_or_malformed_counts_do_not_masquerade_as_empty_work(self):
        good={k:0 for k in enrollment.COUNT_KEYS}
        with patch.object(enrollment,'sql') as sql:
            for value in [{},[],{**good,'unfinished_runs':False},{**good,'unfinished_runs':-1},{**good,'preservable_server_runs':1}]:
                sql.return_value=json.dumps(value)
                with self.assertRaises(enrollment.EnrollmentPreflightError):enrollment.counts(Mock(),{})
            sql.side_effect=[json.dumps(good),json.dumps({'reservations':None})]
            with self.assertRaises(enrollment.EnrollmentPreflightError):enrollment.counts(Mock(),{})


if __name__=='__main__':unittest.main()
