"""Document preparation and explicit structure/template operations, no ambient Office."""
import hashlib,importlib.util,json,os,pathlib,sys,zipfile,xml.etree.ElementTree as ET
d=pathlib.Path(sys.argv[1]).resolve(strict=True);r=json.loads((d/sys.argv[2]).read_text(encoding='utf-8'))
spec=importlib.util.spec_from_file_location('device_document_policy',r['policy_helper']);policy=importlib.util.module_from_spec(spec);spec.loader.exec_module(policy)
def emit(v):print(json.dumps({'ok':True,**v},ensure_ascii=False))
if r['phase']=='dependencies':
    import importlib.metadata
    packages={}
    for name in r['packages']:
        if name not in {'python-docx','python-pptx','openpyxl','reportlab','pypdf'}:raise ValueError('office_dependency_name')
        packages[name]=importlib.metadata.version(name)
    emit({'packages':packages});sys.exit(0)
if r['phase']=='inspect':
    raw=(d/r['input_name']).read_bytes();emit({'policy':policy.inspect(raw,r['extension'])});sys.exit(0)
if r['phase']=='generic':
    import contextlib,io,runpy
    a=dict(r['arguments']);op=a['operation'];destination=a.get('output')
    if op not in {'read_text','read_docx','read_xlsx','read_pdf','read_pptx','write_text','create_docx','create_xlsx','create_pptx','create_pdf','convert'}:raise ValueError('office_operation')
    if a.get('input'):a['input']=r['input_name']
    if destination:a['output']='generated.'+pathlib.PurePosixPath(destination).suffix.removeprefix('.')
    if r.get('libreoffice'):a['libreoffice']=r['libreoffice']
    encoded=d/'generic-input.json'
    with encoded.open('x',encoding='utf-8') as f:json.dump(a,f,ensure_ascii=False)
    if op=='convert':
        import subprocess,threading,time,tempfile
        if not r.get('libreoffice'):raise ValueError('office_selected_converter_missing')
        source=d/a['input'];out=d/a['output']
        with tempfile.TemporaryDirectory(dir=d) as profile,tempfile.TemporaryDirectory(dir=d) as converted:
            command=[r['libreoffice'],'--headless','-env:UserInstallation='+pathlib.Path(profile).as_uri(),'--convert-to',out.suffix[1:],'--outdir',converted,str(source)]
            child=subprocess.Popen(command,stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
            overflow=threading.Event();closed=[];guard=threading.Lock();total=[0]
            def drain(stream):
                try:
                    while True:
                        raw=stream.read(16384)
                        if not raw:break
                        with guard:
                            total[0]+=len(raw)
                            if total[0]>1024*1024:overflow.set();break
                finally:stream.close()
            threads=[threading.Thread(target=drain,args=(stream,),daemon=True) for stream in [child.stdout,child.stderr]]
            for thread in threads:thread.start()
            deadline=time.monotonic()+60
            try:
                while child.poll() is None:
                    if overflow.is_set() or time.monotonic()>=deadline:raise ValueError('office_converter_bound')
                    time.sleep(.025)
                for thread in threads:thread.join(max(0,deadline-time.monotonic()))
                if child.returncode or overflow.is_set() or any(t.is_alive() for t in threads):raise ValueError('office_converter_capture_incomplete')
                produced=pathlib.Path(converted)/(source.stem+out.suffix)
                if produced.is_symlink() or not produced.is_file() or produced.stat().st_nlink!=1 or produced.stat().st_size>32*1024*1024:raise ValueError('office_converter_output')
                with produced.open('rb') as src,out.open('xb') as dst:
                    import shutil
                    shutil.copyfileobj(src,dst,1024*1024);dst.flush();os.fsync(dst.fileno())
            finally:
                if child.poll() is None:child.kill()
                child.wait(timeout=5)
        result={'operation':op,'output':out.name,'output_bytes':out.stat().st_size}
    else:
        capture=io.StringIO();sys.argv=[r['office_helper'],str(d),'@'+str(encoded)]
        with contextlib.redirect_stdout(capture):runpy.run_path(r['office_helper'],run_name='__main__')
        result=json.loads(capture.getvalue())
    outputs=[]
    if destination:
        out=d/a['output'];raw=out.read_bytes()
        if len(raw)>32*1024*1024:raise ValueError('office_output_bound')
        if out.suffix.lower() in {'.docx','.xlsx','.pptx','.pdf'}:proof=policy.inspect(raw,out.suffix[1:])
        else:proof={'format':'device-plain-output/1','sha256':hashlib.sha256(raw).hexdigest(),'bytes':len(raw)}
        outputs.append({'file':out.name,'bytes':len(raw),'sha256':hashlib.sha256(raw).hexdigest(),'policy':proof})
    emit({'result':result,'outputs':outputs});sys.exit(0)
a=r['arguments'];mode=a.get('mode','native');fmt=a['format'];text=a.get('title') or 'LocalAI Cowork';outputs=[]
if fmt not in ('docx','pptx','xlsx','pdf') or mode not in ('native','template','parallel'):raise ValueError('office_workflow_shape')
sections=a.get('sections')
if sections is None and 'sections_json' in a:sections=json.loads(a['sections_json'])
if sections is None:sections=[{'title':'','body':v,'bullets':[]} for v in a.get('paragraphs',[])]+[{'title':'','body':'','bullets':a.get('bullets',[])}]
if not isinstance(sections,list) or len(sections)>200:raise ValueError('office_sections_bound')
for s in sections:
    if not isinstance(s,dict) or set(s)-{'title','body','bullets'} or not isinstance(s.get('bullets',[]),list) or len(s.get('bullets',[]))>1000:raise ValueError('office_section_shape')
    if any(not isinstance(v,str) for v in [s.get('title',''),s.get('body',''),*s.get('bullets',[])]):raise ValueError('office_section_text')
if len(json.dumps(sections).encode())>1024*1024:raise ValueError('office_text_bound')
def record(p):
    raw=p.read_bytes()
    if len(raw)>32*1024*1024:raise ValueError('office_output_bound')
    proof=policy.inspect(raw,p.suffix[1:]);outputs.append({'file':p.name,'bytes':len(raw),'sha256':proof['sha256'],'policy':proof})
if mode in ('native','parallel'):
    out=d/('native.'+fmt)
    if fmt=='docx':
        from docx import Document
        document=Document();document.add_heading(text,level=0)
        for s in sections:
            if s.get('title'):document.add_heading(s['title'],level=1)
            if s.get('body'):document.add_paragraph(s['body'])
            for b in s.get('bullets',[]):document.add_paragraph(b,style='List Bullet')
        document.save(out)
    elif fmt=='pptx':
        from pptx import Presentation
        document=Presentation();slide=document.slides.add_slide(document.slide_layouts[0]);slide.shapes.title.text=text
        for s in sections:
            slide=document.slides.add_slide(document.slide_layouts[1]);slide.shapes.title.text=s.get('title','');frame=slide.placeholders[1].text_frame;frame.clear()
            for i,v in enumerate(([s['body']] if s.get('body') else [])+s.get('bullets',[])):
                p=frame.paragraphs[0] if i==0 else frame.add_paragraph();p.text=v;p.level=0
        document.save(out)
    elif fmt=='xlsx':
        import openpyxl
        book=openpyxl.Workbook();sheet=book.active
        for i,s in enumerate(sections,1):
            for column,v in enumerate([s.get('title',''),s.get('body',''),'\n'.join(s.get('bullets',[]))],1):cell=sheet.cell(i,column);cell.value=v;cell.data_type='s'
        book.save(out);book.close()
    else:
        from reportlab.pdfgen import canvas
        import textwrap
        document=canvas.Canvas(str(out));document.setTitle(text);y=800;pages=1
        for v in [text,*[v for s in sections for v in [s.get('title',''),s.get('body',''),*s.get('bullets',[])]]]:
            for line in v.splitlines() or ['']:
                for segment in textwrap.wrap(line,width=85,replace_whitespace=False) or ['']:
                    if y<40:pages+=1;document.showPage();y=800
                    if pages>200:raise ValueError('office_page_bound')
                    document.drawString(40,y,segment);y-=16
        document.save()
    record(out)
if mode in ('template','parallel'):
    if fmt not in ('docx','pptx'):raise ValueError('office_template_format')
    original=(d/r['input_name']).read_bytes();proof=policy.inspect(original,fmt)
    if proof['sha256']!=r['input_sha256']:raise ValueError('office_original_template_changed')
    transforms=a.get('transforms',{})
    if not isinstance(transforms,dict) or len(transforms)>1000 or any(not isinstance(k,str) or not isinstance(v,str) or len(k)>200 for k,v in transforms.items()):raise ValueError('office_transforms')
    out=d/('template.'+fmt);count=0
    with zipfile.ZipFile(d/r['input_name']) as source,zipfile.ZipFile(out,'x',zipfile.ZIP_DEFLATED) as target:
        for item in source.infolist():
            raw=source.read(item)
            if item.filename.endswith('.xml') and (item.filename.startswith('word/') if fmt=='docx' else item.filename.startswith('ppt/')):
                from xml.sax.saxutils import escape
                text=raw.decode('utf-8')
                for key,value in transforms.items():
                    marker='{{'+key+'}}';count+=text.count(marker)
                    text=text.replace(marker,escape(value,{'"':'&quot;',"'":'&apos;'}))
                raw=text.encode('utf-8');ET.fromstring(raw)
            target.writestr(item,raw)
    record(out)
emit({'outputs':outputs,'placeholders_applied':count if mode in ('template','parallel') else 0})
