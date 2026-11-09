"""Synthetic inputs for the engine tests: sample documents, QA-bug reproductions and hostile files."""
import io
import struct
import zipfile
import zlib

W = 'http://schemas.openxmlformats.org/wordprocessingml/2006/main'
CONTENT_TYPES = ('<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
                 '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
                 '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
                 '<Default Extension="xml" ContentType="application/xml"/>'
                 '<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.'
                 'wordprocessingml.document.main+xml"/></Types>')
ROOT_RELS = ('<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
             '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
             '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/'
             'officeDocument" Target="word/document.xml"/></Relationships>')


def raw_docx(body_xml, document_xml=None, extra=None, document_bytes=None):
    """A minimal valid WordprocessingML package around body_xml."""
    document = document_bytes or (document_xml or (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="%s"><w:body>%s'
        '<w:sectPr/></w:body></w:document>' % (W, body_xml))).encode('utf-8')
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, 'w', zipfile.ZIP_DEFLATED) as archive:
        archive.writestr('[Content_Types].xml', CONTENT_TYPES)
        archive.writestr('_rels/.rels', ROOT_RELS)
        archive.writestr('word/document.xml', document)
        for name, data in (extra or {}).items():
            archive.writestr(name, data)
    return buffer.getvalue()


def docx_tabs_breaks():
    """QA bug: alpha<w:tab/>beta<w:br/>gamma must read as 'alpha\\tbeta\\ngamma'."""
    return raw_docx('<w:p><w:r><w:t>alpha</w:t><w:tab/><w:t>beta</w:t><w:br/><w:t>gamma</w:t></w:r></w:p>'
                    '<w:p><w:pPr><w:tabs><w:tab w:val="left" w:pos="720"/></w:tabs></w:pPr>'
                    '<w:r><w:t>second</w:t></w:r></w:p>')


def docx_with_placeholders():
    import docx
    document = docx.Document()
    paragraph = document.add_paragraph()
    paragraph.add_run('Dear {{')
    bold = paragraph.add_run('customer')
    bold.bold = True
    paragraph.add_run('}}, your order {{order}} ships today.')
    document.add_paragraph('Keep me')
    table = document.add_table(rows=2, cols=2)
    table.cell(0, 0).text = 'Item'
    table.cell(0, 1).text = 'Qty'
    table.cell(1, 0).text = 'Apple'
    table.cell(1, 1).text = '3'
    run = document.add_paragraph().add_run('Formatted original')
    run.italic = True
    document.sections[0].header.paragraphs[0].text = 'Header for {{customer}}'
    buffer = io.BytesIO()
    document.save(buffer)
    return buffer.getvalue()


def xxe_docx():
    document = ('<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE d [<!ENTITY xxe SYSTEM "file:///C:/Windows/win.ini">]>'
                '<w:document xmlns:w="%s"><w:body><w:p><w:r><w:t>&xxe;</w:t></w:r></w:p></w:body></w:document>' % W)
    return raw_docx('', document_xml=document)


def utf16_doctype_docx():
    document = ('<?xml version="1.0" encoding="UTF-16"?><!DOCTYPE d [<!ENTITY a "aaaa">]>'
                '<w:document xmlns:w="%s"><w:body><w:p><w:r><w:t>&a;</w:t></w:r></w:p></w:body></w:document>' % W)
    return raw_docx('', document_bytes=document.encode('utf-16'))


def billion_laughs_xlsx():
    import openpyxl
    book = openpyxl.Workbook()
    book.active['A1'] = 'x'
    buffer = io.BytesIO()
    book.save(buffer)
    laughs = ('<?xml version="1.0"?><!DOCTYPE lolz [<!ENTITY lol "lol">'
              + ''.join('<!ENTITY lol%d "%s">' % (i, ('&lol%d;' % (i - 1) if i > 1 else '&lol;') * 10)
                        for i in range(1, 10))
              + ']><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><si><t>&lol9;</t></si></sst>')
    return replace_entry(buffer.getvalue(), 'xl/sharedStrings.xml', laughs.encode('utf-8'), add=True)


def replace_entry(data, name, content, add=False):
    out = io.BytesIO()
    with zipfile.ZipFile(io.BytesIO(data)) as source, zipfile.ZipFile(out, 'w', zipfile.ZIP_DEFLATED) as target:
        seen = False
        for info in source.infolist():
            if info.filename == name:
                target.writestr(info, content)
                seen = True
            else:
                target.writestr(info, source.read(info))
        if add and not seen:
            target.writestr(name, content)
    return out.getvalue()


def sample_docx():
    import docx
    document = docx.Document()
    document.add_paragraph('Hello')
    buffer = io.BytesIO()
    document.save(buffer)
    return buffer.getvalue()


def zip_bomb_docx():
    """A valid docx plus one entry that expands to 70 MiB (limit: 64 MiB)."""
    out = io.BytesIO()
    with zipfile.ZipFile(io.BytesIO(sample_docx())) as source, zipfile.ZipFile(out, 'w', zipfile.ZIP_DEFLATED) as target:
        for info in source.infolist():
            target.writestr(info, source.read(info))
        target.writestr('word/media/zeros.bin', b'\0' * (70 * 1024 * 1024))
    return out.getvalue()


def many_entries_docx():
    out = io.BytesIO()
    with zipfile.ZipFile(io.BytesIO(sample_docx())) as source, zipfile.ZipFile(out, 'w', zipfile.ZIP_STORED) as target:
        for info in source.infolist():
            target.writestr(info, source.read(info))
        for index in range(10_001):
            target.writestr('customXml/x%d.bin' % index, b'')
    return out.getvalue()


def png_header(width, height):
    """Only a PNG signature and IHDR: enough for Pillow to read the size."""
    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data) & 0xffffffff)
    return (b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, 8, 2, 0, 0, 0))
            + chunk(b'IDAT', zlib.compress(b'')) + chunk(b'IEND', b''))


def image(width=64, height=48, color=(200, 30, 30), mode='RGB', fmt='PNG', exif=None, **params):
    from PIL import Image
    picture = Image.new(mode, (width, height), color)
    if mode in ('RGB', 'RGBA'):
        for x in range(min(width, 16)):
            picture.putpixel((x, 0), (0, 0, 255) if mode == 'RGB' else (0, 0, 255, 255))
    buffer = io.BytesIO()
    if exif is not None:
        params['exif'] = exif
    picture.save(buffer, fmt, **params)
    return buffer.getvalue()


def jpeg_with_exif(orientation=6, width=60, height=40):
    from PIL import Image
    exif = Image.Exif()
    exif[0x010F] = 'SecretCam'
    exif[0x010E] = 'secret-location'
    exif[0x0112] = orientation
    gps = exif.get_ifd(0x8825)
    gps[1] = 'N'
    gps[2] = (48.0, 8.0, 30.0)
    return image(width, height, fmt='JPEG', exif=exif.tobytes(), quality=95)


def sample_pdf(pages=3, label='Page', size=(595, 842)):
    from reportlab.pdfgen import canvas
    buffer = io.BytesIO()
    drawing = canvas.Canvas(buffer, pagesize=size)
    for number in range(1, pages + 1):
        drawing.setFont('Helvetica', 24)
        drawing.drawString(72, size[1] - 100, '%s %d' % (label, number))
        drawing.showPage()
    drawing.save()
    return buffer.getvalue()


def form_pdf(name='', agree=False, extra_page=False):
    from reportlab.pdfgen import canvas
    buffer = io.BytesIO()
    drawing = canvas.Canvas(buffer, pagesize=(595, 842))
    drawing.drawString(72, 760, 'Form')
    drawing.acroForm.textfield(name='Name', x=72, y=700, width=200, height=20, value=name)
    drawing.acroForm.checkbox(name='Agree', x=72, y=650, size=16, checked=agree, buttonStyle='check')
    drawing.showPage()
    if extra_page:
        drawing.drawString(72, 760, 'Second page')
        drawing.showPage()
    drawing.save()
    return buffer.getvalue()


def encrypted_pdf(user='secret', owner='owner'):
    from pypdf import PdfReader, PdfWriter
    writer = PdfWriter(clone_from=PdfReader(io.BytesIO(sample_pdf(1))))
    writer.encrypt(user_password=user, owner_password=owner, algorithm='RC4-128')
    buffer = io.BytesIO()
    writer.write(buffer)
    return buffer.getvalue()


def pptx_reordered_runs():
    """QA bugs: slides reordered via sldIdLst, and a word split over two runs (Quan + bold tization)."""
    import pptx
    presentation = pptx.Presentation()
    first = presentation.slides.add_slide(presentation.slide_layouts[5])
    first.shapes.title.text = ''
    paragraph = first.shapes.title.text_frame.paragraphs[0]
    paragraph.add_run().text = 'Quan'
    bold = paragraph.add_run()
    bold.text = 'tization'
    bold.font.bold = True
    second = presentation.slides.add_slide(presentation.slide_layouts[5])
    second.shapes.title.text = 'Second slide'
    ids = presentation.slides._sldIdLst
    entries = list(ids)
    ids.remove(entries[1])
    ids.insert(0, entries[1])
    buffer = io.BytesIO()
    presentation.save(buffer)
    return buffer.getvalue()


def sample_pptx():
    import pptx
    from pptx.util import Pt
    presentation = pptx.Presentation()
    for title in ('Intro {{event}}', 'Middle', 'End'):
        slide = presentation.slides.add_slide(presentation.slide_layouts[1])
        slide.shapes.title.text = title
        run = slide.shapes.title.text_frame.paragraphs[0].runs[0]
        run.font.size = Pt(40)
        slide.placeholders[1].text = 'Body of ' + title
    buffer = io.BytesIO()
    presentation.save(buffer)
    return buffer.getvalue()


def sample_xlsx():
    import openpyxl
    book = openpyxl.Workbook()
    sheet = book.active
    sheet.title = 'Data'
    sheet.append(['Name', 'Qty', 'Price'])
    sheet.append(['Apple', 3, 0.5])
    sheet.append(['Pear', 5, 0.75])
    sheet['D2'] = '=B2*C2'
    sheet['A6'] = 'Hello {{who}}'
    second = book.create_sheet('Other')
    second['A1'] = 'x'
    buffer = io.BytesIO()
    book.save(buffer)
    return buffer.getvalue()
