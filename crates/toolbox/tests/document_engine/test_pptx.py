"""PPTX create/edit/read through the engine, reopened with python-pptx."""
import io

import pptx
from pptx.util import Emu

import builders
from harness import EngineCase


def reopen(data):
    return pptx.Presentation(io.BytesIO(data))


def slide_texts(presentation):
    return [[shape.text_frame.text for shape in slide.shapes if shape.has_text_frame] for slide in presentation.slides]


class PptxCreateTests(EngineCase):
    def test_rich_deck(self):
        stage = self.stage()
        stage.add('photo.png', builders.image(300, 200))
        result = self.ok(stage, 'document_write', 'create', {
            'format': 'pptx', 'properties': {'title': 'Deck'},
            'slides': [
                {'layout': 'title', 'title': 'Quartalsbericht', 'subtitle': 'Oktober 2026'},
                {'title': 'Agenda', 'bullets': ['Ergebnisse', {'text': 'Details', 'level': 1},
                                                {'runs': [{'text': 'fett', 'bold': True}, ' normal']}],
                 'notes': 'Sprechernotiz'},
                {'title': 'Zahlen', 'chart': {'type': 'column', 'categories': ['Q1', 'Q2'],
                                              'series': [{'name': '2026', 'values': [10, 12]}],
                                              'data_labels': True, 'number_format': '0'}},
                {'title': 'Bild', 'text': 'Links Text', 'image': {'input': 0}},
                {'title': 'Tabelle', 'table': {'rows': [['A', 'B'], ['1', '2']]}},
                {'layout': 'two_content', 'title': 'Vergleich', 'left': ['a', 'b'], 'right': {'image': 0}},
                {'layout': 'section', 'title': 'Teil 2'},
                {'layout': 'blank'},
                {'title': 'Streu', 'chart': {'type': 'scatter', 'series': [{'name': 's', 'points': [[1, 2], [3, 4]]}]}},
            ]}, output='d.pptx')['result']
        self.assertEqual(result['slides'], 9)
        self.assertEqual((result['charts'], result['images'], result['tables']), (2, 2, 1))
        presentation = reopen(stage.read('out/d.pptx'))
        self.assertEqual(presentation.slide_width, Emu(12192000))
        texts = slide_texts(presentation)
        self.assertEqual(texts[0], ['Quartalsbericht', 'Oktober 2026'])
        self.assertIn('Ergebnisse\nDetails\nfett normal', texts[1])
        self.assertEqual(presentation.slides[1].notes_slide.notes_text_frame.text, 'Sprechernotiz')
        body = next(s for s in presentation.slides[1].shapes if s.has_text_frame and 'Details' in s.text_frame.text)
        self.assertEqual(body.text_frame.paragraphs[1].level, 1)
        self.assertTrue(body.text_frame.paragraphs[2].runs[0].font.bold)
        for slide in presentation.slides:
            for shape in slide.placeholders:
                self.assertTrue(shape.has_text_frame and shape.text_frame.text.strip() or not shape.has_text_frame,
                                'empty placeholder left on slide')
        self.assertEqual(presentation.core_properties.title, 'Deck')
        title = presentation.slide_master.shapes[0]
        self.assertGreater(title.width, Emu(9144000))  # 4:3 master stretched to 16:9

    def test_aspect_4_3_and_layout_name(self):
        stage = self.stage()
        self.ok(stage, 'document_write', 'create', {'format': 'pptx', 'aspect': '4:3', 'slides': [
            {'layout_name': 'Title Only', 'title': 'x'}]}, output='a.pptx')
        presentation = reopen(stage.read('out/a.pptx'))
        self.assertEqual(presentation.slide_width, Emu(9144000))
        self.assertEqual(presentation.slides[0].slide_layout.name, 'Title Only')
        receipt = self.failed(stage, 'invalid_spec', 'document_write', 'create', {
            'format': 'pptx', 'slides': [{'layout_name': 'Nope'}]}, output='b.pptx')
        self.assertIn('Title Slide', receipt['error']['message'])

    def test_template(self):
        stage = self.stage()
        stage.add('t.pptx', builders.sample_pptx())
        result = self.ok(stage, 'document_write', 'create', {
            'format': 'pptx', 'template_input': 0, 'keep_template_slides': True, 'values': {'event': 'Expo'},
            'slides': [{'title': 'Neu', 'bullets': ['x']}]}, output='t2.pptx')['result']
        self.assertEqual(result['filled'], 2)  # title and body of the first template slide
        texts = slide_texts(reopen(stage.read('out/t2.pptx')))
        self.assertEqual(texts[0][0], 'Intro Expo')
        self.assertEqual(texts[-1], ['Neu', 'x'])


class PptxEditTests(EngineCase):
    def setUp(self):
        self.st = self.stage()
        self.st.add('deck.pptx', builders.sample_pptx())
        self.inspected = self.ok(self.st, 'document_read', 'inspect')['result']

    def edit(self, patches, output='e.pptx', code=None):
        spec = {'source_sha256': self.inspected['source_sha256'], 'patches': patches}
        if code:
            return self.failed(self.st, code, 'document_write', 'edit', spec, output=output)
        receipt = self.ok(self.st, 'document_write', 'edit', spec, output=output)
        return receipt, reopen(self.st.read('out/' + output))

    def test_inspect(self):
        self.assertEqual(self.inspected['slides'], 3)
        first = self.inspected['items'][0]
        self.assertEqual(first['shapes'][0]['kind'], 'placeholder:title')
        self.assertEqual(first['shapes'][0]['text'], 'Intro {{event}}')

    def test_edit_ops(self):
        title_id = self.inspected['items'][0]['shapes'][0]['shape']
        body_id = self.inspected['items'][0]['shapes'][1]['shape']
        _, presentation = self.edit([
            {'slide': 0, 'shape': title_id, 'text': 'Neuer Titel'},
            {'op': 'set_text', 'slide': 0, 'shape': body_id, 'paragraphs': ['a', {'text': 'b', 'level': 1}]},
            {'op': 'add_slide', 'after': 0, 'title': 'Eingefügt 1'},
            {'op': 'add_slide', 'after': 0, 'title': 'Eingefügt 2'},
            {'op': 'add_slide', 'after': -1, 'title': 'Ganz vorne'},
            {'op': 'delete_slide', 'slide': 1},
            {'op': 'move_slide', 'slide': 2, 'after': -1},
            {'op': 'set_notes', 'slide': 0, 'text': 'Notiz'},
            {'op': 'add_textbox', 'slide': 0, 'text': 'Hinweis', 'top_cm': 15},
            {'op': 'replace_text', 'find': 'Body', 'replace': 'Inhalt'}])
        titles = [slide.shapes.title.text if slide.shapes.title is not None else None for slide in presentation.slides]
        self.assertEqual(titles, ['End', 'Ganz vorne', 'Neuer Titel', 'Eingefügt 1', 'Eingefügt 2'])
        edited = presentation.slides[2]
        title_run = edited.shapes.title.text_frame.paragraphs[0].runs[0]
        self.assertEqual(title_run.font.size.pt, 40)
        body = [s for s in edited.shapes if s.has_text_frame and s.shape_id == body_id][0]
        self.assertEqual(body.text_frame.paragraphs[1].level, 1)
        self.assertEqual(edited.notes_slide.notes_text_frame.text, 'Notiz')
        self.assertIn('Inhalt of End', slide_texts(presentation)[0])

    def test_errors(self):
        self.edit([{'op': 'delete_slide', 'slide': 0}, {'op': 'set_notes', 'slide': 0, 'text': 'x'}],
                  code='invalid_spec')
        self.edit([{'slide': 0, 'shape': 999, 'text': 'x'}], code='invalid_spec')
        self.edit([{'op': 'delete_slide', 'slide': i} for i in range(3)], code='invalid_spec')
