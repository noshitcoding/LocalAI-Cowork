"""Assemble the fixed alpha.2 delivery from completed build-only preparations.

This performs byte/receipt checks and packaging, never builds, tests, installs,
starts services, signs or publishes a release. Run --assemble only after both
native package owners have finished. Existing inputs and alpha.1 stay untouched.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import importlib.util
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import tarfile
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parents[2]
VERSION = "2.0.0-alpha.2"
DELIVERY = "2.0.0-alpha.2-2026-10-10-45b62a09"
HOSTS = {"windows": "x86_64-pc-windows-msvc", "linux": "x86_64-unknown-linux-gnu"}
SOURCE_TREES = (".cargo", ".github", "apps", "contracts", "crates", "deploy", "docs", "extensions", "packages", "services", "tests", "tools")
EXCLUDED = {".git", ".local", ".reference", ".evidence", "node_modules", "target", "dist", "__pycache__", "binaries", "gen"}
BINARY_SUFFIXES = {".exe", ".dll", ".pdb", ".so", ".dylib", ".rlib", ".rmeta", ".a", ".o", ".obj", ".bin", ".pyc", ".pyo", ".log", ".zip", ".gz", ".deb", ".appimage", ".msi"}
PRIVATE_SUFFIXES = {".key", ".pem", ".pfx", ".p12", ".token"}
DOCS = ("FEATURES-ALPHA2.md", "DEMO-AND-INSTALLATION.md", "DESKTOP-RELEASE.md", "MIGRATION-V1.md", "MODEL-BUDGETS.md", "PERSONAL-MONEY-CONFIGURATION.md", "NATIVE-SECRET-SETUP.md")


def load_helper(name: str, relative: str):
    spec = importlib.util.spec_from_file_location(name, ROOT / relative)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader
    spec.loader.exec_module(module)
    return module


release = load_helper("delivery_release", "tools/desktop/release.py")
desktop = load_helper("delivery_desktop", "tools/desktop/package.py")
frontend = load_helper("delivery_frontend", "tools/desktop/frontend.py")
Refused = release.Refused


def pin(path: Path) -> dict:
    path = release.regular(path)
    return {"size": path.stat().st_size, "sha256": release.sha256(path)}


def relative(name: str) -> str:
    path = PurePosixPath(name)
    if (not isinstance(name, str) or not name or path.is_absolute() or "\\" in name
            or any(part in ("", ".", "..") for part in path.parts)
            or any(char in name for char in "\r\n:") or path.as_posix() != name):
        raise Refused("a portable ordinary relative input name is required")
    return name


def read_receipt(path: Path) -> dict:
    path = release.regular(path)
    if path.stat().st_size > 8 * 1024 * 1024:
        raise Refused("receipt exceeds the delivery bound")
    return desktop.strict_json(path.read_bytes())


def checked_pins(records: dict) -> dict:
    if not isinstance(records, dict) or not records:
        raise Refused("a nonempty input manifest is required")
    for name, expected in records.items():
        relative(name)
        if (not isinstance(expected, dict) or type(expected.get("size")) is not int
                or expected["size"] < 0 or not isinstance(expected.get("sha256"), str)
                or len(expected["sha256"]) != 64
                or any(c not in "0123456789abcdef" for c in expected["sha256"])):
            raise Refused("invalid size/hash input pin")
    return records


def tree(directory: Path) -> dict:
    desktop.directory(directory)
    result = {}
    for parent, directories, files in os.walk(directory, followlinks=False):
        for name in directories:
            desktop.directory(Path(parent) / name)
        for name in files:
            path = Path(parent) / name
            result[relative(path.relative_to(directory).as_posix())] = pin(path)
    return dict(sorted(result.items()))


def copy_pinned(source: Path, destination: Path, expected: dict) -> None:
    if pin(source) != expected:
        raise Refused("an input changed before its delivery copy")
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, destination)
    shutil.copymode(source, destination)
    if pin(source) != expected or pin(destination) != expected:
        raise Refused("an input changed during its delivery copy")


def write_json(path: Path, value: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False) + "\n", encoding="utf-8", newline="\n")


def source_manifest() -> dict:
    paths = [ROOT / name for name in ("README.md", "LICENSE", "package.json", "package-lock.json", "Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "tsconfig.json")]
    synthetic = tuple(PurePosixPath(name) for name in release.HISTORICAL_DATA_DIRECTORIES)
    for folder in SOURCE_TREES:
        for parent, directories, files in os.walk(ROOT / folder, followlinks=False):
            # Never enter local data, dependency caches or generated build trees.
            directories[:] = sorted(name for name in directories if name not in EXCLUDED)
            for name in directories:
                desktop.directory(Path(parent) / name)
            for name in files:
                path = Path(parent) / name
                rel = PurePosixPath(path.relative_to(ROOT).as_posix())
                suffix = path.suffix.lower()
                fixture = any(rel.is_relative_to(base) for base in synthetic)
                if (suffix in BINARY_SUFFIXES or suffix in PRIVATE_SUFFIXES
                        or name == ".env" or name.startswith(".env.") and not name.endswith(".example")
                        or name.endswith(("-wal", "-shm"))
                        or suffix in {".db", ".sqlite", ".sqlite3"} and not fixture
                        or name in {"daemon.token", "native.token", "toolbox.json"}):
                    continue
                paths.append(path)
    return {relative(path.relative_to(ROOT).as_posix()): pin(path) for path in sorted(set(paths))}


def public_receipt(original: dict, keys: tuple[str, ...], original_pin: dict) -> dict:
    # Curated fields only: no commands, Cargo package IDs, source_context,
    # absolute compiler/source paths, stderr/stdout logs or machine inventory.
    value = {key: original[key] for key in keys if key in original}
    value.update(original_receipt=original_pin, public_receipt=True,
                 validation_scope="build_only", signed=False, installed=False,
                 published=False, release_qualified=False)
    return value


def verify_tar(path: Path, bundle: Path, members: dict) -> dict:
    found = set()
    modes = {}
    with tarfile.open(release.regular(path), "r:gz") as archive:
        for item in archive:
            parts = PurePosixPath(item.name).parts
            if not parts or parts[0] != bundle.name:
                raise Refused("server archive has an unexpected root")
            name = "/".join(parts[1:])
            if item.isdir():
                if name:
                    relative(name)
                continue
            name = relative(name)
            if not item.isfile() or name in found or name not in members or item.size != members[name]["size"]:
                raise Refused("server archive has an unexpected or duplicate member")
            digest = hashlib.sha256()
            source = archive.extractfile(item)
            if source is None:
                raise Refused("server archive member is unreadable")
            for chunk in iter(lambda: source.read(1024 * 1024), b""):
                digest.update(chunk)
            if digest.hexdigest() != members[name]["sha256"]:
                raise Refused("server archive differs from its checked bundle")
            if item.mode & ~0o777:
                raise Refused("server archive has unexpected special permission bits")
            modes[name] = item.mode
            found.add(name)
    if found != set(members):
        raise Refused("server archive omits bundle files")
    return modes


def verify_zip(path: Path, members: dict, prefix: str = "") -> None:
    with zipfile.ZipFile(release.regular(path), "r") as archive:
        names = [item.filename for item in archive.infolist()]
        if len(names) != len(set(names)) or set(names) != {prefix + name for name in members}:
            raise Refused("ZIP members differ from the delivery manifest")
        for name, expected in members.items():
            item = archive.getinfo(prefix + name)
            if item.file_size != expected["size"]:
                raise Refused("ZIP member size differs")
            digest = hashlib.sha256()
            with archive.open(item) as source:
                for chunk in iter(lambda: source.read(1024 * 1024), b""):
                    digest.update(chunk)
            if digest.hexdigest() != expected["sha256"]:
                raise Refused("ZIP member bytes differ")


def checked_native(prepared: Path, host: str, core: dict) -> tuple[dict, Path, dict]:
    """Check both native receipts without treating the current OS as their host.

    The Linux owner builds this same Windows checkout through its fixed DrvFS
    pathname. The compiler source must name that checkout, not an arbitrary
    matching filename. Existing native-only preparation helpers stay strict.
    """
    desktop.directory(prepared)
    receipt = read_receipt(prepared / "receipt.json")
    if (receipt.get("format") != "localai-cowork-native-release-sidecar/1"
            or receipt.get("host") != host or receipt.get("profile") != "release"
            or receipt.get("cargo_exit_code") != 0 or receipt.get("package_version") != VERSION
            or receipt.get("source_before_after_equal") is not True or receipt.get("sources") != core
            or any(receipt.get(flag) is not False for flag in ("signed", "installed", "release_qualified"))):
        raise Refused("a current successful native alpha.2 release receipt is required")
    filename = "cowork-daemon-" + host + (".exe" if host.endswith("msvc") else "")
    artifact = receipt.get("artifact", {})
    sidecar = prepared / filename
    expected = {key: artifact.get(key) for key in ("size", "sha256")}
    if artifact.get("filename") != filename or pin(sidecar) != expected:
        raise Refused("prepared native sidecar bytes changed")
    release.validate_executable(sidecar, host)
    cargo_path = release.regular(prepared / "cargo-build.jsonl")
    raw = cargo_path.read_bytes()
    if hashlib.sha256(raw).hexdigest() != receipt.get("cargo_json_sha256"):
        raise Refused("native Cargo evidence changed")
    messages = desktop.cargo_messages(raw.decode("utf-8"))
    candidates = [item for item in messages if item.get("reason") == "compiler-artifact"
                  and item.get("package_id") == receipt.get("package_id")
                  and item.get("target", {}).get("name") == "cowork-daemon"
                  and item.get("target", {}).get("kind") == ["bin"] and item.get("executable")]
    if len(candidates) != 1 or [item.get("success") for item in messages if item.get("reason") == "build-finished"] != [True]:
        raise Refused("one successful actual daemon Cargo artifact is required")
    expected_source = (ROOT / "services/daemon/src/main.rs").as_posix()
    if os.name == "nt" and host == HOSTS["linux"]:
        expected_source = "/mnt/" + ROOT.drive[0].lower() + ROOT.as_posix()[2:] + "/services/daemon/src/main.rs"
    item = candidates[0]
    profile = item.get("profile", {})
    if (profile.get("opt_level") != "3" or profile.get("debug_assertions") is not False
            or profile.get("test") is not False or profile.get("overflow_checks") is not False
            or item["executable"] != receipt.get("cargo_reported_artifact")
            or item.get("target", {}).get("src_path", "").replace("\\", "/") != expected_source):
        raise Refused("native Cargo evidence does not describe this exact release daemon")
    return receipt, sidecar, {"receipt.json": pin(prepared / "receipt.json"), "cargo-build.jsonl": pin(cargo_path), filename: expected}


def verify_inputs() -> dict:
    local = ROOT / ".local"
    front_path = local / "alpha2-frontend-20261010" / "receipt.json"
    front = read_receipt(front_path)
    if (front.get("format") != "localai-cowork-frontend-build/1" or front.get("exit_code") != 0
            or front.get("sources_unchanged") is not True or front.get("validation_scope") != "build_only"
            or front.get("version") != VERSION or front.get("sources") != frontend.sources()):
        raise Refused("current successful unchanged alpha.2 frontend build is required")
    outputs = checked_pins(front.get("outputs"))
    actual_web = {"apps/web/dist/" + name: expected for name, expected in tree(ROOT / "apps/web/dist").items()}
    if outputs != actual_web or "apps/web/dist/index.html" not in outputs:
        raise Refused("frontend build output bytes changed")
    core = release.source_manifest(ROOT)
    packages = {}
    for platform, host in HOSTS.items():
        prepared = local / f"alpha2-prepared-{platform}-20261010"
        native, sidecar, sidecar_pins = checked_native(prepared, host, core)
        if native.get("package_version") != VERSION or native["sources"] != core:
            raise Refused("matching alpha.2 native source/version binding is required")
        directory = local / f"alpha2-package-{platform}-20261010"
        report_path = directory / "receipt.json"
        report = read_receipt(report_path)
        required_kinds = {"nsis"} if platform == "windows" else {"deb", "appimage"}
        if (report.get("format") != "localai-cowork-native-desktop-package/1" or report.get("result") != "passed"
                or report.get("host") != host or report.get("validation_scope") != "build_only"
                or report.get("source_before_after_equal") is not True or report.get("staged_source_bytes_equal") is not True
                or report.get("sources") != desktop.input_manifest(ROOT)
                or report.get("sidecar_preparation") != sidecar_pins
                or report.get("cli_version") != "2.11.4" or set(report.get("bundles", {})) != required_kinds
                or any(report.get(flag) is not False for flag in ("signed", "installed", "published", "release_qualified"))):
            raise Refused("completed current build-only native desktop package is required")
        for kind, artifact in report["bundles"].items():
            name = relative(artifact.get("filename", ""))
            if "/" in name or pin(directory / name) != {key: artifact.get(key) for key in ("size", "sha256")}:
                raise Refused("native bundle bytes differ from their receipt")
            desktop.validate_bundle(directory / name, kind, host)
        packages[platform] = {"directory": directory, "report": report, "report_pin": pin(report_path),
                              "native": native, "native_pin": pin(prepared / "receipt.json"), "sidecar": sidecar,
                              "sidecar_pins": sidecar_pins}
    server = local / "alpha2-server-20261010"
    server_files = tree(server)
    server_report = read_receipt(server / "bundle-receipt.json")
    server_native = read_receipt(server / "daemon-release-receipt.json")
    linux = packages["linux"]
    web_relative = {name.removeprefix("apps/web/dist/"): expected for name, expected in outputs.items()}
    if (server_report.get("format") != "localai-cowork-personal-server-bundle/1"
            or server_report.get("host") != HOSTS["linux"] or server_report.get("source_before_after_equal") is not True
            or server_native != linux["native"] or server_report.get("daemon") != server_native["artifact"]
            or server_report.get("daemon_release_receipt_sha256") != linux["native_pin"]["sha256"]
            or server_report.get("source_files") != len(core) or server_native.get("sources") != core
            or server_report.get("project_license") != {"source": "LICENSE", **pin(ROOT / "LICENSE")}
            or pin(server / "LICENSE") != pin(ROOT / "LICENSE")
            or pin(server / "bin/cowork-daemon") != {key: server_native["artifact"][key] for key in ("size", "sha256")}
            or server_report.get("web_files") != web_relative
            or {name.removeprefix("web/"): value for name, value in server_files.items() if name.startswith("web/")} != web_relative
            or server_report.get("package_helper_sha256") != pin(ROOT / "tools/server/package.py")["sha256"]):
        raise Refused("matching current server bundle/core/web bytes are required")
    if set(server_report.get("helpers", {})) != {"start.sh", "install-user.sh", "cowork-v2.service.example", "nginx.conf.example", "README.md"}:
        raise Refused("server helpers differ from the fixed packaging contract")
    for name, expected in checked_pins({name: {"size": (ROOT / "tools/server" / relative(name)).stat().st_size, "sha256": sha} for name, sha in server_report["helpers"].items()}).items():
        if pin(ROOT / "tools/server" / name) != expected or pin(server / name) != expected:
            raise Refused("server helper changed")
    checks = (server / "SHA256SUMS").read_text(encoding="utf-8").splitlines()
    manifest = {}
    for line in checks:
        if len(line) < 67 or line[64:66] != "  ":
            raise Refused("invalid server checksum manifest")
        name = relative(line[66:])
        if name in manifest or name == "SHA256SUMS" or name not in server_files:
            raise Refused("invalid server checksum member")
        manifest[name] = {"size": server_files[name]["size"], "sha256": line[:64]}
    if manifest != {name: expected for name, expected in server_files.items() if name != "SHA256SUMS"}:
        raise Refused("server checksum manifest does not bind all actual members")
    server_tar = local / "alpha2-server-20261010.tar.gz"
    server_modes = verify_tar(server_tar, server, server_files)
    return {"core": core, "front": front, "front_pin": pin(front_path), "packages": packages,
            "server": server, "server_files": server_files, "server_report": server_report,
            "server_report_pin": pin(server / "bundle-receipt.json"), "server_tar": server_tar, "server_tar_pin": pin(server_tar), "server_modes": server_modes}


def checksum_manifest(directory: Path) -> None:
    records = tree(directory)
    (directory / "SHA256SUMS").write_text("".join(f"{item['sha256']}  {name}\n" for name, item in records.items() if name != "SHA256SUMS"), encoding="utf-8", newline="\n")


def assemble() -> dict:
    inputs = verify_inputs()  # No output directory until all native inputs exist.
    source_before = source_manifest()
    if any(source_before.get(name) != expected for name, expected in inputs["core"].items()):
        raise Refused("source archive allowlist does not contain the complete prepared core")
    parent = desktop.directory(ROOT / "dist")
    output = parent / DELIVERY
    installation_zip = parent / (DELIVERY + "-installation.zip")
    zip_checksum = parent / (installation_zip.name + ".sha256")
    if any(path.exists() or path.is_symlink() for path in (output, installation_zip, zip_checksum)):
        raise Refused("alpha.2 delivery and installation ZIP must be new")
    staged = Path(tempfile.mkdtemp(prefix=".alpha2-delivery-", dir=parent))
    receipt_keys = ("format", "host", "package_version", "profile", "cargo_exit_code", "artifact", "sources", "source_before_after_equal", "cargo_json_sha256")
    for platform, value in inputs["packages"].items():
        for artifact in value["report"]["bundles"].values():
            name = artifact["filename"]
            copy_pinned(value["directory"] / name, staged / "installers" / platform / name, {key: artifact[key] for key in ("size", "sha256")})
        write_json(staged / "PublicReceipts" / f"sidecar-{platform}.json", public_receipt(value["native"], receipt_keys, value["native_pin"]))
        write_json(staged / "PublicReceipts" / f"desktop-{platform}.json", public_receipt(value["report"],
            ("format", "result", "host", "bundles", "sources", "sidecar_preparation", "desktop_artifact", "bundle_input_desktop", "cli_version", "node", "source_before_after_equal", "staged_source_bytes_equal", "installer_payload_extraction_qualified"), value["report_pin"]))
    write_json(staged / "PublicReceipts/frontend.json", public_receipt(inputs["front"],
        ("format", "version", "exit_code", "sources_unchanged", "sources", "outputs", "tests_run"), inputs["front_pin"]))
    for name, expected in inputs["front"]["outputs"].items():
        copy_pinned(ROOT / name, staged / "web" / name.removeprefix("apps/web/dist/"), expected)
    portable_server = staged / "server" / "localai-cowork-server"
    for name, expected in inputs["server_files"].items():
        if name not in {"bundle-receipt.json", "daemon-release-receipt.json", "SHA256SUMS"}:
            copy_pinned(inputs["server"] / name, portable_server / name, expected)
    public_native = public_receipt(inputs["packages"]["linux"]["native"], receipt_keys, inputs["packages"]["linux"]["native_pin"])
    write_json(portable_server / "daemon-release-receipt.json", public_native)
    public_server = public_receipt(inputs["server_report"],
        ("format", "host", "daemon", "source_files", "source_before_after_equal", "web_files", "helpers", "package_helper_sha256", "project_license", "bind", "default_port", "worker", "origin", "requires_docker", "release_channel", "bind_environment_override", "team_tools_require_operator_docker_configuration", "team_file_backends", "enterprise_complete", "v1_bridge", "v1_bridge_scope", "full_v1_compatibility", "android_apk_included", "web_build_or_browser_qualification_claim", "feature_runtime_qualification_claim", "enterprise_deployment_qualification_claim", "s3_deployment_qualification_claim"), inputs["server_report_pin"])
    public_server["original_server_archive"] = inputs["server_tar_pin"]
    public_server["daemon_release_receipt_sha256"] = pin(portable_server / "daemon-release-receipt.json")["sha256"]
    public_server["portable_receipts_only_change"] = True
    write_json(portable_server / "bundle-receipt.json", public_server)
    write_json(staged / "PublicReceipts/server.json", public_server)
    checksum_manifest(portable_server)
    def portable_tar_member(item: tarfile.TarInfo) -> tarfile.TarInfo:
        name = PurePosixPath(item.name).relative_to(portable_server.name).as_posix()
        item.mode = 0o700 if item.isdir() else inputs["server_modes"][name]
        item.uid = item.gid = 0
        item.uname = item.gname = ""
        return item
    with tarfile.open(staged / "server/localai-cowork-server-linux-x64.tar.gz", "x:gz") as archive:
        archive.add(portable_server, arcname=portable_server.name, filter=portable_tar_member)
    portable_modes = verify_tar(staged / "server/localai-cowork-server-linux-x64.tar.gz", portable_server, tree(portable_server))
    if portable_modes != inputs["server_modes"]:
        raise Refused("portable server file modes differ from the original verified archive")
    for name, expected in source_before.items():
        if name.startswith("deploy/v2/") or name.startswith("tools/toolbox/") or name in {"LICENSE", "README.md"} or name in {"docs/" + doc for doc in DOCS}:
            copy_pinned(ROOT / name, staged / name, expected)
    with zipfile.ZipFile(staged / "SOURCES.zip", "x", compression=zipfile.ZIP_DEFLATED, compresslevel=6) as archive:
        for name, expected in source_before.items():
            if pin(ROOT / name) != expected:
                raise Refused("a source input changed before archiving")
            archive.write(ROOT / name, name)
            if pin(ROOT / name) != expected:
                raise Refused("a source input changed while archiving")
    verify_zip(staged / "SOURCES.zip", source_before)
    write_json(staged / "SOURCE-PINS.json", {"format": "localai-cowork-delivery-source-pins/1", "sources": source_before, "core": inputs["core"], "synthetic_fixture_directories": list(release.HISTORICAL_DATA_DIRECTORIES)})
    # Replace the top-level historical README with a concise current entry point;
    # its exact original remains in SOURCES.zip. Historical docs keep their scope.
    (staged / "README.md").write_text("# LocalAI Cowork 2.0.0-alpha.2\n\nUnsignierte Alpha-Lieferung mit Windows-NSIS, Linux-DEB/AppImage, eigenem Linux-Server, Webfrontend und Quellen.\n\nDieser Durchlauf umfasst Builds und Byte-/Quellbindungen. Installation, native Laufzeit, Isolation, S3/HA und vollständige V1-/M0–M10-Abnahme wurden für diese neuen Artefakte nicht geprüft. Ältere Dokumentnachweise gelten ausschließlich für ihre historischen Quellen. Ein Android-APK ist nicht enthalten.\n\nEinrichtung und Funktionsgrenzen: [Features](docs/FEATURES-ALPHA2.md). Eigener Server: [Selfhosting](deploy/v2/README.md). Prüfsummen: SHA256SUMS. SOURCES.zip enthält ausschließlich die aufgelisteten Produktquellen und festen synthetischen Fixtures, keine privaten Daten.\n\nDas Server-Tar erhält portable öffentliche Quittungen; Daemon, Web und Startdateien bleiben bytegleich zur geprüften Ausgangsbündelung. Installation/Start erfolgt später ausdrücklich durch den Benutzer.\n", encoding="utf-8", newline="\n")
    if source_manifest() != source_before:
        raise Refused("delivery source inputs changed during assembly")
    final_inputs = verify_inputs()
    if (final_inputs["core"] != inputs["core"] or final_inputs["front_pin"] != inputs["front_pin"]
            or final_inputs["server_files"] != inputs["server_files"] or final_inputs["server_tar_pin"] != inputs["server_tar_pin"]
            or any(final_inputs["packages"][platform]["report_pin"] != value["report_pin"]
                   or final_inputs["packages"][platform]["native_pin"] != value["native_pin"] for platform, value in inputs["packages"].items())):
        raise Refused("delivery preparation inputs changed during assembly")
    delivery_receipt = {"format": "localai-cowork-alpha-delivery/1", "version": VERSION, "created_at": datetime.now(timezone.utc).isoformat(),
        "validation_scope": "build_only", "signed": False, "installed": False, "published": False, "release_qualified": False,
        "tests_run_by_aggregator": False, "native_runtime_qualified": False, "enterprise_complete": False,
        "source_before_after_equal": True, "core_sources_equal_across_platforms": True,
        "source_count": len(source_before), "core_source_count": len(inputs["core"]),
        "files": tree(staged), "server_archive_derivation": "unchanged_payload_with_sanitized_public_receipts_and_regenerated_checksums"}
    write_json(staged / "DELIVERY-RECEIPT.json", delivery_receipt)
    checksum_manifest(staged)
    temporary_zip = parent / ("." + DELIVERY + "-" + os.urandom(8).hex() + ".zip")
    delivered = tree(staged)
    with zipfile.ZipFile(temporary_zip, "x", compression=zipfile.ZIP_DEFLATED, compresslevel=6) as archive:
        for name, expected in delivered.items():
            if pin(staged / name) != expected:
                raise Refused("staged delivery bytes changed")
            archive.write(staged / name, DELIVERY + "/" + name)
    if tree(staged) != delivered:
        raise Refused("delivery bytes changed during ZIP creation")
    verify_zip(temporary_zip, delivered, DELIVERY + "/")
    if source_manifest() != source_before:
        raise Refused("product sources changed before delivery publication")
    zip_pin = pin(temporary_zip)
    desktop.publish(staged, output)
    desktop.publish(temporary_zip, installation_zip)
    with zip_checksum.open("x", encoding="utf-8", newline="\n") as stream:
        stream.write(f"{zip_pin['sha256']}  {installation_zip.name}\n")
    return {"delivery": output.name, "installation_zip": {"filename": installation_zip.name, **zip_pin},
            "files": len(delivered), "validation_scope": "build_only", "signed": False, "installed": False, "published": False, "release_qualified": False}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--assemble", action="store_true", help="verify all completed fixed inputs and create the new alpha.2 delivery")
    args = parser.parse_args()
    if args.assemble:
        print(json.dumps(assemble(), indent=2))
    else:
        print(json.dumps({"delivery": DELIVERY, "action": "await_completed_native_packages", "run": "--assemble", "validation_scope": "build_only"}, indent=2))
