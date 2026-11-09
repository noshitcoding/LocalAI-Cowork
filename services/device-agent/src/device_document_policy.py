"""Bounded document inspection before any native Office opening. No COM or network."""
import hashlib,io,json,pathlib,re,struct,zipfile,xml.etree.ElementTree as ET,urllib.parse
MAX=32*1024*1024
ACTIVE=('vbaproject','/activex/','/embeddings/','macrosheet','dialogsheet','externallink','connections.xml','querytable','customui')
def inspect(raw,extension):
    if not isinstance(raw,bytes) or not 0<len(raw)<=MAX:raise ValueError('document_size')
    extension=extension.lower()
    if extension in {'docx','xlsx','pptx','dotx','potx'}:
        with zipfile.ZipFile(io.BytesIO(raw)) as book:
            members=book.infolist()
            if len(members)>10000 or sum(x.file_size for x in members)>64*1024*1024:raise ValueError('document_expansion')
            seen=set()
            for item in members:
                name=item.filename.lower()
                if name in seen or '\\' in name or ':' in name or name.startswith('/') or '..' in pathlib.PurePosixPath(name).parts or any(v in name for v in ACTIVE):raise ValueError('document_active_content')
                seen.add(name)
                if item.file_size>16*1024*1024:raise ValueError('document_part_size')
                if not name.endswith(('.xml','.rels')):continue
                body=book.read(item)
                if b'<!DOCTYPE' in body.upper() or b'<!ENTITY' in body.upper():raise ValueError('document_xml_entity')
                tree=ET.fromstring(body)
                joined_fields=''.join(''.join(n.itertext()) for n in tree.iter() if str(n.tag).split('}')[-1]=='instrText').upper()
                if re.search(r'\b(DDE|DDEAUTO|INCLUDETEXT|INCLUDEPICTURE|LINK|HYPERLINK)\b',joined_fields):raise ValueError('document_active_split_field')
                for node in tree.iter():
                    for attribute,value in node.attrib.items():
                        if attribute.split('}')[-1] in {'Type','ContentType'} and re.search(r'vbaproject|vbadata|macroenabled|macrosheet|dialogsheet|activex|oleobject|embeddedpackage|/package$',urllib.parse.unquote(value).lower()):raise ValueError('document_active_part_type')
                    if str(node.tag).split('}')[-1]=='Relationship':
                        target=urllib.parse.unquote(node.attrib.get('Target',''))
                        if ':' in target or '\\' in target:raise ValueError('document_external_target')
                    if node.attrib.get('TargetMode','').lower()=='external':raise ValueError('document_external_relationship')
                    if any(v in str(node.tag).lower() for v in ['oleobject','dde','externalreference']):raise ValueError('document_active_xml')
                    if str(node.tag).split('}')[-1] in {'f','formula','instrText','fldSimple'}:
                        value=(''.join(node.itertext())+' '+node.attrib.get('{http://schemas.openxmlformats.org/wordprocessingml/2006/main}instr','')).upper()
                        if re.search(r'(HTTPS?|FTP|FILE):|\\\\|\[[^\]]+\]|\b(DDE|DDEAUTO|INCLUDETEXT|INCLUDEPICTURE|LINK|WEBSERVICE|HYPERLINK|RTD|CALL|REGISTER\.ID|INDIRECT|EVALUATE|EXEC|IMAGE|FILTERXML|STOCKHISTORY)\b',value):raise ValueError('document_active_formula')
    elif extension in {'doc','xls','ppt'}:
        # CFB stream enumeration is structural, not extension-based macro assurance.
        if raw[:8]!=bytes.fromhex('d0cf11e0a1b11ae1') or len(raw)<512:raise ValueError('document_legacy_format')
        major=struct.unpack_from('<H',raw,26)[0]
        if struct.unpack_from('<H',raw,28)[0]!=0xfffe or struct.unpack_from('<H',raw,32)[0]!=6:raise ValueError('document_cfb_header')
        size=1<<struct.unpack_from('<H',raw,30)[0]
        if (major,size) not in ((3,512),(4,4096)) or len(raw)%size:raise ValueError('document_cfb_sector')
        sectors=len(raw)//size-1
        def sector(n):
            if not 0<=n<sectors:raise ValueError('document_cfb_chain')
            return raw[(n+1)*size:(n+2)*size]
        count=struct.unpack_from('<I',raw,44)[0]
        if count>sectors:raise ValueError('document_cfb_fat')
        ids=list(struct.unpack_from('<109I',raw,76));next_difat=struct.unpack_from('<I',raw,68)[0];difat_count=struct.unpack_from('<I',raw,72)[0]
        visited=set()
        for _ in range(difat_count):
            if next_difat in visited:raise ValueError('document_cfb_cycle')
            visited.add(next_difat);d=struct.unpack('<'+'I'*(size//4),sector(next_difat));ids.extend(d[:-1]);next_difat=d[-1]
        ids=[n for n in ids if n<0xfffffffa]
        if len(ids)!=count:raise ValueError('document_cfb_fat')
        fat=[]
        for n in ids:fat.extend(struct.unpack('<'+'I'*(size//4),sector(n)))
        current=struct.unpack_from('<I',raw,48)[0];visited=set();entries=[]
        while current!=0xfffffffe:
            if current in visited or current>=len(fat):raise ValueError('document_cfb_cycle')
            visited.add(current);block=sector(current)
            for pos in range(0,size,128):
                entry=block[pos:pos+128];length=struct.unpack_from('<H',entry,64)[0]
                if entry[66] not in (0,1,2,5) or length>64 or length%2:raise ValueError('document_cfb_directory')
                name=entry[:max(0,length-2)].decode('utf-16-le').lower()
                if any(x in name for x in ('vba','macro','objectpool','\x01ole','activex','external','link','encrypt')):raise ValueError('document_legacy_active')
                if entry[66] in (2,5):entries.append((name,entry[66],struct.unpack_from('<I',entry,116)[0],struct.unpack_from('<Q',entry,120)[0]))
            current=fat[current]
        def chain(start,length,links,read):
            if length>MAX:raise ValueError('document_stream_bound')
            chunks=[];seen=set();current=start;actual=0
            while current!=0xfffffffe:
                if current in seen or current>=len(links):raise ValueError('document_stream_cycle')
                seen.add(current);chunk=read(current);chunks.append(chunk);actual+=len(chunk)
                if actual>length+size:raise ValueError('document_stream_length')
                current=links[current]
            if actual<length:raise ValueError('document_stream_incomplete')
            return b''.join(chunks)[:length]
        roots=[e for e in entries if e[1]==5]
        if len(roots)!=1:raise ValueError('document_cfb_root')
        mini_data=chain(roots[0][2],roots[0][3],fat,sector) if roots[0][3] else b''
        mini_start=struct.unpack_from('<I',raw,60)[0];mini_count=struct.unpack_from('<I',raw,64)[0]
        mini_raw=chain(mini_start,mini_count*size,fat,sector) if mini_count else b''
        mini_fat=list(struct.unpack('<'+'I'*(len(mini_raw)//4),mini_raw)) if mini_raw else []
        def mini(n):
            if n*64+64>len(mini_data):raise ValueError('document_mini_stream_range')
            return mini_data[n*64:n*64+64]
        workbook=[]
        for name,kind,start,length in entries:
            if kind!=2 or not length:continue
            stream=chain(start,length,mini_fat,mini) if length<4096 else chain(start,length,fat,sector)
            for text in [stream.decode('latin-1').lower(),stream.decode('utf-16-le',errors='ignore').lower()]:
                if re.search(r'https?:|file:|ftp:|[a-z]:\\|\\\\|\b(ddeauto|dde|hyperlink|includetext|includepicture|webservice|register\.id)\b',text):raise ValueError('document_legacy_external')
            if extension=='xls' and name in ('workbook','book'):workbook.append(stream)
        # Legacy BIFF/field/link records are not activated by a mere macro setting.
        for encoding in ('ascii','utf-16-le'):
            lowered=raw.lower() if encoding=='ascii' else raw
            for marker in ('DDEAUTO','WEBSERVICE','REGISTER.ID','https:','http:','file:','\\\\'):
                needle=marker.lower().encode(encoding) if encoding=='ascii' else marker.encode(encoding)
                if needle in lowered:raise ValueError('document_legacy_external')
        if extension=='xls':
            if len(workbook)!=1:raise ValueError('document_legacy_workbook_missing')
            stream=workbook[0];position=0
            while position<len(stream):
                if position+4>len(stream):raise ValueError('document_biff_header')
                code,length=struct.unpack_from('<HH',stream,position);position+=4
                if code in (0x0018,0x0006):raise ValueError('document_legacy_formula_requires_safe_reencoding')
                if position+length>len(stream):raise ValueError('document_biff_length')
                body=stream[position:position+length];position+=length
                if code==0x85 and len(body)>=6 and body[5] in (1,6):raise ValueError('document_legacy_xlm')
                if code in (0x809,0x409,0x209,9) and len(body)>=4 and struct.unpack_from('<H',body,2)[0]==0x40:raise ValueError('document_legacy_xlm')
                if code==0x1ae and (len(body)<4 or struct.unpack_from('<H',body,2)[0]!=0x401):raise ValueError('document_legacy_external_book')
    elif extension=='pdf':
        if not raw.startswith(b'%PDF-') or any(v in raw for v in [b'/JavaScript',b'/JS ',b'/Launch',b'/EmbeddedFile',b'/RichMedia']):raise ValueError('document_active_pdf')
    else:raise ValueError('document_extension')
    return {'format':'device-document-policy/1','sha256':hashlib.sha256(raw).hexdigest(),'bytes':len(raw),'extension':extension,'inspection':'bounded_static_policy_only','unsafe_patterns_rejected':True,'runtime_qualified':False}
