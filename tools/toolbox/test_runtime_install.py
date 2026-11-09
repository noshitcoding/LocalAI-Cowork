"""Offline checks for the private tool setup: lock split, Tectonic pins, archive and cache helpers."""
import importlib.util
import io
import json
import os
import pathlib
import re
import tarfile
import tempfile
import unittest
import zipfile
from unittest import mock

HERE = pathlib.Path(__file__).resolve().parent
specification = importlib.util.spec_from_file_location('cowork_runtime_install', HERE / 'runtime_install.py')
setup = importlib.util.module_from_spec(specification)
specification.loader.exec_module(setup)


def pins(name):
    lines = [line for line in (HERE / name).read_text(encoding='utf-8').splitlines() if line and not line.startswith('#')]
    result = {}
    for line in lines:
        requirement, *hashes = line.split(' ')
        package, version = requirement.split('==')
        result[package] = (version, hashes)
    return result


class LockSplitTest(unittest.TestCase):
    def test_office_setup_selects_the_console_entrypoint_on_windows(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            def extract(archive, destination):
                program = destination / 'program'
                program.mkdir()
                for name in ('soffice.com', 'soffice.exe', 'soffice'):
                    (program / name).write_bytes(b'fixture')
            programs, owned = {}, {}
            with mock.patch.object(setup, 'download', return_value=root / 'fixture.archive'), \
                 mock.patch.object(setup, 'progress'), \
                 mock.patch.object(setup, 'msi_extract', side_effect=extract), \
                 mock.patch.object(setup, 'linux_office_extract', side_effect=extract):
                setup.install_libreoffice(root, programs, owned)
            self.assertEqual(pathlib.Path(programs['libreoffice']).name,
                'soffice.com' if os.name == 'nt' else 'soffice')
            self.assertIn('libreoffice', owned)

    def test_office_selection_must_exist_and_is_never_silently_replaced(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            selected = root / 'chosen-office'
            with mock.patch.object(setup, 'download', side_effect=AssertionError('Unexpected download')):
                with self.assertRaisesRegex(RuntimeError, 'selected LibreOffice'):
                    setup.install_libreoffice(root, {'libreoffice': str(selected)}, {})
                selected.write_bytes(b'fixture')
                programs = {'libreoffice': str(selected)}
                setup.install_libreoffice(root, programs, {})
                self.assertEqual(programs, {'libreoffice': str(selected)})

    def test_office_only_keeps_existing_configuration_without_installing_other_tools(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            configuration = {'programs': {'python': 'existing-python'}, 'custom': {'retained': True}}
            (root / 'toolbox.json').write_text(json.dumps(configuration), encoding='utf-8')

            def office(runtime, programs, owned):
                programs['libreoffice'] = str(runtime / 'office' / 'soffice')

            with mock.patch.object(setup.sys, 'argv', ['runtime_install.py', '--data-dir', str(root), '--libreoffice-only']), \
                 mock.patch.object(setup, 'install_libreoffice', side_effect=office) as selected, \
                 mock.patch.object(setup, 'install_tectonic', side_effect=AssertionError('Unexpected TeX setup')), \
                 mock.patch.object(setup, 'execute', side_effect=AssertionError('Unexpected Python/browser setup')), \
                 mock.patch.object(setup, 'progress'):
                setup.main()
            selected.assert_called_once()
            result = json.loads((root / 'toolbox.json').read_text(encoding='utf-8'))
            self.assertEqual(result['programs']['python'], 'existing-python')
            self.assertEqual(result['custom'], configuration['custom'])
            self.assertIn('libreoffice', result['programs'])
            self.assertFalse((root / 'tool-setup-receipt.json').exists())
            self.assertFalse(list((root / 'tool-runtime').glob('setup-*')))

    def test_trickling_downloads_cannot_extend_the_total_deadline(self):
        with tempfile.TemporaryDirectory() as directory:
            with mock.patch.object(setup,'STAGE',pathlib.Path(directory),create=True), \
                 mock.patch.object(setup.urllib.request,'urlopen',return_value=io.BytesIO(b'partial')), \
                 mock.patch.object(setup.time,'monotonic',side_effect=[0,601]):
                with self.assertRaisesRegex(RuntimeError,'time bound'):
                    setup.download('test.archive',('https://example.invalid/test','0'*64),100)

    def test_documents_and_browser_locks_are_disjoint_and_fully_hashed(self):
        documents, browser = pins('requirements-documents.lock'), pins('requirements-browser.lock')
        self.assertEqual(set(browser), {'greenlet', 'playwright', 'pyee'})
        self.assertEqual(documents['python-docx'][0], '1.2.0')
        self.assertEqual(documents['pypdfium2'][0], '5.12.1')
        # reportlab needs charset-normalizer, pyee and python-docx need typing-extensions.
        self.assertTrue({'charset-normalizer', 'typing-extensions', 'lxml', 'pillow', 'pypdf', 'python-pptx', 'openpyxl', 'et-xmlfile', 'xlsxwriter', 'reportlab'} <= set(documents))
        self.assertFalse(set(documents) & set(browser))
        for package, (_, hashes) in {**documents, **browser}.items():
            self.assertTrue(hashes, package)
            self.assertTrue(all(re.fullmatch(r'--hash=sha256:[0-9a-f]{64}', value) for value in hashes), package)

    def test_browser_profiles_always_include_the_documents_lock(self):
        self.assertEqual(setup.PROFILES['documents'], ('documents', 'ocr'))
        for profile in ('browser', 'full'):
            self.assertEqual(setup.PROFILES[profile], ('documents', 'ocr', 'browser'))


class TectonicPinTest(unittest.TestCase):
    def test_release_assets_and_binaries_are_pinned_together(self):
        version = setup.TECTONIC['version']
        for platform, (name, digest) in setup.TECTONIC['binary'].items():
            url, archive_digest = setup.DOWNLOADS['tectonic_' + platform]
            self.assertTrue(url.startswith('https://github.com/tectonic-typesetting/tectonic/releases/download/tectonic%40' + version + '/'))
            self.assertRegex(archive_digest, '^[0-9a-f]{64}$')
            self.assertRegex(digest, '^[0-9a-f]{64}$')
            self.assertEqual(name, 'tectonic.exe' if platform == 'windows' else 'tectonic')
        self.assertIn('x86_64-pc-windows-msvc', setup.DOWNLOADS['tectonic_windows'][0])
        self.assertIn('x86_64-unknown-linux-musl', setup.DOWNLOADS['tectonic_linux'][0])

    def test_warmup_loads_fonts_by_file_name_only(self):
        # Family names would reach the host font manager outside the stage and the cache.
        checked = 0
        for name, source in setup.TEX_WARMUP.items():
            for match in re.finditer(r'\\(?:set(?:main|sans|mono|math)font|setCJK(?:main|sans|mono)font|newCJKfontfamily\\\w+)\{([^}]*)\}(\[[^\]]*\])?', source):
                font, options = match[1], match[2] or ''
                self.assertTrue(font.endswith(('.otf', '.ttf')) or 'Extension=' in options, name + ': ' + font)
                checked += 1
        self.assertGreaterEqual(checked, 15)

    def test_bundle_digests_unescape_tectonic_cache_names(self):
        with tempfile.TemporaryDirectory() as directory:
            hashes = pathlib.Path(directory) / 'bundles' / 'hashes'
            hashes.mkdir(parents=True)
            (hashes / 'https,58,,47,,47,relay.fullyjustified.net,47,default_bundle_v33.tar').write_text(setup.TECTONIC['bundle_digest'] + '\n', encoding='ascii')
            (hashes / 'https,58,,47,,47,relay.fullyjustified.net,47,default_bundle_v33.lock').write_text('1792370619', encoding='ascii')
            self.assertEqual(setup.bundle_digests(pathlib.Path(directory)), {setup.TECTONIC['bundle']: setup.TECTONIC['bundle_digest']})

    @unittest.skipUnless(os.name == 'nt', 'Windows MAX_PATH limit')
    def test_windows_cache_path_limit_fails_before_any_download(self):
        with tempfile.TemporaryDirectory() as directory:
            runtime = pathlib.Path(directory) / ('r' * max(1, 120 - len(directory)))
            runtime.mkdir()
            with self.assertRaisesRegex(RuntimeError, 'too long'):
                setup.warm_tectonic(pathlib.Path('unused-tectonic.exe'), runtime)


class ExtractSingleTest(unittest.TestCase):
    def archive(self, root, members):
        if os.name == 'nt':
            path = root / 'archive.zip'
            with zipfile.ZipFile(path, 'w') as package:
                for name, data in members.items():
                    package.writestr(name, data)
        else:
            path = root / 'archive.tar.gz'
            with tarfile.open(path, 'w:gz') as package:
                for name, data in members.items():
                    info = tarfile.TarInfo(name)
                    info.size = len(data)
                    package.addfile(info, io.BytesIO(data))
        return path

    def test_writes_only_the_expected_member(self):
        name = 'tectonic.exe' if os.name == 'nt' else 'tectonic'
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            archive = self.archive(root, {name: b'binary', '../escape.txt': b'x', 'other.dll': b'y'})
            target = root / 'out'
            target.mkdir()
            setup.extract_single(archive, name, target / name, 1024)
            self.assertEqual((target / name).read_bytes(), b'binary')
            self.assertEqual(sorted(p.name for p in root.rglob('*') if p.is_file()), sorted([archive.name, name]))
            with self.assertRaises(RuntimeError):
                setup.extract_single(archive, name, target / 'second', 3)
            with self.assertRaises(KeyError):
                setup.extract_single(archive, 'missing', target / 'third', 1024)


if __name__ == '__main__':
    unittest.main()
