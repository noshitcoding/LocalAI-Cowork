"""Rollback safety boundaries; Docker acceptance verifies actual data recovery."""
import copy
import json
import shutil
import unittest
import uuid
from unittest.mock import Mock, patch

import instance_rollbacks as rollbacks
import instance_updates as updates
import execution_generation as epochs
from release_catalog import ReleaseError, image_set_id
import test_instance_updates as fixture


class InstanceRollbackTests(unittest.TestCase):
    setUp = fixture.InstanceUpdateTests.setUp
    operation = fixture.InstanceUpdateTests.operation

    def test_rollback_to_old_release_still_creates_a_fresh_execution_epoch(self):
        self.infra.config.update({'backend':'compose','fleet_management_url':'http://management:8080',
                                 'execution_admission':{'server_images':[self.images['server']]}})
        self.infra.run.return_value=json.dumps(fixture.runtime.CONTRACT).encode()
        source=str(uuid.uuid4())
        self.instance['execution_epoch']=source
        epochs.write(self.infra,self.instance,self.directory,source)
        self.infra.write_compose(self.instance,self.directory)
        self.operation=lambda canary=True:fixture.InstanceUpdateTests.bound_operation(self,canary)
        operation=self.promoted()
        promoted=epochs.read(self.instance,self.infra.active_directory(self.directory))
        self.assertNotEqual(promoted,source)
        self.instance['execution_epoch']=promoted
        operation['instance']=copy.deepcopy(self.instance)
        operation['execution_binding']={'operation_id':operation['id'],'instance_id':self.instance['id'],
            'organisation_id':self.instance['organisation_id'],'source_epoch':promoted,'candidate_epoch':str(uuid.uuid4())}
        engine=rollbacks.InstanceRollbacks(self.infra); engine.probe=Mock()
        receipt=epochs.attach(self.infra,operation,engine.execute(operation,self.directory),True)
        self.assertEqual(receipt['execution']['active_epoch'],operation['execution_binding']['candidate_epoch'])
        self.assertNotIn(receipt['execution']['active_epoch'],[source,promoted])
        self.assertEqual(self.infra.deployed_images(self.directory),self.images)

    def promoted(self):
        engine = updates.InstanceUpdates(self.infra)
        engine.probe = Mock()
        engine.execute(self.operation(), self.directory)
        promotion = self.operation(False)
        receipt = engine.execute(promotion, self.directory)
        operation = {"id": str(uuid.uuid4()), "action": "rollback", "instance": copy.deepcopy(self.instance),
                     "arguments": {"rollback_operation_id": promotion["id"], "source_release": self.release["id"], "release": image_set_id(self.images)},
                     "backup": {"id": promotion["id"], "instance_id": self.instance["id"], "organisation_id": self.instance["organisation_id"], "sha256": receipt["backup_sha256"]}}
        self.backups.restored.clear()
        extra_patch = patch.object(rollbacks, "InstanceBackups", return_value=self.backups)
        extra_patch.start()
        self.addCleanup(extra_patch.stop)
        return operation

    def test_rollback_restores_backup_into_new_generation_and_backs_up_current_data(self):
        operation = self.promoted()
        engine = rollbacks.InstanceRollbacks(self.infra)
        engine.probe = Mock()
        self.infra.run.reset_mock()
        receipt = engine.execute(operation, self.directory)
        self.assertEqual(receipt["backup_id"], operation["id"])
        self.assertEqual(receipt["restored_backup_id"], operation["backup"]["id"])
        self.assertEqual(receipt["generation"], operation["id"])
        self.assertEqual(self.infra.deployed_images(self.directory), self.images)
        self.assertEqual(self.backups.restored, [(False, False)])
        self.assertFalse(any(call.kwargs.get("data") == updates.PROMOTE_SCHEDULES_SQL for call in self.infra.run.call_args_list))
        self.assertTrue((self.directory / "generations" / uuid.UUID(operation["backup"]["id"]).hex).is_dir())

    def test_rollback_rejects_foreign_scope_tampered_backup_and_unrelated_release(self):
        operation = self.promoted()
        engine = rollbacks.InstanceRollbacks(self.infra)
        self.infra.run.reset_mock()
        for group, key, value in [("instance", "organisation_id", str(uuid.uuid4())), ("arguments", "release", "d" * 64),
                                  ("backup", "sha256", "d" * 64), ("backup", "instance_id", str(uuid.uuid4()))]:
            bad = copy.deepcopy(operation)
            bad[group][key] = value
            with self.subTest(key=key), self.assertRaises(ReleaseError):
                engine.execute(bad, self.directory)
        self.infra.run.assert_not_called()

    def test_rollback_rejects_a_replaced_generation_even_with_same_images(self):
        operation = self.promoted()
        active = self.infra.active_directory(self.directory)
        new_id = str(uuid.uuid4())
        new_active = active.parent / uuid.UUID(new_id).hex
        shutil.copytree(active, new_active)
        (new_active / "generation.json").write_text(json.dumps({"generation": new_id}), encoding="utf-8")
        (self.directory / "active.json").write_text(json.dumps({"generation": new_id}), encoding="utf-8")
        with self.assertRaisesRegex(ReleaseError, "generation_changed"):
            rollbacks.InstanceRollbacks(self.infra).execute(operation, self.directory)

    def test_failed_rollback_preserves_newer_data_and_release(self):
        operation = self.promoted()
        active = self.infra.active_directory(self.directory)
        engine = rollbacks.InstanceRollbacks(self.infra)
        engine.probe = Mock(side_effect=RuntimeError("bad candidate"))
        with self.assertRaisesRegex(ReleaseError, "source_preserved") as error:
            engine.execute(operation, self.directory)
        self.assertEqual(error.exception.instance_state, "stopped")
        self.assertEqual(error.exception.result["backup_id"], operation["id"])
        self.assertEqual(self.infra.active_directory(self.directory), active)
        self.assertEqual(self.infra.deployed_images(self.directory), self.target)

    def test_interrupted_rollback_recovers_newer_source_without_replaying(self):
        operation = self.promoted()
        active = self.infra.active_directory(self.directory)
        engine = rollbacks.InstanceRollbacks(self.infra)
        engine.probe = Mock(side_effect=KeyboardInterrupt)
        with self.assertRaises(KeyboardInterrupt):
            engine.execute(operation, self.directory)
        self.assertEqual(self.infra.deployed_images(self.directory), self.images)
        engine.recover(operation)
        self.assertEqual(self.infra.active_directory(self.directory), active)
        self.assertEqual(self.infra.deployed_images(self.directory), self.target)
        self.assertEqual(self.backups.restored, [(False, False)])
        with self.assertRaisesRegex(ReleaseError, "started_without_completion"):
            engine.execute(operation, self.directory)

    def test_unproven_failed_rollback_requires_reconciliation(self):
        operation = self.promoted()
        engine = rollbacks.InstanceRollbacks(self.infra)
        engine.probe = Mock(side_effect=RuntimeError("bad candidate"))
        self.infra.stop_compose.side_effect = RuntimeError("cannot stop")
        with self.assertRaisesRegex(ReleaseError, "requires_review") as error:
            engine.execute(operation, self.directory)
        self.assertIsNone(error.exception.instance_state)
