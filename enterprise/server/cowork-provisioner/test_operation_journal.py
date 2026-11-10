import copy
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import uuid
from unittest.mock import Mock, patch
import provisioner as p
from operation_journal import OperationJournal, JournalError, operator_lock, operator_pass_fds
from fleet_images import pinned_fleet_images, retain_fleet_images, IMAGE_NAMES


class RecoveryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.infra = p.Infrastructure({"state_directory": str(self.root), "backend": "compose"})
        self.op = {"id": str(uuid.uuid4()), "organisation_id": str(uuid.uuid4()), "instance_id": str(uuid.uuid4()),
                   "action": "start", "arguments": {}, "request_digest": "a" * 64, "lease_token": str(uuid.uuid4())}
        self.op["instance"] = {"id": self.op["instance_id"], "organisation_id": self.op["organisation_id"], "backend": "compose", "state": "stopped", "policy": {"minimum_isolation": "container"}}
        self.journal = OperationJournal(self.infra, self.op)

    def test_success_and_failure_survive_new_process_without_executing_again(self):
        for succeeded in [True, False]:
            expected = self.journal.finish(succeeded, {"receipt_id": self.op["id"]} if succeeded else {"error_code": "infrastructure_command_failed:docker"})
            script = 'import json,sys;from provisioner import Infrastructure;from operation_journal import OperationJournal; print(json.dumps(OperationJournal(Infrastructure(json.loads(sys.argv[1])),json.loads(sys.argv[2])).recover()))'
            actual = json.loads(subprocess.check_output([sys.executable, "-c", script, json.dumps(self.infra.config), json.dumps(self.op)], cwd=Path(__file__).parent))
            self.assertEqual(actual, expected)
            manager = Mock()
            manager.call.return_value = [self.op]
            with patch.object(self.infra, "execute", side_effect=AssertionError("must never replay")):
                p.reconcile_operations(manager, self.infra)
            self.assertEqual(manager.call.call_args.args, (f"/operations/{self.op['id']}/complete", expected))

    def test_incomplete_start_is_never_reexecuted_or_declared_failed(self):
        self.journal.begin()
        with self.assertRaisesRegex(JournalError, "started_without_completion"):
            self.journal.begin()
        manager = Mock()
        manager.call.return_value = [self.op]
        with patch.object(self.infra, "execute", side_effect=AssertionError("replay")), patch.object(self.infra, "stop_compose", side_effect=AssertionError("unapproved stop")):
            p.reconcile_operations(manager, self.infra)
        self.assertEqual(manager.call.call_args.args[1]["reason"], "operation_completion_unproven")
        self.assertIsNone(self.journal.recover())

    def test_legacy_completion_gap_and_receipt_scope(self):
        self.journal.begin()
        self.infra.private_write(self.journal.legacy_receipt, json.dumps({"receipt_id": str(uuid.uuid4())}).encode())
        with self.assertRaisesRegex(JournalError, "receipt_mismatch"):
            self.journal.recover()
        self.infra.private_write(self.journal.legacy_receipt, json.dumps({"receipt_id": self.op["id"]}).encode())
        self.assertTrue(self.journal.recover()["succeeded"])

    def test_changed_scope_arguments_lease_or_request_cannot_reuse_receipt(self):
        self.journal.finish(True, {"receipt_id": self.op["id"]})
        for field, value in [("lease_token", str(uuid.uuid4())), ("arguments", {"backup_id": str(uuid.uuid4())}), ("action", "restore"), ("request_digest", "b" * 64)]:
            altered = copy.deepcopy(self.op); altered[field] = value
            with self.assertRaisesRegex(JournalError, "binding_mismatch"):
                OperationJournal(self.infra, altered).recover()
        altered = copy.deepcopy(self.op); altered["organisation_id"] = str(uuid.uuid4())
        with self.assertRaisesRegex(JournalError, "scope_mismatch"):
            OperationJournal(self.infra, altered)

    def test_receipt_wins_over_recovery_stop(self):
        self.op["recovery_authorized"] = True
        self.journal.finish(True, {"receipt_id": self.op["id"]})
        manager = Mock(); manager.call.return_value = [self.op]
        with patch.object(self.infra, "stop_compose", side_effect=AssertionError("receipt wins")):
            p.reconcile_operations(manager, self.infra)
        self.assertTrue(manager.call.call_args.args[1]["succeeded"])

    def test_corrupt_journal_requires_review_even_when_stop_authorized(self):
        self.op["recovery_authorized"] = True
        self.journal.path.write_text('[]')
        manager = Mock(); manager.call.return_value = [self.op]
        with patch.object(self.infra, "stop_compose", side_effect=AssertionError("corrupt evidence")):
            p.reconcile_operations(manager, self.infra)
        self.assertEqual(manager.call.call_args.args[1]["reason"], "operation_recovery_requires_review")

    def test_unsafe_update_failure_keeps_incomplete_journal_for_reconciliation(self):
        self.op["action"] = "upgrade"
        manager = Mock()
        manager.call.side_effect = [[], self.op, {}]
        with patch.object(p, "Manager", return_value=manager), patch.object(p, "Infrastructure", return_value=self.infra), patch.object(self.infra, "execute", side_effect=p.ProvisioningError("upgrade_rollback_requires_review")):
            p.work(self.infra.config, once=True)
        self.assertIsNone(OperationJournal(self.infra, self.op).recover())
        self.assertTrue(manager.call.call_args.args[0].endswith('/reconciliation'))

    def test_recovery_only_reports_stopped_after_quiescence_check(self):
        self.op["recovery_authorized"] = True
        manager = Mock(); manager.call.return_value = [self.op]
        with patch.object(self.infra, "deployed_images"), patch.object(self.infra, "stop_compose") as stop, patch.object(self.infra, "write_compose"), patch.object(self.infra, "verify_stopped", side_effect=p.ProvisioningError("busy")):
            p.reconcile_operations(manager, self.infra)
            stop.assert_called_once()
        self.assertIsNone(self.journal.recover())
        self.assertEqual(manager.call.call_args.args[1]["reason"], "operation_recovery_requires_review")
        with patch.object(self.infra, "deployed_images"), patch.object(self.infra, "stop_compose"), patch.object(self.infra, "write_compose"), patch.object(self.infra, "verify_stopped"):
            p.reconcile_operations(manager, self.infra)
        self.assertFalse(self.journal.recover()["succeeded"])
        self.assertEqual(self.journal.recover()["result"]["instance_state"], "stopped")

    @unittest.skipUnless(os.name == "posix", "Linux advisory lock")
    def test_lock_excludes_another_process_and_is_released_on_exit(self):
        script = 'import sys;from pathlib import Path;from operation_journal import operator_lock;\nwith operator_lock(Path(sys.argv[1])): print("acquired")'
        with operator_lock(self.root):
            result = subprocess.run([sys.executable, "-c", script, str(self.root)], capture_output=True, cwd=Path(__file__).parent)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(b"another_operator_owns_state_directory", result.stderr)
        result = subprocess.run([sys.executable, "-c", script, str(self.root)], capture_output=True, cwd=Path(__file__).parent)
        self.assertEqual(result.returncode, 0)

    @unittest.skipUnless(os.name == "posix", "Linux advisory lock")
    def test_surviving_child_keeps_lock_after_owner_closes_it(self):
        with operator_lock(self.root):
            child = subprocess.Popen([sys.executable, "-c", "import sys;sys.stdin.read()"], stdin=subprocess.PIPE, pass_fds=operator_pass_fds())
        try:
            with self.assertRaisesRegex(JournalError, "another_operator_owns_state_directory"):
                with operator_lock(self.root):
                    self.fail("infrastructure child is still alive")
        finally:
            child.communicate(timeout=10)
        with operator_lock(self.root):
            pass

    def test_management_rebuild_never_advances_fleet_release(self):
        original = {name: "cowork@sha256:" + "a" * 64 for name in IMAGE_NAMES}
        (self.root / "config.json").write_text(json.dumps({"images": original}))
        resolve = Mock(side_effect=AssertionError("must preserve deployed release"))
        self.assertEqual(pinned_fleet_images(self.root, resolve, p.immutable_image, p.private_write), original)
        (self.root / "config.json").write_text(json.dumps({"images": {name: "cowork@sha256:" + "b" * 64 for name in IMAGE_NAMES}}))
        self.assertEqual(pinned_fleet_images(self.root, resolve, p.immutable_image, p.private_write), original)
        (self.root / "image-set.json").write_text(json.dumps({name: "cowork:latest" for name in IMAGE_NAMES}))
        with self.assertRaisesRegex(p.ProvisioningError, "digest_pinned"):
            pinned_fleet_images(self.root, resolve, p.immutable_image, p.private_write)

    def test_retention_tags_cannot_alias_two_releases(self):
        run = Mock()
        images = {"server": "cowork-server@sha256:" + "a" * 64, "runner": "cowork-runner@sha256:" + "b" * 64}
        retain_fleet_images(images, run, p.immutable_image)
        targets = {call.args[0][-1] for call in run.call_args_list}
        self.assertEqual(targets, {"cowork-server:retained-sha256-" + "a" * 64, "cowork-runner:retained-sha256-" + "b" * 64})
        with self.assertRaises(p.ProvisioningError):
            retain_fleet_images({"server": "cowork-server:latest"}, run, p.immutable_image)


if __name__ == "__main__":
    unittest.main()
