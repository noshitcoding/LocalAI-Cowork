"""Hostile inputs: zip bombs, XXE, billion laughs, huge image headers, encrypted and corrupt files."""
import builders
from harness import EngineCase


class HostileInputTests(EngineCase):
    def read(self, name, data, code, operation='text'):
        stage = self.stage()
        stage.add(name, data)
        return self.failed(stage, code, 'document_read', operation)

    def test_zip_bomb(self):
        receipt = self.read('bomb.docx', builders.zip_bomb_docx(), 'limit')
        self.assertIn('64 MiB', receipt['error']['message'])

    def test_too_many_entries(self):
        self.read('many.docx', builders.many_entries_docx(), 'limit')

    def test_xxe_is_refused(self):
        receipt = self.read('xxe.docx', builders.xxe_docx(), 'corrupt_input')
        self.assertIn('DOCTYPE', receipt['error']['message'])
        self.assertNotIn('fonts', str(receipt))  # nothing of win.ini leaks

    def test_utf16_doctype_is_refused(self):
        self.read('u16.docx', builders.utf16_doctype_docx(), 'corrupt_input')

    def test_billion_laughs(self):
        self.read('lol.xlsx', builders.billion_laughs_xlsx(), 'corrupt_input', 'inspect')

    def test_dtd_in_edit_source(self):
        stage = self.stage()
        stage.add('xxe.docx', builders.xxe_docx())
        self.failed(stage, 'corrupt_input', 'document_write', 'edit', {
            'source_sha256': stage.inputs[0]['sha256'], 'patches': [{'paragraph': 0, 'text': 'x'}]}, output='o.docx')

    def test_huge_image_header(self):
        receipt = self.read('huge.png', builders.png_header(100_000, 100_000), 'limit', 'metadata')
        self.assertIn('pixel', receipt['error']['message'])
        self.read('warn.png', builders.png_header(8000, 8000), 'limit', 'preview')

    def test_image_in_document_is_limited(self):
        stage = self.stage()
        stage.add('huge.png', builders.png_header(100_000, 100_000))
        self.failed(stage, 'limit', 'document_write', 'create', {
            'format': 'docx', 'blocks': [{'type': 'image', 'input': 0}]}, output='a.docx')

    def test_encrypted_pdf(self):
        self.read('locked.pdf', builders.encrypted_pdf(), 'encrypted')
        self.read('locked.pdf', builders.encrypted_pdf(), 'encrypted', 'preview')
        stage = self.stage()
        stage.add('locked.pdf', builders.encrypted_pdf())
        result = self.ok(stage, 'document_read', 'metadata')['result']
        self.assertEqual((result['encrypted'], result['readable']), (True, False))

    def test_owner_only_pdf_reads_but_refuses_writes(self):
        stage = self.stage()
        stage.add('owner.pdf', builders.encrypted_pdf(user=''))
        result = self.ok(stage, 'document_read', 'text')['result']
        self.assertTrue(result['encrypted'])
        self.assertIn('Page 1', result['text'])
        self.failed(stage, 'encrypted', 'document_write', 'pdf', {'action': 'rotate', 'degrees': 90}, output='r.pdf')

    def test_encrypted_office_file(self):
        self.read('enc.docx', b'\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1' + b'\0' * 600, 'encrypted')

    def test_corrupt_files(self):
        self.read('a.docx', b'PK\x03\x04garbage', 'corrupt_input')
        self.read('a.docx', b'plain text', 'corrupt_input')
        self.read('a.pdf', b'%PDF-1.7\n garbage', 'corrupt_input')
        self.read('a.pdf', b'not a pdf', 'corrupt_input')
        self.read('a.png', b'\x89PNG\r\n\x1a\nbroken', 'corrupt_input', 'metadata')
        self.read('a.xlsx', builders.sample_docx(), 'corrupt_input')

    def test_unsupported_types(self):
        self.read('old.doc', b'\0' * 100, 'unsupported')
        self.read('x.exe', b'MZ', 'unsupported')
        stage = self.stage()
        stage.add('a.docx', builders.sample_docx())
        self.failed(stage, 'unsupported', 'document_read', 'preview')
        stage.add('b.docm', builders.sample_docx())
        stage.inputs = stage.inputs[1:]
        self.failed(stage, 'unsupported', 'document_write', 'edit', {
            'source_sha256': stage.inputs[0]['sha256'], 'patches': [{'paragraph': 0, 'text': 'x'}]}, output='o.docx')

    def test_too_many_patches_and_blocks(self):
        stage = self.stage()
        self.failed(stage, 'limit', 'document_write', 'create', {
            'format': 'docx', 'blocks': ['x'] * 5001}, output='a.docx')
        self.failed(stage, 'limit', 'document_write', 'create', {
            'format': 'docx', 'blocks': [{'type': 'paragraph', 'text': 'x' * 130_000}]}, output='a.docx')
