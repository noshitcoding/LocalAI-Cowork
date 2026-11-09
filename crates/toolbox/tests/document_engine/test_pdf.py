"""PDF operations (pypdf), rendering (pypdfium2) and PDF reading."""
import io

from pypdf import PdfReader
from PIL import Image

import builders
from harness import EngineCase


def pages_text(data):
    return [page.extract_text().strip() for page in PdfReader(io.BytesIO(data)).pages]


class PdfOperationTests(EngineCase):
    def setUp(self):
        self.st = self.stage()
        self.st.add('a.pdf', builders.sample_pdf(3, 'Alpha'))
        self.st.add('b.pdf', builders.sample_pdf(2, 'Beta'))

    def pdf(self, spec, output, inputs=None):
        if inputs is not None:
            self.st.inputs = [self.st.inputs[i] for i in inputs]
        return self.ok(self.st, 'document_write', 'pdf', spec, output=output)

    def test_merge(self):
        receipt = self.pdf({'action': 'merge', 'properties': {'title': 'Merged'}}, 'm.pdf')
        self.assertEqual(receipt['result']['pages'], 5)
        self.assertEqual(receipt['qa']['pages'], 5)
        data = self.st.read('out/m.pdf')
        self.assertEqual(pages_text(data), ['Alpha 1', 'Alpha 2', 'Alpha 3', 'Beta 1', 'Beta 2'])
        self.assertEqual(PdfReader(io.BytesIO(data)).metadata.title, 'Merged')
        receipt = self.pdf({'action': 'merge', 'items': [{'input': 1, 'pages': '2'}, {'input': 0, 'pages': [1, 3]}]},
                           'm2.pdf')
        self.assertEqual(pages_text(self.st.read('out/m2.pdf')), ['Beta 2', 'Alpha 1', 'Alpha 3'])

    def test_split_into_folder(self):
        receipt = self.pdf({'action': 'split', 'every': 2}, 'parts', inputs=[0])
        self.assertEqual([o['name'] for o in receipt['outputs']], ['out/parts/part-01.pdf', 'out/parts/part-02.pdf'])
        self.assertEqual(pages_text(self.st.read('out/parts/part-02.pdf')), ['Alpha 3'])
        receipt = self.pdf({'action': 'split', 'ranges': ['1', '2-3']}, 'ranges')
        self.assertEqual(len(receipt['outputs']), 2)
        self.failed(self.st, 'invalid_spec', 'document_write', 'pdf', {'action': 'split'}, output='x.pdf')

    def test_extract_pages(self):
        receipt = self.pdf({'action': 'extract_pages', 'pages': '3,1'}, 'sel.pdf', inputs=[0])
        self.assertEqual(pages_text(self.st.read('out/sel.pdf')), ['Alpha 1', 'Alpha 3'])
        receipt = self.pdf({'action': 'extract_pages', 'pages': '2-'}, 'pages')
        self.assertEqual([o['name'] for o in receipt['outputs']], ['out/pages/page-2.pdf', 'out/pages/page-3.pdf'])

    def test_rotate_reorder_delete(self):
        self.st.inputs = self.st.inputs[:1]
        self.pdf({'action': 'rotate', 'degrees': 90, 'pages': '2'}, 'r.pdf')
        reader = PdfReader(io.BytesIO(self.st.read('out/r.pdf')))
        self.assertEqual([page.rotation for page in reader.pages], [0, 90, 0])
        self.pdf({'action': 'reorder', 'order': [3, 1, 2]}, 'o.pdf')
        self.assertEqual(pages_text(self.st.read('out/o.pdf')), ['Alpha 3', 'Alpha 1', 'Alpha 2'])
        self.pdf({'action': 'delete_pages', 'pages': '1,3'}, 'd.pdf')
        self.assertEqual(pages_text(self.st.read('out/d.pdf')), ['Alpha 2'])
        self.failed(self.st, 'invalid_spec', 'document_write', 'pdf', {'action': 'reorder', 'order': [1, 1, 2]},
                    output='x.pdf')
        self.failed(self.st, 'invalid_spec', 'document_write', 'pdf', {'action': 'delete_pages', 'pages': 'all'},
                    output='x.pdf')
        self.failed(self.st, 'invalid_spec', 'document_write', 'pdf', {'action': 'rotate', 'degrees': 45},
                    output='x.pdf')

    def test_watermark(self):
        self.pdf({'action': 'watermark', 'text': 'ENTWURF', 'pages': '1-2'}, 'w.pdf', inputs=[0])
        texts = pages_text(self.st.read('out/w.pdf'))
        self.assertIn('ENTWURF', texts[0])
        self.assertNotIn('ENTWURF', texts[2])
        self.failed(self.st, 'invalid_spec', 'document_write', 'pdf', {'action': 'watermark', 'text': '中文'},
                    output='x.pdf')

    def test_stamp(self):
        self.pdf({'action': 'watermark', 'stamp_input': 1, 'position': 'under'}, 's.pdf')
        self.assertIn('Beta 1', pages_text(self.st.read('out/s.pdf'))[1])

    def test_metadata_and_preview(self):
        self.st.inputs = self.st.inputs[:1]
        receipt = self.ok(self.st, 'document_write', 'pdf', {'action': 'set_metadata', 'properties': {
            'title': 'Titel', 'author': 'Ada'}}, output='meta.pdf', options={'preview': True})
        self.assertEqual(len(receipt['previews']), 3)
        reader = PdfReader(io.BytesIO(self.st.read('out/meta.pdf')))
        self.assertEqual((reader.metadata.title, reader.metadata.author), ('Titel', 'Ada'))


class PdfFormTests(EngineCase):
    def test_page_operations_preserve_selected_form_fields(self):
        source = builders.form_pdf(name='Ada Lovelace', agree=True, extra_page=True)
        cases = [({'action': 'reorder', 'order': [2, 1]}, 'reordered.pdf', 'out/reordered.pdf', 2),
                 ({'action': 'delete_pages', 'pages': '2'}, 'deleted.pdf', 'out/deleted.pdf', 1),
                 ({'action': 'extract_pages', 'pages': '1'}, 'extracted.pdf', 'out/extracted.pdf', 1),
                 ({'action': 'split'}, 'parts', 'out/parts/part-01.pdf', 1),
                 ({'action': 'merge'}, 'merged.pdf', 'out/merged.pdf', 3)]
        for spec, output, result_path, page_count in cases:
            with self.subTest(action=spec['action']):
                stage = self.stage()
                stage.add('form.pdf', source)
                if spec['action'] == 'merge':
                    stage.add('plain.pdf', builders.sample_pdf(1))
                self.ok(stage, 'document_write', 'pdf', spec, output=output)
                reader = PdfReader(io.BytesIO(stage.read(result_path)))
                self.assertEqual(len(reader.pages), page_count)
                fields = reader.get_fields() or {}
                self.assertEqual(fields.get('Name', {}).get('/V'), 'Ada Lovelace')
                self.assertEqual(fields['Agree']['/V'], '/Yes')
                if spec['action'] == 'split':
                    second = PdfReader(io.BytesIO(stage.read('out/parts/part-02.pdf')))
                    self.assertFalse(second.get_fields(), 'removed-page form fields must not survive as orphans')

    def test_merge_refuses_ambiguous_duplicate_form_field_names(self):
        stage = self.stage()
        stage.add('first.pdf', builders.form_pdf(name='Ada'))
        stage.add('second.pdf', builders.form_pdf(name='Grace'))
        self.failed(stage, 'invalid_spec', 'document_write', 'pdf', {'action': 'merge'}, output='merged.pdf')

    def test_filled_form_preview_contains_text_and_checked_checkbox(self):
        stage = self.stage()
        stage.add('form.pdf', builders.form_pdf())
        receipt = self.ok(stage, 'document_write', 'pdf', {
            'action': 'fill_form', 'fields': {'Name': 'Ada Lovelace', 'Agree': True}},
            output='filled.pdf', options={'preview': True})
        self.assertEqual(len(receipt['previews']), 1)
        with Image.open(io.BytesIO(stage.read(receipt['previews'][0]['name']))) as image:
            scale_x, scale_y = image.width / 595, image.height / 842
            def dark_pixels(box):
                left, bottom, right, top = box
                crop = image.crop((round(left * scale_x), round((842 - top) * scale_y),
                                   round(right * scale_x), round((842 - bottom) * scale_y))).convert('L')
                return sum(count for value, count in enumerate(crop.histogram()) if value < 80)
            # Interior regions exclude field borders and the document's ordinary page text.
            self.assertGreater(dark_pixels((75, 703, 260, 717)), 20, 'form text is missing from the preview')
            self.assertGreater(dark_pixels((75, 653, 85, 663)), 3, 'checked form state is missing from the preview')

    def test_fill_form(self):
        stage = self.stage()
        stage.add('form.pdf', builders.form_pdf())
        fields = self.ok(stage, 'document_read', 'inspect')['result']['form_fields']
        self.assertEqual(sorted(field['name'] for field in fields), ['Agree', 'Name'])
        self.ok(stage, 'document_write', 'pdf', {'action': 'fill_form', 'fields': {'Name': 'Ada Lovelace',
                                                                                     'Agree': True}},
                output='filled.pdf')
        values = PdfReader(io.BytesIO(stage.read('out/filled.pdf'))).get_fields()
        self.assertEqual(values['Name'].get('/V'), 'Ada Lovelace')
        self.assertNotEqual(str(values['Agree'].get('/V')), '/Off')
        receipt = self.failed(stage, 'invalid_spec', 'document_write', 'pdf', {
            'action': 'fill_form', 'fields': {'Nope': 'x'}}, output='x.pdf')
        self.assertIn('Name', receipt['error']['message'])


class PdfReadTests(EngineCase):
    def test_text_inspect_preview(self):
        stage = self.stage()
        stage.add('a.pdf', builders.sample_pdf(6))
        result = self.ok(stage, 'document_read', 'text', options={'pages': '2-3'})['result']
        self.assertEqual(result['text'], '## Page 2\nPage 2\n\n## Page 3\nPage 3')
        self.assertEqual(result['pages'], 6)
        inspected = self.ok(stage, 'document_read', 'inspect', options={'limit': 2})['result']
        self.assertEqual([item['page'] for item in inspected['items']], [1, 2])
        self.assertEqual(inspected['next_offset'], 2)
        receipt = self.ok(stage, 'document_read', 'preview', options={'pages': [1, 6]})
        self.assertEqual([p['page'] for p in receipt['previews']], [1, 6])
        self.assertLessEqual(max(receipt['previews'][0]['width'], receipt['previews'][0]['height']), 1600)
        receipt = self.ok(stage, 'document_read', 'preview')
        self.assertEqual(len(receipt['previews']), 4)
        receipt = self.ok(stage, 'document_read', 'preview', options={'pages': 'all'},
                          limits={'preview_max_edge': 300})
        self.assertEqual(len(receipt['previews']), 4)
        self.assertTrue(any('max_preview_images' in d['message'] for d in receipt['diagnostics']))
        self.assertLessEqual(receipt['previews'][0]['height'], 300)

    def test_scanned_pdf_warns(self):
        from reportlab.pdfgen import canvas
        buffer = io.BytesIO()
        drawing = canvas.Canvas(buffer)
        drawing.rect(10, 10, 100, 100, fill=1)
        drawing.showPage()
        drawing.save()
        stage = self.stage()
        stage.add('scan.pdf', buffer.getvalue())
        receipt = self.ok(stage, 'document_read', 'text')
        self.assertTrue(any('OCR' in d['message'] for d in receipt['diagnostics']))

    def test_office_render_phase(self):
        stage = self.stage()
        stage.add('a.docx', builders.sample_docx())
        stage.write('work/office-out/a.pdf', builders.sample_pdf(2))
        receipt = self.ok(stage, 'document_read', 'preview', phase='render_pdf',
                          options={'render_source': 'work/office-out/a.pdf'})
        self.assertEqual(receipt['result']['format'], 'docx')
        self.assertEqual(len(receipt['previews']), 2)
        self.failed(stage, 'unsupported', 'document_read', 'preview')

    def test_office_finalize_phase(self):
        stage = self.stage()
        stage.add('a.docx', builders.sample_docx())
        stage.write('work/office-out/a.pdf', builders.sample_pdf(2))
        receipt = self.ok(stage, 'document_write', 'convert', phase='office_finalize', output='a.pdf',
                          options={'converted': 'work/office-out/a.pdf', 'preview': True})
        self.assertEqual(receipt['qa']['pages'], 2)
        self.assertEqual(len(receipt['previews']), 2)
        stage.write('work/office-out/b.docx', b'not a zip')
        self.failed(stage, 'corrupt_input', 'document_write', 'convert', phase='office_finalize', output='b.docx',
                    options={'converted': 'work/office-out/b.docx'})
