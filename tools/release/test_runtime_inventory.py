"""Installed RECORD ownership and nested bundled metadata regression checks."""
from __future__ import annotations

import base64
import csv
import hashlib
import io
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import runtime_inventory as inventory
import sbom


class RuntimeInventoryTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='cowork-inventory-test-')
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.site = 'lib/python3.11/site-packages/'
        self.owner = self.site + 'owner-1.0.dist-info/'
        self.vendor = self.site + 'owner/_vendor/'
        self.nested = self.vendor + 'typing_extensions-4.12.dist-info/'
        self.write(self.owner + 'METADATA', 'Name: owner\nVersion: 1.0\n')
        self.write(self.site + 'owner/__init__.py', '# owner\n')
        self.write(self.nested + 'METADATA', 'Name: typing_extensions\nVersion: 4.12\n')
        # This is a bundled build-time record, including absent old build paths.
        self.write(self.nested + 'RECORD', '../../bin/wheel,,,\n')
        self.write(self.vendor + 'typing_extensions.py', '# bundled version\n')
        self.record_owner()

    def write(self, name, content):
        target = self.root / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content, encoding='utf-8', newline='\n')

    def record_owner(self, *, omit=(), weak=(), extra=()):
        rows = []
        for path in sorted(self.root.rglob('*')):
            if not path.is_file():
                continue
            name = path.relative_to(self.root).as_posix()
            if name in omit or name == self.owner + 'RECORD':
                continue
            raw = path.read_bytes()
            relative = name.removeprefix(self.site)
            digest = 'sha256=' + base64.urlsafe_b64encode(hashlib.sha256(raw).digest()).decode().rstrip('=')
            rows.append((relative, '' if name in weak else digest, '' if name in weak else str(len(raw))))
        rows.extend(extra)
        rows.append(('owner-1.0.dist-info/RECORD', '', ''))
        output = io.StringIO(newline='')
        csv.writer(output).writerows(rows)
        self.write(self.owner + 'RECORD', output.getvalue())

    def capture(self):
        return inventory.capture(self.root, name='Selected Python', version='3.11')

    def test_validated_owner_keeps_bundled_record_as_data_and_runtime_name(self):
        result = self.capture()
        self.assertEqual(result['name'], 'Selected Python')
        self.assertEqual([item['name'] for item in result['components']], ['owner'])
        owner = result['components'][0]
        self.assertEqual(owner['vendored_metadata'][0]['version'], '4.12')
        self.assertIn('no separate installed dist', owner['source'])
        self.assertIn(self.nested + 'RECORD', owner['files'])
        self.assertIn(self.vendor + 'typing_extensions.py', owner['files'])
        self.assertEqual(result['files'], inventory.tree(self.root))
        report = sbom.generate_runtime([result])
        package = next(item for item in report['packages'] if item['name'] == 'owner')
        self.assertIn('typing_extensions 4.12', package['sourceInfo'])
        file = next(item for item in report['files'] if item['fileName'].endswith('/' + self.nested + 'RECORD'))
        self.assertIn({'spdxElementId': package['SPDXID'], 'relationshipType': 'CONTAINS',
                       'relatedSpdxElement': file['SPDXID']}, report['relationships'])

    def test_genuine_installed_missing_file_remains_rejected(self):
        self.record_owner(extra=[('owner/missing.py', '', '')])
        with self.assertRaisesRegex(ValueError, 'absent file'):
            self.capture()

    def test_modified_vendor_file_still_fails_owner_hash(self):
        self.write(self.vendor + 'typing_extensions.py', '# malicious replacement\n')
        with self.assertRaisesRegex(ValueError, 'RECORD (size|hash) differs'):
            self.capture()

    def test_owner_must_bind_every_file_under_nested_site(self):
        self.record_owner(omit=[self.vendor + 'typing_extensions.py'])
        with self.assertRaisesRegex(ValueError, 'one complete validated owner'):
            self.capture()

    def test_nested_metadata_and_record_need_strong_owner_hashes(self):
        for filename in (self.nested + 'METADATA', self.nested + 'RECORD'):
            with self.subTest(filename=filename):
                self.record_owner(weak=[filename])
                with self.assertRaisesRegex(ValueError, 'one complete validated owner'):
                    self.capture()

    def test_nested_without_owner_is_not_promoted_to_installed(self):
        (self.root / (self.owner + 'METADATA')).unlink()
        (self.root / (self.owner + 'RECORD')).unlink()
        with self.assertRaisesRegex(ValueError, 'one complete validated owner'):
            self.capture()

    def test_missing_installed_record_is_rejected(self):
        (self.root / (self.owner + 'RECORD')).unlink()
        with self.assertRaisesRegex(ValueError, 'lacks RECORD'):
            self.capture()

    def test_top_level_and_bundled_versions_keep_separate_meanings(self):
        top = self.site + 'typing_extensions-4.16.dist-info/'
        self.write(top + 'METADATA', 'Name: typing_extensions\nVersion: 4.16\n')
        self.write(top + 'RECORD', '')
        self.record_owner(omit=[top + 'METADATA', top + 'RECORD'])
        raw = (self.root / (top + 'METADATA')).read_bytes()
        digest = base64.urlsafe_b64encode(hashlib.sha256(raw).digest()).decode().rstrip('=')
        self.write(top + 'RECORD', 'typing_extensions-4.16.dist-info/METADATA,sha256=' + digest + ',' + str(len(raw)) + '\n'
                   'typing_extensions-4.16.dist-info/RECORD,,\n')
        result = self.capture()
        report = sbom.generate_runtime([result])
        versions = [item['versionInfo'] for item in report['packages'] if item['name'] == 'typing_extensions']
        self.assertEqual(versions, ['4.16'])
        owner = next(item for item in report['packages'] if item['name'] == 'owner')
        self.assertIn('typing_extensions 4.12', owner['sourceInfo'])

    def test_mutation_during_capture_remains_rejected(self):
        actual = inventory.tree
        calls = 0
        def changing(*args, **kwargs):
            nonlocal calls
            calls += 1
            if calls == 2:
                self.write('changed.txt', 'new bytes')
            return actual(*args, **kwargs)
        with patch.object(inventory, 'tree', side_effect=changing):
            with self.assertRaisesRegex(ValueError, 'changed during inventory'):
                self.capture()

    def test_ambiguous_owner_is_rejected(self):
        other = self.site + 'other-1.0.dist-info/'
        self.write(other + 'METADATA', 'Name: other\nVersion: 1.0\n')
        self.record_owner()
        owner_record = (self.root / (self.owner + 'RECORD')).read_text(encoding='utf-8')
        self.write(other + 'RECORD', owner_record + 'other-1.0.dist-info/RECORD,,\n')
        with self.assertRaisesRegex(ValueError, 'one complete validated owner'):
            self.capture()


if __name__ == '__main__':
    unittest.main()
