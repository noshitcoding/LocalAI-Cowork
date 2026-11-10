"""Failure injection for the update transaction; real Docker acceptance is separate."""
import copy
import json
from pathlib import Path
import tempfile
import unittest
import uuid
from unittest.mock import Mock, patch

import instance_updates as updates
import provisioner as p
import execution_generation as epochs
import execution_runtime as runtime
from release_catalog import IMAGE_KEYS, ReleaseCatalog, ReleaseError, image_set_id, validate_release


class FakeBackups:
    def __init__(self, infrastructure):
        self.infra = infrastructure
        self.config = infrastructure.config
        self.quiescent = Mock()
        self.restored = []

    def backup(self, operation, directory, active):
        return {"backup_id": operation["id"], "backup_sha256": "c" * 64, "backup_bytes": 1234, "manifest_version": 1}

    def restore(self, operation, directory, active, *, activate=True, upgrade=False):
        self.restored.append((activate, upgrade))
        stage = directory / "generations" / uuid.UUID(operation["id"]).hex
        stage.mkdir(parents=True)
        epochs.stage(self.infra,operation,stage)
        (stage / "secrets").mkdir()
        (stage / "secrets" / "database_url").write_text("postgres://fixture:fixture@db:5432/fixture", encoding="utf-8")
        (stage / "generation.json").write_text(json.dumps({"generation": operation["id"], "launch_not_before": 1700000000}), encoding="utf-8")
        (stage / "images.json").write_text(json.dumps(self.config["images"]), encoding="utf-8")


class InstanceUpdateTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.images = {key: "registry.example/" + key + "@sha256:" + "a" * 64 for key in IMAGE_KEYS}
        self.target = {**self.images, "runner": "registry.example/runner@sha256:" + "b" * 64}
        self.instance = {"id": str(uuid.uuid4()), "organisation_id": str(uuid.uuid4()), "state": "stopped", "backend": "compose", "policy": {"minimum_isolation": "container", "limits": {"cpu_millis": 2000, "memory_mib": 4096, "storage_gib": 50, "concurrent_runs": 2}}}
        self.infra = p.Infrastructure({"state_directory": str(self.root / "instances"), "release_directory": str(self.root / "releases"), "images": self.images,
                                      "s3_endpoint": "http://storage:9000", "services_network": "services", "routing_network": "routing"})
        self.directory = self.infra.directory(self.instance["id"])
        self.infra.write_compose(self.instance, self.directory)
        self.original = (self.directory / "compose.json").read_bytes()
        self.infra.run = Mock(return_value=b"")
        self.infra.stop_compose = Mock()
        self.catalog = ReleaseCatalog(self.infra.config, p.private_write)
        self.release = self.catalog.publish("Runner admission", self.target, [image_set_id(self.images)])
        self.backups = FakeBackups(self.infra)
        self.patch = patch.object(updates, "InstanceBackups", return_value=self.backups)
        self.patch.start()
        self.addCleanup(self.patch.stop)

    def operation(self, canary=True):
        return {"id": str(uuid.uuid4()), "action": "upgrade", "instance": copy.deepcopy(self.instance),
                "arguments": {"release": self.release["id"], "source_release": image_set_id(self.images), "qualification_only": canary}}

    def bound_operation(self, canary=True):
        operation=InstanceUpdateTests.operation(self,canary)
        epoch=self.instance['execution_epoch']
        operation['execution_binding']={'operation_id':operation['id'],'instance_id':self.instance['id'],
            'organisation_id':self.instance['organisation_id'],'source_epoch':epoch,'candidate_epoch':str(uuid.uuid4())}
        return operation

    def test_execution_epoch_follows_canary_promotion_failure_and_explicit_recovery(self):
        self.infra.config.update({'backend':'compose','fleet_management_url':'http://management:8080',
                                 'execution_admission':{'server_images':[self.images['server']]}})
        self.infra.run.return_value=json.dumps(runtime.CONTRACT).encode()
        source=str(uuid.uuid4())
        self.instance['execution_epoch']=source
        epochs.write(self.infra,self.instance,self.directory,source)
        self.infra.write_compose(self.instance,self.directory)
        original=(self.directory/'compose.json').read_bytes()
        engine=updates.InstanceUpdates(self.infra)
        observations=[]
        def probe(instance,directory,active,*,worker):
            manifest=json.loads((directory/'compose.json').read_text(encoding='utf-8'))
            observed=[manifest['services'][s]['environment']['COWORK_EXECUTION_EPOCH'] for s in ['api','worker']]
            self.assertEqual(observed,[epochs.read(instance,active)]*2)
            observations.append(observed[0])
        engine.probe=probe
        canary=self.bound_operation()
        receipt=epochs.attach(self.infra,canary,engine.execute(canary,self.directory),True)
        self.assertEqual(receipt['execution']['active_epoch'],source)
        self.assertEqual(observations,[canary['execution_binding']['candidate_epoch'],source])
        self.assertEqual((self.directory/'compose.json').read_bytes(),original)
        interrupted=self.bound_operation(False)
        engine.probe=Mock(side_effect=KeyboardInterrupt)
        with self.assertRaises(KeyboardInterrupt): engine.execute(interrupted,self.directory)
        self.assertEqual(epochs.read(self.instance,self.infra.active_directory(self.directory)),interrupted['execution_binding']['candidate_epoch'])
        engine.recover(interrupted)
        result=epochs.attach(self.infra,interrupted,{'error_code':'operation_interrupted_resources_stopped','instance_state':'stopped'},False)
        self.assertEqual(result['execution']['active_epoch'],source)
        # Recovery reserializes the saved object, so key order can differ.
        self.assertEqual(json.loads((self.directory/'compose.json').read_bytes()),json.loads(original))
        failed=self.bound_operation(False)
        engine.probe=Mock(side_effect=RuntimeError('failed probe'))
        with self.assertRaisesRegex(ReleaseError,'source_preserved'): engine.execute(failed,self.directory)
        self.assertEqual(epochs.read(self.instance,self.directory),source)
        promoted=self.bound_operation(False)
        engine.probe=probe
        receipt=epochs.attach(self.infra,promoted,engine.execute(promoted,self.directory),True)
        self.assertEqual(receipt['execution']['active_epoch'],promoted['execution_binding']['candidate_epoch'])
        self.assertEqual(epochs.read(self.instance,self.directory),source)

    def test_manifest_is_immutable_and_only_digest_pinned_images_are_accepted(self):
        for bad in ["../escape", "latest", "A" * 64, "--privileged"]:
            with self.assertRaises(ReleaseError):
                self.catalog.release(bad, self.images)
        with self.assertRaisesRegex(ReleaseError, "immutable"):
            self.catalog.publish("Rewritten", self.target, [image_set_id(self.images)])
        with self.assertRaises(ReleaseError):
            image_set_id({**self.target, "runner": "image:latest"})
        tampered = {**self.release, "images": self.images}
        with self.assertRaisesRegex(ReleaseError, "digest_mismatch"):
            validate_release(tampered, self.release["id"])
        with self.assertRaisesRegex(ReleaseError, "unsupported_release_transition"):
            self.catalog.release(self.release["id"], self.target)

    def test_unqualified_release_cannot_change_an_instance(self):
        engine = updates.InstanceUpdates(self.infra)
        with self.assertRaisesRegex(ReleaseError, "missing"):
            engine.execute(self.operation(False), self.directory)
        self.infra.run.assert_not_called()
        self.assertEqual((self.directory / "compose.json").read_bytes(), self.original)

    def test_withdrawal_blocks_local_execution_and_catalog_publication(self):
        self.assertEqual(self.catalog.manifests(), [self.release])
        self.catalog.withdraw(self.release["id"])
        self.assertEqual(self.catalog.manifests(), [])
        with self.assertRaisesRegex(ReleaseError, "withdrawn"):
            updates.InstanceUpdates(self.infra).execute(self.operation(), self.directory)
        self.infra.run.assert_not_called()
        # Republishing the immutable document cannot undo a withdrawal.
        self.catalog.publish(self.release["name"], self.target, [image_set_id(self.images)])
        self.assertEqual(self.catalog.manifests(), [])

    def test_canary_qualifies_only_after_source_is_restored_and_probed(self):
        engine = updates.InstanceUpdates(self.infra)
        observations = []
        def probe(instance, directory, active, *, worker):
            observations.append((worker, self.infra.deployed_images(directory)))
            self.assertFalse(self.catalog.evidence_path(self.release["id"], image_set_id(self.images)).exists())
        engine.probe = probe
        receipt = engine.execute(self.operation(), self.directory)
        self.assertEqual(observations, [(True, self.target), (False, self.images)])
        self.assertEqual(self.backups.restored, [(False, True)])
        self.assertEqual((self.directory / "compose.json").read_bytes(), self.original)
        self.assertFalse((self.directory / "active.json").exists())
        self.assertTrue(receipt["rollback_verified"])
        self.catalog.require_qualification(self.release["id"], image_set_id(self.images))

    def test_failed_candidate_preserves_source_and_does_not_qualify(self):
        engine = updates.InstanceUpdates(self.infra)
        engine.probe = Mock(side_effect=RuntimeError("candidate failed"))
        operation = self.operation()
        with self.assertRaisesRegex(ReleaseError, "source_preserved") as raised:
            engine.execute(operation, self.directory)
        self.assertEqual(raised.exception.instance_state, "stopped")
        self.assertEqual(raised.exception.result["backup_id"], operation["id"])
        self.assertEqual((self.directory / "compose.json").read_bytes(), self.original)
        self.assertTrue((self.directory / "generations" / uuid.UUID(operation["id"]).hex).is_dir())
        self.assertFalse(self.catalog.evidence_path(self.release["id"], image_set_id(self.images)).exists())

    def test_canary_evidence_rejects_corruption_and_incomplete_transaction(self):
        engine = updates.InstanceUpdates(self.infra)
        engine.probe = Mock()
        operation = self.operation()
        engine.execute(operation, self.directory)
        path = self.catalog.evidence_path(self.release["id"], image_set_id(self.images))
        original = json.loads(path.read_text())
        for key, bad in [("version", True), ("probe_version", True), ("rollback_verified", 1),
                         ("operation_id", "../escape"), ("instance_id", str(uuid.uuid4()))]:
            path.write_text(json.dumps({**original, key: bad}), encoding="utf-8")
            with self.subTest(key=key), self.assertRaises(ReleaseError):
                self.catalog.require_qualification(self.release["id"], image_set_id(self.images))
        path.write_text(json.dumps(original), encoding="utf-8")
        marker = self.directory / ("upgrade-" + uuid.UUID(operation["id"]).hex + ".json")
        state = json.loads(marker.read_text())
        state["phase"] = "verifying_rollback"
        marker.write_text(json.dumps(state), encoding="utf-8")
        self.infra.run.reset_mock()
        with self.assertRaisesRegex(ReleaseError, "not_qualified"):
            engine.execute(self.operation(False), self.directory)
        self.infra.run.assert_not_called()

    def test_probe_is_detached_from_gateway_until_verified_stopped(self):
        engine = updates.InstanceUpdates(self.infra)
        for fails in [False, True]:
            def services(instance, directory, active, *, worker):
                manifest = json.loads((directory / "compose.json").read_bytes())
                self.assertNotIn("routing", manifest["services"]["api"]["networks"])
                self.assertIn("services", manifest["services"]["api"]["networks"])
                if fails:
                    raise ReleaseError("probe_failed")
            engine._probe_services = services
            if fails:
                with self.assertRaisesRegex(ReleaseError, "probe_failed"):
                    engine.probe(self.instance, self.directory, self.directory, worker=True)
            else:
                engine.probe(self.instance, self.directory, self.directory, worker=True)
            self.infra.stop_compose.assert_called_with(self.instance, self.directory)
            self.backups.quiescent.assert_called_with(self.instance, self.directory)
            self.assertEqual((self.directory / "compose.json").read_bytes(), self.original)

    def test_failed_rollback_is_not_reported_as_startable(self):
        engine = updates.InstanceUpdates(self.infra)
        engine.probe = Mock(side_effect=RuntimeError("candidate failed"))
        self.infra.stop_compose.side_effect = RuntimeError("cannot prove stop")
        with self.assertRaisesRegex(ReleaseError, "rollback_requires_review") as raised:
            engine.execute(self.operation(), self.directory)
        self.assertIsNone(raised.exception.instance_state)
        self.assertFalse(self.catalog.evidence_path(self.release["id"], image_set_id(self.images)).exists())

    def test_qualified_update_activates_fresh_generation_and_restores_schedules(self):
        engine = updates.InstanceUpdates(self.infra)
        engine.probe = Mock()
        engine.execute(self.operation(), self.directory)
        operation = self.operation(False)
        receipt = engine.execute(operation, self.directory)
        self.assertEqual(receipt["generation"], operation["id"])
        self.assertEqual(self.infra.deployed_images(self.directory), self.target)
        self.assertEqual(json.loads((self.directory / "images.json").read_text()), self.images)
        self.assertEqual(self.infra.active_directory(self.directory).name, uuid.UUID(operation["id"]).hex)
        self.assertTrue(any(call.kwargs.get("data") == updates.PROMOTE_SCHEDULES_SQL for call in self.infra.run.call_args_list))

    def test_incomplete_operation_never_replays_staging(self):
        engine = updates.InstanceUpdates(self.infra)
        operation = self.operation()
        self.backups.backup = Mock(side_effect=KeyboardInterrupt)
        with self.assertRaises(KeyboardInterrupt):
            engine.execute(operation, self.directory)
        with self.assertRaisesRegex(ReleaseError, "started_without_completion"):
            engine.execute(operation, self.directory)
        self.assertEqual(self.backups.backup.call_count, 1)

    def test_explicit_recovery_restores_source_after_interruption_without_replaying(self):
        engine = updates.InstanceUpdates(self.infra)
        operation = self.operation()
        engine.probe = Mock(side_effect=KeyboardInterrupt)
        with self.assertRaises(KeyboardInterrupt):
            engine.execute(operation, self.directory)
        self.assertEqual(self.infra.deployed_images(self.directory), self.target)
        engine.recover(operation)
        self.assertEqual(self.infra.deployed_images(self.directory), self.images)
        self.assertEqual(self.infra.active_directory(self.directory), self.directory)
        self.assertEqual(self.backups.restored, [(False, True)])
        self.assertEqual(engine.probe.call_count, 1)
        engine.recover(operation)  # Retry reconciliation, never staging.
        self.assertEqual(self.backups.restored, [(False, True)])

    def test_recovery_refuses_changed_scope_and_source(self):
        engine = updates.InstanceUpdates(self.infra)
        operation = self.operation()
        engine.probe = Mock(side_effect=KeyboardInterrupt)
        with self.assertRaises(KeyboardInterrupt):
            engine.execute(operation, self.directory)
        for key, value in [("source_release", "c" * 64), ("release", "d" * 64)]:
            altered = copy.deepcopy(operation)
            altered["arguments"][key] = value
            with self.assertRaises(ReleaseError):
                engine.recover(altered)
        self.assertEqual(self.infra.deployed_images(self.directory), self.target)

    def test_changed_installed_release_is_rejected_before_infrastructure(self):
        operation = self.operation()
        operation["arguments"]["source_release"] = "d" * 64
        with self.assertRaisesRegex(ReleaseError, "installed_release_changed"):
            updates.InstanceUpdates(self.infra).execute(operation, self.directory)
        self.infra.run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
