"""Read-only checks shared by preview publication and its isolated update feed."""
from __future__ import annotations

import base64
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
from urllib.parse import urlsplit

HOSTS = {"x86_64-pc-windows-msvc": ("windows-x86_64", {"nsis"}),
         "x86_64-unknown-linux-gnu": ("linux-x86_64", {"deb", "appimage"})}
PREVIEW_ENDPOINT_SUFFIX = "/updates/v2/preview.json"


def ordinary(path: Path, *, directory: bool = False) -> Path:
    path = Path(os.path.abspath(path))
    for part in [*reversed(path.parents), path]:
        info = part.lstat()
        if part.is_symlink() or getattr(info, "st_file_attributes", 0) & 0x400:
            raise ValueError("Linked/reparse release inputs are not supported")
    if directory:
        if not path.is_dir():
            raise ValueError("An ordinary release directory is required")
    elif not path.is_file() or path.stat().st_nlink != 1:
        raise ValueError("An ordinary release file without hardlinks is required")
    return path


def pin(path: Path) -> dict:
    path = ordinary(path)
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return {"size": path.stat().st_size, "sha256": digest.hexdigest()}


def read(path: Path) -> dict:
    path = ordinary(path)
    if path.stat().st_size > 64 * 1024 * 1024:
        raise ValueError("Release JSON exceeds its bound")
    def pairs(items):
        value = {}
        for key, item in items:
            if key in value:
                raise ValueError("Duplicate release JSON fields are unsupported")
            value[key] = item
        return value
    value = json.loads(path.read_bytes(), object_pairs_hook=pairs)
    if not isinstance(value, dict):
        raise ValueError("A release JSON object is required")
    return value


def filename(value: str) -> str:
    if (not isinstance(value, str) or not 1 <= len(value) <= 200
            or value in {".", ".."} or value.rstrip(" .") != value
            or any(ord(char) < 32 or char in '/\\:<>"|?*' for char in value)
            or Path(value).name != value):
        raise ValueError("Release assets must use ordinary single filenames")
    return value


def checked_asset(folder: Path, record: dict) -> Path:
    path = folder / filename(record["filename"])
    if pin(path) != {key: record.get(key) for key in ("size", "sha256")}:
        raise ValueError("Release asset bytes differ from their original receipt")
    return path


def https_url(value: str) -> str:
    url = urlsplit(value)
    if (url.scheme != "https" or not url.hostname or url.username or url.password
            or url.query or url.fragment or any(char in value for char in "\r\n")):
        raise ValueError("Select a public HTTPS URL without credentials/query/fragment")
    return value


def preview_version(value: str) -> str:
    if not re.fullmatch(r"2\.\d+\.\d+-(?:alpha|beta|rc)\.[1-9]\d*", value):
        raise ValueError("V2 preview publication requires a 2.x prerelease version")
    return value


def preview_endpoint(value: str) -> str:
    https_url(value)
    if not urlsplit(value).path.endswith(PREVIEW_ENDPOINT_SUFFIX):
        raise ValueError("V2 previews require their isolated /updates/v2/preview.json feed")
    return value


def verify_updater_signature(payload: Path, signature: Path, public_key: str, version: str) -> None:
    """Verify Tauri's outer-base64 minisign encoding with Node's Ed25519 primitive.

    Follows minisign-verify 0.2.5: prehashed ED signatures sign BLAKE2b-512;
    the global signature also authenticates signature bytes + trusted comment.
    No private material or third-party Python dependencies are involved.
    """
    try:
        public_lines = base64.b64decode(public_key, validate=True).decode("utf-8").splitlines()
        lines = base64.b64decode(signature.read_text().strip(), validate=True).decode("utf-8").splitlines()
        key = base64.b64decode(public_lines[1], validate=True)
        signed = base64.b64decode(lines[1], validate=True)
        global_signature = base64.b64decode(lines[3], validate=True)
        if (len(public_lines) != 2 or len(lines) != 4 or len(key) != 42 or len(signed) != 74
                or len(global_signature) != 64 or key[:2] not in {b"Ed", b"ED"}
                or signed[:2] != b"ED" or key[2:10] != signed[2:10]
                or not lines[0].startswith("untrusted comment: ")
                or not lines[2].startswith("trusted comment: ")):
            raise ValueError("Unsupported updater signing encoding or key identity")
        comment = lines[2][17:]
        signed_versions = [field[8:] for field in comment.split("\t") if field.startswith("version:")]
        if signed_versions and signed_versions != [version]:
            raise ValueError("Updater signature names a different product version")
        digest = hashlib.blake2b()
        with payload.open("rb") as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                digest.update(chunk)
        data = {"key": key[10:].hex(), "digest": digest.hexdigest(), "signature": signed[10:].hex(),
                "global_signature": global_signature.hex(), "comment": comment}
        verifier = """const fs=require('node:fs'),c=require('node:crypto');
const v=JSON.parse(fs.readFileSync(0,'utf8'));
const key=c.createPublicKey({key:Buffer.concat([Buffer.from('302a300506032b6570032100','hex'),Buffer.from(v.key,'hex')]),format:'der',type:'spki'});
const sig=Buffer.from(v.signature,'hex');
if(!c.verify(null,Buffer.from(v.digest,'hex'),key,sig)||!c.verify(null,Buffer.concat([sig,Buffer.from(v.comment,'utf8')]),key,Buffer.from(v.global_signature,'hex')))process.exit(1);
"""
        result = subprocess.run(["node", "-e", verifier], input=json.dumps(data), text=True,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30)
        if result.returncode:
            raise ValueError("Updater payload/global signature verification failed")
    except (IndexError, UnicodeError, OSError, subprocess.TimeoutExpired) as error:
        raise ValueError("Cannot verify the actual updater signature") from error


def checked_package(folder: Path, version: str, *, strict_preview: bool = False,
                    trusted_policy: dict | None = None) -> tuple[dict, dict]:
    if not folder.is_absolute():
        raise ValueError("Select an absolute immutable package directory")
    folder = ordinary(folder, directory=True)
    receipt = read(folder / "receipt.json")
    if (receipt.get("format") != "localai-cowork-native-desktop-package/1"
            or receipt.get("result") != "passed" or receipt.get("signed") is not True
            or receipt.get("product_version") != version
            or receipt.get("source_before_after_equal") is not True
            or receipt.get("staged_source_bytes_equal") is not True):
        raise ValueError("Current source-bound signed package receipts are required")
    host = receipt.get("host")
    if host not in HOSTS:
        raise ValueError("Unsupported update platform")
    kinds = HOSTS[host][1]
    bundles = receipt.get("bundles", {})
    if not bundles or not set(bundles) <= kinds:
        raise ValueError("Package bundles do not match the actual native host")
    updater_kind = "nsis" if host.endswith("msvc") else "appimage"
    if updater_kind not in bundles:
        raise ValueError("The actual native updater bundle is required")
    paths = {}
    for kind, record in bundles.items():
        path = checked_asset(folder, record)
        suffix = {"nsis": ".exe", "deb": ".deb", "appimage": ".appimage"}[kind]
        if not path.name.lower().endswith(suffix):
            raise ValueError("Release asset suffix does not match its bundle kind")
        paths[kind] = path
        if kind == updater_kind:
            signature = checked_asset(folder, record["updater_signature"])
            if signature.name != path.name + ".sig" or not 1 <= signature.stat().st_size <= 16384:
                raise ValueError("Updater signature must be the bounded matching .sig file")
            try:
                raw = base64.b64decode(signature.read_text().strip(), validate=True)
            except (ValueError, UnicodeError) as error:
                raise ValueError("Updater signature is not encoded minisign data") from error
            if not raw.startswith(b"untrusted comment: ") or b"\ntrusted comment: " not in raw:
                raise ValueError("Updater signature is not encoded minisign data")
            paths[kind + "_signature"] = signature
    profile = receipt.get("signing_profile") or {}
    if not profile.get("updater_public_key"):
        raise ValueError("The original public signing profile is required")
    preview_endpoint(profile.get("updater_endpoint", ""))
    if trusted_policy is not None:
        from preview_trust import identity
        identity(receipt, trusted_policy)
    elif strict_preview:
        raise ValueError("An independently reviewed preview identity policy is required")
    verify_updater_signature(paths[updater_kind], paths[updater_kind + "_signature"],
                             profile["updater_public_key"], version)
    sbom_path = checked_asset(folder, receipt["sbom"])
    sbom = read(sbom_path)
    if sbom.get("spdxVersion") != "SPDX-2.3" or sbom.get("SPDXID") != "SPDXRef-DOCUMENT":
        raise ValueError("The original SPDX-2.3 release SBOM is required")
    coverage = {(item.get("fileName"), check.get("checksumValue"))
                for item in sbom.get("files", []) for check in item.get("checksums", [])
                if check.get("algorithm") == "SHA256"}
    for record in bundles.values():
        if ("./artifact/" + record["filename"], record["sha256"]) not in coverage:
            raise ValueError("SPDX does not bind every delivered package")
    paths["sbom"] = sbom_path
    if host.endswith("msvc"):
        if not profile.get("windows_certificate_thumbprint"):
            raise ValueError("Windows previews require actual Authenticode signing")
        contract = receipt.get("native_installer_contract") or {}
        if (contract.get("contract") != "nsis.currentUser.originalProcess.v1"
                or contract.get("product_version") != version
                or any(contract.get(key) != bundles["nsis"].get(key) for key in ("size", "sha256"))):
            raise ValueError("The actual signed NSIS contract is required")
    if strict_preview:
        if (set(bundles) != kinds or receipt.get("desktop_format_test_clippy_exit_codes") != [0, 0, 0]
                or receipt.get("validation_scope") != "format_tests_clippy_build"):
            raise ValueError("All advertised native packages require original passing desktop gates")
        scope = receipt.get("native_process_scope") or {}
        if host.endswith("msvc") and (scope.get("own_job_empty") is not True or scope.get("forced_cleanup")):
            raise ValueError("Windows package processes did not end cleanly")
    return receipt, paths
