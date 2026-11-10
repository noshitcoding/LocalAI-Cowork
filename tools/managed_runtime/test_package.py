"""Actual producer/native-verifier integration with public deterministic vectors."""
from pathlib import Path
import importlib.util
import json
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("runtime_package_builder", Path(__file__).with_name("package.py"))
builder = importlib.util.module_from_spec(spec)
spec.loader.exec_module(builder)
VECTOR = json.loads((ROOT / "crates/managed-runtime/tests/fixtures/python-package.json").read_text())
VERIFIER = ROOT / "target/debug" / ("cowork-runtime-package.exe" if sys.platform == "win32" else "cowork-runtime-package")


class PackageProduction(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="cowork-package-builder-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.source = self.root / "source.payload"
        self.source.write_bytes(b"test")
        self.index = json.loads(VECTOR["manifest_json"])
        for component in self.index["components"]:
            component["source"] = str(self.source)
        self.index_path = self.root / "index.json"
        self.index_path.write_text(json.dumps(self.index))
        self.output = self.root / "package"
        self.key = self.root / "external.pub"
        self.key.write_bytes(bytes(VECTOR["public_key"]))
        self.signature = self.root / "release.sig"
        self.signature.write_bytes(bytes(VECTOR["prepared_signature"]))

    def test_prepared_bytes_match_independent_signature_and_native_seal(self):
        receipt = builder.prepare(self.index_path, self.output)
        self.assertFalse(receipt["signed"])
        self.assertFalse(receipt["qualified"])
        self.assertEqual((self.output / "manifest.json").read_bytes(), VECTOR["prepared_manifest_json"].encode())
        self.assertNotIn(str(self.root), (self.output / "manifest.json").read_text())
        final = builder.seal(self.output, self.signature, "python-fixture", self.key, VERIFIER)
        self.assertTrue(final["signature_verified"])
        self.assertTrue(final["payload_verified"])
        self.assertFalse(final["installed"])
        self.assertFalse(final["qualified"])
        self.assertFalse(final["execution_authority"])
        self.assertEqual(self.source.read_bytes(), b"test")

    def test_changed_source_cannot_finish_a_prepared_manifest(self):
        self.source.write_bytes(b"fail")
        with self.assertRaises(ValueError):
            builder.prepare(self.index_path, self.output)
        self.assertFalse((self.output / "manifest.json").exists())
        self.assertFalse((self.output / "manifest.sig.json").exists())
        self.assertEqual(self.source.read_bytes(), b"fail")

    def test_native_verifier_rejects_payload_tampering_after_preparation(self):
        builder.prepare(self.index_path, self.output)
        (self.output / "payload/base_rootfs.payload").write_bytes(b"fail")
        with self.assertRaisesRegex(ValueError, "verification rejected"):
            builder.seal(self.output, self.signature, "python-fixture", self.key, VERIFIER)
        self.assertEqual(self.source.read_bytes(), b"test")

    def test_package_cannot_choose_its_own_public_trust_key(self):
        builder.prepare(self.index_path, self.output)
        inside = self.output / "self.pub"
        inside.write_bytes(bytes(VECTOR["public_key"]))
        with self.assertRaisesRegex(ValueError, "Trust root"):
            builder.seal(self.output, self.signature, "python-fixture", inside, VERIFIER)
        self.assertFalse((self.output / "manifest.sig.json").exists())


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--verifier":
        VERIFIER = Path(sys.argv[2]).resolve(strict=True)
        sys.argv[1:] = []
    unittest.main()
