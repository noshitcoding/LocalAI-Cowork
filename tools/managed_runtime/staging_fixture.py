"""Actual ENOSPC in an owned private 8-MiB Linux filesystem; no workload starts.

Only a new mount namespace and its own tmpfs are used. No global WSL settings,
foreign mounts, processes or files are changed. This is storage exhaustion, not
rootfs/package isolation qualification or a persistent block-device proof.
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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--test-binary", type=Path, required=True)
    parser.add_argument("--expected-sha256", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--suite", choices=("enospc", "extraction", "custody", "deep", "installation", "installation-failure", "publication", "journal", "journal-status", "release-floor", "installed-capability", "floor-anchor"), default="enospc")
    parser.add_argument("--package-binary", type=Path)
    parser.add_argument("--expected-package-sha256")
    args = parser.parse_args()
    if os.geteuid() != 0:
        raise ValueError("Use root only for this owned private filesystem fixture")
    repo = Path(__file__).resolve().parents[2]
    output = args.output.resolve(strict=True)
    if not output.is_relative_to(repo / ".local/v2-rebuild-validation"):
        raise ValueError("Fresh project validation output required")
    binary = args.test_binary.resolve(strict=True)
    with binary.open("rb") as handle:
        content = handle.read(128 * 1024**2 + 1)
    if len(content) > 128 * 1024**2 or not content.startswith(b"\x7fELF") or hashlib.sha256(content).hexdigest() != args.expected_sha256:
        raise ValueError("Exact bounded native ELF required")
    package_content = None
    if args.suite != "enospc":
        if not args.package_binary or not args.expected_package_sha256:
            raise ValueError("Exact native package CLI required for extraction fixture")
        with args.package_binary.resolve(strict=True).open("rb") as handle:
            package_content = handle.read(128 * 1024**2 + 1)
        if len(package_content) > 128 * 1024**2 or not package_content.startswith(b"\x7fELF") or hashlib.sha256(package_content).hexdigest() != args.expected_package_sha256:
            raise ValueError("Actual native package CLI checksum changed")
    native = ctypes.CDLL(None, use_errno=True)
    native.mount.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_char_p, ctypes.c_ulong, ctypes.c_void_p]
    native.umount2.argtypes = [ctypes.c_char_p, ctypes.c_int]
    before = os.readlink("/proc/thread-self/ns/mnt")
    if native.unshare(0x00020000) != 0 or native.mount(None, b"/", None, 16384 | 262144, None) != 0:
        raise OSError(ctypes.get_errno(), "Private mount namespace unavailable")
    if os.readlink("/proc/thread-self/ns/mnt") == before:
        raise RuntimeError("Mount namespace unchanged")
    root = Path("/var/tmp") / ("cowork-staging-space-" + uuid.uuid4().hex)
    root.mkdir(mode=0o700)
    original = root.stat()
    mountpoint = root / "bounded"
    mountpoint.mkdir(mode=0o700)
    mounted = False
    private_binary = root / "staging-tests"
    with private_binary.open("xb") as handle:
        handle.write(content)
        handle.flush()
        os.fsync(handle.fileno())
    private_binary.chmod(0o500)
    binary_id = private_binary.stat()
    private_package = root / "runtime-package"
    package_id = None
    if package_content is not None:
        with private_package.open("xb") as handle:
            handle.write(package_content)
            handle.flush()
            os.fsync(handle.fileno())
        private_package.chmod(0o500)
        package_id = private_package.stat()
    report = {"scope": "Original signed-package staging rejects actual ENOSPC before copying; private 8-MiB tmpfs only",
              "result": "running", "fixture": str(root), "test_binary_sha256": args.expected_sha256,
              "filesystem_limit_bytes": 8 * 1024**2, "private_mount_namespace": True,
              "installed": False, "qualified": False, "execution_authority": False,
              "fixture_removed": False, "normal_unmount_confirmed": False}
    report["suite"] = args.suite
    label = {"enospc": "enospc", "extraction": "archive-extraction", "custody": "archive-custody", "deep": "archive-deep-budget", "installation": "nine-role-installation", "installation-failure": "nine-role-storage-failure", "publication": "atomic-publication", "journal": "publication-journal", "journal-status": "publication-journal-status", "release-floor": "runtime-release-floor", "installed-capability": "installed-package-current", "floor-anchor": "release-floor-anchor"}[args.suite]
    if args.suite != "enospc":
        report["scope"] = "Actual UID-0 bounded archive extraction and original-object readback; no workload or profile qualification"
        report["package_binary_sha256"] = args.expected_package_sha256
    if args.suite == "custody":
        report["scope"] = "Actual UID-0 original target descriptors after unlink/replacement, own descriptor Drop and scoped child EMFILE; no workload qualification"
    if args.suite == "deep":
        report["scope"] = "Valid signed forty-directory tree, scoped CLI RLIMIT_NOFILE140 and rejection before target members; no workload qualification"
    if args.suite.startswith("installation"):
        report["scope"] = "Actual whole-nine-role preparation or last-role storage failure before any role payload; synthetic packages, no installation/activation qualification"
    if args.suite == "publication":
        report["scope"] = "Actual UID-0 atomic NOREPLACE publication of original signed input plus all nine roles; collision/no-replay and retained custody, tmpfs only, no persistent crash/activation proof"
    if args.suite == "journal":
        report["scope"] = "Actual CLI publication requires its original durable journal; synthetic package, no installation/activation qualification"
    if args.suite == "journal-status":
        report["scope"] = "Actual original-owner loss, independent signature trust, status-only CLI and corrupt/missing journal denial; no process-crash or activation claim"
    if args.suite == "release-floor":
        report["scope"] = "Actual root-only restrictive release policy, CLI downgrade/reuse denial, torn-slot and original replacement rejection, EFBIG completion; no activation or installed profile"
    if args.suite == "floor-anchor":
        report["scope"] = "Test-build-only deterministic scheduling of actual valid policy replacement files and directory ABA; synthetic signed releases, no unchanged original ELF or workload qualification"
        report["test_build_instrumentation"] = True
    if args.suite == "installed-capability":
        report["scope"] = "Fresh current signed inputs plus all nine actual role trees, wrong real identity and mixed-role rejection; synthetic bytes, no backend/workload/activation"
    inode_limit = 512 if args.suite in ("publication", "journal", "journal-status", "installed-capability", "floor-anchor") else 256 if args.suite == "installation" else 64
    report["filesystem_inode_limit"] = inode_limit
    try:
        if native.mount(b"cowork-staging-fixture", os.fsencode(mountpoint), b"tmpfs", 2 | 4 | 8,
                        ("size=8388608,nr_inodes=" + str(inode_limit) + ",mode=0700").encode()) != 0:
            raise OSError(ctypes.get_errno(), "Own bounded tmpfs mount failed")
        mounted = True
        mounted_id = mountpoint.stat()
        if mounted_id.st_dev == original.st_dev:
            raise RuntimeError("Own mount not distinct")
        child_env = dict(os.environ, COWORK_STAGING_ENOSPC_PARENT=str(mountpoint))
        if package_id is not None:
            child_env["COWORK_STAGING_PACKAGE_BINARY"] = str(private_package)
        case = {"enospc": "staging::actual_reserved_storage_exhaustion_cannot_publish_staging",
                "extraction": "staging::archive::actual_root_archive_extraction_keeps_original_objects_and_never_follows_links",
                "custody": "staging::archive::actual_root_target_custody_survives_unlink_and_rejects_descriptor_exhaustion",
                "deep": "staging::archive::actual_deep_valid_tree_descriptor_budget_rejects_before_target_mutation",
                "installation": "staging::archive::installation::actual_nine_role_session_prepares_every_role_and_cli_and_rejects_replacement",
                "installation-failure": "staging::archive::installation::actual_last_role_storage_failure_happens_before_any_role_payload_copy",
                "publication": "staging::archive::installation::publication::actual_atomic_publication_retains_every_original_and_never_overwrites_or_replays",
                "journal": "staging::archive::installation::publication::journal::actual_cli_publication_requires_its_durable_original_journal",
                "journal-status": "staging::archive::installation::publication::journal::actual_history_reads_never_dispatch_or_initialize_and_require_independent_trust",
                "release-floor": "release_floor::actual_release_floor_cli_and_original_policy_fail_closed",
                "floor-anchor": "staging::release_floor::anchor_tests::", "installed-capability": "staging::archive::installation::publication::current::actual_current_package_proves_all_roles_and_rejects_wrong_or_mixed_bytes"}[args.suite]
        result = subprocess.run([str(private_binary), case,
                                 *([] if args.suite == "floor-anchor" else ["--exact"]), "--ignored", "--nocapture", "--test-threads=1"],
                                env=child_env, capture_output=True, timeout=30, check=False)
        (output / (label + ".stdout.log")).write_bytes(result.stdout)
        (output / (label + ".stderr.log")).write_bytes(result.stderr)
        report["test_exit_code"] = result.returncode
        report["test_stdout_sha256"] = hashlib.sha256(result.stdout).hexdigest()
        report["test_stderr_sha256"] = hashlib.sha256(result.stderr).hexdigest()
        if result.returncode or (b"2 passed; 0 failed;" if args.suite == "floor-anchor" else b"1 passed; 0 failed;") not in result.stdout:
            raise RuntimeError("Actual native " + args.suite + " test failed")
        if args.suite == "extraction":
            if b"ARCHIVE_NATIVE_CLI_EXTRACTED=true" not in result.stdout:
                raise RuntimeError("Actual native CLI extraction marker absent")
            report["actual_cli_extraction_observed"] = True
        if args.suite == "custody":
            for marker, key in ((b"ARCHIVE_TARGET_ORIGINALS_PINNED=true", "originals_pinned_after_unlink"),
                                (b"ARCHIVE_TARGET_DESCRIPTORS_DROPPED=true", "original_descriptors_closed_on_drop"),
                                (b"ARCHIVE_TARGET_FD_LIMIT_REJECTED=true", "scoped_child_descriptor_exhaustion_rejected")):
                if marker not in result.stdout:
                    raise RuntimeError("Actual original-target custody witness absent: " + key)
                report[key] = True
        if args.suite == "deep":
            if b"ARCHIVE_DEEP_FD_BUDGET_REJECTED_BEFORE_MUTATION=true" not in result.stdout or b"DEEP_CLI_TARGET_MEMBERS=0" not in result.stdout:
                raise RuntimeError("Deep-tree admission witness absent")
            report["deep_budget_rejected_before_mutation"] = True
        if args.suite.startswith("installation"):
            marker = b"NINE_ROLE_API_AND_CLI_PREPARED_WITH_ORIGINAL_CUSTODY=true" if args.suite == "installation" else b"NINE_ROLE_ENOSPC_BEFORE_ANY_PAYLOAD=true"
            if marker not in result.stdout:
                raise RuntimeError("Actual complete-installation witness absent")
            report["all_nine_roles_prepared_with_original_custody" if args.suite == "installation" else "last_role_enospc_before_any_payload"] = True
            if args.suite == "installation":
                if b"NINE_ROLE_FD_BUDGET_BEFORE_ROLE_ROOTS=true" not in result.stdout:
                    raise RuntimeError("Actual aggregate descriptor witness absent")
                report["aggregate_descriptors_admitted_before_role_roots"] = True
        if args.suite == "publication":
            for marker, key in ((b"ATOMIC_PUBLICATION_ORIGINAL_ROOT_AND_NINE_ROLES=true", "original_root_and_nine_roles_after_rename"),
                                (b"ATOMIC_PUBLICATION_NOREPLACE_AND_NO_REPLAY=true", "actual_noreplace_and_no_rename_replay"),
                                (b"ATOMIC_PUBLICATION_ACTUAL_CLI=true", "actual_cli_publication"),
                                (b"ATOMIC_PUBLICATION_REOBSERVE_ORIGINAL_AFTER_DISPATCH=true", "original_after_successful_unconfirmed_dispatch"),
                                (b"ATOMIC_PUBLICATION_CROSS_FILESYSTEM_REJECTED_WITHOUT_COPY=true", "actual_cross_filesystem_rejected_without_copy"),
                                (b"ATOMIC_PUBLICATION_COMBINED_INPUT_AND_EXPANDED_BUDGET=true", "combined_input_and_expanded_budget"),
                                (b"ATOMIC_PUBLICATION_ORIGINAL_CUSTODY_AFTER_RENAME=true", "original_custody_after_rename")):
                if marker not in result.stdout: raise RuntimeError("Actual atomic publication witness absent: " + key)
                report[key] = True
        if args.suite == "journal":
            if b"ACTUAL_CLI_DURABLE_ORIGINAL_JOURNAL=true" not in result.stdout:
                raise RuntimeError("Actual durable journal witness absent")
            report["actual_durable_journal_observed"] = True
        if args.suite == "journal-status":
            for marker in (b"PUBLICATION_HISTORY_STATUS_ONLY_AND_NO_INIT=true", b"PUBLICATION_HISTORY_INDEPENDENT_TRUST_REQUIRED=true", b"PUBLICATION_HISTORY_CORRUPT_OR_MISSING_STATE_FAILS_CLOSED=true"):
                if marker not in result.stdout: raise RuntimeError("Actual status-only history witness absent")
            report["actual_journal_status_observed"] = True
        if args.suite == "release-floor":
            for marker in (b"ACTUAL_RELEASE_FLOOR_POLICY_ONLY=true", b"ACTUAL_RELEASE_FLOOR_EFBIG_AND_FAIL_CLOSED=true", b"ACTUAL_RELEASE_FLOOR_CONCURRENT_READS=256"):
                if marker not in result.stdout: raise RuntimeError("Actual release policy witness absent")
            report["actual_restrictive_release_floor_observed"] = True
            report["actual_concurrent_policy_reads"] = 256
        if args.suite == "installed-capability":
            for marker in (b"INSTALLED_ANCHORED_POLICY_REPLACEMENT_REJECTED=true", b"INSTALLED_CURRENT_NINE_ROLES_AND_NONBLOCKING_FLOOR=true", b"INSTALLED_WRONG_REAL_PACKAGE_IDENTITY_REJECTED=true", b"INSTALLED_MIXED_SIGNED_ROLE_TREES_REJECTED=true", b"INSTALLED_ORIGINAL_REPLACEMENT_NEVER_ADOPTED=true", b"INSTALLED_FIXTURE_NO_BACKEND_OR_WORKLOAD_STARTED=true"):
                if marker not in result.stdout: raise RuntimeError("Current installed-byte witness absent")
            report["actual_current_installed_bytes_observed"] = True
        if args.suite == "floor-anchor":
            for marker in (b"FLOOR_ANCHOR_ACQUISITION_REJECTED=true", b"FLOOR_ANCHOR_ADMISSION_ABA_REJECTED=true", b"FLOOR_ANCHOR_INITIAL_MINIMUM_RETAINED=true"):
                if marker not in result.stdout: raise RuntimeError("Actual instrumented anchor race witness absent")
            report["actual_instrumented_anchor_races_observed"] = True
        current = mountpoint.stat()
        if (current.st_dev, current.st_ino) != (mounted_id.st_dev, mounted_id.st_ino):
            raise RuntimeError("Own bounded mount identity changed")
        fs = os.statvfs(mountpoint)
        report["allocated_filesystem_bytes"] = (fs.f_blocks - fs.f_bfree) * fs.f_frsize
        report["remaining_available_bytes"] = fs.f_bavail * fs.f_frsize
        report[{"enospc": "actual_enospc_observed", "extraction": "actual_root_extraction_observed", "custody": "actual_original_target_custody_observed", "deep": "actual_deep_descriptor_budget_observed", "installation": "actual_nine_role_installation_observed", "installation-failure": "actual_nine_role_storage_failure_observed", "publication": "actual_atomic_publication_observed", "journal": "actual_durable_journal_observed", "journal-status": "actual_journal_status_observed", "release-floor": "actual_restrictive_release_floor_observed", "installed-capability": "actual_current_installed_bytes_observed", "floor-anchor": "actual_instrumented_anchor_races_observed"}[args.suite]] = True
        report["result"] = "passed"
    except Exception as error:
        report.update(result="failed", failure=str(error))
    finally:
        if mounted:
            if native.umount2(os.fsencode(mountpoint), 0) != 0:
                report.update(result="failed", cleanup_failure="Own normal unmount unconfirmed; fixture retained")
            else:
                report["normal_unmount_confirmed"] = True
                mounted = False
        if not mounted:
            current = root.lstat()
            if stat.S_ISDIR(current.st_mode) and (current.st_dev, current.st_ino) == (original.st_dev, original.st_ino):
                # No recursion: only the original now-unmounted empty mountpoint
                # and our own empty root. Never delete an adopted child path.
                current_binary = private_binary.lstat()
                if not stat.S_ISREG(current_binary.st_mode) or (current_binary.st_dev, current_binary.st_ino) != (binary_id.st_dev, binary_id.st_ino):
                    raise RuntimeError("Own original test executable replaced; fixture retained")
                private_binary.unlink()
                if package_id is not None:
                    current_package = private_package.lstat()
                    if not stat.S_ISREG(current_package.st_mode) or (current_package.st_dev, current_package.st_ino) != (package_id.st_dev, package_id.st_ino):
                        raise RuntimeError("Own original package tool replaced; fixture retained")
                    private_package.unlink()
                mountpoint.rmdir()
                root.rmdir()
                report["fixture_removed"] = True
        (output / (label + "-report.json")).write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report), flush=True)
    return int(report["result"] != "passed")


if __name__ == "__main__":
    raise SystemExit(main())
