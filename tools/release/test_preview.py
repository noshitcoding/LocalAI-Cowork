"""Synthetic counterexamples for publication gates; these do not qualify a release."""
from __future__ import annotations

import base64
import json
import hashlib
from pathlib import Path
import tempfile
import subprocess
import unittest
import zipfile
from unittest.mock import patch

import draft_preview
import preview
import preview_trust
import release_checks as checks
import update_feed

VERSION = "2.0.0-alpha.8"
TAG = "v" + VERSION
REPOSITORY = "owner/cowork"
BASE = f"https://github.com/{REPOSITORY}/releases/download/{TAG}"
ENDPOINT = "https://owner.github.io/cowork/updates/v2/preview.json"


def write(path, value):
    path.write_text(json.dumps(value), encoding="utf-8")


class PreviewTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        # Ephemeral test-only keys never touch the filesystem or a release profile.
        script = """const c=require('node:crypto'),fs=require('node:fs');
const input=JSON.parse(fs.readFileSync(0,'utf8')),pair=c.generateKeyPairSync('ed25519');
const id=Buffer.from('testkey1'),raw=pair.publicKey.export({format:'der',type:'spki'}).subarray(-32);
const pub=Buffer.from('untrusted comment: test fixture\\n'+Buffer.concat([Buffer.from('Ed'),id,raw]).toString('base64')+'\\n').toString('base64');
const signatures={};for(const [kind,digest] of Object.entries(input)){
const comment='timestamp:0\\tfile:fixture\\tversion:2.0.0-alpha.8';
const sig=c.sign(null,Buffer.from(digest,'hex'),pair.privateKey);
const global=c.sign(null,Buffer.concat([sig,Buffer.from(comment)]),pair.privateKey);
signatures[kind]=Buffer.from('untrusted comment: test fixture\\n'+Buffer.concat([Buffer.from('ED'),id,sig]).toString('base64')+'\\ntrusted comment: '+comment+'\\n'+global.toString('base64')+'\\n').toString('base64');}
process.stdout.write(JSON.stringify({public:pub,signatures}));
"""
        payloads = {kind: hashlib.blake2b(("synthetic " + kind).encode()).hexdigest() for kind in ("nsis", "appimage")}
        cls.signing_fixture = json.loads(subprocess.check_output(["node", "-e", script], input=json.dumps(payloads), text=True))

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.candidate = self.root / "candidate"
        self.candidate.mkdir()
        self.output = self.root / "public"
        self.archive = self.root / "candidate.zip"
        self.trusted = {"format": "localai-cowork-preview-trust/1", "enabled": True,
                        "repository": REPOSITORY, "updater_endpoint": ENDPOINT,
                        "updater_key_sha256": hashlib.sha256(self.signing_fixture["public"].encode()).hexdigest(),
                        "windows_certificate_thumbprint": "D" * 40,
                        "candidate_workflow": ".github/workflows/candidate.yml"}
        self.policy_path = self.root / "trusted-policy.json"
        write(self.policy_path, self.trusted)
        self.source = {"format": "localai-cowork-preview-source/1", "version": VERSION, "tag": TAG,
                       "commit": "a" * 40, "files": {name: {"size": 1, "sha256": "b" * 64}
                           for name in ("Cargo.toml", "package.json", "tools/desktop/frontend.py")}}
        self.source["source_sha256"] = preview.canonical(self.source["files"])
        self.core = {"Cargo.toml": self.source["files"]["Cargo.toml"]}
        self.outputs = {"apps/web/dist/index.html": {"size": 2, "sha256": "c" * 64}}
        self.packages = {}
        for host in checks.HOSTS:
            folder = self.candidate / host
            folder.mkdir()
            bundles = {}
            files = []
            for kind in checks.HOSTS[host][1]:
                name = "fixture " + VERSION + {"nsis": ".exe", "deb": ".deb", "appimage": ".AppImage"}[kind]
                path = folder / name
                path.write_bytes(("synthetic " + kind).encode())
                bundles[kind] = {"filename": name, **checks.pin(path)}
                files.append({"fileName": "./artifact/" + name,
                              "checksums": [{"algorithm": "SHA256", "checksumValue": checks.pin(path)["sha256"]}]})
                if kind in {"nsis", "appimage"}:
                    sig = folder / (name + ".sig")
                    sig.write_text(self.signing_fixture["signatures"][kind], encoding="utf-8")
                    bundles[kind]["updater_signature"] = {"filename": sig.name, **checks.pin(sig)}
            write(folder / "sbom.spdx.json", {"spdxVersion": "SPDX-2.3", "SPDXID": "SPDXRef-DOCUMENT", "files": files})
            receipt = {"format": "localai-cowork-native-desktop-package/1", "result": "passed", "signed": True,
                       "product_version": VERSION, "host": host, "bundles": bundles,
                       "sources": {**self.core, "package.json": self.source["files"]["package.json"], **self.outputs}, "source_before_after_equal": True,
                       "staged_source_bytes_equal": True, "desktop_format_test_clippy_exit_codes": [0, 0, 0],
                       "validation_scope": "format_tests_clippy_build", "native_process_scope": {"own_job_empty": True},
                       "sbom": {"filename": "sbom.spdx.json", **checks.pin(folder / "sbom.spdx.json")},
                       "signing_profile": {"updater_public_key": self.signing_fixture["public"], "updater_endpoint": ENDPOINT,
                                           "windows_certificate_thumbprint": "D" * 40}}
            if "nsis" in bundles:
                receipt["native_installer_contract"] = {"contract": "nsis.currentUser.originalProcess.v1",
                    "product_version": VERSION, **{key: bundles["nsis"][key] for key in ("size", "sha256")}}
            write(folder / "receipt.json", receipt)
            self.packages[host] = folder
        self.save_candidate()

    def save_candidate(self):
        write(self.candidate / "source-provenance.json", self.source)
        write(self.candidate / "frontend.json", {"format": "localai-cowork-frontend-build/1", "version": VERSION,
            "exit_code": 0, "sources_unchanged": True, "outputs": self.outputs,
            "sources": {name: self.source["files"][name] for name in ("package.json", "tools/desktop/frontend.py")}})
        log = self.candidate / "acceptance.log"
        log.write_text("synthetic acceptance log\n", encoding="utf-8")
        acceptance = {"format": "localai-cowork-preview-installed-acceptance/1",
                      **{key: self.source[key] for key in ("version", "commit", "source_sha256")}, "platforms": {}}
        for host, folder in self.packages.items():
            receipt = checks.read(folder / "receipt.json")
            acceptance["platforms"][host] = {"receipt_sha256": checks.pin(folder / "receipt.json")["sha256"],
                "packages": {kind: record["sha256"] for kind, record in receipt["bundles"].items()},
                "checks": {key: {"result": "passed", "evidence": "acceptance.log", "sha256": checks.pin(log)["sha256"]}
                           for key in preview.ACCEPTANCE_CHECKS}}
        write(self.candidate / "acceptance.json", acceptance)
        (self.candidate / "notes.md").write_text("Preview " + VERSION + ": synthetic test; no qualification", encoding="utf-8")
        write(self.candidate / "candidate.json", {"format": "localai-cowork-preview-candidate/1",
            "frontend_receipt": "frontend.json", "notes": "notes.md", "acceptance": "acceptance.json",
            "packages": {host: folder.name for host, folder in self.packages.items()}})

    def mutate(self, folder, callback):
        receipt = checks.read(folder / "receipt.json")
        callback(receipt)
        write(folder / "receipt.json", receipt)

    def stage(self):
        with zipfile.ZipFile(self.archive, "w", zipfile.ZIP_DEFLATED) as archive:
            for path in self.candidate.rglob("*"):
                if path.is_file():
                    archive.write(path, path.relative_to(self.candidate).as_posix())
        producer = {"run_id": 2, "artifact_id": 3, "artifact_digest": "sha256:" + checks.pin(self.archive)["sha256"],
                    "producer_workflow": self.trusted["candidate_workflow"], "head_sha": self.source["commit"]}
        with patch.object(preview, "source_snapshot", return_value=self.source), \
             patch.object(preview.native_release, "source_manifest", return_value=self.core), \
             patch.object(preview, "live_ci", return_value={"id": 1, "conclusion": "success"}), \
             patch.object(preview_trust, "live_producer", return_value=producer), \
             patch.object(preview, "command", return_value=json.dumps({"sha": self.source["commit"]})):
            return preview.stage(self.root, self.candidate, self.output, TAG, REPOSITORY, 1,
                                 self.policy_path, 2, 3, self.archive)

    def feed(self, packages, version, base_url, notes=""):
        return update_feed.generate(packages, version, base_url, notes, trusted_policy=self.trusted)

    def test_stage_curates_public_assets_and_draft_cannot_become_latest(self):
        plan = self.stage()
        self.assertEqual(plan["make_latest"], False)
        self.assertFalse((self.output / "acceptance.log").exists())
        self.assertEqual(set(self.feed(list(self.packages.values()), VERSION, BASE)["platforms"]),
                         {"windows-x86_64", "linux-x86_64"})
        command = draft_preview.draft_command(self.output)
        for flag in ("--draft", "--prerelease", "--latest=false", "--verify-tag"):
            self.assertIn(flag, command)

    def test_tampered_public_asset_cannot_be_uploaded(self):
        self.stage()
        (self.output / "RELEASE-NOTES.md").write_text("changed")
        with self.assertRaisesRegex(ValueError, "asset bytes differ"):
            draft_preview.draft_command(self.output)

    def test_draft_plan_rejects_stable_latest(self):
        self.stage()
        plan = checks.read(self.output / "release-plan.json")
        plan["make_latest"] = True
        write(self.output / "release-plan.json", plan)
        with self.assertRaisesRegex(ValueError, "draft plan"):
            draft_preview.draft_command(self.output)

    def test_assets_cannot_escape_package_directory(self):
        folder = next(iter(self.packages.values()))
        self.mutate(folder, lambda value: value["bundles"]["nsis"].update(filename="../secret.exe"))
        with self.assertRaisesRegex(ValueError, "single filenames"):
            self.feed([folder], VERSION, BASE)

    def test_unsigned_unknown_host_wrong_version_and_fast_gates_are_refused(self):
        folder = next(iter(self.packages.values()))
        original = (folder / "receipt.json").read_bytes()
        mutations = [{"signed": False}, {"host": "aarch64-unknown-linux-gnu"},
                     {"product_version": "2.0.0-alpha.7"}, {"source_before_after_equal": False},
                     {"desktop_format_test_clippy_exit_codes": None, "validation_scope": "build_only"}]
        for mutation in mutations:
            with self.subTest(mutation=mutation):
                (folder / "receipt.json").write_bytes(original)
                self.mutate(folder, lambda value: value.update(mutation))
                with self.assertRaises(ValueError):
                    checks.checked_package(folder, VERSION, strict_preview=True, trusted_policy=self.trusted)

    def test_sbom_payload_omission_is_refused_even_with_matching_sbom_pin(self):
        folder = next(iter(self.packages.values()))
        sbom_path = folder / "sbom.spdx.json"
        value = checks.read(sbom_path)
        value["files"] = []
        write(sbom_path, value)
        self.mutate(folder, lambda value: value.update(sbom={"filename": sbom_path.name, **checks.pin(sbom_path)}))
        with self.assertRaisesRegex(ValueError, "SPDX does not bind"):
            self.feed([folder], VERSION, BASE)

    def test_stable_feed_latest_alias_duplicate_platform_and_different_identity_are_refused(self):
        folders = list(self.packages.values())
        with self.assertRaisesRegex(ValueError, "immutable version"):
            self.feed(folders, VERSION, "https://github.com/owner/repo/releases/latest/download")
        with self.assertRaisesRegex(ValueError, "Duplicate update platform"):
            self.feed([folders[0], folders[0]], VERSION, BASE)
        self.mutate(folders[1], lambda value: value["signing_profile"].update(updater_public_key="other identity"))
        with self.assertRaises(ValueError):
            self.feed(folders, VERSION, BASE)
        self.mutate(folders[0], lambda value: value["signing_profile"].update(updater_endpoint="https://example.com/latest.json"))
        with self.assertRaisesRegex(ValueError, "isolated"):
            self.feed([folders[0]], VERSION, BASE)

    def test_installed_acceptance_cannot_be_reused_for_different_packages(self):
        acceptance = checks.read(self.candidate / "acceptance.json")
        host = next(iter(acceptance["platforms"]))
        acceptance["platforms"][host]["receipt_sha256"] = "0" * 64
        write(self.candidate / "acceptance.json", acceptance)
        with self.assertRaisesRegex(ValueError, "different package receipt"):
            self.stage()
        self.assertFalse(self.output.exists())

    def test_missing_product_acceptance_does_not_create_public_output(self):
        acceptance = checks.read(self.candidate / "acceptance.json")
        next(iter(acceptance["platforms"].values()))["checks"].pop("restart_recovery")
        write(self.candidate / "acceptance.json", acceptance)
        with self.assertRaisesRegex(ValueError, "required product paths"):
            self.stage()
        self.assertFalse(self.output.exists())

    def test_old_frontend_or_partial_source_receipts_are_refused(self):
        frontend = checks.read(self.candidate / "frontend.json")
        frontend["sources"].pop("package.json")
        write(self.candidate / "frontend.json", frontend)
        with self.assertRaisesRegex(ValueError, "complete tracked frontend"):
            self.stage()

    def test_duplicate_json_and_relative_candidate_traversal_are_refused(self):
        path = self.candidate / "bad.json"
        path.write_text('{"signed":true,"signed":false}')
        with self.assertRaisesRegex(ValueError, "Duplicate"):
            checks.read(path)
        for value in ("../outside", "/outside", "C:/outside", "folder/./file", "folder\\file"):
            with self.subTest(path=value), self.assertRaises(ValueError):
                preview.selected(self.candidate, value)

    def test_matching_receipt_pin_cannot_replace_actual_signature_verification(self):
        folder = next(iter(self.packages.values()))
        value = checks.read(folder / "receipt.json")
        signature = folder / value["bundles"]["nsis"]["updater_signature"]["filename"]
        lines = base64.b64decode(signature.read_text()).decode().splitlines()
        raw = bytearray(base64.b64decode(lines[1]))
        raw[-1] ^= 1
        lines[1] = base64.b64encode(raw).decode()
        signature.write_bytes(base64.b64encode(("\n".join(lines) + "\n").encode()))
        value["bundles"]["nsis"]["updater_signature"].update(checks.pin(signature))
        write(folder / "receipt.json", value)
        with self.assertRaisesRegex(ValueError, "signature verification failed"):
            self.feed([folder], VERSION, BASE)

    def test_upstream_minisign_verify_025_known_vector(self):
        # Independent public vector from minisign-verify 0.2.5's library example.
        key = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3"
        signature = ("untrusted comment: signature from minisign secret key\n"
            "RUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/"
            "z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=\n"
            "trusted comment: timestamp:1633700835\tfile:test\tprehashed\n"
            "wLMDjy9FLAuxZ3q4NlEvkgtyhrr0gtTu6KC4KBJdITbbOeAi1zBIYo0v4iTgt8jJpIidRJnp94ABQkJAgAooBQ==\n")
        path = self.root / "upstream-vector"
        path.write_bytes(b"test")
        sigpath = self.root / "upstream-vector.sig"
        sigpath.write_bytes(base64.b64encode(signature.encode()))
        public = base64.b64encode(("untrusted comment: public vector\n" + key + "\n").encode()).decode()
        checks.verify_updater_signature(path, sigpath, public, VERSION)
        path.write_bytes(b"changed")
        with self.assertRaisesRegex(ValueError, "signature verification failed"):
            checks.verify_updater_signature(path, sigpath, public, VERSION)

    def test_ci_requires_the_exact_successful_workflow_and_both_platforms(self):
        run = {"id": 1, "html_url": "https://github.com/owner/cowork/actions/runs/1", "head_sha": "a" * 40,
               "conclusion": "success", "status": "completed", "path": ".github/workflows/verify.yml",
               "repository": {"full_name": REPOSITORY}}
        jobs = [{"name": f"{group} ({host})", "conclusion": "success"}
                for group in ("core", "desktop", "release-tools", "documents", "office", "document-ui")
                for host in ("windows-latest", "ubuntu-24.04")]
        self.assertEqual(preview.checked_ci(run, jobs, "a" * 40, REPOSITORY)["id"], 1)
        for wrong in ({"head_sha": "b" * 40}, {"path": ".github/workflows/other.yml"}, {"conclusion": "failure"}):
            with self.subTest(run=wrong), self.assertRaises(ValueError):
                preview.checked_ci(dict(run, **wrong), jobs, "a" * 40, REPOSITORY)
        with self.assertRaises(ValueError):
            preview.checked_ci(run, jobs[:-1], "a" * 40, REPOSITORY)

    def test_candidate_cannot_select_its_own_trusted_signer_or_certificate(self):
        folder = next(iter(self.packages.values()))
        wrong = dict(self.trusted, updater_key_sha256="f" * 64)
        with self.assertRaisesRegex(ValueError, "independent release policy"):
            checks.checked_package(folder, VERSION, strict_preview=True, trusted_policy=wrong)
        wrong = dict(self.trusted, windows_certificate_thumbprint="E" * 40)
        with self.assertRaisesRegex(ValueError, "Authenticode identity"):
            checks.checked_package(folder, VERSION, strict_preview=True, trusted_policy=wrong)
        with self.assertRaisesRegex(ValueError, "identity policy"):
            update_feed.generate([folder], VERSION, BASE)
        write(self.policy_path, dict(self.trusted, enabled=False))
        with self.assertRaisesRegex(ValueError, "independently reviewed"):
            preview_trust.policy(self.policy_path, REPOSITORY)

    def test_producer_requires_its_trusted_workflow_exact_head_run_id_and_digest(self):
        run = {"id": 2, "head_sha": self.source["commit"], "conclusion": "success", "status": "completed",
               "path": self.trusted["candidate_workflow"], "repository": {"full_name": REPOSITORY},
               "html_url": "https://github.com/owner/cowork/actions/runs/2"}
        artifact = {"id": 3, "name": "candidate", "expired": False, "digest": "sha256:" + "d" * 64,
                    "workflow_run": {"id": 2, "head_sha": self.source["commit"]}}
        self.assertEqual(preview_trust.checked_producer(run, artifact, self.trusted, self.source["commit"], 2, 3)["artifact_id"], 3)
        for change in ({"path": ".github/workflows/untrusted.yml"}, {"head_sha": "b" * 40}, {"id": 7}):
            with self.subTest(run=change), self.assertRaises(ValueError):
                preview_trust.checked_producer(dict(run, **change), artifact, self.trusted, self.source["commit"], 2, 3)
        for change in ({"digest": None}, {"id": 4}, {"expired": True},
                       {"workflow_run": {"id": 4, "head_sha": self.source["commit"]}}):
            with self.subTest(artifact=change), self.assertRaises(ValueError):
                preview_trust.checked_producer(run, dict(artifact, **change), self.trusted, self.source["commit"], 2, 3)

    def test_archive_digest_and_safe_members_are_independent_of_candidate_receipts(self):
        self.stage()
        with self.assertRaisesRegex(ValueError, "live immutable"):
            preview_trust.checked_archive(self.archive, self.candidate, {"artifact_digest": "sha256:" + "e" * 64})
        malicious = self.root / "traversal.zip"
        with zipfile.ZipFile(malicious, "x") as archive:
            archive.writestr("../outside", "must not escape")
        with self.assertRaisesRegex(ValueError, "Unsafe"):
            preview_trust.archive_files(malicious)

    def test_incomplete_desktop_sources_cannot_hide_behind_complete_core(self):
        self.source["files"]["apps/desktop/src-tauri/src/main.rs"] = {"size": 1, "sha256": "d" * 64}
        self.source["source_sha256"] = preview.canonical(self.source["files"])
        self.save_candidate()
        with self.assertRaisesRegex(ValueError, "complete tracked desktop"):
            self.stage()
        self.assertFalse(self.output.exists())

    def test_candidate_change_after_acceptance_is_refused_before_public_staging(self):
        original = preview.checked_acceptance
        def mutate_after_acceptance(*args):
            result = original(*args)
            (self.candidate / "notes.md").write_text("changed " + VERSION)
            return result
        with patch.object(preview, "checked_acceptance", side_effect=mutate_after_acceptance):
            with self.assertRaisesRegex(ValueError, "Extracted candidate differs"):
                self.stage()
        self.assertFalse(self.output.exists())

    def test_remote_tag_move_between_stage_and_draft_prevents_creation(self):
        self.stage()
        with patch.object(preview_trust, "live_producer", return_value={}), \
             patch.object(draft_preview.subprocess, "check_output", return_value=json.dumps({"sha": "b" * 40})), \
             patch.object(draft_preview.subprocess, "run") as execute:
            with self.assertRaisesRegex(ValueError, "tag moved"):
                draft_preview.create_draft(self.output, self.policy_path)
            execute.assert_not_called()


if __name__ == "__main__":
    unittest.main()
