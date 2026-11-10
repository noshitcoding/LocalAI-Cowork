import copy
import hashlib
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
import uuid
from unittest.mock import Mock

from instance_backups import BackupError, InstanceBackups, unpack_verified, validate_manifest
from provisioner import Infrastructure, ProvisioningError, resource_names, private_write


class BackupIntegrityTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.instance = {"id": str(uuid.uuid4()), "organisation_id": str(uuid.uuid4()), "state": "stopped",
                         "backend": "compose", "policy": {"minimum_isolation": "container"}}
        self.backup_id = str(uuid.uuid4())
        self.images = {"server": "test@sha256:" + "a" * 64}
        self.files = {name: name.encode() for name in ["database.dump", "instance.json", "secrets/storage_master_key", "secrets/bootstrap_token", "secrets/runner_signing_key"]}
        self.manifest = {"schema_version": 1, "instance_id": self.instance["id"], "organisation_id": self.instance["organisation_id"],
                         "backup_id": self.backup_id, "images": self.images, "workspace_runs": [],
                         "files": {name: {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()} for name, data in self.files.items()}}

    def archive(self, *, extra=None, exclude=None, link=None, corrupt=None):
        path = self.root / "backup.tar"
        with tarfile.open(path, "w") as out:
            items = [("manifest.json", json.dumps(self.manifest).encode()), *self.files.items()]
            if extra:
                items.append(extra)
            for name, content in items:
                if name == exclude:
                    continue
                info = tarfile.TarInfo(name)
                if name == link:
                    info.type = tarfile.SYMTYPE
                    info.linkname = "/etc/passwd"
                    out.addfile(info)
                else:
                    if name == corrupt:
                        content = b"X" * len(content)
                    info.size = len(content)
                    out.addfile(info, io.BytesIO(content))
        return path

    def unpack(self, path):
        target = self.root / str(uuid.uuid4())
        target.mkdir()
        return unpack_verified(path, target, self.instance, self.backup_id, self.images)

    def test_valid_archive_checks_every_file(self):
        self.assertEqual(self.unpack(self.archive()), self.manifest)

    def test_empty_object_and_workspace_trees_are_valid_restore_sources(self):
        target = self.root / 'empty-instance'
        target.mkdir()
        unpack_verified(self.archive(), target, self.instance, self.backup_id, self.images)
        self.assertEqual(list((target / 'objects').iterdir()), [])
        self.assertEqual(list((target / 'workspaces').iterdir()), [])

    def test_scope_and_release_must_match_before_extracting(self):
        for field in ["instance_id", "organisation_id", "backup_id"]:
            manifest = {**self.manifest, field: str(uuid.uuid4())}
            with self.subTest(field=field), self.assertRaisesRegex(BackupError, "scope"):
                validate_manifest(manifest, self.instance, self.backup_id, self.images)
        with self.assertRaisesRegex(BackupError, "release"):
            validate_manifest(self.manifest, self.instance, self.backup_id, {"server": "different"})

    def test_paths_links_missing_duplicate_and_tampered_files_rejected(self):
        for path in ["../outside", "/etc/passwd", "secrets/../../escape", "a\\b", "a//b", "./database.dump"]:
            manifest = copy.deepcopy(self.manifest)
            manifest["files"][path] = manifest["files"]["database.dump"]
            with self.subTest(path=path), self.assertRaises(BackupError):
                validate_manifest(manifest, self.instance, self.backup_id, self.images)
        for options in [dict(link="database.dump"), dict(exclude="database.dump"), dict(corrupt="database.dump"),
                        dict(extra=("database.dump", self.files["database.dump"])), dict(extra=("../escape", b"bad"))]:
            with self.subTest(options=options), self.assertRaises(BackupError):
                self.unpack(self.archive(**options))

    def test_incomplete_or_malformed_manifest_fails_closed(self):
        for mutate in [lambda x: x.update(workspace_runs="invalid"), lambda x: x.update(workspace_runs=["a" * 32]),
                       lambda x: x["files"].pop("secrets/runner_signing_key"),
                       lambda x: x["files"]["database.dump"].update(sha256=None),
                       lambda x: x["files"]["database.dump"].update(bytes=True)]:
            value = copy.deepcopy(self.manifest)
            mutate(value)
            with self.assertRaises(BackupError):
                validate_manifest(value, self.instance, self.backup_id, self.images)

    def test_running_workload_or_database_connection_prevents_capture(self):
        backup = object.__new__(InstanceBackups)
        backup.infra = Mock(resource_names=resource_names)
        admin_url = self.root / "url"
        admin_url.write_text("postgres://operator:secret@db:5432/postgres")
        backup.config = {"database_admin_url_file": str(admin_url)}
        backup.run = Mock(return_value=b"running-container")
        with self.assertRaisesRegex(BackupError, "workloads"):
            backup.quiescent(self.instance, self.root)
        backup.run = Mock(side_effect=[b"", b"1"])
        with self.assertRaisesRegex(BackupError, "database"):
            backup.quiescent(self.instance, self.root)
        backup.run = Mock(side_effect=[b"", b"0"])
        backup.quiescent(self.instance, self.root)
        backup.run = Mock()
        with self.assertRaisesRegex(BackupError, "stopped"):
            backup.quiescent({**self.instance, "state": "ready"}, self.root)
        backup.run.assert_not_called()

    def test_invalid_catalog_never_decrypts_or_mutates_instance(self):
        backup = object.__new__(InstanceBackups)
        backup.root = self.root
        backup.config = {}
        backup.infra = Mock()
        backup.run = Mock()
        path = self.root / (uuid.UUID(self.backup_id).hex + ".age")
        path.write_bytes(b"encrypted")
        operation = {"instance": self.instance, "arguments": {"backup_id": self.backup_id}, "backup": {
            "id": self.backup_id, "instance_id": self.instance["id"], "organisation_id": self.instance["organisation_id"], "sha256": "wrong"}}
        with self.assertRaisesRegex(BackupError, "catalog_integrity"):
            backup.restore(operation, self.root, self.root)
        backup.run.assert_not_called()
        self.assertEqual(backup.infra.mock_calls, [])

    def test_generation_has_distinct_database_bucket_volume_and_credentials(self):
        infrastructure = Infrastructure({"state_directory": str(self.root), "database_endpoint": "postgres:5432"})
        base = infrastructure.directory(self.instance["id"])
        original = resource_names(self.instance, base)
        old_keys = infrastructure.ensure_secrets(self.instance, base)
        generation = str(uuid.uuid4())
        stage = base / "generations" / uuid.UUID(generation).hex
        stage.mkdir(parents=True)
        (stage / "generation.json").write_text(json.dumps({"generation": generation}))
        fresh = resource_names(self.instance, stage)
        for name in ["database", "bucket", "volume_prefix"]:
            self.assertNotEqual(original[name], fresh[name])
        keys = infrastructure.ensure_secrets(self.instance, stage)
        for name in ["database_password", "s3_access_key", "s3_secret_key"]:
            self.assertNotEqual(old_keys[name], keys[name])
        self.assertEqual(infrastructure.active_directory(base), base)
        (base / "active.json").write_text(json.dumps({"generation": generation}))
        self.assertEqual(infrastructure.active_directory(base), stage)
        (stage / "generation.json").write_text(json.dumps({"generation": str(uuid.uuid4())}))
        with self.assertRaisesRegex(ProvisioningError, "mismatch"):
            infrastructure.active_directory(base)

    def test_replacement_recovery_key_cannot_silently_mix_backup_identities(self):
        infrastructure = Mock(config={"backup_directory": str(self.root), "backup_identity_file": "identity"},
                              run=Mock(return_value=b"age1originalkey"), private_write=private_write)
        InstanceBackups(infrastructure)
        infrastructure.run.return_value = b"age1replacementkey"
        with self.assertRaisesRegex(BackupError, "identity_mismatch"):
            InstanceBackups(infrastructure)
        self.assertEqual((self.root / "recipient.txt").read_text().strip(), "age1originalkey")


if __name__ == "__main__":
    unittest.main()
