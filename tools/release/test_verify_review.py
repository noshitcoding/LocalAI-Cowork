"""Bounded synthetic negative tests; no real archive download/package execution."""
import copy
import hashlib
import json
from pathlib import Path
import stat
import tempfile
import unittest
from unittest.mock import patch
import zipfile

import verify_review as verifier


def sha(value):
    return hashlib.sha256(value).hexdigest()


def file_record(path):
    return {'size': path.stat().st_size, 'sha256': sha(path.read_bytes())}


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, sort_keys=True), encoding='utf-8')


def fixture(root):
    source = root / 'source'
    files = root / 'original'
    source.mkdir()
    files.mkdir()
    write_json(source / 'package.json', {'version': '2.0.0-alpha.8'})
    sources = {'package.json': file_record(source / 'package.json')}
    front_outputs = {'apps/web/dist/index.html': {'size': 6, 'sha256': sha(b'<html>')}}
    selection = {'repository': 'owner/repo', 'source_commit': 'a' * 40, 'producer_run_id': 42,
                 'platform': 'windows', 'artifact_id': 88, 'archive_sha256': 'b' * 64}
    run = {'id': 42, 'head_sha': 'a' * 40, 'path': '.github/workflows/preview-build-review.yml',
           'event': 'workflow_dispatch', 'status': 'completed', 'conclusion': 'success', 'run_attempt': 1,
           'repository': {'id': 11, 'full_name': 'owner/repo'}, 'head_repository': {'full_name': 'owner/repo'}}
    artifact = {'id': 88, 'name': 'v2-unsigned-review-windows-42-1', 'expired': False, 'digest': 'sha256:' + 'b' * 64,
                'size_in_bytes': 10, 'workflow_run': {'id': 42, 'head_sha': 'a' * 40, 'repository_id': 11, 'head_repository_id': 11}}
    verification = {'id': 40, 'head_sha': 'a' * 40, 'path': '.github/workflows/verify.yml', 'conclusion': 'success'}
    front = {'format': 'localai-cowork-frontend-build/1', 'version': '2.0.0-alpha.8', 'exit_code': 0,
             'sources_unchanged': True, 'sources': sources, 'outputs': front_outputs}
    write_json(files / 'frontend/receipt.json', front)
    (files / 'sidecar-cargo-build.jsonl').write_bytes(b'{"reason":"build-finished","success":true}\n')
    sidecar = {'format': 'localai-cowork-native-release-sidecar/1', 'host': verifier.HOSTS['windows'],
               'package_version': '2.0.0-alpha.8', 'profile': 'release', 'cargo_exit_code': 0,
               'source_before_after_equal': True, 'signed': False, 'installed': False,
               'sources': sources, 'cargo_json_sha256': file_record(files / 'sidecar-cargo-build.jsonl')['sha256'],
               'artifact': {'filename': 'cowork-daemon-x86_64-pc-windows-msvc.exe', 'size': 6, 'sha256': sha(b'daemon')}}
    write_json(files / 'sidecar-receipt.json', sidecar)
    (files / 'package.exe').write_bytes(b'not-an-executable; never run')
    (files / 'sbom.spdx.json').write_bytes(b'{"spdxVersion":"SPDX-2.3"}')
    (files / 'build-logs').mkdir()
    (files / 'build-logs/native.stdout.log').write_bytes(b'synthetic success\n')
    flags = dict(signed=False, installed=False, published=False, release_qualified=False)
    bundle = {'filename': 'package.exe', **file_record(files / 'package.exe')}
    package = {'format': 'localai-cowork-native-desktop-package/1', 'result': 'passed', 'host': verifier.HOSTS['windows'],
               'product_version': '2.0.0-alpha.8', 'validation_scope': 'format_tests_clippy_build',
               'desktop_format_test_clippy_exit_codes': [0, 0, 0], 'source_before_after_equal': True,
               'staged_source_bytes_equal': True, 'sources': {**sources, **front_outputs}, 'bundles': {'nsis': bundle},
               'sbom': {'filename': 'sbom.spdx.json', **file_record(files / 'sbom.spdx.json')},
               'sidecar_preparation': {'receipt.json': file_record(files / 'sidecar-receipt.json'),
                    'cargo-build.jsonl': file_record(files / 'sidecar-cargo-build.jsonl'),
                    sidecar['artifact']['filename']: {key: sidecar['artifact'][key] for key in ('size', 'sha256')}},
               'native_installer_contract': {'contract': 'nsis.currentUser.originalProcess.v1',
                    'product_version': '2.0.0-alpha.8', **file_record(files / 'package.exe')},
               'desktop_artifact': {'size': 7, 'sha256': sha(b'desktop')}, 'cli_version': '2.11.4',
               'bundle_input_desktop': {'size': 7, 'sha256': sha(b'desktop'), 'marker': 'unchanged', 'other_bytes_unchanged': True},
               'browser_host': {'size': 7, 'sha256': sha(b'browser')}, **flags}
    write_json(files / 'receipt.json', package)
    provenance = {'format': 'localai-cowork-unsigned-review-source/1', 'commit': 'a' * 40,
                  'sources': package['sources'], 'source_sha256': verifier.canonical(package['sources']),
                  'verification': verification, **flags}
    write_json(files / 'source-provenance.json', provenance)
    review = {'format': 'localai-cowork-unsigned-build-review/1', 'commit': 'a' * 40, 'host': verifier.HOSTS['windows'],
              'run_id': '42', 'run_attempt': '1', 'verification': verification,
              'source_sha256': provenance['source_sha256'], 'package_receipt_sha256': file_record(files / 'receipt.json')['sha256'],
              'frontend_receipt_sha256': file_record(files / 'frontend/receipt.json')['sha256'],
              'installer_execution_performed': False, 'installed_acceptance': 'pending',
              'build_logs': {'native.stdout.log': file_record(files / 'build-logs/native.stdout.log')}, **flags}
    write_json(files / 'UNSIGNED-BUILD-REVIEW.json', review)
    return source, files, selection, run, artifact


class VerifyReviewTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).absolute()
        self.source, self.files, self.selection, self.run, self.artifact = fixture(self.root)

    def test_original_synthetic_binding_passes_and_claims_no_execution(self):
        verifier.checked_metadata(self.selection, self.run, self.artifact)
        value = verifier.checked_tree(self.files, self.source, self.selection, self.run)
        self.assertFalse(value['installer_execution_performed'])
        self.assertFalse(value['release_qualified'])
        self.assertEqual(value['installed_smoke'], 'not_run')
        self.assertEqual(value['expected_payloads']['desktop']['sha256'], sha(b'desktop'))
        self.assertEqual(value['expected_payloads']['desktop']['allowed_marker'], 'NSS')

    def test_original_synthetic_linux_requires_both_distinct_bundle_pins(self):
        self.selection['platform'] = 'linux'
        self.artifact['name'] = 'v2-unsigned-review-linux-42-1'
        sidecar = verifier.read_json(self.files / 'sidecar-receipt.json')
        old_name = sidecar['artifact']['filename']
        sidecar['host'] = verifier.HOSTS['linux']
        sidecar['artifact']['filename'] = 'cowork-daemon-x86_64-unknown-linux-gnu'
        write_json(self.files / 'sidecar-receipt.json', sidecar)
        package = verifier.read_json(self.files / 'receipt.json')
        package['host'] = verifier.HOSTS['linux']
        prepared = package['sidecar_preparation']
        prepared[sidecar['artifact']['filename']] = prepared.pop(old_name)
        prepared['receipt.json'] = file_record(self.files / 'sidecar-receipt.json')
        package['bundles'] = {}
        for kind, name in [('deb', 'test.deb'), ('appimage', 'test.AppImage')]:
            (self.files / name).write_bytes(('synthetic-' + kind).encode())
            package['bundles'][kind] = {'filename': name, **file_record(self.files / name)}
        write_json(self.files / 'receipt.json', package)
        review = verifier.read_json(self.files / 'UNSIGNED-BUILD-REVIEW.json')
        review['host'] = verifier.HOSTS['linux']
        review['package_receipt_sha256'] = file_record(self.files / 'receipt.json')['sha256']
        write_json(self.files / 'UNSIGNED-BUILD-REVIEW.json', review)
        verifier.checked_metadata(self.selection, self.run, self.artifact)
        value = verifier.checked_tree(self.files, self.source, self.selection, self.run)
        self.assertEqual(set(value['packages']), {'deb', 'appimage'})
        (self.files / 'test.AppImage').write_bytes(b'different original image')
        with self.assertRaises(ValueError):
            verifier.checked_tree(self.files, self.source, self.selection, self.run)

    def test_changed_run_metadata_is_rejected(self):
        for key, value in [('id', 7), ('head_sha', 'c' * 40), ('event', 'push'),
                           ('path', '.github/workflows/other.yml'), ('status', 'in_progress'),
                           ('conclusion', 'failure'), ('run_attempt', 2), ('head_repository', {'full_name': 'fork/repo'})]:
            with self.subTest(key=key):
                changed = copy.deepcopy(self.run)
                changed[key] = value
                with self.assertRaises(ValueError):
                    verifier.checked_metadata(self.selection, changed, self.artifact)
        changed = copy.deepcopy(self.run)
        del changed['repository']['id']
        with self.assertRaises(ValueError):
            verifier.checked_metadata(self.selection, changed, self.artifact)

    def test_expired_foreign_replaced_artifact_is_rejected(self):
        for key, value in [('id', 89), ('expired', True), ('digest', None), ('digest', 'sha256:' + 'c' * 64),
                           ('name', 'same-name-new-run'), ('workflow_run', {'id': 41}), ('size_in_bytes', 0)]:
            with self.subTest(key=key, value=value):
                changed = copy.deepcopy(self.artifact)
                changed[key] = value
                with self.assertRaises(ValueError):
                    verifier.checked_metadata(self.selection, self.run, changed)

    def test_changed_original_receipts_payload_sources_or_logs_are_rejected(self):
        for relative in ['receipt.json', 'frontend/receipt.json', 'sidecar-receipt.json',
                         'sidecar-cargo-build.jsonl', 'package.exe', 'sbom.spdx.json', 'build-logs/native.stdout.log']:
            with self.subTest(relative=relative):
                path = self.files / relative
                original = path.read_bytes()
                path.write_bytes(original + b' ')
                with self.assertRaises(ValueError):
                    verifier.checked_tree(self.files, self.source, self.selection, self.run)
                path.write_bytes(original)
        (self.source / 'package.json').write_bytes(b'{"version":"2.0.0-alpha.8", "changed":true}')
        with self.assertRaises(ValueError):
            verifier.checked_tree(self.files, self.source, self.selection, self.run)

    def test_fast_or_installed_receipt_cannot_be_relabelled(self):
        path = self.files / 'receipt.json'
        original = verifier.read_json(path)
        for key, value in [('validation_scope', 'build_only'), ('signed', True), ('installed', True),
                           ('desktop_format_test_clippy_exit_codes', [0, 0, 1])]:
            changed = {**original, key: value}
            write_json(path, changed)
            review = verifier.read_json(self.files / 'UNSIGNED-BUILD-REVIEW.json')
            review['package_receipt_sha256'] = file_record(path)['sha256']
            write_json(self.files / 'UNSIGNED-BUILD-REVIEW.json', review)
            with self.subTest(key=key), self.assertRaises(ValueError):
                verifier.checked_tree(self.files, self.source, self.selection, self.run)

    def test_cargo_pin_and_exact_cli_marker_claim_are_required(self):
        path = self.files / 'receipt.json'
        original = verifier.read_json(path)
        changes = [dict(cli_version='2.12.0'), dict(desktop_artifact={}),
            dict(bundle_input_desktop={**original['bundle_input_desktop'], 'marker': 'APP'}),
            dict(bundle_input_desktop={**original['bundle_input_desktop'], 'other_bytes_unchanged': False}),
            dict(bundle_input_desktop={**original['bundle_input_desktop'], 'sha256': 'c' * 64})]
        for change in changes:
            write_json(path, {**original, **change})
            review = verifier.read_json(self.files / 'UNSIGNED-BUILD-REVIEW.json')
            review['package_receipt_sha256'] = file_record(path)['sha256']
            write_json(self.files / 'UNSIGNED-BUILD-REVIEW.json', review)
            with self.subTest(change=change), self.assertRaises(ValueError):
                verifier.checked_tree(self.files, self.source, self.selection, self.run)
        selected = {**original['bundle_input_desktop'], 'marker': 'NSS', 'sha256': 'd' * 64}
        write_json(path, {**original, 'bundle_input_desktop': selected})
        review = verifier.read_json(self.files / 'UNSIGNED-BUILD-REVIEW.json')
        review['package_receipt_sha256'] = file_record(path)['sha256']
        write_json(self.files / 'UNSIGNED-BUILD-REVIEW.json', review)
        value = verifier.checked_tree(self.files, self.source, self.selection, self.run)
        self.assertEqual(value['expected_payloads']['desktop']['sha256'], original['desktop_artifact']['sha256'])
        self.assertEqual(value['expected_payloads']['desktop']['original_bundle_input'], selected)

    def archive(self, entries):
        path = self.root / 'fixture.zip'
        with zipfile.ZipFile(path, 'w') as stream:
            for name, content in entries:
                if isinstance(name, str):
                    info = zipfile.ZipInfo('placeholder')
                    # ZipInfo normalizes Windows backslashes in its constructor.
                    # Write the actual adversarial ZIP name, not a normalized test.
                    info.filename = name
                    info.orig_filename = name
                else:
                    info = name
                stream.writestr(info, content)
        return path

    def test_safe_zip_extract_and_exact_archive_digest(self):
        path = self.archive([('nested/receipt.json', b'{}')])
        with self.assertRaisesRegex(ValueError, 'digest'):
            verifier.safe_extract(path, self.root / 'wrong', '0' * 64)
        self.assertFalse((self.root / 'wrong').exists())
        verifier.safe_extract(path, self.root / 'good', sha(path.read_bytes()))
        self.assertEqual((self.root / 'good/nested/receipt.json').read_bytes(), b'{}')

    def test_archive_traversal_aliases_case_collision_and_symlink_are_rejected(self):
        for index, name in enumerate(['../escape', '/escape', 'C:/escape', 'a\\escape', 'a/../escape', 'a//escape',
                     '.', 'NUL.txt', 'space. ', 'x:stream', 'a\x01b']):
            with self.subTest(name=name):
                path = self.archive([(name, b'bad')])
                with self.assertRaises(ValueError):
                    verifier.safe_extract(path, self.root / f'refused-{index}', sha(path.read_bytes()))
                self.assertFalse((self.root / f'refused-{index}').exists())
        path = self.archive([('A', b'1'), ('a', b'2')])
        with self.assertRaises(ValueError):
            verifier.safe_extract(path, self.root / 'refused', sha(path.read_bytes()))
        link = zipfile.ZipInfo('link')
        link.create_system = 3
        link.external_attr = (stat.S_IFLNK | 0o777) << 16
        path = self.archive([(link, b'../escape')])
        with self.assertRaises(ValueError):
            verifier.safe_extract(path, self.root / 'refused', sha(path.read_bytes()))

    def test_zip_expansion_limit_and_duplicate_json_are_rejected(self):
        path = self.archive([('large', b'12345')])
        with patch.object(verifier, 'MAX_EXPANDED', 4), self.assertRaises(ValueError):
            verifier.safe_extract(path, self.root / 'refused', sha(path.read_bytes()))
        path = self.root / 'duplicate.json'
        path.write_text('{"x":1,"x":2}')
        with self.assertRaises(ValueError):
            verifier.read_json(path)


if __name__ == '__main__':
    unittest.main(verbosity=2)
