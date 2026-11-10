"""create_pdf without LaTeX (phase null, options.latex_available false): ReportLab Platypus renderer."""
import io

from pypdf import PdfReader

import builders
from harness import EngineCase

MARKDOWN = '''# Einleitung
Text mit **fett**, *kursiv*, `code` und einem [Link](https://example.org).
Zeile mit Grüßen — „Anführung“ € 100 %.

## Liste
1. eins
2. zwei
   - zwei a

| Name | Menge |
|------|------:|
| Apfel | 3 |

> Ein Zitat

```
code block <b>not markup</b> & stuff
```

$$a^2 + b^2$$

\\newpage
# Zweiter Teil
Schluss.
'''


class FallbackPdfTests(EngineCase):
    def render(self, spec, stage=None, options=None):
        stage = stage or self.stage()
        receipt = self.ok(stage, 'document_write', 'create_pdf', spec, output='doc.pdf',
                          options=dict({'latex_available': False}, **(options or {})))
        return receipt, PdfReader(io.BytesIO(stage.read('out/doc.pdf'))), stage

    def test_markdown_document(self):
        receipt, reader, _ = self.render({'title': 'Bericht', 'author': 'Ada', 'date': '19. Oktober 2026',
                                          'markdown': MARKDOWN, 'toc': True}, options={'preview': True})
        result = receipt['result']
        self.assertEqual(result['renderer'], 'reportlab')
        self.assertEqual(receipt['qa']['renderer'], 'reportlab')
        self.assertEqual(result['missing_characters'], 0, receipt['diagnostics'])
        self.assertEqual(len(reader.pages), result['pages'])
        self.assertGreaterEqual(len(reader.pages), 3)
        text = '\n'.join(page.extract_text() for page in reader.pages)
        for needle in ('Bericht', 'Ada', 'Einleitung', 'fett', 'Grüßen', '€', 'eins', 'zwei a', 'Apfel',
                       'Ein Zitat', 'code block <b>not markup</b> & stuff', 'a^2 + b^2', 'Schluss', '19. Oktober 2026'):
            self.assertIn(needle, text)
        self.assertEqual(reader.metadata.title, 'Bericht')
        self.assertEqual(reader.metadata.author, 'Ada')
        outline = [item.title for item in reader.outline if not isinstance(item, list)]
        self.assertIn('Einleitung', outline)
        self.assertTrue(receipt['previews'])
        self.assertTrue(any('LaTeX source' in d['message'] for d in receipt['diagnostics']))
        links = [annotation.get_object().get('/A', {}).get('/URI') for page in reader.pages
                 for annotation in (page.get('/Annots') or [])]
        self.assertIn('https://example.org', links)

    def test_blocks_tables_images_and_page_numbers(self):
        stage = self.stage()
        stage.add('photo.jpg', builders.jpeg_with_exif(orientation=1, width=400, height=200))
        receipt, reader, _ = self.render({'paper': 'a5', 'margins_cm': 1.5, 'font': 'sans', 'font_size': 10,
                                          'blocks': [
                                              {'type': 'heading', 'text': 'Tabelle', 'level': 1},
                                              {'type': 'table', 'rows': [['A', 'B', 'C'],
                                                                         [{'text': 'span', 'colspan': 2,
                                                                           'fill': 'FFF2CC'}, 'c']] +
                                               [['r%d' % i, str(i), 'x'] for i in range(80)],
                                               'widths_cm': [3, 3], 'caption': 'Tab 1'},
                                              {'type': 'image', 'input': 0, 'width_cm': 8, 'caption': 'Bild'},
                                              {'type': 'paragraph', 'text': 'Rechts', 'align': 'right'},
                                          ]}, stage=stage)
        self.assertGreaterEqual(len(reader.pages), 3)
        self.assertAlmostEqual(float(reader.pages[0].mediabox.width), 419.5, delta=1)
        second = reader.pages[1].extract_text()
        self.assertIn('A', second.split())  # header row repeats on the next page
        self.assertIn('2', second.split())  # page number
        self.assertEqual(receipt['result']['images'], 1)
        self.assertEqual(receipt['result']['tables'], 1)

    def test_missing_characters_are_reported(self):
        source = 'Привет мир und Ελληνικά \u0378'
        receipt, reader, stage = self.render({'markdown': source})
        self.assertGreaterEqual(receipt['result']['missing_characters'], 1)
        self.assertTrue(any('not in the available font' in d['message'] for d in receipt['diagnostics']))
        self.assertIn('\u0378 U+0378', receipt['qa']['missing_characters'])
        self.assert_pdf_unicode_kept_or_reported(receipt, source, stage.read('out/doc.pdf'))

    def test_cjk_uses_cid_fonts(self):
        receipt, reader, _ = self.render({'markdown': '中文 und 日本語のテキスト und 한국어'})
        self.assertEqual(receipt['result']['missing_characters'], 0)
        fonts = set()
        for page in reader.pages:
            for font in page['/Resources']['/Font'].values():
                fonts.add(str(font.get_object()['/BaseFont']))
        self.assertTrue({'/STSong-Light', '/HeiseiMin-W3', '/HYSMyeongJo-Medium'} <= {f.split('-Identity')[0]
                                                                                     for f in fonts}, fonts)
        self.assertTrue(any('CJK' in d['message'] for d in receipt['diagnostics']))

    def test_refusals(self):
        stage = self.stage()
        self.failed(stage, 'unsupported', 'document_write', 'create_pdf', {'markdown': 'x'}, output='a.pdf',
                    options={'latex_available': True})
        self.failed(stage, 'invalid_spec', 'document_write', 'create_pdf', {'markdown': 'x'}, output='a.docx')
        stage.add('plot.pdf', builders.sample_pdf(1))
        self.failed(stage, 'unsupported', 'document_write', 'create_pdf', {'markdown': '![x](input:0)'},
                    output='a.pdf')

    def test_huge_image_row_is_a_limit(self):
        stage = self.stage()
        receipt = self.failed(stage, 'limit', 'document_write', 'create_pdf', {'blocks': [
            {'type': 'table', 'rows': [['x\n' * 400]]}]}, output='a.pdf')
        self.assertIn('too large for one page', receipt['error']['message'])
