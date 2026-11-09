"""One strict action inside the operator-selected isolated team-tool container."""
import json, os, pathlib, re, selectors, subprocess, sys, time, uuid
from urllib.parse import urlsplit

ROOT=pathlib.Path('/workspace').resolve(strict=True)
ADAPTERS=pathlib.Path('/opt/cowork-team-tools')
PROGRAMS={'python':'/opt/cowork/python/bin/python','node':'/usr/bin/node','git':'/usr/bin/git'}

def require(value,message):
    if not value:raise ValueError(message)
def pairs(items):
    result={}
    for key,value in items:
        require(key not in result,'Duplicate action JSON member');result[key]=value
    return result
def text(value,maximum,empty=False):
    require(isinstance(value,str) and (empty or value) and '\0' not in value and len(value.encode())<=maximum,'Invalid bounded action text')
    return value
def shape(value,allowed,required):
    require(isinstance(value,dict) and set(value)<=set(allowed) and set(required)<=set(value),'Unknown or missing action members')
def relative(value,exists=False):
    value=text(value,4096,True)
    path=pathlib.Path(value or '.')
    require(not path.is_absolute() and '..' not in path.parts,'Workspace-relative path required')
    target=(ROOT/path).resolve(strict=exists)
    require(target.is_relative_to(ROOT),'Path escapes the selected order workspace')
    return target
def arguments(value):
    require(isinstance(value,list) and len(value)<=128,'Invalid argument vector')
    result=[text(item,8192,True) for item in value]
    require(sum(len(item.encode()) for item in result)<=32768,'Argument vector is oversized')
    return result
def url(value):
    value=text(value,8192);parsed=urlsplit(value)
    require(parsed.scheme in {'http','https'} and parsed.hostname and not parsed.username and not parsed.password,'Explicit HTTP(S) URL without credentials required')
    return value
def run(command,cwd=ROOT):
    # Docker's retained named-container lifecycle owns every guest process.
    child=subprocess.Popen(command,cwd=cwd,stdin=subprocess.DEVNULL)
    try:code=child.wait(timeout=600)
    except subprocess.TimeoutExpired:
        child.terminate()
        try:child.wait(timeout=5)
        except subprocess.TimeoutExpired:child.kill();child.wait()
        raise
    require(code==0,'Guest command failed with exit code '+str(code))

def github(argv,token,host):
    require(argv and argv[0] in {'api','repo','issue','pr','release','workflow','run','search','gist'},'Unsupported GitHub command')
    require(host and len(host)<=253 and re.fullmatch(r'[A-Za-z0-9](?:[A-Za-z0-9.-]*[A-Za-z0-9])?',host)
        and all(label and len(label)<=63 and not label.startswith('-') and not label.endswith('-') for label in host.split('.')),'Invalid fixed GitHub hostname')
    require(token and len(token.encode())<=4096 and not any(ord(c)<=32 or ord(c)==127 for c in token),'Missing or invalid team GitHub credential')
    denied={'--','--web','--browser','--verbose','--hostname','--header','-H','--input','--config','--git-protocol','--with-token','--clone','--remote','--source','-s','--push','--body-file','--templatefile','--template','--jq','-q','-t'}
    text_positions=set()
    for index,value in enumerate(argv):
        if value in {'--title','-b','--body'}:
            require(index+1<len(argv) and not argv[index+1].startswith(('-','@')) and '=@' not in argv[index+1],'Missing or invalid content flag value');text_positions.add(index+1)
        if value in {'-f','-F','--raw-field','--field'}:
            require(index+1<len(argv) and '=' in argv[index+1] and not argv[index+1].split('=',1)[1].startswith('@'),'Inline API field required')
            text_positions.add(index+1)
        if value.startswith(('--raw-field=','--field=')):
            selected=value.split('=',1)[1];require('=' in selected and not selected.split('=',1)[1].startswith('@'),'Inline API field required')
    for index,value in enumerate(argv):
        if index in text_positions or value.startswith(('--title=','--body=','--raw-field=','--field=')):continue
        require(value not in denied and not any(value.startswith(flag+'=') for flag in denied if flag.startswith('--'))
            and not value.startswith(('-H','-q','-t','-s')) and not (value.startswith(('-f','-F')) and len(value)>2) and not re.match(r'^[A-Za-z][A-Za-z0-9+.-]*:',value)
            and not value.startswith('//'),'GitHub host, header, file or interactive override refused')
    # These operations execute git or use workspace state. Authentication is
    # deliberately restricted to remote API management from a new private cwd.
    if argv[0]=='repo':require(len(argv)>1 and argv[1] in {'view','list','create','edit','delete','rename','archive','unarchive'},'Git-spawning repository operation refused')
    if argv[0]=='pr':require(len(argv)>1 and argv[1] not in {'checkout','diff'},'Local Git operation refused')
    if argv[0]=='gist':require(len(argv)>1 and argv[1]!='clone','Local Git operation refused')
    if argv[0]=='api':
        endpoint=next((v for v in argv[1:] if not v.startswith('-')),None)
        require(endpoint and not endpoint.startswith('/') and '://' not in endpoint,'Relative GitHub API endpoint required')
    for index,value in enumerate(argv):
        selected=None
        if value in {'-R','--repo'}:
            require(index+1<len(argv),'Missing explicit repository');selected=argv[index+1]
        elif value.startswith('--repo='):selected=value.split('=',1)[1]
        elif value.startswith('-R') and len(value)>2:selected=value[2:]
        if selected:
            parts=selected.split('/');require(len(parts) in {2,3} and all(re.fullmatch(r'[A-Za-z0-9_.-]+',part) for part in parts),'Invalid remote repository')
            require(len(parts)==2 or parts[0].lower()==host.lower(),'Foreign GitHub repository host refused')
    cwd=pathlib.Path('/tmp')/('github-'+uuid.uuid4().hex);cwd.mkdir(mode=0o700)
    config=cwd/'config';config.mkdir(mode=0o700)
    os.environ.update({'GH_HOST':host,'GH_TOKEN':token,'GH_CONFIG_DIR':str(config),'GH_PROMPT_DISABLED':'1',
        'GH_NO_UPDATE_NOTIFIER':'1','GIT_TERMINAL_PROMPT':'0','GIT_CONFIG_NOSYSTEM':'1','GIT_CONFIG_GLOBAL':'/dev/null'})
    child=subprocess.Popen(['/usr/bin/gh',*argv],cwd=cwd,stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
    selector=selectors.DefaultSelector();secret=token.encode();buffers={};counts={}
    for stream,destination in ((child.stdout,sys.stdout.buffer),(child.stderr,sys.stderr.buffer)):
        os.set_blocking(stream.fileno(),False);selector.register(stream,selectors.EVENT_READ,destination);buffers[stream]=b'';counts[stream]=0
    deadline=time.monotonic()+600
    try:
        while selector.get_map():
            require(time.monotonic()<deadline,'GitHub command exceeded timeout')
            for key,_ in selector.select(timeout=.2):
                chunk=os.read(key.fileobj.fileno(),65536)
                raw=buffers[key.fileobj]+chunk
                if chunk:
                    boundary=max(0,len(raw)-len(secret)+1);safe=raw[:boundary];buffers[key.fileobj]=raw[boundary:]
                    # Do not split a token across output chunks.
                    occurrence=raw.rfind(secret,0,boundary+len(secret)-1)
                    if occurrence>=0 and occurrence+len(secret)>boundary:
                        safe=raw[:occurrence];buffers[key.fileobj]=raw[occurrence:]
                else:safe=raw;buffers[key.fileobj]=b'';selector.unregister(key.fileobj);key.fileobj.close()
                safe=safe.replace(secret,b'[REDACTED]');remaining=max(0,1024*1024-counts[key.fileobj])
                key.data.write(safe[:remaining]);key.data.flush();counts[key.fileobj]+=len(safe)
        require(child.wait(timeout=max(.1,deadline-time.monotonic()))==0,'GitHub command failed')
    finally:
        selector.close()
        if child.poll() is None:
            child.terminate()
            try:child.wait(timeout=5)
            except subprocess.TimeoutExpired:child.kill();child.wait()
        else:child.wait()
def main():
    require(len(sys.argv)==2 and len(sys.argv[1].encode())<=2*1024*1024,'One bounded JSON action required')
    value=json.loads(sys.argv[1],object_pairs_hook=pairs,parse_constant=lambda _:(_ for _ in ()).throw(ValueError('Nonfinite action JSON')))
    shape(value,{'tool','action'},{'tool','action'});tool=value['tool'];action=value['action']
    os.umask(0o077)
    # No inherited daemon authority or user profile credentials are made available.
    github_token=os.environ.get('GH_TOKEN','') if tool=='github' else ''
    github_host=os.environ.get('GH_HOST','') if tool=='github' else ''
    os.environ.clear();os.environ.update({'LANG':'C.UTF-8','HOME':'/tmp/home','TMPDIR':'/tmp',
        'PATH':'/opt/cowork/python/bin:/usr/local/bin:/usr/bin:/bin','PYTHONUNBUFFERED':'1'})
    pathlib.Path('/tmp/home').mkdir(mode=0o700,exist_ok=True)
    if tool=='execute_program':
        shape(action,{'program','arguments','cwd'},{'program'})
        require(action['program'] in PROGRAMS,'Unknown guest program alias')
        cwd=relative(action.get('cwd',''),True);require(cwd.is_dir(),'Working directory is not a directory')
        run([PROGRAMS[action['program']],*arguments(action.get('arguments',[]))],cwd)
    elif tool=='browser':
        shape(action,{'session_id','operation','url','selector','text'},{'operation'})
        require(action['operation'] in {'navigate','dom','screenshot','click','fill'},'Unknown browser operation')
        identifier=str(uuid.UUID(action['session_id'])) if action.get('session_id') else str(uuid.uuid4())
        parent=relative('.browser');parent.mkdir(mode=0o700,exist_ok=True)
        profile=relative('.browser/'+identifier);profile.mkdir(mode=0o700,exist_ok=True)
        metadata=profile/'metadata.json'
        previous=json.loads(metadata.read_bytes()) if metadata.exists() else {}
        selected=url(action.get('url') or previous.get('url'))
        selected_action=dict(action,session_id=identifier,url=selected)
        if action['operation'] in {'click','fill'}:text(action.get('selector'),4096)
        if action['operation']=='fill':text(action.get('text'),32768,True)
        temporary=profile/('metadata-'+uuid.uuid4().hex+'.json')
        with temporary.open('x') as output:json.dump({'url':selected,'session_id':identifier},output)
        os.replace(temporary,metadata)
        run([PROGRAMS['python'],'-I',str(ADAPTERS/'browser.py'),str(profile),str(ROOT),'/usr/bin/chromium',json.dumps(selected_action,separators=(',',':'))])
    elif tool=='office':
        shape(action,{'operation','input','output','text','title'},{'operation'})
        require(action['operation'] in {'read_text','read_docx','read_xlsx','read_pptx','read_pdf','write_text','create_docx','create_xlsx','create_pptx','create_pdf','convert'},'Unknown Office operation')
        for field in ('input','output'):
            if action.get(field):relative(action[field],field=='input')
        if action.get('text') is not None:text(action['text'],1024*1024,True)
        if action.get('title') is not None:text(action['title'],300,True)
        encoded=dict(action)
        if action['operation']=='convert':encoded['libreoffice']='/usr/bin/libreoffice'
        # A private /tmp input avoids the OS argument-size limit for documents.
        path=pathlib.Path('/tmp')/('office-'+uuid.uuid4().hex+'.json')
        try:
            with path.open('x') as output:json.dump(encoded,output,separators=(',',':'))
            run([PROGRAMS['python'],'-I',str(ADAPTERS/'office.py'),str(ROOT),'@'+str(path)])
        finally:path.unlink(missing_ok=True)
    elif tool=='github':
        if 'operation' in action:
            import importlib.util
            spec=importlib.util.spec_from_file_location('cowork_github_repository',ADAPTERS/'github_repository.py')
            require(spec is not None and spec.loader is not None,'GitHub repository adapter unavailable')
            adapter=importlib.util.module_from_spec(spec);spec.loader.exec_module(adapter)
            adapter.execute(action,github_token,github_host,ROOT)
        else:
            shape(action,{'arguments'},{'arguments'});argv=arguments(action['arguments'])
            github(argv,github_token,github_host)
    else:raise ValueError('Unknown guest tool')
if __name__=='__main__':
    try:main()
    except (ValueError,AssertionError,OSError,subprocess.SubprocessError,KeyError,TypeError) as error:
        print('Team guest refused: '+str(error),file=sys.stderr);raise SystemExit(1)
