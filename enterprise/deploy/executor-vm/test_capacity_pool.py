"""Own temporary pool/inventory/kernel fixtures and actual two-process flock.

No guest, Docker operation, download or real user's pool is used.
"""
from contextlib import redirect_stdout
import io
import json
import multiprocessing
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock
import uuid

import local_executor_vm as vm

capacity = vm.capacity_pool

BOUNDS = {"cpu_millis":100_000,"memory_mib":100_000}


def contend(parent, pool_root, ready, go, results):
    with mock.patch.object(capacity, "canonical_pool_root", return_value=Path(pool_root)), \
            mock.patch.object(capacity, "observed_bounds", return_value=BOUNDS):
        ready.put(True)
        go.wait(10)
        try:
            root, value = vm.reserve_vm_ports(Path(parent), str(uuid.uuid4()))
            results.put(("reserved", str(root), value["id"]))
        except Exception as error:
            results.put(("refused", str(error)))


@unittest.skipUnless(os.name == "posix" and os.getuid() != 0,
                     "requires an unprivileged POSIX user")
class CapacityPoolTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="cowork-parent-budget-")
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)
        self.pool_root = self.base / "capacity"
        self.first = self.base / "first"
        self.second = self.base / "second"
        self.first.mkdir(mode=0o700)
        self.second.mkdir(mode=0o700)
        self.patches = [mock.patch.object(capacity, "canonical_pool_root", return_value=self.pool_root),
                        mock.patch.object(capacity, "observed_bounds", return_value=BOUNDS)]
        for patch in self.patches:
            patch.start()
            self.addCleanup(patch.stop)
        self.plan = {"version":1,"pool_id":str(uuid.uuid4()),"cpu_millis":5100,"memory_mib":5376,
                     "reserved_cpu_millis":1000,"reserved_memory_mib":1024,
                     "qemu_overhead_cpu_millis":100,"qemu_overhead_memory_mib":256,
                     "inventory_complete":True,"existing_vm_roots":[]}

    def configure(self, plan=None, revision=0):
        with capacity.locked_pool(allow_unconfigured=True) as pool:
            pool.configure(plan or self.plan, revision)
            return pool.data["revision"]

    def ledger(self):
        return capacity.read_private_json(self.pool_root / "pool.json")

    def reserve(self, parent=None):
        return vm.reserve_vm_ports(parent or self.first, str(uuid.uuid4()))

    def ready(self, root, value):
        vm.atomic_json(root / "vm.json", {**value,"bootstrap_state":"ready","ubuntu_image_sha256":"a" * 64})

    def test_missing_plan_blocks_all_new_parents_and_unadopted_start(self):
        with self.assertRaisesRegex(RuntimeError, "capacity_plan_required"):
            self.reserve()
        self.assertFalse(list(self.first.glob(vm.VM_PREFIX + "*")))
        self.configure()
        root, value = self.reserve()
        self.ready(root, value)
        # A fresh second parent still uses this same account-wide pool.
        with self.assertRaisesRegex(RuntimeError, "parent_capacity_exhausted"):
            self.reserve(self.second)
        self.assertFalse(list(self.second.glob(vm.VM_PREFIX + "*")))

    def test_retained_ports_cannot_be_reused_through_a_different_parent(self):
        self.configure({**self.plan,"cpu_millis":100_000,"memory_mib":100_000})
        _, first = self.reserve()
        for port in (first["ssh_port"],first["docker_port"]):
            with self.assertRaisesRegex(RuntimeError,"vm_port_already_reserved"):
                vm.reserve_vm_ports(self.second,str(uuid.uuid4()),port)
        _, second = self.reserve(self.second)
        self.assertEqual(len({first["ssh_port"],first["docker_port"],second["ssh_port"],second["docker_port"]}),4)

    def test_private_complete_plan_and_positive_reserves_are_required(self):
        for changed in ({"inventory_complete":False},{"reserved_memory_mib":0},
                        {"qemu_overhead_cpu_millis":0},{"memory_mib":True}):
            with self.subTest(changed=changed), self.assertRaises(RuntimeError):
                self.configure({**self.plan,**changed})
        path = self.base / "host-plan.json"
        vm.write(path,json.dumps(self.plan))
        path.chmod(0o644)
        with mock.patch.object(vm.sys,"argv",["vm","configure-capacity","--host-plan",str(path)]), \
                self.assertRaisesRegex(RuntimeError,"unsafe_capacity_file"):
            vm.main()
        self.assertFalse((self.pool_root / "pool.json").exists())

    def test_two_processes_different_parents_share_one_allocation_limit(self):
        self.configure()
        context = multiprocessing.get_context("spawn")
        ready, results, go = context.Queue(), context.Queue(), context.Event()
        children = [context.Process(target=contend,
                    args=(str(parent), str(self.pool_root), ready, go, results))
                    for parent in (self.first, self.second)]
        try:
            for child in children:
                child.start()
            for _ in children:
                self.assertTrue(ready.get(timeout=10))
            go.set()
            outcomes = [results.get(timeout=10) for _ in children]
            self.assertEqual(sorted(item[0] for item in outcomes), ["refused", "reserved"])
            self.assertEqual(next(item[1] for item in outcomes if item[0] == "refused"), "parent_capacity_exhausted")
            for child in children:
                child.join(5)
                self.assertEqual(child.exitcode, 0)
            ledger = self.ledger()
            self.assertEqual(ledger["revision"], 2)
            self.assertEqual(len(ledger["allocations"]), 1)
            self.assertEqual(sum(len(list(parent.glob(vm.VM_PREFIX + "*"))) for parent in (self.first,self.second)), 1)
            entry = next(iter(ledger["allocations"].values()))
            self.assertEqual((entry["cpu_millis"],entry["memory_mib"]), (4100,4352))
        finally:
            go.set()
            for child in children:
                if child.is_alive():
                    child.terminate()
                child.join(5)
            ready.close()
            results.close()

    def test_stopped_and_failed_bootstrap_keep_charge(self):
        self.configure()
        with mock.patch.object(vm.os, "access", return_value=True), \
                mock.patch.object(vm.shutil, "which", return_value="/unused"), \
                mock.patch.object(vm, "urlopen", side_effect=RuntimeError("synthetic_download_failure")), \
                redirect_stdout(io.StringIO()), self.assertRaisesRegex(RuntimeError,"synthetic_download_failure"):
            vm.create(self.first, str(uuid.uuid4()))
        root = next(self.first.glob(vm.VM_PREFIX + "*"))
        original = self.ledger()
        with self.assertRaisesRegex(RuntimeError, "parent_capacity_exhausted"):
            self.reserve(self.second)
        value = vm.load(root, allow_preparing=True)
        self.ready(root, value)
        with mock.patch.object(vm, "process", return_value=None):
            vm.stop(root)
        self.assertEqual(self.ledger(), original)
        with self.assertRaisesRegex(RuntimeError, "parent_capacity_exhausted"):
            self.reserve(self.second)

    def test_ledger_first_crash_retains_charge_and_unresolved_gate(self):
        self.configure()
        with mock.patch.object(vm, "atomic_json", side_effect=RuntimeError("synthetic_metadata_failure")), \
                self.assertRaisesRegex(RuntimeError,"synthetic_metadata_failure"):
            self.reserve()
        original = self.ledger()
        self.assertEqual(len(original["allocations"]), 1)
        with self.assertRaises(FileNotFoundError):
            self.reserve(self.second)
        self.assertEqual(self.ledger(), original)

    def test_missing_root_and_changed_binding_never_release_or_start(self):
        self.configure({**self.plan,"cpu_millis":100_000,"memory_mib":100_000})
        root, value = self.reserve()
        self.ready(root, value)
        original = self.ledger()
        with mock.patch.object(vm, "call") as command:
            vm.atomic_json(root / "vm.json", {**value,"bootstrap_state":"ready","operator_state_id":str(uuid.uuid4())})
            with self.assertRaisesRegex(RuntimeError,"capacity_vm_binding_changed"):
                vm.start(root)
            command.assert_not_called()
        self.assertEqual(self.ledger(), original)
        vm.atomic_json(root / "vm.json", value)
        (root / "vm.json").unlink()
        root.rmdir()
        with self.assertRaisesRegex(RuntimeError,"capacity_inventory_unresolved"):
            self.reserve(self.second)
        self.assertEqual(self.ledger(), original)

    def test_explicit_legacy_adoption_is_read_only_and_charged_before_adoption(self):
        ident = str(uuid.uuid4())
        root = self.first / (vm.VM_PREFIX + uuid.UUID(ident).hex)
        root.mkdir(mode=0o700)
        value = {"version":1,"id":ident,"organisation_id":str(uuid.uuid4()),"cpus":4,
                 "memory_mib":4096,"ssh_port":2244,"docker_endpoint":"tcp://localhost:2376"}
        vm.write(root / "vm.json", json.dumps(value))
        vm.write(root / "disk.qcow2", b"own-test-sentinel")
        before = {p.name:p.read_bytes() for p in root.iterdir()}
        self.configure({**self.plan,"existing_vm_roots":[str(root)]})
        original = self.ledger()
        self.assertEqual(original["allocations"], {})
        with capacity.locked_pool() as pool:
            self.assertEqual(pool.totals(), {"cpu_millis":4100,"memory_mib":4352})
        with mock.patch.object(vm, "call") as command, self.assertRaisesRegex(RuntimeError,"capacity_adoption_required"):
            vm.start(root)
        command.assert_not_called()
        with self.assertRaisesRegex(RuntimeError,"capacity_adoption_required"):
            self.reserve(self.second)
        with mock.patch.object(vm.sys,"argv",["vm","adopt-capacity","--root",str(root)]), redirect_stdout(io.StringIO()):
            vm.main()
        after = self.ledger()
        self.assertEqual(after["unadopted_roots"], [])
        self.assertTrue(after["allocations"][ident]["adopted"])
        self.assertEqual({p.name:p.read_bytes() for p in root.iterdir()}, before)
        with capacity.locked_pool() as pool:
            pool.check_inventory(vm.load)
            pool.require_registered(root, vm.load(root))
            self.assertEqual(pool.totals(), {"cpu_millis":4100,"memory_mib":4352})
        # Identical adoption is idempotent and does not mint another allocation.
        with mock.patch.object(vm.sys,"argv",["vm","adopt-capacity","--root",str(root)]), redirect_stdout(io.StringIO()):
            vm.main()
        self.assertEqual(self.ledger(), after)

    def test_plan_cas_ceiling_shrink_and_frozen_overhead(self):
        self.configure()
        root, _ = self.reserve()
        original = self.ledger()
        with self.assertRaisesRegex(RuntimeError,"capacity_revision_conflict"):
            self.configure(self.plan, revision=1)
        with self.assertRaisesRegex(RuntimeError,"parent_capacity_exhausted"):
            self.configure({**self.plan,"memory_mib":5375}, revision=2)
        with self.assertRaisesRegex(RuntimeError,"capacity_plan_identity_and_overhead_are_immutable"):
            self.configure({**self.plan,"qemu_overhead_memory_mib":128}, revision=2)
        with self.assertRaisesRegex(RuntimeError,"capacity_plan_exceeds_linux_ceiling"):
            self.configure({**self.plan,"memory_mib":100_001}, revision=2)
        self.assertEqual(self.ledger(), original)
        self.assertEqual(self.configure({**self.plan,"cpu_millis":10_000,"memory_mib":12_000}, revision=2), 3)
        self.assertEqual(self.ledger()["allocations"], original["allocations"])
        self.assertTrue(root.exists())

    def test_reduced_linux_bounds_block_start_without_releasing_held_capacity(self):
        self.configure()
        root, value = self.reserve()
        self.ready(root, value)
        original = self.ledger()
        with mock.patch.object(capacity,"observed_bounds",return_value={"cpu_millis":4000,"memory_mib":5000}), \
                mock.patch.object(vm,"call") as command, self.assertRaisesRegex(RuntimeError,"capacity_plan_exceeds_linux_ceiling"):
            vm.start(root)
        command.assert_not_called()
        self.assertEqual(self.ledger(), original)

    def test_pool_metadata_symlink_hardlink_and_permissions_are_rejected(self):
        self.configure()
        path = self.pool_root / "pool.json"
        before = path.read_bytes()
        path.chmod(0o644)
        with self.assertRaisesRegex(RuntimeError,"unsafe_capacity_file"):
            self.reserve()
        path.chmod(0o600)
        link = self.base / "linked.json"
        os.link(path, link)
        with self.assertRaisesRegex(RuntimeError,"unsafe_capacity_file"):
            self.reserve()
        link.unlink()
        foreign = self.base / "foreign.json"
        vm.write(foreign,before)
        path.unlink()
        path.symlink_to(foreign)
        with self.assertRaisesRegex(RuntimeError,"capacity_path_must_be_canonical|unsafe_capacity_path"):
            self.reserve()
        self.assertEqual(foreign.read_bytes(), before)

    def test_canonical_pool_ignores_home_xdg_and_vm_parent(self):
        # Call the original function captured before setUp's path patch.
        with mock.patch.dict(os.environ,{"HOME":str(self.second),"XDG_DATA_HOME":str(self.first)}):
            self.assertEqual(REAL_CANONICAL_POOL_ROOT(), capacity.account_home() / ".local/share/cowork/executor-vm-capacity")

    def test_exact_companion_import_works_from_an_unrelated_directory(self):
        path = Path(vm.__file__).resolve()
        code = ("import importlib.util; from pathlib import Path; "
                "s=importlib.util.spec_from_file_location('isolated_vm'," + repr(str(path)) + "); "
                "m=importlib.util.module_from_spec(s); s.loader.exec_module(m); "
                "assert Path(m.capacity_pool.__file__).resolve()==Path(" + repr(str(path.with_name("capacity_pool.py"))) + ")")
        result = subprocess.run([sys.executable,"-B","-c",code],cwd=self.base,
                                capture_output=True,text=True,timeout=10)
        self.assertEqual(result.returncode,0,result.stderr)

    def test_missing_ledger_cannot_reinitialize_and_forget_allocations(self):
        self.configure()
        self.reserve()
        (self.pool_root / "pool.json").unlink()
        with self.assertRaisesRegex(RuntimeError,"capacity_ledger_missing"):
            self.configure()
        self.assertFalse((self.pool_root / "pool.json").exists())

    def test_older_ledger_revision_cannot_forget_a_newer_allocation(self):
        self.configure()
        old = (self.pool_root / "pool.json").read_bytes()
        self.reserve()
        anchor = capacity.read_private_json(self.pool_root / "identity.json")
        self.assertEqual(anchor["revision"],2)
        (self.pool_root / "pool.json").write_bytes(old)
        with self.assertRaisesRegex(RuntimeError,"capacity_pool_identity_changed"):
            self.reserve(self.second)
        with self.assertRaisesRegex(RuntimeError,"capacity_pool_identity_changed"):
            self.configure()
        self.assertEqual(capacity.read_private_json(self.pool_root / "identity.json"),anchor)

    def test_linux_memtotal_affinity_and_nested_cgroup_limits_bound_plan(self):
        proc = self.base / "proc"
        fs = self.base / "fs"
        (proc / "self").mkdir(parents=True)
        group = fs / "sys/fs/cgroup/team/task"
        group.mkdir(parents=True)
        (proc / "meminfo").write_text("MemTotal:       16777216 kB\nMemAvailable:   100 kB\n")
        (proc / "self/cgroup").write_text("0::/team/task\n")
        (proc / "self/mountinfo").write_text("1 0 0:1 / /sys/fs/cgroup rw - cgroup2 cgroup rw\n")
        for directory in (group,group.parent,group.parent.parent):
            (directory / "memory.max").write_text("max")
            (directory / "cpu.max").write_text("max 100000")
        (group.parent / "memory.max").write_text(str(8 * 1024**3))
        (group.parent / "cpu.max").write_text("750000 100000")
        (group / "memory.max").write_text(str(6 * 1024**3))
        # Invoke the real function, including parent cgroup constraints.
        with mock.patch.object(capacity.os,"sched_getaffinity",return_value=set(range(12))):
            self.assertEqual(REAL_OBSERVED_BOUNDS(proc,fs), {"cpu_millis":7500,"memory_mib":6144})
        (group / "cpu.max").write_text("broken")
        with self.assertRaisesRegex(RuntimeError,"invalid_cgroup_cpu_ceiling"):
            REAL_OBSERVED_BOUNDS(proc,fs)


REAL_CANONICAL_POOL_ROOT = capacity.canonical_pool_root
REAL_OBSERVED_BOUNDS = capacity.observed_bounds

if __name__ == "__main__":
    unittest.main()
