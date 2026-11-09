"""Freeze V2 preview provenance or stage a checked draft release. Never uploads.

The candidate is an explicit ordinary directory. GitHub verification is queried
read-only with gh; signed packages and installed acceptance remain prerequisites.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import importlib.util
import json
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import tomllib

from release_checks import HOSTS, checked_package, filename, ordinary, pin, preview_version, read
import update_feed
import preview_trust

ROOT = Path(__file__).resolve().parents[2]
_spec = importlib.util.spec_from_file_location("preview_native_release", ROOT / "tools/desktop/release.py")
native_release = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(native_release)
ACCEPTANCE_CHECKS = {"installed_start", "first_setup", "model_connection", "chat_tool_approval",
                     "file_import_download", "workflow", "cancel", "restart_recovery",
                     "backup_restore", "fresh_profile", "v1_profile_isolation"}


def canonical(value) -> str:
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def command(root: Path, args: list[str]) -> str:
    return subprocess.check_output(args, cwd=root, text=True, encoding="utf-8").strip()


def source_snapshot(root: Path, tag: str) -> dict:
    root = ordinary(root, directory=True)
    if command(root, ["git", "status", "--porcelain", "--untracked-files=normal"]):
        raise ValueError("Commit the complete reviewed source before freezing a preview")
    commit = command(root, ["git", "rev-parse", "HEAD"])
    version = preview_version(read(root / "package.json")["version"])
    if tag != "v" + version or command(root, ["git", "rev-parse", "refs/tags/" + tag + "^{commit}"]) != commit:
        raise ValueError("The exact existing preview tag must point at the clean source commit")
    cargo = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    desktop = read(root / "apps/desktop/src-tauri/tauri.conf.json")
    if cargo["workspace"]["package"]["version"] != version or desktop["version"] != version:
        raise ValueError("Rust, npm and desktop release versions differ")
    files = {}
    for line in command(root, ["git", "ls-files", "--stage", "-z"]).split("\0"):
        if not line:
            continue
        metadata, name = line.split("\t", 1)
        if metadata.split()[0] not in {"100644", "100755"}:
            raise ValueError("Only ordinary tracked source files can enter preview provenance")
        if ("\\" in name or any(char in name for char in "\r\n:")
                or PurePosixPath(name).is_absolute() or ".." in PurePosixPath(name).parts):
            raise ValueError("Nonportable Git filenames need a separately reviewed source bundle")
        files[name] = pin(root / name)
    if not files:
        raise ValueError("Tracked source inputs are required")
    return {"format": "localai-cowork-preview-source/1", "version": version, "tag": tag,
            "commit": commit, "files": files, "source_sha256": canonical(files)}


def selected(candidate: Path, relative: str, *, directory: bool = False) -> Path:
    if (not isinstance(relative, str) or not relative or "\\" in relative or ":" in relative
            or any(char in relative for char in "\r\n")):
        raise ValueError("Select an ordinary path inside the candidate directory")
    value = PurePosixPath(relative)
    if value.is_absolute() or any(part in {".", ".."} for part in value.parts) or value.as_posix() != relative:
        raise ValueError("Candidate paths must stay inside their explicit root")
    return ordinary(candidate / relative, directory=directory)


def checked_ci(run: dict, jobs: list[dict], commit: str, repository: str) -> dict:
    if (run.get("head_sha") != commit or run.get("conclusion") != "success"
            or run.get("status") != "completed" or run.get("path") != ".github/workflows/verify.yml"
            or run.get("repository", {}).get("full_name") != repository
            or not jobs or any(job.get("conclusion") != "success" for job in jobs)):
        raise ValueError("A complete successful verify.yml run on the exact preview commit is required")
    names = {job.get("name") for job in jobs}
    for group in ("core", "desktop", "release-tools", "documents", "office", "document-ui"):
        for host in ("windows-latest", "ubuntu-24.04"):
            if f"{group} ({host})" not in names:
                raise ValueError("CI is missing a required native verification job")
    return {key: run[key] for key in ("id", "html_url", "head_sha", "conclusion", "path")}


def live_ci(root: Path, repository: str, run_id: int, commit: str) -> dict:
    run = json.loads(command(root, ["gh", "api", f"repos/{repository}/actions/runs/{run_id}"]))
    pages = json.loads(command(root, ["gh", "api", "--paginate", "--slurp",
                                    f"repos/{repository}/actions/runs/{run_id}/jobs?per_page=100&filter=latest"]))
    return checked_ci(run, [job for page in pages for job in page["jobs"]], commit, repository)


def checked_acceptance(candidate: Path, value: dict, source: dict, packages: dict) -> dict:
    if (value.get("format") != "localai-cowork-preview-installed-acceptance/1"
            or any(value.get(key) != source[key] for key in ("version", "commit", "source_sha256"))):
        raise ValueError("Installed acceptance must bind the exact current source")
    platforms = value.get("platforms", {})
    if set(platforms) != set(packages):
        raise ValueError("Installed acceptance is required for every offered platform")
    for host, package in packages.items():
        accepted = platforms[host]
        if accepted.get("receipt_sha256") != pin(package["folder"] / "receipt.json")["sha256"]:
            raise ValueError("Installed acceptance is bound to a different package receipt")
        expected = {kind: record["sha256"] for kind, record in package["receipt"]["bundles"].items()}
        if accepted.get("packages") != expected or set(accepted.get("checks", {})) != ACCEPTANCE_CHECKS:
            raise ValueError("Installed acceptance must cover the exact packages and required product paths")
        for record in accepted["checks"].values():
            if record.get("result") != "passed":
                raise ValueError("A required installed product path is not accepted")
            log = selected(candidate, record["evidence"])
            if pin(log)["sha256"] != record.get("sha256") or not log.stat().st_size:
                raise ValueError("Original installed acceptance evidence changed or is empty")
    return {"format": value["format"], "evidence_sha256": canonical(value),
            "platforms": {host: sorted(ACCEPTANCE_CHECKS) for host in platforms}}


def stage(root: Path, candidate: Path, output: Path, tag: str, repository: str, run_id: int,
          policy_path: Path, candidate_run_id: int, artifact_id: int, archive_path: Path) -> dict:
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository):
        raise ValueError("Select the explicit GitHub owner/repository")
    if not output.is_absolute() or output.exists() or output.is_symlink():
        raise ValueError("Select a NEW absolute staging directory")
    ordinary(output.parent, directory=True)
    candidate = ordinary(candidate, directory=True)
    trusted = preview_trust.policy(policy_path, repository)
    policy_pin = pin(policy_path)
    source = source_snapshot(root, tag)
    producer = preview_trust.live_producer(root, trusted, source["commit"], candidate_run_id, artifact_id)
    candidate_inputs = preview_trust.checked_archive(archive_path, candidate, producer)
    saved_source = read(candidate / "source-provenance.json")
    if source != saved_source:
        raise ValueError("Candidate provenance differs from the current tagged source")
    manifest = read(candidate / "candidate.json")
    if manifest.get("format") != "localai-cowork-preview-candidate/1":
        raise ValueError("Unsupported preview candidate")
    # The exact CI run is fetched from GitHub, never inferred from a local status file.
    ci = live_ci(root, repository, run_id, source["commit"])
    remote_commit = json.loads(command(root, ["gh", "api", f"repos/{repository}/commits/{tag}"]))
    if remote_commit.get("sha") != source["commit"]:
        raise ValueError("The public preview tag differs from the source accepted by CI")
    frontend_path = selected(candidate, manifest["frontend_receipt"])
    front = read(frontend_path)
    if (front.get("format") != "localai-cowork-frontend-build/1" or front.get("exit_code") != 0
            or front.get("sources_unchanged") is not True or front.get("version") != source["version"]
            or not front.get("sources") or not front.get("outputs")):
        raise ValueError("An original successful frontend build receipt is required")
    for name, original in front["sources"].items():
        if source["files"].get(name) != original:
            raise ValueError("Frontend inputs differ from the tagged source")
    expected_front = {name for name in source["files"]
                      if name in {"package.json", "package-lock.json", "tsconfig.json", "tools/desktop/frontend.py"}
                      or name.startswith(("apps/web/", "packages/", "contracts/"))}
    if set(front["sources"]) != expected_front:
        raise ValueError("The frontend receipt does not cover the complete tracked frontend")
    if set(manifest.get("packages", {})) != set(HOSTS):
        raise ValueError("The advertised Windows and Linux packages are both required")
    packages = {}
    for host, relative in manifest["packages"].items():
        folder = selected(candidate, relative, directory=True)
        receipt, paths = checked_package(folder, source["version"], strict_preview=True, trusted_policy=trusted)
        if receipt["host"] != host:
            raise ValueError("Candidate platform and original native package differ")
        sources = receipt.get("sources", {})
        if not sources or any(source["files"].get(name) != record
                              for name, record in sources.items() if not name.startswith("apps/web/dist/")):
            raise ValueError("Native package inputs differ from the tagged source")
        if any(sources.get(name) != record for name, record in native_release.source_manifest(root).items()):
            raise ValueError("The native receipt does not cover the complete current core")
        required = {name for name in source["files"] if name.startswith("apps/desktop/src-tauri/")
                    or name in {"package.json", "package-lock.json", "tools/desktop/package.py",
                                "tools/desktop/signing.py", "tools/desktop/test_package.py", "packages/ui/src/secretMessages.json"}
                    or name.startswith("tools/toolbox/") and Path(name).suffix in {".py", ".ps1", ".lock", ".json", ".md"}
                    or name.startswith("crates/") and name.endswith(".sqlite3")}
        if any(sources.get(name) != source["files"][name] for name in required):
            raise ValueError("The native receipt does not cover the complete tracked desktop/package inputs")
        if {name: record for name, record in sources.items() if name.startswith("apps/web/dist/")} != front["outputs"]:
            raise ValueError("Packaged frontend outputs differ from the original frontend build")
        packages[host] = {"folder": folder, "receipt": receipt, "paths": paths}
    acceptance = checked_acceptance(candidate, read(selected(candidate, manifest["acceptance"])), source, packages)
    notes_path = selected(candidate, manifest["notes"])
    notes = notes_path.read_text(encoding="utf-8")
    if not notes.strip() or source["version"] not in notes:
        raise ValueError("Preview notes must identify this exact version and its limits")
    feed = update_feed.generate([value["folder"] for value in packages.values()], source["version"],
                                f"https://github.com/{repository}/releases/download/{tag}", notes, trusted_policy=trusted)
    if preview_trust.checked_archive(archive_path, candidate, producer) != candidate_inputs or pin(policy_path) != policy_pin:
        raise ValueError("Candidate or trusted policy changed before staging")
    # No stage is created until every publication prerequisite has passed.
    output.mkdir()
    assets = {}
    def copy(original: Path, target_name: str):
        filename(target_name)
        if target_name in assets:
            raise ValueError("Duplicate public release asset name")
        expected = candidate_inputs[original.relative_to(candidate).as_posix()]
        if pin(original) != expected:
            raise ValueError("Original candidate changed before release staging")
        target = output / target_name
        shutil.copyfile(original, target)
        if pin(original) != expected or pin(target) != expected:
            raise ValueError("Original candidate changed during release staging")
        assets[target_name] = expected
    for host, package in packages.items():
        for kind, path in package["paths"].items():
            copy(path, HOSTS[host][0] + "-sbom.spdx.json" if kind == "sbom" else path.name)
    for name, value in {"source-provenance.json": source, "preview.json": feed,
                        "release-provenance.json": {"format": "localai-cowork-preview-provenance/1",
                            "version": source["version"], "tag": tag, "commit": source["commit"],
                            "source_sha256": source["source_sha256"], "ci": ci, "acceptance": acceptance,
                            "producer": producer, "trusted_policy_sha256": policy_pin["sha256"],
                            "signed_packages": True, "stable_release_qualified": False,
                            "created_at": datetime.now(timezone.utc).isoformat()}}.items():
        (output / name).write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        assets[name] = pin(output / name)
    copy(notes_path, "RELEASE-NOTES.md")
    (output / "SHA256SUMS").write_text("".join(record["sha256"] + "  " + name + "\n"
                                               for name, record in sorted(assets.items())), encoding="utf-8")
    assets["SHA256SUMS"] = pin(output / "SHA256SUMS")
    if source_snapshot(root, tag) != source:
        raise ValueError("Tagged source changed while staging; partial evidence retained")
    if preview_trust.checked_archive(archive_path, candidate, producer) != candidate_inputs or pin(policy_path) != policy_pin:
        raise ValueError("Candidate or trusted policy changed during staging; partial evidence retained")
    plan = {"format": "localai-cowork-preview-draft-plan/1", "repository": repository,
            "tag": tag, "commit": source["commit"], "version": source["version"],
            "draft": True, "prerelease": True, "make_latest": False,
            "assets": assets, "published": False, "stable_feed_modified": False}
    (output / "release-plan.json").write_text(json.dumps(plan, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return plan


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="action", required=True)
    freeze = sub.add_parser("snapshot")
    freeze.add_argument("--tag", required=True)
    freeze.add_argument("--output", type=Path, required=True)
    freeze.add_argument("--source-root", type=Path, default=ROOT)
    download = sub.add_parser("fetch")
    for name in ("tag", "repository"):
        download.add_argument("--" + name, required=True)
    download.add_argument("--trusted-policy", type=Path, required=True)
    download.add_argument("--candidate-run-id", type=int, required=True)
    download.add_argument("--candidate-artifact-id", type=int, required=True)
    download.add_argument("--archive", type=Path, required=True)
    download.add_argument("--output", type=Path, required=True)
    download.add_argument("--source-root", type=Path, default=ROOT)
    prepare = sub.add_parser("stage")
    prepare.add_argument("--tag", required=True)
    prepare.add_argument("--candidate", type=Path, required=True)
    prepare.add_argument("--output", type=Path, required=True)
    prepare.add_argument("--repository", required=True)
    prepare.add_argument("--ci-run-id", type=int, required=True)
    prepare.add_argument("--trusted-policy", type=Path, required=True)
    prepare.add_argument("--candidate-run-id", type=int, required=True)
    prepare.add_argument("--candidate-artifact-id", type=int, required=True)
    prepare.add_argument("--archive", type=Path, required=True)
    prepare.add_argument("--source-root", type=Path, default=ROOT)
    args = parser.parse_args()
    if args.action == "snapshot":
        value = source_snapshot(args.source_root, args.tag)
        if not args.output.is_absolute():
            raise ValueError("Select an absolute new provenance output")
        with args.output.open("x", encoding="utf-8", newline="\n") as stream:
            stream.write(json.dumps(value, indent=2, sort_keys=True) + "\n")
        print(json.dumps({key: value[key] for key in ("version", "tag", "commit", "source_sha256")}))
    elif args.action == "fetch":
        source = source_snapshot(args.source_root, args.tag)
        trusted = preview_trust.policy(args.trusted_policy, args.repository)
        print(json.dumps(preview_trust.fetch(args.source_root, trusted, source["commit"], args.candidate_run_id,
                         args.candidate_artifact_id, args.archive, args.output), indent=2))
    else:
        print(json.dumps(stage(args.source_root, args.candidate, args.output, args.tag, args.repository, args.ci_run_id,
                         args.trusted_policy, args.candidate_run_id, args.candidate_artifact_id, args.archive), indent=2))


if __name__ == "__main__":
    main()
