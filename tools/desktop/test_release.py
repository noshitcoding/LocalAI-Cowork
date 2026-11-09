"""Counterexamples for release sidecar selection and write-once publication."""
import json
import os
from pathlib import Path
import re
import struct
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import release


HOST = "x86_64-pc-windows-msvc" if os.name == "nt" else "x86_64-unknown-linux-gnu"
EXE = "cowork-daemon.exe" if os.name == "nt" else "cowork-daemon"


def executable(host=HOST):
    data = bytearray(256)
    if host == "x86_64-pc-windows-msvc":
        data[:2] = b"MZ"
        struct.pack_into("<I", data, 60, 128)
        data[128:132] = b"PE\0\0"
        struct.pack_into("<H", data, 132, 0x8664)
        struct.pack_into("<H", data, 150, 2)
        struct.pack_into("<H", data, 152, 0x20B)
    else:
        data[:6] = b"\x7fELF\x02\x01"
        struct.pack_into("<H", data, 16, 3)
        struct.pack_into("<H", data, 18, 62)
    return bytes(data)


class ReleaseArtifactTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.target = self.root / "target"
        self.binary = self.target / HOST / "release" / EXE
        self.binary.parent.mkdir(parents=True)
        self.binary.write_bytes(executable())
        self.item = {"reason": "compiler-artifact", "package_id": "owned-daemon",
                     "target": {"name": "cowork-daemon", "kind": ["bin"]},
                     "executable": str(self.binary),
                     "profile": {"opt_level": "3", "debug_assertions": False,
                                 "overflow_checks": False, "test": False}}

    def messages(self, *items, success=True):
        return "\n".join(json.dumps(i) for i in [*items, {"reason": "build-finished", "success": success}])

    def select(self, messages):
        return release.select_artifact(messages, "owned-daemon", HOST, self.target)

    def test_cargo_reported_native_release_is_selected(self):
        self.assertEqual(self.select(self.messages(self.item)), self.binary)

    def test_stale_binary_after_failed_build_cannot_be_selected(self):
        with self.assertRaises(release.Refused):
            self.select(self.messages(self.item, success=False))

    def test_debug_profile_and_debug_directory_are_rejected(self):
        self.item["profile"]["debug_assertions"] = True
        with self.assertRaises(release.Refused):
            self.select(self.messages(self.item))
        self.item["profile"]["debug_assertions"] = False
        debug = self.target / HOST / "debug" / EXE
        debug.parent.mkdir()
        debug.write_bytes(executable())
        self.item["executable"] = str(debug)
        with self.assertRaises(release.Refused):
            self.select(self.messages(self.item))

    def test_ambiguous_or_foreign_package_cannot_supply_daemon(self):
        with self.assertRaises(release.Refused):
            self.select(self.messages(self.item, self.item))
        self.item["package_id"] = "another-workspace-daemon"
        with self.assertRaises(release.Refused):
            self.select(self.messages(self.item))

    def test_wrong_architecture_and_non_executable_are_rejected(self):
        for host in release.SUPPORTED_HOSTS:
            wrong = bytearray(executable(host))
            struct.pack_into("<H", wrong, 132 if host.endswith("msvc") else 18, 0xAA64 if host.endswith("msvc") else 183)
            self.binary.write_bytes(wrong)
            with self.assertRaises(release.Refused):
                release.validate_executable(self.binary, host)
        self.binary.write_bytes(b"not an executable")
        with self.assertRaises(release.Refused):
            release.validate_executable(self.binary, HOST)

    def test_cargo_hardlinked_artifact_publishes_an_independent_copy(self):
        os.link(self.binary, self.root / "another-name")
        self.assertEqual(self.select(self.messages(self.item)), self.binary)
        receipt = self.prepare(lambda args, **_: subprocess.CompletedProcess(args, 0, self.messages(self.item)))
        staged = self.root / "prepared" / receipt["artifact"]["filename"]
        self.assertEqual(receipt["cargo_artifact_hardlink_count"], 2)
        self.assertEqual(staged.stat().st_nlink, 1)
        self.binary.write_bytes(executable() + b"later build mutation")
        self.assertEqual(release.sha256(staged), receipt["artifact"]["sha256"])

    def fixture_sources(self):
        (self.root / "extensions/browser").mkdir(parents=True,exist_ok=True)
        (self.root / "extensions/browser/identity.json").write_text("{}",encoding="utf-8")
        for name in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml"):
            (self.root / name).write_text("fixture", encoding="utf-8")
        for directory in ("crates", "services/daemon", "tools/contracts", "tools/desktop", "tools/release", "contracts", "deploy/v2", "docs"):
            (self.root / directory).mkdir(parents=True, exist_ok=True)
        (self.root / "services/daemon/Cargo.toml").write_text("fixture", encoding="utf-8")
        (self.root / "services/daemon/main.rs").write_text("original", encoding="utf-8")
        (self.root / "tools/desktop/release.py").write_bytes(Path(release.__file__).read_bytes())
        for name in ("sbom.py", "runtime_inventory.py", "container_inventory.py", "source_bundle.py", "delivery.py"):
            (self.root / "tools/release" / name).write_text("fixture", encoding="utf-8")
        (self.root / "tools/private_crew_setup.py").write_text("fixture", encoding="utf-8")
        (self.root / "docs/DEVICE-AGENT-V2.md").write_text("fixture", encoding="utf-8")

    def check_output(self, args, **_):
        if args[0] == "rustc":
            return "rustc fixture\nhost: " + HOST + "\n"
        if args[1] == "--version":
            return "cargo fixture\n"
        return json.dumps({"packages": [{"name": "cowork-daemon", "id": "owned-daemon",
                                         "version": "fixture", "manifest_path": str(self.root / "services/daemon/Cargo.toml")} ]})

    def prepare(self, build):
        self.fixture_sources()
        with patch.dict(os.environ, {}, clear=True), patch.object(subprocess, "check_output", self.check_output), patch.object(subprocess, "run", build):
            return release.prepare(self.root, self.root / "prepared", self.target, True)

    def test_source_drift_during_build_publishes_nothing(self):
        def build(args, **_):
            (self.root / "services/daemon/main.rs").write_text("changed", encoding="utf-8")
            return subprocess.CompletedProcess(args, 0, self.messages(self.item))
        with self.assertRaises(release.Refused):
            self.prepare(build)
        self.assertFalse((self.root / "prepared").exists())

    def test_historical_embedded_and_fixture_bytes_are_all_pinned(self):
        self.fixture_sources()
        names = [
            "crates/storage/src/historical_imports/verifier/data/source.sqlite3",
            "crates/storage/src/historical_imports/verifier/data/profiles.json",
            "crates/storage/tests/fixtures/v1_history_verifier/bundle/export.jsonl",
            "crates/storage/tests/fixtures/v1_history_verifier/bundle/pins",
        ]
        for name in names:
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(("pinned synthetic input " + name).encode("utf-8"))
        before = release.source_manifest(self.root)
        for name in names:
            self.assertEqual(before[name]["sha256"], release.sha256(self.root / name))
        (self.root / names[0]).write_bytes(b"changed SQLite seed")
        after = release.source_manifest(self.root)
        self.assertNotEqual(before[names[0]], after[names[0]])
        self.assertTrue(all(before[name] == after[name] for name in names[1:]))

    def test_actual_embedded_toolbox_and_passkey_closure_is_pinned_and_drift_refuses_publication(self):
        self.fixture_sources()
        actual_root = Path(release.__file__).resolve().parents[2]
        inputs = set()
        for caller in ("crates/toolbox/src/lib.rs", "services/daemon/src/android_passkey_routes.rs"):
            original = actual_root / caller
            target = self.root / caller
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(original.read_bytes())
            for included in re.findall(r'include_(?:bytes|str)!\s*\(\s*"([^"]+)"', original.read_text(encoding="utf-8")):
                source = (original.parent / included).resolve()
                relative = source.relative_to(actual_root)
                copied = self.root / relative
                copied.parent.mkdir(parents=True, exist_ok=True)
                copied.write_bytes(source.read_bytes())
                inputs.add(relative.as_posix())
        adapter = "crates/toolbox/src/persistent_browser.cjs"
        passkey = "services/daemon/src/android_native_passkey.js"
        self.assertIn(adapter, inputs)
        self.assertIn(passkey, inputs)
        before = release.source_manifest(self.root)
        for name in inputs:
            self.assertEqual(before[name]["sha256"], release.sha256(self.root / name))
        (self.root / adapter).write_bytes((self.root / adapter).read_bytes() + b"\n// changed embedded adapter\n")
        after = release.source_manifest(self.root)
        self.assertNotEqual(before[adapter], after[adapter])
        self.assertTrue(all(before[name] == after[name] for name in inputs - {adapter}))
        def build(args, **_):
            script = self.root / passkey
            script.write_bytes(script.read_bytes() + b"\n// changed embedded passkey\n")
            return subprocess.CompletedProcess(args, 0, self.messages(self.item))
        with self.assertRaisesRegex(release.Refused, "source inputs changed"):
            self.prepare(build)
        self.assertFalse((self.root / "prepared").exists())

    def test_historical_seed_drift_during_build_publishes_nothing(self):
        seed = self.root / "crates/storage/src/historical_imports/verifier/data/source.sqlite3"
        seed.parent.mkdir(parents=True)
        seed.write_bytes(b"original synthetic seed")
        def build(args, **_):
            seed.write_bytes(b"changed synthetic seed")
            return subprocess.CompletedProcess(args, 0, self.messages(self.item))
        with self.assertRaises(release.Refused):
            self.prepare(build)
        self.assertFalse((self.root / "prepared").exists())

    def test_artifact_drift_during_copy_publishes_nothing(self):
        original = release.shutil.copyfile
        def copy(source, target):
            result = original(source, target)
            source.write_bytes(executable() + b"changed")
            return result
        with patch.object(release.shutil, "copyfile", copy), self.assertRaises(release.Refused):
            self.prepare(lambda args, **_: subprocess.CompletedProcess(args, 0, self.messages(self.item)))
        self.assertFalse((self.root / "prepared").exists())
        self.assertFalse(list(self.root.glob(".sidecar-preparing-*")))

    def test_successful_preparation_receipt_and_binary_agree_without_release_claim(self):
        receipt = self.prepare(lambda args, **_: subprocess.CompletedProcess(args, 0, self.messages(self.item)))
        output = self.root / "prepared"
        saved = json.loads((output / "receipt.json").read_text())
        self.assertEqual(saved, receipt)
        self.assertEqual(release.sha256(output / saved["artifact"]["filename"]), saved["artifact"]["sha256"])
        self.assertFalse(saved["release_qualified"])
        self.assertFalse(saved["signed"])
        with patch.object(subprocess, "run") as build, self.assertRaises(release.Refused):
            release.prepare(self.root, output, self.target)
        build.assert_not_called()
        self.assertEqual(json.loads((output / "receipt.json").read_text()), receipt)


if __name__ == "__main__":
    unittest.main()
