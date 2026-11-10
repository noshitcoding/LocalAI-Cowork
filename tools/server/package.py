"""Prepare an immutable personal/team Linux alpha bundle from an exact release receipt.

No daemon/service/install/publication runs here. The web snapshot is bound as
supplied bytes; separate frontend/browser qualification belongs to its receipt.
"""
from __future__ import annotations
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import shutil
import tarfile
from datetime import datetime, timezone

ROOT = Path(__file__).resolve().parents[2]
HOST = "x86_64-unknown-linux-gnu"
HELPERS = ("start.sh", "install-user.sh", "cowork-v2.service.example", "nginx.conf.example", "README.md")
spec = importlib.util.spec_from_file_location("cowork_release", ROOT / "tools/desktop/release.py")
release = importlib.util.module_from_spec(spec)
assert spec.loader
spec.loader.exec_module(release)


def ordinary_tree(directory: Path) -> dict[str, dict]:
    release.regular(directory / "index.html")
    files = {}
    for path in sorted(directory.rglob("*")):
        if path.is_symlink():
            raise release.Refused("bundle inputs cannot contain symlinks")
        if path.is_file():
            release.regular(path)
            relative = path.relative_to(directory).as_posix()
            if any(char in relative for char in "\r\n\\"):
                raise release.Refused("bundle paths must be ordinary relative names")
            files[relative] = {"size": path.stat().st_size, "sha256": release.sha256(path)}
    if not files or sum(value["size"] for value in files.values()) > 128 * 1024 * 1024:
        raise release.Refused("bounded web distribution is required")
    return files


def prepare(prepared: Path, web_dist: Path, output: Path, archive: Path | None = None) -> dict:
    prepared, web_dist, output = prepared.absolute(), web_dist.absolute(), output.absolute()
    if output.exists() or output.is_symlink() or not output.parent.is_dir():
        raise release.Refused("supply a new bundle directory with an existing parent")
    if archive and (archive.exists() or archive.is_symlink() or not archive.parent.is_dir()):
        raise release.Refused("supply a new archive path with an existing parent")
    receipt_path = release.regular(prepared / "receipt.json")
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    if (receipt.get("format") != "localai-cowork-native-release-sidecar/1"
            or receipt.get("host") != HOST or receipt.get("profile") != "release"
            or receipt.get("cargo_exit_code") != 0 or receipt.get("source_before_after_equal") is not True):
        raise release.Refused("an actual successful native Linux release preparation is required")
    before = release.source_manifest(ROOT)
    if receipt.get("sources") != before:
        raise release.Refused("release receipt does not bind the current exact source closure")
    source_spec = importlib.util.spec_from_file_location("server_build_source", ROOT / "tools/release/source_bundle.py")
    source_module = importlib.util.module_from_spec(source_spec)
    source_spec.loader.exec_module(source_module)
    build_source_before = source_module.manifest()
    expected_name = "cowork-daemon-" + HOST
    artifact = receipt.get("artifact", {})
    if artifact.get("filename") != expected_name:
        raise release.Refused("the receipt has an unexpected artifact filename")
    binary = release.regular(prepared / expected_name)
    release.validate_executable(binary, HOST)
    if binary.stat().st_size != artifact.get("size") or release.sha256(binary) != artifact.get("sha256"):
        raise release.Refused("the actual daemon bytes differ from their release receipt")
    web_before = ordinary_tree(web_dist)
    helpers_before = {name: release.sha256(release.regular(ROOT / "tools/server" / name)) for name in HELPERS}
    project_license = release.regular(ROOT / "LICENSE")
    license_before = {"size": project_license.stat().st_size, "sha256": release.sha256(project_license)}
    helper_hash = release.sha256(Path(__file__))
    output.mkdir(mode=0o700)
    (output / "bin").mkdir(mode=0o700)
    (output / "web").mkdir(mode=0o700)
    build_source = source_module.prepare(output / "build-source", build_source_before)
    shutil.copyfile(binary, output / "bin/cowork-daemon")
    (output / "bin/cowork-daemon").chmod(0o700)
    for name in web_before:
        target = output / "web" / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(web_dist / name, target)
    for name in HELPERS:
        shutil.copyfile(ROOT / "tools/server" / name, output / name)
    for name in ("start.sh", "install-user.sh"):
        (output / name).chmod(0o700)
    shutil.copyfile(receipt_path, output / "daemon-release-receipt.json")
    shutil.copyfile(project_license, output / "LICENSE")
    sbom_spec = importlib.util.spec_from_file_location("cowork_server_sbom", ROOT / "tools/release/sbom.py")
    sbom_module = importlib.util.module_from_spec(sbom_spec)
    sbom_spec.loader.exec_module(sbom_module)
    sbom = sbom_module.generate(ROOT, [], payloads=[output / "bin/cowork-daemon"], include_desktop=False)
    sbom_path = output / "sbom.spdx.json"
    sbom_path.write_text(json.dumps(sbom, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    copied_web = ordinary_tree(output / "web")
    if (copied_web != web_before or ordinary_tree(web_dist) != web_before
            or release.sha256(output / "bin/cowork-daemon") != artifact["sha256"]
            or release.sha256(binary) != artifact["sha256"]
            or release.source_manifest(ROOT) != before
            or source_module.manifest() != build_source_before
            or {name: release.sha256(ROOT / "tools/server" / name) for name in HELPERS} != helpers_before
            or release.sha256(Path(__file__)) != helper_hash
            or {"size": project_license.stat().st_size, "sha256": release.sha256(release.regular(project_license))} != license_before
            or {"size": (output / "LICENSE").stat().st_size, "sha256": release.sha256(release.regular(output / "LICENSE"))} != license_before
            or release.sha256(receipt_path) != release.sha256(output / "daemon-release-receipt.json")):
        raise release.Refused("source, daemon, helper, receipt or web bytes changed during bundle preparation")
    result = {
        "format": "localai-cowork-personal-server-bundle/1", "prepared_at": datetime.now(timezone.utc).isoformat(),
        "host": HOST, "daemon": artifact, "daemon_release_receipt_sha256": release.sha256(receipt_path),
        "source_files": len(before), "source_before_after_equal": True, "web_files": web_before,
        "helpers": helpers_before, "package_helper_sha256": helper_hash,
        "build_source": {"directory": "build-source", **build_source},
        "project_license": {"source": "LICENSE", **license_before},
        "sbom": {"filename": "sbom.spdx.json", "size": sbom_path.stat().st_size, "sha256": release.sha256(sbom_path)},
        "bind": "127.0.0.1", "default_port": 43821, "worker": "normal",
        "origin": "personal_and_team_alpha", "release_channel": "alpha", "requires_docker": False,
        "bind_environment_override": "COWORK_BIND_ADDRESS",
        "team_tools_require_operator_docker_configuration": True,
        "team_file_backends": ["managed_local", "operator_configured_s3"],
        "web_build_or_browser_qualification_claim": False,
        "feature_runtime_qualification_claim": False,
        "enterprise_deployment_qualification_claim": False,
        "s3_deployment_qualification_claim": False,
        "signed": False, "installed": False, "published": False, "service_started": False,
        "enterprise_complete": False, "v1_bridge": True,
        "v1_bridge_scope": "android_schema2_passkeys_push_fleet_device_leases_rfb_over_v2_team_projects",
        "full_v1_compatibility": False, "android_apk_included": False,
    }
    (output / "bundle-receipt.json").write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8", newline="\n")
    members = sorted(path for path in output.rglob("*") if path.is_file())
    checksum_lines = [f"{release.sha256(path)}  {path.relative_to(output).as_posix()}\n" for path in members]
    (output / "SHA256SUMS").write_text("".join(checksum_lines), encoding="utf-8", newline="\n")
    if archive:
        with tarfile.open(archive, "x:gz") as stream:
            stream.add(output, arcname=output.name, recursive=True)
        result["archive"] = {"path": str(archive.absolute()), "sha256": release.sha256(archive), "size": archive.stat().st_size}
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prepared", type=Path, required=True)
    parser.add_argument("--web-dist", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--archive", type=Path)
    args = parser.parse_args()
    print(json.dumps(prepare(args.prepared, args.web_dist, args.output, args.archive), indent=2))
