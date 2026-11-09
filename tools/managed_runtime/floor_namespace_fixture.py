"""Actual same-parent release-policy swap with one exact supplied ELF.

Own mount namespace and 8-MiB tmpfs only. Synthetic public signing fixtures;
no execution/activation or installed-profile authority is tested or granted.
"""
import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess
import uuid
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from journal_fixture import inventory

ROLES = ("base_rootfs", "browser_tools", "containerd_bundle", "document_tools", "git_tools",
         "gvisor_bundle", "node_tools", "python_tools", "runtime_service")


def digest(data):
    return hashlib.sha256(data).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package-binary", type=Path, required=True)
    parser.add_argument("--package-sha256", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if os.geteuid() != 0: raise RuntimeError("Owned root fixture required")
    repo = Path(__file__).resolve().parents[2]
    output = args.output.resolve(strict=True)
    if not output.is_relative_to(repo / ".local/v2-rebuild-validation"):
        raise ValueError("Own fresh qualification output required")
    source = args.package_binary.resolve(strict=True)
    if not stat.S_ISREG(source.stat().st_mode) or source.stat().st_size > 128 * 1024**2:
        raise ValueError("Bounded original native program required")
    data = source.read_bytes()
    if not data.startswith(b"\x7fELF") or digest(data) != args.package_sha256:
        raise ValueError("Exact original native ELF required")
    native = ctypes.CDLL(None, use_errno=True)
    native.mount.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_char_p, ctypes.c_ulong, ctypes.c_void_p]
    native.umount2.argtypes = [ctypes.c_char_p, ctypes.c_int]
    old_namespace = os.readlink("/proc/thread-self/ns/mnt")
    if native.unshare(0x00020000) != 0 or native.mount(None, b"/", None, 16384 | 262144, None) != 0:
        raise OSError(ctypes.get_errno(), "Private mount namespace unavailable")
    if os.readlink("/proc/thread-self/ns/mnt") == old_namespace:
        raise RuntimeError("Own namespace unchanged")
    root = Path("/var/tmp") / ("cowork-floor-namespace-" + uuid.uuid4().hex)
    root.mkdir(mode=0o700)
    root_id = root.stat()
    mountpoint = root / "store"
    mountpoint.mkdir(mode=0o700)
    binary = root / "package"
    with binary.open("xb") as f:
        f.write(data); f.flush(); os.fsync(f.fileno())
    binary.chmod(0o500)
    binary_id = binary.stat()
    mounted = False
    report = {"scope": "Actual same-parent floor-policy namespace substitution using exact signed files and exact native ELF; no workload or activation authority",
              "result": "running", "package_binary_sha256": args.package_sha256,
              "private_mount_namespace": True, "filesystem_limit_bytes": 8 * 1024**2,
              "filesystem_inode_limit": 128, "commands": [], "normal_unmount_confirmed": False,
              "fixture_removed": False, "namespace_swap_rejected": False, "installed": False,
              "activated": False, "qualified": False, "execution_authority": False}

    def call(argv, success=True):
        result = subprocess.run([str(binary), *map(str, argv)], capture_output=True, timeout=10, check=False)
        report["commands"].append({"arguments": list(map(str, argv)), "exit_code": result.returncode,
                                   "stdout": result.stdout.decode(errors="replace"), "stderr": result.stderr.decode(errors="replace")})
        if success and result.returncode: raise RuntimeError("Valid original policy command failed")
        return result

    try:
        if native.mount(b"cowork-floor-namespace", os.fsencode(mountpoint), b"tmpfs", 2 | 4 | 8,
                        b"size=8388608,nr_inodes=128,mode=0700") != 0:
            raise OSError(ctypes.get_errno(), "Own bounded mount failed")
        mounted = True
        mount_id = mountpoint.stat()
        if mount_id.st_dev == root_id.st_dev: raise RuntimeError("Own mount not distinct")
        key = Ed25519PrivateKey.generate()
        public = key.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)
        keypath = mountpoint / "trusted.pub"
        keypath.write_bytes(public); keypath.chmod(0o600)
        packages = {}
        for name, sequence in (("a", 7), ("b", 11)):
            package = mountpoint / ("package-" + name)
            package.mkdir(mode=0o700)
            (package / "payload").mkdir(mode=0o700)
            components = []
            for role in ROLES:
                filename = role + ".payload"
                (package / "payload" / filename).write_bytes(b"test")
                components.append({"role": role, "filename": filename, "version": "fixture.1", "bytes": 4, "sha256": digest(b"test")})
            manifest = {"format_version": 1, "platform": "linux_x86_64", "version": "2.0.0-alpha.7",
                        "release_sequence": sequence, "components": components}
            raw = (json.dumps(manifest, indent=2, sort_keys=True) + "\n").encode()
            (package / "manifest.json").write_bytes(raw)
            signature = {"key_id": "fixture-only", "manifest_sha256": digest(raw),
                         "signature": list(key.sign(b"localai-cowork/runtime-package/v1\0" + raw))}
            (package / "manifest.sig.json").write_text(json.dumps(signature), encoding="utf-8")
            packages[name] = (package, sequence, digest(raw))
            call(["--initialize-release-floor", mountpoint, name, keypath, "fixture-only", "linux_x86_64"])
            call([package, keypath, "fixture-only", "linux_x86_64", "--advance-release-floor", mountpoint, name])
        def status(name, success=True):
            return call(["--release-floor-status", mountpoint, name, keypath, "fixture-only", "linux_x86_64"], success)
        original = {name: json.loads(status(name).stdout) for name in ("a", "b")}
        for name, (_, sequence, expected_hash) in packages.items():
            if original[name]["release_sequence"] != sequence or original[name]["manifest_sha256"] != expected_hash:
                raise RuntimeError("Sequential controls do not bind the exact signed files")
        report["original_sequential_controls"] = original
        report["exact_signed_manifest_hashes"] = {name: value[2] for name, value in packages.items()}
        (mountpoint / "b").rename(mountpoint / "saved-b")
        (mountpoint / "a").rename(mountpoint / "b")
        before = inventory(mountpoint)
        observed = status("b", False)
        if inventory(mountpoint) != before: raise RuntimeError("Policy Open mutated retained inputs")
        report["status_read_unchanged"] = True
        if observed.returncode == 0:
            report["after_swap"] = json.loads(observed.stdout)
            report["actual_lower_floor_admitted_under_b"] = report["after_swap"]["release_sequence"] == 7
        else:
            report["namespace_swap_rejected"] = True
        # Explicit fixture restoration of the same original inodes. This is not
        # production recovery and never reclaims/adopts a runtime or job volume.
        (mountpoint / "b").rename(mountpoint / "a")
        (mountpoint / "saved-b").rename(mountpoint / "b")
        restored = {name: json.loads(status(name).stdout) for name in ("a", "b")}
        if restored != original: raise RuntimeError("Restored original policy controls changed")
        report["original_policies_restored_and_verified"] = True
        if not report["namespace_swap_rejected"]:
            raise RuntimeError("Original CLI admitted a different policy's lower floor under b")
        report["result"] = "passed"
    except Exception as error:
        report.update(result="failed", failure=str(error))
    finally:
        if mounted:
            if native.umount2(os.fsencode(mountpoint), 0) != 0:
                report.update(result="failed", cleanup_failure="Own normal unmount unconfirmed; retained")
            else:
                mounted = False; report["normal_unmount_confirmed"] = True
        if not mounted:
            m = root.lstat(); b = binary.lstat()
            if (m.st_dev, m.st_ino) != (root_id.st_dev, root_id.st_ino) or root.parent != Path("/var/tmp"):
                raise RuntimeError("Original fixture root changed")
            if (b.st_dev, b.st_ino) != (binary_id.st_dev, binary_id.st_ino) or digest(binary.read_bytes()) != args.package_sha256:
                raise RuntimeError("Original fixture executable changed")
            binary.unlink(); mountpoint.rmdir(); root.rmdir(); report["fixture_removed"] = True
        (output / "release-floor-namespace-report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({k: report.get(k) for k in ("result", "namespace_swap_rejected", "actual_lower_floor_admitted_under_b", "normal_unmount_confirmed", "fixture_removed", "failure")}), flush=True)
    return int(report["result"] != "passed")


if __name__ == "__main__": raise SystemExit(main())
