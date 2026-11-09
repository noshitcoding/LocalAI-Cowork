import copy
import json
from unittest.mock import Mock,patch
import unittest
import uuid
import execution_recovery as recovery
import execution_generation as epochs
import provisioner as p
import test_execution_generation as fixture
from operation_journal import OperationJournal


class ExecutionRecoveryTests(unittest.TestCase):
    def setUp(self):
        fixture.ExecutionGenerationTests.setUp(self)
        fixture.ExecutionGenerationTests.initialize(self)
        self.op['action']='stop'
        entry={'reservation_id':str(uuid.uuid4()),'execution_epoch':self.instance['execution_epoch'],'run_id':str(uuid.uuid4())}
        self.op['arguments']={'reconcile_reservation_ids':[entry['reservation_id']],'acknowledge_execution_stop':True}
        self.op['execution_recovery_binding']={'generation':self.op['execution_binding'],'reservations':[entry]}
        self.infra.stop_compose=Mock()
        self.infra.run=Mock(return_value=b'0\n')
        admin=self.root/'admin-url'; admin.write_text('postgres://operator:fixture@db:5432/admin',encoding='utf-8')
        self.infra.config['database_admin_url_file']=str(admin)
        self.scope='cowork-'+uuid.UUID(self.instance['id']).hex
        self.container={'Id':'a'*64,'Name':'/'+self.scope+'-worker-1',
            'Config':{'Labels':{'com.docker.compose.project':self.scope}},
            'State':{'Running':False,'Paused':False,'Restarting':False}}
        self.inventory=Mock()
        self.inventory.inventory.return_value={'scope':self.scope,'resources':[{'database':'ci_'+'a'*32},{'database':'ci_'+'b'*32}]}
        self.inventory.docker.side_effect=[[self.container],[]]
        self.patch=patch.object(recovery,'InstanceDeletion',return_value=self.inventory)
        self.patch.start(); self.addCleanup(self.patch.stop)

    def marker(self): return self.directory/('execution-stop-'+uuid.UUID(self.op['id']).hex+'.json')

    def test_verified_stop_covers_all_generations_and_durable_receipt_never_reexecutes(self):
        receipt=recovery.stop(self.infra,self.op,self.directory)
        self.assertEqual(receipt['verified_generations'],2)
        queries=[call.args[0] for call in self.infra.run.call_args_list]
        self.assertEqual(queries[0],['docker','rm','a'*64])
        self.assertEqual(len([q for q in queries if q[0]=='psql']),2)
        self.infra.run.reset_mock(); self.infra.stop_compose.reset_mock()
        self.assertEqual(recovery.stop(self.infra,self.op,self.directory),receipt)
        self.infra.run.assert_not_called(); self.infra.stop_compose.assert_not_called()
        journal=OperationJournal(self.infra,self.op); journal.begin()
        result=journal.finish(True,{'receipt_id':self.op['id'],'execution_recovery':receipt})
        self.assertEqual(journal.recover(),result)

    def test_foreign_running_or_restarting_containers_and_survivors_never_prove_stop(self):
        for change in [lambda c:c['Config']['Labels'].update({'com.docker.compose.project':'foreign'}),
                       lambda c:c['State'].update({'Running':True}),lambda c:c['State'].update({'Restarting':True})]:
            bad=copy.deepcopy(self.container); change(bad)
            self.inventory.docker.side_effect=[[bad]]
            with self.assertRaises(epochs.ExecutionGenerationError): recovery.stop(self.infra,self.op,self.directory)
            self.assertFalse(self.marker().exists())
        self.inventory.docker.side_effect=[[self.container],[self.container]]
        with self.assertRaisesRegex(epochs.ExecutionGenerationError,'containers_remain'): recovery.stop(self.infra,self.op,self.directory)

    def test_retained_database_connection_or_changed_inventory_keeps_capacity_unproven(self):
        self.infra.run.return_value=b'1\n'
        with self.assertRaisesRegex(epochs.ExecutionGenerationError,'database_still_active'): recovery.stop(self.infra,self.op,self.directory)
        self.assertFalse(self.marker().exists())
        self.inventory.docker.side_effect=[[],[]]
        original=self.inventory.inventory.return_value
        self.inventory.inventory.side_effect=[original,{**original,'resources':[]}]
        with self.assertRaisesRegex(epochs.ExecutionGenerationError,'generation_changed'): recovery.stop(self.infra,self.op,self.directory)

    def test_selection_and_journal_evidence_cannot_be_substituted(self):
        for mutate in [lambda op:op['arguments'].update({'acknowledge_execution_stop':False}),
                       lambda op:op['execution_recovery_binding']['reservations'][0].update({'reservation_id':str(uuid.uuid4())}),
                       lambda op:op['execution_recovery_binding']['generation'].update({'instance_id':str(uuid.uuid4())})]:
            bad=copy.deepcopy(self.op); mutate(bad)
            with self.assertRaises(epochs.ExecutionGenerationError): recovery.binding(bad)
        journal=OperationJournal(self.infra,self.op); journal.begin()
        with self.assertRaises(epochs.ExecutionGenerationError): journal.finish(False,{'error_code':'unproven'})
        self.assertIsNone(journal.recover())

    def test_crash_before_marker_can_be_explicitly_verified_again_without_a_false_receipt(self):
        self.infra.private_write=Mock(side_effect=OSError('injected disk failure'))
        with self.assertRaises(OSError): recovery.stop(self.infra,self.op,self.directory)
        self.assertFalse(self.marker().exists())
        self.infra.private_write=p.private_write
        self.inventory.docker.side_effect=[[],[]]
        receipt=recovery.stop(self.infra,self.op,self.directory)
        self.assertTrue(receipt['stopped'])


if __name__=='__main__': unittest.main()
