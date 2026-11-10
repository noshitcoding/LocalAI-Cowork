"""Own 512-MiB ext4 process-abort proof; never a power-loss/profile proof.

Only newly spawned exact test workers are stopped. Parent fsync boundaries use
PTRACE_TRACEME on the worker's own thread, not a production fault switch.
"""
import argparse
import ctypes
import fcntl
import hashlib
import json
import os
from pathlib import Path
import platform
import select
import signal
import stat
import struct
import subprocess
import time
import uuid

CASE = "staging::archive::installation::publication::journal::crash::owned_ext4_publication_crash_worker"
FLOOR_CASE = "release_floor::crash::owned_ext4_release_floor_crash_worker"
FLOOR_MODES = ("initialized", "advanced", "before-write", "after-write", "after-sync", "complete", "torn-write", "enospc")
MODES = ("prepared", "intent", "renamed", "first-parent-sync", "second-parent-sync", "complete", "intent-io-error", "enospc")


def digest(path):
    h = hashlib.sha256()
    with path.open("rb") as source:
        while block := source.read(65536): h.update(block)
    return h.hexdigest()


def inventory(root):
    rows = {}
    pending = [root]
    while pending:
        path = pending.pop()
        m = path.lstat()
        if len(rows) >= 1024 or m.st_uid != 0: raise RuntimeError("Owned inventory bounds/owner changed")
        row = {"device": m.st_dev, "inode": m.st_ino, "mode": stat.S_IMODE(m.st_mode), "bytes": m.st_size}
        if stat.S_ISDIR(m.st_mode): pending.extend(path.iterdir())
        elif stat.S_ISREG(m.st_mode):
            if m.st_nlink != 1 or m.st_size > 512 * 1024**2: raise RuntimeError("Owned regular-file bound changed")
            row["sha256"] = digest(path)
        elif stat.S_ISLNK(m.st_mode): row["link"] = os.readlink(path)
        else: raise RuntimeError("Unexpected owned object")
        rows[str(path.relative_to(root))] = row
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("test-binary", "package-binary", "output"): parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--test-sha256", required=True)
    parser.add_argument("--package-sha256", required=True)
    parser.add_argument("--release-floor", action="store_true", help="Own separate eight-case restrictive policy proof, never activation")
    args = parser.parse_args()
    case_name = FLOOR_CASE if args.release_floor else CASE
    modes = FLOOR_MODES if args.release_floor else MODES
    log_prefix = "release-floor-crash-" if args.release_floor else "journal-crash-"
    report_name = "release-floor-ext4-report.json" if args.release_floor else "publication-journal-ext4-report.json"
    if os.geteuid() != 0 or platform.machine() != "x86_64": raise RuntimeError("Owned UID-0 x86_64 fixture required")
    repo = Path(__file__).resolve().parents[2]
    output = args.output.resolve(strict=True)
    if not output.is_relative_to(repo / ".local/v2-rebuild-validation"): raise ValueError("Own qualification output required")
    binaries = {}
    for label, path, expected in (("tests", args.test_binary, args.test_sha256), ("package", args.package_binary, args.package_sha256)):
        path = path.resolve(strict=True)
        if path.stat().st_size > 128 * 1024**2 or digest(path) != expected: raise ValueError("Exact bounded original ELF required")
        data = path.read_bytes()
        if not data.startswith(b"\x7fELF"): raise ValueError("Actual Linux ELF required")
        binaries[label] = data
    native = ctypes.CDLL(None, use_errno=True)
    native.mount.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_char_p, ctypes.c_ulong, ctypes.c_void_p]
    native.umount2.argtypes = [ctypes.c_char_p, ctypes.c_int]
    native.ptrace.restype = ctypes.c_long
    native.ptrace.argtypes = [ctypes.c_uint, ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p]
    before_ns = os.readlink("/proc/thread-self/ns/mnt")
    if native.unshare(0x00020000) != 0 or native.mount(None, b"/", None, 16384 | 262144, None) != 0:
        raise OSError(ctypes.get_errno(), "Private namespace unavailable")
    if os.readlink("/proc/thread-self/ns/mnt") == before_ns: raise RuntimeError("Mount namespace did not change")
    root = Path("/var/tmp") / (("cowork-release-floor-" if args.release_floor else "cowork-publication-journal-") + uuid.uuid4().hex)
    root.mkdir(mode=0o700)
    root_id = root.stat()
    image = root / "fixture.ext4"
    mountpoint = root / "store"
    mountpoint.mkdir(mode=0o700)
    image_fd = None
    loop_fd = None
    loop_name = None
    mounted = False
    active = None
    trace_tid = None
    tools = {}
    report = {"scope": "Original-journal process loss on own ext4, successful syscall boundary stops and normal remount; no power-loss or installed runtime qualification",
              "result": "running", "outer_reserved_bytes": 512 * 1024**2, "inode_limit": 4096,
              "private_mount_namespace": True, "test_binary_sha256": args.test_sha256,
              "package_binary_sha256": args.package_sha256, "cases": [], "normal_unmount_confirmed": False,
              "loop_detached_confirmed": False, "fixture_removed": False,
              "installed": False, "activated": False, "execution_authority": False, "power_loss_qualified": False}

    if args.release_floor:
        report["scope"] = "Restrictive release-floor policy after eight actual own ext4 process/I/O faults, positional concurrent-reader fix and normal remount; no power-loss, installed admission or activation proof"

    def call(argv):
        result = subprocess.run(argv, capture_output=True, timeout=30, check=False)
        if result.returncode: raise RuntimeError("Owned fixture command failed: " + str(argv[0]))
        return result.stdout.decode().strip()

    def loop_info():
        raw = bytearray(232)
        try: fcntl.ioctl(loop_fd, 0x4C05, raw, True)
        except OSError as error:
            if error.errno == 6: return None
            raise
        values = struct.unpack("=QQQQQIIII64s64s32sQQ", raw)
        return values[0], values[1]

    def require_loop():
        m = os.fstat(image_fd)
        if loop_info() != (m.st_dev, m.st_ino): raise RuntimeError("Original loop backing changed")

    def mount():
        nonlocal mounted
        require_loop()
        call(["/usr/bin/mount", "-t", "ext4", "-o", "nodev,nosuid,noexec", "--", loop_name, str(mountpoint)])
        mountpoint.chmod(0o700)
        mounted = True
        if mountpoint.stat().st_dev == root_id.st_dev: raise RuntimeError("Own ext4 was not mounted")

    def unmount():
        nonlocal mounted
        call(["/usr/bin/umount", "--", str(mountpoint)])
        mounted = False
        if any(str(mountpoint) == line.split()[4] for line in Path("/proc/self/mountinfo").read_text().splitlines()):
            raise RuntimeError("Own ext4 still mounted")
        require_loop()
        report["normal_unmount_confirmed"] = True

    def observe(case, expected):
        before = inventory(case)
        if args.release_floor:
            command = [str(tools["package"][0]), "--release-floor-status", str(case), "floor", str(case / "trusted.pub"), "fixture-only", "linux_x86_64"]
        else:
            command = [str(tools["package"][0]), "--publication-status", str(case / "attempt-journal"), str(case / "trusted.pub"), "fixture-only", "linux_x86_64"]
        result = subprocess.run(command, capture_output=True, timeout=15, check=False)
        if expected is None:
            if result.returncode == 0: raise RuntimeError("Incomplete policy/journal unexpectedly admitted")
            view = {"rejected_unconfirmed_floor" if args.release_floor else "rejected_incomplete_journal": True}
        elif args.release_floor:
            if result.returncode != 0: raise RuntimeError("Restrictive floor observer failed: " + result.stderr.decode(errors="replace"))
            view = json.loads(result.stdout)
            if view["release_sequence"] != expected or not view["restrictive_policy_only"]: raise RuntimeError("Wrong retained restrictive floor")
            if any(view[k] for k in ("installed", "activated", "qualified", "execution_authority", "recovered_native_authority")):
                raise RuntimeError("Floor reader invented runtime authority")
        else:
            if result.returncode != 0: raise RuntimeError("Status-only observer failed: " + result.stderr.decode(errors="replace"))
            view = json.loads(result.stdout)
            if view["last_valid_phase"] != expected or not view["status_only"]: raise RuntimeError("Wrong retained diagnostic phase")
            if not view["physical_publication_unconfirmed_after_process_loss"]: raise RuntimeError("History invented physical custody")
            if any(view[k] for k in ("can_dispatch", "can_activate", "can_reclaim", "execution_authority", "native_custody_reconstructible")):
                raise RuntimeError("History reconstructed native authority")
        if inventory(case) != before: raise RuntimeError("Observer changed retained files")
        return view

    class Registers(ctypes.Structure):
        _fields_ = [(name, ctypes.c_ulonglong) for name in ("r15", "r14", "r13", "r12", "rbp", "rbx", "r11", "r10", "r9", "r8", "rax", "rcx", "rdx", "rsi", "rdi", "orig_rax", "rip", "cs", "eflags", "rsp", "ss", "fs_base", "gs_base", "ds", "es", "fs", "gs")]

    def ptrace(request, tid, data=None):
        if native.ptrace(request, tid, None, data) < 0: raise OSError(ctypes.get_errno(), "Owned worker trace failed")

    def wait_trace(tid):
        end = time.monotonic() + 15
        while time.monotonic() < end:
            got, status = os.waitpid(tid, 0x40000000 | os.WNOHANG | os.WUNTRACED)
            if got: return status
            time.sleep(.001)
        raise RuntimeError("Owned worker trace timeout")

    def stop_after_sync(tid, case, wanted, floor_boundary=None):
        targets = [str(case / "floor/floor.a")] if floor_boundary else [str(case), str(case / "versions")]
        syscall = 1 if floor_boundary == "after-write" else 74
        return_value = 256 * 1024 if floor_boundary == "after-write" else 0
        status = wait_trace(tid)
        if not os.WIFSTOPPED(status): raise RuntimeError("Owned trace did not stop")
        ptrace(0x4200, tid, ctypes.c_void_p(1))  # PTRACE_O_TRACESYSGOOD
        entry = True
        selected = None
        seen = []
        deadline = time.monotonic() + 30
        for _ in range(32768):
            if time.monotonic() > deadline: raise RuntimeError("Owned original-fsync trace deadline")
            ptrace(24, tid)  # PTRACE_SYSCALL
            status = wait_trace(tid)
            if not os.WIFSTOPPED(status): raise RuntimeError("Owned trace exited before fsync")
            if os.WSTOPSIG(status) != signal.SIGTRAP | 0x80: continue
            regs = Registers()
            ptrace(12, tid, ctypes.byref(regs))  # x86_64 PTRACE_GETREGS
            if entry:
                selected = None
                if regs.orig_rax == syscall:  # x86_64 write/fsync; original own descriptor only
                    path = os.readlink("/proc/" + str(tid) + "/fd/" + str(regs.rdi))
                    if path in targets:
                        if syscall == 1 and regs.rdx != 256 * 1024: raise RuntimeError("Unexpected original floor write length")
                        selected = path
            elif selected is not None:
                if regs.rax != return_value: raise RuntimeError("Selected original write/sync did not succeed")
                seen.append(selected)
                if len(seen) == wanted:
                    if seen != targets[:wanted]: raise RuntimeError("Unexpected original syscall order")
                    return [{"syscall": "write" if syscall == 1 else "fsync", "path": path, "return": return_value} for path in seen] if floor_boundary else seen
            entry = not entry
        raise RuntimeError("Selected original fsync boundary not observed")

    try:
        for label, data in binaries.items():
            path = root / label
            with path.open("xb") as f: f.write(data); f.flush(); os.fsync(f.fileno())
            path.chmod(0o500)
            m = path.stat()
            tools[label] = (path, (m.st_dev, m.st_ino), hashlib.sha256(data).hexdigest())
        image_fd = os.open(image, os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
        os.posix_fallocate(image_fd, 0, report["outer_reserved_bytes"])
        os.fsync(image_fd)
        report["formatter_sha256"] = digest(Path("/usr/sbin/mke2fs").resolve())
        call(["/usr/sbin/mke2fs", "-q", "-F", "-t", "ext4", "-m", "0", "-N", "4096", "-E", "nodiscard,lazy_itable_init=0,lazy_journal_init=0", str(image)])
        loop_name = call(["/usr/sbin/losetup", "--find", "--show", str(image)])
        loop_fd = os.open(loop_name, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
        m = os.fstat(loop_fd)
        if not stat.S_ISBLK(m.st_mode) or os.major(m.st_rdev) != 7: raise RuntimeError("Unexpected loop object")
        mount()
        for mode in modes:
            case = mountpoint / mode
            case.mkdir(mode=0o700)
            (case / "tmp").mkdir(mode=0o700)
            env = {k: v for k, v in os.environ.items() if not k.startswith(("COWORK_", "PG"))}
            env.update(COWORK_JOURNAL_CRASH_DIRECTORY=str(case), COWORK_JOURNAL_CRASH_MODE=mode, TMPDIR=str(case / "tmp"))
            active = subprocess.Popen([str(tools["tests"][0]), case_name, "--exact", "--ignored", "--nocapture"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env, start_new_session=True, bufsize=0)
            lines = []
            end = time.monotonic() + 20
            ready = None
            while time.monotonic() < end and ready is None:
                if not select.select([active.stdout], [], [], 1)[0]:
                    if active.poll() is not None: break
                    continue
                line = active.stdout.readline()
                if not line: break
                if sum(map(len, lines)) + len(line) > 65536: raise RuntimeError("Owned worker log bound")
                lines.append(line)
                if line.startswith(b"FLOOR_WORKER_READY=" if args.release_floor else b"JOURNAL_WORKER_READY="): ready = json.loads(line.split(b"=", 1)[1])
            if ready is None: raise RuntimeError("Owned worker not ready: " + b"".join(lines).decode(errors="replace"))
            if ready["pid"] != active.pid or ready["mode"] != mode or not (Path("/proc") / str(active.pid) / "task" / str(ready["tid"])).exists():
                raise RuntimeError("Owned worker identity changed")
            syncs = []
            if mode.endswith("parent-sync"):
                trace_tid = ready["tid"]
                syncs = stop_after_sync(trace_tid, case, 1 if mode.startswith("first") else 2)
            if args.release_floor and mode in ("after-write", "after-sync"):
                trace_tid = ready["tid"]
                syncs = stop_after_sync(trace_tid, case, 1, mode)
            active.kill()  # exact new Popen child, never PID/name discovery
            if trace_tid is not None:
                status = wait_trace(trace_tid)
                if not os.WIFSIGNALED(status) or os.WTERMSIG(status) != signal.SIGKILL: raise RuntimeError("Owned trace termination unconfirmed")
                trace_tid = None
            tail, errors = active.communicate(timeout=10)
            if active.returncode != -signal.SIGKILL: raise RuntimeError("Owned worker termination unconfirmed")
            (output / (log_prefix + mode + ".stdout.log")).write_bytes(b"".join(lines) + tail)
            (output / (log_prefix + mode + ".stderr.log")).write_bytes(errors)
            active = None
            if args.release_floor:
                expected = None if mode in ("torn-write", "enospc") else 0 if mode == "initialized" else 7 if mode in ("advanced", "before-write") else 11
                view = observe(case, expected)
                if mode == "torn-write" and b"RELEASE_FLOOR_ACTUAL_PARTIAL_WRITE_EFBIG=true" not in b"".join(lines) + tail:
                    raise RuntimeError("Actual partial write error unconfirmed")
                if mode == "enospc" and b"RELEASE_FLOOR_ACTUAL_INITIAL_ENOSPC=true" not in b"".join(lines) + tail:
                    raise RuntimeError("Actual floor allocation error unconfirmed")
                boundary = {"expected_release_floor": expected, "successful_floor_syscalls_before_kill": syncs}
            else:
                expected = None if mode == "enospc" else "published" if mode == "complete" else "prepared" if mode in ("prepared", "intent-io-error") else "intent"
                count = len(list((case / "versions").iterdir()))
                if count != (1 if mode in ("renamed", "first-parent-sync", "second-parent-sync", "complete") else 0): raise RuntimeError("Unexpected physical rename result")
                view = observe(case, expected)
                boundary = {"version_roots": count, "expected_history_phase": expected, "successful_original_parent_fsyncs_before_kill": syncs}
            report["cases"].append({"mode": mode, "worker_exit_signal": "SIGKILL", **boundary,
                "status_only_unchanged_readback": True, "history": view,
                "stdout_sha256": digest(output / (log_prefix + mode + ".stdout.log")), "stderr_sha256": digest(output / (log_prefix + mode + ".stderr.log"))})
        unmount()
        mount()
        for case in report["cases"]:
            if observe(mountpoint / case["mode"], case["expected_release_floor" if args.release_floor else "expected_history_phase"]) != case["history"]: raise RuntimeError("Normal ext4 remount changed retained history")
        report["normal_remount_policy_confirmed" if args.release_floor else "normal_remount_history_confirmed"] = True
        report["all_eight_release_floor_fault_cases_confirmed" if args.release_floor else "all_eight_process_fault_cases_confirmed"] = True
        report["result"] = "passed"
    except Exception as error:
        report.update(result="failed", failure=str(error))
    finally:
        try:
            if active is not None:
                if active.poll() is None: active.kill()
                if trace_tid is not None:
                    try: wait_trace(trace_tid)
                    except (ChildProcessError, RuntimeError): pass
                tail, errors = active.communicate(timeout=10)
                (output / (log_prefix + mode + ".stdout.log")).write_bytes(b"".join(lines) + tail)
                (output / (log_prefix + mode + ".stderr.log")).write_bytes(errors)
            if mounted: unmount()
            if loop_fd is not None:
                require_loop()
                fcntl.ioctl(loop_fd, 0x4C01, 0)  # clear this pinned original loop
                if loop_info() is not None: raise RuntimeError("Own loop detach not physically confirmed")
                os.close(loop_fd); loop_fd = None
                report["loop_detached_confirmed"] = True
            if loop_name is not None and not report["loop_detached_confirmed"]:
                raise RuntimeError("Original loop detachment unknown; image retained")
            if image_fd is not None:
                original = os.fstat(image_fd); current = image.lstat()
                if not stat.S_ISREG(current.st_mode) or (current.st_dev, current.st_ino) != (original.st_dev, original.st_ino): raise RuntimeError("Original image changed")
                image.unlink(); os.close(image_fd); image_fd = None
            for path, identity, expected in tools.values():
                m = path.lstat()
                if (m.st_dev, m.st_ino) != identity or digest(path) != expected: raise RuntimeError("Own executable changed")
                path.unlink()
            m = root.lstat()
            if (m.st_dev, m.st_ino) != (root_id.st_dev, root_id.st_ino) or root.parent != Path("/var/tmp"): raise RuntimeError("Original fixture root changed")
            mountpoint.rmdir(); root.rmdir(); report["fixture_removed"] = True
        except Exception as error:
            report.update(result="failed", cleanup_failure=str(error))
        (output / report_name).write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"result": report["result"], "cases": len(report["cases"]), "fixture_removed": report["fixture_removed"], "failure": report.get("failure"), "cleanup_failure": report.get("cleanup_failure")}), flush=True)
    return int(report["result"] != "passed")


if __name__ == "__main__": raise SystemExit(main())
