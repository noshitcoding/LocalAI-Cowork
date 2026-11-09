"""Native Linux IO boundary for the *offline* historical migration CLI.

Read leases reject existing writers and interrupt on new write opens/truncates.
Pinned dirfds and inotify reject namespace changes, including changes restored
before the next stat. No advisory flock, source SQLite open, filesystem freeze,
mount change, sysctl change, or writer kill is used. This is a single-threaded
CLI boundary; signal ownership is deliberately not offered to embedded workers.
"""
from __future__ import annotations
import ctypes
import fcntl
import hashlib
import os
from pathlib import Path
import signal
import stat
import struct
import sys
import threading
import time

PENDING = '.m9-linux-publication-pending'
FREEZE = 'linux-read-leases-dirfd-inotify-through-publication/2'
_namespaces = []
_files = []
_signals = None
_refused = None
# Ext2/3/4, XFS, Btrfs, tmpfs. Lease acquisition must additionally succeed.
_LOCAL = {0xEF53, 0x58465342, 0x9123683E, 0x01021994}
_SELF = 0x00000400 | 0x00000800 | 0x00002000 | 0x00008000
_CHANGE = 0x00000002 | 0x00000004 | 0x00000040 | 0x00000080 | 0x00000100 | 0x00000200
_OVERFLOW = 0x00004000
_DIR = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC


def configure(refused):
    global _refused
    _refused = refused


def fail(message):
    raise _refused(message)


def native():
    if sys.platform != 'linux' or ctypes.sizeof(ctypes.c_void_p) != 8:
        fail('Native Linux 64-bit migration boundary required')
    library = ctypes.CDLL(None, use_errno=True)
    library.inotify_init1.argtypes = [ctypes.c_int]
    library.inotify_init1.restype = ctypes.c_int
    library.inotify_add_watch.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_uint32]
    library.inotify_add_watch.restype = ctypes.c_int
    library.fstatfs.argtypes = [ctypes.c_int, ctypes.c_void_p]
    library.fstatfs.restype = ctypes.c_int
    return library


def identity(st):
    return st.st_dev, st.st_ino

def outside_pending(directory):
    return directory.parent / ('.m9-linux-pending-' + hashlib.sha256(os.fsencode(directory.name)).hexdigest())


def local(fd):
    # Read just the documented leading long f_type; leave room for native statfs.
    buffer = ctypes.create_string_buffer(512)
    if native().fstatfs(fd, buffer):
        fail('Cannot identify native filesystem')
    magic = ctypes.c_long.from_buffer(buffer).value & 0xffffffff
    if magic not in _LOCAL:
        fail('Local lease-capable Linux filesystem required; network/FUSE/DrvFs refused')


def path(value, existing=True):
    value = Path(value)
    if not value.is_absolute() or '..' in value.parts or len(os.fsencode(value)) > 4096 or len(value.parts) > 128:
        fail('Bounded absolute Linux path without traversal required')
    value = Path(os.path.abspath(value))
    with Namespaces([value.parent]) as namespace:
        if existing:
            fd = namespace.open(value)
            try:
                st = os.fstat(fd)
                if not stat.S_ISREG(st.st_mode) or st.st_nlink != 1 or st.st_uid != os.geteuid():
                    fail('One-link operator-owned ordinary source file required')
                local(fd)
            finally:
                os.close(fd)
        elif os.path.lexists(value):
            fail('Output must be a new exclusive directory')
        if namespace.pending(value.parent):
            fail('Unconfirmed Linux publication cannot be a migration input')
        namespace.check()
    return value


class Namespaces:
    """Hold real directory identities; observe changes to exact selected names."""
    def __init__(self, directories):
        self.requested = sorted(set(Path(p) for p in directories), key=str)
        self.fds = {}
        self.identities = {}
        self.watches = {}
        self.events = -1
        self.broken = False
        self.closed = False
        self.expected = {}

    def __enter__(self):
        try:
            library = native()
            self.events = library.inotify_init1(os.O_CLOEXEC | os.O_NONBLOCK)
            if self.events < 0:
                fail('Cannot establish Linux namespace observations')
            self._add(Path('/'), None)
            for directory in self.requested:
                if not directory.is_absolute() or '..' in directory.parts or len(directory.parts) > 128:
                    fail('Absolute directory namespace required')
                for item in [*reversed(directory.parents), directory]:
                    if item not in self.fds:
                        self._add(item, item.parent)
                    if item != Path('/'):
                        self.watch(item)
            _namespaces.append(self)
            self.check()
            return self
        except BaseException:
            self.close()
            raise

    def _add(self, directory, parent):
        fd = os.open(str(directory) if parent is None else directory.name,
                     _DIR, dir_fd=None if parent is None else self.fds[parent])
        try:
            local(fd)
            st = os.fstat(fd)
            if not stat.S_ISDIR(st.st_mode):
                fail('Ordinary directory namespace required')
            wd = native().inotify_add_watch(self.events, os.fsencode('/proc/self/fd/'+str(fd)),
                                           _SELF | _CHANGE | 0x01000000)
            if wd < 0 or wd in self.watches:
                fail('Cannot bind distinct original directory watch')
            self.fds[directory] = fd
            self.identities[directory] = identity(st)
            self.watches[wd] = (directory, set())
        except BaseException:
            os.close(fd)
            raise

    def watch(self, entry):
        entry = Path(entry)
        for directory, names in self.watches.values():
            if directory == entry.parent:
                names.add(os.fsencode(entry.name))
                return
        fail('Entry outside pinned namespace')

    def expect(self, entry, masks):
        self.watch(entry)
        self.expected[os.fsencode(str(entry))] = list(masks)

    def observed(self, entry):
        self.check()
        if os.fsencode(str(entry)) in self.expected:
            self.broken = True
            fail('Original publication syscall event was not observed')

    def pending(self, directory, allowed=None):
        for p in [directory, *directory.parents]:
            if allowed is not None and p == allowed[0]:
                if identity(os.fstat(self.fds[p])) != allowed[1]:
                    fail('Owned publication directory changed')
                continue
            if self.exists(p/PENDING):
                return True
            if p != Path('/') and self.exists(outside_pending(p)):
                return True
        return False

    def watch_pending(self):
        for p in self.fds:
            self.watch(p/PENDING)
            if p != Path('/'):
                self.watch(outside_pending(p))

    def open(self, entry):
        entry = Path(entry)
        return os.open(entry.name, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC | os.O_NONBLOCK,
                       dir_fd=self.fds[entry.parent])

    def exists(self, entry):
        try:
            os.stat(Path(entry).name, dir_fd=self.fds[Path(entry).parent], follow_symlinks=False)
            return True
        except FileNotFoundError:
            return False

    def check(self):
        if self.closed or self.broken:
            fail('Linux namespace fence lost')
        count = 0
        try:
            while True:
                try:
                    raw = os.read(self.events, 64 * 1024)
                except BlockingIOError:
                    break
                if not raw:
                    fail('Linux namespace observation ended')
                offset = 0
                while offset < len(raw):
                    if offset + 16 > len(raw):
                        fail('Incomplete native namespace event')
                    wd, mask, _, length = struct.unpack_from('iIII', raw, offset)
                    end = offset + 16 + length
                    if end > len(raw):
                        fail('Incomplete native namespace name')
                    name = raw[offset+16:end].split(b'\0', 1)[0]
                    count += 1
                    if count > 8192 or mask & _OVERFLOW or wd not in self.watches:
                        fail('Linux namespace observation overflow/loss')
                    directory, names = self.watches[wd]
                    key = os.fsencode(str(directory / os.fsdecode(name)))
                    expected = self.expected.get(key)
                    if expected and mask == expected[0]:
                        expected.pop(0)
                        if not expected:
                            del self.expected[key]
                    elif mask & _SELF or mask & _CHANGE and name in names:
                        fail('Original Linux namespace changed')
                    offset = end
            for directory, fd in self.fds.items():
                held = os.fstat(fd)
                current = os.stat('/') if directory == Path('/') else os.stat(
                    directory.name, dir_fd=self.fds[directory.parent], follow_symlinks=False)
                if not stat.S_ISDIR(current.st_mode) or identity(held) != self.identities[directory] or identity(current) != identity(held):
                    fail('Original Linux directory identity changed')
        except BaseException:
            self.broken = True
            raise

    def close(self):
        if self in _namespaces:
            _namespaces.remove(self)
        if self.events >= 0:
            os.close(self.events)
            self.events = -1
        for fd in reversed(list(self.fds.values())):
            os.close(fd)
        self.fds.clear()
        self.closed = True

    def __exit__(self, *_):
        self.close()


class LeaseSignals:
    """Own blocked lease signals in the exclusive CLI, with no Python handler."""
    def __init__(self):
        if threading.current_thread() is not threading.main_thread() or len(os.listdir('/proc/self/task')) != 1:
            fail('Linux historical migration must run in its own single-threaded CLI')
        self.number = signal.SIGRTMAX
        self.selected = {signal.SIGIO, self.number}
        if any(signal.getsignal(n) != signal.SIG_DFL for n in self.selected):
            fail('Migration lease signals are already owned')
        self.mask = signal.pthread_sigmask(signal.SIG_BLOCK, self.selected)
        self.fd = -1
        self.users = 0
        self.broken = False
        try:
            if self.mask & self.selected or signal.sigpending() & self.selected:
                fail('Migration lease signals already blocked or pending')
            library = native()
            library.sigemptyset.argtypes = [ctypes.c_void_p]
            library.sigaddset.argtypes = [ctypes.c_void_p, ctypes.c_int]
            library.signalfd.argtypes = [ctypes.c_int, ctypes.c_void_p, ctypes.c_int]
            library.signalfd.restype = ctypes.c_int
            mask = ctypes.create_string_buffer(128)
            if library.sigemptyset(mask) or any(library.sigaddset(mask, n) for n in self.selected):
                fail('Cannot construct native lease signal set')
            self.fd = library.signalfd(-1, mask, os.O_CLOEXEC | os.O_NONBLOCK)
            if self.fd < 0:
                fail('Cannot establish native lease signal descriptor')
        except BaseException:
            self.close()
            raise

    def drain(self):
        seen = False
        for _ in range(64):
            try:
                raw = os.read(self.fd, 128 * 32)
            except BlockingIOError:
                return seen
            if not raw or len(raw) % 128:
                fail('Lease signal observation lost')
            seen = True
        fail('Lease signal observation overflow')

    def check(self):
        if self.drain():
            self.broken = True
        if self.broken:
            fail('Original Linux source has a new writer/lease-break request')

    def close(self):
        try:
            if self.fd >= 0:
                try:
                    self.drain()
                finally:
                    os.close(self.fd)
                    self.fd = -1
        finally:
            signal.pthread_sigmask(signal.SIG_SETMASK, self.mask)


class Files:
    def __init__(self, paths, maximum, owned_publication=None):
        self.paths = sorted(set(paths), key=str)
        self.maximum = maximum
        self.streams = {}
        self.before = {}
        self.stamps = {}
        self.namespace = None
        self.signals = None
        # Only the trusted publisher supplies its freshly created and natively
        # proven directory. Ordinary source/intake fences never bypass a marker.
        self.owned_publication = owned_publication

    def __enter__(self):
        global _signals
        try:
            if _signals is None:
                _signals = LeaseSignals()
            self.signals = _signals
            self.signals.users += 1
            self.namespace = Namespaces([p.parent for p in self.paths]).__enter__()
            self.namespace.watch_pending()
            for p in self.paths:
                # Observe both existing inputs and absent SQLite sidecars/commit marker.
                for name in [p, *[Path(str(p)+s) for s in ('-wal', '-shm', '-journal')], p.parent/PENDING]:
                    self.namespace.watch(name)
            total = 0
            for p in self.paths:
                fd = self.namespace.open(p)
                try:
                    st = os.fstat(fd)
                    if not stat.S_ISREG(st.st_mode) or st.st_nlink != 1 or st.st_uid != os.geteuid():
                        fail('One-link operator-owned ordinary file required')
                    if not 0 <= st.st_dev < 1 << 32 or not 0 <= st.st_ino < 1 << 64:
                        fail('Native source identity exceeds historical receipt format')
                    total += st.st_size
                    if total > self.maximum:
                        fail('Complete input byte limit exceeded')
                    local(fd)
                    # Both RT notifications and SIGIO queue-exhaustion fallback are blocked.
                    fcntl.fcntl(fd, fcntl.F_SETSIG, self.signals.number)
                    try:
                        fcntl.fcntl(fd, fcntl.F_SETLEASE, fcntl.F_RDLCK)
                    except OSError as error:
                        raise _refused('Cannot acquire native read lease; writer/mmap or unsupported filesystem') from error
                    # F_SETLEASE initially uses TGID; this pins subsequent signals to this thread.
                    fcntl.fcntl(fd, 15, struct.pack('ii', 0, threading.get_native_id()))  # F_SETOWN_EX / F_OWNER_TID
                    self.streams[p] = os.fdopen(fd, 'rb')
                    fd = -1
                    self.before[p] = {'bytes': st.st_size, 'volume': st.st_dev, 'file_id': str(st.st_ino)}
                    self.stamps[p] = (st.st_dev, st.st_ino, st.st_size, st.st_mtime_ns, st.st_ctime_ns)
                finally:
                    if fd >= 0:
                        os.close(fd)
            _files.append(self)
            self.check()
            return self
        except BaseException:
            self.close()
            raise

    def check(self):
        self.signals.check()
        self.namespace.check()
        for p, stream in self.streams.items():
            fd = stream.fileno()
            if fcntl.fcntl(fd, fcntl.F_GETLEASE) != fcntl.F_RDLCK:
                fail('Original Linux read lease is breaking or lost')
            held = os.fstat(fd)
            current = os.stat(p.name, dir_fd=self.namespace.fds[p.parent], follow_symlinks=False)
            stamp = lambda s: (s.st_dev, s.st_ino, s.st_size, s.st_mtime_ns, s.st_ctime_ns)
            if held.st_nlink != 1 or not stat.S_ISREG(current.st_mode) or stamp(held) != self.stamps[p] or stamp(current) != self.stamps[p]:
                fail('Original Linux file identity/content metadata changed')
            if self.namespace.pending(p.parent, self.owned_publication):
                fail('Unconfirmed Linux publication cannot be read')

    def close(self):
        global _signals
        if self in _files:
            _files.remove(self)
        try:
            for stream in self.streams.values():
                # Closing the original description releases exactly its own lease.
                stream.close()
            self.streams.clear()
            if self.namespace is not None:
                self.namespace.close()
        finally:
            if self.signals is not None:
                self.signals.users -= 1
                if self.signals.users == 0:
                    try:
                        self.signals.close()
                    finally:
                        _signals = None
                self.signals = None


def check_active():
    for fence in list(_files):
        fence.check()
    for namespace in list(_namespaces):
        namespace.check()


def publication_guard(deadline):
    # Never call Budget.check here: it calls check_active itself. The original
    # operation deadline is retained, rather than allocating another 120 seconds.
    if deadline is not None and time.monotonic() > deadline:
        fail('Original migration publication deadline exceeded')
    check_active()

def _tree(fd, chmod=None, remove=False, count=None, deadline=None):
    count = [0] if count is None else count
    for name in os.listdir(fd):
        if deadline is not None: publication_guard(deadline)
        count[0] += 1
        if count[0] > 20000:
            fail('Private stage tree exceeds bound')
        before = os.stat(name, dir_fd=fd, follow_symlinks=False)
        if before.st_uid != os.geteuid() or stat.S_ISLNK(before.st_mode):
            fail('Private stage contains unowned/link entry; retained')
        child = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC | os.O_NONBLOCK |
                        (os.O_DIRECTORY if stat.S_ISDIR(before.st_mode) else 0), dir_fd=fd)
        try:
            if identity(os.fstat(child)) != identity(before):
                fail('Private stage entry identity changed; retained')
            if stat.S_ISDIR(before.st_mode):
                _tree(child, chmod, remove, count, deadline)
            elif stat.S_ISREG(before.st_mode) and before.st_nlink == 1:
                if chmod is not None:
                    os.fchmod(child, chmod)
                if not remove:
                    os.fsync(child)
            else:
                fail('Private stage contains nonordinary/hardlinked entry; retained')
            if identity(os.stat(name, dir_fd=fd, follow_symlinks=False)) != identity(before):
                fail('Private stage entry changed before finalization; retained')
            if remove:
                (os.rmdir if stat.S_ISDIR(before.st_mode) else os.unlink)(name, dir_fd=fd)
        finally:
            os.close(child)
    os.fsync(fd)


def _members(fd, base, result=None, deadline=None):
    result = [] if result is None else result
    for name in os.listdir(fd):
        if deadline is not None: publication_guard(deadline)
        if len(result) >= 20000:
            fail('Private publication source tree exceeds bound')
        before = os.stat(name, dir_fd=fd, follow_symlinks=False)
        if before.st_uid != os.geteuid() or stat.S_ISLNK(before.st_mode):
            fail('Unowned/link publication source')
        if stat.S_ISDIR(before.st_mode):
            child = os.open(name, _DIR, dir_fd=fd)
            try:
                if identity(os.fstat(child)) != identity(before):
                    fail('Publication source directory changed')
                _members(child, base/name, result, deadline)
            finally:
                os.close(child)
        elif stat.S_ISREG(before.st_mode) and before.st_nlink == 1:
            result.append(base/name)
        else:
            fail('Nonordinary publication source')
    return result


def _copy_tree(source, target, base, held, namespace, output_base, copied, deadline, count=None):
    count = [0] if count is None else count
    for name in os.listdir(source):
        publication_guard(deadline)
        count[0] += 1
        if count[0] > 20000:
            fail('Private publication tree exceeds bound')
        before = os.stat(name, dir_fd=source, follow_symlinks=False)
        if stat.S_ISDIR(before.st_mode):
            child = os.open(name, _DIR, dir_fd=source)
            try:
                if identity(os.fstat(child)) != identity(before):
                    fail('Publication source directory changed')
                namespace.expect(output_base/name,[0x100|0x40000000])
                os.mkdir(name, mode=0o700, dir_fd=target)
                destination = os.open(name, _DIR, dir_fd=target)
                try:
                    namespace.observed(output_base/name)
                    namespace._add(output_base/name,output_base)
                    if identity(os.fstat(namespace.fds[output_base/name])) != identity(os.fstat(destination)):
                        fail('New nested destination directory changed')
                    namespace.check()
                    _copy_tree(child, destination, base/name, held, namespace, output_base/name, copied, deadline, count)
                finally:
                    os.close(destination)
            finally:
                os.close(child)
        elif base/name in held.streams and identity(os.fstat(held.streams[base/name].fileno())) == identity(before):
            original = held.streams[base/name]; original.seek(0)
            fd = os.open(name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC,
                         0o600, dir_fd=target)
            with os.fdopen(fd, 'wb') as stream:
                proof = hashlib.sha256(); size = 0
                while chunk := original.read(1024*1024):
                    publication_guard(deadline)
                    stream.write(chunk)
                    proof.update(chunk); size += len(chunk)
                stream.flush(); os.fsync(stream.fileno())
                os.fchmod(stream.fileno(),0o444)
                physical = identity(os.fstat(stream.fileno()))
                if identity(os.stat(name,dir_fd=target,follow_symlinks=False)) != physical:
                    fail('Own destination file replaced')
                copied[output_base/name] = (physical,size,proof.hexdigest())
        else:
            fail('Publication source differs from originally leased member')
    os.fsync(target)


def publish(stage, output, expected, deadline=None, final_check=None):
    """Exclusive create+FD-copy: never rename a replaceable source name.

An independent parent marker blocks every descendant intake until commit, even
if another process replaces the stage or output name. Unknown outputs/markers
are retained; they are never adopted, recursively deleted or silently confirmed.
"""
    deadline = time.monotonic()+120 if deadline is None else deadline
    publication_guard(deadline)
    with Namespaces([stage.parent, output.parent]) as namespace:
        namespace.watch(stage); namespace.watch(output)
        source = os.open(stage.name, _DIR, dir_fd=namespace.fds[stage.parent])
        held = destination_hold = None
        destination = marker = -1
        try:
            if stage.parent != output.parent or identity(os.fstat(source)) != expected:
                fail('Original sibling stage required')
            paths = _members(source, stage, deadline=deadline)
            held = Files(paths, 16*1024*1024*1024).__enter__()
            if paths and identity(os.fstat(held.namespace.fds[stage])) != expected:
                fail('Publication source path differs from original held stage')
            parent = namespace.fds[output.parent]
            independent = outside_pending(output)
            namespace.expect(independent,[0x100,0x2])  # our exclusive create and one bounded write
            marker = os.open(independent.name,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW|os.O_CLOEXEC,
                             0o600,dir_fd=parent)
            marker_id = identity(os.fstat(marker))
            os.write(marker,b'Unconfirmed historical-only publication; every descendant intake is blocked.\n')
            os.fsync(marker); os.fsync(parent)
            namespace.observed(independent)
            publication_guard(deadline)
            namespace.expect(output,[0x100|0x40000000])  # exact native mkdir event
            os.mkdir(output.name,mode=0o700,dir_fd=parent)
            destination = os.open(output.name,_DIR,dir_fd=parent)
            namespace.observed(output)  # reject replacement before the first destination write
            destination_id = identity(os.fstat(destination))
            if identity(os.stat(output.name,dir_fd=parent,follow_symlinks=False)) != destination_id:
                fail('New publication directory identity changed')
            publication_guard(deadline)
            namespace._add(output,output.parent)
            if identity(os.fstat(namespace.fds[output])) != destination_id:
                fail('Original output namespace changed')
            namespace.check()
            copied = {}
            _copy_tree(source,destination,stage,held,namespace,output,copied,deadline)
            publication_guard(deadline)
            held.close(); held = None
            # Retain output read leases through commit too; only this proven
            # native publication can bypass its own independent pending marker.
            destination_hold = Files(_members(destination,output,deadline=deadline),16*1024*1024*1024,
                                     (output,destination_id)).__enter__()
            if set(destination_hold.streams) != set(copied):
                fail('Own destination membership changed')
            for p,stream in destination_hold.streams.items():
                physical,size,expected_hash = copied[p]
                if identity(os.fstat(stream.fileno())) != physical:
                    fail('Own destination file identity changed')
                proof = hashlib.sha256(); actual = 0; stream.seek(0)
                while chunk := stream.read(1024*1024):
                    publication_guard(deadline); proof.update(chunk); actual += len(chunk)
                if actual != size or proof.hexdigest() != expected_hash:
                    fail('Own destination bytes changed before commit')
            publication_guard(deadline)
            namespace.expect(stage,[0x200|0x40000000])
            cleanup(stage,expected,deadline)
            namespace.observed(stage)
            if identity(os.stat(independent.name,dir_fd=parent,follow_symlinks=False)) != marker_id:
                fail('Independent publication marker changed; output retained')
            if identity(os.stat(output.name,dir_fd=parent,follow_symlinks=False)) != destination_id:
                fail('Publication directory changed; output retained')
            publication_guard(deadline)
            if final_check is not None: final_check()
            # Observe this precise own commit event; no foreign marker is removed.
            namespace.expect(independent,[0x200])
            os.unlink(independent.name,dir_fd=parent)
            os.fsync(parent)
            namespace.observed(independent)
        finally:
            if destination_hold is not None: destination_hold.close()
            if held is not None: held.close()
            if marker >= 0: os.close(marker)
            if destination >= 0: os.close(destination)
            os.close(source)


def cleanup(stage, expected, deadline=None):
    if not os.path.lexists(stage):
        return
    with Namespaces([stage.parent]) as namespace:
        namespace.watch(stage)
        directory = os.open(stage.name, _DIR, dir_fd=namespace.fds[stage.parent])
        try:
            if identity(os.fstat(directory)) != expected:
                fail('Refuse cleanup of unowned Linux stage')
            # Preflight the entire bounded tree before the first removal. Failures
            # preserve unknown entries; all IO is relative to original owned dirfds.
            _tree(directory,deadline=deadline)
            namespace.check()
            _tree(directory, chmod=0o600, remove=True,deadline=deadline)
            namespace.check()
            if identity(os.stat(stage.name, dir_fd=namespace.fds[stage.parent], follow_symlinks=False)) != expected:
                fail('Refuse cleanup after stage replacement')
            os.rmdir(stage.name, dir_fd=namespace.fds[stage.parent])
            os.fsync(namespace.fds[stage.parent])
        finally:
            os.close(directory)
