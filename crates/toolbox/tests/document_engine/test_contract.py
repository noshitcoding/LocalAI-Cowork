"""Receipt/probe contract, request validation, sandbox policy, pagination and help."""
import importlib.util
import json
import os
import pathlib
import tempfile

import builders
from harness import SRC, EngineCase, run_adapter


def load_module(name):
    spec = importlib.util.spec_from_file_location('engine_' + name.replace('.py', ''), SRC / name)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class CompatibilityTests(EngineCase):
    def test_adapters_parse_as_python_311(self):
        """Docker images run the adapters with Debian bookworm's Python 3.11."""
        import ast
        for name in ('document_engine.py', 'image_engine.py'):
            ast.parse((SRC / name).read_text(encoding='utf-8'), filename=name, feature_version=(3, 11))


class ProbeTests(EngineCase):
    def test_document_probe_reports_pinned_modules(self):
        code, receipt, _ = run_adapter('document_engine.py', ['--probe'], pathlib.Path(self.tmp))
        self.assertEqual(code, 0)
        self.assertEqual(receipt['format'], 'cowork-document-probe/1')
        self.assertTrue(receipt['ok'], receipt)
        self.assertEqual(receipt['missing'], [])
        self.assertEqual(receipt['modules'], {'docx': '1.2.0', 'openpyxl': '3.1.5', 'pptx': '1.0.2', 'pypdf': '6.20.0',
                                              'pypdfium2': '5.12.1', 'PIL': '12.3.0', 'lxml': '6.1.3'})
        self.assertTrue(receipt['checks']['freetype'] and receipt['checks']['pdfium'])

    def test_image_probe(self):
        code, receipt, _ = run_adapter('image_engine.py', ['--probe'], pathlib.Path(self.tmp))
        self.assertEqual(code, 0)
        self.assertTrue(receipt['ok'], receipt)

    def test_usage_error_is_not_a_receipt(self):
        import subprocess
        from harness import PYTHON
        process = subprocess.run([PYTHON, '-I', str(SRC / 'document_engine.py')], capture_output=True)
        self.assertEqual(process.returncode, 64)
        self.assertEqual(process.stdout, b'')


class RequestTests(EngineCase):
    def test_receipt_is_ascii_json(self):
        stage = self.stage()
        stage.add('a.txt', 'Grüße 中文 Привет\n'.encode('utf-8'))
        import subprocess
        from harness import PYTHON, engine_env
        stage.run('document_read', 'text')  # writes request.json
        raw = subprocess.run([PYTHON, '-I', '-X', 'utf8', str(SRC / 'document_engine.py'), str(stage.root)],
                             capture_output=True, env=engine_env(stage.root), cwd=str(stage.root)).stdout
        raw.decode('ascii')
        self.assertEqual(json.loads(raw)['result']['text'], 'Grüße 中文 Привет\n')

    def test_unknown_operation_and_phase(self):
        stage = self.stage()
        receipt = self.failed(stage, 'unsupported', 'document_read', 'summarize')
        self.assertIn('text', receipt['error']['message'])
        self.failed(stage, 'unsupported', 'document_write', 'create', phase='latex_finalize')
        self.failed(stage, 'unsupported', 'image_edit', 'pipeline', adapter='document_engine.py')

    def test_invalid_request_json(self):
        stage = self.stage()
        stage.write('request.json', '{"format": NaN}')
        code, receipt, _ = run_adapter('document_engine.py', [str(stage.root)], stage.root)
        self.assertEqual((code, receipt['error']['code']), (2, 'invalid_spec'))
        stage.write('request.json', '[' * 100_000)
        code, receipt, _ = run_adapter('document_engine.py', [str(stage.root)], stage.root)
        self.assertEqual((code, receipt['error']['code']), (2, 'invalid_spec'))

    def test_stage_names_cannot_escape(self):
        stage = self.stage()
        for name in ('../evil.docx', 'out/../../x.docx', 'C:/x.docx', '/abs.docx', 'work/x.docx', 'out/CON.docx'):
            stage.write('request.json', json.dumps({
                'format': 'cowork-document-request/1', 'tool': 'document_write', 'operation': 'create',
                'inputs': [], 'output': {'name': name}, 'spec': {'format': 'docx', 'blocks': ['x']}}))
            code, receipt, _ = run_adapter('document_engine.py', [str(stage.root)], stage.root)
            self.assertEqual((code, receipt['error']['code']), (2, 'invalid_spec'), name)
            self.assertEqual(list((stage.root / 'out').iterdir()), [])

    def test_existing_output_is_never_overwritten(self):
        stage = self.stage()
        stage.write('out/a.docx', b'original')
        code, receipt, _ = stage.run('document_write', 'create', {'format': 'docx', 'blocks': ['x']}, output='a.docx')
        self.assertEqual((code, receipt['error']['code']), (2, 'invalid_spec'))
        self.assertEqual(stage.read('out/a.docx'), b'original')

    def test_staged_input_hash_is_verified(self):
        stage = self.stage()
        stage.add('a.txt', b'hello')
        (stage.root / stage.inputs[0]['name']).write_bytes(b'tampered')
        self.failed(stage, 'source_changed', 'document_read', 'text')

    def test_unknown_spec_key_names_the_path(self):
        stage = self.stage()
        receipt = self.failed(stage, 'invalid_spec', 'document_write', 'create',
                              {'format': 'docx', 'blocks': [{'type': 'paragraph', 'text': 'x', 'colour': 'red'}]},
                              output='a.docx')
        self.assertEqual(receipt['error']['path'], 'spec.blocks[0].colour')


class SandboxTests(EngineCase):
    def test_audit_hook_policy(self):
        for name in ('document_engine.py', 'image_engine.py'):
            module = load_module(name)
            stage = pathlib.Path(tempfile.mkdtemp(dir=self.tmp))
            (stage / 'work').mkdir()
            (stage / 'out').mkdir()
            sandbox = module.Sandbox(str(stage))
            outside = str(pathlib.Path(self.tmp).parent / 'outside.txt')
            sandbox('open', (str(stage / 'in' / 'x.docx'), 'rb', os.O_RDONLY))
            sandbox('open', (str(stage / 'out' / 'x.docx'), 'xb', os.O_WRONLY | os.O_CREAT))
            sandbox('open', (str(stage / 'work' / 'tmp' / 'x'), 'w+b', os.O_RDWR | os.O_CREAT))
            sandbox('open', (os.__file__, 'rb', os.O_RDONLY))  # the standard library stays readable
            for event, args in (('open', (outside, 'rb', os.O_RDONLY)),
                                ('open', (str(stage / 'request.json'), 'wb', os.O_WRONLY)),
                                ('open', (str(stage / 'in' / 'x.docx'), 'r+b', os.O_RDWR)),
                                ('os.remove', (str(stage / 'in' / 'x.docx'), None)),
                                ('os.rename', (str(stage / 'out' / 'a'), outside, None, None)),
                                ('os.listdir', (str(pathlib.Path(self.tmp).parent),)),
                                ('subprocess.Popen', ('cmd', ['cmd'], None, None)),
                                ('os.system', ('calc',)),
                                ('socket.connect', (None, ('example.org', 80))),
                                ('socket.getaddrinfo', ('example.org', 80, 0, 0, 0)),
                                ('urllib.Request', ('http://x', None, {}, 'GET')),
                                ('ctypes.dlopen', (outside,))):
                with self.assertRaises(PermissionError, msg='%s %s %r' % (name, event, args)):
                    sandbox(event, args)


class PaginationTests(EngineCase):
    def test_text_pages_with_next_offset(self):
        import docx
        import io
        document = docx.Document()
        for number in range(2000):
            document.add_paragraph('Paragraph %04d with some text that makes the document long enough.' % number)
        buffer = io.BytesIO()
        document.save(buffer)
        stage = self.stage()
        stage.add('long.docx', buffer.getvalue())
        chunks, offset, total = [], 0, None
        while offset is not None:
            result = self.ok(stage, 'document_read', 'text', options={'offset': offset})['result']
            self.assertTrue(result['truncated'] or result['next_offset'] is None)
            chunks.append(result['text'])
            total = result['total_chars']
            offset = result['next_offset']
        text = ''.join(chunks)
        self.assertGreater(len(chunks), 3)
        self.assertEqual(len(text), total)
        self.assertTrue(text.startswith('Paragraph 0000'))
        self.assertIn('Paragraph 1999', text)

    def test_inspect_items_are_paginated_and_fit(self):
        stage = self.stage()
        stage.add('lines.txt', (b'x' * 3000 + b'\n') * 200)
        result = self.ok(stage, 'document_read', 'inspect', options={'limit': 500})['result']
        self.assertLess(len(result['items']), 200)
        self.assertEqual(result['next_offset'], len(result['items']))
        result = self.ok(stage, 'document_read', 'inspect', options={'offset': 199, 'limit': 5})['result']
        self.assertEqual([item['line'] for item in result['items']], [200, 201])

    def test_offset_beyond_end(self):
        stage = self.stage()
        stage.add('a.txt', b'abc')
        self.failed(stage, 'invalid_spec', 'document_read', 'text', options={'offset': 10})


class HelpTests(EngineCase):
    def test_all_topics_fit(self):
        stage = self.stage()
        module = load_module('document_engine.py')
        for topic, text in module.HELP.items():
            self.assertLessEqual(len(text.encode('utf-8')), 24 * 1024, topic)
            result = self.ok(stage, 'document_read', 'help', options={'topic': topic})['result']
            self.assertIsNone(result['next_offset'], topic)
            self.assertEqual(result['reference'], text.strip() + '\n')
        self.failed(stage, 'invalid_spec', 'document_read', 'help', options={'topic': 'nope'})

    def test_help_mentions_every_edit_op(self):
        module = load_module('document_engine.py')
        for topic, ops in (('docx', module.DOCX_EDIT_OPS), ('xlsx', module.XLSX_EDIT_OPS),
                           ('pptx', module.PPTX_EDIT_OPS), ('pdf', module.PDF_ACTIONS)):
            for op in ops:
                self.assertIn(op, module.HELP[topic], '%s: %s' % (topic, op))
        for step in load_module('image_engine.py').STEP_KEYS:
            self.assertIn('"%s"' % step, module.HELP['image'], step)


class MetadataTests(EngineCase):
    def test_metadata_per_format(self):
        cases = (('a.docx', builders.docx_with_placeholders(), 'docx', 'paragraphs'),
                 ('a.xlsx', builders.sample_xlsx(), 'xlsx', 'sheets'),
                 ('a.pptx', builders.sample_pptx(), 'pptx', 'slides'),
                 ('a.pdf', builders.sample_pdf(2), 'pdf', 'pages'),
                 ('a.png', builders.image(), 'image', 'width'),
                 ('a.md', b'# x\nline', 'md', 'lines'))
        for name, data, kind, key in cases:
            stage = self.stage()
            stage.add(name, data)
            result = self.ok(stage, 'document_read', 'metadata')['result']
            self.assertEqual(result['format'], kind, name)
            self.assertIn(key, result, name)
            self.assertEqual(result['sha256'], stage.inputs[0]['sha256'])
