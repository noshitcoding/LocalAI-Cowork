"""Independent preview identity policy and exact GitHub artifact transport."""
from __future__ import annotations

import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import stat
import subprocess
import zipfile

from release_checks import ordinary, pin, preview_endpoint, read

MAX_ARCHIVE = 2 * 1024**3
MAX_FILES = 16384


def policy(path: Path, repository: str) -> dict:
    value = read(path)
    if (value.get("format") != "localai-cowork-preview-trust/1" or value.get("enabled") is not True
            or value.get("repository") != repository
            or not re.fullmatch(r"[0-9a-f]{64}", value.get("updater_key_sha256") or "")
            or value["updater_key_sha256"] == "0" * 64
            or not re.fullmatch(r"[A-F0-9]{40}", value.get("windows_certificate_thumbprint") or "")
            or value["windows_certificate_thumbprint"] == "0" * 40
            or not re.fullmatch(r"\.github/workflows/[A-Za-z0-9_.-]+\.ya?ml", value.get("candidate_workflow") or "")):
        raise ValueError("Configure the independently reviewed preview identity/producer policy first")
    preview_endpoint(value.get("updater_endpoint", ""))
    return value


def identity(receipt: dict, trusted: dict) -> None:
    profile = receipt.get("signing_profile") or {}
    key = profile.get("updater_public_key", "")
    if (hashlib.sha256(key.encode()).hexdigest() != trusted["updater_key_sha256"]
            or profile.get("updater_endpoint") != trusted["updater_endpoint"]):
        raise ValueError("Candidate updater identity differs from the independent release policy")
    if receipt.get("host", "").endswith("msvc") and profile.get("windows_certificate_thumbprint", "").upper() != trusted["windows_certificate_thumbprint"]:
        raise ValueError("Candidate Authenticode identity differs from the independent release policy")


def checked_producer(run: dict, artifact: dict, trusted: dict, commit: str, run_id: int, artifact_id: int) -> dict:
    if (run.get("id") != run_id or run.get("head_sha") != commit or run.get("conclusion") != "success"
            or run.get("status") != "completed" or run.get("path") != trusted["candidate_workflow"]
            or run.get("repository", {}).get("full_name") != trusted["repository"]
            or artifact.get("id") != artifact_id or artifact.get("expired") is not False
            or artifact.get("workflow_run", {}).get("id") != run_id
            or artifact.get("workflow_run", {}).get("head_sha") != commit
            or not re.fullmatch(r"sha256:[0-9a-f]{64}", artifact.get("digest") or "")):
        raise ValueError("Candidate must come from the trusted producer, exact source commit and immutable artifact digest")
    return {"run_id": run_id, "artifact_id": artifact_id, "artifact_digest": artifact["digest"],
            "artifact_name": artifact["name"], "producer_workflow": run["path"],
            "head_sha": commit, "producer_url": run["html_url"]}


def live_producer(root: Path, trusted: dict, commit: str, run_id: int, artifact_id: int) -> dict:
    repository = trusted["repository"]
    def api(path):
        return json.loads(subprocess.check_output(["gh", "api", f"repos/{repository}/{path}"], cwd=root, text=True))
    return checked_producer(api(f"actions/runs/{run_id}"), api(f"actions/artifacts/{artifact_id}"),
                            trusted, commit, run_id, artifact_id)


def archive_files(path: Path) -> dict:
    ordinary(path)
    if path.stat().st_size > MAX_ARCHIVE:
        raise ValueError("Candidate archive exceeds its bound")
    records = {}
    total = 0
    with zipfile.ZipFile(path) as archive:
        members = archive.infolist()
        if len(members) > MAX_FILES:
            raise ValueError("Too many candidate archive entries")
        seen = set()
        for item in members:
            name = item.filename.rstrip("/") if item.is_dir() else item.filename
            relative = PurePosixPath(name)
            mode = stat.S_IFMT(item.external_attr >> 16)
            if (not name or name in seen or relative.is_absolute() or relative.as_posix() != name
                    or any(part in {".", ".."} for part in relative.parts)
                    or any(char in name for char in "\\:\r\n")
                    or mode not in {0, stat.S_IFREG, stat.S_IFDIR} or item.flag_bits & 1):
                raise ValueError("Unsafe or duplicate candidate archive member")
            seen.add(name)
            if item.is_dir():
                continue
            total += item.file_size
            if total > MAX_ARCHIVE:
                raise ValueError("Uncompressed candidate exceeds its bound")
            digest = hashlib.sha256()
            with archive.open(item) as stream:
                for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                    digest.update(chunk)
            records[name] = {"size": item.file_size, "sha256": digest.hexdigest()}
    if not records:
        raise ValueError("An original populated candidate archive is required")
    return records


def checked_archive(archive: Path, candidate: Path, producer: dict) -> dict:
    expected = producer["artifact_digest"].removeprefix("sha256:")
    if pin(archive)["sha256"] != expected:
        raise ValueError("Candidate ZIP differs from the live immutable GitHub artifact digest")
    records = archive_files(archive)
    actual = {path.relative_to(candidate).as_posix(): pin(path)
              for path in ordinary(candidate, directory=True).rglob("*") if path.is_file()}
    if records != actual:
        raise ValueError("Extracted candidate differs from the digest-bound original artifact ZIP")
    if pin(archive)["sha256"] != expected:
        raise ValueError("Original candidate archive changed during inspection")
    return records


def fetch(root: Path, trusted: dict, commit: str, run_id: int, artifact_id: int,
          archive_path: Path, output: Path) -> dict:
    if (not output.is_absolute() or output.exists() or output.is_symlink()
            or not archive_path.is_absolute() or archive_path.exists() or archive_path.is_symlink()):
        raise ValueError("Select new absolute candidate archive and extraction outputs")
    ordinary(output.parent, directory=True)
    ordinary(archive_path.parent, directory=True)
    producer = live_producer(root, trusted, commit, run_id, artifact_id)
    with archive_path.open("xb") as target:
        subprocess.run(["gh", "api", f"repos/{trusted['repository']}/actions/artifacts/{artifact_id}/zip"],
                       cwd=root, stdout=target, check=True)
    if pin(archive_path)["sha256"] != producer["artifact_digest"].removeprefix("sha256:"):
        raise ValueError("Downloaded candidate ZIP does not match GitHub's immutable digest")
    records = archive_files(archive_path)
    output.mkdir()
    with zipfile.ZipFile(archive_path) as archive:
        for name, record in records.items():
            destination = output / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            with destination.open("xb") as target, archive.open(name) as source:
                while block := source.read(1024 * 1024):
                    target.write(block)
            if pin(destination) != record:
                raise ValueError("Candidate archive changed during extraction")
    checked_archive(archive_path, output, producer)
    return producer
