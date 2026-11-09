"""Synthetic installed-byte checks only; no native package or process execution."""
import hashlib
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import installed_lifetime_linux as linux
import verify_review as verify


class DesktopMarkerTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name).resolve()
        self.path = self.root / 'localai-cowork-v2-desktop'
        self.original = b'synthetic-ELF\0' + verify.DESKTOP_MARKER_PREFIX + b'UNK\0constant:' + verify.DESKTOP_MARKER_PREFIX + b'APP\0tail'
        self.path.write_bytes(self.original)
        self.expected = {**verify.pin(self.path), 'marker_policy': verify.DESKTOP_MARKER_POLICY}

    def installed(self, kind):
        data = self.original.replace(verify.DESKTOP_MARKER_PREFIX + b'UNK',
                                     verify.DESKTOP_MARKER_PREFIX + verify.DESKTOP_MARKERS[kind], 1)
        self.path.write_bytes(data)
        return data

    def test_original_cargo_bytes_remain_exact_for_each_kind(self):
        for kind in ('nsis', 'deb', 'appimage'):
            result = verify.desktop_payload_pin(self.path, self.expected, kind)
            self.assertEqual(result['raw'], {key: self.expected[key] for key in ('size', 'sha256')})
            self.assertEqual(result['normalization']['changed_bytes'], 0)
            self.assertEqual(result['normalization']['marker'], 'unchanged')

    def test_each_selected_three_byte_marker_binds_to_cargo_and_retains_raw(self):
        for kind in ('nsis', 'deb', 'appimage'):
            with self.subTest(kind=kind):
                data = self.installed(kind)
                result = verify.desktop_payload_pin(self.path, self.expected, kind)
                self.assertEqual(result['raw']['sha256'], hashlib.sha256(data).hexdigest())
                self.assertNotEqual(result['raw']['sha256'], self.expected['sha256'])
                self.assertEqual(result['normalization']['normalized_sha256'], self.expected['sha256'])
                self.assertEqual(result['normalization']['marker'], verify.DESKTOP_MARKERS[kind].decode())
                self.assertEqual(result['normalization']['changed_bytes'], 3)
                self.assertEqual(result['normalization']['metadata_byte_offset'], self.original.index(b'UNK'))

    def test_other_unchanged_marker_constant_is_never_replaced(self):
        data = self.installed('appimage')
        self.assertEqual(data.count(verify.DESKTOP_MARKER_PREFIX + b'APP'), 2)
        verify.desktop_payload_pin(self.path, self.expected, 'appimage')
        self.path.write_bytes(data.replace(verify.DESKTOP_MARKER_PREFIX + b'APP', verify.DESKTOP_MARKER_PREFIX + b'UNK'))
        with self.assertRaises(ValueError):
            verify.desktop_payload_pin(self.path, self.expected, 'appimage')

    def test_wrong_package_marker_and_policy_are_refused(self):
        self.installed('appimage')
        with self.assertRaises(ValueError):
            verify.desktop_payload_pin(self.path, self.expected, 'deb')
        for kind, expected in [('foreign', self.expected), ('appimage', {**self.expected, 'marker_policy': 'unknown'}),
                ('appimage', {**self.expected, 'allowed_marker': 'NSS'})]:
            with self.subTest(kind=kind, expected=expected), self.assertRaises(ValueError):
                verify.desktop_payload_pin(self.path, expected, kind)

    def test_extra_byte_or_length_or_missing_marker_is_refused(self):
        changed = self.installed('deb')
        for data in (changed[:-1] + b'X', changed + b'\0', changed[:-1], b'x' * len(changed)):
            self.path.write_bytes(data)
            with self.subTest(data=data), self.assertRaises(ValueError):
                verify.desktop_payload_pin(self.path, self.expected, 'deb')

    def test_excessive_marker_candidates_are_bounded(self):
        self.original = self.original + (verify.DESKTOP_MARKER_PREFIX + b'NSS') * 33
        self.path.write_bytes(self.original)
        self.expected.update(verify.pin(self.path))
        self.installed('nsis')
        with self.assertRaisesRegex(ValueError, 'count exceeds bound'):
            verify.desktop_payload_pin(self.path, self.expected, 'nsis')

    def test_binding_pin_must_have_reviewed_finite_cargo_size_and_digest(self):
        for changed in ({'size': True}, {'size': 513 * 1024**2}, {'sha256': 'unknown'}):
            with self.subTest(changed=changed), self.assertRaises(ValueError):
                verify.desktop_payload_pin(self.path, {**self.expected, **changed}, 'deb')

    def test_change_during_cargo_binding_is_refused(self):
        with patch.object(verify, 'pin', return_value={'size': len(self.original), 'sha256': '0' * 64}), self.assertRaisesRegex(ValueError, 'changed during'):
            verify.desktop_payload_pin(self.path, self.expected, 'deb')

    def test_linux_manifest_accepts_selected_marker_and_keeps_sidecars_exact(self):
        self.installed('deb')
        daemon = self.root / 'cowork-daemon'
        browser = self.root / 'cowork-browser-host'
        daemon.write_bytes(b'exact-daemon')
        browser.write_bytes(b'exact-browser')
        expected = {'desktop': self.expected, 'daemon': verify.pin(daemon), 'browser_host': verify.pin(browser)}
        result = linux.find_payloads([self.path, daemon, browser], expected, 'deb')
        self.assertEqual(result['desktop'], self.path)
        daemon.write_bytes(b'changed-daemon')
        with self.assertRaises(ValueError):
            linux.find_payloads([self.path, daemon, browser], expected, 'deb')


if __name__ == '__main__':
    unittest.main(verbosity=2)
