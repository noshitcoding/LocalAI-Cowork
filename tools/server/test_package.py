"""Counterexamples for bundle integrity and private no-service installation.

The tiny ELF header below is a synthetic format fixture and is never executed.
Actual optimized daemon execution is qualified separately on actual release bytes.
"""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("server_package", Path(__file__).with_name("package.py"))
package = importlib.util.module_from_spec(spec)
assert spec.loader
spec.loader.exec_module(package)


class BundleIntegrity(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="cowork-own-server-package-")
        self.base = Path(self.temp.name)
        self.root = self.base / "repo"
        self.root.mkdir()
        (self.root / "LICENSE").write_bytes((package.ROOT / "LICENSE").read_bytes())
        for name in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml"):
            (self.root / name).write_text("private synthetic source fixture\n")
        for directory in ("crates", "services", "tools/contracts", "contracts", "tools/desktop", "tools/server"):
            (self.root / directory).mkdir(parents=True, exist_ok=True)
        (self.root / "tools/desktop/release.py").write_bytes((package.ROOT / "tools/desktop/release.py").read_bytes())
        for name in package.HELPERS:
            (self.root / "tools/server" / name).write_bytes((package.ROOT / "tools/server" / name).read_bytes())
        self.embedded = self.root / "crates/storage/src/historical_imports/verifier/data/fixed-input"
        self.embedded.parent.mkdir(parents=True)
        self.embedded.write_bytes(b"private compiler input including no suffix")
        self.sqlite = self.root / "crates/storage/tests/fixtures/v1_history_verifier/test.sqlite"
        self.sqlite.parent.mkdir(parents=True)
        self.sqlite.write_bytes(b"private synthetic corpus bytes")
        self.web = self.base / "web"
        self.web.mkdir(); (self.web / "index.html").write_text("<main>private static fixture</main>")
        self.prepared = self.base / "prepared"
        self.prepared.mkdir()
        header = bytearray(64); header[:6] = b"\x7fELF\x02\x01"
        struct.pack_into("<H", header, 16, 3); struct.pack_into("<H", header, 18, 62)
        self.binary = self.prepared / ("cowork-daemon-" + package.HOST)
        self.binary.write_bytes(header)
        self.original_root = package.ROOT
        package.ROOT = self.root
        self.receipt = {"format":"localai-cowork-native-release-sidecar/1", "host":package.HOST, "profile":"release", "cargo_exit_code":0, "source_before_after_equal":True,
            "artifact":{"filename":self.binary.name,"size":64,"sha256":package.release.sha256(self.binary)}, "sources":package.release.source_manifest(self.root)}
        self.save_receipt()
        self.output = self.base / "new-bundle"
    def tearDown(self):
        package.ROOT = self.original_root
        self.temp.cleanup()
    def save_receipt(self):
        (self.prepared / "receipt.json").write_text(json.dumps(self.receipt))
    def make(self):
        return package.prepare(self.prepared,self.web,self.output)
    def test_exact_package_is_checksummed_and_does_not_start_service(self):
        result = self.make()
        self.assertFalse(result["service_started"]); self.assertFalse(result["installed"])
        self.assertFalse(result["web_build_or_browser_qualification_claim"])
        self.assertEqual(result["bind"],"127.0.0.1")
        self.assertEqual((self.output / "bin/cowork-daemon").read_bytes(),self.binary.read_bytes())
        self.assertEqual((self.output / "LICENSE").read_bytes(),(self.root / "LICENSE").read_bytes())
        self.assertEqual(result["project_license"]["sha256"],package.release.sha256(self.root / "LICENSE"))
        for line in (self.output / "SHA256SUMS").read_text().splitlines():
            digest,name = line.split("  ",1)
            self.assertEqual(package.release.sha256(self.output / name),digest)
    def test_artifact_host_hash_profile_and_existing_output_fail_closed(self):
        for field,bad in (("host","x86_64-pc-windows-msvc"),("profile","debug"),("cargo_exit_code",1),("source_before_after_equal",False)):
            original = self.receipt[field]; self.receipt[field] = bad; self.save_receipt()
            with self.assertRaises(package.release.Refused): self.make()
            self.assertFalse(self.output.exists());self.receipt[field]=original
        self.save_receipt(); self.binary.write_bytes(b"changed artifact")
        with self.assertRaises(package.release.Refused): self.make()
        self.assertFalse(self.output.exists())
    def test_complete_embedded_and_sqlite_corpus_changes_invalidate_release(self):
        for source in (self.embedded,self.sqlite):
            original = source.read_bytes();source.write_bytes(original+b"changed")
            with self.assertRaisesRegex(package.release.Refused,"source closure"):self.make()
            self.assertFalse(self.output.exists());source.write_bytes(original)
    def test_mutation_during_copy_never_emits_completed_receipt(self):
        original_copy = shutil.copyfile
        def mutate(source,target,*args,**kwargs):
            result = original_copy(source,target,*args,**kwargs)
            if Path(source) == self.binary:self.embedded.write_bytes(b"changed during actual copy")
            return result
        with patch.object(package.shutil,"copyfile",side_effect=mutate):
            with self.assertRaisesRegex(package.release.Refused,"changed during"):self.make()
        self.assertFalse((self.output / "bundle-receipt.json").exists())
        self.assertFalse((self.output / "SHA256SUMS").exists())
    def test_aliases_and_existing_destination_are_refused(self):
        alias=self.base / "alias";os.link(self.binary,alias)
        with self.assertRaises(package.release.Refused):self.make()
        alias.unlink();self.output.mkdir()
        with self.assertRaises(package.release.Refused):self.make()
    def test_real_project_license_drift_during_copy_never_emits_receipt(self):
        original_copy=shutil.copyfile
        def mutate(source,target,*args,**kwargs):
            result=original_copy(source,target,*args,**kwargs)
            if Path(source)==self.root / "LICENSE":
                (self.root / "LICENSE").write_bytes((self.root / "LICENSE").read_bytes()+b"changed during copy")
            return result
        with patch.object(package.shutil,"copyfile",side_effect=mutate):
            with self.assertRaisesRegex(package.release.Refused,"changed during"):self.make()
        self.assertFalse((self.output / "bundle-receipt.json").exists())
        self.assertFalse((self.output / "SHA256SUMS").exists())
    @unittest.skipUnless(os.name=="posix","actual Linux shell installation")
    def test_actual_private_installer_preserves_existing_and_bad_checksum_starts_nothing(self):
        self.make(); installed=self.base / "own-installed"
        process=subprocess.Popen(["bash",str(self.output / "install-user.sh"),str(installed)],stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
        output,_=process.communicate(timeout=10);self.assertEqual(process.wait(timeout=1),0,output.decode())
        baseline={p.relative_to(installed).as_posix():p.read_bytes() for p in installed.rglob("*") if p.is_file()}
        process=subprocess.Popen(["bash",str(self.output / "install-user.sh"),str(installed)],stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
        process.communicate(timeout=10);self.assertNotEqual(process.wait(timeout=1),0)
        self.assertEqual(baseline,{p.relative_to(installed).as_posix():p.read_bytes() for p in installed.rglob("*") if p.is_file()})
        (self.output / "bin/cowork-daemon").write_bytes(b"changed checksum")
        data=self.base / "own-not-created-data"
        env=dict(os.environ,COWORK_DATA_DIR=str(data))
        process=subprocess.Popen(["bash",str(self.output / "start.sh")],env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
        process.communicate(timeout=10);self.assertNotEqual(process.wait(timeout=1),0)
        self.assertFalse(data.exists())
        print("Actual private installer Popen waits0/reject; checksum stop before daemon/data creation",flush=True)


if __name__ == "__main__":
    unittest.main(verbosity=2)
