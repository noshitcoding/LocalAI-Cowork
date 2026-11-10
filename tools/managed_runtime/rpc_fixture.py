"""Run only the exact transport test ELF in a private Linux mount namespace.

containerd 2.2 also creates its compiled-in /run/containerd, even with --state.
Hide /run for this fixture; never create or change a host containerd directory.
This starts no tasks and is not workload/runtime qualification.
"""
import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess
import tempfile


def checked_binary(path, expected):
    path = path.resolve(strict=True)
    with path.open("rb") as source:
        content = source.read(128 * 1024**2 + 1)
    if len(content) > 128 * 1024**2 or not content.startswith(b"\x7fELF") or hashlib.sha256(content).hexdigest() != expected:
        raise ValueError("Exact fixture ELF checksum changed")
    return content


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--test-binary", type=Path, required=True)
    parser.add_argument("--test-sha256", required=True)
    parser.add_argument("--containerd-binary", type=Path, required=True)
    parser.add_argument("--containerd-sha256", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if os.geteuid() != 0:
        raise ValueError("Only the explicitly private root fixture is supported")
    root = Path(__file__).resolve().parents[2]
    output = args.output.resolve(strict=True)
    if not output.is_relative_to(root / ".local/v2-rebuild-validation"):
        raise ValueError("Use a fresh project qualification output directory")
    test = checked_binary(args.test_binary, args.test_sha256)
    daemon = checked_binary(args.containerd_binary, args.containerd_sha256)
    libc = ctypes.CDLL(None, use_errno=True)

    def check(value):
        if value != 0:
            raise OSError(ctypes.get_errno(), "Private mount namespace operation failed")

    original_namespace = os.readlink("/proc/self/ns/mnt")
    check(libc.unshare(0x00020000))  # CLONE_NEWNS; no change to parent namespace.
    check(libc.mount(None, b"/", None, ctypes.c_ulong((1 << 18) | (1 << 14)), None))  # MS_PRIVATE|MS_REC
    if os.readlink("/proc/self/ns/mnt") == original_namespace:
        raise ValueError("Mount namespace did not change")
    check(libc.mount(b"tmpfs", b"/run", b"tmpfs", ctypes.c_ulong(2 | 4), b"mode=0755,size=8m,nr_inodes=128"))
    report = {"scope": "Actual private containerd transport only; no workload task dispatch",
              "result": "running", "private_mount_namespace": True, "runtime_qualified": False,
              "test_binary_sha256": args.test_sha256, "containerd_binary_sha256": args.containerd_sha256,
              "own_test_directory_removed": False, "private_run_unmounted": False,
              "own_daemons_reaped": False, "workload_tasks_launched": False}
    directory = Path(tempfile.mkdtemp(prefix="cowork-containerd-fixture-"))
    report["fixture_directory"] = str(directory)
    try:
        directory.chmod(0o700)
        identities = {}
        for name, data in (("transport-tests", test), ("containerd", daemon)):
            path = directory / name
            with path.open("xb") as target:
                target.write(data)
            path.chmod(0o500)
            m = path.stat()
            if not stat.S_ISREG(m.st_mode) or m.st_uid != 0:
                raise ValueError("Fixture executable custody changed")
            identities[name] = (m.st_dev, m.st_ino)
        env = {"PATH": "/usr/sbin:/usr/bin:/sbin:/bin", "COWORK_TEST_PRIVATE_CONTAINERD_NS": "1",
               "COWORK_TEST_CONTAINERD_BINARY": str(directory / "containerd"),
               "COWORK_TEST_CONTAINERD_SHA256": args.containerd_sha256}
        result = subprocess.run([str(directory / "transport-tests"), "--ignored", "--nocapture"],
                                env=env, capture_output=True, check=False, timeout=90)
        (output / "physical-rpc.stdout.log").write_bytes(result.stdout)
        (output / "physical-rpc.stderr.log").write_bytes(result.stderr)
        if result.returncode != 0 or b"own_daemons_reaped=true" not in result.stdout:
            raise RuntimeError("Private transport test failed; inspect bounded test logs")
        report["own_daemons_reaped"] = True
        # No automatic recursive removal on test/termination failure. Only the
        # two exact owned immutable files are removed after positive test exit.
        for name, identity in identities.items():
            path = directory / name
            m = path.lstat()
            if not stat.S_ISREG(m.st_mode) or (m.st_dev, m.st_ino) != identity:
                raise ValueError("Fixture executable identity changed before cleanup")
            path.unlink()
        directory.rmdir()
        report["own_test_directory_removed"] = not directory.exists()
        check(libc.umount2(b"/run", 0))  # Normal unmount only, no force/lazy.
        report.update(result="passed", private_run_unmounted=True)
    except Exception as error:
        report.update(result="failed", failure=str(error))
    (output / "physical-rpc-report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report), flush=True)
    return int(report["result"] != "passed")


if __name__ == "__main__":
    raise SystemExit(main())
