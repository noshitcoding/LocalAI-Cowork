"""Optional immutable WSL-root reader, and its unprivileged pipe adapter.

The privileged entry point imports only the OS Python standard library. It never
writes a file, loads operator code, reads file contents other than bounded VM/
pool/deletion-proof metadata, or sends a signal/QMP command. No receipt file is
accepted by the adapter; its witness comes directly from the pinned WSL child.
"""
import hashlib
import json
import os
from pathlib import Path
import posixpath
import re
import socket
import stat
import subprocess
import time
import uuid

MAX_REQUEST = 1024 * 1024
FIELDS = {"version", "mode", "auditor_sha256", "wsl_executable", "wsl_executable_sha256",
          "distribution", "allowed_binding"}
BINDING_FIELDS = {"vm_id", "root", "metadata_sha256", "operator_uid", "pool_id", "initial_revision", "boot_id"}
REFUSAL_CODES = {"audit_invalid_identity", "audit_unsafe_path", "audit_inventory_too_large", "audit_unsafe_metadata",
    "audit_unsafe_entry", "audit_deletion_proof_changed", "audit_root_changed", "audit_inventory_changed",
    "audit_deleted_entry_reappeared", "audit_delete_proof_missing", "audit_metadata_changed",
    "audit_deleted_root_reappeared", "audit_process_evidence_unreadable", "audit_mount_active", "audit_deadline",
    "audit_entry_active", "audit_invalid_pid", "audit_process_active", "audit_invalid_socket", "audit_socket_unknown",
    "audit_socket_active", "audit_root_required", "audit_invalid_request", "audit_boot_changed", "audit_stale_request",
    "audit_ledger_changed", "audit_scope_changed", "audit_unreadable"}


def check(ok, code):
    if not ok:
        raise RuntimeError(code)


def sha(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def identifier(value):
    check(type(value) is str and str(uuid.UUID(value)) == value and uuid.UUID(value).int != 0,
          "audit_invalid_identity")


def canonical(path):
    path = Path(path)
    check(path.is_absolute() and path == path.resolve()
          and not path.is_symlink() and all(not p.is_symlink() for p in path.parents), "audit_unsafe_path")
    return path


def read_bytes(path, limit):
    with path.open("rb") as stream:
        value = stream.read(limit + 1)
    check(len(value) <= limit, "audit_inventory_too_large")
    return value


def metadata(path, uid, limit=2 * 1024 * 1024):
    canonical(path)
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    with os.fdopen(fd, "rb") as stream:
        info = os.fstat(stream.fileno())
        present = path.lstat()
        check(stat.S_ISREG(info.st_mode) and info.st_uid == uid and info.st_nlink == 1
              and stat.S_IMODE(info.st_mode) == 0o600 and info.st_size <= limit
              and (info.st_dev, info.st_ino) == (present.st_dev, present.st_ino), "audit_unsafe_metadata")
        value = stream.read(limit + 1)
        check(len(value) <= limit, "audit_inventory_too_large")
    return json.loads(value), hashlib.sha256(value).hexdigest()


def entry(path, uid):
    info = path.lstat()
    check(info.st_uid == uid and not info.st_mode & 0o022, "audit_unsafe_entry")
    if stat.S_ISDIR(info.st_mode):
        check(stat.S_IMODE(info.st_mode) == 0o700, "audit_unsafe_entry")
        kind = "directory"
    elif stat.S_ISREG(info.st_mode):
        check(info.st_nlink == 1, "audit_unsafe_entry")
        kind = "file"
    elif stat.S_ISSOCK(info.st_mode):
        kind = "socket"
    else:
        raise RuntimeError("audit_unsafe_entry")
    return {"dev": info.st_dev, "ino": info.st_ino, "mode": info.st_mode, "kind": kind,
            "size": info.st_size, "mtime_ns": info.st_mtime_ns}


def same(actual, expected):
    fields = ("dev", "ino", "mode", "kind") if expected["kind"] == "directory" else tuple(expected)
    return all(actual[field] == expected[field] for field in fields)


def scope(record):
    return sha({key: value for key, value in record.items() if key not in ("state", "deletion_witness_sha256")})


def proof(pool, record, relative, uid):
    selected = record["root_entry"] if relative == "." else record["entries"][relative]
    expected = {"version": 1, "retirement_id": record["retirement_id"], "scope_sha256": scope(record),
        "relative_path": relative, "entry": selected,
        "operation": "rmdir" if selected["kind"] == "directory" else "unlink", "parent_fsynced": True,
        "observed": {key: selected[key] for key in ("dev", "ino", "mode")} | {"nlink": 0}}
    path = pool / "retirement-proofs" / uuid.UUID(record["retirement_id"]).hex / (hashlib.sha256(relative.encode()).hexdigest() + ".json")
    if not os.path.lexists(path):
        return None
    check(entry(path.parent.parent, uid)["kind"] == entry(path.parent, uid)["kind"] == "directory", "audit_unsafe_metadata")
    value, _ = metadata(path, uid)
    check(value == expected, "audit_deletion_proof_changed")
    return value


def observe_tree(root, record, uid, pool):
    check(same(entry(root, uid), record["root_entry"]), "audit_root_changed")
    seen = set()
    def visit(path, prefix):
        for child in sorted(path.iterdir()):
            name = prefix + child.name
            check(len(seen) < 2048 and name in record["entries"], "audit_inventory_changed")
            actual = entry(child, uid)
            check(actual["dev"] == record["root_entry"]["dev"] and same(actual, record["entries"][name]),
                  "audit_inventory_changed")
            check(actual["kind"] != "socket" or name == "qmp.sock", "audit_inventory_changed")
            check(proof(pool, record, name, uid) is None, "audit_deleted_entry_reappeared")
            seen.add(name)
            if actual["kind"] == "directory":
                visit(child, name + "/")
    visit(root, "")
    for name in set(record["entries"]) - seen:
        check(proof(pool, record, name, uid) is not None, "audit_delete_proof_missing")
    if "vm.json" in seen:
        _, digest = metadata(root / "vm.json", uid)
        check(digest == record["metadata_sha256"], "audit_metadata_changed")
    check(proof(pool, record, ".", uid) is None, "audit_deleted_root_reappeared")


class _ProcInventory:
    """Kernel metadata only, including every thread's possibly unshared FDs."""
    def __init__(self, root, deadline):
        self.root, self.deadline = root, deadline

    def bounded(self):
        check(time.monotonic() < self.deadline, "audit_deadline")

    def directory(self, task):
        pid, tid = task
        return self.root / str(pid) / "task" / str(tid)

    def names(self, directory, limit):
        values = []
        with os.scandir(directory) as entries:
            for item in entries:
                self.bounded()
                if item.name.isdecimal():
                    check(len(values) < limit and int(item.name) > 0, "audit_inventory_too_large")
                    values.append(int(item.name))
                else:
                    check(directory == self.root, "audit_process_evidence_unreadable")
        check(len(values) == len(set(values)), "audit_process_evidence_unreadable")
        return sorted(values)

    def pids(self):
        return self.names(self.root, 32768)

    def tasks(self, pid):
        return self.names(self.root / str(pid) / "task", 4096)

    def raw(self, task, name, limit):
        self.bounded()
        return read_bytes(self.directory(task) / name, limit)

    def identity(self, task):
        status = self.raw(task, "status", 65536).decode()
        def field(name, pattern):
            values = re.findall(r"^" + name + r":\s+(" + pattern + r")$", status, re.M)
            check(len(values) == 1, "audit_process_evidence_unreadable")
            return values[0]
        uid = tuple(int(value) for value in field("Uid", r"[0-9]+\s+[0-9]+\s+[0-9]+\s+[0-9]+").split())
        check(int(field("Tgid", r"[0-9]+")) == task[0] and int(field("Pid", r"[0-9]+")) == task[1],
              "audit_process_evidence_unreadable")
        state = field("State", r"[A-Za-z] .*")[0]
        kernel = re.findall(r"^Kthread:\s+([01])$", status, re.M)
        check(len(kernel) <= 1, "audit_process_evidence_unreadable")
        raw = self.raw(task, "stat", 65536)
        end = raw.rfind(b")")
        fields = raw[end + 2:].split()
        check(end > 0 and raw.split(b" ", 1)[0] == str(task[1]).encode()
              and len(fields) >= 20 and fields[19].isdigit(), "audit_process_evidence_unreadable")
        return uid, int(fields[19]), state in "ZX", kernel == ["1"]

    def links(self, task):
        result = {}
        for name in ("cwd", "exe"):
            self.bounded()
            path = self.directory(task) / name
            try:
                target = os.readlink(path)
                check(len(os.fsencode(target)) <= 4096, "audit_inventory_too_large")
                info = path.stat()
                result[name] = target, (info.st_dev, info.st_ino)
            except FileNotFoundError:
                result[name] = None
        return result

    def fds(self, task):
        fd = os.open(self.directory(task) / "fd", os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
        result = {}
        try:
            # Both scanner directory FDs remain open while their links are
            # inspected; no blanket exemption for the auditor's own process.
            with os.scandir(fd) as entries:
                for item in entries:
                    self.bounded()
                    check(item.name.isdecimal() and len(result) < 4096, "audit_inventory_too_large")
                    target = os.readlink(item.name, dir_fd=fd)
                    check(len(os.fsencode(target)) <= 4096, "audit_inventory_too_large")
                    info = os.stat(item.name, dir_fd=fd, follow_symlinks=True)
                    result[int(item.name)] = target, (info.st_dev, info.st_ino)
            return result
        finally:
            os.close(fd)

    def maps(self, task):
        return self.raw(task, "maps", 8 * 1024 * 1024)

    def mounts(self):
        self.bounded()
        return read_bytes(self.root / "self/mountinfo", 4 * 1024 * 1024)


def process_inventory(record, *, proc=Path("/proc"), reader=None):
    # Every UID and every task is covered. Permission denial, a disappearing
    # task or incomplete evidence is uncertainty, never a quiet exemption.
    roots = (record["allocation"]["identity"]["root"], record["quarantine"])
    inodes = {(e["dev"], e["ino"]) for e in [record["root_entry"], *record["entries"].values()]}
    # Linux dev_t layout, without depending on a Windows host's os.makedev.
    map_inodes = {(((dev & 0xfff00) >> 8 | (dev & 0xfffff00000000000) >> 32,
                    dev & 0xff | (dev & 0xffffff00000) >> 12), ino) for dev, ino in inodes}
    def within(path):
        path = posixpath.normpath(path.removesuffix(" (deleted)"))
        return any(path == root or path.startswith(root + "/") for root in roots)
    deadline = time.monotonic() + 8
    reader = reader if reader is not None else _ProcInventory(proc, deadline)
    descriptors, mapping_bytes, mapping_rows = 0, 0, 0
    def bounded():
        check(time.monotonic() < deadline, "audit_deadline")
    def inventory():
        bounded()
        pids = reader.pids()
        check(len(pids) <= 32768 and len(pids) == len(set(pids))
              and all(type(pid) is int and pid > 0 for pid in pids), "audit_inventory_too_large")
        tasks, total = {}, 0
        for pid in pids:
            bounded()
            tids = reader.tasks(pid)
            check(0 < len(tids) <= 4096 and pid in tids and len(tids) == len(set(tids))
                  and all(type(tid) is int and tid > 0 for tid in tids), "audit_process_evidence_unreadable")
            total += len(tids)
            check(total <= 131072, "audit_inventory_too_large")
            tasks[pid] = tuple(sorted(tids))
        return tuple(sorted(pids)), tasks
    def reference(target):
        path, inode = target
        check(not within(path) and inode not in inodes, "audit_entry_active")
    def fds(task):
        nonlocal descriptors
        bounded()
        values = reader.fds(task)
        check(type(values) is dict and len(values) <= 4096
              and all(type(number) is int and number >= 0 for number in values), "audit_process_evidence_unreadable")
        descriptors += len(values)
        check(descriptors <= 262144, "audit_inventory_too_large")
        for target in values.values():
            bounded()
            reference(target)
        return values
    def maps(task):
        nonlocal mapping_bytes, mapping_rows
        bounded()
        raw = reader.maps(task)
        check(type(raw) is bytes and len(raw) <= 8 * 1024 * 1024, "audit_inventory_too_large")
        mapping_bytes += len(raw)
        check(mapping_bytes <= 256 * 1024 * 1024, "audit_inventory_too_large")
        previous_end = 0
        for line in raw.splitlines():
            bounded()
            mapping_rows += 1
            check(mapping_rows <= 1048576, "audit_inventory_too_large")
            match = re.fullmatch(rb"([0-9a-fA-F]{1,16})-([0-9a-fA-F]{1,16}) ([r-][w-][x-][ps]) "
                rb"([0-9a-fA-F]{1,16}) ([0-9a-fA-F]{1,8}):([0-9a-fA-F]{1,8}) ([0-9]{1,20})(?: +(.*))?", line)
            check(match is not None, "audit_process_evidence_unreadable")
            start, end = int(match[1], 16), int(match[2], 16)
            check(previous_end <= start < end < 2**64, "audit_process_evidence_unreadable")
            previous_end = end
            path = os.fsdecode(match[8] or b"")
            path = re.sub(r"\\([0-7]{3})", lambda item: chr(int(item[1], 8)), path)
            check(len(os.fsencode(path)) <= 4096 and "\0" not in path
                  and (not path or path.startswith("/") or path.startswith("[") and path.endswith("]")),
                  "audit_process_evidence_unreadable")
            check(not within(path), "audit_entry_active")
            device, inode = (int(match[5], 16), int(match[6], 16)), int(match[7])
            check(inode < 2**64, "audit_process_evidence_unreadable")
            check(inode == 0 or (device, inode) not in map_inodes, "audit_entry_active")
        return bool(raw)
    def mounts():
        bounded()
        mount_bytes = reader.mounts()
        check(type(mount_bytes) is bytes and len(mount_bytes) <= 4 * 1024 * 1024, "audit_inventory_too_large")
        for line in mount_bytes.decode().splitlines():
            bounded()
            fields = line.split()
            check(len(fields) >= 6, "audit_process_evidence_unreadable")
            path = re.sub(r"\\([0-7]{3})", lambda m: chr(int(m[1], 8)), fields[4])
            check(not within(path), "audit_mount_active")
    try:
        mounts()
        before = inventory()
        identities = {}
        for pid, tids in before[1].items():
            for tid in tids:
                task = pid, tid
                bounded()
                identity = reader.identity(task)
                links = reader.links(task)
                check(set(links) == {"cwd", "exe"}, "audit_process_evidence_unreadable")
                dormant = identity[2] or identity[3]
                check(dormant or all(target is not None for target in links.values()), "audit_process_evidence_unreadable")
                for target in links.values():
                    if target is not None:
                        reference(target)
                first_fds, first_maps = fds(task), maps(task)
                second_fds, second_maps = fds(task), maps(task)
                check(not dormant or not first_fds and not second_fds and not first_maps and not second_maps,
                      "audit_process_evidence_unreadable")
                check(reader.identity(task) == identity and reader.links(task) == links,
                      "audit_inventory_changed")
                identities[task] = identity
        check(inventory() == before, "audit_inventory_changed")
        for task, identity in identities.items():
            bounded()
            check(reader.identity(task) == identity, "audit_inventory_changed")
        mounts()
    except (OSError, ValueError, UnicodeError):
        raise RuntimeError("audit_process_evidence_unreadable") from None
    for name in roots:
        root = Path(name)
        if not os.path.lexists(root):
            continue
        pidfile = root / "qemu.pid"
        if os.path.lexists(pidfile):
            info = entry(pidfile, record["allocation"]["identity"].get("owner_uid", os.stat(root).st_uid))
            check(info["kind"] == "file" and info["size"] <= 32, "audit_invalid_pid")
            pid = read_bytes(pidfile, 32).decode().strip()
            check(pid.isdigit() and int(pid) > 0 and not (proc / pid).exists(), "audit_process_active")
        socket_path = root / "qmp.sock"
        if os.path.lexists(socket_path):
            check(stat.S_ISSOCK(socket_path.lstat().st_mode), "audit_invalid_socket")
            fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
            try:
                with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
                    sock.settimeout(.25)
                    try:
                        sock.connect("/proc/self/fd/" + str(fd) + "/qmp.sock")
                    except OSError as error:
                        check(error.errno == 111, "audit_socket_unknown")
                    else:
                        raise RuntimeError("audit_socket_active")
            finally:
                os.close(fd)


def audit(request):
    import pwd
    check(os.getuid() == 0, "audit_root_required")
    check(type(request) is dict and set(request) == {"version", "binding", "phase", "record", "revision",
        "ledger_sha256", "nonce", "boot_id", "requested_monotonic_ns"} and type(request["version"]) is int
          and request["version"] == 1, "audit_invalid_request")
    binding, record = request["binding"], request["record"]
    check(type(binding) is dict and set(binding) == BINDING_FIELDS, "audit_invalid_request")
    uid = binding["operator_uid"]
    check(type(uid) is int and uid > 0 and type(binding["initial_revision"]) is int and binding["initial_revision"] >= 1,
          "audit_invalid_request")
    for field in ("vm_id", "pool_id", "boot_id"):
        identifier(binding[field])
    identifier(request["nonce"])
    identifier(record["retirement_id"])
    boot = read_bytes(Path("/proc/sys/kernel/random/boot_id"), 64).decode().strip()
    check(boot == binding["boot_id"] == request["boot_id"], "audit_boot_changed")
    now = time.monotonic_ns()
    check(type(request["requested_monotonic_ns"]) is int and 0 <= now - request["requested_monotonic_ns"] <= 10_000_000_000,
          "audit_stale_request")
    check(request["phase"] in ("prepare", "before_delete", "final", "recovery"), "audit_invalid_request")
    pool = canonical(Path(pwd.getpwuid(uid).pw_dir) / ".local/share/cowork/executor-vm-capacity")
    check(entry(pool, uid)["kind"] == "directory", "audit_unsafe_metadata")
    data, _ = metadata(pool / "pool.json", uid)
    anchor, _ = metadata(pool / "identity.json", uid)
    selected = data
    if request["phase"] == "recovery":
        wal, _ = metadata(pool / "retirement-pending.json", uid, 4 * 1024 * 1024)
        check(wal.get("instance_id") == binding["vm_id"] and data in (wal["before"], wal["after"]), "audit_ledger_changed")
        selected = wal["after"]
        check(selected["revision"] == wal["before"]["revision"] + 1, "audit_ledger_changed")
        anchors = [dict(version=1, owner_uid=uid, account_home=pwd.getpwuid(uid).pw_dir,
            pool_id=v["plan"]["pool_id"], revision=v["revision"], ledger_sha256=sha(v)) for v in (wal["before"], wal["after"])]
        check(anchor in anchors, "audit_ledger_changed")
    else:
        check(anchor == dict(version=1, owner_uid=uid, account_home=pwd.getpwuid(uid).pw_dir,
            pool_id=data["plan"]["pool_id"], revision=data["revision"], ledger_sha256=sha(data)), "audit_ledger_changed")
    check(selected["owner_uid"] == uid and selected["account_home"] == pwd.getpwuid(uid).pw_dir
          and selected["plan"]["pool_id"] == binding["pool_id"]
          and selected["revision"] == request["revision"] and sha(selected) == request["ledger_sha256"], "audit_ledger_changed")
    allocation = record["allocation"]
    check(allocation["adopted"] is False and allocation["identity"]["version"] == 2
          and allocation["identity"]["id"] == binding["vm_id"] and allocation["identity"]["root"] == binding["root"]
          and binding["root"] not in selected["plan"]["existing_vm_roots"]
          and record["metadata_sha256"] == binding["metadata_sha256"], "audit_scope_changed")
    if request["phase"] == "prepare":
        check(selected["allocations"].get(binding["vm_id"]) == allocation
              and binding["vm_id"] not in selected.get("retirements", {}) and record["state"] == "prepared", "audit_scope_changed")
    else:
        check(selected["retirements"].get(binding["vm_id"]) == record, "audit_scope_changed")
    root = canonical(binding["root"])
    check(root.name == "cowork-executor-" + uuid.UUID(binding["vm_id"]).hex, "audit_scope_changed")
    quarantine = canonical(root.parent / (".retire-" + uuid.UUID(record["retirement_id"]).hex))
    check(str(quarantine) == record["quarantine"] and same(entry(root.parent, uid), record["parent"])
          and 0 < len(record["entries"]) <= 2048 and "vm.json" in record["entries"]
          and sha(record["entries"]) == record["inventory_sha256"], "audit_scope_changed")
    for name in record["entries"]:
        path = Path(name)
        check(not path.is_absolute() and str(path) == name and all(part not in (".", "..") for part in path.parts), "audit_scope_changed")
    original, moved = os.path.lexists(root), os.path.lexists(quarantine)
    if record["state"] == "prepared":
        check(original != moved, "audit_root_changed")
        observe_tree(root if original else quarantine, record, uid, pool)
    elif request["phase"] == "prepare":
        check(original and not moved, "audit_root_changed")
        observe_tree(root, record, uid, pool)
    elif original or moved:
        check(not original and moved, "audit_root_changed")
        observe_tree(quarantine, record, uid, pool)
    else:
        witness = proof(pool, record, ".", uid)
        check(witness is not None and all(proof(pool, record, name, uid) is not None for name in record["entries"]),
              "audit_delete_proof_missing")
        if record["state"] == "retired":
            check(sha(witness) == record["deletion_witness_sha256"], "audit_deletion_proof_changed")
    process_inventory(record)
    check(metadata(pool / "pool.json", uid)[0] == data and metadata(pool / "identity.json", uid)[0] == anchor,
          "audit_ledger_changed")
    check(read_bytes(Path("/proc/sys/kernel/random/boot_id"), 64).decode().strip() == boot, "audit_boot_changed")
    return {"version": 1, "verdict": "quiet", "auditor_uid": 0, "nonce": request["nonce"], "boot_id": boot,
            "request_sha256": sha(request), "observed_monotonic_ns": time.monotonic_ns()}


class Adapter:
    def __init__(self, profile, *, vm_id, root, metadata_sha256, revision):
        check(type(profile) is dict and set(profile) == FIELDS and type(profile["version"]) is int and profile["version"] == 1
              and profile["mode"] == "wsl_root_read_only_v1", "retirement_auditor_invalid_profile")
        binding = profile["allowed_binding"]
        check(type(binding) is dict and set(binding) == BINDING_FIELDS
              and type(binding["initial_revision"]) is int and type(binding["operator_uid"]) is int
              and binding["vm_id"] == vm_id and binding["root"] == str(root)
              and binding["metadata_sha256"] == metadata_sha256 and binding["initial_revision"] == revision
              and binding["operator_uid"] == os.getuid() and os.getuid() != 0, "retirement_auditor_scope_changed")
        for field in ("vm_id", "pool_id", "boot_id"):
            identifier(binding[field])
        check(profile["distribution"] == "Ubuntu-24.04", "retirement_auditor_untrusted_distribution")
        executable = canonical(profile["wsl_executable"])
        check(str(executable) == "/mnt/c/Windows/System32/wsl.exe", "retirement_auditor_untrusted_executable")
        source = read_bytes(Path(__file__).resolve(), 1024 * 1024)
        check(hashlib.sha256(source).hexdigest() == profile["auditor_sha256"], "retirement_auditor_source_changed")
        check(hashlib.sha256(read_bytes(executable, 16 * 1024 * 1024)).hexdigest() == profile["wsl_executable_sha256"],
              "retirement_auditor_executable_changed")
        self.profile, self.binding = profile, binding
        self.argv = [str(executable), "--distribution", profile["distribution"], "--user", "root", "--exec",
                     "/usr/bin/python3", "-I", "-S", "-B", "-c", source.decode()]
        check(len(subprocess.list2cmdline(self.argv).encode("utf-16-le")) // 2 < 32767,
              "retirement_auditor_source_too_large")

    def verify(self, record, data, phase):
        check(phase in ("prepare", "before_delete", "final", "recovery"), "retirement_auditor_invalid_phase")
        check(data["plan"]["pool_id"] == self.binding["pool_id"] and data["owner_uid"] == self.binding["operator_uid"]
              and record["allocation"]["identity"]["id"] == self.binding["vm_id"]
              and record["metadata_sha256"] == self.binding["metadata_sha256"], "retirement_auditor_scope_changed")
        boot = read_bytes(Path("/proc/sys/kernel/random/boot_id"), 64).decode().strip()
        check(boot == self.binding["boot_id"], "retirement_auditor_boot_changed")
        executable = canonical(self.profile["wsl_executable"])
        check(hashlib.sha256(read_bytes(executable, 16 * 1024 * 1024)).hexdigest()
              == self.profile["wsl_executable_sha256"], "retirement_auditor_executable_changed")
        started = time.monotonic_ns()
        request = {"version": 1, "binding": self.binding, "phase": phase, "record": record,
            "revision": data["revision"], "ledger_sha256": sha(data), "nonce": str(uuid.uuid4()),
            "boot_id": boot, "requested_monotonic_ns": started}
        encoded = json.dumps(request).encode()
        check(len(encoded) <= MAX_REQUEST, "retirement_auditor_request_too_large")
        env = {name: os.environ[name] for name in ("WSL_INTEROP", "WSL_DISTRO_NAME", "PATH", "SYSTEMROOT", "WINDIR") if name in os.environ}
        try:
            result = subprocess.run(self.argv, input=encoded, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                    timeout=10, env=env, check=False)
        except (OSError, subprocess.SubprocessError):
            raise RuntimeError("retirement_auditor_transport_failed") from None
        finished = time.monotonic_ns()
        check(len(result.stdout) <= 16384, "retirement_auditor_invalid_response")
        try:
            witness = json.loads(result.stdout)
        except (ValueError, UnicodeError):
            raise RuntimeError("retirement_auditor_invalid_response") from None
        if result.returncode != 0:
            if type(witness) is dict and set(witness) == {"version", "verdict", "reason"} \
                    and type(witness["version"]) is int and witness["version"] == 1 \
                    and witness["verdict"] == "refused" and witness["reason"] in REFUSAL_CODES:
                raise RuntimeError("retirement_auditor_refused:" + phase + ":" + witness["reason"])
            raise RuntimeError("retirement_auditor_refused")
        fields = {"version", "verdict", "auditor_uid", "nonce", "boot_id", "request_sha256", "observed_monotonic_ns"}
        check(type(witness) is dict and set(witness) == fields and type(witness["version"]) is int and witness["version"] == 1
              and witness["verdict"] == "quiet" and type(witness["auditor_uid"]) is int and witness["auditor_uid"] == 0
              and witness["nonce"] == request["nonce"] and witness["boot_id"] == boot
              and witness["request_sha256"] == sha(request), "retirement_auditor_response_binding_changed")
        check(type(witness["observed_monotonic_ns"]) is int and started <= witness["observed_monotonic_ns"] <= finished
              and finished - started <= 10_000_000_000, "retirement_auditor_response_stale")
        check(read_bytes(Path("/proc/sys/kernel/random/boot_id"), 64).decode().strip() == boot, "retirement_auditor_boot_changed")


def main():
    try:
        encoded = os.read(0, MAX_REQUEST + 1)
        # A pipe may return short reads; consume only the bounded request.
        while len(encoded) <= MAX_REQUEST:
            chunk = os.read(0, MAX_REQUEST + 1 - len(encoded))
            if not chunk:
                break
            encoded += chunk
        check(len(encoded) <= MAX_REQUEST, "audit_invalid_request")
        result = audit(json.loads(encoded))
    except Exception as error:
        reason = str(error) if type(error) is RuntimeError and str(error) in REFUSAL_CODES else "audit_unreadable"
        print(json.dumps({"version": 1, "verdict": "refused", "reason": reason}), flush=True)
        raise SystemExit(1)
    print(json.dumps(result), flush=True)


if __name__ == "__main__":
    main()
