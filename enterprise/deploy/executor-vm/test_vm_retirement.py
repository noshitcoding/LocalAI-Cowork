"""Explicit retirement uses only own temporary roots; no VM or real pool.

The quiet checks inspect our own public /proc process metadata and open our own
UNIX sockets. No guest, Docker command, private runtime key or download is used.
"""
import copy
import json
import os
from pathlib import Path
import socket
import unittest
from unittest import mock
import uuid

import local_executor_vm as vm
import test_capacity_pool as pool_tests

capacity = vm.capacity_pool


@unittest.skipUnless(os.name == "posix" and os.getuid() != 0,
                     "requires an unprivileged POSIX user")
class RetirementTests(unittest.TestCase):
    def setUp(self):
        pool_tests.CapacityPoolTests.setUp(self)
        # A readable proc fixture contains the real test process descriptors and
        # mappings. It does not claim full actual account coverage. Production
        # must refuse any other own-account process whose evidence is unreadable.
        self.proc = self.base / "readable-proc"
        (self.proc / "self").mkdir(parents=True)
        (self.proc / "self/mountinfo").symlink_to("/proc/self/mountinfo")
        process = self.proc / str(os.getpid())
        process.mkdir()
        for name in ("status", "fd", "cwd", "maps"):
            (process / name).symlink_to("/proc/self/" + name)
        quiet = vm.retirement_quiet
        patch = mock.patch.object(vm, "retirement_quiet", side_effect=lambda record, **kwargs:
                                  quiet(record, proc_root=kwargs.get("proc_root", self.proc)))
        patch.start()
        self.addCleanup(patch.stop)
    configure = pool_tests.CapacityPoolTests.configure
    ledger = pool_tests.CapacityPoolTests.ledger
    reserve = pool_tests.CapacityPoolTests.reserve
    ready = pool_tests.CapacityPoolTests.ready

    def fixture(self):
        self.configure()
        root, value = self.reserve()
        self.ready(root, value)
        vm.write(root / "disk.qcow2", b"own-test-disk")
        (root / "shared").mkdir(mode=0o700)
        vm.write(root / "shared" / "marker", b"private-test-content-is-never-in-the-journal")
        return root, value, vm.retirement_metadata_sha(root)

    def invoke(self, root, value, sha, revision=None):
        return vm.retire(root, value["id"], sha, revision or self.ledger()["revision"])

    def held(self):
        with capacity.locked_pool() as pool:
            return pool.totals()

    def assert_held(self):
        self.assertEqual(self.held(), {"cpu_millis": 4100, "memory_mib": 4352})

    def pending(self, value):
        return self.ledger()["retirements"][value["id"]]

    def pause_before_delete(self, root, value, sha):
        with mock.patch.object(vm, "retirement_delete", side_effect=RuntimeError("injected_crash")), \
                self.assertRaisesRegex(RuntimeError, "injected_crash"):
            self.invoke(root, value, sha)
        self.assertEqual(self.pending(value)["state"], "deleting")
        self.assert_held()
        return Path(self.pending(value)["quarantine"])

    def test_delete_proves_exact_root_then_totals_and_idempotent_tombstone(self):
        root, value, sha = self.fixture()
        self.assert_held()
        with self.assertRaisesRegex(RuntimeError, "parent_capacity_exhausted"):
            self.reserve(self.second)
        result = self.invoke(root, value, sha)
        self.assertFalse(root.exists())
        self.assertFalse(Path(self.pending(value)["quarantine"]).exists())
        self.assertEqual(result["held"], {"cpu_millis": 0, "memory_mib": 0})
        self.assertEqual(self.ledger()["allocations"], {})
        tombstone = self.pool_root / ("retirement-" + uuid.UUID(result["retirement_id"]).hex + ".json")
        before = tombstone.read_bytes()
        self.assertNotIn(b"private-test-content", before)
        second = self.invoke(root, value, sha)
        self.assertTrue(second["already_retired"])
        self.assertEqual(second["retirement_id"], result["retirement_id"])
        self.assertEqual(tombstone.read_bytes(), before)
        # Exactly one new charge can now be reserved; a stop never did this.
        new_root, _ = self.reserve(self.second)
        self.assertTrue(new_root.exists())
        self.assert_held()

    def test_wrong_id_revision_and_hash_do_not_mutate_or_remove(self):
        root, value, sha = self.fixture()
        before = self.ledger()
        for ident, digest, revision, reason in (
                (str(uuid.uuid4()), sha, 2, "retirement_vm_identity_mismatch"),
                (value["id"], "b" * 64, 2, "retirement_metadata_changed"),
                (value["id"], sha, 1, "capacity_revision_conflict")):
            with self.subTest(reason=reason), self.assertRaisesRegex(RuntimeError, reason):
                vm.retire(root, ident, digest, revision)
            self.assertEqual(self.ledger(), before)
            self.assertTrue((root / "disk.qcow2").exists())

    def test_explicit_cli_requires_all_identity_fields(self):
        root, _, _ = self.fixture()
        with mock.patch.object(vm.sys, "argv", ["vm", "retire", "--root", str(root)]), \
                self.assertRaisesRegex(RuntimeError, "retirement_exact_identity_required"):
            vm.main()
        self.assertTrue(root.exists())
        self.assert_held()

    def test_adopted_v2_root_never_retires(self):
        root, value, sha = self.fixture()
        with capacity.locked_pool() as pool:
            pool.data["allocations"][value["id"]]["adopted"] = True
            pool.save()
        before = self.ledger()
        with self.assertRaisesRegex(RuntimeError, "retirement_adopted_vm_forbidden"):
            self.invoke(root, value, sha)
        self.assertEqual(self.ledger(), before)
        self.assertTrue(root.exists())

    def test_unregistered_and_changed_metadata_are_denied(self):
        root, value, sha = self.fixture()
        other = self.first / (vm.VM_PREFIX + uuid.uuid4().hex)
        other.mkdir(mode=0o700)
        with self.assertRaisesRegex(RuntimeError, "retirement_vm_not_registered"):
            vm.retire(other, str(uuid.UUID(other.name[len(vm.VM_PREFIX):])), sha, 2)
        other.rmdir()
        vm.atomic_json(root / "vm.json", {**value, "operator_state_id": str(uuid.uuid4())})
        with self.assertRaisesRegex(RuntimeError, "capacity_vm_binding_changed"):
            self.invoke(root, value, sha)
        self.assert_held()

    def test_symlink_hardlink_and_writable_foreign_entries_refuse(self):
        root, value, sha = self.fixture()
        foreign = self.base / "foreign"
        vm.write(foreign, b"must-not-delete")
        link = root / "unsafe"
        for kind in ("symlink", "hardlink", "writable"):
            if kind == "symlink":
                link.symlink_to(foreign)
            elif kind == "hardlink":
                os.link(foreign, link)
            else:
                vm.write(link, b"own-file")
                link.chmod(0o666)
            with self.subTest(kind=kind), self.assertRaisesRegex(RuntimeError, "retirement_"):
                self.invoke(root, value, sha)
            link.unlink()
            self.assertEqual(foreign.read_bytes(), b"must-not-delete")
            self.assert_held()

    def test_recorded_live_or_reused_foreign_pid_is_not_stopped(self):
        root, value, sha = self.fixture()
        vm.write(root / "qemu.pid", str(os.getpid()))
        with self.assertRaisesRegex(RuntimeError, "retirement_recorded_process_alive"):
            self.invoke(root, value, sha)
        self.assertTrue(root.exists())
        self.assert_held()

    def test_open_disk_fd_is_positive_physical_refusal(self):
        root, value, sha = self.fixture()
        with (root / "disk.qcow2").open("rb"), \
                self.assertRaisesRegex(RuntimeError, "retirement_disk_or_directory_in_use"):
            self.invoke(root, value, sha)
        self.assertTrue(root.exists())
        self.assert_held()

    def test_live_unknown_qmp_socket_is_refused_without_commands(self):
        root, value, sha = self.fixture()
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
            sock.bind(str(root / "qmp.sock"))
            sock.listen()
            with self.assertRaisesRegex(RuntimeError, "retirement_qmp_listener_alive"):
                self.invoke(root, value, sha)
            peer, _ = sock.accept()
            with peer:
                self.assertEqual(peer.recv(1), b"")
        self.assertTrue(root.exists())
        self.assert_held()

    def test_dead_socket_and_gone_pid_can_retire(self):
        root, value, sha = self.fixture()
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
            sock.bind(str(root / "qmp.sock"))
        vm.write(root / "qemu.pid", "2147483647")
        self.assertFalse((Path("/proc") / "2147483647").exists())
        self.assertEqual(self.invoke(root, value, sha)["held"]["cpu_millis"], 0)

    def test_missing_or_inaccessible_own_process_evidence_fails_closed(self):
        root, value, sha = self.fixture()
        quarantine = self.pause_before_delete(root, value, sha)
        proc = self.base / "fake-proc"
        (proc / "self").mkdir(parents=True)
        (proc / "self/mountinfo").write_text("1 0 0:1 / / rw - tmpfs none rw\n")
        own = proc / "123"
        own.mkdir()
        (own / "status").write_text("State:\tS (sleeping)\nUid:\t" + "\t".join([str(os.getuid())] * 4) + "\n")
        with self.assertRaisesRegex(RuntimeError, "retirement_process_inventory_unknown"):
            vm.retirement_quiet(self.pending(value), proc_root=proc)
        self.assertTrue(quarantine.exists())
        self.assert_held()

    def test_mount_inventory_detects_bound_subtree(self):
        root, value, sha = self.fixture()
        self.pause_before_delete(root, value, sha)
        proc = self.base / "fake-proc"
        (proc / "self").mkdir(parents=True)
        path = Path(self.pending(value)["quarantine"]) / "shared"
        (proc / "self/mountinfo").write_text("1 0 0:1 / " + str(path) + " rw - tmpfs none rw\n")
        with self.assertRaisesRegex(RuntimeError, "retirement_mount_not_allowed"):
            vm.retirement_quiet(self.pending(value), proc_root=proc)
        self.assert_held()

    def test_pending_blocks_start_create_and_configure_without_early_release(self):
        root, value, sha = self.fixture()
        self.pause_before_delete(root, value, sha)
        with capacity.locked_pool() as pool:
            for action in (lambda: pool.require_registered(root, value),
                           lambda: pool.configure(self.plan, pool.data["revision"]),
                           lambda: pool.check_inventory(vm.load)):
                with self.assertRaisesRegex(RuntimeError, "capacity_retirement_in_progress"):
                    action()
        with self.assertRaisesRegex(RuntimeError, "capacity_retirement_in_progress"):
            self.reserve(self.second)
        self.assert_held()

    def test_crash_before_pending_write_leaves_original_and_full_charge(self):
        root, value, sha = self.fixture()
        atomic = capacity.atomic_json
        def fail(path, data):
            if path.name == "retirement-pending.json":
                raise RuntimeError("injected_crash")
            return atomic(path, data)
        with mock.patch.object(capacity, "atomic_json", side_effect=fail), \
                self.assertRaisesRegex(RuntimeError, "injected_crash"):
            self.invoke(root, value, sha)
        self.assertNotIn("retirements", self.ledger())
        self.assertTrue(root.exists())
        self.assert_held()
        self.invoke(root, value, sha)

    def wal_crash(self, target, *, after=False, release=False):
        root, value, sha = self.fixture()
        atomic = capacity.atomic_json
        def fail(path, data):
            # identity fence does not carry the record; its revision identifies
            # prepared (3) versus the final retired transition (5).
            selected = data.get("revision") == (5 if release else 3)
            if path.name == target and selected:
                if after:
                    atomic(path, data)
                raise RuntimeError("injected_crash")
            return atomic(path, data)
        with mock.patch.object(capacity, "atomic_json", side_effect=fail), \
                self.assertRaisesRegex(RuntimeError, "injected_crash"):
            self.invoke(root, value, sha)
        wal = capacity.read_private_json(self.pool_root / "retirement-pending.json")
        self.assertEqual(wal["after"]["revision"], 5 if release else 3)
        with self.assertRaisesRegex(RuntimeError, "capacity_retirement_recovery_required"):
            self.held()
        # Wrong CAS cannot perform even the journal recovery.
        with self.assertRaisesRegex(RuntimeError, "capacity_revision_conflict"):
            self.invoke(root, value, sha, 2)
        self.assertTrue((self.pool_root / "retirement-pending.json").exists())
        result = self.invoke(root, value, sha, wal["after"]["revision"])
        self.assertEqual(result["held"], {"cpu_millis": 0, "memory_mib": 0})
        self.assertFalse((self.pool_root / "retirement-pending.json").exists())
        self.assertFalse(root.exists())

    def test_wal_crash_before_anchor_is_resumable(self):
        self.wal_crash("identity.json")

    def test_wal_crash_after_anchor_before_ledger_is_resumable(self):
        self.wal_crash("pool.json")

    def test_wal_crash_after_ledger_before_journal_removal_is_resumable(self):
        self.wal_crash("pool.json", after=True)

    def test_release_wal_crash_keeps_charge_until_explicit_recovery(self):
        self.wal_crash("pool.json", release=True)

    def test_crash_after_rename_before_phase_commit_resumes_same_inode(self):
        root, value, sha = self.fixture()
        commit = capacity.CapacityPool.commit_retirement
        def fail(pool, ident, record):
            if record["state"] == "deleting":
                raise RuntimeError("injected_crash")
            return commit(pool, ident, record)
        with mock.patch.object(capacity.CapacityPool, "commit_retirement", new=fail), \
                self.assertRaisesRegex(RuntimeError, "injected_crash"):
            self.invoke(root, value, sha)
        record = self.pending(value)
        self.assertEqual(record["state"], "prepared")
        self.assertFalse(root.exists())
        self.assertTrue(Path(record["quarantine"]).exists())
        self.assert_held()
        self.invoke(root, value, sha)

    def test_crash_mid_unlink_without_positive_witness_stays_unknown_and_held(self):
        root, value, sha = self.fixture()
        unlink = os.unlink
        def fail(path, *args, **kwargs):
            result = unlink(path, *args, **kwargs)
            if path == "marker" and kwargs.get("dir_fd") is not None:
                raise RuntimeError("injected_crash")
            return result
        with mock.patch.object(vm.os, "unlink", side_effect=fail), \
                self.assertRaisesRegex(RuntimeError, "injected_crash"):
            self.invoke(root, value, sha)
        record = self.pending(value)
        quarantine = Path(record["quarantine"])
        self.assertEqual(record["state"], "deleting")
        self.assertFalse((quarantine / "shared/marker").exists())
        self.assertTrue((quarantine / "disk.qcow2").exists())
        self.assert_held()
        with self.assertRaisesRegex(RuntimeError, "retirement_delete_proof_missing_unknown"):
            self.invoke(root, value, sha)
        self.assert_held()

    def test_crash_after_rmdir_before_parent_fsync_remains_unknown_and_held(self):
        root, value, sha = self.fixture()
        parent_ino = root.parent.stat().st_ino
        fsync = os.fsync
        def fail(fd):
            if os.fstat(fd).st_ino == parent_ino and not root.exists() \
                    and not any(root.parent.glob(".retire-*")):
                raise RuntimeError("injected_crash")
            return fsync(fd)
        with mock.patch.object(vm.os, "fsync", side_effect=fail), \
                self.assertRaisesRegex(RuntimeError, "injected_crash"):
            self.invoke(root, value, sha)
        self.assertEqual(self.pending(value)["state"], "deleting")
        self.assert_held()
        with self.assertRaisesRegex(RuntimeError, "retirement_delete_proof_missing_unknown"):
            self.invoke(root, value, sha)
        self.assert_held()

    def test_crash_after_physical_delete_before_tombstone_keeps_charge(self):
        root, value, sha = self.fixture()
        with mock.patch.object(capacity, "retirement_tombstone", side_effect=RuntimeError("injected_crash")), \
                self.assertRaisesRegex(RuntimeError, "injected_crash"):
            self.invoke(root, value, sha)
        self.assertEqual(self.pending(value)["state"], "deleting")
        self.assertFalse(root.exists())
        self.assertFalse(Path(self.pending(value)["quarantine"]).exists())
        self.assert_held()
        self.invoke(root, value, sha)

    def test_reappearing_original_or_unexpected_quarantine_files_keep_charge(self):
        root, value, sha = self.fixture()
        quarantine = self.pause_before_delete(root, value, sha)
        root.mkdir(mode=0o700)
        vm.write(root / "foreign", b"must-not-delete")
        with self.assertRaisesRegex(RuntimeError, "retirement_original_root_reappeared"):
            self.invoke(root, value, sha)
        self.assertEqual((root / "foreign").read_bytes(), b"must-not-delete")
        (root / "foreign").unlink()
        root.rmdir()
        vm.write(quarantine / "unexpected", b"must-not-delete")
        with self.assertRaisesRegex(RuntimeError, "retirement_inventory_changed"):
            self.invoke(root, value, sha)
        self.assertTrue((quarantine / "unexpected").exists())
        self.assert_held()

    def test_changed_or_symlink_quarantine_never_deletes_foreign_root(self):
        root, value, sha = self.fixture()
        quarantine = self.pause_before_delete(root, value, sha)
        hidden = self.first / "held-original"
        quarantine.rename(hidden)
        quarantine.symlink_to(self.second, target_is_directory=True)
        with self.assertRaisesRegex(RuntimeError, "unsafe_capacity_path|capacity_path_must_be_canonical"):
            self.invoke(root, value, sha)
        self.assertTrue(hidden.exists())
        self.assertTrue(self.second.exists())
        self.assert_held()

    def test_missing_identity_anchor_never_releases_or_deletes(self):
        root, value, sha = self.fixture()
        self.pause_before_delete(root, value, sha)
        (self.pool_root / "identity.json").unlink()
        with self.assertRaisesRegex(RuntimeError, "capacity_identity_missing"):
            self.invoke(root, value, sha)
        self.assertTrue(Path(self.pending(value)["quarantine"]).exists())
        self.assertEqual(len(self.ledger()["allocations"]), 1)

    def test_retired_id_state_and_root_can_never_be_rebound(self):
        root, value, sha = self.fixture()
        self.invoke(root, value, sha)
        with capacity.locked_pool() as pool:
            with self.assertRaisesRegex(RuntimeError, "capacity_retired_binding_reuse"):
                pool.reserve(root, value)
        root.mkdir(mode=0o700)
        with self.assertRaisesRegex(RuntimeError, "capacity_retired_root_reappeared"):
            self.held()
        self.assertTrue(root.exists())

    def test_tombstone_loss_or_tamper_blocks_future_allocation(self):
        root, value, sha = self.fixture()
        result = self.invoke(root, value, sha)
        path = self.pool_root / ("retirement-" + uuid.UUID(result["retirement_id"]).hex + ".json")
        original = path.read_bytes()
        modified = json.loads(original)
        modified["allocation"]["cpu_millis"] -= 1
        path.write_text(json.dumps(modified))
        with self.assertRaisesRegex(RuntimeError, "capacity_retirement_tombstone_changed"):
            self.reserve(self.second)
        path.write_bytes(original)
        path.unlink()
        with self.assertRaises(FileNotFoundError):
            self.reserve(self.second)

    def test_wal_cannot_remove_unrelated_allocation_or_change_frozen_binding(self):
        root, value, sha = self.fixture()
        self.pause_before_delete(root, value, sha)
        before = self.ledger()
        after = copy.deepcopy(before)
        after["revision"] += 1
        record = after["retirements"][value["id"]]
        record["state"] = "retired"
        record["metadata_sha256"] = "f" * 64
        del after["allocations"][value["id"]]
        with self.assertRaisesRegex(RuntimeError, "invalid_retirement_"):
            capacity.retirement_transition(before, after, value["id"])
        self.assert_held()

    def test_replacement_root_inode_is_not_the_frozen_vm(self):
        root, value, sha = self.fixture()
        quarantine = self.pause_before_delete(root, value, sha)
        preserved = self.first / "original-frozen-root"
        quarantine.rename(preserved)
        quarantine.mkdir(mode=0o700)
        vm.write(quarantine / "foreign-data", b"preserve-foreign-root")
        with self.assertRaisesRegex(RuntimeError, "retirement_root_binding_changed"):
            self.invoke(root, value, sha)
        self.assertTrue((preserved / "disk.qcow2").exists())
        self.assertEqual((quarantine / "foreign-data").read_bytes(), b"preserve-foreign-root")
        self.assert_held()

    def test_map_inode_proves_disk_use_even_after_renaming_outside_root(self):
        root, value, sha = self.fixture()
        self.pause_before_delete(root, value, sha)
        record = self.pending(value)
        proc = self.base / "mapped-proc"
        (proc / "self").mkdir(parents=True)
        (proc / "self/mountinfo").write_text("1 0 0:1 / / rw - tmpfs none rw\n")
        own = proc / "123"
        (own / "fd").mkdir(parents=True)
        (own / "status").write_text("State:\tS (sleeping)\nUid:\t" + "\t".join([str(os.getuid())] * 4) + "\n")
        (own / "cwd").symlink_to(self.base)
        disk = record["entries"]["disk.qcow2"]
        device = f'{os.major(disk["dev"]):02x}:{os.minor(disk["dev"]):02x}'
        (own / "maps").write_text("1000-2000 rw-s 00000000 " + device + " " + str(disk["ino"]) + " /unrelated-renamed-path\n")
        with self.assertRaisesRegex(RuntimeError, "retirement_mapping_in_use"):
            vm.retirement_quiet(record, proc_root=proc)
        self.assert_held()

    def test_missing_anchor_during_wal_recovery_keeps_charge(self):
        root, value, sha = self.fixture()
        atomic = capacity.atomic_json
        def fail(path, data):
            if path.name == "identity.json" and data.get("revision") == 3:
                raise RuntimeError("injected_crash")
            return atomic(path, data)
        with mock.patch.object(capacity, "atomic_json", side_effect=fail), \
                self.assertRaisesRegex(RuntimeError, "injected_crash"):
            self.invoke(root, value, sha)
        (self.pool_root / "identity.json").unlink()
        with self.assertRaisesRegex(RuntimeError, "capacity_retirement_anchor_changed"):
            self.invoke(root, value, sha, 3)
        self.assertTrue(root.exists())
        self.assertEqual(len(self.ledger()["allocations"]), 1)

    def test_wrong_vm_or_hash_cannot_recover_another_retirement_wal(self):
        root, value, sha = self.fixture()
        atomic = capacity.atomic_json
        def fail(path, data):
            if path.name == "identity.json" and data.get("revision") == 3:
                raise RuntimeError("injected_crash")
            return atomic(path, data)
        with mock.patch.object(capacity, "atomic_json", side_effect=fail), \
                self.assertRaisesRegex(RuntimeError, "injected_crash"):
            self.invoke(root, value, sha)
        before = {name: (self.pool_root / name).read_bytes()
                  for name in ["pool.json", "identity.json", "retirement-pending.json"]}
        other_id = str(uuid.uuid4())
        other_root = self.first / (vm.VM_PREFIX + uuid.UUID(other_id).hex)
        for selected_root, ident, digest in [(other_root, other_id, sha), (root, value["id"], "f" * 64)]:
            with self.subTest(ident=ident), self.assertRaisesRegex(RuntimeError, "capacity_retirement_recovery_binding_changed"):
                vm.retire(selected_root, ident, digest, 3)
            self.assertEqual({name: (self.pool_root / name).read_bytes() for name in before}, before)
            self.assertTrue(root.exists())
        self.invoke(root, value, sha, 3)

    def test_renamed_quarantine_inside_or_outside_parent_is_never_a_delete(self):
        root, value, sha = self.fixture()
        quarantine = self.pause_before_delete(root, value, sha)
        for destination in (self.first / "held-original", self.second / "moved-outside-parent"):
            quarantine.rename(destination)
            with self.subTest(destination=destination.name), \
                    self.assertRaisesRegex(RuntimeError, "retirement_delete_proof_missing_unknown"):
                self.invoke(root, value, sha)
            self.assertTrue((destination / "disk.qcow2").exists())
            self.assert_held()
            destination.rename(quarantine)
        self.invoke(root, value, sha)

    def test_moved_file_is_not_accepted_as_partial_deletion(self):
        root, value, sha = self.fixture()
        quarantine = self.pause_before_delete(root, value, sha)
        destination = self.second / "moved-disk"
        (quarantine / "disk.qcow2").rename(destination)
        with self.assertRaisesRegex(RuntimeError, "retirement_delete_proof_missing_unknown"):
            self.invoke(root, value, sha)
        self.assertEqual(destination.read_bytes(), b"own-test-disk")
        self.assertTrue((quarantine / "vm.json").exists())
        self.assert_held()

    def test_crash_after_entry_witness_resumes_remaining_proven_inventory(self):
        root, value, sha = self.fixture()
        write = capacity.write_deletion_witness
        def fail(proof_root, record, relative, observed):
            result = write(proof_root, record, relative, observed)
            if relative == "shared/marker":
                raise RuntimeError("injected_crash_after_witness")
            return result
        with mock.patch.object(capacity, "write_deletion_witness", side_effect=fail), \
                self.assertRaisesRegex(RuntimeError, "injected_crash_after_witness"):
            self.invoke(root, value, sha)
        record = self.pending(value)
        self.assertIsNotNone(capacity.read_deletion_witness(self.pool_root, record, "shared/marker"))
        self.assert_held()
        self.assertEqual(self.invoke(root, value, sha)["held"]["cpu_millis"], 0)

    def test_crash_before_root_delete_witness_cannot_synthesize_one(self):
        root, value, sha = self.fixture()
        write = capacity.write_deletion_witness
        def fail(proof_root, record, relative, observed):
            if relative == ".":
                raise RuntimeError("injected_crash_before_root_witness")
            return write(proof_root, record, relative, observed)
        with mock.patch.object(capacity, "write_deletion_witness", side_effect=fail), \
                self.assertRaisesRegex(RuntimeError, "injected_crash_before_root_witness"):
            self.invoke(root, value, sha)
        record = self.pending(value)
        self.assertFalse(root.exists())
        self.assertFalse(Path(record["quarantine"]).exists())
        self.assertIsNone(capacity.read_deletion_witness(self.pool_root, record, "."))
        with self.assertRaisesRegex(RuntimeError, "retirement_delete_proof_missing_unknown"):
            self.invoke(root, value, sha)
        self.assert_held()

    def test_final_wal_recovery_cannot_release_without_positive_root_witness(self):
        root, value, sha = self.fixture()
        atomic = capacity.atomic_json
        def fail(path, data):
            if path.name == "pool.json" and data.get("revision") == 5:
                raise RuntimeError("injected_crash")
            return atomic(path, data)
        with mock.patch.object(capacity, "atomic_json", side_effect=fail), \
                self.assertRaisesRegex(RuntimeError, "injected_crash"):
            self.invoke(root, value, sha)
        record = self.pending(value)
        witness = capacity.deletion_witness_path(self.pool_root, record, ".")
        witness.unlink()
        before = {name: (self.pool_root / name).read_bytes()
                  for name in ["pool.json", "identity.json", "retirement-pending.json"]}
        with self.assertRaisesRegex(RuntimeError, "retirement_delete_proof_missing_unknown"):
            self.invoke(root, value, sha, 5)
        self.assertEqual({name: (self.pool_root / name).read_bytes() for name in before}, before)
        self.assertEqual(len(self.ledger()["allocations"]), 1)


if __name__ == "__main__":
    unittest.main()
