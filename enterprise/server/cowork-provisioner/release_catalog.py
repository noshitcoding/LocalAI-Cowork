"""Operator-owned, immutable release manifests and scoped canary evidence.

This directory is not mounted into tenants. A management request selects a
digest; it cannot supply images, commands, compatibility claims or host paths.
"""
import hashlib
import json
from pathlib import Path
import re
import uuid


class ReleaseError(Exception):
    def __init__(self, code, *, instance_state=None, result=None):
        super().__init__(code)
        self.instance_state = instance_state
        self.result = result or {}


IMAGE_KEYS = {"server", "runner", "sandbox_core", "sandbox_gui", "sandbox_crew", "egress_proxy"}
IMAGE = re.compile(r"[a-z0-9][a-z0-9._:/-]*@sha256:[a-f0-9]{64}")
DIGEST = re.compile(r"[a-f0-9]{64}")


def image_set_id(images):
    if not isinstance(images, dict) or set(images) != IMAGE_KEYS or any(not isinstance(image, str) or not IMAGE.fullmatch(image) for image in images.values()):
        raise ReleaseError("invalid_release_images")
    return hashlib.sha256(json.dumps(images, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def validate_release(value, release_id):
    if (not isinstance(value, dict) or set(value) != {"version", "id", "name", "images", "supported_sources"}
            or type(value["version"]) is not int or value["version"] != 1 or value["id"] != release_id
            or not isinstance(value["name"], str) or not 1 <= len(value["name"]) <= 120
            or any(ord(ch) < 32 for ch in value["name"])
            or not isinstance(value["supported_sources"], list) or not value["supported_sources"]
            or any(not isinstance(source, str) or not DIGEST.fullmatch(source) for source in value["supported_sources"])
            or len(set(value["supported_sources"])) != len(value["supported_sources"])):
        raise ReleaseError("invalid_release_manifest")
    if image_set_id(value["images"]) != release_id:
        raise ReleaseError("release_digest_mismatch")
    return value


class ReleaseCatalog:
    def __init__(self, config, write):
        self.root = Path(config.get("release_directory", ""))
        self.instances = Path(config.get("state_directory", ""))
        if not self.root.is_absolute() or self.root.is_symlink() or self.root.resolve() == Path(self.root.anchor):
            raise ReleaseError("release_directory_not_configured")
        self.root.mkdir(mode=0o700, parents=True, exist_ok=True)
        self.write = write

    def read(self, path):
        if path.is_symlink() or not path.is_file():
            raise ReleaseError("release_evidence_missing")
        try:
            return json.loads(path.read_text(encoding="utf-8"))
        except (OSError, ValueError) as error:
            raise ReleaseError("release_evidence_invalid") from error

    def manifests(self):
        return [validate_release(self.read(path), path.stem) for path in sorted(self.root.glob("*.json")) if DIGEST.fullmatch(path.stem) and self.enabled(path.stem)]

    def enabled(self, release_id):
        if not isinstance(release_id, str) or not DIGEST.fullmatch(release_id):
            raise ReleaseError("invalid_release_identifier")
        marker = self.root / (release_id + ".withdrawn.json")
        return not marker.exists() and not marker.is_symlink()

    def withdraw(self, release_id):
        self.enabled(release_id)  # Validate before constructing paths.
        validate_release(self.read(self.root / (release_id + ".json")), release_id)
        self.write(self.root / (release_id + ".withdrawn.json"), json.dumps({"version": 1, "release": release_id, "withdrawn": True}).encode())

    def release(self, release_id, source_images):
        if not isinstance(release_id, str) or not DIGEST.fullmatch(release_id):
            raise ReleaseError("invalid_release_identifier")
        if not self.enabled(release_id):
            raise ReleaseError("release_withdrawn")
        value = validate_release(self.read(self.root / (release_id + ".json")), release_id)
        if image_set_id(source_images) not in value["supported_sources"]:
            raise ReleaseError("unsupported_release_transition")
        return value

    def publish(self, name, images, supported_sources):
        release_id = image_set_id(images)
        value = validate_release({"version": 1, "id": release_id, "name": name, "images": images, "supported_sources": supported_sources}, release_id)
        path = self.root / (release_id + ".json")
        if path.exists() or path.is_symlink():
            if self.read(path) != value:
                raise ReleaseError("release_manifest_is_immutable")
        else:
            self.write(path, json.dumps(value, sort_keys=True).encode())
        return value

    def evidence_path(self, release_id, source_id, operation=None):
        if any(not isinstance(value, str) or not DIGEST.fullmatch(value) for value in [release_id, source_id]):
            raise ReleaseError("invalid_release_identifier")
        profile=None if operation is None else operation.get('backend_profile')
        suffix='' if profile is None else '-backend-'+profile['specification_sha256']+'-revision-'+str(profile['revision'])
        if profile is not None and (not DIGEST.fullmatch(profile['specification_sha256']) or type(profile['revision']) is not int or profile['revision']<=0):
            raise ReleaseError('invalid_original_canary_backend')
        return self.root / (release_id + "-from-" + source_id + suffix + ".qualified.json")

    def require_qualification(self, release_id, source_id, operation=None):
        value = self.read(self.evidence_path(release_id, source_id, operation))
        expected = {"version": 1, "release": release_id, "source_release": source_id, "backend": "compose", "isolation": "container", "probe_version": 1, "rollback_verified": True}
        profile=None if operation is None else operation.get('backend_profile')
        if profile is not None:
            if profile['specification']['kind']!='vm_compose':raise ReleaseError('original_canary_backend_differs')
            expected.update(backend='vm_compose',isolation='virtual_machine',backend_profile=profile)
        if (not isinstance(value, dict) or set(value) != set(expected) | {"instance_id", "operation_id", "backup_id"}
                or any(type(value.get(key)) is not type(entry) or value[key] != entry for key, entry in expected.items())):
            raise ReleaseError("release_canary_not_qualified")
        try:
            ids = {key: uuid.UUID(value[key]) for key in ["instance_id", "operation_id", "backup_id"]}
            if any(str(parsed) != value[key] for key, parsed in ids.items()) or value["operation_id"] != value["backup_id"]:
                raise ValueError()
        except (ValueError, TypeError, AttributeError):
            raise ReleaseError("release_canary_not_qualified") from None
        if not self.instances.is_absolute() or self.instances.is_symlink():
            raise ReleaseError("release_canary_not_qualified")
        directory = self.instances / ids["instance_id"].hex
        if directory.is_symlink():
            raise ReleaseError("release_canary_not_qualified")
        marker = self.read(directory / ("upgrade-" + ids["operation_id"].hex + ".json"))
        # Evidence is usable only after the durable update marker confirms that
        # this exact canary completed. A crash after writing evidence cannot
        # qualify an incomplete transaction.
        if (not isinstance(marker, dict) or marker.get("phase") != "completed"
                or marker.get("qualification_only") is not True
                or any(marker.get(key) != value[key] for key in ["instance_id", "operation_id", "release", "source_release"])
                or not isinstance(marker.get("receipt"), dict)
                or marker["receipt"].get("rollback_verified") is not True
                or marker["receipt"].get("backup_id") != value["backup_id"]
                or marker.get('backend_profile')!=profile):
            raise ReleaseError("release_canary_not_qualified")
        return value

    def qualify(self, receipt, operation=None):
        if receipt.get("qualification_only") is not True or receipt.get("rollback_verified") is not True:
            raise ReleaseError("release_canary_not_qualified")
        value = {"version": 1, "release": receipt["release"], "source_release": receipt["source_release"],
                 "backend": "compose", "isolation": "container", "probe_version": 1, "rollback_verified": True,
                 "instance_id": receipt["instance_id"], "operation_id": receipt["receipt_id"], "backup_id": receipt["backup_id"]}
        profile=None if operation is None else operation.get('backend_profile')
        if profile is not None:
            if profile['specification']['kind']!='vm_compose':raise ReleaseError('original_canary_backend_differs')
            value.update(backend='vm_compose',isolation='virtual_machine',backend_profile=profile)
        self.write(self.evidence_path(value["release"], value["source_release"],operation), json.dumps(value, sort_keys=True).encode())
        return value
