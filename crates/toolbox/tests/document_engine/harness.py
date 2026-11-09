"""Test harness for the document/image engines: builds a private stage like the Rust runtime does,
writes request.json, runs the adapter as `python -I -X utf8 <adapter> <stage>` and parses the receipt.

The interpreter is COWORK_ENGINE_PYTHON or the interpreter running the tests (use the pinned dev venv).
"""
import hashlib
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parents[3]
SRC = REPO / 'crates' / 'toolbox' / 'src'
FIXTURES = HERE / 'fixtures'
PYTHON = os.environ.get('COWORK_ENGINE_PYTHON') or sys.executable
LIMITS = {'max_output_bytes': 64 * 1024 * 1024, 'max_outputs': 64, 'max_result_bytes': 32768, 'max_pages_read': 500,
          'max_pages_render': 200, 'max_preview_images': 4, 'preview_max_edge': 1600,
          'preview_max_bytes': 2 * 1024 * 1024, 'max_image_pixels': 50_000_000}


def sha256(data):
    return hashlib.sha256(data).hexdigest()


class Stage:
    """One private stage directory: in/, work/previews/, out/, tmp/, home/."""

    def __init__(self, root):
        self.root = pathlib.Path(tempfile.mkdtemp(prefix='stage-', dir=root))
        for directory in ('in', 'work/previews', 'out', 'tmp', 'home'):
            (self.root / directory).mkdir(parents=True, exist_ok=True)
        self.inputs = []

    def add(self, filename, data, name=None):
        """Stage an input; returns its index. `name` overrides the stage name (e.g. in/tex/main.tex)."""
        name = name or 'in/%d/%s' % (len(self.inputs), filename)
        target = self.root / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
        self.inputs.append({'name': name, 'path': 'docs/' + filename, 'sha256': sha256(data), 'bytes': len(data)})
        return len(self.inputs) - 1

    def write(self, name, data):
        target = self.root / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data if isinstance(data, bytes) else data.encode('utf-8'))
        return name

    def read(self, name):
        return (self.root / name).read_bytes()

    def run(self, tool, operation, spec=None, output=None, options=None, phase=None, limits=None, adapter=None,
            timeout=180):
        adapter = adapter or ('image_engine.py' if tool == 'image_edit' else 'document_engine.py')
        request = {'format': 'cowork-document-request/1', 'tool': tool, 'operation': operation, 'phase': phase,
                   'inputs': self.inputs, 'output': {'name': 'out/' + output} if output else None,
                   'spec': spec or {}, 'options': options or {}, 'limits': dict(LIMITS, **(limits or {}))}
        request_path = self.root / 'request.json'
        if request_path.exists():
            request_path.unlink()
        request_path.write_text(json.dumps(request), encoding='utf-8')
        return run_adapter(adapter, [str(self.root)], self.root, timeout)


def engine_env(home):
    env = {'HOME': str(home / 'home'), 'USERPROFILE': str(home / 'home'), 'TEMP': str(home / 'tmp'),
           'TMP': str(home / 'tmp'), 'TMPDIR': str(home / 'tmp'), 'PATH': os.path.dirname(PYTHON)}
    for key in ('SystemRoot', 'WINDIR', 'SYSTEMDRIVE'):
        if os.environ.get(key):
            env[key] = os.environ[key]
    return env


def run_adapter(adapter, args, cwd, timeout=180):
    process = subprocess.run([PYTHON, '-I', '-X', 'utf8', str(SRC / adapter), *args], cwd=str(cwd),
                             env=engine_env(pathlib.Path(cwd)), capture_output=True, timeout=timeout)
    stdout = process.stdout.decode('ascii')  # receipts are ASCII JSON
    lines = [line for line in stdout.splitlines() if line.strip()]
    if len(lines) != 1:
        raise AssertionError('expected exactly one receipt line, got %r\nstderr: %s' % (
            stdout[:2000], process.stderr.decode('utf-8', 'replace')[-4000:]))
    return process.returncode, json.loads(lines[0]), process.stderr.decode('utf-8', 'replace')


class EngineCase(unittest.TestCase):
    """Base class: a fresh stage per test, helpers to assert ok/failed receipts."""

    maxDiff = 4000

    def assert_pdf_unicode_kept_or_reported(self, receipt, source, data):
        import io
        from pypdf import PdfReader
        extracted = ''.join(page.extract_text() for page in PdfReader(io.BytesIO(data)).pages)
        missing = receipt['qa']['missing_characters']
        self.assertEqual(receipt['result']['missing_characters'], len(missing))
        for char in set(source):
            if not char.isspace() and char not in extracted:
                self.assertIn('%s U+%04X' % (char, ord(char)), missing,
                              'A source character disappeared without an explicit diagnostic')
        if not missing:
            self.assertIn(''.join(source.split()), ''.join(extracted.split()))
        return extracted

    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.mkdtemp(prefix='cowork-engine-tests-')

    @classmethod
    def tearDownClass(cls):
        if not os.environ.get('COWORK_KEEP_STAGES'):
            shutil.rmtree(cls.tmp, ignore_errors=True)

    def stage(self):
        return Stage(self.tmp)

    def ok(self, stage, *args, **kwargs):
        code, receipt, stderr = stage.run(*args, **kwargs)
        self.assertEqual(receipt.get('format'), 'cowork-document-receipt/1', receipt)
        self.assertTrue(receipt.get('ok'), 'receipt not ok: %s\nstderr: %s' % (json.dumps(receipt)[:3000],
                                                                               stderr[-3000:]))
        self.assertEqual(code, 0, stderr[-2000:])
        self.assertEqual(receipt['engine_version'], '1')
        for output in receipt['outputs']:
            data = stage.read(output['name'])
            self.assertEqual(sha256(data), output['sha256'])
            self.assertEqual(len(data), output['bytes'])
            self.assertTrue(output['name'].startswith('out/'), output)
        for preview in receipt['previews']:
            self.assertTrue(preview['name'].startswith('work/previews/'), preview)
            self.assertEqual(sha256(stage.read(preview['name'])), preview['sha256'])
        result_size = len(json.dumps(receipt['result'], separators=(',', ':')))
        self.assertLessEqual(result_size, LIMITS['max_result_bytes'])
        return receipt

    def failed(self, stage, code, *args, **kwargs):
        before = sorted((stage.root / 'out').rglob('*'))
        exit_code, receipt, stderr = stage.run(*args, **kwargs)
        self.assertEqual(receipt.get('format'), 'cowork-document-receipt/1', receipt)
        self.assertFalse(receipt.get('ok'), receipt)
        self.assertEqual(exit_code, 2, stderr[-2000:])
        self.assertEqual(receipt['error']['code'], code, receipt)
        self.assertEqual(sorted((stage.root / 'out').rglob('*')), before, 'a failed run must not leave outputs')
        return receipt
