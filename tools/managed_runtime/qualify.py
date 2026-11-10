"""Bind package verification/staging tests and unsigned tools to exact sources."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import uuid
import zipfile


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def source_inputs(root):
    paths = [root / n for n in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml")]
    for folder in ("crates/managed-runtime", "crates/executor", "tools/managed_runtime"):
        paths += [p for p in (root / folder).rglob("*") if p.is_file() and not p.is_symlink() and p.suffix in (".rs", ".toml", ".json", ".py", ".c") and "__pycache__" not in p.parts]
    return {p.relative_to(root).as_posix(): digest(p) for p in sorted(paths)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--wsl-distro")
    parser.add_argument("--linux-target-directory", help="Independent absolute Linux Cargo cache for coordinated private work")
    parser.add_argument("--staging-enospc", action="store_true", help="Own private 8-MiB tmpfs fixture, root only inside selected WSL namespace")
    args = parser.parse_args()
    if args.staging_enospc and not args.wsl_distro:
        parser.error("Select WSL for the owned Linux ENOSPC fixture")
    root = Path(__file__).resolve().parents[2]
    output = root / ".local/v2-rebuild-validation" / ("runtime-package-verifier-" + uuid.uuid4().hex)
    output.mkdir(parents=True)
    sources = source_inputs(root)
    (output / "sources.json").write_text(json.dumps(sources, indent=2) + "\n", encoding="utf-8")
    report = {"scope": "Fresh installed-byte custody of signed inputs and all nine roles plus restrictive current-policy admission; no mandatory bootstrap, activation, authority recovery or workload qualification", "result": "running",
              "runtime_service_delivered": False, "installer_delivered": False, "physical_runtime_qualified": False,
              "complete_A_to_I": False, "installed": False, "published": False, "checks": [], "artifacts": [],
              "limitations": ["Synthetic public signing-key vectors; no signed real runtime/tool bundle",
                              "WSL tests are filesystem/program checks, not native Linux hardware qualification",
                              "No activation, rollback, WSL distribution installation or workload dispatch; complete runtime installer integration remains open"]}

    def run(argv, label, minimum=0):
        result = subprocess.run(argv, cwd=root, capture_output=True, check=False)
        (output / (label + ".stdout.log")).write_bytes(result.stdout)
        (output / (label + ".stderr.log")).write_bytes(result.stderr)
        text = (result.stdout + result.stderr).decode("utf-8", errors="replace")
        count = sum(map(int, re.findall(r"test result: ok\. (\d+) passed;", text)))
        python_count = re.search(r"Ran (\d+) tests", text)
        if python_count:
            count += int(python_count.group(1))
        passed = result.returncode == 0 and count >= minimum
        report["checks"].append({"name": label, "command": argv, "exit_code": result.returncode, "tests": count, "passed": passed,
                                 "stdout_sha256": digest(output / (label + ".stdout.log")), "stderr_sha256": digest(output / (label + ".stderr.log"))})
        print(json.dumps({"check": label, "passed": passed, "tests": count}), flush=True)
        if not passed:
            raise RuntimeError("Check failed: " + label)
        return text

    def artifact(content, name, kind="development_package_inspector_only"):
        path = output / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)
        report["artifacts"].append({"path": name, "sha256": digest(path), "signed": False, "kind": kind})

    try:
        log = run(["cargo", "test", "--locked", "--offline", "-p", "cowork-managed-runtime", "--message-format=json,json-diagnostic-short"], "native-tests", 20)
        binaries = [json.loads(line)["executable"] for line in log.splitlines() if line.startswith('{"reason":"compiler-artifact"')
                    and json.loads(line).get("target", {}).get("kind") == ["bin"] and not json.loads(line).get("profile", {}).get("test")
                    and json.loads(line).get("executable")]
        if len(binaries) != 1:
            raise RuntimeError("One exact native package-inspector binary is required")
        path = Path(binaries[0])
        artifact(path.read_bytes(), "bin/native/" + path.name)
        native_test_binaries = [json.loads(line)["executable"] for line in log.splitlines()
                                if line.startswith('{"reason":"compiler-artifact"')
                                and json.loads(line).get("target", {}).get("name") == "packages"
                                and json.loads(line).get("profile", {}).get("test")
                                and json.loads(line).get("executable")]
        if len(native_test_binaries) != 1:
            raise RuntimeError("One exact native packages test executable required")
        test_path = Path(native_test_binaries[0])
        artifact(test_path.read_bytes(), "bin/native/" + test_path.name, "development_native_staging_test_only")
        run(["cargo", "clippy", "--locked", "--offline", "-p", "cowork-managed-runtime", "--all-targets", "--message-format=short", "--", "-D", "warnings"], "clippy")
        run(["cargo", "fmt", "-p", "cowork-managed-runtime", "--", "--check"], "format")
        run([shutil.which("python") or "python3", "tools/managed_runtime/test_package.py", "--verifier", str(path)], "native-builder", 4)
        if args.wsl_distro:
            prefix = ["wsl", "-d", args.wsl_distro, "--exec"]
            linux_root = subprocess.check_output(prefix + ["wslpath", "-u", str(root)], text=True).strip()
            linux_home = subprocess.check_output(prefix + ["printenv", "HOME"], text=True).strip()
            target = args.linux_target_directory or linux_home + "/.cache/localai-cowork-v2/workspace-receive-target"
            if not target.startswith("/") or ".." in target.split("/") or "\0" in target:
                raise ValueError("Absolute independent Linux target directory required")
            program = target + "/debug/cowork-runtime-package"
            linux_log = run(prefix + ["bash", "-lc", "cd " + shlex.quote(linux_root) + " && cargo test --locked --offline -p cowork-managed-runtime --message-format=json,json-diagnostic-short --target-dir " + shlex.quote(target)], "wsl-linux-tests", 24)
            run(prefix + ["bash", "-lc", "cd " + shlex.quote(linux_root) + " && cargo clippy --locked --offline -p cowork-managed-runtime --all-targets --message-format short --target-dir " + shlex.quote(target) + " -- -D warnings"], "wsl-linux-clippy")
            run(prefix + ["bash", "-lc", "cd " + shlex.quote(linux_root) + " && python3 tools/managed_runtime/test_package.py --verifier " + shlex.quote(program)], "wsl-linux-builder", 4)
            content = subprocess.run(prefix + ["cat", program], capture_output=True, check=True).stdout
            if not content.startswith(b"\x7fELF"):
                raise RuntimeError("Expected the actual Linux package-inspector ELF")
            artifact(content, "bin/linux/cowork-runtime-package")
            if args.staging_enospc:
                test_binaries = [json.loads(line)["executable"] for line in linux_log.splitlines()
                                 if line.startswith('{"reason":"compiler-artifact"')
                                 and json.loads(line).get("target", {}).get("name") == "packages"
                                 and json.loads(line).get("profile", {}).get("test")
                                 and json.loads(line).get("executable")]
                if len(test_binaries) != 1:
                    raise RuntimeError("One exact Linux packages test ELF required")
                test_bytes = subprocess.run(prefix + ["cat", test_binaries[0]], capture_output=True, check=True).stdout
                if not test_bytes.startswith(b"\x7fELF"):
                    raise RuntimeError("Actual package test ELF required")
                artifact(test_bytes, "bin/linux/staging-package-tests", "development_native_staging_test_only")
                unit_binaries = [json.loads(line)["executable"] for line in linux_log.splitlines()
                                 if line.startswith('{"reason":"compiler-artifact"')
                                 and json.loads(line).get("target", {}).get("kind") == ["lib"]
                                 and json.loads(line).get("profile", {}).get("test")
                                 and json.loads(line).get("executable")]
                if len(unit_binaries) != 1:
                    raise RuntimeError("One exact Linux instrumented unit-test ELF required")
                unit_bytes = subprocess.run(prefix + ["cat", unit_binaries[0]], capture_output=True, check=True).stdout
                if not unit_bytes.startswith(b"\x7fELF"):
                    raise RuntimeError("Actual instrumented unit-test ELF required")
                artifact(unit_bytes, "bin/linux/instrumented-anchor-unit-tests", "development_cfg_test_instrumentation_only")
                run(["wsl", "-d", args.wsl_distro, "--user", "root", "--exec", "python3",
                     linux_root + "/tools/managed_runtime/staging_fixture.py", "--test-binary", unit_binaries[0],
                     "--expected-sha256", hashlib.sha256(unit_bytes).hexdigest(), "--suite", "floor-anchor",
                     "--package-binary", program, "--expected-package-sha256", hashlib.sha256(content).hexdigest(),
                     "--output", subprocess.check_output(prefix + ["wslpath", "-u", str(output)], text=True).strip()], "wsl-linux-actual-floor-anchor")
                physical_path = output / "release-floor-anchor-report.json"
                physical = json.loads(physical_path.read_bytes())
                if not all(physical.get(key) is True for key in ("actual_instrumented_anchor_races_observed", "test_build_instrumentation", "fixture_removed", "normal_unmount_confirmed")):
                    raise RuntimeError("Actual instrumented anchor race proof or cleanup unconfirmed")
                report["bounded_floor_anchor"] = {"path": physical_path.name, "sha256": digest(physical_path)}
                run(["wsl", "-d", args.wsl_distro, "--user", "root", "--exec", "python3",
                     linux_root + "/tools/managed_runtime/staging_fixture.py", "--test-binary", test_binaries[0],
                     "--expected-sha256", hashlib.sha256(test_bytes).hexdigest(),
                     "--output", subprocess.check_output(prefix + ["wslpath", "-u", str(output)], text=True).strip()], "wsl-linux-actual-enospc")
                physical = json.loads((output / "enospc-report.json").read_bytes())
                if not all(physical.get(key) is True for key in ("actual_enospc_observed", "fixture_removed", "normal_unmount_confirmed")):
                    raise RuntimeError("Actual ENOSPC fixture or own cleanup unconfirmed")
                report["bounded_filesystem_enospc"] = {"path": "enospc-report.json", "sha256": digest(output / "enospc-report.json")}
                run(["wsl", "-d", args.wsl_distro, "--user", "root", "--exec", "python3",
                     linux_root + "/tools/managed_runtime/staging_fixture.py", "--test-binary", test_binaries[0],
                     "--expected-sha256", hashlib.sha256(test_bytes).hexdigest(), "--suite", "extraction",
                     "--package-binary", program, "--expected-package-sha256", hashlib.sha256(content).hexdigest(),
                     "--output", subprocess.check_output(prefix + ["wslpath", "-u", str(output)], text=True).strip()], "wsl-linux-actual-extraction")
                physical = json.loads((output / "archive-extraction-report.json").read_bytes())
                if not all(physical.get(key) is True for key in ("actual_root_extraction_observed", "actual_cli_extraction_observed", "fixture_removed", "normal_unmount_confirmed")):
                    raise RuntimeError("Actual archive extraction fixture or cleanup unconfirmed")
                report["bounded_archive_extraction"] = {"path": "archive-extraction-report.json", "sha256": digest(output / "archive-extraction-report.json")}
                run(["wsl", "-d", args.wsl_distro, "--user", "root", "--exec", "python3",
                     linux_root + "/tools/managed_runtime/staging_fixture.py", "--test-binary", test_binaries[0],
                     "--expected-sha256", hashlib.sha256(test_bytes).hexdigest(), "--suite", "custody",
                     "--package-binary", program, "--expected-package-sha256", hashlib.sha256(content).hexdigest(),
                     "--output", subprocess.check_output(prefix + ["wslpath", "-u", str(output)], text=True).strip()], "wsl-linux-actual-target-custody")
                physical = json.loads((output / "archive-custody-report.json").read_bytes())
                if not all(physical.get(key) is True for key in ("actual_original_target_custody_observed", "originals_pinned_after_unlink", "original_descriptors_closed_on_drop", "scoped_child_descriptor_exhaustion_rejected", "fixture_removed", "normal_unmount_confirmed")):
                    raise RuntimeError("Actual original target custody or cleanup unconfirmed")
                report["bounded_archive_target_custody"] = {"path": "archive-custody-report.json", "sha256": digest(output / "archive-custody-report.json")}
                run(["wsl", "-d", args.wsl_distro, "--user", "root", "--exec", "python3",
                     linux_root + "/tools/managed_runtime/staging_fixture.py", "--test-binary", test_binaries[0],
                     "--expected-sha256", hashlib.sha256(test_bytes).hexdigest(), "--suite", "deep",
                     "--package-binary", program, "--expected-package-sha256", hashlib.sha256(content).hexdigest(),
                     "--output", subprocess.check_output(prefix + ["wslpath", "-u", str(output)], text=True).strip()], "wsl-linux-actual-deep-budget")
                physical = json.loads((output / "archive-deep-budget-report.json").read_bytes())
                if not all(physical.get(key) is True for key in ("actual_deep_descriptor_budget_observed", "deep_budget_rejected_before_mutation", "fixture_removed", "normal_unmount_confirmed")):
                    raise RuntimeError("Actual deep descriptor admission or cleanup unconfirmed")
                report["bounded_archive_deep_descriptor_budget"] = {"path": "archive-deep-budget-report.json", "sha256": digest(output / "archive-deep-budget-report.json")}
                for suite, label, marker in (
                    ("installation", "nine-role-installation", "all_nine_roles_prepared_with_original_custody"),
                    ("installation-failure", "nine-role-storage-failure", "last_role_enospc_before_any_payload"),
                    ("publication", "atomic-publication", "actual_atomic_publication_observed"),
                    ("journal", "publication-journal", "actual_durable_journal_observed"),
                    ("journal-status", "publication-journal-status", "actual_journal_status_observed"),
                    ("release-floor", "runtime-release-floor", "actual_restrictive_release_floor_observed"),
                    ("installed-capability", "installed-package-current", "actual_current_installed_bytes_observed"),
                ):
                    run(["wsl", "-d", args.wsl_distro, "--user", "root", "--exec", "python3",
                         linux_root + "/tools/managed_runtime/staging_fixture.py", "--test-binary", test_binaries[0],
                         "--expected-sha256", hashlib.sha256(test_bytes).hexdigest(), "--suite", suite,
                         "--package-binary", program, "--expected-package-sha256", hashlib.sha256(content).hexdigest(),
                         "--output", subprocess.check_output(prefix + ["wslpath", "-u", str(output)], text=True).strip()], "wsl-linux-actual-" + suite)
                    physical = json.loads((output / (label + "-report.json")).read_bytes())
                    if not all(physical.get(key) is True for key in (marker, "fixture_removed", "normal_unmount_confirmed")):
                        raise RuntimeError("Actual nine-role fixture or cleanup unconfirmed: " + suite)
                    if suite == "installation" and physical.get("aggregate_descriptors_admitted_before_role_roots") is not True:
                        raise RuntimeError("Aggregate descriptor admission before role roots unconfirmed")
                    report["bounded_" + suite.replace("-", "_")] = {"path": label + "-report.json", "sha256": digest(output / (label + "-report.json"))}
                run(["wsl", "-d", args.wsl_distro, "--user", "root", "--exec", "python3",
                     linux_root + "/tools/managed_runtime/floor_namespace_fixture.py", "--package-binary", program,
                     "--package-sha256", hashlib.sha256(content).hexdigest(),
                     "--output", subprocess.check_output(prefix + ["wslpath", "-u", str(output)], text=True).strip()], "wsl-linux-actual-floor-namespace")
                physical = json.loads((output / "release-floor-namespace-report.json").read_bytes())
                if not all(physical.get(key) is True for key in ("namespace_swap_rejected", "status_read_unchanged", "original_policies_restored_and_verified", "normal_unmount_confirmed", "fixture_removed")):
                    raise RuntimeError("Actual same-parent floor namespace rejection or own cleanup unconfirmed")
                report["bounded_floor_namespace"] = {"path": "release-floor-namespace-report.json", "sha256": digest(output / "release-floor-namespace-report.json")}
                run(["wsl", "-d", args.wsl_distro, "--user", "root", "--exec", "python3",
                     linux_root + "/tools/managed_runtime/journal_fixture.py", "--test-binary", test_binaries[0],
                     "--test-sha256", hashlib.sha256(test_bytes).hexdigest(),
                     "--package-binary", program, "--package-sha256", hashlib.sha256(content).hexdigest(),
                     "--output", subprocess.check_output(prefix + ["wslpath", "-u", str(output)], text=True).strip()], "wsl-linux-actual-journal-ext4")
                physical = json.loads((output / "publication-journal-ext4-report.json").read_bytes())
                if not all(physical.get(key) is True for key in ("all_eight_process_fault_cases_confirmed", "normal_remount_history_confirmed", "normal_unmount_confirmed", "loop_detached_confirmed", "fixture_removed")):
                    raise RuntimeError("Actual ext4 journal process-loss proof or own cleanup unconfirmed")
                report["bounded_journal_ext4"] = {"path": "publication-journal-ext4-report.json", "sha256": digest(output / "publication-journal-ext4-report.json")}
                run(["wsl", "-d", args.wsl_distro, "--user", "root", "--exec", "python3",
                     linux_root + "/tools/managed_runtime/journal_fixture.py", "--release-floor", "--test-binary", test_binaries[0],
                     "--test-sha256", hashlib.sha256(test_bytes).hexdigest(),
                     "--package-binary", program, "--package-sha256", hashlib.sha256(content).hexdigest(),
                     "--output", subprocess.check_output(prefix + ["wslpath", "-u", str(output)], text=True).strip()], "wsl-linux-actual-release-floor-ext4")
                physical = json.loads((output / "release-floor-ext4-report.json").read_bytes())
                if not all(physical.get(key) is True for key in ("all_eight_release_floor_fault_cases_confirmed", "normal_remount_policy_confirmed", "normal_unmount_confirmed", "loop_detached_confirmed", "fixture_removed")):
                    raise RuntimeError("Actual restrictive floor process-loss proof or own cleanup unconfirmed")
                report["bounded_release_floor_ext4"] = {"path": "release-floor-ext4-report.json", "sha256": digest(output / "release-floor-ext4-report.json")}

        drift = [name for name, sha in sources.items() if digest(root / name) != sha]
        report["source_drift"] = drift
        if drift:
            raise RuntimeError("Package verifier sources changed during qualification")
        archive = output / "review-source-inputs.zip"
        with zipfile.ZipFile(archive, "x", zipfile.ZIP_DEFLATED) as bundle:
            for name, sha in sources.items():
                content = (root / name).read_bytes()
                if hashlib.sha256(content).hexdigest() != sha:
                    raise RuntimeError("Source changed during archival")
                bundle.writestr(name, content)
        with zipfile.ZipFile(archive) as bundle:
            if bundle.testzip() is not None or any(hashlib.sha256(bundle.read(n)).hexdigest() != h for n, h in sources.items()):
                raise RuntimeError("Source archive verification failed")
        report["source_archive"] = {"path": archive.name, "sha256": digest(archive), "requires_full_checkout": True}
        report["source_manifest_sha256"] = digest(output / "sources.json")
        report["source_count"] = len(sources)
        report["result"] = "passed"
    except Exception as error:
        report.update(result="failed", failure=str(error))
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"report": str(output / "report.json"), "result": report["result"]}), flush=True)
    return int(report["result"] != "passed")


if __name__ == "__main__":
    raise SystemExit(main())
