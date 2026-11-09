"""Office/PDF adapters; inputs and outputs stay under one explicit managed order."""
import csv,hashlib,io,json,math,os,pathlib,re,subprocess,sys,tempfile,uuid,zipfile
from xml.sax.saxutils import escape
root=pathlib.Path(sys.argv[1]).resolve(strict=True)
encoded=sys.argv[2];action=json.loads(pathlib.Path(encoded[1:]).read_text(encoding='utf-8') if encoded.startswith('@') else encoded);operation=action['operation']
def path(value,existing=False):
    assert isinstance(value,str) and value and '\0' not in value
    relative=pathlib.Path(value);assert not relative.is_absolute() and '..' not in relative.parts
    result=(root/relative).resolve(strict=existing);assert result.is_relative_to(root)
    if not existing:result.parent.mkdir(parents=True,exist_ok=True);assert not result.exists(),'Output already exists; choose a new document revision'
    return result
input_path=path(action['input'],True) if action.get('input') else None
output_path=path(action['output']) if action.get('output') else None
text=action.get('text') or '';assert len(text.encode())<=1024*1024
model=None;source_sha256=None
if operation.startswith(('inspect_','edit_')):
    assert input_path and input_path.stat().st_size<=32*1024*1024
    original=input_path.read_bytes();source_sha256=hashlib.sha256(original).hexdigest();extension=operation.split('_',1)[1]
    assert input_path.suffix.lower()=='.'+extension and extension in {'docx','xlsx','pptx'},'Choose a matching Office document'
    with zipfile.ZipFile(io.BytesIO(original)) as archive:
        assert len(archive.infolist())<=10000 and sum(item.file_size for item in archive.infolist())<=64*1024*1024,'Expanded Office file exceeds limit'
    payload=json.loads(text or '{}');assert isinstance(payload,dict)
    if operation.startswith('edit_'):
        assert set(payload)<={'source_sha256','patches'} and payload.get('source_sha256')==source_sha256,'Source changed; inspect this exact document revision again'
        patches=payload.get('patches');assert isinstance(patches,list) and 0<len(patches)<=5000 and output_path and output_path.suffix.lower()==input_path.suffix.lower(),'Invalid Office patch set'
    else:
        assert set(payload)<={'offset','limit','sheet'};patches=[]
    offset=payload.get('offset',0);limit=payload.get('limit',20);assert type(offset) is int and offset>=0 and type(limit) is int and 1<=limit<=100
    items=[]
    if extension=='docx':
        from lxml import etree as E
        namespace='http://schemas.openxmlformats.org/wordprocessingml/2006/main';tag=lambda name:'{'+namespace+'}'+name
        with zipfile.ZipFile(io.BytesIO(original)) as archive:xml=archive.read('word/document.xml')
        assert b'<!DOCTYPE' not in xml.upper(),'DOCTYPE is unsupported in an Office document'
        document=E.fromstring(xml,parser=E.XMLParser(resolve_entities=False,no_network=True,huge_tree=False))
        paragraphs=list(document.iter(tag('p')))
        for patch in patches:
            assert isinstance(patch,dict) and set(patch)<={'paragraph','text'} and type(patch.get('paragraph')) is int and 0<=patch['paragraph']<len(paragraphs) and isinstance(patch.get('text'),str) and len(patch['text'].encode())<=128000,'Invalid paragraph edit'
            paragraph=paragraphs[patch['paragraph']];nodes=list(paragraph.iter(tag('t')))
            if not nodes:nodes=[E.SubElement(E.SubElement(paragraph,tag('r')),tag('t'))]
            for node in nodes:node.text=''
            nodes[0].text=patch['text'];nodes[0].set('{http://www.w3.org/XML/1998/namespace}space','preserve')
        if patches:
            with zipfile.ZipFile(io.BytesIO(original)) as archive,zipfile.ZipFile(output_path,'x',zipfile.ZIP_DEFLATED) as output:
                for item in archive.infolist():output.writestr(item,E.tostring(document,encoding='utf-8',xml_declaration=True) if item.filename=='word/document.xml' else archive.read(item))
        else:items=[{'paragraph':index,'text':''.join(node.text or '' for node in paragraph.iter(tag('t')))} for index,paragraph in enumerate(paragraphs)]
    elif extension=='xlsx':
        import openpyxl
        book=openpyxl.load_workbook(io.BytesIO(original),data_only=False,keep_links=True)
        try:
            for patch in patches:
                assert isinstance(patch,dict) and set(patch)<={'sheet','cell','value','type','number_format'} and patch.get('sheet') in book.sheetnames and re.fullmatch(r'[A-Z]{1,3}[1-9][0-9]{0,6}',patch.get('cell','')),'Invalid spreadsheet edit'
                cell=book[patch['sheet']][patch['cell']];value=patch.get('value');assert value is None or isinstance(value,(str,int,float,bool));assert not isinstance(value,float) or math.isfinite(value)
                assert not isinstance(value,str) or len(value.encode())<=64000
                if patch.get('type')=='formula':assert isinstance(value,str) and value.startswith('=');cell.value=value
                else:
                    assert patch.get('type') in {None,'text','number','boolean','empty'};cell.value=value
                    if isinstance(value,str):cell.data_type='s'
                if 'number_format' in patch:assert isinstance(patch['number_format'],str) and len(patch['number_format'])<=200;cell.number_format=patch['number_format']
            if patches:
                with output_path.open('xb') as output:book.save(output)
            else:
                for sheet in book:
                    if payload.get('sheet') and payload['sheet']!=sheet.title:continue
                    assert sheet.max_row<=100000 and sheet.max_column<=1024 and sheet.max_row*sheet.max_column<=1000000,'Choose a bounded spreadsheet'
                    for row in sheet:
                        for cell in row:
                            if cell.value is not None:
                                value=cell.value if isinstance(cell.value,(str,int,float,bool)) else str(cell.value)
                                items.append({'sheet':sheet.title,'cell':cell.coordinate,'value':value,'type':'formula' if cell.data_type=='f' else 'boolean' if isinstance(value,bool) else 'number' if isinstance(value,(int,float)) else 'text','number_format':cell.number_format})
                                assert len(items)<=100000,'Spreadsheet inspection exceeds limit'
        finally:book.close()
    else:
        from pptx import Presentation
        document=Presentation(io.BytesIO(original));assert len(document.slides)<=200
        for patch in patches:
            assert isinstance(patch,dict) and set(patch)<={'slide','shape','text'} and type(patch.get('slide')) is int and 0<=patch['slide']<len(document.slides) and isinstance(patch.get('text'),str) and len(patch['text'].encode())<=128000,'Invalid slide edit'
            shape=next((shape for shape in document.slides[patch['slide']].shapes if shape.shape_id==patch.get('shape')),None);assert shape is not None and shape.has_text_frame,'Choose a text shape';shape.text=patch['text']
        if patches:
            with output_path.open('xb') as output:document.save(output)
        else:
            for index,slide in enumerate(document.slides):
                for shape in slide.shapes:
                    if shape.has_text_frame:items.append({'slide':index,'shape':shape.shape_id,'name':shape.name,'text':shape.text})
    if not patches:
        selected=items[offset:offset+limit]
        for item in selected:
            if isinstance(item.get('text'),str) and len(item['text'].encode())>128000:item['text']=item['text'][:16000];item['truncated']=True
        model={'format':extension,'source_sha256':source_sha256,'input':action['input'],'items':selected,'total':len(items),'offset':offset,'next_offset':offset+limit if offset+limit<len(items) else None}
        assert len(json.dumps(model,ensure_ascii=False).encode())<=900000,'Choose a smaller inspection page'
    text=''
elif operation=='snapshot':
    assert input_path and not output_path and input_path.stat().st_size<=32*1024*1024
    source_sha256=hashlib.sha256(input_path.read_bytes()).hexdigest()
    assert json.loads(text).get('source_sha256')==source_sha256,'Source changed; open this exact document revision again'
    text=''
elif operation=='read_text':
    assert input_path and input_path.stat().st_size<=8*1024*1024
    text=input_path.read_text(encoding='utf-8')
elif operation=='read_docx':
    assert input_path and input_path.stat().st_size<=32*1024*1024
    import xml.etree.ElementTree as E
    with zipfile.ZipFile(input_path) as archive:
        info=archive.getinfo('word/document.xml');assert info.file_size<=16*1024*1024
        document=E.fromstring(archive.read(info))
    text='\n'.join(''.join(p.itertext()) for p in document.iter('{http://schemas.openxmlformats.org/wordprocessingml/2006/main}p'))
elif operation=='read_xlsx':
    import openpyxl
    assert input_path and input_path.stat().st_size<=32*1024*1024
    with zipfile.ZipFile(input_path) as archive:
        assert len(archive.infolist())<=10000 and sum(item.file_size for item in archive.infolist())<=64*1024*1024,'Expanded workbook exceeds limit'
    book=openpyxl.load_workbook(input_path,read_only=True,data_only=True)
    try:
        lines=[]
        for sheet in book:
            lines.append('['+sheet.title+']')
            for i,row in enumerate(sheet.iter_rows(values_only=True)):
                assert i<10000;lines.append('\t'.join('' if v is None else str(v) for v in row))
                assert sum(len(line) for line in lines)<=1024*1024,'Workbook text exceeds limit'
        text='\n'.join(lines)
    finally:book.close()
elif operation=='read_pdf':
    from pypdf import PdfReader
    assert input_path and input_path.stat().st_size<=32*1024*1024
    book=PdfReader(input_path);assert len(book.pages)<=200
    text='\n'.join(page.extract_text() or '' for page in book.pages)
elif operation=='read_pptx':
    import re,xml.etree.ElementTree as E
    assert input_path and input_path.stat().st_size<=32*1024*1024
    with zipfile.ZipFile(input_path) as archive:
        selected=[item for item in archive.infolist() if re.fullmatch(r'ppt/slides/slide[0-9]+\.xml',item.filename)]
        assert len(selected)<=200 and sum(item.file_size for item in selected)<=16*1024*1024
        slides=[]
        for item in sorted(selected,key=lambda item:int(re.search(r'([0-9]+)\.xml$',item.filename)[1])):
            slide=E.fromstring(archive.read(item));slides.append('\n'.join(node.text or '' for node in slide.iter('{http://schemas.openxmlformats.org/drawingml/2006/main}t')))
        text='\n\n'.join(slides)
elif operation=='write_text':
    assert output_path;output_path.write_text(text,encoding='utf-8')
elif operation=='create_docx':
    assert output_path
    content=''.join('<w:p><w:r><w:t xml:space="preserve">'+escape(line)+'</w:t></w:r></w:p>' for line in text.splitlines())
    with zipfile.ZipFile(output_path,'x',zipfile.ZIP_DEFLATED) as archive:
        archive.writestr('[Content_Types].xml','<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>')
        archive.writestr('_rels/.rels','<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>')
        archive.writestr('word/document.xml','<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>'+content+'</w:body></w:document>')
elif operation=='create_xlsx':
    import openpyxl
    assert output_path
    book=openpyxl.Workbook();sheet=book.active
    for row_number,row in enumerate(csv.reader(io.StringIO(text)),1):
        assert row_number<=10000 and len(row)<=256
        # Preserve imported/user values as text, avoiding spreadsheet formula activation.
        for index,value in enumerate(row,1):
            cell=sheet.cell(row_number,index);cell.value=value;cell.data_type='s'
    book.save(output_path);book.close()
elif operation=='create_pptx':
    from pptx import Presentation
    assert output_path
    document=Presentation();parts=text.split('\n\n')
    assert len(parts)<=200
    for number,part in enumerate(parts,1):
        slide=document.slides.add_slide(document.slide_layouts[1])
        slide.shapes.title.text=(action.get('title') or 'LocalAI Cowork')+' '+str(number)
        slide.placeholders[1].text=part
    document.save(output_path)
elif operation=='create_pdf':
    from reportlab.pdfgen import canvas
    import textwrap
    assert output_path
    document=canvas.Canvas(str(output_path));document.setTitle(action.get('title') or 'LocalAI Cowork document')
    y=800;pages=1;document.setFont('Helvetica',11)
    for line in text.splitlines():
        for wrapped in textwrap.wrap(line,width=85,replace_whitespace=False) or ['']:
            if y<40:
                pages+=1;assert pages<=200,'Generated PDF exceeds200pages'
                document.showPage();document.setFont('Helvetica',11);y=800
            document.drawString(40,y,wrapped);y-=16
    document.save()
elif operation=='convert':
    assert input_path and output_path
    source_sha256=hashlib.sha256(input_path.read_bytes()).hexdigest()
    assert json.loads(text).get('source_sha256')==source_sha256,'Source changed; open this exact document revision again'
    text=''
    executable=action.get('libreoffice');assert executable and pathlib.Path(executable).is_file(),'Configure LibreOffice for native Office conversion'
    with tempfile.TemporaryDirectory(dir=root) as profile,tempfile.TemporaryDirectory(dir=root) as output:
        command=[executable,'--headless','-env:UserInstallation='+pathlib.Path(profile).as_uri(),
            '--convert-to',output_path.suffix.removeprefix('.'),'--outdir',output,str(input_path)]
        child=subprocess.Popen(command,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
        stdout,stderr=child.communicate(timeout=60);assert child.returncode==0
        generated=pathlib.Path(output)/(input_path.stem+output_path.suffix);assert generated.is_file()
        output_path.write_bytes(generated.read_bytes())
else:raise ValueError('Unknown Office operation')
print(json.dumps({'operation':operation,'text':text[:1024*1024],'model':model,'source_sha256':source_sha256,
    'text_truncated':len(text)>1024*1024,'output':output_path.relative_to(root).as_posix() if output_path else None,
    'output_bytes':output_path.stat().st_size if output_path and output_path.exists() else None,
    'output_sha256':hashlib.sha256(output_path.read_bytes()).hexdigest() if output_path and output_path.exists() else None}))
