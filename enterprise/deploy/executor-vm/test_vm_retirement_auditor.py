"""Auditor protocol/metadata/kernel fixtures only; never invoke privileged WSL."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock
import uuid

import local_executor_vm as vm
import vm_retirement_auditor as reader

capacity = vm.capacity_pool
WSL = Path("/mnt/c/Windows/System32/wsl.exe")


class FakeKernel:
    """No real /proc reads: independent thread FD tables and mutable metadata."""
    def __init__(self):
        self.groups = {10: [10, 11], 20: [20, 21]}
        self.identities = {(10, 10): ((0, 0, 0, 0), 100, False, False),
            (10, 11): ((1000, 1000, 1000, 1000), 101, False, False),
            (20, 20): ((54321, 54321, 54321, 54321), 200, False, False),
            (20, 21): ((43210, 43210, 43210, 43210), 201, False, False)}
        self.descriptors = {task: {3: ("/unrelated/file", (999, 888))} for task in self.identities}
        self.mappings = {task: b"1000-2000 r--p 00000000 00:00 0 [anon]\n" for task in self.identities}
        self.references = {task: {"cwd": ("/unrelated", (999, 777)),
            "exe": ("/unrelated/python", (999, 666))} for task in self.identities}
        self.calls, self.hooks = {}, {}
        self.mountinfo = b"1 0 0:1 / / rw - tmpfs none rw\n"

    def value(self, method, key, value):
        selected = method, key
        self.calls[selected] = self.calls.get(selected, 0) + 1
        hook = self.hooks.get(selected)
        return hook(self.calls[selected], value) if hook else value

    def pids(self):
        return self.value("pids", None, list(self.groups))

    def tasks(self, pid):
        return self.value("tasks", pid, list(self.groups.get(pid, [pid])))

    def identity(self, task):
        return self.value("identity", task, self.identities[task])

    def links(self, task):
        return self.value("links", task, dict(self.references[task]))

    def fds(self, task):
        return self.value("fds", task, dict(self.descriptors[task]))

    def maps(self, task):
        return self.value("maps", task, self.mappings[task])

    def mounts(self):
        return self.value("mounts", None, self.mountinfo)


class CompleteThreadInventoryTests(unittest.TestCase):
    def setUp(self):
        self.kernel = FakeKernel()
        self.root, self.quarantine = "/owned/VM", "/owned/.retire-UUID"
        self.record = {"allocation": {"identity": {"root": self.root}}, "quarantine": self.quarantine,
            "root_entry": {"dev": 256, "ino": 100}, "entries": {"disk.qcow2": {"dev": 256, "ino": 101}}}

    def scan(self):
        # Path absence here is mocked too: no filesystem or real kernel scan.
        with mock.patch.object(reader.os.path, "lexists", return_value=False):
            return reader.process_inventory(self.record, reader=self.kernel)

    def test_all_uids_and_every_thread_have_two_fd_maps_and_stable_identity_checks(self):
        self.assertIsNone(self.scan())
        for task in self.kernel.identities:
            for method, count in [("fds", 2), ("maps", 2), ("links", 2), ("identity", 3)]:
                self.assertEqual(self.kernel.calls[method, task], count)
        self.assertEqual(self.kernel.calls["pids", None], 2)
        self.assertEqual(self.kernel.calls["mounts", None], 2)

    def test_unshared_nonleader_foreign_uid_fd_blocks_retirement(self):
        self.kernel.descriptors[20, 21][4] = (self.root + "/disk.qcow2", (256, 101))
        with self.assertRaisesRegex(RuntimeError, "audit_entry_active"):
            self.scan()
        self.assertEqual(self.kernel.calls["fds", (20, 20)], 2)
        self.assertEqual(self.kernel.calls["fds", (20, 21)], 1)

    def test_thread_fd_inode_alias_deleted_and_quarantine_paths_are_rejected(self):
        for target in [("/renamed/elsewhere", (256, 101)),
                       (self.root + "/disk.qcow2 (deleted)", (999, 888)),
                       (self.quarantine + "/nested/../disk.qcow2", (999, 888))]:
            with self.subTest(target=target):
                self.kernel.descriptors[20, 21] = {9: target}
                with self.assertRaisesRegex(RuntimeError, "audit_entry_active"):
                    self.scan()

    def test_no_exemption_for_auditor_or_controller_task(self):
        self.kernel.descriptors[10, 10][7] = ("/unrelated-alias", (256, 100))
        with self.assertRaisesRegex(RuntimeError, "audit_entry_active"):
            self.scan()

    def test_second_fd_pass_catches_new_thread_reference(self):
        self.kernel.hooks["fds", (10, 11)] = lambda count, value: value if count == 1 else {
            **value, 9: (self.root + "/disk.qcow2", (256, 101))}
        with self.assertRaisesRegex(RuntimeError, "audit_entry_active"):
            self.scan()

    def test_unrelated_fd_changes_do_not_fabricate_a_vm_reference(self):
        self.kernel.hooks["fds", (10, 11)] = lambda count, value: value if count == 1 else {
            9: ("/unrelated/new-file", (999, 111))}
        self.assertIsNone(self.scan())

    def test_all_four_uid_fields_and_start_time_are_stable(self):
        original = self.kernel.identities[20, 21]
        for changed in [((43210, 0, 43210, 43210), *original[1:]),
                        (original[0], original[1] + 1, *original[2:])]:
            with self.subTest(changed=changed):
                self.kernel.calls.clear()
                self.kernel.hooks["identity", (20, 21)] = lambda count, value: value if count == 1 else changed
                with self.assertRaisesRegex(RuntimeError, "audit_inventory_changed"):
                    self.scan()

    def test_final_recheck_catches_pid_reuse_after_first_task_pass(self):
        original = self.kernel.identities[10, 10]
        self.kernel.hooks["identity", (10, 10)] = lambda count, value: value if count < 3 else (
            original[0], original[1] + 1, *original[2:])
        with self.assertRaisesRegex(RuntimeError, "audit_inventory_changed"):
            self.scan()

    def test_full_process_and_thread_inventory_changes_fail_closed(self):
        for method, key, changed in [("pids", None, [10, 20, 30]),
                                     ("tasks", 20, [20]), ("tasks", 20, [20, 21, 22])]:
            with self.subTest(method=method, changed=changed):
                self.kernel.calls.clear()
                self.kernel.hooks.clear()
                self.kernel.hooks[method, key] = lambda count, value: value if count == 1 else changed
                with self.assertRaisesRegex(RuntimeError, "audit_inventory_changed"):
                    self.scan()

    def test_missing_leader_duplicate_or_empty_thread_inventory_refuses(self):
        for values in [[], [21], [20, 21, 21], [20, 0]]:
            with self.subTest(values=values):
                self.kernel.groups[20] = values
                with self.assertRaisesRegex(RuntimeError, "audit_process_evidence_unreadable"):
                    self.scan()

    def test_permission_or_disappearing_thread_metadata_has_no_quiet_fallback(self):
        for method in ["identity", "fds", "links", "maps", "tasks"]:
            for error in [PermissionError("secret-process-path"), FileNotFoundError("secret-process-path")]:
                with self.subTest(method=method, error=type(error)):
                    self.kernel.hooks.clear()
                    key = 20 if method == "tasks" else (20, 21)
                    def refuse(count, value):
                        raise error
                    self.kernel.hooks[method, key] = refuse
                    with self.assertRaisesRegex(RuntimeError, "^audit_process_evidence_unreadable$"):
                        self.scan()

    def test_thread_cwd_exe_paths_and_inode_aliases_are_checked(self):
        for name in ["cwd", "exe"]:
            for target in [(self.root + "/nested", (999, 888)), ("/elsewhere", (256, 101))]:
                with self.subTest(name=name, target=target):
                    self.kernel.references[20, 21][name] = target
                    with self.assertRaisesRegex(RuntimeError, "audit_entry_active"):
                        self.scan()
                    self.kernel.references[20, 21][name] = ("/unrelated", (999, 888))

    def test_missing_live_thread_exe_or_changed_link_refuses(self):
        self.kernel.references[20, 21]["exe"] = None
        with self.assertRaisesRegex(RuntimeError, "audit_process_evidence_unreadable"):
            self.scan()
        self.kernel.references[20, 21]["exe"] = ("/unrelated", (999, 888))
        self.kernel.calls.clear()
        self.kernel.hooks["links", (20, 21)] = lambda count, value: value if count == 1 else {
            **value, "cwd": ("/changed", (999, 555))}
        with self.assertRaisesRegex(RuntimeError, "audit_inventory_changed"):
            self.scan()

    def test_zombie_or_kernel_task_requires_positive_empty_fd_maps_evidence(self):
        task = 20, 21
        for zombie, kernel in [(True, False), (False, True)]:
            with self.subTest(zombie=zombie, kernel=kernel):
                self.kernel.identities[task] = ((43210,) * 4, 201, zombie, kernel)
                self.kernel.references[task] = {"cwd": None, "exe": None}
                self.kernel.descriptors[task], self.kernel.mappings[task] = {}, b""
                self.assertIsNone(self.scan())
                self.kernel.descriptors[task] = {9: ("/unrelated", (999, 888))}
                with self.assertRaisesRegex(RuntimeError, "audit_process_evidence_unreadable"):
                    self.scan()

    def test_thread_maps_path_inode_alias_and_escaped_deleted_names_refuse(self):
        for value in [b"1000-2000 r--p 00000000 00:00 0 /owned/VM/disk.qcow2\n",
                      b"1000-2000 r--p 00000000 01:00 101 /outside/alias\n",
                      b"1000-2000 r--p 00000000 00:00 0 /owned/VM/disk\\056qcow2 (deleted)\n"]:
            with self.subTest(value=value):
                self.kernel.mappings[20, 21] = value
                with self.assertRaisesRegex(RuntimeError, "audit_entry_active"):
                    self.scan()

    def test_second_mapping_pass_catches_late_reference(self):
        self.kernel.hooks["maps", (20, 21)] = lambda count, value: value if count == 1 else (
            b"1000-2000 r--p 00000000 01:00 101 /outside/alias\n")
        with self.assertRaisesRegex(RuntimeError, "audit_entry_active"):
            self.scan()

    def test_malformed_maps_refuse_instead_of_skipping_evidence(self):
        for value in [b"not maps\n", b"2000-1000 r--p 00000000 00:00 0\n",
                      b"1000-2000 r--p 00000000 00:00 0 relative/path\n",
                      b"1000-2000 r--p 00000000 00:00 18446744073709551616\n"]:
            with self.subTest(value=value):
                self.kernel.mappings[20, 21] = value
                with self.assertRaisesRegex(RuntimeError, "audit_process_evidence_unreadable"):
                    self.scan()

    def test_mount_appearing_after_thread_scan_refuses(self):
        self.kernel.hooks["mounts", None] = lambda count, value: value if count == 1 else (
            b"1 0 0:1 / /owned/VM rw - tmpfs none rw\n")
        with self.assertRaisesRegex(RuntimeError, "audit_mount_active"):
            self.scan()

    def test_bounded_deadline_and_inventory_are_enforced(self):
        with mock.patch.object(reader.time, "monotonic", side_effect=[0, 9]), \
                self.assertRaisesRegex(RuntimeError, "audit_deadline"):
            self.scan()
        self.kernel.hooks["pids", None] = lambda count, value: list(range(1, 32770))
        with self.assertRaisesRegex(RuntimeError, "audit_inventory_too_large"):
            self.scan()

    def test_kernel_status_pid_tgid_uid_and_start_parser_has_no_uid_filter(self):
        actual = reader._ProcInventory(Path("/fake-proc"), float("inf"))
        task = 456, 457
        status = b"State:\tS (sleeping)\nTgid:\t456\nPid:\t457\nUid:\t54321\t0\t54321\t54321\nKthread:\t0\n"
        fields = [b"S", *([b"0"] * 18), b"987"]
        raw_stat = b"457 (unshared thread) " + b" ".join(fields)
        with mock.patch.object(actual, "raw", side_effect=lambda unused, name, limit: status if name == "status" else raw_stat):
            self.assertEqual(actual.identity(task), ((54321, 0, 54321, 54321), 987, False, False))
        for wrong in [status.replace(b"Tgid:\t456", b"Tgid:\t123"),
                      status.replace(b"Pid:\t457", b"Pid:\t123"),
                      status.replace(b"54321\t0\t54321\t54321", b"54321\t54321"),
                      status + b"Uid:\t1\t1\t1\t1\n"]:
            with self.subTest(wrong=wrong), mock.patch.object(actual, "raw", side_effect=lambda unused, name, limit: wrong if name == "status" else raw_stat), \
                    self.assertRaisesRegex(RuntimeError, "audit_process_evidence_unreadable"):
                actual.identity(task)

    def test_real_reader_selects_each_thread_directory_not_leader_fd_alias(self):
        actual = reader._ProcInventory(Path("/fake-proc"), float("inf"))
        self.assertEqual(actual.directory((20, 21)), Path("/fake-proc/20/task/21"))
        self.assertNotEqual(actual.directory((20, 21)), Path("/fake-proc/20"))
        with mock.patch.object(reader, "read_bytes", return_value=b"") as reading:
            actual.maps((20, 21))
        reading.assert_called_once_with(Path("/fake-proc/20/task/21/maps"), 8 * 1024 * 1024)

    def test_complete_immutable_auditor_still_fits_direct_windows_child_argv(self):
        source = Path(reader.__file__).read_bytes().decode()
        argv = [str(WSL), "--distribution", "Ubuntu-24.04", "--user", "root", "--exec",
                "/usr/bin/python3", "-I", "-S", "-B", "-c", source]
        self.assertLess(len(subprocess.list2cmdline(argv).encode("utf-16-le")) // 2, 32767)


@unittest.skipUnless(os.name == "posix" and os.getuid() != 0,
                     "requires an unprivileged POSIX user")
class ReadOnlyAuditorTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="cowork-readonly-audit-")
        self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name)
        self.pool_root = self.home / ".local/share/cowork/executor-vm-capacity"
        self.parent = self.home / "vms"
        self.parent.mkdir(mode=0o700)
        self.uid = os.getuid()
        self.boot = Path("/proc/sys/kernel/random/boot_id").read_text().strip()
        for patch in [mock.patch.object(capacity, "canonical_pool_root", return_value=self.pool_root),
                      mock.patch.object(capacity, "account_home", return_value=self.home),
                      mock.patch.object(capacity, "observed_bounds", return_value={"cpu_millis": 100000, "memory_mib": 100000})]:
            patch.start()
            self.addCleanup(patch.stop)
        self.plan = {"version": 1, "pool_id": str(uuid.uuid4()), "cpu_millis": 5100, "memory_mib": 5376,
            "reserved_cpu_millis": 1000, "reserved_memory_mib": 1024,
            "qemu_overhead_cpu_millis": 100, "qemu_overhead_memory_mib": 256,
            "inventory_complete": True, "existing_vm_roots": []}
        with capacity.locked_pool(allow_unconfigured=True) as pool:
            pool.configure(self.plan, 0)
        self.root, self.value = vm.reserve_vm_ports(self.parent, str(uuid.uuid4()))
        vm.write(self.root / "disk.qcow2", b"own-test-file-not-read-by-auditor")
        self.digest = vm.retirement_metadata_sha(self.root)
        self.profile = {"version": 1, "mode": "wsl_root_read_only_v1",
            "auditor_sha256": hashlib.sha256(Path(reader.__file__).read_bytes()).hexdigest(),
            "wsl_executable": str(WSL), "wsl_executable_sha256": hashlib.sha256(b"test-only-executable").hexdigest(),
            "distribution": "Ubuntu-24.04", "allowed_binding": {"vm_id": self.value["id"], "root": str(self.root),
                "metadata_sha256": self.digest, "operator_uid": self.uid, "pool_id": self.plan["pool_id"],
                "initial_revision": 2, "boot_id": self.boot}}
        real_canonical, real_read = reader.canonical, reader.read_bytes
        for patch in [mock.patch.object(reader, "canonical", side_effect=lambda p: WSL if str(p) == str(WSL) else real_canonical(p)),
                      mock.patch.object(reader, "read_bytes", side_effect=lambda p, n: b"test-only-executable" if p == WSL else real_read(p, n))]:
            patch.start()
            self.addCleanup(patch.stop)

    def data(self):
        return capacity.read_private_json(self.pool_root / "pool.json")

    def record(self):
        parent, root_entry, entries = vm.retirement_tree(self.root)
        ident = str(uuid.uuid4())
        return {"version": 1, "retirement_id": ident, "state": "prepared",
                "allocation": self.data()["allocations"][self.value["id"]], "metadata_sha256": self.digest,
                "parent": parent, "root_entry": root_entry, "entries": entries,
                "inventory_sha256": capacity.ledger_digest(entries),
                "quarantine": str(self.parent / (".retire-" + uuid.UUID(ident).hex)), "deletion_witness_sha256": None}

    def adapter(self, profile=None):
        return reader.Adapter(profile or self.profile, vm_id=self.value["id"], root=self.root,
                              metadata_sha256=self.digest, revision=2)

    def request(self, record=None):
        import time
        data = self.data()
        return {"version": 1, "binding": self.profile["allowed_binding"], "phase": "prepare", "record": record or self.record(),
                "revision": data["revision"], "ledger_sha256": reader.sha(data), "nonce": str(uuid.uuid4()),
                "boot_id": self.boot, "requested_monotonic_ns": time.monotonic_ns()}

    def root_audit(self, request):
        import pwd
        with mock.patch.object(reader.os, "getuid", return_value=0), \
                mock.patch.object(pwd, "getpwuid", return_value=SimpleNamespace(pw_dir=str(self.home))), \
                mock.patch.object(reader, "process_inventory") as observed:
            result = reader.audit(request)
            observed.assert_called_once_with(request["record"])
            return result

    def pipeline(self, argv, *, input, **kwargs):
        self.assertEqual(argv[:10], [str(WSL), "--distribution", "Ubuntu-24.04", "--user", "root", "--exec",
                                    "/usr/bin/python3", "-I", "-S", "-B"])
        self.assertEqual(argv[10], "-c")
        self.assertEqual(hashlib.sha256(argv[11].encode()).hexdigest(), self.profile["auditor_sha256"])
        self.assertEqual(kwargs["timeout"], 10)
        self.assertNotIn("shell", kwargs)
        self.assertNotIn("FAKE_OPERATOR_SECRET", kwargs["env"])
        return subprocess.CompletedProcess(argv, 0, json.dumps(self.root_audit(json.loads(input))).encode(), b"")

    def test_pinned_pipe_adapter_with_independent_metadata_audit_covers_all_phases(self):
        phases = []
        def pipeline(argv, **kwargs):
            phases.append(json.loads(kwargs["input"])["phase"])
            return self.pipeline(argv, **kwargs)
        with mock.patch.object(reader.subprocess, "run", side_effect=pipeline), \
                mock.patch.dict(os.environ, {"FAKE_OPERATOR_SECRET": "do-not-forward"}):
            result = vm.retire(self.root, self.value["id"], self.digest, 2, auditor=self.adapter())
        self.assertEqual(phases, ["prepare", "before_delete", "before_delete", "final"])
        self.assertEqual(result["held"], {"cpu_millis": 0, "memory_mib": 0})

    def test_profile_pins_selection_source_executable_distribution_and_initial_cas(self):
        for key, value, reason in [("auditor_sha256", "f" * 64, "source_changed"),
                                  ("wsl_executable_sha256", "f" * 64, "executable_changed"),
                                  ("distribution", "Other", "untrusted_distribution"),
                                  ("wsl_executable", "/usr/bin/echo", "untrusted_executable")]:
            with self.subTest(key=key), self.assertRaisesRegex(RuntimeError, reason):
                self.adapter({**self.profile, key: value})
        for key, value in [("vm_id", str(uuid.uuid4())), ("metadata_sha256", "f" * 64),
                           ("initial_revision", 1), ("operator_uid", self.uid + 1)]:
            changed = {**self.profile, "allowed_binding": {**self.profile["allowed_binding"], key: value}}
            with self.subTest(key=key), self.assertRaisesRegex(RuntimeError, "scope_changed"):
                self.adapter(changed)

    def test_nonce_boot_hash_uid_and_timestamp_mismatch_refuse_no_mutation(self):
        adapter, record, data = self.adapter(), self.record(), self.data()
        def fake(modification):
            def run(argv, **kwargs):
                response = self.root_audit(json.loads(kwargs["input"]))
                response.update(modification)
                return subprocess.CompletedProcess(argv, 0, json.dumps(response).encode(), b"")
            return run
        for changed in [{"nonce": str(uuid.uuid4())}, {"boot_id": str(uuid.uuid4())},
                        {"request_sha256": "f" * 64}, {"auditor_uid": 1000}, {"observed_monotonic_ns": 0}]:
            with self.subTest(changed=changed), mock.patch.object(reader.subprocess, "run", side_effect=fake(changed)), \
                    self.assertRaisesRegex(RuntimeError, "response_binding_changed|response_stale"):
                adapter.verify(record, data, "prepare")
            self.assertEqual(self.data(), data)
            self.assertTrue(self.root.exists())

    def test_timeout_refusal_malformed_and_unreadable_evidence_have_no_fallback(self):
        adapter, record, data = self.adapter(), self.record(), self.data()
        for outcome, reason in [(subprocess.TimeoutExpired("pinned-child", 10), "transport_failed"),
                                (subprocess.CompletedProcess([], 1, b'{"verdict":"refused","reason":"audit_process_evidence_unreadable"}', b""), "refused"),
                                (subprocess.CompletedProcess([], 0, b"not-json", b""), "invalid_response")]:
            kwargs = {"side_effect": outcome} if isinstance(outcome, Exception) else {"return_value": outcome}
            with self.subTest(reason=reason), mock.patch.object(reader.subprocess, "run", **kwargs), \
                    self.assertRaisesRegex(RuntimeError, reason):
                adapter.verify(record, data, "prepare")
            self.assertEqual(self.data(), data)

    def test_root_reader_independently_rejects_changed_ledger_scope_inode_or_metadata(self):
        request = self.request()
        for change, reason in [({"ledger_sha256": "f" * 64}, "ledger_changed"),
                               ({"boot_id": str(uuid.uuid4())}, "boot_changed")]:
            with self.subTest(reason=reason), self.assertRaisesRegex(RuntimeError, reason):
                self.root_audit({**request, **change})
        changed = {**request["record"], "allocation": {**request["record"]["allocation"], "adopted": True}}
        with self.assertRaisesRegex(RuntimeError, "scope_changed"):
            self.root_audit({**request, "record": changed})
        changed = {**request["record"], "root_entry": {**request["record"]["root_entry"], "ino": 1}}
        with self.assertRaisesRegex(RuntimeError, "root_changed"):
            self.root_audit({**request, "record": changed})
        with (self.root / "vm.json").open("a") as stream:
            stream.write("\n")
        with self.assertRaisesRegex(RuntimeError, "inventory_changed|metadata_changed"):
            self.root_audit(request)

    def test_nonroot_entry_point_returns_fixed_refusal_without_any_file_change(self):
        import sys
        before = self.data()
        result = subprocess.run([sys.executable, "-I", "-S", "-B", "-c", Path(reader.__file__).read_text()],
            input=json.dumps(self.request()).encode(), capture_output=True, timeout=10)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(json.loads(result.stdout), {"version": 1, "verdict": "refused", "reason": "audit_root_required"})
        self.assertEqual(self.data(), before)

    def test_root_inventory_checks_foreign_uid_and_refuses_unreadable_processes(self):
        record = self.record()
        kernel = FakeKernel()
        kernel.descriptors[20, 21][3] = (str(self.root / "disk.qcow2"), (999, 888))
        with self.assertRaisesRegex(RuntimeError, "audit_entry_active"):
            reader.process_inventory(record, reader=kernel)
        kernel.descriptors[20, 21] = {}
        def refuse(count, value):
            raise PermissionError("not-exported")
        kernel.hooks["fds", (20, 21)] = refuse
        with self.assertRaisesRegex(RuntimeError, "audit_process_evidence_unreadable"):
            reader.process_inventory(record, reader=kernel)

    def test_recovery_uses_fresh_direct_pipe_before_any_anchor_publication(self):
        adapter = self.adapter()
        atomic = capacity.atomic_json
        def fail(path, data):
            if path.name == "identity.json" and data.get("revision") == 3:
                raise RuntimeError("injected_crash")
            return atomic(path, data)
        with mock.patch.object(reader.subprocess, "run", side_effect=self.pipeline), \
                mock.patch.object(capacity, "atomic_json", side_effect=fail), \
                self.assertRaisesRegex(RuntimeError, "injected_crash"):
            vm.retire(self.root, self.value["id"], self.digest, 2, auditor=adapter)
        profile = {**self.profile, "allowed_binding": {**self.profile["allowed_binding"], "initial_revision": 3}}
        adapter = reader.Adapter(profile, vm_id=self.value["id"], root=self.root, metadata_sha256=self.digest, revision=3)
        before = {name: (self.pool_root / name).read_bytes() for name in ["pool.json", "identity.json", "retirement-pending.json"]}
        with mock.patch.object(reader.subprocess, "run", side_effect=subprocess.TimeoutExpired("pinned-child", 10)), \
                self.assertRaisesRegex(RuntimeError, "transport_failed"):
            vm.retire(self.root, self.value["id"], self.digest, 3, auditor=adapter)
        self.assertEqual({name: (self.pool_root / name).read_bytes() for name in before}, before)
        phases = []
        def observed(argv, **kwargs):
            phases.append(json.loads(kwargs["input"])["phase"])
            return self.pipeline(argv, **kwargs)
        with mock.patch.object(reader.subprocess, "run", side_effect=observed):
            result = vm.retire(self.root, self.value["id"], self.digest, 3, auditor=adapter)
        self.assertEqual(phases[0], "recovery")
        self.assertEqual(result["held"]["cpu_millis"], 0)


if __name__ == "__main__":
    unittest.main()
