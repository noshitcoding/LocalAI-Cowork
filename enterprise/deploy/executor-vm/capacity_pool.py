"""Owner-private, retained VM allocations within one Linux user's host plan.

This records an operator allocation plan, not exclusive physical coverage of
Windows or other WSL users/distributions. No allocation expires or is released
when a VM stops, disappears, or fails bootstrap.
"""
from contextlib import contextmanager
import copy
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import time
import uuid

VM_PREFIX = "cowork-executor-"
PLAN_FIELDS = {"version", "pool_id", "cpu_millis", "memory_mib",
               "reserved_cpu_millis", "reserved_memory_mib",
               "qemu_overhead_cpu_millis", "qemu_overhead_memory_mib",
               "inventory_complete", "existing_vm_roots"}
IDENTITY_FIELDS = ("version", "id", "organisation_id", "operator_state_id",
                   "cpus", "memory_mib", "ssh_port", "docker_port", "docker_endpoint")
RETIREMENT_FIELDS = {"version", "retirement_id", "state", "allocation", "metadata_sha256",
                     "parent", "root_entry", "entries", "inventory_sha256", "quarantine", "deletion_witness_sha256"}


def require(ok, reason):
    if not ok:
        raise RuntimeError(reason)


def account_home():
    import pwd
    require(os.name == "posix" and os.getuid() != 0, "use_an_unprivileged_linux_user")
    return Path(pwd.getpwuid(os.getuid()).pw_dir)


def canonical_pool_root():
    # Neither a selected VM parent nor environment HOME/XDG can select a pool.
    return account_home() / ".local/share/cowork/executor-vm-capacity"


def canonical_path(path):
    path = Path(path)
    require(path.is_absolute() and path == path.resolve(), "capacity_path_must_be_canonical")
    require(not path.is_symlink() and all(not p.is_symlink() for p in path.parents),
            "unsafe_capacity_path")
    return path


def private_directory(path):
    canonical_path(path)
    info = path.stat()
    require(stat.S_ISDIR(info.st_mode) and info.st_uid == os.getuid()
            and stat.S_IMODE(info.st_mode) == 0o700, "unsafe_capacity_directory")


def checked_file(path, fd):
    info = os.fstat(fd)
    present = path.lstat()
    require(stat.S_ISREG(info.st_mode) and info.st_uid == os.getuid()
            and stat.S_IMODE(info.st_mode) == 0o600 and info.st_nlink == 1
            and (info.st_dev, info.st_ino) == (present.st_dev, present.st_ino),
            "unsafe_capacity_file")


def read_private_json(path):
    canonical_path(path)
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    with os.fdopen(fd, "r") as stream:
        checked_file(path, stream.fileno())
        limit = (4 if path.name == "retirement-pending.json" else 2) * 1024 * 1024
        require(os.fstat(stream.fileno()).st_size <= limit,
                "capacity_file_too_large")
        return json.load(stream)


def atomic_json(path, value):
    if path.exists() or path.is_symlink():
        read_private_json(path)
    temporary = path.with_name("." + path.name + "." + uuid.uuid4().hex + ".tmp")
    encoded = json.dumps(value, indent=2) + "\n"
    limit = (4 if path.name == "retirement-pending.json" else 2) * 1024 * 1024
    require(len(encoded.encode()) <= limit, "capacity_file_too_large")
    try:
        fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        with os.fdopen(fd, "w") as stream:
            stream.write(encoded)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
        fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            os.fsync(fd)
        finally:
            os.close(fd)
    finally:
        temporary.unlink(missing_ok=True)


def ledger_digest(data):
    return hashlib.sha256(json.dumps(data, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def identity_fence(data):
    return {"version":1,"owner_uid":os.getuid(),"account_home":str(account_home()),
            "pool_id":data["plan"]["pool_id"],"revision":data["revision"],
            "ledger_sha256":ledger_digest(data)}


def sync_directory(path):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def validate_retirement(ident, record):
    require(type(record) is dict and set(record) == RETIREMENT_FIELDS and record["version"] == 1
            and type(record["version"]) is int and record["state"] in ("prepared", "deleting", "retired"),
            "invalid_capacity_retirement")
    valid_uuid(record["retirement_id"])
    allocation = record["allocation"]
    require(type(allocation) is dict and allocation.get("adopted") is False
            and allocation["identity"]["id"] == ident, "invalid_capacity_retirement")
    root = Path(allocation["identity"]["root"])
    require(record["quarantine"] == str(root.parent / (".retire-" + uuid.UUID(record["retirement_id"]).hex)),
            "invalid_capacity_retirement")
    require(type(record["metadata_sha256"]) is str and re.fullmatch(r"[0-9a-f]{64}", record["metadata_sha256"])
            and type(record["entries"]) is dict and 0 < len(record["entries"]) <= 2048
            and "vm.json" in record["entries"]
            and record["inventory_sha256"] == ledger_digest(record["entries"]), "invalid_capacity_retirement")
    for name, entry in {".": record["root_entry"], "..": record["parent"], **record["entries"]}.items():
        if name not in (".", ".."):
            parts = Path(name).parts
            require(not Path(name).is_absolute() and parts and all(part not in (".", "..") for part in parts)
                    and str(Path(name)) == name, "invalid_capacity_retirement_entry")
        require(type(entry) is dict and set(entry) == {"dev", "ino", "mode", "kind", "size", "mtime_ns"}
                and all(type(entry[field]) is int and entry[field] >= 0
                        for field in ("dev", "ino", "mode", "size", "mtime_ns"))
                and entry["kind"] in ("directory", "file", "socket"), "invalid_capacity_retirement_entry")
    require(record["root_entry"]["kind"] == record["parent"]["kind"] == "directory",
            "invalid_capacity_retirement_entry")
    witness = record["deletion_witness_sha256"]
    require((record["state"] != "retired" and witness is None)
            or (record["state"] == "retired" and type(witness) is str and re.fullmatch(r"[0-9a-f]{64}", witness)),
            "invalid_retirement_deletion_witness")


def deletion_scope(record):
    return ledger_digest({key: value for key, value in record.items()
                          if key not in ("state", "deletion_witness_sha256")})


def deletion_witness_path(root, record, relative):
    require(relative == "." or relative in record["entries"], "invalid_retirement_deletion_witness")
    base = root / "retirement-proofs"
    directory = base / uuid.UUID(record["retirement_id"]).hex
    return directory / (hashlib.sha256(relative.encode()).hexdigest() + ".json")


def expected_deletion_witness(record, relative):
    entry = record["root_entry"] if relative == "." else record["entries"][relative]
    return {"version": 1, "retirement_id": record["retirement_id"], "scope_sha256": deletion_scope(record),
            "relative_path": relative, "entry": entry, "operation": "rmdir" if entry["kind"] == "directory" else "unlink",
            "parent_fsynced": True, "observed": {key: entry[key] for key in ("dev", "ino", "mode")} | {"nlink": 0}}


def read_deletion_witness(root, record, relative):
    path = deletion_witness_path(root, record, relative)
    if not path.exists() and not path.is_symlink():
        return None
    private_directory(path.parent.parent)
    private_directory(path.parent)
    value = read_private_json(path)
    require(value == expected_deletion_witness(record, relative), "retirement_deletion_witness_changed")
    return value


def write_deletion_witness(root, record, relative, observed):
    expected = expected_deletion_witness(record, relative)
    # Called only with fstat from the still-open exact entry descriptor AFTER
    # unlink/rmdir and parent fsync. Path absence never manufactures a witness.
    require(observed == expected["observed"], "retirement_unlink_unconfirmed")
    path = deletion_witness_path(root, record, relative)
    if read_deletion_witness(root, record, relative) is not None:
        return
    for directory in (path.parent.parent, path.parent):
        if directory.exists() or directory.is_symlink():
            private_directory(directory)
        else:
            directory.mkdir(mode=0o700)
            sync_directory(directory.parent)
    atomic_json(path, expected)


def require_root_deletion_witness(root, record):
    value = read_deletion_witness(root, record, ".")
    require(value is not None, "retirement_delete_proof_missing_unknown")
    require(all(read_deletion_witness(root, record, name) is not None for name in record["entries"]),
            "retirement_delete_proof_missing_unknown")
    if record["state"] == "retired":
        require(ledger_digest(value) == record["deletion_witness_sha256"], "retirement_deletion_witness_changed")
    return ledger_digest(value)


def retirement_transition(before, after, ident):
    """The WAL may only add/advance one frozen retirement, never forget a charge."""
    CapacityPool(Path("/unused"), before)
    CapacityPool(Path("/unused"), after)
    valid_uuid(ident)
    require(after["revision"] == before["revision"] + 1 and after["version"] == 2,
            "invalid_retirement_transaction")
    previous = before.get("retirements", {}).get(ident)
    current = after["retirements"].get(ident)
    require(current is not None, "invalid_retirement_transaction")
    expected = copy.deepcopy(before)
    expected["version"] = 2
    expected.setdefault("retirements", {})
    expected["revision"] += 1
    if previous is None:
        require(current["state"] == "prepared" and current["allocation"] == before["allocations"].get(ident),
                "invalid_retirement_transaction")
    else:
        require((previous["state"], current["state"]) in (("prepared", "deleting"), ("deleting", "retired"))
                and {**previous, "state": current["state"], "deletion_witness_sha256": current["deletion_witness_sha256"]}
                == current, "invalid_retirement_transaction")
    expected["retirements"][ident] = current
    if current["state"] == "retired":
        del expected["allocations"][ident]
    require(expected == after, "invalid_retirement_transaction")


def retirement_tombstone(root, record):
    path = root / ("retirement-" + uuid.UUID(record["retirement_id"]).hex + ".json")
    if path.exists() or path.is_symlink():
        require(read_private_json(path) == record, "capacity_retirement_tombstone_changed")
        return
    # This file is append-only through this API; no overwrite or deletion path.
    # Atomic publication avoids a partial immutable file after a process crash.
    # All controller writers hold the canonical pool flock.
    atomic_json(path, record)


def recover_retirement(root, data, identity, *, enabled, expected_revision, expected_binding, verifier):
    path = root / "retirement-pending.json"
    if not path.exists() and not path.is_symlink():
        return data, identity
    value = read_private_json(path)
    require(type(value) is dict and set(value) == {"version", "instance_id", "before", "after"}
            and value["version"] == 1, "invalid_retirement_transaction")
    before, after = value["before"], value["after"]
    retirement_transition(before, after, value["instance_id"])
    require(data is not None and identity is not None
            and (data == before or data == after)
            and (identity == identity_fence(before) or identity == identity_fence(after)),
            "capacity_retirement_anchor_changed")
    require(enabled, "capacity_retirement_recovery_required:revision:" + str(after["revision"]))
    require(type(expected_revision) is int and expected_revision == after["revision"], "capacity_revision_conflict")
    record = after["retirements"][value["instance_id"]]
    require(expected_binding == {"id": value["instance_id"], "root": record["allocation"]["identity"]["root"],
                                 "metadata_sha256": record["metadata_sha256"]}, "capacity_retirement_recovery_binding_changed")
    parent = Path(record["allocation"]["identity"]["root"]).parent
    private_directory(parent)
    parent_info = parent.lstat()
    require((parent_info.st_dev, parent_info.st_ino, parent_info.st_mode)
            == tuple(record["parent"][field] for field in ("dev", "ino", "mode")),
            "capacity_retirement_parent_changed")
    if record["state"] == "retired":
        tombstone = root / ("retirement-" + uuid.UUID(record["retirement_id"]).hex + ".json")
        require(read_private_json(tombstone) == record, "capacity_retirement_tombstone_changed")
        require_root_deletion_witness(root, record)
        require(not os.path.lexists(record["allocation"]["identity"]["root"])
                and not os.path.lexists(record["quarantine"]), "capacity_retired_root_reappeared")
    require(verifier is not None, "retirement_recovery_verifier_required")
    verifier(record, after, "recovery")
    atomic_json(root / "identity.json", identity_fence(after))
    atomic_json(root / "pool.json", after)
    path.unlink()
    sync_directory(root)
    return after, identity_fence(after)


def positive(value, field, limit):
    require(type(value) is int and 0 < value <= limit, "invalid_capacity_plan:" + field)
    return value


def valid_uuid(value):
    require(type(value) is str and str(uuid.UUID(value)) == value
            and uuid.UUID(value).int != 0, "invalid_capacity_identity")
    return value


def validate_plan(plan):
    require(type(plan) is dict and set(plan) == PLAN_FIELDS and type(plan["version"]) is int
            and plan["version"] == 1, "invalid_capacity_plan")
    valid_uuid(plan["pool_id"])
    require(plan["inventory_complete"] is True and type(plan["existing_vm_roots"]) is list
            and len(set(plan["existing_vm_roots"])) == len(plan["existing_vm_roots"]),
            "capacity_complete_inventory_required")
    for root in plan["existing_vm_roots"]:
        path = canonical_path(Path(root))
        require(re.fullmatch(VM_PREFIX + r"[0-9a-f]{32}", path.name), "invalid_capacity_legacy_root")
    for field in ("cpu_millis", "reserved_cpu_millis", "qemu_overhead_cpu_millis"):
        positive(plan[field], field, 1_024_000)
    for field in ("memory_mib", "reserved_memory_mib", "qemu_overhead_memory_mib"):
        positive(plan[field], field, 16_777_216)
    require(plan["reserved_cpu_millis"] < plan["cpu_millis"]
            and plan["reserved_memory_mib"] < plan["memory_mib"], "capacity_reserve_exceeds_ceiling")
    return plan


def cgroup_v2_directories(proc_root=Path("/proc"), filesystem_root=Path("/")):
    """Locate our unified hierarchy, including a delegated/non-root mount."""
    entries = (proc_root / "self/cgroup").read_text().splitlines()
    unified = [line[3:] for line in entries if line.startswith("0::")]
    require(len(unified) == 1 and unified[0].startswith("/"), "cgroup_v2_required")
    group = Path(unified[0])
    require(".." not in group.parts, "invalid_cgroup_path")
    mounts = []
    for line in (proc_root / "self/mountinfo").read_text().splitlines():
        before, separator, after = line.partition(" - ")
        if separator and after.split()[0] == "cgroup2":
            fields = before.split()
            require(len(fields) >= 5 and "\\" not in fields[3] + fields[4],
                    "unsupported_cgroup_mount_path")
            mount_root, mount_path = Path(fields[3]), Path(fields[4])
            try:
                relative = group.relative_to(mount_root)
            except ValueError:
                continue
            mounts.append((len(mount_root.parts), mount_path, relative))
    require(bool(mounts), "cgroup_v2_mount_missing")
    _, mount, relative = max(mounts, key=lambda item: item[0])
    base = filesystem_root / mount.relative_to("/")
    current = base / relative
    require(current.is_dir(), "cgroup_v2_directory_missing")
    directories = []
    while True:
        directories.append(current)
        if current == base:
            return directories
        current = current.parent


def observed_bounds(proc_root=Path("/proc"), filesystem_root=Path("/")):
    """Conservative Linux ceiling; MemAvailable is deliberately not capacity."""
    text = (proc_root / "meminfo").read_text()
    match = re.search(r"^MemTotal:\s+([0-9]+) kB$", text, re.MULTILINE)
    require(match is not None, "linux_memory_ceiling_unavailable")
    memory = int(match[1]) // 1024
    cpu = len(os.sched_getaffinity(0)) * 1000
    for directory in cgroup_v2_directories(proc_root, filesystem_root):
        memory_path = directory / "memory.max"
        if memory_path.exists():
            value = memory_path.read_text().strip()
            require(value == "max" or (value.isdigit() and int(value) > 0), "invalid_cgroup_memory_ceiling")
            if value != "max":
                memory = min(memory, int(value) // (1024 * 1024))
        cpu_path = directory / "cpu.max"
        if cpu_path.exists():
            fields = cpu_path.read_text().split()
            require(len(fields) == 2 and fields[1].isdigit() and int(fields[1]) > 0
                    and (fields[0] == "max" or (fields[0].isdigit() and int(fields[0]) > 0)),
                    "invalid_cgroup_cpu_ceiling")
            if fields[0] != "max":
                cpu = min(cpu, int(fields[0]) * 1000 // int(fields[1]))
    require(cpu > 0 and memory > 0, "linux_capacity_ceiling_unavailable")
    return {"cpu_millis": cpu, "memory_mib": memory}


def snapshot(root, value):
    root = canonical_path(root)
    require(root.name == VM_PREFIX + uuid.UUID(valid_uuid(value["id"])).hex,
            "capacity_vm_identity_mismatch")
    valid_uuid(value["organisation_id"])
    valid_uuid(value["operator_state_id"])
    require(type(value["version"]) is int and value["version"] in (1, 2)
            and type(value["cpus"]) is int and value["cpus"] == 4
            and type(value["memory_mib"]) is int and value["memory_mib"] == 4096,
            "capacity_vm_profile_mismatch")
    return {"root": str(root), **{key: value[key] for key in IDENTITY_FIELDS}}


class CapacityPool:
    def __init__(self, root, data):
        self.root = root
        self.data = data
        if data is not None:
            self.validate()

    def validate(self):
        data = self.data
        fields = {"version", "owner_uid", "account_home", "revision", "plan", "parents", "allocations", "unadopted_roots"}
        require(type(data) is dict and data.get("version") in (1, 2)
                and set(data) == (fields | {"retirements"} if data.get("version") == 2 else fields)
                and type(data["version"]) is int and type(data["owner_uid"]) is int
                and data["owner_uid"] == os.getuid() and data["account_home"] == str(account_home())
                and type(data["revision"]) is int and data["revision"] >= 1,
                "invalid_capacity_ledger")
        plan = validate_plan(data["plan"])
        require(type(data["parents"]) is list and len(set(data["parents"])) == len(data["parents"])
                and type(data["allocations"]) is dict and type(data["unadopted_roots"]) is list
                and len(set(data["unadopted_roots"])) == len(data["unadopted_roots"])
                and set(data["unadopted_roots"]) <= set(plan["existing_vm_roots"]),
                "invalid_capacity_inventory")
        for parent in data["parents"]:
            canonical_path(Path(parent))
        roots = set()
        for ident, entry in data["allocations"].items():
            valid_uuid(ident)
            require(type(entry) is dict and set(entry) == {"identity", "cpu_millis", "memory_mib", "adopted"}
                    and type(entry["adopted"]) is bool, "invalid_capacity_allocation")
            identity = entry["identity"]
            require(type(identity) is dict and set(identity) == {"root", *IDENTITY_FIELDS},
                    "invalid_capacity_allocation_identity")
            root = Path(identity["root"])
            require(snapshot(root, identity) == identity and identity["id"] == ident
                    and str(root.parent) in data["parents"] and root not in roots,
                    "invalid_capacity_allocation_identity")
            roots.add(root)
            require(entry["cpu_millis"] == 4000 + plan["qemu_overhead_cpu_millis"]
                    and entry["memory_mib"] == 4096 + plan["qemu_overhead_memory_mib"],
                    "invalid_capacity_allocation_charge")
        require(not {str(root) for root in roots}.intersection(data["unadopted_roots"]),
                "invalid_capacity_inventory")
        require(set(plan["existing_vm_roots"]) <= {str(root) for root in roots}.union(data["unadopted_roots"]),
                "capacity_declared_inventory_omitted")
        retirements = data.get("retirements", {})
        require(type(retirements) is dict, "invalid_capacity_retirement")
        retired_roots, retired_states = set(), set()
        for ident, record in retirements.items():
            valid_uuid(ident)
            validate_retirement(ident, record)
            allocation = record["allocation"]
            identity = allocation["identity"]
            require(set(allocation) == {"identity", "cpu_millis", "memory_mib", "adopted"}
                    and set(identity) == {"root", *IDENTITY_FIELDS}
                    and snapshot(Path(identity["root"]), identity) == identity
                    and identity["version"] == 2 and str(Path(identity["root"]).parent) in data["parents"]
                    and identity["root"] not in plan["existing_vm_roots"]
                    and allocation["cpu_millis"] == 4000 + plan["qemu_overhead_cpu_millis"]
                    and allocation["memory_mib"] == 4096 + plan["qemu_overhead_memory_mib"],
                    "invalid_capacity_retirement_allocation")
            require(identity["root"] not in retired_roots and identity["operator_state_id"] not in retired_states,
                    "invalid_capacity_retirement")
            retired_roots.add(identity["root"])
            retired_states.add(identity["operator_state_id"])
            require((record["state"] == "retired" and ident not in data["allocations"])
                    or (record["state"] != "retired" and data["allocations"].get(ident) == allocation),
                    "capacity_retirement_charge_missing")
        self.require_fits(plan)

    def require_no_pending_retirements(self):
        require(all(record["state"] == "retired" for record in self.data.get("retirements", {}).values()),
                "capacity_retirement_in_progress")

    def check_tombstones(self):
        for record in self.data.get("retirements", {}).values():
            if record["state"] != "retired":
                continue
            path = self.root / ("retirement-" + uuid.UUID(record["retirement_id"]).hex + ".json")
            require(read_private_json(path) == record, "capacity_retirement_tombstone_changed")
            require_root_deletion_witness(self.root, record)
            for name in (record["allocation"]["identity"]["root"], record["quarantine"]):
                require(not os.path.lexists(name), "capacity_retired_root_reappeared")

    def commit_retirement(self, ident, record):
        before = copy.deepcopy(self.data)
        after = copy.deepcopy(before)
        after["version"] = 2
        after.setdefault("retirements", {})[ident] = copy.deepcopy(record)
        after["revision"] += 1
        if record["state"] == "retired":
            del after["allocations"][ident]
        retirement_transition(before, after, ident)
        if record["state"] == "retired":
            require_root_deletion_witness(self.root, record)
        path = self.root / "retirement-pending.json"
        require(not path.exists() and not path.is_symlink(), "capacity_retirement_recovery_required")
        # Before anchor advancement the complete old and new state are durable.
        atomic_json(path, {"version": 1, "instance_id": ident, "before": before, "after": after})
        atomic_json(self.root / "identity.json", identity_fence(after))
        atomic_json(self.root / "pool.json", after)
        path.unlink()
        sync_directory(self.root)
        self.data = after

    def configured(self):
        require(self.data is not None, "capacity_plan_required")

    def totals(self):
        self.configured()
        plan = self.data["plan"]
        pending = len(self.data["unadopted_roots"])
        return {"cpu_millis": sum(entry["cpu_millis"] for entry in self.data["allocations"].values())
                + pending * (4000 + plan["qemu_overhead_cpu_millis"]),
                "memory_mib": sum(entry["memory_mib"] for entry in self.data["allocations"].values())
                + pending * (4096 + plan["qemu_overhead_memory_mib"])}

    def require_fits(self, plan, *, extra_cpu=0, extra_memory=0):
        totals = self.totals()
        require(totals["cpu_millis"] + extra_cpu + plan["reserved_cpu_millis"] <= plan["cpu_millis"]
                and totals["memory_mib"] + extra_memory + plan["reserved_memory_mib"] <= plan["memory_mib"],
                "parent_capacity_exhausted")

    def require_bounds(self, plan=None):
        if plan is None:
            self.configured()
            plan = self.data["plan"]
        bounds = observed_bounds()
        require(plan["cpu_millis"] <= bounds["cpu_millis"] and plan["memory_mib"] <= bounds["memory_mib"],
                "capacity_plan_exceeds_linux_ceiling")
        return bounds

    def save(self):
        self.data["revision"] += 1
        self.validate()
        identity_path = self.root / "identity.json"
        if not identity_path.exists() and not identity_path.is_symlink():
            require(self.data["revision"] == 1 and not self.data["allocations"],
                    "capacity_identity_missing")
        # The revision/hash anchor is persisted first. Lost or older ledger
        # bytes cannot become a new empty pool; interrupted writes fail closed.
        atomic_json(identity_path, identity_fence(self.data))
        atomic_json(self.root / "pool.json", self.data)

    def configure(self, plan, expected_revision):
        validate_plan(plan)
        self.require_bounds(plan)
        actual = self.data["revision"] if self.data else 0
        require(type(expected_revision) is int and expected_revision == actual, "capacity_revision_conflict")
        if self.data:
            self.require_no_pending_retirements()
            previous = self.data["plan"]
            require(all(previous[field] == plan[field] for field in
                        ("pool_id", "qemu_overhead_cpu_millis", "qemu_overhead_memory_mib",
                         "inventory_complete", "existing_vm_roots")),
                    "capacity_plan_identity_and_overhead_are_immutable")
            self.require_fits(plan)
            self.data["plan"] = dict(plan)
        else:
            self.data = {"version": 1, "owner_uid": os.getuid(), "account_home": str(account_home()),
                         "revision": 0, "plan": dict(plan), "parents": [], "allocations": {},
                         "unadopted_roots": list(plan["existing_vm_roots"])}
        self.save()

    def check_inventory(self, loader, *, extra_parent=None):
        """No omitted/unresolved root can create apparent free capacity."""
        self.configured()
        self.require_no_pending_retirements()
        self.check_tombstones()
        require(not self.data["unadopted_roots"], "capacity_adoption_required")
        parents = {Path(parent) for parent in self.data["parents"]}
        if extra_parent is not None:
            parents.add(canonical_path(extra_parent))
        observed, ports = set(), set()
        for parent in sorted(parents):
            private_directory(parent)
            for root in sorted(parent.iterdir()):
                if not root.name.startswith(VM_PREFIX):
                    continue
                value = loader(root, allow_preparing=True)
                entry = self.data["allocations"].get(value["id"])
                require(entry is not None, "capacity_adoption_required")
                require(entry["identity"] == snapshot(root, value), "capacity_vm_binding_changed")
                require(not ports.intersection((value["ssh_port"], value["docker_port"])),
                        "capacity_inventory_port_conflict")
                ports.update((value["ssh_port"], value["docker_port"]))
                observed.add(value["id"])
        require(observed == set(self.data["allocations"]), "capacity_inventory_unresolved")

    def reserve(self, root, value, *, adopted=False):
        self.configured()
        self.require_no_pending_retirements()
        self.check_tombstones()
        self.require_bounds()
        identity = snapshot(root, value)
        require(all(ident != value["id"] and record["allocation"]["identity"]["root"] != identity["root"]
                    and record["allocation"]["identity"]["operator_state_id"] != identity["operator_state_id"]
                    for ident, record in self.data.get("retirements", {}).items()), "capacity_retired_binding_reuse")
        existing = self.data["allocations"].get(value["id"])
        if existing is not None:
            require(adopted and existing["identity"] == identity, "capacity_vm_already_registered")
            return
        require(all(entry["identity"]["root"] != identity["root"]
                    and entry["identity"]["operator_state_id"] != identity["operator_state_id"]
                    for entry in self.data["allocations"].values()), "capacity_vm_binding_conflict")
        plan = self.data["plan"]
        cpu, memory = 4000 + plan["qemu_overhead_cpu_millis"], 4096 + plan["qemu_overhead_memory_mib"]
        pending = identity["root"] in self.data["unadopted_roots"]
        require(not pending or adopted, "capacity_adoption_required")
        self.require_fits(plan, extra_cpu=0 if pending else cpu, extra_memory=0 if pending else memory)
        parent = str(root.parent)
        if parent not in self.data["parents"]:
            self.data["parents"].append(parent)
            self.data["parents"].sort()
        self.data["allocations"][value["id"]] = {"identity": identity, "cpu_millis": cpu,
                                                "memory_mib": memory, "adopted": adopted}
        if pending:
            self.data["unadopted_roots"].remove(identity["root"])
        # Persist before publishing VM metadata. A crash afterwards retains the
        # charge and becomes unresolved, never silently frees the allocation.
        self.save()

    def require_registered(self, root, value):
        self.configured()
        self.require_no_pending_retirements()
        self.check_tombstones()
        self.require_bounds()
        entry = self.data["allocations"].get(value["id"])
        require(entry is not None, "capacity_adoption_required")
        require(entry["identity"] == snapshot(root, value), "capacity_vm_binding_changed")


@contextmanager
def locked_pool(*, allow_unconfigured=False, recover_pending_retirement=False, expected_retirement_revision=None,
                expected_retirement_binding=None, retirement_recovery_verifier=None):
    import fcntl
    root = canonical_path(canonical_pool_root())
    root.mkdir(mode=0o700, parents=True, exist_ok=True)
    private_directory(root)
    lock = root / ".capacity.lock"
    fd = os.open(lock, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
    try:
        checked_file(lock, fd)
        deadline = time.monotonic() + 5
        while True:
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except BlockingIOError:
                require(time.monotonic() < deadline, "capacity_pool_lock_timeout")
                time.sleep(.025)
        checked_file(lock, fd)
        path = root / "pool.json"
        data = read_private_json(path) if path.exists() or path.is_symlink() else None
        identity_path = root / "identity.json"
        identity = read_private_json(identity_path) if identity_path.exists() or identity_path.is_symlink() else None
        data, identity = recover_retirement(root, data, identity, enabled=recover_pending_retirement,
                                           expected_revision=expected_retirement_revision,
                                           expected_binding=expected_retirement_binding,
                                           verifier=retirement_recovery_verifier)
        require(data is not None or identity is None, "capacity_ledger_missing")
        require(data is None or identity is not None, "capacity_identity_missing")
        if identity is not None:
            require(identity == identity_fence(data), "capacity_pool_identity_changed")
        pool = CapacityPool(root, data)
        if not allow_unconfigured:
            pool.configured()
        if data is not None:
            pool.check_tombstones()
        yield pool
    finally:
        os.close(fd)
