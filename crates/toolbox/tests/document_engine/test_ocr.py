"""Actual native OCR of raster-only sources; no fake OCR/model responses."""
import io
import os
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont
from pypdf import PdfReader
from reportlab.pdfgen import canvas
from reportlab.lib.utils import ImageReader

from harness import EngineCase


def invoice(label='RECHNUNG R-2026-1042'):
    image = Image.new('RGB', (1400, 1900), 'white')
    draw = ImageDraw.Draw(image)
    font = ImageFont.load_default(size=38)
    for index, text in enumerate([label, 'Muster GmbH', 'Datum: 31.10.2026',
                                  'Nettobetrag: 100,00 EUR', 'Umsatzsteuer 19%: 19,00 EUR',
                                  'Gesamtbetrag: 119,00 EUR', 'Invoice reference: INV-1042']):
        draw.text((80, 100 + index * 100), text, fill='black', font=font)
    return image


def png(image):
    data = io.BytesIO()
    image.save(data, 'PNG')
    return data.getvalue()


def scanned_pdf(images):
    data = io.BytesIO()
    pdf = canvas.Canvas(data, pagesize=(595, 842))
    for image in images:
        pdf.drawImage(ImageReader(image), 0, 0, width=595, height=842)
        pdf.showPage()
    pdf.save()
    result = data.getvalue()
    assert all(not page.extract_text().strip() for page in PdfReader(io.BytesIO(result)).pages)
    return result


class OcrTests(EngineCase):
    def source(self, filename='invoice.png', data=None, models=True):
        stage = self.stage()
        stage.add(filename, png(invoice()) if data is None else data)
        if models:
            pack = Path(os.environ['COWORK_TEST_OCR_MODELS'])
            for language in ('deu', 'eng'):
                stage.write('in/ocr-models/' + language + '.traineddata',
                            (pack / (language + '.traineddata')).read_bytes())
        return stage

    def recognize(self, stage, **options):
        return self.ok(stage, 'document_read', 'ocr', options=options)

    def test_actual_invoice_words_scores_locations_and_source_provenance(self):
        stage = self.source()
        receipt = self.recognize(stage)
        result = receipt['result']
        self.assertIn('R-2026-1042', result['text'])
        self.assertIn('119,00', result['text'])
        self.assertIn('INV-1042', result['text'])
        self.assertEqual(result['source_sha256'], stage.inputs[0]['sha256'])
        self.assertEqual(result['engine']['languages'], ['deu', 'eng'])
        self.assertEqual(result['engine']['binding_version'], '2.12.0')
        self.assertEqual(result['pages'][0]['word_count'], result['total_words'])
        self.assertFalse(result['truncated'])
        self.assertIsNone(result['next_offset'])
        self.assertEqual(receipt['outputs'], [])
        self.assertEqual(result['pages'][0]['preview_sha256'], receipt['previews'][0]['sha256'])
        for word in result['words']:
            self.assertEqual(word['page'], 1)
            self.assertGreater(word['width'], 0)
            self.assertGreater(word['height'], 0)
            self.assertGreaterEqual(word['score'], 0)
            self.assertLessEqual(word['score'], 100)
            self.assertLessEqual(word['x'] + word['width'], result['pages'][0]['width'])
            self.assertLessEqual(word['y'] + word['height'], result['pages'][0]['height'])

    def test_actual_raster_only_pdf_preserves_selected_page_numbers(self):
        stage = self.source('scan.pdf', scanned_pdf([invoice('FIRST PAGE INVOICE'), invoice('SECOND PAGE REPORT')]))
        result = self.recognize(stage, pages=[2, 1])['result']
        self.assertEqual([page['page'] for page in result['pages']], [1, 2])
        self.assertEqual({word['page'] for word in result['words']}, {1, 2})
        self.assertIn('FIRST', result['text'])
        self.assertIn('SECOND', result['text'])
        self.assertEqual(result['total_pages'], 2)
        self.assertFalse(result['pages_truncated'])

    def test_tiff_frame_and_explicit_clockwise_rotation(self):
        data = io.BytesIO()
        invoice('FIRST FRAME').save(data, 'TIFF', save_all=True, append_images=[invoice('SECOND FRAME')])
        stage = self.source('frames.tiff', data.getvalue())
        result = self.recognize(stage, pages=[2])['result']
        self.assertEqual(result['pages'][0]['page'], 2)
        self.assertIn('SECOND', result['text'])
        self.assertNotIn('FIRST', result['text'])
        rotated = self.source('rotated.png', png(invoice().rotate(90, expand=True)))
        result = self.recognize(rotated, rotation=90)['result']
        self.assertIn('119,00', result['text'])
        self.assertEqual(result['pages'][0]['rotation'], 90)

    def test_word_pagination_reassembles_all_original_words(self):
        stage = self.source()
        full = self.recognize(stage)['result']
        chunks, offset = [], 0
        while True:
            result = self.recognize(stage, offset=offset, limit=4)['result']
            self.assertEqual(result['offset'], offset)
            self.assertLessEqual(len(result['words']), 4)
            chunks.extend(result['words'])
            if result['next_offset'] is None:
                self.assertFalse(result['truncated'])
                break
            self.assertTrue(result['truncated'])
            self.assertGreater(result['next_offset'], offset)
            offset = result['next_offset']
        self.assertEqual(chunks, full['words'])

    def test_result_byte_limit_has_a_real_continuation(self):
        stage = self.source()
        receipt = self.ok(stage, 'document_read', 'ocr', limits={'max_result_bytes': 2200})
        result = receipt['result']
        self.assertTrue(result['truncated'])
        self.assertGreater(result['next_offset'], 0)
        next_chunk = self.recognize(stage, offset=result['next_offset'])['result']
        self.assertEqual(next_chunk['words'][0]['index'], result['next_offset'])

    def test_blank_page_is_explicitly_empty(self):
        stage = self.source(data=png(Image.new('RGB', (1000, 1000), 'white')))
        receipt = self.recognize(stage)
        self.assertEqual(receipt['result']['words'], [])
        self.assertEqual(receipt['result']['total_words'], 0)
        self.assertEqual(receipt['result']['text'], '')
        self.assertTrue(any('no readable words' in d['message'] for d in receipt['diagnostics']))

    def test_missing_or_changed_model_pack_never_falls_back(self):
        stage = self.source(models=False)
        self.failed(stage, 'ocr_unavailable', 'document_read', 'ocr')
        stage = self.source()
        model = stage.root / 'in/ocr-models/deu.traineddata'
        data = model.read_bytes()
        model.write_bytes(bytes([data[0] ^ 1]) + data[1:])
        self.failed(stage, 'ocr_unavailable', 'document_read', 'ocr')

    def test_invalid_language_page_rotation_and_offset_are_rejected(self):
        for options in [{'languages': ['fra']}, {'languages': ['eng', 'eng']},
                        {'languages': ['../../eng']}, {'languages': 'eng'},
                        {'rotation': 15}, {'pages': [2]}, {'pages': []}, {'offset': 99999}]:
            with self.subTest(options=options):
                self.failed(self.source(), 'invalid_spec', 'document_read', 'ocr', options=options)

    def test_source_hash_and_pixel_bounds_are_checked(self):
        stage = self.source()
        stage.inputs[0]['sha256'] = '0' * 64
        self.failed(stage, 'source_changed', 'document_read', 'ocr')
        data = io.BytesIO()
        pdf = canvas.Canvas(data, pagesize=(50000, 50000))
        pdf.drawString(10, 10, 'Too large')
        pdf.showPage()
        pdf.save()
        self.failed(self.source('large.pdf', data.getvalue()), 'limit', 'document_read', 'ocr')

    def test_slightly_skewed_scan_still_reports_real_words_and_locations(self):
        stage = self.source(data=png(invoice().rotate(2, expand=True, fillcolor='white')))
        result = self.recognize(stage)['result']
        self.assertIn('119,00', result['text'])
        self.assertTrue(result['words'])
        self.assertGreater(result['pages'][0]['width'], 1400)

    def test_default_page_cap_and_explicit_remaining_page_are_truthful(self):
        blank = Image.new('RGB', (200, 200), 'white')
        stage = self.source('nine.pdf', scanned_pdf([blank] * 9))
        result = self.recognize(stage, dpi=100)['result']
        self.assertEqual(result['total_pages'], 9)
        self.assertEqual([page['page'] for page in result['pages']], list(range(1, 9)))
        self.assertTrue(result['pages_truncated'])
        self.assertTrue(result['truncated'])
        self.assertEqual(result['next_page'], 9)
        remaining = self.recognize(stage, pages=[9], dpi=100)['result']
        self.assertEqual([page['page'] for page in remaining['pages']], [9])
        self.assertFalse(remaining['pages_truncated'])
        self.assertIsNone(remaining['next_page'])

    def test_corrupt_encrypted_and_output_requests_are_rejected(self):
        self.failed(self.source('broken.png', b'not an image'), 'corrupt_input', 'document_read', 'ocr')
        from pypdf import PdfWriter
        writer = PdfWriter()
        writer.add_blank_page(595, 842)
        writer.encrypt('private-test-password')
        buffer = io.BytesIO()
        writer.write(buffer)
        self.failed(self.source('locked.pdf', buffer.getvalue()), 'encrypted', 'document_read', 'ocr')
        self.failed(self.source(), 'invalid_spec', 'document_read', 'ocr', output='unexpected.pdf')
