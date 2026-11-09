"""Prepare pinned offline runtime bytes, then verify an externally signed bundle.

No downloads, private signing keys, installation, activation or workload dispatch.
The native shared verifier is the authority for the final package contract.
"""
from __future__ import annotations
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import subprocess

ROLES = ("base_rootfs", "browser_tools", "containerd_bundle", "document_tools", "git_tools",
         "gvisor_bundle", "node_tools", "python_tools", "runtime_service")
MAX_COMPONENT = 12 * 1024**3
MAX_PACKAGE = 32 * 1024**3


def link(path: Path) -> bool:
    try:
        metadata = path.lstat()
    except FileNotFoundError:
        return False
    return stat.S_ISLNK(metadata.st_mode) or bool(getattr(metadata, "st_file_attributes", 0) & 0x400)


def ordinary(path: Path, *, single_link: bool = True) -> None:
    if not path.is_absolute() or link(path) or any(link(p) for p in path.parents):
        raise ValueError("Select an absolute ordinary build input")
    metadata = path.stat()
    if not stat.S_ISREG(metadata.st_mode) or (single_link and metadata.st_nlink != 1):
        raise ValueError("Build input must be a regular unlinked file")


def small(path: Path, maximum: int) -> bytes:
    ordinary(path)
    with path.open("rb") as source:
        data = source.read(maximum + 1)
    if not data or len(data) > maximum:
        raise ValueError("Oversized or empty build input")
    return data


def token(value: str) -> bool:
    return isinstance(value, str) and bool(re.fullmatch(r"[A-Za-z0-9_-][A-Za-z0-9._-]{0,95}", value)) and not value.endswith(".")


def prepare(index_path: Path, output: Path) -> dict:
    index = json.loads(small(index_path, 64 * 1024))
    if set(index) != {"format_version", "platform", "version", "release_sequence", "components"}:
        raise ValueError("Explicit versioned bundle index required")
    if index["format_version"] != 1 or index["platform"] not in ("linux_x86_64", "windows_wsl2_x86_64"):
        raise ValueError("Unsupported package index")
    if not token(index["version"]) or type(index["release_sequence"]) is not int or not 0 < index["release_sequence"] < 2**63:
        raise ValueError("Version and positive signed release sequence required")
    components = index["components"]
    if not isinstance(components, list) or [c.get("role") for c in components] != list(ROLES):
        raise ValueError("All nine runtime and offline tool roles must appear in canonical order")
    total, names = 0, set()
    for c in components:
        if set(c) != {"role", "filename", "version", "source", "sha256", "bytes"}:
            raise ValueError("Each artifact needs an exact source, version, size and checksum")
        if not token(c["filename"]) or not token(c["version"]) or c["filename"].lower() in names:
            raise ValueError("Invalid or duplicate payload filename")
        if type(c["bytes"]) is not int or not 0 < c["bytes"] <= MAX_COMPONENT or not re.fullmatch(r"[0-9a-f]{64}", c["sha256"]):
            raise ValueError("Invalid pinned component bounds")
        ordinary(Path(c["source"]))
        names.add(c["filename"].lower())
        total += c["bytes"]
    if total > MAX_PACKAGE or not output.is_absolute() or output.exists() or link(output) or any(link(p) for p in output.parents):
        raise ValueError("Select a NEW absolute bounded package output")
    output.mkdir(mode=0o700)
    payload = output / "payload"
    payload.mkdir(mode=0o700)
    for c in components:
        digest, count = hashlib.sha256(), 0
        # A mismatch leaves only an unsealed review directory. It cannot acquire
        # a valid signature and no recursive cleanup touches any original file.
        with Path(c["source"]).open("rb") as source, (payload / c["filename"]).open("xb") as target:
            before = os.fstat(source.fileno())
            if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1 or before.st_size != c["bytes"]:
                raise ValueError("Pinned source type or size changed")
            while block := source.read(min(1024 * 1024, c["bytes"] - count + 1)):
                count += len(block)
                if count > c["bytes"]:
                    raise ValueError("Pinned source grew")
                digest.update(block)
                target.write(block)
            target.flush()
            os.fsync(target.fileno())
            after = os.fstat(source.fileno())
            if count != c["bytes"] or digest.hexdigest() != c["sha256"] or before.st_mtime_ns != after.st_mtime_ns or after.st_nlink != 1:
                raise ValueError("Pinned source checksum or identity changed")
    manifest = {**index, "components": [{k: v for k, v in c.items() if k != "source"} for c in components]}
    raw = (json.dumps(manifest, indent=2, sort_keys=True) + "\n").encode("utf-8")
    with (output / "manifest.json").open("xb") as target:
        target.write(raw)
        target.flush()
        os.fsync(target.fileno())
    return {"prepared": True, "manifest_sha256": hashlib.sha256(raw).hexdigest(), "signed": False,
            "installed": False, "qualified": False, "signing_domain": "localai-cowork/runtime-package/v1\\0", "payload_bytes": total}


def seal(package: Path, signature_path: Path, key_id: str, trusted_key: Path, verifier: Path) -> dict:
    if not package.is_absolute() or link(package) or any(link(p) for p in package.parents):
        raise ValueError("Select an ordinary prepared package directory")
    if not token(key_id):
        raise ValueError("Explicit trusted key ID required")
    raw = small(package / "manifest.json", 64 * 1024)
    signature = small(signature_path, 64)
    if len(signature) != 64 or len(small(trusted_key, 32)) != 32:
        raise ValueError("Raw Ed25519 signature and external public key required")
    if trusted_key.resolve().is_relative_to(package.resolve()):
        raise ValueError("Trust root cannot come from the package")
    # This is an explicitly trusted build tool, not a package payload. Cargo
    # legitimately hardlinks its top-level binary to the artifact directory.
    ordinary(verifier, single_link=False)
    with (package / "manifest.sig.json").open("x", encoding="utf-8") as target:
        json.dump({"key_id": key_id, "manifest_sha256": hashlib.sha256(raw).hexdigest(), "signature": list(signature)}, target, sort_keys=True)
        target.write("\n")
        target.flush()
        os.fsync(target.fileno())
    platform = json.loads(raw)["platform"]
    result = subprocess.run([str(verifier), str(package), str(trusted_key), key_id, platform], capture_output=True, check=False)
    if result.returncode:
        raise ValueError("Native runtime package verification rejected this bundle")
    receipt = json.loads(result.stdout)
    if not receipt.get("signature_verified") or not receipt.get("payload_verified") or receipt.get("installed") or receipt.get("qualified"):
        raise ValueError("Unexpected package verifier receipt")
    return receipt


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    p = commands.add_parser("prepare")
    p.add_argument("index", type=Path)
    p.add_argument("output", type=Path)
    p = commands.add_parser("seal")
    for name in ("package", "signature", "trusted_key", "verifier"):
        p.add_argument(name, type=Path)
    p.add_argument("key_id")
    args = parser.parse_args()
    result = prepare(args.index, args.output) if args.command == "prepare" else seal(args.package, args.signature, args.key_id, args.trusted_key, args.verifier)
    print(json.dumps(result, sort_keys=True))


if __name__ == "__main__":
    main()
