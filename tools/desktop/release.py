"""Build one native release sidecar and publish its immutable preparation receipt.

This prepares bytes for a later installer build. It neither installs nor signs
them, and does not turn source integrity into a release qualification.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile
from datetime import datetime, timezone

SUPPORTED_HOSTS = {"x86_64-pc-windows-msvc", "x86_64-unknown-linux-gnu"}
SOURCE_SUFFIXES = {".rs", ".toml", ".sql", ".json", ".js", ".cjs", ".mjs", ".py", ".ps1"}
# These are compiler inputs and the fixed synthetic verifier test corpus. Their
# complete bytes matter, including SQLite, JSONL and extensionless manifests.
HISTORICAL_DATA_DIRECTORIES = (
    "crates/storage/src/historical_imports/verifier/data",
    "crates/storage/tests/fixtures/v1_history_verifier",
    "tools/migration/reference_source",
    "tools/migration/reference_source64",
    "tools/migration/reference_source66",
    "tools/migration/reference_source67",
    "tools/migration/reference_source71",
    "tools/migration/reference_source75",
)
MAX_BINARY = 512 * 1024 * 1024
ROOT = Path(__file__).resolve().parents[2]


class Refused(ValueError):
    """Preparation cannot establish the requested artifact binding."""


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def regular(path: Path, *, cargo_artifact: bool = False) -> Path:
    # A sidecar receipt describes actual ordinary files, never a followed link.
    path = Path(os.path.abspath(path))
    for part in [*reversed(path.parents), path]:
        info = part.lstat()
        if part.is_symlink() or getattr(info, "st_file_attributes", 0) & 0x400:
            raise Refused("reparse points and symlinks are not artifact inputs")
    if not path.is_file() or (not cargo_artifact and path.stat().st_nlink != 1):
        raise Refused("a regular artifact without hardlinks is required")
    return path


def source_manifest(root: Path) -> dict:
    paths = [root / name for name in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml")]
    for directory in ("crates", "services", "tools/contracts"):
        paths.extend(p for p in (root / directory).rglob("*")
                     if p.is_file() and p.suffix in SOURCE_SUFFIXES
                     and not {"target", "node_modules", ".git"}.intersection(p.relative_to(root).parts))
    paths.extend((root / "contracts").glob("*.json"))
    paths.append(root / "extensions/browser/identity.json")
    for directory in HISTORICAL_DATA_DIRECTORIES:
        paths.extend(p for p in (root / directory).rglob("*") if p.is_file())
    if (root / ".cargo").exists():
        paths.extend((root / ".cargo").glob("*.toml"))
    paths.extend([root / "tools/desktop/release.py"])
    paths.extend((root / "tools/release").glob("*.py"))
    paths.append(root / "tools/private_crew_setup.py")
    paths.extend((root / "tools/device_agent").glob("*.py"))
    paths.append(root / "docs/DEVICE-AGENT-V2.md")
    # These shipped runner bridges and image recipes are part of the original
    # daemon/agent release, rather than unbound operator-side conveniences.
    paths.extend(path for path in (root / "deploy/v2").iterdir()
                 if path.is_file() and (path.suffix in {".py", ".yml", ".yaml", ".json", ".dockerignore"}
                                       or path.name.startswith("Dockerfile")
                                       or path.name in {".env.example", "Caddyfile", "README.md"}))
    # Bind the complete shipped historical CLI closure, including the lazy native
    # Linux boundary and Stage 3. Old receipts retain their original scope.
    paths.extend((root / "tools/migration").glob("*.py"))
    paths.extend((root / "tools/migration").glob("current-target-*.json"))
    records = {}
    for path in sorted(set(paths)):
        regular(path)
        records[path.relative_to(root).as_posix()] = {"size": path.stat().st_size, "sha256": sha256(path)}
    return records


def native_host(version: str) -> str:
    hosts = [line[6:] for line in version.splitlines() if line.startswith("host: ")]
    if len(hosts) != 1 or hosts[0] not in SUPPORTED_HOSTS:
        raise Refused("one supported Windows/Linux native x64 Rust host is required")
    if (os.name == "nt") != (hosts[0] == "x86_64-pc-windows-msvc"):
        raise Refused("cross-platform sidecars are not native preparation")
    return hosts[0]


def select_artifact(messages: str, package_id: str, host: str, target_dir: Path) -> Path:
    candidates = []
    finished = []
    for line in messages.splitlines():
        if not line.startswith("{"):
            continue
        try:
            item = json.loads(line)
        except json.JSONDecodeError as error:
            raise Refused("malformed Cargo JSON") from error
        if item.get("reason") == "build-finished":
            finished.append(item.get("success"))
        if (item.get("reason") == "compiler-artifact"
                and item.get("package_id") == package_id
                and item.get("target", {}).get("name") == "cowork-daemon"
                and item.get("target", {}).get("kind") == ["bin"]
                and item.get("executable")):
            profile = item.get("profile", {})
            if (profile.get("opt_level") != "3" or profile.get("debug_assertions") is not False
                    or profile.get("overflow_checks") is not False or profile.get("test") is not False):
                raise Refused("the daemon artifact is not the expected optimized release profile")
            candidates.append(Path(item["executable"]))
    if finished != [True] or len(candidates) != 1:
        raise Refused("one successful Cargo build and exactly one daemon artifact are required")
    expected = Path(os.path.abspath(target_dir)) / host / "release" / ("cowork-daemon.exe" if os.name == "nt" else "cowork-daemon")
    # Cargo normally hardlinks release/cowork-daemon to release/deps/....
    # Accept that reported build artifact, then publish an independent copy.
    artifact = regular(candidates[0], cargo_artifact=True)
    if artifact != expected:
        raise Refused("Cargo reported an executable outside the exact native release directory")
    if not 0 < artifact.stat().st_size <= MAX_BINARY:
        raise Refused("release artifact size is outside the preparation bound")
    validate_executable(artifact, host)
    return artifact


def validate_executable(path: Path, host: str) -> None:
    with path.open("rb") as stream:
        header = stream.read(64)
        if host == "x86_64-pc-windows-msvc":
            if len(header) != 64 or header[:2] != b"MZ":
                raise Refused("the Windows artifact is not PE")
            offset = struct.unpack_from("<I", header, 60)[0]
            if offset < 64 or offset > min(path.stat().st_size - 26, 1024 * 1024):
                raise Refused("invalid PE header offset")
            stream.seek(offset)
            pe = stream.read(26)
            if (pe[:4] != b"PE\0\0" or struct.unpack_from("<H", pe, 4)[0] != 0x8664
                    or not (struct.unpack_from("<H", pe, 22)[0] & 2)
                    or struct.unpack_from("<H", pe, 22)[0] & 0x2000
                    or struct.unpack_from("<H", pe, 24)[0] != 0x20B):
                raise Refused("the Windows artifact is not an x64 PE32+ executable")
        elif host == "x86_64-unknown-linux-gnu":
            if (len(header) != 64 or header[:6] != b"\x7fELF\x02\x01"
                    or struct.unpack_from("<H", header, 18)[0] != 62
                    or struct.unpack_from("<H", header, 16)[0] not in (2, 3)):
                raise Refused("the Linux artifact is not an x64 ELF executable")
        else:
            raise Refused("unsupported native executable")


def prepare(root: Path, output: Path, target_dir: Path, offline: bool = False) -> dict:
    output = Path(os.path.abspath(output))
    target_dir = Path(os.path.abspath(target_dir))
    if output.exists() or output.is_symlink():
        raise Refused("the immutable output directory must not already exist")
    if not output.parent.is_dir():
        raise Refused("the output parent must already exist")
    for name in ("RUSTFLAGS", "RUSTDOCFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_ENCODED_RUSTDOCFLAGS",
                 "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "RUSTC"):
        if os.environ.get(name):
            raise Refused("build flags or compiler wrappers need a separate reviewed build profile")
    if any(name.startswith("CARGO_PROFILE_RELEASE_") for name in os.environ):
        raise Refused("environment release-profile overrides are not supported")
    before = source_manifest(root)
    rustc = subprocess.check_output(["rustc", "-vV"], cwd=root, text=True)
    host = native_host(rustc)
    cargo = subprocess.check_output(["cargo", "--version"], cwd=root, text=True).strip()
    metadata_args = ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"]
    if offline:
        metadata_args.append("--offline")
    metadata = json.loads(subprocess.check_output(metadata_args, cwd=root, text=True))
    daemon = [p for p in metadata["packages"] if p["name"] == "cowork-daemon"
              and Path(p["manifest_path"]) == root / "services/daemon/Cargo.toml"]
    if len(daemon) != 1:
        raise Refused("the exact workspace daemon package must be unique")
    command = ["cargo", "build", "--locked", "--release", "-p", "cowork-daemon", "--bin",
               "cowork-daemon", "--target", host, "--target-dir", str(target_dir),
               "--message-format=json-diagnostic-short"]
    if offline:
        command.append("--offline")
    # Cargo's exit, JSON artifact and release profile must all agree. Never infer
    # a successful build from an executable left in a previous target directory.
    build = subprocess.run(command, cwd=root, stdout=subprocess.PIPE, text=True, check=True)
    artifact = select_artifact(build.stdout, daemon[0]["id"], host, target_dir)
    if source_manifest(root) != before:
        raise Refused("source inputs changed during release preparation")
    original_hash = sha256(artifact)
    filename = "cowork-daemon-" + host + (".exe" if os.name == "nt" else "")
    # A fresh sibling directory is published only after its bytes and receipt
    # agree. A failed build never replaces an earlier prepared sidecar.
    temporary = Path(tempfile.mkdtemp(prefix=".sidecar-preparing-", dir=output.parent))
    try:
        staged = temporary / filename
        shutil.copyfile(artifact, staged)
        shutil.copymode(artifact, staged)
        if sha256(staged) != original_hash or sha256(artifact) != original_hash:
            raise Refused("the Cargo artifact changed while staging")
        validate_executable(staged, host)
        if source_manifest(root) != before:
            raise Refused("source inputs changed while staging")
        receipt = {"format": "localai-cowork-native-release-sidecar/1",
                   "prepared_at": datetime.now(timezone.utc).isoformat(),
                   "host": host, "profile": "release", "package_id": daemon[0]["id"],
                   "package_version": daemon[0]["version"], "rustc": rustc, "cargo": cargo,
                   "command": command, "cargo_exit_code": build.returncode,
                   "cargo_reported_artifact": str(artifact),
                   "cargo_artifact_hardlink_count": artifact.stat().st_nlink,
                   "artifact": {"filename": filename, "size": staged.stat().st_size,
                                "sha256": original_hash},
                   "sources": before, "source_before_after_equal": True,
                   "cargo_json_sha256": hashlib.sha256(build.stdout.encode("utf-8")).hexdigest(),
                   "native_process_tree_qualified": False, "installer_built": False,
                   "signed": False, "installed": False, "release_qualified": False}
        (temporary / "cargo-build.jsonl").write_text(build.stdout, encoding="utf-8", newline="\n")
        (temporary / "receipt.json").write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n",
                                                 encoding="utf-8", newline="\n")
        # rename() also refuses a concurrently created nonempty destination.
        temporary.rename(output)
        return receipt
    finally:
        # No recursive removal: only this invocation's three known staging files.
        if temporary.exists():
            for name in (filename, "cargo-build.jsonl", "receipt.json"):
                (temporary / name).unlink(missing_ok=True)
            temporary.rmdir()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path, required=True, help="new immutable preparation directory")
    parser.add_argument("--target-dir", type=Path, default=ROOT / "target/desktop-release")
    parser.add_argument("--offline", action="store_true", help="require cached locked Cargo dependencies")
    args = parser.parse_args()
    receipt = prepare(ROOT, args.output_dir, args.target_dir, args.offline)
    print(json.dumps({"output_directory": str(args.output_dir.absolute()), "artifact": receipt["artifact"],
                      "release_qualified": False}, indent=2))


if __name__ == "__main__":
    main()
