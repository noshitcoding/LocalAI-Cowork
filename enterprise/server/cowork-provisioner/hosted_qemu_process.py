"""Owned QEMU child diagnostics for the private qualification harness only."""
import hashlib
import json
import os
from pathlib import Path
import stat
import socket
import subprocess
import threading
import time

STDERR_LIMIT = 65536
QEMU_AS_LIMIT = 8589934592


def require(value, code):
    if not value:
        raise RuntimeError(code)


def private_root(root):
    root = Path(root)
    value = root.lstat()
    require(root.is_absolute() and stat.S_ISDIR(value.st_mode) and not root.is_symlink()
            and value.st_uid == os.geteuid() and stat.S_IMODE(value.st_mode) == 0o700,
            'invalid_owned_qemu_root')
    metadata = (root / 'vm.json').lstat()
    require(stat.S_ISREG(metadata.st_mode) and metadata.st_uid == os.geteuid()
            and stat.S_IMODE(metadata.st_mode) == 0o600, 'invalid_owned_qemu_metadata')
    return root


def classification(prefix, exit_code):
    value = prefix.lower()
    if any(marker in value for marker in (b'cannot allocate memory', b'could not allocate', b'unable to allocate')):
        return 'memory_allocation_failure'
    if b'failed to initialize kvm' in value or b'could not access kvm' in value:
        return 'kvm_initialization_failure'
    if b'address already in use' in value:
        return 'listener_conflict'
    if b'permission denied' in value:
        return 'permission_denied'
    if exit_code is None:
        return 'running'
    if exit_code == 0:
        return 'clean_exit'
    if exit_code == -9:
        return 'terminated_by_sigkill'  # No unsupported OOM inference.
    if exit_code < 0:
        return 'terminated_by_signal'
    return 'qemu_error_exit'


class OwnedQemu:
    """Track only a child started here; never signal an inherited/foreign PID."""
    def __init__(self, root, child, diagnostic_fd):
        self.root, self.child = root, child
        self.lock = threading.Lock()
        self.prefix = bytearray()
        self.total = 0
        self.write_failed = False
        self.thread = threading.Thread(target=self._drain, args=(diagnostic_fd,), daemon=True)
        self.thread.start()

    def _drain(self, diagnostic_fd):
        try:
            while True:
                data = os.read(self.child.stderr.fileno(), 4096)
                if not data:
                    break
                with self.lock:
                    self.total += len(data)
                    retained = data[:max(0, STDERR_LIMIT - len(self.prefix))]
                    self.prefix.extend(retained)
                if retained:
                    try:
                        view = memoryview(retained)
                        while view:
                            view = view[os.write(diagnostic_fd, view):]
                    except OSError:
                        self.write_failed = True
            os.fsync(diagnostic_fd)
        except OSError:
            self.write_failed = True
        finally:
            os.close(diagnostic_fd)
            self.child.stderr.close()

    def evidence(self):
        code = self.child.poll()
        with self.lock:
            content = bytes(self.prefix)
            return {'owned_child_pid': self.child.pid, 'exit_code': code,
                    'stderr_retained_bytes': len(content), 'stderr_observed_bytes': self.total,
                    'stderr_sha256': hashlib.sha256(content).hexdigest(),
                    'stderr_truncated': self.total > STDERR_LIMIT,
                    'diagnostic_write_failed': self.write_failed,
                    'error_classification': classification(content, code),
                    'guest_memory_mib': 4096, 'qemu_address_space_bytes': QEMU_AS_LIMIT}

    def reap(self, timeout=10):
        self.child.wait(timeout=timeout)
        self.thread.join(timeout=2)
        require(not self.thread.is_alive(), 'owned_qemu_stderr_drain_unconfirmed')
        return self.evidence()

    def running(self):
        if self.child.poll() is not None:
            self.reap()
            return False
        return True


class QemuChildren:
    def __init__(self):
        self.children = {}

    def launch(self, argv, *, cwd=None, data=None, timeout=10):
        argv = list(argv)
        require(argv and Path(argv[0]).name == 'qemu-system-x86_64' and argv.count('-pidfile') == 1
                and argv.count('-qmp') == 1 and argv.count('-daemonize') <= 1 and data is None,
                'invalid_owned_qemu_arguments')
        pidfile = Path(argv[argv.index('-pidfile') + 1])
        root = private_root(pidfile.parent)
        require(pidfile.name == 'qemu.pid' and (cwd is None or Path(cwd) == root), 'foreign_qemu_cwd_refused')
        qmp_index = argv.index('-qmp') + 1
        require(argv[qmp_index] in ('unix:' + str(root / 'qmp.sock') + ',server=on,wait=off',
                                   'unix:qmp.sock,server=on,wait=off'), 'foreign_qemu_socket_refused')
        require(root not in self.children and not pidfile.exists() and not (root / 'qmp.sock').exists(),
                'owned_qemu_launch_requires_fresh_files')
        argv[qmp_index] = 'unix:qmp.sock,server=on,wait=off'
        if '-daemonize' in argv:
            argv.remove('-daemonize')
        command = ['prlimit', '--as=' + str(QEMU_AS_LIMIT) + ':' + str(QEMU_AS_LIMIT),
                   '--cpu=5400:5400', '--', *argv]
        descriptor = os.open(root / 'qemu-stderr.private', os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        try:
            child = subprocess.Popen(command, cwd=root, stdin=subprocess.DEVNULL,
                                     stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        except BaseException:
            os.close(descriptor)
            raise
        owned = self.children[root] = OwnedQemu(root, child, descriptor)
        deadline = time.monotonic() + min(30, max(1, timeout))
        while time.monotonic() < deadline:
            if not owned.running():
                raise RuntimeError('owned_qemu_exited:' + owned.evidence()['error_classification'])
            if pidfile.exists() and (root / 'qmp.sock').exists():
                pid_metadata, socket_metadata = pidfile.lstat(), (root / 'qmp.sock').lstat()
                require(stat.S_ISREG(pid_metadata.st_mode) and pid_metadata.st_uid == os.geteuid()
                        and pid_metadata.st_size <= 32 and int(pidfile.read_text().strip()) == child.pid,
                        'owned_qemu_pidfile_mismatch')
                require(stat.S_ISSOCK(socket_metadata.st_mode) and socket_metadata.st_uid == os.geteuid(),
                        'foreign_qemu_socket_refused')
                directory_fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
                try:
                    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as probe:
                        probe.settimeout(min(2, max(.05, deadline - time.monotonic())))
                        probe.connect('/proc/self/fd/' + str(directory_fd) + '/qmp.sock')
                        with probe.makefile('rb') as stream:
                            greeting = stream.readline(65537)
                        require(len(greeting) <= 65536 and greeting.endswith(b'\n')
                                and isinstance(json.loads(greeting).get('QMP'), dict), 'invalid_owned_qmp_greeting')
                    return b''
                except (ConnectionRefusedError, TimeoutError, FileNotFoundError):
                    pass
                finally:
                    os.close(directory_fd)
            time.sleep(.05)
        raise RuntimeError('owned_qemu_qmp_readiness_timeout')

    def evidence(self, root):
        owned = self.children.get(Path(root))
        return owned.evidence() if owned else None

    def require_running(self, root):
        owned = self.children.get(Path(root))
        require(owned is not None, 'owned_qemu_child_missing')
        if not owned.running():
            raise RuntimeError('owned_qemu_exited:' + owned.evidence()['error_classification'])
        return owned.child.pid

    def reap_after_stop(self, root):
        owned = self.children.get(Path(root))
        return owned.reap() if owned else None
