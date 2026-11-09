"""Real age/file-archive verification; no PG restore, Engine or runtime qualification."""
import copy
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest
import uuid
from unittest.mock import Mock

import instance_imports
from instance_backups import BackupError, InstanceBackups, compact, digest, verify_backup
from provisioner import private_write


def tar_bytes(entries):
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w") as archive:
        for name, data, kind in entries:
            entry = tarfile.TarInfo(name)
            entry.type = kind
            entry.mode = 0o750 if kind == tarfile.DIRTYPE else 0o640
            if kind in (tarfile.SYMTYPE, tarfile.LNKTYPE):
                entry.linkname = "../outside"
            entry.size = len(data) if data is not None else 0
            archive.addfile(entry, io.BytesIO(data) if data is not None else None)
    return output.getvalue()


@unittest.skipUnless(shutil.which("age") and shutil.which("age-keygen"), "requires an actual age runtime")
class ActualBackupVerificationTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="cowork-real-age-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.root.chmod(0o700)
        self.source = {"id": str(uuid.uuid4()), "organisation_id": str(uuid.uuid4()), "backend": "compose",
                       "state": "stopped", "execution_required": False, "executor_host_id": None,
                       "execution_epoch": str(uuid.uuid4()), "policy": {"minimum_isolation": "container"}}
        self.backup_id = str(uuid.uuid4())
        self.images = {"server": "fixture@sha256:" + "a" * 64}
        self.workspace = uuid.uuid4().hex
        self.archive = self.root / (compact(self.backup_id) + ".age")
        self.identity = self.root / "identity"
        self.calls = []
        self.process_exits = []
        self.command(["age-keygen", "-o", str(self.identity)])
        self.recipient = self.command(["age-keygen", "-y", str(self.identity)]).decode().strip()
        self.marker = "original workspace é🙂\n".encode()
        self.files = {
            "database.dump": b"Transport fixture only; this is not a physical PostgreSQL dump.",
            "instance.json": json.dumps(self.source).encode(),
            "secrets/storage_master_key": os.urandom(32),
            "secrets/bootstrap_token": os.urandom(32),
            "secrets/runner_signing_key": os.urandom(32),
            "objects/result.txt": b"original object bytes",
            "workspaces/" + self.workspace + ".tar": tar_bytes([
                (".", None, tarfile.DIRTYPE), ("./sub", None, tarfile.DIRTYPE),
                ("./sub/marker.txt", self.marker, tarfile.REGTYPE)]),
        }
        self.manifest = {"schema_version": 1, "backup_id": self.backup_id, "instance_id": self.source["id"],
                         "organisation_id": self.source["organisation_id"], "images": self.images,
                         "workspace_runs": [self.workspace], "files": {}}

    def command(self, arguments, **options):
        self.calls.append(arguments)
        completed = subprocess.run(arguments, capture_output=True, timeout=min(options.get("timeout", 15), 15))
        self.process_exits.append((arguments[0], completed.returncode))
        completed.check_returncode()
        return completed.stdout

    def encrypt(self, *, extra=(), exclude=None, link=None, suffix=b"", raw=None):
        self.manifest["files"] = {name: {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}
                                  for name, data in self.files.items()}
        if raw is None:
            entries = [("manifest.json", json.dumps(self.manifest).encode(), tarfile.REGTYPE)]
            entries += [(name, data if name != link else None, tarfile.REGTYPE if name != link else tarfile.SYMTYPE)
                        for name, data in self.files.items() if name != exclude]
            entries += list(extra)
            raw = tar_bytes(entries) + suffix
        plain = self.root / (uuid.uuid4().hex + ".tar")
        plain.write_bytes(raw)
        if self.archive.exists():
            self.archive.unlink()
        self.command(["age", "--encrypt", "-r", self.recipient, "-o", str(self.archive), str(plain)])
        return {"id": self.backup_id, "instance_id": self.source["id"], "organisation_id": self.source["organisation_id"],
                "size_bytes": self.archive.stat().st_size, "sha256": digest(self.archive)}

    def verify(self, catalog, **options):
        scratch = self.root / uuid.uuid4().hex
        scratch.mkdir(mode=0o700)
        return verify_backup(self.archive, catalog, self.source, self.backup_id, self.images,
                             str(self.identity), self.command, scratch, **options)

    def manager(self):
        manager = object.__new__(InstanceBackups)
        manager.root = self.root
        manager.identity = str(self.identity)
        manager.config = {"images": self.images}
        manager.run = self.command
        manager.write = private_write
        manager.infra = Mock()
        return manager

    def import_operation(self, catalog):
        target = {**self.source, "id": str(uuid.uuid4()), "execution_required": True,
                  "execution_epoch": str(uuid.uuid4()), "executor_host_id": str(uuid.uuid4())}
        frozen = {"version": 1, "import_id": str(uuid.uuid4()), "organisation_id": self.source["organisation_id"],
                  "source": self.source, "target": target, "source_host_binding": None,
                  "target_host_binding": {"version": 1, "host_id": target["executor_host_id"],
                                          "trust_domain": self.source["organisation_id"]},
                  "source_allocation": None, "target_allocation": {}, "source_epoch": self.source["execution_epoch"],
                  "target_epoch": target["execution_epoch"], "candidate_epoch": str(uuid.uuid4()),
                  "backup": catalog, "reservation_ids": []}
        exported = {"backup_id": self.backup_id, "backup_sha256": catalog["sha256"],
                    "manifest_sha256": instance_imports.canonical(self.manifest), "workspace_count": 1,
                    "images": self.images}
        return {"id": str(uuid.uuid4()), "action": "import_restore", "organisation_id": self.source["organisation_id"],
                "instance_id": target["id"], "instance": target, "executor_host_binding": frozen["target_host_binding"],
                "backup": catalog, "arguments": {"backup_id": self.backup_id, "import_binding": frozen,
                    "import_binding_sha256": instance_imports.canonical(frozen), "import_export": exported}}

    def assert_no_target_writes(self, catalog, *, imported=False):
        manager = self.manager()
        directory = self.root / ("target-" + uuid.uuid4().hex)
        directory.mkdir()
        (directory / "retained-marker").write_bytes(b"unchanged")
        before = {path.relative_to(directory): path.read_bytes() for path in directory.rglob("*") if path.is_file()}
        if imported:
            operation = self.import_operation(catalog)
            options = {"source_import": operation["arguments"]["import_binding"]}
        else:
            operation = {"id": str(uuid.uuid4()), "instance": self.source,
                         "arguments": {"backup_id": self.backup_id}, "backup": catalog}
            options = {}
        with self.assertRaises(BackupError):
            manager.restore(operation, directory, directory, **options)
        manager.infra.assert_not_called()
        self.assertEqual(manager.infra.mock_calls, [])
        self.assertEqual(before, {path.relative_to(directory): path.read_bytes() for path in directory.rglob("*") if path.is_file()})
        self.assertFalse((directory / "generations").exists())
        self.assertFalse((directory / "active.json").exists())
        self.assertFalse(any(path.name.startswith("receipt-") for path in directory.rglob("*")))

    def test_real_age_roundtrip_original_scope_and_complete_workspace_inventory(self):
        catalog = self.encrypt()
        expected_manifest = copy.deepcopy(self.manifest)
        verified = self.verify(catalog, manifest_sha256=instance_imports.canonical(self.manifest))
        self.assertEqual(verified.manifest, expected_manifest)
        self.assertEqual(verified.cipher_sha256, digest(self.archive))
        self.assertEqual(verified.cipher_bytes, self.archive.stat().st_size)
        self.assertEqual(verified.manifest_sha256, instance_imports.canonical(expected_manifest))
        self.assertEqual((verified.payload / "objects/result.txt").read_bytes(), self.files["objects/result.txt"])
        marker = next(entry for entry in verified.workspace_inventory[self.workspace] if entry["path"] == "sub/marker.txt")
        self.assertEqual(marker["sha256"], hashlib.sha256(self.marker).hexdigest())
        decrypt = next(call for call in self.calls if call[:2] == ["age", "--decrypt"])
        self.assertNotEqual(Path(decrypt[-1]), self.archive)
        self.assertEqual(digest(Path(decrypt[-1])), catalog["sha256"])
        self.assertTrue(all(code == 0 for _, code in self.process_exits))

    def test_import_export_proof_uses_original_cipher_manifest_and_measured_inner_files(self):
        catalog = self.encrypt()
        original = copy.deepcopy(self.manifest)
        operation = self.import_operation(catalog)
        operation.update(action='import_export', instance=self.source, instance_id=self.source['id'], executor_host_binding=None)
        manager = self.manager()
        # Only the engine-stop gate is stubbed here. Encryption, original scope,
        # catalog and every outer/inner archive byte are checked by real code.
        manager.quiescent = Mock()
        infrastructure = Mock(active_directory=Mock(return_value=self.root), deployed_images=Mock(return_value=self.images))
        proof = instance_imports.export(infrastructure, operation, self.root, manager)
        self.assertEqual(proof['backup_sha256'], digest(self.archive))
        self.assertEqual(proof['backup_bytes'], self.archive.stat().st_size)
        self.assertEqual(proof['manifest_sha256'], instance_imports.canonical(original))
        self.assertEqual(proof['workspace_count'], 1)
        self.assertEqual(self.manifest, original)
        instance_imports.verify_export({**operation, 'arguments': {**operation['arguments'], 'import_export': proof}}, original)
        self.files['workspaces/' + self.workspace + '.tar'] = tar_bytes([('../escape', b'x', tarfile.REGTYPE)])
        changed = self.encrypt()
        failed = self.import_operation(changed)
        failed.update(action='import_export', instance=self.source, instance_id=self.source['id'], executor_host_binding=None)
        with self.assertRaises(BackupError):
            instance_imports.export(infrastructure, failed, self.root, manager)
        self.assertFalse(any(path.name.startswith('receipt-') for path in self.root.rglob('*')))

    def test_capture_checks_actual_cipher_and_inner_tar_before_publishing_catalog(self):
        active = self.root / 'owned-source-fixture'
        (active / 'secrets').mkdir(parents=True)
        for name in ['storage_master_key', 'bootstrap_token', 'runner_signing_key']:
            (active / 'secrets' / name).write_bytes(self.files['secrets/' + name])
        (active / 'secrets/database_url').write_text('postgres://fixture:fixture@localhost:5432/fixture')
        manager = self.manager()
        manager.recipient = self.recipient
        manager.config.update(s3_admin_alias='fixture')
        manager.infra.deployed_images.return_value = self.images
        manager.infra.resource_names.return_value = {'database': 'fixture', 'bucket': 'fixture', 'volume_prefix': 'owned-run-'}
        actual = self.command
        def capture_commands(arguments, **options):
            # Capture callbacks are file fixtures. Only the age cryptographic
            # subprocess is real; no DB/Docker Engine is contacted by this test.
            if arguments[0] == 'pg_dump':
                Path(arguments[arguments.index('--file') + 1]).write_bytes(self.files['database.dump'])
                return b''
            if arguments[0] == 'psql':
                return json.dumps([str(uuid.UUID(self.workspace))]).encode()
            if arguments[:3] == ['docker', 'volume', 'ls']:
                return ('owned-run-' + self.workspace + '\n').encode()
            self.assertEqual(arguments[0], 'age')
            return actual(arguments, **options)
        manager.run = capture_commands
        manager.storage = lambda action, source, target: (Path(target) / 'result.txt').write_bytes(self.files['objects/result.txt'])
        manager.volume_archive = lambda volume, target, name: (target / name).write_bytes(self.files['workspaces/' + self.workspace + '.tar'])
        receipt = manager.backup({'id': self.backup_id, 'instance': self.source}, active, active)
        self.assertEqual(receipt['backup_sha256'], digest(self.archive))
        self.assertEqual(receipt['backup_bytes'], self.archive.stat().st_size)
        published = json.loads(self.archive.with_suffix('.json').read_text())
        self.assertEqual(published['backup_sha256'], digest(self.archive))
        self.files['workspaces/' + self.workspace + '.tar'] = tar_bytes([('bad-link', None, tarfile.SYMTYPE)])
        failed = str(uuid.uuid4())
        with self.assertRaises(BackupError):
            manager.backup({'id': failed, 'instance': self.source}, active, active)
        self.assertFalse((self.root / (compact(failed) + '.age')).exists())
        self.assertFalse((self.root / (compact(failed) + '.json')).exists())
        self.assertEqual(json.loads(self.archive.with_suffix('.json').read_text()), published)

    def test_catalog_scope_size_hash_missing_size_and_cipher_truncation_refuse_before_decrypt(self):
        catalog = self.encrypt()
        for key, value in [("id", str(uuid.uuid4())), ("instance_id", str(uuid.uuid4())),
                           ("organisation_id", str(uuid.uuid4())), ("size_bytes", catalog["size_bytes"] + 1),
                           ("size_bytes", None), ("size_bytes", True),
                           ("sha256", hashlib.sha256(b"other actual bytes").hexdigest())]:
            count = len(self.calls)
            with self.subTest(key=key, value=value), self.assertRaises(BackupError):
                self.verify({**catalog, key: value})
            self.assertEqual(len(self.calls), count)
        self.archive.write_bytes(self.archive.read_bytes()[:-12])
        self.assert_no_target_writes(catalog)
        self.assert_no_target_writes(catalog, imported=True)

    def test_wrong_age_key_and_authenticated_cipher_corruption_refuse_target_writes(self):
        catalog = self.encrypt()
        original = self.identity
        wrong = self.root / "wrong-identity"
        self.command(["age-keygen", "-o", str(wrong)])
        self.identity = wrong
        self.assert_no_target_writes(catalog)
        self.assert_no_target_writes(catalog, imported=True)
        self.identity = original
        corrupted = bytearray(self.archive.read_bytes())
        corrupted[-8] ^= 1
        self.archive.write_bytes(corrupted)
        # Even a catalog measuring these actual corrupted bytes cannot turn an
        # unauthenticated age stream into a valid plaintext artifact.
        actual_corrupted = {**catalog, "sha256": digest(self.archive)}
        self.assert_no_target_writes(actual_corrupted)

    def test_instance_json_swap_manifest_scope_release_and_original_manifest_refuse(self):
        for field in ["id", "organisation_id"]:
            self.files["instance.json"] = json.dumps({**self.source, field: str(uuid.uuid4())}).encode()
            catalog = self.encrypt()
            self.assert_no_target_writes(catalog)
            self.assert_no_target_writes(catalog, imported=True)
        self.files["instance.json"] = json.dumps(self.source).encode()
        for field in ["instance_id", "organisation_id", "backup_id"]:
            original = self.manifest[field]
            self.manifest[field] = str(uuid.uuid4())
            catalog = self.encrypt()
            self.assert_no_target_writes(catalog)
            self.manifest[field] = original
        self.manifest["images"] = {"server": "another release"}
        catalog = self.encrypt()
        self.assert_no_target_writes(catalog)
        self.manifest["images"] = self.images
        catalog = self.encrypt()
        with self.assertRaisesRegex(BackupError, "original_manifest"):
            self.verify(catalog, manifest_sha256=hashlib.sha256(b"another original manifest").hexdigest())

    def test_outer_duplicate_traversal_link_missing_file_and_hidden_tail_refuse(self):
        cases = [dict(extra=[("database.dump", self.files["database.dump"], tarfile.REGTYPE)]),
                 dict(extra=[("../outside", b"escape", tarfile.REGTYPE)]), dict(link="database.dump"),
                 dict(exclude="database.dump"), dict(suffix=tar_bytes([("hidden", b"hidden", tarfile.REGTYPE)]))]
        for options in cases:
            with self.subTest(options=tuple(options)):
                catalog = self.encrypt(**options)
                self.assert_no_target_writes(catalog)
                self.assert_no_target_writes(catalog, imported=True)
        catalog = self.encrypt(raw=b"short non-tar plaintext")
        self.assert_no_target_writes(catalog)

    def test_missing_extra_and_unlisted_workspace_inventory_refuse(self):
        workspace_path = "workspaces/" + self.workspace + ".tar"
        original = self.files[workspace_path]
        del self.files[workspace_path]
        self.assert_no_target_writes(self.encrypt())
        self.files[workspace_path] = original
        extra = "workspaces/" + uuid.uuid4().hex + ".tar"
        self.files[extra] = original
        self.assert_no_target_writes(self.encrypt())
        del self.files[extra]
        self.manifest["workspace_runs"] = []
        self.assert_no_target_writes(self.encrypt())
        self.manifest["workspace_runs"] = [self.workspace]
        self.assert_no_target_writes(self.encrypt(exclude=workspace_path), imported=True)

    def test_all_inner_tars_checked_before_target_generation_for_links_aliases_and_conflicts(self):
        cases = []
        for name in ["../outside", "/absolute", "bad\\name", "sub/./file", "././alias"]:
            cases.append([(name, b"x", tarfile.REGTYPE)])
        for kind in [tarfile.SYMTYPE, tarfile.LNKTYPE, tarfile.FIFOTYPE, tarfile.CHRTYPE]:
            cases.append([("unsafe", None, kind)])
        cases += [[("same", b"x", tarfile.REGTYPE), ("./same", b"x", tarfile.REGTYPE)],
                  [("parent", b"x", tarfile.REGTYPE), ("parent/file", b"x", tarfile.REGTYPE)],
                  [("parent/file", b"x", tarfile.REGTYPE), ("parent", b"x", tarfile.REGTYPE)]]
        for entries in cases:
            with self.subTest(entry=entries[0][0]):
                self.files["workspaces/" + self.workspace + ".tar"] = tar_bytes(entries)
                catalog = self.encrypt()
                self.assert_no_target_writes(catalog)
                self.assert_no_target_writes(catalog, imported=True)
        inner = tar_bytes([("large", b"x" * 65536, tarfile.REGTYPE)])[:1024]
        self.files["workspaces/" + self.workspace + ".tar"] = inner
        self.assert_no_target_writes(self.encrypt())
        self.assertFalse((self.root / "outside").exists())

    def test_bad_later_workspace_prevents_any_earlier_workspace_or_database_write(self):
        second = uuid.uuid4().hex
        self.manifest["workspace_runs"].append(second)
        self.files["workspaces/" + second + ".tar"] = tar_bytes([("../escape", b"x", tarfile.REGTYPE)])
        catalog = self.encrypt()
        self.assert_no_target_writes(catalog)
        self.assert_no_target_writes(catalog, imported=True)

    def test_change_and_restore_during_decrypt_cannot_substitute_unbound_cipher(self):
        catalog = self.encrypt()
        original_cipher = self.archive.read_bytes()
        actual_run = self.command
        def replace_during_decrypt(arguments, **options):
            self.assertNotEqual(Path(arguments[-1]), self.archive)
            self.assertEqual(digest(Path(arguments[-1])), catalog["sha256"])
            self.archive.write_bytes(b"X" * len(original_cipher))
            try:
                return actual_run(arguments, **options)
            finally:
                self.archive.write_bytes(original_cipher)
        self.command = replace_during_decrypt
        self.assert_no_target_writes(catalog, imported=True)
        self.assertEqual(self.archive.read_bytes(), original_cipher)

    @unittest.skipUnless(os.name == "posix", "actual no-follow and inode checks")
    def test_symlink_and_hardlink_cipher_sources_are_refused(self):
        catalog = self.encrypt()
        alternate = self.root / "actual-cipher"
        self.archive.rename(alternate)
        self.archive.symlink_to(alternate)
        with self.assertRaises(BackupError):
            self.verify(catalog)
        self.archive.unlink()
        alternate.rename(self.archive)
        os.link(self.archive, self.root / "hardlink")
        with self.assertRaises(BackupError):
            self.verify(catalog)


if __name__ == "__main__":
    unittest.main()
