#!/usr/bin/env python3
"""Create and qualify one private local KVM executor candidate, never a fleet.

Run as an unprivileged Linux user with /dev/kvm access. Keys, images and writable
disks live outside the repository. The parent Docker daemon is only inspected
and used to export one immutable sandbox image; no existing workloads are changed.
"""
import argparse
from contextlib import contextmanager
import errno
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import shutil
import socket
import ssl
import stat
import subprocess
import sys
import tarfile
import time
from urllib.request import urlopen
import uuid

# The qualification harness also loads this controller by absolute file path.
# Always load our exact companion, independent of cwd or module search paths.
_capacity_path = Path(__file__).resolve().with_name("capacity_pool.py")
_capacity_name = "_cowork_vm_capacity_pool_" + hashlib.sha256(str(_capacity_path).encode()).hexdigest()
capacity_pool = sys.modules.get(_capacity_name)
if capacity_pool is None:
    _capacity_spec = importlib.util.spec_from_file_location(_capacity_name, _capacity_path)
    capacity_pool = importlib.util.module_from_spec(_capacity_spec)
    sys.modules[_capacity_name] = capacity_pool
    _capacity_spec.loader.exec_module(capacity_pool)

IMAGE_BASE = "https://cloud-images.ubuntu.com/noble/current/"
IMAGE_NAME = "noble-server-cloudimg-amd64.img"
VM_PREFIX = "cowork-executor-"
LOCK_NAME = ".ports.lock"

def require(ok, message):
    if not ok:
        raise RuntimeError(message)

def call(argv, *, data=None, timeout=120, cwd=None):
    result = subprocess.run(argv, input=data, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=timeout, cwd=cwd)
    require(result.returncode == 0, "command_failed:" + Path(argv[0]).name)
    return result.stdout

def write(path, value):
    if isinstance(value, str):
        value = value.encode()
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "wb") as output:
        output.write(value)

def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()

def private_directory(path, message):
    require(path.is_absolute() and not path.is_symlink(), message)
    require(all(not item.is_symlink() for item in path.parents), message)
    info = path.stat()
    require(stat.S_ISDIR(info.st_mode) and info.st_uid == os.getuid()
            and stat.S_IMODE(info.st_mode) == 0o700, message)

def private_file(path, message):
    info = path.lstat()
    require(stat.S_ISREG(info.st_mode) and info.st_uid == os.getuid()
            and stat.S_IMODE(info.st_mode) == 0o600 and info.st_nlink == 1, message)

def port_number(value, *, auto=False):
    require(type(value) is int and ((auto and value == 0) or 1024 <= value <= 65535), "invalid_host_port")
    return value

def sync_directory(path):
    directory = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)

def atomic_json(path, value):
    if path.exists() or path.is_symlink():
        private_file(path, "unsafe_vm_metadata")
    temporary = path.with_name("." + path.name + "." + uuid.uuid4().hex + ".tmp")
    try:
        write(temporary, json.dumps(value, indent=2) + "\n")
        with temporary.open("rb") as stream:
            os.fsync(stream.fileno())
        os.replace(temporary, path)
        sync_directory(path.parent)
    finally:
        temporary.unlink(missing_ok=True)

def load(root, *, allow_preparing=False):
    require(os.name == "posix" and os.getuid() != 0, "use_an_unprivileged_linux_user")
    private_directory(root, "vm_root_requires_owner_and_0700")
    private_file(root / "vm.json", "unsafe_vm_metadata")
    value = json.loads((root / "vm.json").read_text())
    require(type(value.get("version")) is int and value["version"] in (1, 2), "unsupported_vm_metadata_version")
    require(str(uuid.UUID(value["id"])) == value["id"] and root.name == VM_PREFIX + uuid.UUID(value["id"]).hex, "vm_root_identity_mismatch")
    require(str(uuid.UUID(value["organisation_id"])) == value["organisation_id"], "vm_organisation_identity_mismatch")
    require(value["memory_mib"] == 4096 and value["cpus"] == 4, "unexpected_vm_capacity")
    port_number(value["ssh_port"])
    match = re.fullmatch(r"tcp://localhost:([0-9]+)", value["docker_endpoint"])
    require(match is not None, "unsafe_vm_docker_endpoint")
    docker_port = port_number(int(match[1]))
    require(value["docker_endpoint"] == "tcp://localhost:" + str(docker_port), "unsafe_vm_docker_endpoint")
    if value["version"] == 1:
        require(docker_port == 2376, "unexpected_legacy_docker_port")
    else:
        require(port_number(value["docker_port"]) == docker_port, "vm_docker_port_mismatch")
        require(value.get("bootstrap_state") in ("preparing", "ready"), "invalid_vm_bootstrap_state")
        require(allow_preparing or value["bootstrap_state"] == "ready", "vm_bootstrap_incomplete")
    require(value["ssh_port"] != docker_port, "vm_ports_must_differ")
    value["docker_port"] = docker_port
    # The retained v1 pilot has immutable VM identity but no separate state ID.
    # Derive its stable ID without changing any existing private metadata.
    if value["version"] == 1 and "operator_state_id" not in value:
        value["operator_state_id"] = str(uuid.uuid5(uuid.UUID(value["id"]), "cowork-operator-state-v1"))
    require(str(uuid.UUID(value["operator_state_id"])) == value["operator_state_id"], "vm_operator_state_identity_mismatch")
    return value

@contextmanager
def inventory_lock(parent):
    import fcntl
    private_directory(parent, "vm_parent_requires_owner_and_0700")
    path = parent / LOCK_NAME
    fd = os.open(path, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
    try:
        private_file(path, "unsafe_vm_inventory_lock")
        info = os.fstat(fd)
        require((info.st_dev, info.st_ino) == (path.stat().st_dev, path.stat().st_ino), "vm_inventory_lock_changed")
        deadline = time.monotonic() + 5
        while True:
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except BlockingIOError:
                require(time.monotonic() < deadline, "vm_inventory_lock_timeout")
                time.sleep(.025)
        private_directory(parent, "vm_parent_requires_owner_and_0700")
        yield
    finally:
        os.close(fd)

def inventory_ports(parent):
    """Read only trusted VM records, including stopped and unfinished VMs."""
    reserved = {}
    for root in sorted(parent.iterdir()):
        if not root.name.startswith(VM_PREFIX):
            continue
        require(re.fullmatch(VM_PREFIX + r"[0-9a-f]{32}", root.name), "unsafe_vm_inventory_entry")
        value = load(root, allow_preparing=True)
        for port in (value["ssh_port"], value["docker_port"]):
            require(port not in reserved, "vm_inventory_port_conflict")
            reserved[port] = value["id"]
    return reserved

def bind_loopback_port(requested, reserved):
    port_number(requested, auto=True)
    require(requested == 0 or requested not in reserved, "vm_port_already_reserved")
    for _ in range(128):
        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        try:
            sock.bind(("127.0.0.1", requested))
            port = sock.getsockname()[1]
            if port in reserved:
                sock.close()
                continue
            return sock, port
        except OSError:
            sock.close()
            raise RuntimeError("vm_loopback_port_unavailable") from None
    raise RuntimeError("no_unreserved_loopback_port")

def reserve_vm_ports(parent, organisation, docker_port=0):
    require(os.name == "posix" and os.getuid() != 0, "use_an_unprivileged_linux_user")
    organisation = str(uuid.UUID(organisation))
    port_number(docker_port, auto=True)
    parent = capacity_pool.canonical_path(parent.absolute())
    require(re.fullmatch(r"/[A-Za-z0-9_./-]+", str(parent)), "vm_parent_requires_simple_linux_path")
    require(not parent.is_symlink() and all(not item.is_symlink() for item in parent.parents), "unsafe_vm_parent")
    parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    # One account-wide capacity lock always precedes a parent inventory lock.
    with capacity_pool.locked_pool() as pool:
        with inventory_lock(parent):
            reserved = inventory_ports(parent)
            pool.check_inventory(load, extra_parent=parent)
            reserved.update({port: ident for ident, entry in pool.data["allocations"].items()
                             for port in (entry["identity"]["ssh_port"], entry["identity"]["docker_port"])})
            docker_socket, docker_port = bind_loopback_port(docker_port, reserved)
            try:
                ssh_socket, ssh_port = bind_loopback_port(0, {*reserved, docker_port})
                try:
                    ident = str(uuid.uuid4())
                    root = parent / (VM_PREFIX + uuid.UUID(ident).hex)
                    value = {"version": 2, "id": ident, "organisation_id": organisation,
                             "operator_state_id": str(uuid.uuid4()), "bootstrap_state": "preparing",
                             "memory_mib": 4096, "cpus": 4, "disk_virtual_gib": 20,
                             "ssh_port": ssh_port, "docker_port": docker_port,
                             "docker_endpoint": "tcp://localhost:" + str(docker_port),
                             "ubuntu_image_sha256": None, "ubuntu_image_url": IMAGE_BASE + IMAGE_NAME}
                    pool.reserve(root, value)
                    root.mkdir(mode=0o700)
                    # Capacity is durable before this early port reservation;
                    # every failure retains both for explicit operator review.
                    atomic_json(root / "vm.json", value)
                    sync_directory(parent)
                finally:
                    ssh_socket.close()
            finally:
                docker_socket.close()
    return root, value

def install_cli(root, version):
    load(root)
    require(re.fullmatch(r"\d+\.\d+\.\d+", version), "invalid_docker_cli_version")
    require(os.uname().machine == "x86_64", "docker_cli_architecture_mismatch")
    tools = root / "tools"
    tools.mkdir(mode=0o700)
    url = "https://download.docker.com/linux/static/stable/x86_64/docker-" + version + ".tgz"
    payload = urlopen(url, timeout=120).read(128 * 1024 * 1024 + 1)
    require(len(payload) <= 128 * 1024 * 1024, "docker_cli_archive_too_large")
    with tarfile.open(fileobj=io.BytesIO(payload), mode="r:gz") as archive:
        member = archive.getmember("docker/docker")
        require(member.isfile() and member.size <= 100 * 1024 * 1024, "unsafe_docker_cli_archive_member")
        write(tools / "docker", archive.extractfile(member).read())
    (tools / "docker").chmod(0o700)
    write(tools / "client.json", json.dumps({"version": version, "url": url, "downloaded_archive_sha256": hashlib.sha256(payload).hexdigest(), "client_sha256": digest(tools / "docker")}, indent=2))
    print("private_test_client_installed=true", flush=True)

def openssl(root):
    for name in ("ca", "server", "client"):
        if name == "ca":
            call(["openssl", "req", "-x509", "-newkey", "rsa:3072", "-nodes", "-sha256", "-days", "90", "-subj", "/CN=Cowork local executor CA", "-keyout", str(root / "ca-key.pem"), "-out", str(root / "ca.pem")])
        else:
            call(["openssl", "req", "-newkey", "rsa:3072", "-nodes", "-sha256", "-subj", "/CN=Cowork executor " + name, "-keyout", str(root / (name + "-key.pem")), "-out", str(root / (name + ".csr"))])
            ext = "extendedKeyUsage=" + ("serverAuth" if name == "server" else "clientAuth") + "\n"
            if name == "server":
                ext += "subjectAltName=DNS:localhost,IP:127.0.0.1\n"
            write(root / (name + ".ext"), ext)
            call(["openssl", "x509", "-req", "-in", str(root / (name + ".csr")), "-CA", str(root / "ca.pem"), "-CAkey", str(root / "ca-key.pem"), "-CAcreateserial", "-out", str(root / (name + ".pem")), "-days", "90", "-sha256", "-extfile", str(root / (name + ".ext"))])
    for path in root.glob("*.pem"):
        path.chmod(0o600)

def create(parent, organisation, docker_port=0):
    require(os.name == "posix" and os.getuid() != 0, "use_an_unprivileged_linux_user")
    port_number(docker_port, auto=True)
    require(os.access("/dev/kvm", os.R_OK | os.W_OK), "kvm_access_required")
    for command in ("qemu-system-x86_64", "qemu-img", "cloud-localds", "openssl", "ssh", "ssh-keygen", "docker"):
        require(shutil.which(command), "missing_tool:" + command)
    root, value = reserve_vm_ports(parent, organisation, docker_port)
    ident = value["id"]
    organisation = value["organisation_id"]
    (root / "shared").mkdir(mode=0o700)
    print("vm_root=" + str(root), flush=True)
    sums = urlopen(IMAGE_BASE + "SHA256SUMS", timeout=60).read().decode()
    matching = [line.split()[0] for line in sums.splitlines() if line.split()[-1].lstrip("*") == IMAGE_NAME]
    require(len(matching) == 1 and re.fullmatch("[0-9a-f]{64}", matching[0]), "invalid_published_image_checksum")
    write(root / "SHA256SUMS", sums)
    with urlopen(IMAGE_BASE + IMAGE_NAME, timeout=120) as source, (root / "base.qcow2").open("xb") as target:
        shutil.copyfileobj(source, target, 1024 * 1024)
    require(digest(root / "base.qcow2") == matching[0], "ubuntu_image_checksum_mismatch")
    (root / "base.qcow2").chmod(0o400)
    call(["qemu-img", "create", "-f", "qcow2", "-F", "qcow2", "-b", str(root / "base.qcow2"), str(root / "disk.qcow2"), "20G"])
    (root / "disk.qcow2").chmod(0o600)
    openssl(root)
    call(["ssh-keygen", "-t", "ed25519", "-N", "", "-C", "cowork-local-executor", "-f", str(root / "ssh-key")])
    shared = str(root / "shared")
    daemon = {"hosts": ["unix:///var/run/docker.sock", "tcp://0.0.0.0:2376"], "tlsverify": True,
              "tlscacert": "/etc/docker/tls/ca.pem", "tlscert": "/etc/docker/tls/server.pem", "tlskey": "/etc/docker/tls/server-key.pem",
              "labels": ["cowork.role=executor", "cowork.trust-domain=" + organisation],
              "log-driver": "json-file", "log-opts": {"max-size": "10m", "max-file": "3"}}
    files = []
    for source, dest in (("ca.pem", "ca.pem"), ("server.pem", "server.pem"), ("server-key.pem", "server-key.pem")):
        files.append({"path": "/etc/docker/tls/" + dest, "permissions": "0600", "owner": "root:root", "content": (root / source).read_text()})
    files += [{"path": "/etc/docker/daemon.json", "permissions": "0600", "content": json.dumps(daemon)},
              {"path": "/etc/systemd/system/docker.service.d/cowork.conf", "permissions": "0644", "content": "[Service]\nExecStart=\nExecStart=/usr/bin/dockerd --containerd=/run/containerd/containerd.sock\n"}]
    data = {"hostname": "cowork-executor-" + uuid.UUID(ident).hex[:8], "manage_etc_hosts": True, "disable_root": True, "ssh_pwauth": False,
            "users": [{"name": "cowork", "groups": ["sudo"], "shell": "/bin/bash", "lock_passwd": True,
                       "sudo": "ALL=(ALL) NOPASSWD:ALL", "ssh_authorized_keys": [(root / "ssh-key.pub").read_text().strip()]}],
            "package_update": True, "package_upgrade": False, "packages": ["docker.io", "openssh-server", "ca-certificates", "python3"], "write_files": files,
            "mounts": [["cowork_state", shared, "9p", "trans=virtio,version=9p2000.L,ro,msize=262144,nofail", "0", "0"]],
            "runcmd": [["mkdir", "-p", shared], ["bash", "-c", "mountpoint -q " + shared + " || mount -t 9p -o trans=virtio,version=9p2000.L,ro,msize=262144 cowork_state " + shared],
                       ["systemctl", "daemon-reload"], ["systemctl", "restart", "docker"], ["systemctl", "enable", "docker"]]}
    write(root / "user-data", "#cloud-config\n" + json.dumps(data, indent=2))
    write(root / "meta-data", "instance-id: " + ident + "\nlocal-hostname: cowork-executor-" + uuid.UUID(ident).hex[:8] + "\n")
    call(["cloud-localds", str(root / "seed.iso"), str(root / "user-data"), str(root / "meta-data")])
    (root / "seed.iso").chmod(0o600)
    value.update({"bootstrap_state": "ready", "ubuntu_image_sha256": matching[0]})
    atomic_json(root / "vm.json", value)
    return root

def process(root):
    path = root / "qemu.pid"
    if not path.exists():
        return None
    pid = int(path.read_text().strip())
    try:
        if Path("/proc/" + str(pid) + "/stat").read_text().split()[2] == "Z":
            return None
        text = Path("/proc/" + str(pid) + "/cmdline").read_bytes().split(b"\0")
        require(str(root / "disk.qcow2").encode() in b" ".join(text) and b"qemu-system" in text[0], "refuse_foreign_qemu_pid")
        return pid
    except FileNotFoundError:
        return None

def qmp(root, command):
    private_directory(root, "vm_root_requires_owner_and_0700")
    require(process(root) is not None, "vm_not_running")
    # Linux resolves this short path through an owned directory descriptor;
    # sockaddr_un has a length limit even when the filesystem path is valid.
    directory_fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        return qmp_at(directory_fd, command)
    finally:
        os.close(directory_fd)

def qmp_at(directory_fd, command):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
        sock.settimeout(10)
        sock.connect("/proc/self/fd/" + str(directory_fd) + "/qmp.sock")
        stream = sock.makefile("rwb")
        json.loads(stream.readline())
        for cmd in ("qmp_capabilities", command):
            stream.write((json.dumps({"execute": cmd}) + "\n").encode()); stream.flush()
            while True:
                result = json.loads(stream.readline())
                if "return" in result:
                    break
                require("error" not in result, "qmp_command_failed")
        return result["return"]

def qemu_arguments(root, value):
    return ["qemu-system-x86_64", "-name", VM_PREFIX + uuid.UUID(value["id"]).hex, "-uuid", value["id"],
          "-machine", "q35,accel=kvm", "-cpu", "host", "-smp", "4", "-m", "4096", "-display", "none", "-daemonize",
          "-pidfile", str(root / "qemu.pid"), "-qmp", "unix:qmp.sock,server=on,wait=off",
          "-serial", "file:" + str(root / "console.log"), "-drive", "file=" + str(root / "disk.qcow2") + ",if=virtio,format=qcow2",
          "-drive", "file=" + str(root / "seed.iso") + ",if=virtio,format=raw,readonly=on",
          "-netdev", "user,id=net,hostfwd=tcp:127.0.0.1:" + str(value["ssh_port"]) + "-:22,hostfwd=tcp:127.0.0.1:" + str(value["docker_port"]) + "-:2376",
          "-device", "virtio-net-pci,netdev=net", "-fsdev", "local,id=state,path=" + str(root / "shared") + ",security_model=mapped-xattr,readonly=on",
          "-device", "virtio-9p-pci,fsdev=state,mount_tag=cowork_state"]

def start(root):
    value = load(root)
    with capacity_pool.locked_pool() as pool:
        with inventory_lock(root.parent):
            reserved = inventory_ports(root.parent)
            pool.require_registered(root, value)
            pool.check_inventory(load)
            require(all(reserved.get(port) == value["id"] for port in (value["ssh_port"], value["docker_port"])), "vm_port_reservation_missing")
            if process(root) is not None:
                return
            docker_socket, _ = bind_loopback_port(value["docker_port"], set())
            try:
                ssh_socket, _ = bind_loopback_port(value["ssh_port"], set())
                ssh_socket.close()
            finally:
                docker_socket.close()
            for name in ("qmp.sock", "qemu.pid"):
                (root / name).unlink(missing_ok=True)
            # Serialize concurrent starts of this identity through daemon launch.
            call(qemu_arguments(root, value), timeout=10, cwd=root)
    require(qmp(root, "query-kvm")["enabled"], "kvm_acceleration_not_enabled")
    print("vm_started=true", flush=True)

def ssh(root, command, timeout=30):
    value = load(root)
    return call(["ssh", "-i", str(root / "ssh-key"), "-p", str(value["ssh_port"]), "-o", "BatchMode=yes", "-o", "IdentitiesOnly=yes",
                 "-o", "StrictHostKeyChecking=accept-new", "-o", "UserKnownHostsFile=" + str(root / "known_hosts"), "-o", "ConnectTimeout=4",
                 "cowork@127.0.0.1", command], timeout=timeout)

def stop(root):
    load(root)
    if process(root) is None:
        return
    qmp(root, "system_powerdown")
    deadline = time.monotonic() + 45
    while process(root) is not None and time.monotonic() < deadline:
        time.sleep(.5)
    if process(root) is not None:
        qmp(root, "quit")
    deadline = time.monotonic() + 10
    while process(root) is not None and time.monotonic() < deadline:
        time.sleep(.2)
    require(process(root) is None, "vm_shutdown_unconfirmed")
    print("vm_stopped=true", flush=True)


def retirement_entry(info):
    require(info.st_uid == os.getuid() and not info.st_mode & 0o022,
            "retirement_foreign_or_writable_entry")
    if stat.S_ISDIR(info.st_mode):
        require(stat.S_IMODE(info.st_mode) == 0o700, "retirement_directory_not_private")
        kind = "directory"
    elif stat.S_ISREG(info.st_mode):
        require(info.st_nlink == 1, "retirement_hardlink_not_allowed")
        kind = "file"
    elif stat.S_ISSOCK(info.st_mode):
        kind = "socket"
    else:
        raise RuntimeError("retirement_unsupported_entry")
    return {"dev": info.st_dev, "ino": info.st_ino, "mode": info.st_mode,
            "kind": kind, "size": info.st_size, "mtime_ns": info.st_mtime_ns}


def retirement_tree(root):
    """Only describe entries; private file contents (other than vm.json) stay unread."""
    capacity_pool.private_directory(root)
    parent = retirement_entry(root.parent.lstat())
    root_entry = retirement_entry(root.lstat())
    require(root_entry["dev"] == parent["dev"], "retirement_mount_not_allowed")
    entries = {}
    def visit(directory, prefix):
        for name in sorted(os.listdir(directory)):
            relative = prefix + name
            require(len(entries) < 2048, "retirement_inventory_too_large")
            info = os.stat(name, dir_fd=directory, follow_symlinks=False)
            entry = retirement_entry(info)
            require(entry["dev"] == root_entry["dev"], "retirement_mount_not_allowed")
            require(entry["kind"] != "socket" or relative == "qmp.sock", "retirement_unknown_socket")
            entries[relative] = entry
            if entry["kind"] == "directory":
                child = os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=directory)
                try:
                    require(retirement_entry(os.fstat(child)) == entry, "retirement_entry_changed")
                    visit(child, relative + "/")
                finally:
                    os.close(child)
    directory = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        require(retirement_entry(os.fstat(directory)) == root_entry, "retirement_entry_changed")
        visit(directory, "")
    finally:
        os.close(directory)
    return parent, root_entry, entries


def retirement_entry_matches(actual, frozen):
    # Removing children necessarily changes directory size/mtime, not identity.
    fields = ("dev", "ino", "mode", "kind") if frozen["kind"] == "directory" else tuple(frozen)
    return all(actual[field] == frozen[field] for field in fields)


def retirement_check_tree(root, record, *, proof_root):
    parent, root_entry, entries = retirement_tree(root)
    require(retirement_entry_matches(parent, record["parent"])
            and retirement_entry_matches(root_entry, record["root_entry"]), "retirement_root_binding_changed")
    require(set(entries) <= set(record["entries"])
            and all(retirement_entry_matches(entry, record["entries"][name]) for name, entry in entries.items()),
            "retirement_inventory_changed")
    if record["state"] == "prepared":
        require(set(entries) == set(record["entries"]), "retirement_inventory_changed")
    else:
        for name in record["entries"]:
            proof = capacity_pool.read_deletion_witness(proof_root, record, name)
            require(proof is None if name in entries else proof is not None,
                    "retirement_deleted_entry_reappeared" if name in entries else "retirement_delete_proof_missing_unknown")
    require(capacity_pool.read_deletion_witness(proof_root, record, ".") is None,
            "retirement_deleted_root_reappeared")
    if "vm.json" in entries:
        require(retirement_metadata_sha(root) == record["metadata_sha256"], "retirement_metadata_changed")
    return entries


def retirement_metadata_sha(root):
    path = root / "vm.json"
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    with os.fdopen(fd, "rb") as stream:
        capacity_pool.checked_file(path, stream.fileno())
        require(os.fstat(stream.fileno()).st_size <= 2 * 1024 * 1024, "retirement_metadata_too_large")
        return hashlib.sha256(stream.read()).hexdigest()


def retirement_quiet(record, *, proc_root=Path("/proc")):
    """Verify no process of this account uses the frozen root, mappings or disks.

    Privileged OS administrators and uncoordinated hostile same-UID programs are
    outside this operator controller's trust boundary. Inaccessible own-account
    process evidence is an error, never evidence that a VM is stopped.
    """
    paths = (Path(record["allocation"]["identity"]["root"]), Path(record["quarantine"]))
    deadline = time.monotonic() + 10
    def bounded_text(path, limit):
        with path.open("r") as stream:
            text = stream.read(limit + 1)
        require(len(text) <= limit, "retirement_process_inventory_too_large")
        return text
    inodes = {(entry["dev"], entry["ino"]) for entry in
              [record["root_entry"], *record["entries"].values()]}
    def within(text):
        text = text.removesuffix(" (deleted)")
        return any(text == str(path) or text.startswith(str(path) + "/") for path in paths)
    for root in paths:
        if not os.path.lexists(root):
            continue
        capacity_pool.private_directory(root)
        pidfile = root / "qemu.pid"
        if os.path.lexists(pidfile):
            private_file(pidfile, "retirement_unsafe_pidfile")
            require(pidfile.stat().st_size <= 32, "retirement_invalid_pidfile")
            pid = pidfile.read_text().strip()
            require(pid.isdigit() and int(pid) > 0, "retirement_invalid_pidfile")
            require(not (proc_root / pid).exists(), "retirement_recorded_process_alive")
        sockfile = root / "qmp.sock"
        if os.path.lexists(sockfile):
            entry = retirement_entry(sockfile.lstat())
            require(entry["kind"] == "socket", "retirement_unsafe_qmp_socket")
            fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
            try:
                with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
                    sock.settimeout(.25)
                    try:
                        sock.connect("/proc/self/fd/" + str(fd) + "/qmp.sock")
                    except OSError as error:
                        require(error.errno == errno.ECONNREFUSED, "retirement_qmp_state_unknown")
                    else:
                        raise RuntimeError("retirement_qmp_listener_alive")
            finally:
                os.close(fd)
    # Root-private paths must not be mounted (including bind mounts of subtrees).
    mountinfo = bounded_text(proc_root / "self/mountinfo", 4 * 1024 * 1024)
    for line in mountinfo.splitlines():
        fields = line.split()
        require(len(fields) >= 6, "retirement_mount_inventory_unknown")
        unescaped = re.sub(r"\\([0-7]{3})", lambda match: chr(int(match[1], 8)), fields[4])
        require(not within(unescaped), "retirement_mount_not_allowed")
    processes = [path for path in proc_root.iterdir() if path.name.isdigit()]
    require(len(processes) <= 32768, "retirement_process_inventory_too_large")
    descriptors = 0
    for process_dir in sorted(processes):
        require(time.monotonic() < deadline, "retirement_process_inventory_deadline")
        if not process_dir.name.isdigit():
            continue
        try:
            status = bounded_text(process_dir / "status", 64 * 1024)
            match = re.search(r"^Uid:\s+([0-9]+)\s+([0-9]+)\s+([0-9]+)\s+([0-9]+)$", status, re.MULTILINE)
            require(match is not None, "retirement_process_inventory_unknown")
            if os.getuid() not in (int(value) for value in match.groups()):
                continue
            names = os.listdir(process_dir / "fd")
            descriptors += len(names)
            require(descriptors <= 262144, "retirement_process_inventory_too_large")
            for name in names:
                require(time.monotonic() < deadline, "retirement_process_inventory_deadline")
                try:
                    path = process_dir / "fd" / name
                    require(not within(os.readlink(path)), "retirement_disk_or_directory_in_use")
                    info = path.stat()
                    require((info.st_dev, info.st_ino) not in inodes, "retirement_disk_or_directory_in_use")
                except FileNotFoundError:
                    continue  # The descriptor closed while inspected.
            if not re.search(r"^State:\s+Z", status, re.MULTILINE):
                require(not within(os.readlink(process_dir / "cwd")), "retirement_directory_in_use")
            for line in bounded_text(process_dir / "maps", 8 * 1024 * 1024).splitlines():
                parts = line.split(maxsplit=5)
                require(len(parts) >= 5, "retirement_process_inventory_unknown")
                if len(parts) == 6:
                    require(not within(parts[5]), "retirement_mapping_in_use")
                major, minor = parts[3].split(":")
                require((os.makedev(int(major, 16), int(minor, 16)), int(parts[4])) not in inodes,
                        "retirement_mapping_in_use")
        except FileNotFoundError:
            require(not process_dir.exists(), "retirement_process_inventory_unknown")
        except (PermissionError, OSError, ValueError):
            raise RuntimeError("retirement_process_inventory_unknown") from None


def retirement_delete(root, record, proof_root):
    entries = retirement_check_tree(root, record, proof_root=proof_root)
    parent_fd = os.open(root.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    root_fd = os.open(root.name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=parent_fd)
    try:
        require(retirement_entry_matches(retirement_entry(os.fstat(parent_fd)), record["parent"])
                and retirement_entry_matches(retirement_entry(os.fstat(root_fd)), record["root_entry"]),
                "retirement_root_binding_changed")
        # Each intermediate directory is opened without following links.
        for name in sorted(entries, key=lambda value: (value == "vm.json", -value.count("/"), value)):
            parts = Path(name).parts
            directory = os.dup(root_fd)
            try:
                prefix = []
                for part in parts[:-1]:
                    prefix.append(part)
                    child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=directory)
                    os.close(directory)
                    directory = child
                    require(retirement_entry_matches(retirement_entry(os.fstat(directory)),
                                                     record["entries"]["/".join(prefix)]), "retirement_entry_changed")
                current = retirement_entry(os.stat(parts[-1], dir_fd=directory, follow_symlinks=False))
                require(retirement_entry_matches(current, entries[name]), "retirement_entry_changed")
                entry_fd = os.open(parts[-1], os.O_PATH | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=directory)
                try:
                    require(retirement_entry_matches(retirement_entry(os.fstat(entry_fd)), entries[name]),
                            "retirement_entry_changed")
                    if current["kind"] == "directory":
                        os.rmdir(parts[-1], dir_fd=directory)
                    else:
                        os.unlink(parts[-1], dir_fd=directory)
                    os.fsync(directory)
                    info = os.fstat(entry_fd)
                    observed = {"dev": info.st_dev, "ino": info.st_ino, "mode": info.st_mode, "nlink": info.st_nlink}
                    capacity_pool.write_deletion_witness(proof_root, record, name, observed)
                finally:
                    os.close(entry_fd)
            finally:
                os.close(directory)
        require(not os.listdir(root_fd), "retirement_inventory_changed")
        require(retirement_entry_matches(retirement_entry(os.stat(root.name, dir_fd=parent_fd, follow_symlinks=False)),
                                         record["root_entry"]), "retirement_root_binding_changed")
        os.rmdir(root.name, dir_fd=parent_fd)
        os.fsync(parent_fd)
        info = os.fstat(root_fd)
        capacity_pool.write_deletion_witness(proof_root, record, ".",
            {"dev": info.st_dev, "ino": info.st_ino, "mode": info.st_mode, "nlink": info.st_nlink})
    finally:
        os.close(root_fd)
        os.close(parent_fd)


def retire(root, expected_vm_id, expected_metadata_sha256, expected_revision, *, auditor=None):
    """Explicit, CAS-bound physical retirement; never called by stop/cleanup."""
    root = capacity_pool.canonical_path(root)
    capacity_pool.valid_uuid(expected_vm_id)
    require(type(expected_metadata_sha256) is str and re.fullmatch(r"[0-9a-f]{64}", expected_metadata_sha256),
            "retirement_metadata_sha256_required")
    require(type(expected_revision) is int and expected_revision >= 1, "retirement_revision_required")
    require(root.name == VM_PREFIX + uuid.UUID(expected_vm_id).hex, "retirement_vm_identity_mismatch")
    def verify(record, data, phase):
        if auditor is None:
            retirement_quiet(record)
        else:
            auditor.verify(record, data, phase)
    with capacity_pool.locked_pool(recover_pending_retirement=True,
                                  expected_retirement_revision=expected_revision,
                                  expected_retirement_binding={"id": expected_vm_id, "root": str(root),
                                                               "metadata_sha256": expected_metadata_sha256},
                                  retirement_recovery_verifier=verify) as pool:
        require(pool.data["revision"] == expected_revision, "capacity_revision_conflict")
        with inventory_lock(root.parent):
            record = pool.data.get("retirements", {}).get(expected_vm_id)
            if record is None:
                entry = pool.data["allocations"].get(expected_vm_id)
                require(entry is not None and entry["identity"]["root"] == str(root), "retirement_vm_not_registered")
                require(entry["adopted"] is False and entry["identity"]["version"] == 2
                        and str(root) not in pool.data["plan"]["existing_vm_roots"], "retirement_adopted_vm_forbidden")
                pool.check_inventory(load)
                value = load(root, allow_preparing=True)
                require(capacity_pool.snapshot(root, value) == entry["identity"], "capacity_vm_binding_changed")
                metadata_sha = retirement_metadata_sha(root)
                require(metadata_sha == expected_metadata_sha256, "retirement_metadata_changed")
                parent, root_entry, entries = retirement_tree(root)
                retirement_id = str(uuid.uuid4())
                record = {"version": 1, "retirement_id": retirement_id, "state": "prepared",
                          "allocation": entry, "metadata_sha256": metadata_sha, "parent": parent,
                          "root_entry": root_entry, "entries": entries,
                          "inventory_sha256": capacity_pool.ledger_digest(entries),
                          "quarantine": str(root.parent / (".retire-" + uuid.UUID(retirement_id).hex)),
                          "deletion_witness_sha256": None}
                require(not os.path.lexists(record["quarantine"]), "retirement_quarantine_exists")
                verify(record, pool.data, "prepare")
                retirement_check_tree(root, record, proof_root=pool.root)
                pool.commit_retirement(expected_vm_id, record)
            require(record["allocation"]["identity"]["root"] == str(root)
                    and record["metadata_sha256"] == expected_metadata_sha256, "retirement_binding_changed")
            require(retirement_entry_matches(retirement_entry(root.parent.lstat()), record["parent"]),
                    "retirement_parent_binding_changed")
            quarantine = Path(record["quarantine"])
            if record["state"] == "retired":
                pool.check_tombstones()
                return {"retired_vm_id": expected_vm_id, "retirement_id": record["retirement_id"],
                        "revision": pool.data["revision"], "held": pool.totals(), "already_retired": True}
            require(not any(other["state"] != "retired" and ident != expected_vm_id
                            for ident, other in pool.data.get("retirements", {}).items()), "capacity_retirement_in_progress")
            verify(record, pool.data, "before_delete")
            if record["state"] == "prepared":
                original, moved = os.path.lexists(root), os.path.lexists(quarantine)
                require(original != moved, "retirement_root_presence_unknown")
                target = root if original else quarantine
                retirement_check_tree(target, record, proof_root=pool.root)
                if original:
                    fd = os.open(root.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
                    try:
                        require(retirement_entry_matches(retirement_entry(os.fstat(fd)), record["parent"]),
                                "retirement_root_binding_changed")
                        os.rename(root.name, quarantine.name, src_dir_fd=fd, dst_dir_fd=fd)
                        os.fsync(fd)
                    finally:
                        os.close(fd)
                record = {**record, "state": "deleting"}
                pool.commit_retirement(expected_vm_id, record)
            require(not os.path.lexists(root), "retirement_original_root_reappeared")
            if os.path.lexists(quarantine):
                retirement_check_tree(quarantine, record, proof_root=pool.root)
                verify(record, pool.data, "before_delete")
                retirement_delete(quarantine, record, pool.root)
            require(not os.path.lexists(root) and not os.path.lexists(quarantine), "retirement_delete_unconfirmed")
            witness_sha = capacity_pool.require_root_deletion_witness(pool.root, record)
            sync_directory(root.parent)
            verify(record, pool.data, "final")
            require(retirement_entry_matches(retirement_entry(root.parent.lstat()), record["parent"]),
                    "retirement_parent_binding_changed")
            record = {**record, "state": "retired", "deletion_witness_sha256": witness_sha}
            capacity_pool.retirement_tombstone(pool.root, record)
            pool.commit_retirement(expected_vm_id, record)
            return {"retired_vm_id": expected_vm_id, "retirement_id": record["retirement_id"],
                    "revision": pool.data["revision"], "held": pool.totals(), "already_retired": False}

def daemon_url(value, path):
    return "https://localhost:" + str(value["docker_port"]) + path

def docker_tls_arguments(root, value):
    return ["docker", "--host", value["docker_endpoint"], "--tlsverify",
            "--tlscacert", str(root / "ca.pem"), "--tlscert", str(root / "client.pem"),
            "--tlskey", str(root / "client-key.pem")]

def qualify(root, provisioner_sources, sandbox, baseline_file=None, parent_client="docker", parent_context=None):
    value = load(root)
    report = {"passed": False, "vm_id": value["id"], "checks": [], "live_fleet_deployed": False,
              "docker_endpoint": value["docker_endpoint"], "ssh_port": value["ssh_port"], "operator_state_id": value["operator_state_id"],
              "memory_mib": 4096, "cpus": 4, "ubuntu_image_sha256": value["ubuntu_image_sha256"],
              "limitations": ["One local nested KVM candidate, not production hostile-tenant qualification or a fleet migration", "Remote backup/update/enrollment and host-aware manager dispatch remain gated", "Certificates expire after 90 days; automatic renewal and disk quotas are not qualified"]}
    def passed(name):
        report["checks"].append(name); print(name + "=ok", flush=True)
    parent = [parent_client] + (["--context", parent_context] if parent_context else [])
    tools = root / "tools"
    if (tools / "docker").is_file():
        require(not (tools / "docker").is_symlink() and (tools / "docker").stat().st_uid == os.getuid() and (tools / "docker").stat().st_mode & 0o022 == 0, "unsafe_private_docker_cli")
        client = json.loads((tools / "client.json").read_text())
        require(digest(tools / "docker") == client["client_sha256"], "private_docker_cli_hash_mismatch")
        os.environ["PATH"] = str(tools) + os.pathsep + os.environ["PATH"]
        report["test_client_sha256"] = client["client_sha256"]
    baseline = json.loads(baseline_file.read_text(encoding="utf-8-sig")) if baseline_file else [json.loads(line) for line in call(parent + ["ps", "--no-trunc", "--format", "{{json .}}"] ).decode().splitlines()]
    baseline_ids = [item["ID"] for item in baseline]
    require(len(set(baseline_ids)) == len(baseline_ids) and all(re.fullmatch(r"[0-9a-f]{12,64}", ident) for ident in baseline_ids), "invalid_container_baseline")
    baseline_images = {ident: json.loads(call(parent + ["inspect", ident]))[0]["Image"] for ident in baseline_ids}
    proof_name = "cowork-vm-proof-" + uuid.UUID(value["id"]).hex
    infra = None
    try:
        start(root)
        require(qmp(root, "query-kvm")["enabled"], "kvm_required")
        passed("unprivileged_qemu_with_hardware_acceleration")
        deadline = time.monotonic() + 1200
        while True:
            require(time.monotonic() < deadline, "guest_boot_deadline")
            parsed = None
            try:
                status = ssh(root, "cloud-init status --format json || true", timeout=10).decode()
                parsed = json.loads(status)
                if parsed.get("status") == "done":
                    break
            except (RuntimeError, subprocess.TimeoutExpired, ValueError):
                if time.monotonic() > deadline:
                    raise RuntimeError("guest_boot_deadline") from None
            if parsed is not None:
                require(parsed.get("status") != "error", "guest_cloud_init_failed")
            time.sleep(5)
        passed("fresh_guest_boot_and_docker_install")
        guest_kernel = ssh(root, "uname -r").decode().strip()
        require("microsoft" not in guest_kernel.lower() and guest_kernel != os.uname().release, "independent_guest_kernel_required")
        report["guest_kernel"] = guest_kernel
        passed("independent_guest_linux_kernel")
        readonly_probe = "import pathlib,errno; p=pathlib.Path(" + repr(str(root / "shared" / "guest-write-refused")) + ");\ntry: p.write_text('unexpected')\nexcept OSError as e: assert e.errno==errno.EROFS; print('shared-state-readonly')\nelse: raise AssertionError('writable-host-share')"
        import shlex
        require(ssh(root, "sudo python3 -c " + shlex.quote(readonly_probe)).decode().strip() == "shared-state-readonly", "guest_can_write_host_state")
        passed("guest_root_cannot_write_operator_state_share")
        context = ssl.create_default_context(cafile=str(root / "ca.pem"))
        context.load_cert_chain(str(root / "client.pem"), str(root / "client-key.pem"))
        info = json.loads(urlopen(daemon_url(value, "/info"), context=context, timeout=10).read())
        require(info["NCPU"] == 4 and info["MemTotal"] <= 4096 * 1024 * 1024, "vm_capacity_mismatch")
        report["daemon_id"] = info["ID"]
        passed("mutual_tls_daemon_identity_and_finite_capacity")
        try:
            urlopen(daemon_url(value, "/_ping"), context=ssl.create_default_context(cafile=str(root / "ca.pem")), timeout=5).read()
        except Exception:
            passed("daemon_refuses_client_without_certificate")
        else:
            raise RuntimeError("unauthenticated_daemon_access")
        image = json.loads(call(parent + ["image", "inspect", sandbox]))[0]["Id"]
        report["sandbox_image_id"] = image
        tls = docker_tls_arguments(root, value)
        exporter = subprocess.Popen(parent + ["save", image], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            imported = subprocess.run(tls + ["load"], stdin=exporter.stdout, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=600)
            exporter.stdout.close(); exporter.wait(timeout=30)
        finally:
            exporter.stdout.close()
            if exporter.poll() is None:
                exporter.terminate()
                try:
                    exporter.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    exporter.kill(); exporter.wait(timeout=5)
            exporter.stderr.close()
        require(imported.returncode == 0 and exporter.returncode == 0, "immutable_image_transfer_failed")
        require(json.loads(call(tls + ["image", "inspect", image]))[0]["Id"] == image, "image_transfer_identity_mismatch")
        passed("immutable_local_sandbox_image_transfer_to_separate_daemon")
        sys.dont_write_bytecode = True
        sys.path.insert(0, str(provisioner_sources))
        import provisioner
        import executor_host
        selection = {"id": value["id"], "operator_state_id": value["operator_state_id"], "trust_domain": value["organisation_id"], "endpoint": value["docker_endpoint"], "daemon_id": info["ID"],
                     "ca_file": str(root / "ca.pem"), "cert_file": str(root / "client.pem"), "key_file": str(root / "client-key.pem")}
        config = {"backend": "compose", "state_directory": str(root / "shared"), "executor_host": selection,
                  "fleet_host_budget": {"cpu_millis": 4000, "memory_mib": 3584, "reserved_cpu_millis": 500, "reserved_memory_mib": 512}}
        infra = provisioner.Infrastructure(config)
        infra.host.verify(); infra.host.bind(); infra.host.verify_shared_state(image)
        passed("real_provisioner_tls_host_binding_and_exact_shared_state_nonce")
        instance = {"id": str(uuid.uuid4()), "organisation_id": str(uuid.uuid4()), "backend": "compose", "policy": {"minimum_isolation": "container"}}
        try:
            infra.host.prepare(instance, root / "shared", "provision")
        except executor_host.HostError as error:
            require(str(error) == "executor_instance_trust_domain_mismatch", "unexpected_org_refusal")
        else:
            raise RuntimeError("foreign_organisation_not_fenced")
        passed("foreign_organisation_cannot_use_executor")
        other = provisioner.Infrastructure({**config, "executor_host": {**selection, "daemon_id": "wrong-daemon-identity"}})
        try:
            other.host.verify()
        except executor_host.HostError as error:
            require(str(error) == "executor_daemon_identity_mismatch", "unexpected_daemon_identity_refusal")
        else:
            raise RuntimeError("daemon_identity_not_fenced")
        passed("real_wrong_daemon_identity_is_refused")
        output = infra.run(["docker", "run", "--name", proof_name, "--rm", "--pull", "never", "--network", "none", "--read-only", "--cap-drop", "ALL",
                            "--security-opt", "no-new-privileges:true", "--cpus", "0.25", "--memory", "128m", "--pids-limit", "64", "--entrypoint", "python3", image,
                            "-c", "import os; print('vm-sandbox-proof'); print(os.uname().release)"], timeout=60).decode().splitlines()
        require(output == ["vm-sandbox-proof", guest_kernel], "sandbox_not_on_guest_kernel")
        passed("bounded_networkless_sandbox_runs_on_guest_kernel")
        report["passed"] = True
    finally:
        if infra is not None:
            try:
                infra.run(["docker", "rm", "-f", proof_name], timeout=10)
            except Exception:
                pass
        stop(root)
        report["vm_stopped"] = process(root) is None
        for ident, image in baseline_images.items():
            observed = json.loads(call(parent + ["inspect", ident]))[0]
            require(observed["State"]["Running"] and observed["Image"] == image, "existing_container_changed")
        report["preexisting_running_containers_preserved"] = len(baseline_images)
        atomic_json(root / "qualification.json", report)
    return report

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["create", "install-cli", "start", "stop", "qualify",
                                          "configure-capacity", "inspect-capacity", "adopt-capacity", "retire"])
    parser.add_argument("--root", type=Path)
    parser.add_argument("--parent", type=Path, default=Path.home() / ".local/share/cowork/executor-vms")
    parser.add_argument("--organisation-id")
    parser.add_argument("--host-plan", type=Path, help="Explicit owner-only Linux host capacity plan")
    parser.add_argument("--expected-capacity-revision", type=int, default=0, help="CAS revision for configuring the canonical capacity pool")
    parser.add_argument("--expected-vm-id", help="Exact registered v2 VM UUID, required for explicit retirement")
    parser.add_argument("--expected-metadata-sha256", help="Unchanged vm.json SHA256, required for explicit retirement")
    parser.add_argument("--retirement-audit-profile", type=Path, help="Optional explicit pinned WSL-root read-only auditor profile")
    parser.add_argument("--docker-port", type=int, default=0, help="New VM's loopback Docker host port: 0 allocates automatically, otherwise 1024..65535")
    parser.add_argument("--provisioner-sources", type=Path)
    parser.add_argument("--sandbox-image", default="cowork-sandbox-core:local-review")
    parser.add_argument("--baseline-file", type=Path, help="Optional existing container IDs, e.g. the original task baseline, to exclude concurrent isolated fixtures")
    parser.add_argument("--docker-cli-version", default="28.4.0")
    parser.add_argument("--parent-docker-client", default="docker.exe" if shutil.which("docker.exe") else "docker")
    parser.add_argument("--parent-docker-context", default="desktop-linux" if shutil.which("docker.exe") else None)
    args = parser.parse_args()
    require(args.action == "create" or args.docker_port == 0, "docker_port_is_immutable_after_creation")
    require(args.action == "retire" or args.retirement_audit_profile is None, "retirement_auditor_requires_explicit_retire")
    if args.action == "configure-capacity":
        require(args.host_plan is not None, "host_plan_required")
        plan = capacity_pool.read_private_json(args.host_plan.absolute())
        with capacity_pool.locked_pool(allow_unconfigured=True) as pool:
            pool.configure(plan, args.expected_capacity_revision)
            print(json.dumps({"pool_root": str(pool.root), "revision": pool.data["revision"],
                              "held": pool.totals(), "unadopted_roots": pool.data["unadopted_roots"]}), flush=True)
    elif args.action == "inspect-capacity":
        with capacity_pool.locked_pool() as pool:
            reason = None
            try:
                bounds = pool.require_bounds()
                pool.check_inventory(load)
            except Exception as error:
                bounds = None
                reason = str(error) if isinstance(error, RuntimeError) else type(error).__name__
            print(json.dumps({"pool_root": str(pool.root), "revision": pool.data["revision"],
                              "plan": pool.data["plan"], "held": pool.totals(), "linux_bounds": bounds,
                              "inventory_ready": reason is None, "waiting_reason": reason,
                              "allocations": pool.data["allocations"],
                              "unadopted_roots": pool.data["unadopted_roots"]}), flush=True)
    elif args.action == "adopt-capacity":
        require(args.root is not None, "vm_root_required")
        root = capacity_pool.canonical_path(args.root.absolute())
        with capacity_pool.locked_pool() as pool:
            with inventory_lock(root.parent):
                inventory_ports(root.parent)
                value = load(root, allow_preparing=True)
                pool.reserve(root, value, adopted=True)
                print(json.dumps({"adopted_vm_root": str(root), "revision": pool.data["revision"],
                                  "held": pool.totals()}), flush=True)
    elif args.action == "retire":
        require(args.root is not None, "vm_root_required")
        require(args.expected_vm_id is not None and args.expected_metadata_sha256 is not None,
                "retirement_exact_identity_required")
        auditor = None
        if args.retirement_audit_profile is not None:
            source = Path(__file__).resolve().with_name("vm_retirement_auditor.py")
            spec = importlib.util.spec_from_file_location("_cowork_retirement_auditor", source)
            module = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)
            profile = capacity_pool.read_private_json(args.retirement_audit_profile.absolute())
            auditor = module.Adapter(profile, vm_id=args.expected_vm_id, root=args.root.absolute(),
                                     metadata_sha256=args.expected_metadata_sha256, revision=args.expected_capacity_revision)
        print(json.dumps(retire(args.root.absolute(), args.expected_vm_id,
                                args.expected_metadata_sha256, args.expected_capacity_revision, auditor=auditor)), flush=True)
    elif args.action == "create":
        require(args.organisation_id, "organisation_id_required")
        root = create(args.parent, args.organisation_id, args.docker_port)
        print("created_vm_root=" + str(root), flush=True)
    else:
        require(args.root is not None, "vm_root_required")
        root = args.root.absolute()
        if args.action == "install-cli":
            install_cli(root, args.docker_cli_version)
        elif args.action == "start":
            start(root)
        elif args.action == "stop":
            stop(root)
        else:
            require(args.provisioner_sources is not None, "provisioner_sources_required")
            report = qualify(root, args.provisioner_sources.absolute(), args.sandbox_image, args.baseline_file, args.parent_docker_client, args.parent_docker_context)
            print(json.dumps(report), flush=True)

if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        safe_message = str(error) if isinstance(error, RuntimeError) or type(error).__name__ in {"ProvisioningError", "HostError", "BudgetError"} else type(error).__name__
        print("executor_vm_failed=" + safe_message, file=sys.stderr)
        raise SystemExit(1)
