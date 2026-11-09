"""Regression tests for the five office.py QA bugs (QA audit 2026-10-17, desktop.json findings 4, 5, 10, 13, 14)."""
import io
import zipfile

from lxml import etree

import builders
from harness import EngineCase


class QaRegressionTests(EngineCase):
    def test_finding4_docx_creation_produces_valid_xml(self):
        stage = self.stage()
        receipt = self.ok(stage, 'document_write', 'create', {'format': 'docx', 'blocks': [
            {'type': 'paragraph', 'text': 'A\u0001B\u000bC￾D'},
            {'type': 'table', 'rows': [['x\u0002y']]}], 'properties': {'title': 'T\u0003'}}, output='a.docx')
        self.assertTrue(any('control characters' in d['message'] for d in receipt['diagnostics']))
        with zipfile.ZipFile(io.BytesIO(stage.read('out/a.docx'))) as archive:
            for name in archive.namelist():
                if name.endswith(('.xml', '.rels')):
                    etree.fromstring(archive.read(name))  # raises if not well formed
        stage.inputs = []
        stage.add('a.docx', stage.read('out/a.docx'))
        self.assertEqual(self.ok(stage, 'document_read', 'text')['result']['text'], 'ABCD\nxy')

    def test_finding5_unicode_pdf_content_is_kept_or_reported(self):
        stage = self.stage()
        text = 'Grüße — 中文 Привет'
        receipt = self.ok(stage, 'document_write', 'create_pdf', {'markdown': text}, output='u.pdf',
                          phase='latex_prepare')
        tex = stage.read('work/latex/main.tex').decode('utf-8')
        self.assertIn('Grüße — 中文 Привет', tex)  # passed through as Unicode to XeLaTeX
        self.assertIn('xeCJK', tex)
        self.assertIn('NotoSerif', tex)
        self.assertTrue(receipt['result']['cjk'])
        fallback = self.stage()
        receipt = self.ok(fallback, 'document_write', 'create_pdf', {'markdown': text}, output='f.pdf',
                          options={'latex_available': False})
        extracted = self.assert_pdf_unicode_kept_or_reported(receipt, text, fallback.read('out/f.pdf'))
        self.assertIn('Grüße — 中文', extracted)

    def test_finding10_docx_reading_keeps_tabs_and_breaks(self):
        stage = self.stage()
        stage.add('t.docx', builders.docx_tabs_breaks())
        self.assertEqual(self.ok(stage, 'document_read', 'text')['result']['text'], 'alpha\tbeta\ngamma\nsecond')
        items = self.ok(stage, 'document_read', 'inspect')['result']['items']
        self.assertEqual(items[0]['text'], 'alpha\tbeta\ngamma')

    def test_finding13_pptx_follows_presentation_order(self):
        stage = self.stage()
        stage.add('r.pptx', builders.pptx_reordered_runs())
        text = self.ok(stage, 'document_read', 'text')['result']['text']
        self.assertLess(text.index('Second slide'), text.index('Quantization'))
        self.assertTrue(text.startswith('## Slide 1\nSecond slide'))
        items = self.ok(stage, 'document_read', 'inspect')['result']['items']
        self.assertEqual(items[0]['shapes'][0]['text'], 'Second slide')

    def test_finding14_pptx_words_are_not_split_at_runs(self):
        stage = self.stage()
        stage.add('r.pptx', builders.pptx_reordered_runs())
        text = self.ok(stage, 'document_read', 'text')['result']['text']
        self.assertIn('\nQuantization', text)
        self.assertNotIn('Quan\ntization', text)
