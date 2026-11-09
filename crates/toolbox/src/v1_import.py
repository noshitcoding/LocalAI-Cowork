"""Selected V1 files -> inert user content. SQLite opens ONLY a private copy.

Windows uses the frozen native deny-write/delete inventory. Linux requires an
explicit cold-file declaration and rejects WAL/SHM/journal sidecars; its raw-copy
hash check is not a Windows writer fence. Neither path activates legacy runs.
"""
import base64, contextlib, hashlib, json, mimetypes, os, pathlib, sqlite3, stat, sys, time, uuid
from datetime import datetime, timezone
sys.path.insert(0, str(pathlib.Path(__file__).parent))
import v1_sqlite_export as legacy

MAX_JSON=32*1024*1024
MAX_SQLITE=512*1024*1024
MAX_CONTENT=16*1024*1024
MAX_BINARY=32*1024*1024
FILE_TYPES={'.pdf','.docx','.xlsx','.pptx','.txt','.md','.csv','.tsv','.json','.html','.svg','.png','.jpg','.jpeg','.gif','.webp','.mp3','.wav','.mp4'}

def sha(raw):return hashlib.sha256(raw).hexdigest()
def obj_pairs(pairs):
    value={}
    for key,item in pairs:
        if key in value:raise ValueError('Duplicate JSON key')
        value[key]=item
    return value
def parse(raw):
    return json.loads(raw,object_pairs_hook=obj_pairs,parse_constant=lambda _:(_ for _ in ()).throw(ValueError('Nonfinite JSON')))
def text(value,maximum,empty=False):
    assert isinstance(value,str) and (empty or value) and '\0' not in value
    assert len(value.encode('utf-8'))<=maximum,'Content field exceeds import limit'
    return value
def unchanged(st):return (st.st_dev,st.st_ino,st.st_size,st.st_mtime_ns)
def timestamp(value):
    if value is None:return None
    if type(value) is int:
        assert 0<=value<=9223372036854775807,'Invalid historical timestamp'
        return value
    assert isinstance(value,str) and len(value)<=64,'Invalid historical timestamp'
    parsed=datetime.fromisoformat(value.replace('Z','+00:00'))
    # SQLite's own datetime() values use UTC, whereas explicit offsets are retained.
    if parsed.tzinfo is None:parsed=parsed.replace(tzinfo=timezone.utc)
    result=int(parsed.timestamp()*1000);assert result>=0
    return result
def message(value):
    role=text(value['role'],64);assert role in {'system','user','assistant','tool'},'Unsupported historical message role'
    return {'source_id':text(value.get('source_id',value.get('id')),200),'role':role,
        'content':text(value['content'],128*1024,True),'created_at':timestamp(value.get('created_at',value.get('timestamp')))}
def artifact(value):
    # This is embedded text, never a path-driven request to read original files.
    result={'source_id':text(value.get('source_id',value.get('id')),200),
        'source_kind':text(value.get('source_kind','json_artifact'),64),
        'name':text(value['name'],300),'media_type':'text/plain',
        'content':text(value['content'],1024*1024,True),'created_at':timestamp(value.get('created_at'))}
    if value.get('source_path'):result['_source_path']=text(value['source_path'],8192)
    if value.get('size_bytes') is not None:
        assert type(value['size_bytes']) is int and value['size_bytes']>=0
        result['_expected_size']=value['size_bytes']
    return result
def unix_copy(source,destination,maximum):
    # Refuse namespace aliases and non-files, then read through a no-follow FD.
    for parent in [source,*source.parents]:
        metadata=parent.lstat()
        assert not stat.S_ISLNK(metadata.st_mode),'Source namespace contains a symlink'
    before=source.lstat()
    assert stat.S_ISREG(before.st_mode) and before.st_nlink==1 and before.st_size<=maximum
    stack=contextlib.ExitStack();opened=[]
    try:
        parent_fd=os.open(source.anchor,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW);stack.callback(os.close,parent_fd)
        current=pathlib.Path(source.anchor)
        for part in source.parts[1:-1]:
            current=current/part;expected=current.lstat()
            next_fd=os.open(part,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW,dir_fd=parent_fd);stack.callback(os.close,next_fd)
            held=os.fstat(next_fd);assert (held.st_dev,held.st_ino)==(expected.st_dev,expected.st_ino),'Source directory identity changed'
            opened.append((current,held.st_dev,held.st_ino));parent_fd=next_fd
        fd=os.open(source.name,os.O_RDONLY|os.O_NOFOLLOW,dir_fd=parent_fd)
    except BaseException:stack.close();raise
    with stack,os.fdopen(fd,'rb') as stream:
        assert unchanged(os.fstat(stream.fileno()))==unchanged(before)
        hasher=hashlib.sha256();count=0
        with destination.open('xb') as output:
            while chunk:=stream.read(1024*1024):
                count+=len(chunk);assert count<=maximum
                hasher.update(chunk);output.write(chunk)
            output.flush();os.fsync(output.fileno())
        assert unchanged(os.fstat(stream.fileno()))==unchanged(before)==unchanged(source.lstat())
        stream.seek(0);again=hashlib.sha256()
        while chunk:=stream.read(1024*1024):again.update(chunk)
        assert again.digest()==hasher.digest(),'Selected source changed during raw capture'
        for directory,device,inode in opened:
            observed=directory.lstat();assert not stat.S_ISLNK(observed.st_mode) and (observed.st_dev,observed.st_ino)==(device,inode),'Source directory namespace changed'
    return hasher.hexdigest()

def normalize_json(value):
    assert isinstance(value,dict) and value.get('format') in {
        'localai-cowork-v1-content/1','localai-cowork-v1-user-content/1'},'Unknown JSON content export format'
    projects=[]
    for project in value['projects']:
        orders=[]
        for order in project.get('orders',[]):
            messages=[message(item) for item in order.get('messages',[])]
            artifacts=[artifact(item) for item in order.get('artifacts',[])]
            orders.append({'source_id':text(order.get('source_id',order.get('id')),256),
                'source_kind':text(order.get('source_kind','order'),64),
                'title':text(order['title'],300),'draft':text(order.get('draft',order.get('prompt','')),128*1024,True),
                'messages':messages,'artifacts':artifacts})
        projects.append({'source_id':text(project.get('source_id',project.get('id')),256),
            'name':text(project.get('name',project.get('title')),200),'orders':orders})
    return projects

def normalize_sqlite(connection):
    tables={row[0] for row in connection.execute("SELECT name FROM sqlite_schema WHERE type='table'")}
    projects={}
    def rows(table,fields):
        if table not in tables:return []
        columns={r[1] for r in connection.execute('PRAGMA table_info('+table+')')};selected=[f for f in fields if f in columns]
        values=[dict(zip(selected,r)) for r in connection.execute('SELECT '+','.join(selected)+' FROM '+table+' ORDER BY id')]
        assert len(values)<=10000,'Historical row bound exceeded';return values
    if 'projects' in tables:
        for identifier,title,instructions in connection.execute('SELECT id,title,instructions FROM projects ORDER BY id'):
            projects[text(identifier,256)]={'source_id':identifier,'name':text(title,200),'orders':[]}
            if instructions:
                projects[identifier]['orders'].append({'source_id':identifier,'source_kind':'project_instructions',
                    'title':'Projektanweisungen','draft':text(instructions,128*1024),'messages':[]})
    unassigned={'source_id':'unassigned-v1-content','name':'Importierte V1-Inhalte','orders':[]}
    linked={}
    if 'project_threads' in tables:
        for project,thread in connection.execute('SELECT project_id,thread_id FROM project_threads ORDER BY thread_id'):
            assert project in projects and thread not in linked,'Unknown or multiple project mappings'
            linked[thread]=project
    def destination(thread):return projects[linked[thread]] if thread in linked else unassigned
    thread_ids=set();thread_orders={}
    threads=connection.execute('SELECT id,title FROM chat_threads ORDER BY id') if 'chat_threads' in tables else []
    for identifier,title in threads:
        identifier=text(identifier,256);assert identifier not in thread_ids
        thread_ids.add(identifier);messages=[];draft=[];chat_artifacts=[]
        for mid,role,content,stamp in connection.execute('SELECT id,role,content,timestamp FROM chat_messages WHERE thread_id=? ORDER BY timestamp,id',(identifier,)):
            if content.lstrip().startswith('{') and 'open-cowork-chat-message' in content:
                stored=parse(text(content,2*1024*1024).encode())
                if isinstance(stored,dict) and stored.get('kind')=='open-cowork-chat-message':
                    assert stored.get('version')==1 and isinstance(stored.get('message'),dict),'Unknown stored chat payload version'
                    payload=stored['message'];assert payload.get('id')==mid and payload.get('role')==role and payload.get('timestamp')==stamp,'Stored chat identity differs'
                    content=text(payload.get('content') or '',128*1024,True)
                    attachments=payload.get('attachments') or [];assert isinstance(attachments,list) and len(attachments)<=1000
                    for index,attachment in enumerate(attachments):
                        assert isinstance(attachment,dict)
                        path=attachment.get('path');item=artifact({'id':mid+':'+str(index),'source_kind':'chat_attachment','name':text(attachment.get('label') or 'Historischer Nachrichtenanhang',300),'content':'Anhang der ursprünglichen Nachricht '+text(mid,200),'created_at':stamp})
                        if isinstance(path,str) and path and not path.startswith(('http://','https://','data:')):item['_source_path']=text(path,8192)
                        else:item['_source_missing_reason']='original_attachment_file_not_available'
                        chat_artifacts.append(item)
            messages.append(message({'id':mid,'role':role,'content':content,'timestamp':stamp}));draft.append('['+role+'] '+content)
            assert len(messages)<=2000
        order={'source_id':identifier,'source_kind':'chat_thread','title':text(title,300),
            'draft':text('\n\n'.join(draft),128*1024,True),'messages':messages,'artifacts':chat_artifacts}
        destination(identifier)['orders'].append(order);thread_orders[identifier]=order
    task_orders={}
    for table in ('tasks','work_tasks'):
        if table not in tables:continue
        for identifier,title,prompt,thread in connection.execute('SELECT id,title,prompt,thread_id FROM '+table+' ORDER BY id'):
            assert thread is None or thread in thread_ids,'Unknown task thread reference'
            order={'source_id':text(identifier,256),'source_kind':table,
                'title':text(title or 'Importierter Auftrag',300),'draft':text(prompt,128*1024,True),'messages':[],'artifacts':[]}
            destination(thread)['orders'].append(order);task_orders[(table,identifier)]=order
    for row in rows('task_steps',['id','task_id','idx','title','state','output']):
        parent=task_orders.get(('tasks',row['task_id']));assert parent is not None,'Unknown task step parent'
        content='Historischer Schritt '+str(row['idx'])+': '+text(row['title'],300)+'; Zustand '+text(row['state'],120)
        if row.get('output'):content+='\n\n'+text(row['output'],120*1024,True)
        parent['messages'].append(message({'id':'task-step:'+row['id'],'role':'assistant','content':content}))
    for row in rows('work_tasks',['id','output','last_run_at']):
        if row.get('output'):task_orders[('work_tasks',row['id'])]['messages'].append(message({'id':'work-output:'+row['id'],'role':'assistant','content':row['output'],'created_at':row.get('last_run_at')}))
    scheduled={}
    for row in rows('scheduled_tasks',['id','name','prompt']):
        order={'source_id':row['id'],'source_kind':'scheduled_task','title':text(row['name'],300),'draft':text(row['prompt'],128*1024,True),'messages':[],'artifacts':[]}
        unassigned['orders'].append(order);scheduled[row['id']]=order
    for row in rows('scheduled_runs',['id','task_id','status','started_at','result']):
        parent=scheduled.get(row['task_id']);assert parent is not None,'Unknown scheduled run parent'
        parent['messages'].append(message({'id':'scheduled-state:'+row['id'],'role':'system','content':'Historischer Laufzustand: '+text(row['status'],120),'created_at':row['started_at']}))
        if row.get('result'):parent['messages'].append(message({'id':'scheduled-result:'+row['id'],'role':'assistant','content':row['result'],'created_at':row['started_at']}))
    crews={}
    for row in rows('crew_runs',['id','crew_name','status','started_at']):
        order={'source_id':row['id'],'source_kind':'crew_run','title':text(row['crew_name'] or 'Historischer Workflow',300),'draft':'','messages':[message({'id':'crew-state:'+row['id'],'role':'system','content':'Historischer Workflowzustand: '+text(row['status'],120),'created_at':row['started_at']})],'artifacts':[]}
        unassigned['orders'].append(order);crews[row['id']]=order
    for row in rows('crew_run_logs',['id','run_id','agent_id','task_id','action','result','timestamp']):
        parent=crews.get(row['run_id']);assert parent is not None,'Unknown workflow log parent'
        content='Agent '+text(row['agent_id'],256)+'; Aufgabe '+text(row['task_id'],256)+'; Aktion '+text(row['action'],256)+'\n\n'+text(row['result'],120*1024,True)
        parent['messages'].append(message({'id':'crew-log:'+row['id'],'role':'tool','content':content,'timestamp':row['timestamp']}))
    runs={}
    if 'engine_runs' in tables:
        for run,thread in connection.execute('SELECT id,thread_id FROM engine_runs ORDER BY id'):
            assert run not in runs and (thread is None or thread in thread_ids),'Unknown run-to-content relationship'
            runs[run]=thread
    orphaned=None
    def artifact_order(run):
        nonlocal orphaned
        thread=runs.get(run)
        if thread is not None:return thread_orders[thread]
        if orphaned is None:
            orphaned={'source_id':'embedded-artifact-history','source_kind':'artifact_history',
                'title':'Importierte Artefakttexte','draft':'Historische eingebettete Zusammenfassungen; Originaldateien wurden nicht geöffnet.',
                'messages':[],'artifacts':[]}
            unassigned['orders'].append(orphaned)
        return orphaned
    version_orders={}
    if 'artifact_versions' in tables:
        for aid,run,label,source_path,kind,summary,preview,stamp,size in connection.execute('SELECT id,run_id,label,source_path,format,summary,preview,created_at,size_bytes FROM artifact_versions ORDER BY id'):
            value='Historischer Artefakttext\nTyp: '+text(kind,120)+'\nUrsprüngliche Run-ID (nur Text): '+text(run or '',256,True)+'\n\nZusammenfassung:\n'+text(summary,512*1024,True)+'\n\nVorschau:\n'+text(preview,512*1024,True)
            parent=artifact_order(run);version_orders[aid]=parent
            parent['artifacts'].append(artifact({'id':aid,'source_kind':'artifact_versions',
                'name':text((label or 'Artefakt')+' (historischer Text)',300),'content':value,'created_at':stamp,'source_path':source_path,'size_bytes':size}))
    if 'engine_run_artifacts' in tables:
        for aid,run,kind,source_path,title,summary,stamp in connection.execute('SELECT id,run_id,kind,path,title,summary,created_at FROM engine_run_artifacts ORDER BY id'):
            assert run in runs,'Unknown artifact run relationship'
            value='Historischer Artefakttext\nTyp: '+text(kind,120)+'\n\nZusammenfassung:\n'+text(summary or '',1024*1024,True)
            artifact_order(run)['artifacts'].append(artifact({'id':aid,'source_kind':'engine_run_artifacts',
                'name':text((title or 'Artefakt')+' (historischer Text)',300),'content':value,'created_at':stamp,'source_path':source_path}))
    for row in rows('artifact_exports',['id','artifact_version_id','export_format','target_path','size_bytes','created_at']):
        parent=version_orders.get(row['artifact_version_id']);assert parent is not None,'Unknown exported artifact version'
        parent['artifacts'].append(artifact({'id':row['id'],'source_kind':'artifact_export','name':'Historischer Export '+text(row['export_format'],100),'content':'Ursprüngliche Artefaktversion: '+text(row['artifact_version_id'],256),'source_path':row['target_path'],'size_bytes':row['size_bytes'],'created_at':row['created_at']}))
    resource_orders={}
    for row in rows('project_resources',['id','project_id','kind','path','label','added_at']):
        project=projects.get(row['project_id']);assert project is not None,'Unknown project resource parent'
        if row['project_id'] not in resource_orders:
            order={'source_id':row['project_id'],'source_kind':'project_resources','title':'Historische Projektressourcen','draft':'','messages':[],'artifacts':[]}
            project['orders'].append(order);resource_orders[row['project_id']]=order
        value={'id':row['id'],'source_kind':'project_resource','name':text(row.get('label') or 'Projektressource',300),'content':'Historischer Ressourcentyp: '+text(row['kind'],30)+'; ursprüngliche Freigaben werden nicht übernommen.','created_at':row['added_at']}
        if row['kind']=='file':value['source_path']=row['path']
        resource_orders[row['project_id']]['artifacts'].append(artifact(value))
    result=list(projects.values())
    if unassigned['orders']:result.append(unassigned)
    return result

def normalize_daemon(connection):
    projects={};entities=[];seen=set();tables={r[0] for r in connection.execute("SELECT name FROM sqlite_schema WHERE type='table'")}
    def entity(kind,identifier,name,project=None,actor=None):
        identifier=text(identifier,256);key=(kind,identifier)
        if key in seen:return
        seen.add(key);entities.append({'kind':kind,'source_id':identifier,'name':text(name,300),
            'source_project_id':project,'source_actor_id':actor})
    original_entities=set()
    for kind,identifier,encoded in connection.execute('SELECT entity_type,id,payload_json FROM daemon_entities WHERE tombstone=0 ORDER BY entity_type,id'):
        selected={'organization':'tenant','tenant':'tenant','user':'actor','actor':'actor','agent':'persona','persona':'persona','project':'project'}.get(kind)
        if selected is None:continue
        payload=parse(text(encoded,1024*1024).encode());assert isinstance(payload,dict)
        assert payload.get('id',identifier)==identifier,'Historical entity ID differs'
        assert (selected,identifier) not in original_entities,'Multiple original entity types map to one historical identity'
        original_entities.add((selected,identifier))
        if selected=='project' and identifier not in projects:
            projects[identifier]={'source_id':text(identifier,256),'name':text(payload.get('name') or payload.get('title') or 'Historisches Daemonprojekt',200),'orders':[]}
        entity(selected,identifier,payload.get('name') or payload.get('title') or selected,
            payload.get('project_id') if isinstance(payload.get('project_id'),str) else None)
    # FrozenReference contains only id/revision. Display names belong to the
    # historical entity rows, and workspace paths remain private capture hints.
    project_workspaces={identifier:text(path,8192) for identifier,path in connection.execute('SELECT project_id,workspace_path FROM daemon_project_bindings ORDER BY project_id')} if 'daemon_project_bindings' in tables else {}
    run_workspaces={identifier:text(path,8192) for identifier,path in connection.execute('SELECT run_id,workspace_path FROM daemon_run_workspaces ORDER BY run_id')} if 'daemon_run_workspaces' in tables else {}
    for identifier,thread,state,encoded,created in connection.execute('SELECT id,thread_id,state,record_json,created_at FROM daemon_runs ORDER BY id'):
        record=parse(text(encoded,2*1024*1024).encode());assert isinstance(record,dict)
        spec=record.get('spec');assert isinstance(spec,dict),'Missing historical daemon spec'
        assert spec.get('id')==identifier and str(uuid.UUID(identifier))==identifier,'Conflicting original daemon run ID'
        assert spec.get('thread_id')==thread and str(uuid.UUID(thread))==thread,'Conflicting original daemon thread ID'
        assert record.get('state')==state,'Conflicting original daemon state'
        project=str(uuid.UUID(spec['project_id']));actor=str(uuid.UUID(spec['creator_user_id']))
        assert project==spec['project_id'] and actor==spec['creator_user_id'],'Noncanonical historical scope'
        original_project=spec.get('project');assert isinstance(original_project,dict) and original_project.get('id')==project,'Conflicting original project'
        if project not in projects:projects[project]={'source_id':project,'name':'Historisches Daemonprojekt','orders':[]}
        entity('project',project,projects[project]['name']);entity('actor',actor,'Historischer Akteur');entity('daemon_run',identifier,'Historischer Run',project,actor)
        original_input=spec.get('input');original_input=original_input if isinstance(original_input,dict) else {}
        prompt=text(original_input.get('prompt',''),128*1024,True)
        messages=[{'source_id':'daemon-state:'+identifier,'role':'system','content':'Historischer Zustand: '+text(state,64),'created_at':timestamp(created)}]
        if prompt:messages.append({'source_id':'daemon-prompt:'+identifier,'role':'user','content':prompt,'created_at':timestamp(created)})
        inputs=original_input.get('messages',[]);assert isinstance(inputs,list) and len(inputs)<=10000,'Invalid historical input messages'
        for index,item in enumerate(inputs):
            assert isinstance(item,dict),'Invalid historical input message'
            if item.get('role') in {'system','user','assistant','tool'} and isinstance(item.get('content'),str):
                messages.append({'source_id':'daemon-input:'+identifier+':'+str(index),'role':item['role'],
                    'content':text(item['content'],128*1024,True),'created_at':timestamp(created)})
        artifacts=[];completed_contents=set()
        for sequence,event in connection.execute('SELECT sequence,event_json FROM daemon_run_events WHERE run_id=? ORDER BY sequence',(identifier,)):
            event=parse(text(event,2*1024*1024).encode());assert isinstance(event,dict)
            assert type(sequence) is int and sequence>=1 and type(event.get('sequence')) is int and event['sequence']==sequence,'Conflicting original daemon event sequence'
            assert event.get('run_id')==identifier,'Conflicting original daemon event run ID'
            event_id=event.get('event_id');assert isinstance(event_id,str) and str(uuid.UUID(event_id))==event_id,'Invalid historical event ID'
            # Select only the original renderer's known text fields. Tool calls,
            # response/usage metadata, approvals and executor state stay opaque.
            payload=event.get('payload');kind=event.get('kind')
            if not isinstance(payload,dict):continue
            if kind=='model_completed' and isinstance(payload.get('content'),str):
                content=text(payload['content'],128*1024,True);completed_contents.add(content)
                messages.append({'source_id':'daemon-event:'+identifier+':'+str(sequence),'role':'assistant',
                    'content':content,'created_at':timestamp(event.get('created_at'))})
            elif kind=='artifact_created' and payload.get('storage')=='project_workspace' and isinstance(payload.get('path'),str):
                original_path=text(payload['path'],8192);relative=pathlib.PurePosixPath(original_path.replace('\\','/'))
                assert not relative.is_absolute() and not pathlib.PureWindowsPath(original_path).drive and relative.parts and '..' not in relative.parts,'Invalid historical workspace artifact path'
                item=artifact({'id':'daemon-event:'+identifier+':'+str(sequence),'source_kind':'daemon_artifact_event',
                    'name':text(relative.name+' (historischer Text)',300),'content':'Historischer Artefakthinweis; Originaldatei nur mit ausdrücklich ausgewähltem Quellordner.',
                    'created_at':event.get('created_at')})
                workspace=run_workspaces.get(identifier,project_workspaces.get(project))
                if workspace and pathlib.Path(workspace).is_absolute():item['_source_path']=str(pathlib.Path(workspace).joinpath(*relative.parts))
                else:item['_source_missing_reason']='historical_workspace_not_available'
                artifacts.append(item)
        result=record.get('result')
        if isinstance(result,dict) and isinstance(result.get('content'),str) and result['content'] not in completed_contents:
            messages.append({'source_id':'daemon-result:'+identifier,'role':'assistant','content':text(result['content'],128*1024,True),
                'created_at':timestamp(record.get('finished_at') or record.get('updated_at'))})
        projects[project]['orders'].append({'source_id':text(identifier,256),'source_kind':'daemon_run',
            'title':'Importierter Daemonauftrag','draft':prompt,'messages':messages,'artifacts':artifacts})
    # Desktop's durable mirrors are independent content, including threads that
    # have never executed a run. Only their known renderer fields are imported.
    mirrored={};threads={};unassigned={'source_id':'unassigned-v1-content','name':'Importierte Daemoninhalte','orders':[]}
    for kind,identifier,encoded in connection.execute("SELECT entity_type,id,payload_json FROM daemon_entities WHERE tombstone=0 AND entity_type IN('thread','message','task') ORDER BY entity_type,id"):
        payload=parse(text(encoded,1024*1024).encode());assert isinstance(payload,dict)
        assert payload.get('id',identifier)==identifier,'Conflicting mirrored entity ID'
        mirrored[(kind,identifier)]=payload
    for (kind,identifier),payload in mirrored.items():
        if kind!='thread':continue
        project=payload.get('project_id');destination=projects.get(project,unassigned)
        if project is not None:assert project in projects,'Unknown mirrored thread project'
        order={'source_id':identifier,'source_kind':'chat_thread','title':text(payload.get('title') or 'Historischer Chat',300),'draft':'','messages':[],'artifacts':[]}
        destination['orders'].append(order);threads[identifier]=order
    for (kind,identifier),payload in mirrored.items():
        if kind=='message':
            thread=payload.get('thread_id');assert thread in threads,'Unknown mirrored message thread'
            threads[thread]['messages'].append(message({'id':'daemon-message:'+identifier,'role':payload['role'],'content':payload['content'],'timestamp':payload.get('timestamp')}))
        elif kind=='task':
            thread=payload.get('thread_id');assert thread is None or thread in threads,'Unknown mirrored task thread'
            project=payload.get('project_id');destination=projects.get(project,unassigned)
            if project is not None:assert project in projects,'Unknown mirrored task project'
            destination['orders'].append({'source_id':identifier,'source_kind':'daemon_task','title':text(payload.get('title') or 'Historische Aufgabe',300),'draft':text(payload.get('description') or payload.get('prompt') or '',128*1024,True),'messages':[message({'id':'daemon-task-state:'+identifier,'role':'system','content':'Historischer Zustand: '+text(payload.get('status') or 'unknown',120)})],'artifacts':[]})
    version_order=None
    for cursor,identifier,revision,encoded in connection.execute("SELECT cursor,entity_id,revision,entity_json FROM daemon_sync_changes WHERE entity_type='message' AND operation='upsert' ORDER BY cursor"):
        value=parse(text(encoded,1024*1024).encode());assert isinstance(value,dict) and value.get('entity_type')=='message' and value.get('id')==identifier and value.get('revision')==revision,'Conflicting historical message version'
        payload=value.get('payload');assert isinstance(payload,dict)
        parent=threads.get(payload.get('thread_id'))
        if parent is None:
            if version_order is None:
                version_order={'source_id':'daemon-message-versions','source_kind':'artifact_history','title':'Historische Nachrichtenfassungen','draft':'Archivierte Fassungen aus dem ausgewählten Daemonsnapshot.','messages':[],'artifacts':[]};unassigned['orders'].append(version_order)
            parent=version_order
        parent['messages'].append(message({'id':'daemon-message-version:'+str(cursor),'role':payload['role'],'content':payload['content'],'timestamp':payload.get('timestamp')}))
    if unassigned['orders']:projects[unassigned['source_id']]=unassigned
    return list(projects.values()),entities

def capture_attachments(projects,selected_root,directory):
    pending=[];missing=[];root=pathlib.Path(selected_root) if selected_root else None
    root_before=None
    if root:
        assert root.is_absolute() and root.is_dir(),'Explicit existing source root required'
        if os.name=='nt':root=legacy.bounded_path(root)
        for part in [root,*root.parents]:
            info=part.lstat();assert not stat.S_ISLNK(info.st_mode) and not getattr(info,'st_file_attributes',0)&0x400,'Source root namespace is aliased'
        root_before=root.lstat()
    for project in projects:
        for order in project['orders']:
            order['attachments']=[]
            for item in order.get('artifacts',[]):
                original=item.pop('_source_path',None)
                expected_size=item.pop('_expected_size',None);expected_sha=item.pop('_expected_sha',None)
                missing_reason=item.pop('_source_missing_reason',None)
                if missing_reason:
                    missing.append({'source_id':item['source_kind']+':'+item['source_id'],'reason':missing_reason});continue
                if not original:continue
                identifier=item['source_kind']+':'+item['source_id']
                if not root:missing.append({'source_id':identifier,'reason':'source_root_not_selected'});continue
                candidate=pathlib.Path(original)
                if not candidate.is_absolute():candidate=root/candidate
                try:relative=candidate.relative_to(root)
                except ValueError:missing.append({'source_id':identifier,'reason':'outside_selected_root'});continue
                if '..' in relative.parts or not relative.parts or candidate.suffix.lower() not in FILE_TYPES:
                    missing.append({'source_id':identifier,'reason':'unsupported_artifact_path'});continue
                if any(word in candidate.name.lower() for word in ('credential','secret','token','password')):
                    missing.append({'source_id':identifier,'reason':'credential_path_refused'});continue
                try:info=candidate.lstat()
                except FileNotFoundError:missing.append({'source_id':identifier,'reason':'original_file_missing'});continue
                if expected_size is not None and info.st_size!=expected_size:
                    missing.append({'source_id':identifier,'reason':'original_size_changed'});continue
                assert stat.S_ISREG(info.st_mode) and info.st_nlink==1 and info.st_size<=16*1024*1024,'Invalid or oversized original artifact'
                pending.append((candidate,relative.as_posix(),identifier,order,expected_sha))
    assert len(pending)<=1000 and sum(path.stat().st_size for path,_,_,_,_ in pending)<=MAX_BINARY,'Original attachment budget exceeded'
    def append(path,relative,identifier,order,raw,identity):
        assert sum(item['size'] for p in projects for o in p['orders'] for item in o['attachments'])+len(raw)<=MAX_BINARY,'Actual attachment bytes exceed limit'
        order['attachments'].append({'source_id':identifier,'name':path.name,'media_type':mimetypes.guess_type(path.name)[0] or 'application/octet-stream',
            'size':len(raw),'sha256':sha(raw),'relative_path':relative,'capture_identity':identity,'content_base64':base64.b64encode(raw).decode()})
    if pending and os.name=='nt':
        with legacy.FrozenFiles(list(dict.fromkeys(path for path,_,_,_,_ in pending))) as fence:
            receipts={path:fence.receipt(path) for path,_,_,_,_ in pending}
            for path,relative,identifier,order,expected_sha in pending:
                raw=fence.read(path,16*1024*1024);receipt=receipts[path];assert sha(raw)==receipt['sha256']
                assert expected_sha is None or sha(raw)==expected_sha,'Selected plaintext export hash differs'
                append(path,relative,identifier,order,raw,'win32:'+str(receipt['volume'])+':'+receipt['file_id'])
            assert all(fence.receipt(path)==receipt for path,receipt in receipts.items()),'Artifact changed under native fence'
    elif pending:
        for index,(path,relative,identifier,order,expected_sha) in enumerate(pending):
            destination=directory/('attachment-'+str(index)+'.bin');digest=unix_copy(path,destination,16*1024*1024)
            raw=destination.read_bytes();assert sha(raw)==digest;info=path.lstat()
            assert expected_sha is None or digest==expected_sha,'Selected plaintext export hash differs'
            append(path,relative,identifier,order,raw,'linux:'+str(info.st_dev)+':'+str(info.st_ino))
    root_identity=None
    if root:
        observed=root.lstat();assert not stat.S_ISLNK(observed.st_mode) and (observed.st_dev,observed.st_ino)==(root_before.st_dev,root_before.st_ino),'Selected source root identity changed'
        root_identity=('win32-directory:' if os.name=='nt' else 'linux-directory:')+str(root_before.st_dev)+':'+str(root_before.st_ino)
    return missing,root_identity

def validate(projects):
    assert 0<len(projects)<=100
    pids=set();oids=set();mids=set();aids=set();content=0;orders=0;messages=0;artifacts=0
    for project in projects:
        assert project['source_id'] not in pids,'Duplicate original project ID'
        pids.add(project['source_id'])
        for order in project['orders']:
            assert order['source_kind'] in {'order','chat_thread','tasks','work_tasks','project_instructions','artifact_history','daemon_run','daemon_task','task_definition','scheduled_task','crew_run','project_resources','project_version'},'Unknown content source kind'
            identity=(order['source_kind'],order['source_id'])
            assert identity not in oids,'Conflicting original order mapping'
            oids.add(identity);orders+=1;content+=len(order['draft'].encode())
            for message in order['messages']:
                assert message['source_id'] not in mids,'Duplicate original message ID'
                mids.add(message['source_id']);messages+=1;content+=len(message['content'].encode())
            for item in order.get('artifacts',[]):
                assert item['source_kind'] in {'json_artifact','artifact_versions','engine_run_artifacts','daemon_artifact_event','artifact_export','project_resource','server_artifact','server_version_file','chat_attachment'},'Unknown artifact source kind'
                identity=(item['source_kind'],item['source_id']);assert identity not in aids,'Conflicting original artifact mapping'
                aids.add(identity);artifacts+=1;content+=len(item['content'].encode())
    assert orders<=2000 and messages<=10000 and artifacts<=1000 and content<=MAX_CONTENT,'Import content budget exceeded'

def empty_catalog():
    return {'version':1,'users':[],'organizations':[],'memberships':[],
        'project_acl':[],'objects':[],'operational_states':[],'limitations':[]}

def catalog_object(kind,row,project=None):
    identifier=text(str(row.get('id',row.get('source_id'))),256)
    instructions=row.get('instructions',row.get('prompt_template',row.get('system_prompt',row.get('content',row.get('prompt',''))))) or ''
    if kind=='persona':instructions='\n\n'.join(str(row.get(k) or '') for k in ('role','goal','system_prompt','skills_markdown'))
    return {'kind':kind,'source_id':identifier,'source_project_id':project,
        'name':text(str(row.get('name') or row.get('title') or row.get('key') or kind),200),
        'description':text(str(row.get('description') or ''),4000,True),
        'instructions':text(str(instructions),256000,True),
        'source_revision':int(row.get('revision',row.get('version',1)) or 1),
        'versions':[],'workflow':None,'schedule_expression':None,'timezone':None,
        'base_url':None,'model':None,'paused':True}

def workflow_projection(value):
    # Exact display/definition fields of V1 Crew. Provider/tool bindings, approvals,
    # runtime state and credentials are deliberately not part of the new plan.
    assert isinstance(value,dict)
    agents=[];steps=[]
    for index,a in enumerate(value.get('agents',[])):
        assert isinstance(a,dict)
        agents.append({'id':'agent_'+str(index),'instructions':text('\n\n'.join(str(a.get(k) or '') for k in ('name','role','goal','backstory','skillsMarkdown','instructions')),64000,True),'skills':[],'knowledge':[]})
    aliases={str(a.get('id')):'agent_'+str(i) for i,a in enumerate(value.get('agents',[]))}
    tasks=value.get('tasks',value.get('steps',[]));taskids={str(a.get('id')):'step_'+str(i) for i,a in enumerate(tasks)}
    for index,t in enumerate(tasks):
        agent=aliases.get(str(t.get('agentId',t.get('agent_id'))))
        assert agent is not None,'Unknown workflow agent reference'
        dependencies=t.get('dependencies',t.get('depends_on',[]))
        assert isinstance(dependencies,list) and all(str(d) in taskids for d in dependencies),'Unknown workflow dependency'
        steps.append({'id':'step_'+str(index),'agent_id':agent,'prompt':text(str(t.get('description',t.get('prompt','')))+'\n'+str(t.get('expectedOutput') or ''),64000,True),'depends_on':[taskids[str(d)] for d in dependencies]})
    assert 0<len(agents)<=16 and 0<len(steps)<=32,'Workflow exceeds V2 definition bounds'
    mode=value.get('process',value.get('mode','sequential'));assert mode in {'sequential','parallel','hierarchical'}
    manager=aliases.get(str(value.get('managerAgentId',value.get('manager_agent_id')))) if mode=='hierarchical' else None
    assert mode!='hierarchical' or manager is not None,'Unknown hierarchical manager'
    completed=set()
    while len(completed)<len(steps):
        available={s['id'] for s in steps if all(d in completed for d in s['depends_on'])}
        assert available-completed,'Cyclic workflow definition';completed|=available
    return {'mode':mode,'agents':agents,'steps':steps,'manager_agent_id':manager}

def pipeline_projection(value):
    assert isinstance(value,list) and 0<len(value)<=32,'Invalid historical RPC pipeline'
    steps=[]
    for index,row in enumerate(value):
        assert isinstance(row,dict)
        prompt=text(str(row.get('prompt') or ''),63000,True)
        if isinstance(row.get('tool'),str):prompt+='\nHistorisches Werkzeug: '+text(row['tool'],256)+'. Eine heutige Werkzeugfreigabe ist separat erforderlich.'
        steps.append({'id':'step_'+str(index),'agent_id':'agent_0','prompt':prompt or 'Historischer Schritt; heutige Anweisung ergänzen.','depends_on':[] if index==0 else ['step_'+str(index-1)]})
    return {'mode':'sequential','agents':[{'id':'agent_0','instructions':'Migrierte RPC-Schrittdefinition; heutige Werkzeuge und Verbindungen ausdrücklich neu zuordnen.','skills':[],'knowledge':[]}],'steps':steps,'manager_agent_id':None}

def sqlite_catalog(connection,family):
    catalog=empty_catalog();tables={r[0] for r in connection.execute("SELECT name FROM sqlite_schema WHERE type='table'")}
    def rows(table,fields):
        if table not in tables:return []
        columns={r[1] for r in connection.execute('PRAGMA table_info('+table+')')}
        selected=[f for f in fields if f in columns]
        if not selected:return []
        cursor=connection.execute('SELECT '+','.join(selected)+' FROM '+table)
        values=[dict(zip(selected,r)) for r in cursor];assert len(values)<=10000,'Catalog row bound exceeded'
        return values
    if family=='desktop':
        for table,kind,fields in (
            ('skills','skill',['id','name','description','prompt_template','version','trigger_pattern','run_mode','parent_skill_id']),
            ('agent_personalities','persona',['id','name','description','role','goal','system_prompt','skills_markdown']),
            ('memory_entries','memory',['id','key','content','scope','scope_ref','category']),
            ('user_profile','knowledge',['id','key','value']),
            ('learning_outcomes','knowledge',['id','description','learned_pattern']),
            ('sessions','knowledge',['id','title','summary']),
            ('runtime_instructions','knowledge',['id','title','content','scope_type','scope_ref']),
            ('api_profiles','provider',['id','name','base_url','model']),
            ('scheduled_tasks','schedule',['id','name','prompt','schedule_expr','active']),
            ('work_tasks','schedule',['id','title','prompt','schedule_expr','schedule_enabled']),
            ('crew_definitions','workflow',['id','name','description','definition_json'])):
            for row in rows(table,fields):
                if kind=='schedule' and not row.get('schedule_expr'):continue
                if table=='user_profile':row['content']=str(row.get('value') or '')
                if table=='sessions':row['content']=str(row.get('summary') or '')
                if table=='learning_outcomes':row['content']='\n'.join(str(row.get(k) or '') for k in ('description','learned_pattern'))
                if table=='runtime_instructions':row['content']=str(row.get('content') or row.get('instruction') or '')
                project=row.get('scope_ref') if row.get('scope',row.get('scope_type'))=='project' else None
                obj=catalog_object(kind,row,project)
                if kind=='knowledge':obj['source_id']=table+':'+obj['source_id']
                if kind in {'skill','memory'}:obj['description']=text(obj['description']+'\n'+'; '.join(k+'='+str(row[k]) for k in ('trigger_pattern','run_mode','parent_skill_id','scope','scope_ref','category') if row.get(k) is not None),4000,True)
                if kind=='provider':obj['base_url']=text(row.get('base_url') or '',8192,True);obj['model']=text(row.get('model') or '',256,True)
                if kind=='schedule':obj['schedule_expression']=text(row['schedule_expr'],256);obj['timezone']='UTC'
                if kind=='workflow':
                    definition=parse(text(row['definition_json'],1024*1024).encode())
                    try:obj['workflow']=workflow_projection(definition)
                    except (AssertionError,TypeError,ValueError):catalog['limitations'].append('Workflow '+obj['source_id']+': requires explicit V2 agent/dependency mapping')
                catalog['objects'].append(obj)
        for row in rows('rpc_pipelines',['id','name','description','steps_json','zero_context']):
            obj=catalog_object('workflow',row);obj['source_id']='rpc:'+obj['source_id']
            raw_steps=parse(text(row['steps_json'],1024*1024).encode())
            def safe_args(value):
                if isinstance(value,dict):
                    assert all(not any(secret in key.lower().replace('-','_') for secret in ('password','secret','token','authorization','api_key','apikey','credential')) for key in value),'Pipeline arguments contain credential fields'
                    for item in value.values():safe_args(item)
                elif isinstance(value,list):
                    for item in value:safe_args(item)
            assert isinstance(raw_steps,list) and all(isinstance(step,dict) and set(step)<={'tool','prompt','args'} for step in raw_steps),'Unsupported pipeline step fields'
            safe_args(raw_steps)
            obj['workflow']=pipeline_projection(raw_steps)
            obj['instructions']=text(json.dumps({'name':row['name'],'steps_json':json.dumps(raw_steps,ensure_ascii=False),'zero_context':bool(row.get('zero_context'))},ensure_ascii=False),256000)
            obj['description']=text(obj['description']+'; RPC-Schritte und zero_context bleiben als inerte Quelle für einen ausdrücklich geprüften V2-Entwurf erhalten. Werkzeugrechte und Zugangsdaten werden neu ausgewählt.',4000,True);catalog['objects'].append(obj)
        for row in rows('skill_improvements',['id','skill_id','version_before','version_after','reason','diff_summary','prompt_before','prompt_after']):
            obj=next((o for o in catalog['objects'] if o['kind']=='skill' and o['source_id']==row['skill_id']),None)
            assert obj is not None,'Unknown skill version parent'
            for key,version in [('prompt_before','version_before'),('prompt_after','version_after')]:
                if row.get(key):obj['versions'].append({'source_id':str(row['id'])+':'+key,'revision':int(row[version]),'content':text(row[key],64000),'summary':text(str(row.get('reason') or ''),4000,True)})
        for row in rows('crew_definition_versions',['id','crew_id','version_number','change_summary','definition_json']):
            obj=next((o for o in catalog['objects'] if o['kind']=='workflow' and o['source_id']==row['crew_id']),None)
            assert obj is not None,'Unknown workflow version parent'
            definition=parse(text(row['definition_json'],1024*1024).encode())
            # Historical version text is a sanitized definition, never raw secret/config JSON.
            try:content=json.dumps(workflow_projection(definition),ensure_ascii=False)
            except (AssertionError,TypeError,ValueError):content='Definition requires explicit V2 agent/dependency mapping'
            obj['versions'].append({'source_id':str(row['id']),'revision':int(row['version_number']),'content':content,'summary':text(str(row.get('change_summary') or ''),4000,True)})
        for table in ('engine_runs','managed_processes','worker_sandboxes','scheduled_runs','crew_runs','task_steps','tasks','work_tasks'):
            for row in rows(table,['id','status','state','revision']):
                catalog['operational_states'].append({'source_id':table+':'+str(row['id']),'source_kind':table,'state':text(str(row.get('state') or row.get('status') or 'unknown'),120),'revision':int(row.get('revision') or 1),'execution_blocked':True})
    else:
        for row in rows('daemon_entities',['entity_type','id','revision','payload_json','tombstone']):
            if row['tombstone']:
                catalog['operational_states'].append({'source_id':row['entity_type']+':'+row['id'],'source_kind':'daemon_entity','state':'deleted','revision':int(row['revision']),'execution_blocked':True});continue
            value=parse(text(row['payload_json'],1024*1024).encode());assert isinstance(value,dict)
            kind={'skill':'skill','memory':'memory','crew':'workflow','schedule':'schedule','provider_profile':'provider'}.get(row['entity_type'])
            if kind is None:continue
            value['id']=row['id'];value['revision']=row['revision']
            obj=catalog_object(kind,value,value.get('project_id'))
            if kind=='workflow':
                try:obj['workflow']=workflow_projection(value)
                except (AssertionError,TypeError,ValueError):catalog['limitations'].append('Workflow '+obj['source_id']+': requires explicit V2 agent/dependency mapping')
            if kind=='schedule':obj['schedule_expression']=text(str(value.get('cron_expression') or value.get('expression') or ''),256,True);obj['timezone']=text(str(value.get('timezone') or 'UTC'),100)
            if kind=='provider':
                defaults=value.get('model_defaults') or {};assert isinstance(defaults,dict)
                obj['base_url']=text(str(defaults.get('base_url') or ''),8192,True);obj['model']=text(str(defaults.get('model') or ''),256,True)
            catalog['objects'].append(obj)
        for row in rows('daemon_sync_changes',['cursor','entity_type','entity_id','revision','operation','entity_json']):
            obj=next((o for o in catalog['objects'] if o['source_id']==row['entity_id'] and o['kind']=={'skill':'skill','memory':'memory','crew':'workflow'}.get(row['entity_type'])),None)
            if obj is None or row['operation']!='upsert':continue
            envelope=parse(text(row['entity_json'],1024*1024).encode());assert isinstance(envelope,dict) and envelope.get('id')==row['entity_id'] and envelope.get('entity_type')==row['entity_type'] and envelope.get('revision')==row['revision'],'Conflicting historical sync version'
            payload=envelope.get('payload');assert isinstance(payload,dict)
            if obj['kind']=='workflow':
                try:content=json.dumps(workflow_projection(payload),ensure_ascii=False)
                except (AssertionError,TypeError,ValueError):content='Historische Definition benötigt eine heutige Agenten-/Abhängigkeitszuordnung.'
            else:content=text(str(payload.get('instructions') or payload.get('prompt_template') or payload.get('content') or ''),256000,True)
            obj['versions'].append({'source_id':'sync:'+str(row['cursor']),'revision':int(row['revision']),'content':content,'summary':'Historische synchronisierte Fassung; keine Ausführungsautorität.'})
        for row in rows('daemon_schedules',['id','expression','timezone','enabled']):
            if any(o['kind']=='schedule' and o['source_id']==row['id'] for o in catalog['objects']):continue
            obj=catalog_object('schedule',row);obj['schedule_expression']=text(row['expression'],256);obj['timezone']=text(row['timezone'],100);catalog['objects'].append(obj)
            catalog['limitations'].append('Schedule '+obj['source_id']+': encrypted template requires freshly supplied prompt and order')
        for row in rows('daemon_runs',['id','state','revision']):catalog['operational_states'].append({'source_id':str(row['id']),'source_kind':'daemon_run','state':text(row['state'],120),'revision':int(row.get('revision') or 1),'execution_blocked':True})
        catalog['limitations'].append('Encrypted daemon provider/MCP bindings and schedule templates are not decrypted; fresh authorization is required')
        catalog['limitations'].append('The daemon schema has no local human-account or organization table. Run actor IDs remain provenance; they do not establish human identities or roles')
    return catalog

def enterprise_json(value):
    assert value.get('format')=='open-cowork-server-public/1' and value.get('schema_revision') in range(1,62),'Unknown enterprise export schema'
    catalog=empty_catalog();data=value['tables'];assert isinstance(data,dict)
    assert set(data)<={'users','teams','team_members','projects','project_members','threads','messages','task_definitions','schedules','provider_profiles','runs','run_events','project_versions','run_artifacts','sync_entities'},'Unknown enterprise table'
    for table,items in data.items():assert isinstance(items,list) and len(items)<=10000,'Invalid enterprise row bound'
    projects={};projectrows={}
    for p in data.get('projects',[]):
        identifier=text(str(p['id']),256);assert identifier not in projects,'Duplicate enterprise project ID'
        projects[identifier]={'source_id':identifier,'name':text(p['name'],200),'orders':[]};projectrows[identifier]=p
    def public(row):
        assert not any(k in row for k in ('password_hash','totp_secret_ciphertext','recovery_codes_hashes','encrypted_secret','secret_envelope','refresh_token_hash','api_key','token')),'Export must exclude authentication and provider secrets'
    for u in data.get('users',[]):
        public(u);catalog['users'].append({'source_id':text(str(u['id']),256),'username':text(str(u.get('email') or u.get('display_name') or 'user'),200),'display_name':text(str(u.get('display_name') or u.get('email') or 'User'),200),'active':not bool(u.get('deleted_at')),'source_revision':int(u.get('revision') or 1)})
    for o in data.get('teams',[]):public(o);catalog['organizations'].append({'source_id':text(str(o['id']),256),'name':text(o['name'],200),'owner_source_id':text(str(o['owner_user_id']),256)})
    for m in data.get('team_members',[]):catalog['memberships'].append({'source_organization_id':text(str(m['team_id']),256),'source_user_id':text(str(m['user_id']),256),'role':text(m['role'],40)})
    for g in data.get('project_members',[]):catalog['project_acl'].append({'source_project_id':text(str(g['project_id']),256),'source_user_id':text(str(g['user_id']),256),'role':text(g['role'],40)})
    users={u['source_id'] for u in catalog['users']};organizations={o['source_id'] for o in catalog['organizations']}
    for p in projectrows.values():
        owner=str(p['owner_user_id']);assert owner in users,'Unknown enterprise project owner'
        if p.get('team_id') is not None:assert str(p['team_id']) in organizations,'Unknown enterprise project team'
        catalog['project_acl'].append({'source_project_id':str(p['id']),'source_user_id':owner,'role':'owner'})
    assert all(g['source_project_id'] in projects for g in catalog['project_acl']),'Unknown enterprise ACL project'
    threads={}
    for t in data.get('threads',[]):
        assert str(t['project_id']) in projects,'Unknown enterprise thread project'
        assert str(t['id']) not in threads,'Duplicate enterprise thread ID'
        order={'source_id':text(str(t['id']),256),'source_kind':'chat_thread','title':text(t.get('title') or 'Imported conversation',200),'draft':'','messages':[],'artifacts':[]};threads[str(t['id'])]=order;projects[str(t['project_id'])]['orders'].append(order)
    for m in sorted(data.get('messages',[]),key=lambda m:(str(m.get('created_at') or ''),str(m['id']))):
        assert str(m['thread_id']) in threads,'Unknown enterprise message thread'
        threads[str(m['thread_id'])]['messages'].append(message(m))
    for t in data.get('task_definitions',[]):
        assert str(t['project_id']) in projects,'Unknown task project'
        projects[str(t['project_id'])]['orders'].append({'source_id':str(t['id'])+':'+str(t['revision']),'source_kind':'task_definition','title':text(t['name'],200),'draft':text(t['instructions'],128*1024,True),'messages':[],'artifacts':[]})
    for row in data.get('provider_profiles',[]):
        public(row);obj=catalog_object('provider',row);defaults=row.get('model_defaults') or {};assert isinstance(defaults,dict);obj['base_url']=text(str(defaults.get('base_url') or ''),8192,True);obj['model']=text(str(defaults.get('model') or ''),256,True);catalog['objects'].append(obj)
    for row in data.get('schedules',[]):
        assert str(row['project_id']) in projects,'Unknown enterprise schedule project'
        obj=catalog_object('schedule',row,str(row['project_id']));obj['schedule_expression']=text(row['cron_expression'],256);obj['timezone']=text(row['timezone'],100);instructions=row.get('input') or {};obj['instructions']=text(str(instructions.get('prompt') or ''),128*1024,True);catalog['objects'].append(obj)
    for row in data.get('sync_entities',[]):
        typ=row.get('entity_type');kind={'skill':'skill','memory':'memory','crew':'workflow'}.get(typ)
        if kind:
            payload=row.get('payload',row.get('payload_json')) or {};payload=parse(payload.encode()) if isinstance(payload,str) else payload
            obj=catalog_object(kind,{**payload,'id':row.get('entity_id',row.get('id')),'revision':row.get('revision',1)},payload.get('project_id'))
            if kind=='workflow':
                try:obj['workflow']=workflow_projection(payload)
                except (AssertionError,TypeError,ValueError):catalog['limitations'].append('Workflow '+obj['source_id']+': requires explicit V2 agent/dependency mapping')
            previous=next((o for o in catalog['objects'] if o['kind']==kind and o['source_id']==obj['source_id']),None)
            if previous is not None:assert previous==obj,'Conflicting multi-user synced definition'
            else:catalog['objects'].append(obj)
    runids={};runorders={}
    for r in data.get('runs',[]):
        identifier=text(str(r['id']),256);assert identifier not in runids,'Duplicate enterprise run ID';runids[identifier]=r
    for r in runids.values():
        project=str(r['project_id']);assert project in projects,'Unknown run project'
        spec=r.get('spec') or {};assert str(spec.get('id'))==str(r['id']),'Enterprise run spec ID differs'
        order=threads.get(str(r.get('thread_id')))
        if order is None:
            order={'source_id':str(r['id']),'source_kind':'daemon_run','title':'Historical run','draft':'','messages':[],'artifacts':[]};projects[project]['orders'].append(order)
        else:assert order in projects[project]['orders'],'Conflicting enterprise run/thread project'
        runorders[str(r['id'])]=order
        inp=spec.get('input') or {};order['draft']=text(str(inp.get('prompt') or ''),128*1024,True)
        for i,m in enumerate(inp.get('messages',[])):
            if not any(x['role']==m.get('role') and x['content']==m.get('content') for x in order['messages']):order['messages'].append(message({**m,'id':str(r['id'])+':input:'+str(i)}))
        result=r.get('result') or {}
        if isinstance(result.get('content'),str) and not any(x['role']=='assistant' and x['content']==result['content'] for x in order['messages']):order['messages'].append(message({'id':str(r['id'])+':result','role':'assistant','content':result['content']}))
        catalog['operational_states'].append({'source_id':str(r['id']),'source_kind':'server_run','state':text(r['state'],120),'revision':int(r.get('revision') or 1),'execution_blocked':True})
    events=set()
    for event in sorted(data.get('run_events',[]),key=lambda e:(str(e['run_id']),e['sequence'])):
        run=str(event['run_id']);sequence=event['sequence'];assert run in runids and type(sequence) is int and sequence>=1 and (run,sequence) not in events,'Unknown or duplicate enterprise run event'
        events.add((run,sequence));payload=event.get('payload') or {};assert isinstance(payload,dict)
        catalog['operational_states'].append({'source_id':str(event['event_id']),'source_kind':'server_event','state':text(event['kind'],120),'revision':sequence,'execution_blocked':True})
        if event['kind']=='model_completed' and isinstance(payload.get('content'),str):
            parent=runorders[run]
            if not any(m['role']=='assistant' and m['content']==payload['content'] for m in parent['messages']):parent['messages'].append(message({'id':'server-event:'+run+':'+str(sequence),'role':'assistant','content':payload['content'],'created_at':event.get('created_at')}))
    versions={};versionorders={}
    for v in data.get('project_versions',[]):
        assert str(v['project_id']) in projects and str(v['id']) not in versions,'Unknown or duplicate enterprise project version';versions[str(v['id'])]=v
        obj=catalog_object('knowledge',{'id':v['id'],'name':'Project version '+str(v['revision']),'content':'Historical project version '+str(v['id'])+'; parent '+str(v.get('parent_version_id') or '')+'; snapshot '+str(v['snapshot_manifest_id']),'revision':v['revision']},str(v['project_id']));catalog['objects'].append(obj)
    for version in versions.values():
        for key in ('parent_version_id','merge_base_version_id'):
            if version.get(key) is not None:assert str(version[key]) in versions and str(versions[str(version[key])]['project_id'])==str(version['project_id']),'Unknown or cross-project historical version parent'
    manifests=value.get('decrypted_files',{});assert isinstance(manifests,dict) and len(manifests)<=1000,'Invalid selected plaintext file manifest'
    artifacts={}
    for row in data.get('run_artifacts',[]):
        identifier=text(str(row['id']),200);run=str(row['run_id']);assert identifier not in artifacts and run in runorders,'Unknown or duplicate enterprise artifact'
        artifacts[identifier]=row
        item=artifact({'id':identifier,'source_kind':'server_artifact','name':text(row['name'],300),'content':'Historisches Serverartefakt; ursprünglicher Run '+run+'; Typ '+text(row['media_type'],120)+'; ursprüngliche Größe '+str(row['size_bytes']),'created_at':row.get('created_at')})
        manifest=manifests.get('artifact:'+identifier)
        if manifest is not None:bind_plaintext_manifest(item,manifest)
        else:item['_source_missing_reason']='decrypted_file_not_provided'
        runorders[run]['artifacts'].append(item)
    for key,manifest in manifests.items():
        text(key,180);parts=key.split(':')
        if parts[0]=='artifact':assert len(parts)==2 and parts[1] in artifacts,'Unknown plaintext artifact manifest identity';continue
        assert len(parts)==3 and parts[0]=='version' and parts[1] in versions and parts[2] and all(c.isalnum() or c in '_-.' for c in parts[2]),'Unknown plaintext project version identity'
        version=versions[parts[1]];parent=versionorders.get(parts[1])
        if parent is None:
            parent={'source_id':parts[1],'source_kind':'project_version','title':'Historische Projektversion '+str(version['revision']),'draft':'Ursprüngliche Version '+parts[1]+'; frühere Datei- und Ausführungsrechte werden nicht übernommen.','messages':[],'artifacts':[]};projects[str(version['project_id'])]['orders'].append(parent);versionorders[parts[1]]=parent
        item=artifact({'id':key,'source_kind':'server_version_file','name':text(str(manifest.get('name') or parts[2]),300),'content':'Ausdrücklich bereitgestellte Klartextdatei der historischen Projektversion '+parts[1],'created_at':version.get('created_at')});bind_plaintext_manifest(item,manifest);parent['artifacts'].append(item)
    if data.get('run_artifacts') or versions:catalog['limitations'].append('Encrypted server chunks require explicitly supplied plaintext files bound to IDs, size and SHA-256. Files without such an export remain listed as unavailable; no source key or object-store credential is imported')
    return list(projects.values()),catalog

def bind_plaintext_manifest(item,manifest):
    assert isinstance(manifest,dict) and set(manifest)<={'relative_path','size','sha256','name'},'Unknown plaintext manifest field'
    path=text(manifest['relative_path'],4096);relative=pathlib.PurePosixPath(path)
    assert not relative.is_absolute() and not pathlib.PureWindowsPath(path).drive and '\\' not in path and relative.parts and '..' not in relative.parts and ':' not in path,'Plaintext export path must be relative to the explicitly selected root'
    size=manifest['size'];digest=manifest['sha256'];assert type(size) is int and 0<=size<=16*1024*1024 and isinstance(digest,str) and len(digest)==64 and all(c in '0123456789abcdef' for c in digest),'Invalid plaintext export byte pins'
    item['_source_path']=path;item['_expected_size']=size;item['_expected_sha']=digest

def main():
    source=pathlib.Path(sys.argv[1]);kind=sys.argv[2];directory=pathlib.Path(sys.argv[3]).resolve(strict=True)
    cold=sys.argv[4]=='true'
    family=sys.argv[5] if len(sys.argv)>5 else 'desktop';assert family in {'desktop','daemon','enterprise'}
    source_root=sys.argv[6] if len(sys.argv)>6 and sys.argv[6] else None
    assert source.is_absolute() and directory.is_dir() and kind in {'json','sqlite'}
    assert source.resolve(strict=True)!=directory and not source.resolve().is_relative_to(directory)
    if kind=='json':
        copied=directory/'source.json'
        if os.name=='nt':
            source=legacy.bounded_path(source)
            with legacy.FrozenFiles([source]) as fence:
                raw=fence.read(source,MAX_JSON);before=fence.receipt(source)
                with copied.open('xb') as stream:stream.write(raw);stream.flush();os.fsync(stream.fileno())
                assert fence.receipt(source)==before
                source_hash=sha(raw)
            provenance='windows-deny-write-delete-json'
        elif sys.platform=='linux':source_hash=unix_copy(source,copied,MAX_JSON);provenance='linux-selected-json-raw-copy'
        else:raise ValueError('Unsupported native source platform')
        value=parse(copied.read_bytes())
        if family=='enterprise':projects,catalog=enterprise_json(value);profile='selected-server-public-v1'
        else:projects=normalize_json(value);catalog=value.get('catalog',empty_catalog());profile='selected-json-content-v1'
        entities=[]
    else:
        assert family!='enterprise','Enterprise import requires the explicit public JSON export'
        if os.name=='nt':
            result=legacy.inventory(source,family,directory/'inventory')
            metadata=parse((directory/'inventory/inventory.json').read_bytes())
            copied=directory/'inventory/snapshot.sqlite3'
            source_hash=result['approved_inventory_sha256'];profile=result['profile'];provenance='windows-deny-write-delete-sqlite-snapshot'
        elif sys.platform=='linux':
            assert cold,'Linux SQLite capture requires an explicitly confirmed cold source'
            siblings=[pathlib.Path(str(source)+suffix) for suffix in ('-wal','-shm','-journal')]
            assert not any(path.exists() for path in siblings),'Linux cold capture refuses SQLite sidecars'
            copied=directory/'snapshot.sqlite3';source_hash=unix_copy(source,copied,MAX_SQLITE)
            assert not any(path.exists() for path in siblings),'SQLite sidecars appeared during cold capture'
            with legacy.database(copied) as connection:
                details=legacy.inspect(connection,family,legacy.Budget(time.monotonic()+110))
            profile=details['profile'];provenance='linux-explicit-cold-raw-copy'
        else:raise ValueError('Unsupported native source platform')
        with legacy.database(copied) as connection:
            catalog=sqlite_catalog(connection,family)
            if family=='daemon':projects,entities=normalize_daemon(connection)
            else:
                projects=normalize_sqlite(connection);entities=[]
                if connection.execute("SELECT 1 FROM sqlite_schema WHERE type='table' AND name='agent_personalities'").fetchone():
                    for identifier,name in connection.execute('SELECT id,name FROM agent_personalities ORDER BY id'):
                        entities.append({'kind':'persona','source_id':text(str(identifier),256),'name':text(name,300),'source_project_id':None,'source_actor_id':None})
    if not projects and (catalog['objects'] or catalog['users']):projects=[{'source_id':'unassigned-v1-content','name':'Importierte V1-Konfiguration','orders':[]}]
    missing,root_identity=capture_attachments(projects,source_root,directory)
    validate(projects)
    result={'format':'localai-cowork-v1-user-content/1','source_sha256':source_hash,
        'source_profile':profile+';'+provenance,'projects':projects,'source_entities':entities,'missing_attachments':missing,'source_root_identity':root_identity,'catalog':catalog}
    raw=json.dumps(result,ensure_ascii=False,allow_nan=False,separators=(',',':')).encode('utf-8')
    assert len(raw)<=64*1024*1024
    with (directory/'content.json').open('xb') as output:output.write(raw);output.flush();os.fsync(output.fileno())
    print(json.dumps({'content_sha256':sha(raw),'source_sha256':source_hash,'profile':profile,
        'provenance':provenance,'projects':len(projects)}))
if __name__=='__main__':main()
