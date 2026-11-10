"""Temporary-inventory/real-loopback tests; no guest, Docker or download actions."""
from contextlib import redirect_stdout
import io
import json
import multiprocessing
import os
from pathlib import Path
import socket
import stat
import tempfile
import threading
import unittest
from unittest import mock
import uuid

import local_executor_vm as vm

capacity = vm.capacity_pool


def reserve_in_child(parent, pool_root, organisation, port, ready, start, result):
    ready.put(True)
    start.wait(5)
    try:
        with mock.patch.object(capacity, "canonical_pool_root", return_value=Path(pool_root)), \
                mock.patch.object(capacity, "observed_bounds", return_value={"cpu_millis":100_000,"memory_mib":100_000}):
            root, value = vm.reserve_vm_ports(Path(parent), organisation, port)
        result.put(("reserved", str(root), value["docker_port"], value["ssh_port"]))
    except Exception as error:
        result.put(("refused", str(error)))


@unittest.skipUnless(os.name == "posix" and os.getuid() != 0,
                     "requires an unprivileged POSIX user")
class PortInventoryTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="cowork-vm-port-tests-")
        self.addCleanup(self.temporary.cleanup)
        self.parent = Path(self.temporary.name) / "vms"
        self.parent.mkdir(mode=0o700)
        self.pool_root = Path(self.temporary.name) / "capacity"
        self.addCleanup(mock.patch.stopall)
        mock.patch.object(capacity, "canonical_pool_root", return_value=self.pool_root).start()
        mock.patch.object(capacity, "observed_bounds", return_value={"cpu_millis":100_000,"memory_mib":100_000}).start()
        with capacity.locked_pool(allow_unconfigured=True) as pool:
            pool.configure({"version":1,"pool_id":str(uuid.uuid4()),"cpu_millis":100_000,"memory_mib":100_000,
                            "reserved_cpu_millis":1000,"reserved_memory_mib":1024,
                            "qemu_overhead_cpu_millis":100,"qemu_overhead_memory_mib":256,
                            "inventory_complete":True,"existing_vm_roots":[]}, 0)
        self.organisation = str(uuid.uuid4())

    def reserve(self, port=0):
        return vm.reserve_vm_ports(self.parent, self.organisation, port)

    def ready(self, root, value):
        value = {**value, "bootstrap_state": "ready", "ubuntu_image_sha256": "a" * 64}
        vm.atomic_json(root / "vm.json", value)
        return value

    def free_port(self):
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
            sock.bind(("127.0.0.1", 0))
            return sock.getsockname()[1]

    def legacy(self):
        ident = str(uuid.uuid4())
        root = self.parent / (vm.VM_PREFIX + uuid.UUID(ident).hex)
        root.mkdir(mode=0o700)
        port = self.free_port()
        while port == 2376:
            port = self.free_port()
        value = {"version": 1, "id": ident, "organisation_id": self.organisation,
                 "memory_mib": 4096, "cpus": 4, "ssh_port": port,
                 "docker_endpoint": "tcp://localhost:2376", "ubuntu_image_sha256": "b" * 64}
        vm.write(root / "vm.json", json.dumps(value))
        return root, value

    def test_auto_ports_are_distinct_private_and_reserved_before_bootstrap(self):
        root, value = self.reserve()
        self.assertEqual(value["bootstrap_state"], "preparing")
        self.assertEqual(vm.load(root, allow_preparing=True), value)
        self.assertEqual(stat.S_IMODE(root.stat().st_mode), 0o700)
        self.assertEqual(stat.S_IMODE((root / "vm.json").stat().st_mode), 0o600)
        self.assertEqual(stat.S_IMODE((self.parent / vm.LOCK_NAME).stat().st_mode), 0o600)
        self.assertEqual(str(uuid.UUID(value["operator_state_id"])), value["operator_state_id"])
        self.assertNotEqual(value["ssh_port"], value["docker_port"])
        with self.assertRaisesRegex(RuntimeError, "vm_bootstrap_incomplete"):
            vm.load(root)
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
            sock.bind(("127.0.0.1", value["docker_port"]))
        # No listener exists, but the unfinished VM still owns both ports.
        for port in (value["docker_port"], value["ssh_port"]):
            with self.assertRaisesRegex(RuntimeError, "vm_port_already_reserved"):
                self.reserve(port)
        second_root, second = self.reserve()
        self.assertNotEqual(root, second_root)
        self.assertEqual(len({value["ssh_port"], value["docker_port"],
                              second["ssh_port"], second["docker_port"]}), 4)

    def test_stopped_ready_vm_keeps_ports_and_atomic_identity(self):
        root, value = self.reserve()
        original_state_id = value["operator_state_id"]
        value = self.ready(root, value)
        self.assertFalse((root / "qemu.pid").exists())
        self.assertEqual(vm.load(root)["operator_state_id"], original_state_id)
        self.assertEqual(stat.S_IMODE((root / "vm.json").stat().st_mode), 0o600)
        self.assertFalse(list(root.glob(".vm.json.*.tmp")))
        for port in (value["ssh_port"], value["docker_port"]):
            with self.assertRaisesRegex(RuntimeError, "vm_port_already_reserved"):
                self.reserve(port)

    def test_bootstrap_failure_keeps_early_reservation_and_releases_inventory_lock(self):
        with mock.patch.object(vm.os, "access", return_value=True), \
                mock.patch.object(vm.shutil, "which", return_value="/unexecuted-tool"), \
                mock.patch.object(vm, "urlopen", side_effect=RuntimeError("synthetic_download_failure")), \
                redirect_stdout(io.StringIO()), self.assertRaisesRegex(RuntimeError, "synthetic_download_failure"):
            vm.create(self.parent, self.organisation)
        roots = list(self.parent.glob(vm.VM_PREFIX + "*"))
        self.assertEqual(len(roots), 1)
        root = roots[0]
        value = vm.load(root, allow_preparing=True)
        self.assertEqual(value["bootstrap_state"], "preparing")
        self.assertFalse((root / "base.qcow2").exists())
        for port in (value["docker_port"], value["ssh_port"]):
            with self.assertRaisesRegex(RuntimeError, "vm_port_already_reserved"):
                self.reserve(port)
        other, _ = self.reserve()
        self.assertNotEqual(root, other)

    def test_explicit_port_is_bound_and_foreign_listener_is_preserved(self):
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
            listener.bind(("127.0.0.1", 0))
            listener.listen()
            port = listener.getsockname()[1]
            with self.assertRaisesRegex(RuntimeError, "vm_loopback_port_unavailable"):
                self.reserve(port)
            self.assertEqual(listener.getsockname(), ("127.0.0.1", port))
            self.assertFalse(list(self.parent.glob(vm.VM_PREFIX + "*")))
        root, value = self.reserve(port)
        self.assertEqual(value["docker_port"], port)
        self.assertEqual(vm.load(root, allow_preparing=True)["docker_endpoint"], "tcp://localhost:" + str(port))

    def test_invalid_ports_do_not_create_inventory_entries(self):
        for port in (-1, 1, 1023, 65536, True, "2376"):
            with self.subTest(port=port), self.assertRaisesRegex(RuntimeError, "invalid_host_port"):
                self.reserve(port)
        self.assertFalse(list(self.parent.iterdir()))

    def test_two_processes_cannot_reserve_the_same_explicit_port(self):
        context = multiprocessing.get_context("spawn")
        ready = context.Queue()
        result = context.Queue()
        start = context.Event()
        port = self.free_port()
        children = [context.Process(target=reserve_in_child,
                    args=(str(self.parent), str(self.pool_root), self.organisation, port, ready, start, result))
                    for _ in range(2)]
        try:
            for child in children:
                child.start()
            for _ in children:
                self.assertTrue(ready.get(timeout=10))
            start.set()
            outcomes = [result.get(timeout=10) for _ in children]
            self.assertEqual(sorted(item[0] for item in outcomes), ["refused", "reserved"])
            refusal = next(item for item in outcomes if item[0] == "refused")
            self.assertEqual(refusal[1], "vm_port_already_reserved")
            for child in children:
                child.join(5)
                self.assertEqual(child.exitcode, 0)
            self.assertEqual(len(list(self.parent.glob(vm.VM_PREFIX + "*"))), 1)
        finally:
            start.set()
            for child in children:
                if child.is_alive():
                    child.terminate()
                child.join(5)
            ready.close()
            result.close()

    def test_legacy_2376_is_reserved_without_rewriting_metadata(self):
        root, original = self.legacy()
        before = (root / "vm.json").read_bytes()
        first = vm.load(root)
        second = vm.load(root)
        self.assertEqual(first["docker_port"], 2376)
        self.assertEqual(first["operator_state_id"], second["operator_state_id"])
        self.assertEqual(str(uuid.UUID(first["operator_state_id"])), first["operator_state_id"])
        self.assertEqual((root / "vm.json").read_bytes(), before)
        # Existing records need explicit, read-only capacity adoption first.
        with self.assertRaisesRegex(RuntimeError, "capacity_adoption_required"):
            self.reserve()
        with capacity.locked_pool() as pool:
            pool.reserve(root, first, adopted=True)
        self.assertEqual((root / "vm.json").read_bytes(), before)
        with self.assertRaisesRegex(RuntimeError, "vm_port_already_reserved"):
            self.reserve(2376)
        _, new = self.reserve()
        self.assertNotIn(2376, (new["ssh_port"], new["docker_port"]))
        self.assertNotIn(original["ssh_port"], (new["ssh_port"], new["docker_port"]))
        args = vm.qemu_arguments(root, first)
        self.assertIn("hostfwd=tcp:127.0.0.1:2376-:2376", args[args.index("-netdev") + 1])

    def test_unsafe_metadata_identity_permissions_and_owner_fail_closed(self):
        root, value = self.reserve()
        path = root / "vm.json"
        path.chmod(0o644)
        with self.assertRaisesRegex(RuntimeError, "unsafe_vm_metadata"):
            self.reserve()
        path.chmod(0o600)
        with mock.patch.object(vm.os, "getuid", return_value=os.getuid() + 1):
            with self.assertRaisesRegex(RuntimeError, "vm_root_requires_owner_and_0700"):
                vm.load(root, allow_preparing=True)
        vm.atomic_json(path, {**value, "id": str(uuid.uuid4())})
        with self.assertRaisesRegex(RuntimeError, "vm_root_identity_mismatch"):
            self.reserve()
        self.assertEqual(len(list(self.parent.glob(vm.VM_PREFIX + "*"))), 1)

    def test_symlink_metadata_and_lock_are_not_followed(self):
        root, _ = self.reserve()
        path = root / "vm.json"
        original = path.read_bytes()
        foreign = self.parent / "unrelated.json"
        vm.write(foreign, original)
        path.unlink()
        path.symlink_to(foreign)
        with self.assertRaisesRegex(RuntimeError, "unsafe_vm_metadata"):
            self.reserve()
        self.assertEqual(foreign.read_bytes(), original)
        path.unlink()
        vm.write(path, original)
        lock = self.parent / vm.LOCK_NAME
        lock.unlink()
        lock.symlink_to(foreign)
        with self.assertRaises(OSError):
            self.reserve()
        self.assertEqual(foreign.read_bytes(), original)

    def test_missing_metadata_or_cross_role_conflicts_block_new_allocation(self):
        root, value = self.reserve()
        (root / "vm.json").unlink()
        with self.assertRaises(FileNotFoundError):
            self.reserve()
        vm.write(root / "vm.json", json.dumps(value))
        other, other_value = self.reserve()
        vm.atomic_json(other / "vm.json", {**other_value, "ssh_port": value["docker_port"]})
        with self.assertRaisesRegex(RuntimeError, "vm_inventory_port_conflict"):
            self.reserve()

    def test_non_loopback_endpoint_or_matching_ssh_docker_port_is_refused(self):
        root, value = self.reserve()
        path = root / "vm.json"
        vm.atomic_json(path, {**value, "docker_endpoint": "tcp://0.0.0.0:" + str(value["docker_port"])})
        with self.assertRaisesRegex(RuntimeError, "unsafe_vm_docker_endpoint"):
            self.reserve()
        vm.atomic_json(path, {**value, "ssh_port": value["docker_port"]})
        with self.assertRaisesRegex(RuntimeError, "vm_ports_must_differ"):
            self.reserve()

    def test_start_and_tls_helpers_use_reserved_dynamic_host_ports(self):
        root, value = self.reserve()
        value = self.ready(root, value)
        with mock.patch.object(vm, "call") as command, mock.patch.object(vm, "qmp", return_value={"enabled": True}), \
                mock.patch.object(vm, "process", return_value=None), redirect_stdout(io.StringIO()):
            vm.start(root)
        args = command.call_args.args[0]
        forwarding = args[args.index("-netdev") + 1]
        self.assertIn("hostfwd=tcp:127.0.0.1:" + str(value["ssh_port"]) + "-:22", forwarding)
        self.assertIn("hostfwd=tcp:127.0.0.1:" + str(value["docker_port"]) + "-:2376", forwarding)
        self.assertEqual(vm.daemon_url(value, "/info"), "https://localhost:" + str(value["docker_port"]) + "/info")
        self.assertEqual(vm.daemon_url(value, "/_ping"), "https://localhost:" + str(value["docker_port"]) + "/_ping")
        tls = vm.docker_tls_arguments(root, value)
        self.assertEqual(tls[tls.index("--host") + 1], value["docker_endpoint"])
        self.assertEqual(args[args.index("-smp") + 1], "4")
        self.assertEqual(args[args.index("-m") + 1], "4096")
        self.assertEqual(args[args.index("-qmp") + 1], "unix:qmp.sock,server=on,wait=off")
        self.assertEqual(command.call_args.kwargs["cwd"], root)

    def test_qmp_connects_in_long_owned_root_without_changing_process_cwd(self):
        root = self.parent / ("long-private-qualification-root-" * 5)
        root.mkdir(mode=0o700)
        self.assertGreater(len(str(root / "qmp.sock").encode()), 108)
        directory_fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        self.addCleanup(os.close, directory_fd)
        listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.addCleanup(listener.close)
        listener.bind("/proc/self/fd/" + str(directory_fd) + "/qmp.sock")
        listener.listen(1)
        listener.settimeout(5)
        errors = []
        def serve():
            try:
                with listener.accept()[0] as connection:
                    stream = connection.makefile("rwb")
                    stream.write(b'{"QMP":{}}\n'); stream.flush()
                    for expected in ("qmp_capabilities", "query-kvm"):
                        self.assertEqual(json.loads(stream.readline())["execute"], expected)
                        stream.write(b'{"return":{"enabled":true}}\n'); stream.flush()
            except BaseException as error:
                errors.append(error)
        server = threading.Thread(target=serve)
        server.start()
        try:
            before = Path.cwd()
            with mock.patch.object(vm, "process", return_value=123):
                self.assertEqual(vm.qmp(root, "query-kvm"), {"enabled": True})
            self.assertEqual(Path.cwd(), before)
        finally:
            server.join(6)
        self.assertFalse(server.is_alive())
        self.assertEqual(errors, [])

    def test_start_refuses_occupied_reserved_ssh_port_before_qemu(self):
        root, value = self.reserve()
        self.ready(root, value)
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
            listener.bind(("127.0.0.1", value["ssh_port"]))
            listener.listen()
            with mock.patch.object(vm, "call") as command, self.assertRaisesRegex(RuntimeError, "vm_loopback_port_unavailable"):
                vm.start(root)
            command.assert_not_called()


if __name__ == "__main__":
    unittest.main()
