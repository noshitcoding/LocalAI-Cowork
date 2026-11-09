"""DOCX create/edit/read through the engine, reopened with python-docx."""
import io
import zipfile

import docx
from docx.oxml.ns import qn

import builders
from harness import EngineCase

RICH_BLOCKS = [
    {'type': 'heading', 'text': 'Bericht', 'level': 0},
    {'type': 'heading', 'text': 'Einleitung', 'level': 1},
    {'type': 'paragraph', 'runs': [{'text': 'Fett', 'bold': True}, ' normal ', {'text': 'kursiv', 'italic': True},
                                   ' ', {'text': 'Link', 'link': 'https://example.org/a?b=1&c=2'},
                                   {'text': 'H2O', 'subscript': True}, {'text': 'rot', 'color': 'C00000'},
                                   {'text': 'gelb', 'highlight': 'yellow'}], 'align': 'justify'},
    {'type': 'list', 'ordered': True, 'items': ['eins', {'text': 'zwei', 'items': ['zwei a', 'zwei b']}, 'drei']},
    {'type': 'list', 'ordered': True, 'items': ['neu eins', 'neu zwei']},
    {'type': 'list', 'items': ['Punkt', 'Punkt 2']},
    {'type': 'table', 'rows': [['Name', 'Menge'], ['Apfel', 3], [{'text': 'Summe', 'colspan': 2, 'fill': 'D9E2F3'}]],
     'widths_cm': [6, 3], 'align': ['left', 'right'], 'caption': 'Tabelle 1'},
    {'type': 'image', 'input': 0, 'width_cm': 5, 'caption': 'Bild 1'},
    {'type': 'page_break'},
    {'type': 'code', 'text': 'def f():\n    return 1'},
    {'type': 'quote', 'text': 'Zitat'},
    {'type': 'horizontal_rule'},
    {'type': 'toc', 'levels': 2},
    {'type': 'math', 'latex': 'a^2+b^2=c^2'},
    {'type': 'markdown', 'text': '## Markdown\n\nText mit **fett** und *kursiv*.\n\n- a\n  - b\n\n| x | y |\n|---|--:|\n| 1 | 2 |'},
]


def reopen(data):
    return docx.Document(io.BytesIO(data))


class DocxCreateTests(EngineCase):
    def test_rich_document(self):
        stage = self.stage()
        stage.add('logo.png', builders.image(200, 100))
        receipt = self.ok(stage, 'document_write', 'create', {
            'format': 'docx', 'blocks': RICH_BLOCKS, 'properties': {'title': 'Titel', 'author': 'Ada'},
            'page': {'size': 'a4', 'orientation': 'landscape', 'margins_cm': 2},
            'header': 'Kopf', 'footer': {'text': 'Seite {page} von {pages}', 'align': 'right'}}, output='r.docx')
        self.assertEqual(receipt['outputs'][0]['media_type'],
                         'application/vnd.openxmlformats-officedocument.wordprocessingml.document')
        data = stage.read('out/r.docx')
        document = reopen(data)
        styles = [p.style.name for p in document.paragraphs]
        self.assertIn('Title', styles)
        self.assertIn('Heading 1', styles)
        self.assertIn('List Number 2', styles)
        self.assertEqual(document.core_properties.title, 'Titel')
        self.assertEqual(document.core_properties.author, 'Ada')
        section = document.sections[0]
        self.assertGreater(section.page_width, section.page_height)
        self.assertAlmostEqual(section.page_width.mm, 297, delta=1)
        body = document.element.body.xml
        self.assertIn('w:hyperlink', body)
        self.assertIn('TOC \\o "1-2"', body)
        self.assertIn('w:tblHeader', body)
        self.assertIn('w:fill="D9E2F3"', body)
        self.assertIn('w:gridSpan', body)
        self.assertEqual(len(document.inline_shapes), 1)
        footer = document.sections[0].footer.paragraphs[0]._p.xml
        self.assertIn('PAGE', footer)
        self.assertIn('NUMPAGES', footer)
        paragraph = next(p for p in document.paragraphs if p.text.startswith('Fett'))
        self.assertTrue(paragraph.runs[0].bold)
        with zipfile.ZipFile(io.BytesIO(data)) as archive:
            rels = archive.read('word/_rels/document.xml.rels').decode()
        self.assertIn('https://example.org/a?b=1&amp;c=2', rels)
        self.assertTrue(any('docx-math' in d['message'] or 'Math' in d['message'] for d in receipt['diagnostics']))

    def test_ordered_lists_restart_numbering(self):
        stage = self.stage()
        self.ok(stage, 'document_write', 'create', {'format': 'docx', 'blocks': [
            {'type': 'list', 'ordered': True, 'items': ['a', 'b']},
            {'type': 'paragraph', 'text': 'between'},
            {'type': 'list', 'ordered': True, 'start': 5, 'items': ['c']}]}, output='l.docx')
        document = reopen(stage.read('out/l.docx'))
        ids = [p._p.pPr.numPr.numId.val for p in document.paragraphs if p._p.pPr is not None
               and p._p.pPr.numPr is not None]
        self.assertEqual(len(ids), 3)
        self.assertEqual(ids[0], ids[1])
        self.assertNotEqual(ids[1], ids[2])
        numbering = document.part.numbering_part.element.xml
        self.assertIn('w:startOverride w:val="5"', numbering)

    def test_markdown_only(self):
        stage = self.stage()
        result = self.ok(stage, 'document_write', 'create', {
            'format': 'docx', 'markdown': '# Titel\n\nAbsatz\nweiter.  \nUmbruch\n\n1. a\n2. b\n\n> Zitat\n\n```\ncode\n```'},
            output='m.docx')['result']
        document = reopen(stage.read('out/m.docx'))
        texts = [p.text for p in document.paragraphs]
        self.assertEqual(texts[0], 'Titel')
        self.assertEqual(texts[1], 'Absatz weiter.\nUmbruch')
        self.assertIn('Zitat', texts)
        self.assertGreaterEqual(result['paragraphs'], 5)

    def test_template_and_placeholders(self):
        stage = self.stage()
        template = builders.docx_with_placeholders()
        dotx = builders.replace_entry(template, '[Content_Types].xml', zipfile.ZipFile(io.BytesIO(template)).read(
            '[Content_Types].xml').replace(b'document.main+xml', b'template.main+xml'))
        stage.add('vorlage.dotx', dotx)
        result = self.ok(stage, 'document_write', 'create', {
            'format': 'docx', 'template_input': 0, 'keep_template_content': True,
            'values': {'customer': 'ACME', 'order': 4711, 'unused': 'x'},
            'blocks': [{'type': 'paragraph', 'text': 'Neu'}]}, output='brief.docx')['result']
        self.assertEqual(result['filled'], 3)
        data = stage.read('out/brief.docx')
        with zipfile.ZipFile(io.BytesIO(data)) as archive:
            self.assertIn(b'document.main+xml', archive.read('[Content_Types].xml'))
        document = reopen(data)
        self.assertEqual(document.paragraphs[0].text, 'Dear ACME, your order 4711 ships today.')
        self.assertTrue(document.paragraphs[0].runs[1].bold)
        self.assertEqual(document.sections[0].header.paragraphs[0].text, 'Header for ACME')
        self.assertEqual(document.paragraphs[-1].text, 'Neu')

    def test_template_body_cleared_by_default(self):
        stage = self.stage()
        stage.add('t.docx', builders.docx_with_placeholders())
        self.ok(stage, 'document_write', 'create', {'format': 'docx', 'template_input': 0, 'blocks': ['Only']},
                output='x.docx')
        document = reopen(stage.read('out/x.docx'))
        self.assertEqual([p.text for p in document.paragraphs], ['Only'])
        self.assertEqual(len(document.tables), 0)

    def test_invalid_specs(self):
        stage = self.stage()
        for spec, path in (({'format': 'docx', 'blocks': [{'type': 'chart'}]}, 'spec.blocks[0].type'),
                           ({'format': 'docx', 'blocks': [{'type': 'paragraph', 'runs': [{'text': 'x', 'link':
                                                                                         'javascript:alert(1)'}]}]},
                            'spec.blocks[0].runs[0].link'),
                           ({'format': 'docx', 'blocks': [{'type': 'image', 'input': 3}]}, 'spec.blocks[0].input'),
                           ({'format': 'docx'}, 'spec.blocks')):
            receipt = self.failed(stage, 'invalid_spec', 'document_write', 'create', spec, output='x.docx')
            self.assertEqual(receipt['error']['path'], path)
        self.failed(stage, 'invalid_spec', 'document_write', 'create', {'format': 'docx', 'blocks': ['x']},
                    output='x.pdf')


class DocxEditTests(EngineCase):
    def setUp(self):
        self.st = self.stage()
        self.st.add('src.docx', builders.docx_with_placeholders())
        inspected = self.ok(self.st, 'document_read', 'inspect', options={'limit': 100})['result']
        self.sha = inspected['source_sha256']
        self.items = inspected['items']

    def edit(self, patches, output='e.docx', code=None):
        spec = {'source_sha256': self.sha, 'patches': patches}
        if code:
            return self.failed(self.st, code, 'document_write', 'edit', spec, output=output)
        receipt = self.ok(self.st, 'document_write', 'edit', spec, output=output)
        return receipt, reopen(self.st.read('out/' + output))

    def test_inspect_model(self):
        kinds = [item['kind'] for item in self.items]
        self.assertEqual(kinds, ['paragraph', 'paragraph', 'table', 'paragraph'])
        table = self.items[2]
        self.assertEqual(table['cells'], [['Item', 'Qty'], ['Apple', '3']])
        self.assertEqual(self.items[3]['paragraph'], 6)

    def test_replace_keeps_formatting_and_legacy_form(self):
        receipt, document = self.edit([{'paragraph': 6, 'text': 'Neuer Text'},
                                       {'op': 'replace_paragraph', 'paragraph': 1,
                                        'runs': [{'text': 'Bold', 'bold': True}]}])
        self.assertEqual(receipt['result']['patches_applied'], 2)
        last = document.paragraphs[-1]
        self.assertEqual(last.text, 'Neuer Text')
        self.assertTrue(last.runs[0].italic)
        self.assertTrue(document.paragraphs[1].runs[0].bold)

    def test_insert_delete_and_indices_refer_to_source(self):
        _, document = self.edit([
            {'op': 'insert_paragraph', 'after': 0, 'text': 'After first', 'style': 'Heading 2'},
            {'op': 'insert_blocks', 'before': 1, 'blocks': [{'type': 'list', 'items': ['x', 'y']}]},
            {'op': 'delete_paragraph', 'paragraph': 1},
            {'op': 'append_blocks', 'markdown': '## Ende\nSchluss'},
            {'op': 'insert_page_break', 'after': 6}])
        texts = [p.text for p in document.paragraphs]
        self.assertEqual(texts[:3], ['Dear {{customer}}, your order {{order}} ships today.', 'After first', 'x'])
        self.assertNotIn('Keep me', texts)
        self.assertEqual(texts[-2:], ['Ende', 'Schluss'])
        self.assertEqual(document.paragraphs[1].style.name, 'Heading 2')
        self.edit([{'op': 'delete_paragraph', 'paragraph': 1}, {'op': 'set_style', 'paragraph': 1,
                                                                 'style': 'Heading 1'}], output='f.docx',
                  code='invalid_spec')

    def test_table_ops(self):
        _, document = self.edit([{'op': 'set_cell', 'table': 0, 'row': 1, 'col': 1, 'text': '42'},
                                 {'op': 'add_table_row', 'table': 0, 'values': ['Pear', 7]},
                                 {'op': 'delete_table_row', 'table': 0, 'row': 0}])
        table = document.tables[0]
        self.assertEqual([[c.text for c in row.cells] for row in table.rows], [['Apple', '42'], ['Pear', '7']])
        _, document = self.edit([{'op': 'delete_table', 'table': 0}], output='g.docx')
        self.assertEqual(len(document.tables), 0)

    def test_fill_and_replace(self):
        receipt, document = self.edit([{'op': 'fill_placeholders', 'values': {'customer': 'Beta GmbH'}},
                                       {'op': 'replace_text', 'find': 'ships', 'replace': 'leaves'},
                                       {'op': 'replace_text', 'find': 'nothing-here', 'replace': 'x'},
                                       {'op': 'set_properties', 'title': 'Neu'}])
        self.assertEqual(document.paragraphs[0].text, 'Dear Beta GmbH, your order {{order}} leaves today.')
        self.assertEqual(receipt['result']['unfilled'], ['order'])
        self.assertEqual(document.core_properties.title, 'Neu')
        self.assertTrue(any('nothing-here' in d['message'] for d in receipt['diagnostics']))

    def test_insert_image(self):
        self.st.add('pic.png', builders.image(40, 40))
        _, document = self.edit([{'op': 'insert_image', 'after': 0, 'input': 1, 'width_cm': 3}])
        self.assertEqual(len(document.inline_shapes), 1)

    def test_source_changed(self):
        self.sha = '0' * 64
        self.edit([{'paragraph': 0, 'text': 'x'}], code='source_changed')

    def test_unknown_op(self):
        receipt = self.edit([{'op': 'explode'}], code='invalid_spec')
        self.assertIn('replace_paragraph', receipt['error']['message'])


class DocxReadTests(EngineCase):
    def test_text_includes_tables_textboxes_once(self):
        stage = self.stage()
        stage.add('a.docx', builders.raw_docx(
            '<w:p><w:r><w:t xml:space="preserve">Before </w:t></w:r><w:r><mc:AlternateContent '
            'xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><mc:Choice Requires="wps">'
            '<w:drawing><w:txbxContent><w:p><w:r><w:t>Box</w:t></w:r></w:p></w:txbxContent></w:drawing>'
            '</mc:Choice><mc:Fallback><w:pict><w:txbxContent><w:p><w:r><w:t>Box</w:t></w:r></w:p>'
            '</w:txbxContent></w:pict></mc:Fallback></mc:AlternateContent></w:r></w:p>'
            '<w:tbl><w:tr><w:tc><w:p><w:r><w:t>a</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>b</w:t></w:r></w:p>'
            '</w:tc></w:tr></w:tbl><w:p><w:r><w:delText>gone</w:delText><w:instrText>PAGE</w:instrText>'
            '<w:t>after</w:t></w:r></w:p>'))
        text = self.ok(stage, 'document_read', 'text')['result']['text']
        self.assertEqual(text, 'Before \nBox\na\tb\nafter')
