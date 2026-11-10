"""Real fixed source schemas and native private-fixture fences; no user data."""
import contextlib
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import sys
import tempfile
import unittest
from unittest.mock import patch
import zipfile
sys.path.insert(0, str(Path(__file__).resolve().parent))
import v1_data_policy_matrix as subject

REFERENCE = subject.ROOT / ".reference/v1-0.3.12-source-build-inputs-2026-10-09-final-unit-fixtures"
MANIFEST_SHA = "cfc5a8e0d141fa94a7755804d549d8ec7acd3f067c5bbbe4c590d3d97986d4dd"
ARCHIVE_SHA = "d4c4f69a61392cdc75f73e9da036ee1aad198eacd208d9af10ac82bdf6d7efc4"

def sha(data):
    return hashlib.sha256(data).hexdigest()

def fixed_source_texts():
    archive = REFERENCE / "source.zip"
    if sha(archive.read_bytes()) != ARCHIVE_SHA:
        raise RuntimeError("Original pinned source fixture changed")
    with zipfile.ZipFile(archive) as zipped:
        result = {name: zipped.read(name) for name in subject.frozen.profiles.SOURCE_BINDINGS}
    for name, raw in result.items():
        if sha(raw) != subject.frozen.profiles.SOURCE_BINDINGS[name]:
            raise RuntimeError("Original fixed schema source changed")
    return result

class SourceMatrixTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.raw_sources = fixed_source_texts()
        cls.sources = {name: raw.decode("utf-8") for name, raw in cls.raw_sources.items()}

    def test_all_supported_schemas_keep_explicit_no_execution_policy(self):
        opened = []
        original = sqlite3.connect
        def memory_only(path, *args, **kwargs):
            self.assertEqual(path, ":memory:")
            opened.append(path)
            return original(path, *args, **kwargs)
        with patch.object(sqlite3, "connect", side_effect=memory_only):
            value = subject.matrix(self.sources)
        self.assertGreater(len(opened), 1)
        self.assertEqual(value["profile_count"], 34)
        self.assertEqual(value["table_counts"], {"desktop": 50, "daemon": 16})
        for item in value["tables"]:
            self.assertTrue(item["v2_execution_policy"]["historical_only"])
            self.assertTrue(all(not flag for key, flag in item["v2_execution_policy"].items() if key != "historical_only"))
            self.assertTrue(item["schema_variants"])
        self.assertFalse(value["v1_runtime_policy_behavior_verified"])
        self.assertFalse(value["installed_schema_parity_proven"])
        self.assertFalse(value["full_m0_complete"])

    def test_different_authority_classes_are_never_reactivated(self):
        rows = {item["table"]: item for item in subject.matrix(self.sources)["tables"]}
        for table, group in [("allowed_folders", "permission_reissue"), ("api_profiles", "credential_recreation"),
                             ("daemon_schedules", "explicit_new_schedule"), ("daemon_sync_peers", "device_reenrollment"),
                             ("engine_run_checkpoints", "inactive_history")]:
            self.assertEqual(rows[table]["v2_review_group"], group)
            self.assertFalse(rows[table]["v2_execution_policy"]["regrant"])
            self.assertFalse(rows[table]["v2_execution_policy"]["resume"])
        self.assertTrue(rows["allowed_folders"]["source_locations"])
        self.assertGreater(len(rows["work_tasks"]["schema_variants"]), 1)

    def test_incomplete_and_ambiguous_disposition_rules_are_refused(self):
        groups = {name: set(tables) for name, tables in subject.REVIEW_GROUPS.items()}
        groups["permission_reissue"].remove("allowed_folders")
        with patch.object(subject, "REVIEW_GROUPS", groups), self.assertRaises(subject.frozen.Refused):
            subject.matrix(self.sources)
        groups["permission_reissue"].add("allowed_folders")
        groups["inactive_history"].add("allowed_folders")
        with patch.object(subject, "REVIEW_GROUPS", groups), self.assertRaises(subject.frozen.Refused):
            subject.matrix(self.sources)

    def test_platform_refusal_precedes_any_selected_filesystem_read(self):
        with patch.object(os, "name", "posix"), patch.object(subject.frozen, "bounded_path", side_effect=AssertionError("Path selected")):
            with self.assertRaises(subject.frozen.Refused):
                subject.run("unselected", "unselected", "0" * 64, "0" * 64, "unselected")

    @unittest.skipUnless(os.name == "nt", "Native Windows fences")
    def test_actual_original_capture_produces_exclusive_current_matrix(self):
        with tempfile.TemporaryDirectory(prefix="m0-data-policy-test-") as temporary:
            output = Path(temporary) / "matrix"
            result = subject.run(str(REFERENCE / "manifest.json"), str(REFERENCE / "source.zip"), MANIFEST_SHA, ARCHIVE_SHA, str(output))
            data = (output / "matrix.json").read_bytes()
            self.assertEqual(sha(data), result["matrix_sha256"])
            self.assertEqual(json.loads(data)["source_capture"]["archive_sha256"], ARCHIVE_SHA)
            with self.assertRaises(subject.frozen.Refused):
                subject.run(str(REFERENCE / "manifest.json"), str(REFERENCE / "source.zip"), MANIFEST_SHA, ARCHIVE_SHA, str(output))
            self.assertEqual((output / "matrix.json").read_bytes(), data)
            os.chmod(output / "matrix.json", 0o600)

    @contextlib.contextmanager
    def private_capture(self, duplicate=False, changed=False):
        with tempfile.TemporaryDirectory(prefix="m0-data-policy-capture-") as temporary:
            root = Path(temporary)
            captured = root / "capture"
            captured.mkdir()
            archive = captured / "source.zip"
            records = []
            with zipfile.ZipFile(archive, "w") as zipped:
                for name, raw in self.raw_sources.items():
                    payload = raw + b"\n// changed fixture" if changed else raw
                    zipped.writestr(name, payload)
                    records.append({"path": name, "size": len(payload), "sha256": sha(payload)})
                if duplicate:
                    name = next(iter(self.raw_sources))
                    with __import__("warnings").catch_warnings():
                        __import__("warnings").simplefilter("ignore", UserWarning)
                        zipped.writestr(name, self.raw_sources[name])
            manifest = captured / "manifest.json"
            manifest.write_text(json.dumps({"schema": 2, "archive_sha256": sha(archive.read_bytes()), "files": records}), encoding="utf-8")
            yield root, manifest, archive, sha(manifest.read_bytes()), sha(archive.read_bytes())

    @unittest.skipUnless(os.name == "nt", "Native Windows fences")
    def test_resealed_changed_or_duplicate_archive_is_refused_without_output(self):
        for duplicate, changed in [(True, False), (False, True)]:
            with self.private_capture(duplicate, changed) as (root, manifest, archive, manifest_sha, archive_sha):
                output = root / "refused"
                with self.assertRaises(subject.frozen.Refused):
                    subject.run(str(manifest), str(archive), manifest_sha, archive_sha, str(output))
                self.assertFalse(output.exists())

    @unittest.skipUnless(os.name == "nt", "Native Windows fences")
    def test_actual_source_write_and_rename_are_denied_until_publication(self):
        with self.private_capture() as (root, manifest, archive, manifest_sha, archive_sha):
            original = subject.matrix
            def during_capture(sources):
                for path in (manifest, archive):
                    with self.assertRaises(OSError):
                        with path.open("r+b"):
                            self.fail("Source writer admitted")
                    with self.assertRaises(OSError):
                        path.rename(root / (path.name + ".moved"))
                return original(sources)
            output = root / "matrix"
            with patch.object(subject, "matrix", side_effect=during_capture):
                subject.run(str(manifest), str(archive), manifest_sha, archive_sha, str(output))
            self.assertEqual(sha(archive.read_bytes()), archive_sha)
            self.assertEqual(sha(manifest.read_bytes()), manifest_sha)
            os.chmod(output / "matrix.json", 0o600)

    @unittest.skipUnless(os.name == "nt", "Native Windows fences")
    def test_native_hardlink_and_wrong_capture_pin_are_refused(self):
        with self.private_capture() as (root, manifest, archive, manifest_sha, archive_sha):
            with self.assertRaises(subject.frozen.Refused):
                subject.run(str(manifest), str(archive), "0" * 64, archive_sha, str(root / "wrong-pin"))
            os.link(archive, root / "source-hardlink.zip")
            with self.assertRaises(subject.frozen.Refused):
                subject.run(str(manifest), str(archive), manifest_sha, archive_sha, str(root / "hardlinked"))

if __name__ == "__main__":
    unittest.main()
