"""Explicit operator lifecycle for one UUID-owned Compose deployment.

Creating this file does not start containers. Commands act only on the selected
deployment marker/project. Backup includes database AND private credential/key
custody, encrypted with age; restore/rollback never silently overwrite a stack.
"""
from __future__ import annotations
import argparse, contextlib, hashlib, json, os, pathlib, re, secrets, shutil
import signal, stat, subprocess, tarfile, tempfile, time, uuid, urllib.parse
from s3_backup import ObjectBackup
import workspace_backup as workspaces

HERE=pathlib.Path(__file__).resolve().parent
FORMAT='localai-cowork-selfhost/1'
def require(value,message):
    if not value:raise ValueError(message)
def sha(path):
    result=hashlib.sha256()
    with path.open('rb') as stream:
        while chunk:=stream.read(1024*1024):result.update(chunk)
    return result.hexdigest()
def private(path,directory=False):
    require(not path.is_symlink(),'Refusing a symlink in deployment namespace')
    if directory:path.mkdir(mode=0o700,parents=True,exist_ok=True)
    if os.name=='posix':os.chmod(path,0o700 if directory else 0o600)
def write_new(path,raw):
    with path.open('xb') as stream:stream.write(raw);stream.flush();os.fsync(stream.fileno())
    private(path)
def write_json(path,value):write_new(path,json.dumps(value,indent=2,sort_keys=True).encode())
def site(value):
    require(bool(re.fullmatch(r'[a-z0-9](?:[a-z0-9.-]{0,251}[a-z0-9])?(?::[0-9]{1,5})?',value)),'Use a canonical lowercase DNS host, optionally :port')
    return value
def image(value):
    require(bool(re.fullmatch(r'[a-zA-Z0-9][a-zA-Z0-9/_.:@-]{0,255}',value)),'Invalid image reference')
    return value
def init(root,hostname,image_name,s3=False,gvisor=False):
    require(root.is_absolute() and not root.exists(),'Initialize an explicitly selected NEW absolute directory')
    parent=root.parent.resolve(strict=True);require(not root.parent.is_symlink(),'Deployment parent must not be a symlink')
    root=parent/root.name;private(root,True)
    identifier=uuid.uuid4().hex
    value={'format':FORMAT,'id':identifier,'root':str(root),'project':'cowork-'+identifier[:16],
        'site':site(hostname),'image':image(image_name),'s3':s3,'gvisor':gvisor,'source_root':str(HERE.parent.parent)}
    for directory in ['data','postgres','minio','secrets','backups','caddy-data','caddy-config']:private(root/directory,True)
    for name in ['postgres_password','minio_access_key','minio_secret_key']:write_new(root/'secrets'/name,secrets.token_hex(32).encode())
    write_json(root/'deployment.json',value);save_env(root,value)
    return value
def load(root):
    require(root.is_absolute() and not root.is_symlink(),'Select the owned absolute deployment directory')
    root=root.resolve(strict=True);path=root/'deployment.json'
    require(path.is_file() and not path.is_symlink() and path.stat().st_size<131072,'Missing deployment ownership marker')
    value=json.loads(path.read_bytes());require(value['format']==FORMAT and value['root']==str(root),'Deployment ownership mismatch')
    require(re.fullmatch(r'[0-9a-f]{32}',value['id']) and value['project']=='cowork-'+value['id'][:16],'Invalid owned project identity')
    site(value['site']);image(value['image']);return root,value
def save_env(root,value):
    entries={'COWORK_DEPLOY_ROOT':str(root),'COWORK_SOURCE_ROOT':value['source_root'],
        'COWORK_PROJECT':value['project'],'COWORK_IMAGE':value['image'],'COWORK_SITE':value['site'],
        'COWORK_UID':str(os.getuid() if hasattr(os,'getuid') else 1000),
        'COWORK_GID':str(os.getgid() if hasattr(os,'getgid') else 1000)}
    existing=root/'.env'
    entries['COWORK_TEAM_MCP_ALLOWED_ORIGINS']=''
    entries['COWORK_TEAM_LITELLM_HTTP_ORIGINS']=''
    for key in ('COWORK_PASSKEY_ORIGIN','COWORK_PASSKEY_RP_ID','COWORK_FCM_SERVICE_ACCOUNT_FILE','COWORK_FLEET_KEY_FILE','COWORK_ORGANISATION_ID','COWORK_INSTANCE_ID'):
        entries[key]=''
    if existing.exists():
        for line in existing.read_text().splitlines():
            if line.startswith("COWORK_TEAM_MCP_ALLOWED_ORIGINS='") and line.endswith("'"):
                entries['COWORK_TEAM_MCP_ALLOWED_ORIGINS']=line.split('=',1)[1][1:-1]
            if line.startswith("COWORK_TEAM_LITELLM_HTTP_ORIGINS='") and line.endswith("'"):
                entries['COWORK_TEAM_LITELLM_HTTP_ORIGINS']=line.split('=',1)[1][1:-1]
            for key in ('COWORK_PASSKEY_ORIGIN','COWORK_PASSKEY_RP_ID','COWORK_FCM_SERVICE_ACCOUNT_FILE','COWORK_FLEET_KEY_FILE','COWORK_ORGANISATION_ID','COWORK_INSTANCE_ID'):
                if line.startswith(key+"='") and line.endswith("'"):
                    entries[key]=line.split('=',1)[1][1:-1]
    if value.get('team_tools'):
        tools=value['team_tools'];entries.update({'COWORK_TEAM_TOOL_IMAGE':tools['image'],
            'COWORK_TEAM_TOOL_HOST_ROOT':tools['host_root'],'COWORK_DOCKER_GID':str(tools['docker_gid']),
            'COWORK_DOCKER_HOST_ID':tools.get('host_id',''),
            'COWORK_TEAM_TOOL_NETWORK':tools['network'],'COWORK_TEAM_TOOL_RUNTIME':tools.get('runtime') or ''})
    if value.get('external_s3'):
        configuration=value['external_s3']
        entries.update(COWORK_S3_ENDPOINT=configuration['endpoint'],COWORK_S3_BUCKET=configuration['bucket'],
                       COWORK_S3_REGION=configuration.get('region','us-east-1'),
                       COWORK_S3_ACCESS_KEY_FILENAME=pathlib.Path(configuration['access_key_file']).name,
                       COWORK_S3_SECRET_KEY_FILENAME=pathlib.Path(configuration['secret_key_file']).name)
    require(all('\n' not in v and '\r' not in v and "'" not in v for v in entries.values()),'Invalid env value')
    raw=''.join(k+"='"+v+"'\n" for k,v in entries.items()).encode()
    temporary=root/('.env-'+uuid.uuid4().hex);write_new(temporary,raw);os.replace(temporary,root/'.env')
def command(argv,*,output=None,input_file=None,capture=False,timeout=1800):
    require(argv and shutil.which(argv[0]),'Required operator executable is unavailable: '+argv[0])
    child=subprocess.Popen(argv,stdin=input_file or subprocess.DEVNULL,stdout=output or (subprocess.PIPE if capture else None),
        stderr=subprocess.PIPE if capture else None,start_new_session=os.name=='posix',
        env={key:value for key,value in os.environ.items() if not key.startswith('COWORK_')})
    pidfd=os.pidfd_open(child.pid) if hasattr(os,'pidfd_open') else None
    try:
        if capture:
            stdout,stderr=child.communicate(timeout=timeout)
            require(len(stdout)+len(stderr)<=16*1024*1024,'Operator command output exceeded limit')
        else:child.wait(timeout=timeout);stdout=b''
        require(child.returncode==0,'Operator command failed: '+argv[0])
        return stdout
    except subprocess.TimeoutExpired:
        # Signal ONLY the retained direct CLI child; never terminate arbitrary containers/PIDs.
        if pidfd is not None:signal.pidfd_send_signal(pidfd,signal.SIGTERM)
        else:child.terminate()
        try:child.wait(timeout=10)
        except subprocess.TimeoutExpired:
            if pidfd is not None:signal.pidfd_send_signal(pidfd,signal.SIGKILL)
            else:child.kill()
            child.wait()
        raise
    finally:
        if pidfd is not None:os.close(pidfd)
def compose(root,value,*args,capture=False,output=None,input_file=None):
    argv=['docker','compose','--project-name',value['project'],'--project-directory',str(root),
        '--env-file',str(root/'.env'),'-f',str(HERE/'compose.yml')]
    if value['s3']:argv+=['-f',str(HERE/'compose.s3.yml'),'--profile','s3']
    if value.get('team_tools'):argv+=['-f',str(HERE/'compose.team-tools.yml')]
    if value.get('gvisor'):argv+=['-f',str(HERE/'compose.gvisor.yml')]
    if value.get('external_s3'):argv+=['-f',str(HERE/'compose.external-s3.yml')]
    return command([*argv,*args],capture=capture,output=output,input_file=input_file)
def running(root,value):
    raw=compose(root,value,'ps','--format','json',capture=True)
    if not raw.strip():return []
    records=json.loads(raw) if raw.lstrip().startswith(b'[') else [json.loads(line) for line in raw.splitlines() if line]
    names=[entry['Service'] for entry in records if entry.get('State')=='running']
    require(all(name in ['postgres','api','worker','proxy','minio','minio-init'] for name in names),'Unexpected service in owned deployment')
    return names
def stop_writers(root,value):
    active=running(root,value);writers=[name for name in active if name!='postgres']
    if writers:compose(root,value,'stop','--timeout','45',*writers)
    return writers

def workspace_quiescence(root,value):
    if not value.get('team_tools'):return None
    host=pathlib.Path(value['team_tools']['host_root'])
    return workspaces.quiesce(host,root/'data',value['id'],host_id=value['team_tools'].get('host_id'))

def restore_workspace(root,value,stage,manifest,new_target=None):
    source=manifest['deployment']
    has_workspace=bool(manifest.get('team_workspace'))
    require(has_workspace or not source.get('team_tools'),'Legacy backup omitted team-tool workspaces; refusing incomplete restoration')
    if not has_workspace:return None
    archive=stage/'team-workspaces.tar';workspaces.validate_archive(archive)
    if new_target is not None:
        require(new_target.is_absolute() and not new_target.exists(),'Fresh team-workspace restore requires a new absent absolute target')
        workspaces.directory(new_target.parent);new_target.mkdir(mode=0o700)
        value['team_tools']={**source['team_tools'],'host_root':str(new_target),'host_id':str(uuid.uuid4())}
        workspaces.owned(new_target,value['id'],True)
        save_marker(root,value)
    require(value.get('team_tools'),'Configure the exact target team-workspace directory before restore')
    target=pathlib.Path(value['team_tools']['host_root'])
    return workspaces.restore_workspace(target,archive,value['id'],source['id'])
def archive_files(root):
    entries=[]
    for directory in ['data','secrets','minio','caddy-data','caddy-config']:
        base=root/directory
        require(base.is_dir() and not base.is_symlink(),'Private source directory changed')
        for path in sorted(base.rglob('*')):
            info=path.lstat();require(not stat.S_ISLNK(info.st_mode),'Backup refuses symlinks')
            if path.is_dir():continue
            require(stat.S_ISREG(info.st_mode) and info.st_nlink==1,'Backup requires regular, unaliased files')
            entries.append({'path':path.relative_to(root).as_posix(),'bytes':info.st_size,'sha256':sha(path)})
    return entries
def backup(root,value,recipient,resume=True):
    require(re.fullmatch(r'age1[0-9a-z]{50,100}|ssh-(?:ed25519|rsa) [A-Za-z0-9+/=]+(?: .*)?',recipient),'Provide an explicit age public recipient')
    writers=stop_writers(root,value)
    quiescence=workspace_quiescence(root,value)
    completed=False
    result=root/'backups'/(time.strftime('%Y%m%dT%H%M%SZ',time.gmtime())+'-'+uuid.uuid4().hex+'.tar.age')
    try:
        with tempfile.TemporaryDirectory(prefix='.cowork-backup-',dir=root/'backups') as temporary:
            stage=pathlib.Path(temporary);private(stage,True);database=stage/'database.dump'
            with database.open('xb') as output:compose(root,value,'exec','-T','postgres','pg_dump','-U','cowork','-d','cowork','--format=custom',output=output)
            object_receipt=ObjectBackup(value['external_s3']).backup(stage/'external-s3') if value.get('external_s3') else None
            files=archive_files(root)
            workspace=None
            if quiescence:
                workspace=workspaces.archive_workspace(pathlib.Path(value['team_tools']['host_root']),stage/'team-workspaces.tar',value['id'],quiescence)
            manifest={'format':FORMAT+'/backup','deployment':value,'files':files,
                'database':{'bytes':database.stat().st_size,'sha256':sha(database)},
                'external_s3_objects_included':object_receipt is not None,'external_s3':object_receipt,
                'local_minio_objects_included':value['s3']}
            if workspace:
                manifest['team_workspace']={'format':workspace['format'],'workspace_id':workspace['workspace_id'],'quiescence':quiescence}
                manifest['files'].append({'path':'team-workspaces.tar','bytes':(stage/'team-workspaces.tar').stat().st_size,'sha256':sha(stage/'team-workspaces.tar')})
            if object_receipt:
                for path in sorted((stage/'external-s3').iterdir()):
                    manifest['files'].append({'path':'external-s3/'+path.name,'bytes':path.stat().st_size,'sha256':sha(path)})
            write_json(stage/'manifest.json',manifest)
            plain=stage/'backup.tar'
            with tarfile.open(plain,'w') as archive:
                archive.add(database,arcname='database.dump',recursive=False);archive.add(stage/'manifest.json',arcname='manifest.json',recursive=False)
                for entry in manifest['files']:
                    source=stage/entry['path'] if entry['path'].startswith('external-s3/') or entry['path']=='team-workspaces.tar' else root/entry['path']
                    archive.add(source,arcname=entry['path'],recursive=False)
            require(archive_files(root)==[entry for entry in files if not entry['path'].startswith('external-s3/') and entry['path']!='team-workspaces.tar'],'Private key/data source changed during stopped backup')
            workspace_quiescence(root,value)
            command(['age','-r',recipient,'-o',str(result),str(plain)])
        private(result);receipt={'format':FORMAT+'/backup-receipt','deployment_id':value['id'],'file':str(result),'sha256':sha(result),'stopped_services':writers,'team_workspace_included':quiescence is not None,'team_workspace_quiescence':quiescence}
        write_json(result.with_suffix(result.suffix+'.json'),receipt)
        completed=True
        return receipt
    finally:
        if writers and resume and completed:compose(root,value,'start',*writers)
@contextlib.contextmanager
def decoded(root,input_path,identity):
    require(input_path.is_absolute() and input_path.is_file() and not input_path.is_symlink(),'Select an encrypted backup file')
    require(identity.is_absolute() and identity.is_file() and not identity.is_symlink(),'Select the private age identity file')
    if os.name=='posix':require(identity.stat().st_mode&0o077==0,'age identity must be private')
    with tempfile.TemporaryDirectory(prefix='.cowork-restore-',dir=root/'backups') as temporary:
        stage=pathlib.Path(temporary);plain=stage/'backup.tar';command(['age','-d','-i',str(identity),'-o',str(plain),str(input_path)])
        with tarfile.open(plain,'r:') as archive:
            members=archive.getmembers();require(len(members)<=100000,'Backup entry budget exceeded')
            byname={}
            for member in members:
                path=pathlib.PurePosixPath(member.name)
                require(member.isfile() and not path.is_absolute() and '..' not in path.parts and '\\' not in member.name and member.name not in byname,'Unsafe or duplicate backup member')
                require(member.name in ['database.dump','manifest.json','team-workspaces.tar'] or path.parts[0] in ['data','secrets','minio','caddy-data','caddy-config','external-s3'],'Unknown backup member')
                byname[member.name]=member
            require(byname['manifest.json'].size<=32*1024*1024,'Oversized backup manifest')
            manifest=json.load(archive.extractfile(byname['manifest.json']));require(manifest['format']==FORMAT+'/backup','Unknown backup format')
            expected={entry['path']:entry for entry in manifest['files']}
            expected['database.dump']=manifest['database']
            require(set(byname)==set(expected)|{'manifest.json'},'Backup manifest/member mismatch')
            for name,entry in expected.items():
                member=byname[name];require(member.size==entry['bytes'],'Backup size mismatch')
                target=stage/name;target.parent.mkdir(mode=0o700,parents=True,exist_ok=True)
                with archive.extractfile(member) as source,target.open('xb') as output:shutil.copyfileobj(source,output,1024*1024)
                private(target);require(sha(target)==entry['sha256'],'Backup content hash mismatch')
            require(bool(manifest.get('team_workspace'))==('team-workspaces.tar' in expected),'Team workspace manifest/member mismatch')
            if manifest.get('team_workspace'):workspaces.validate_archive(stage/'team-workspaces.tar')
        yield stage,manifest
def install_files(root,stage):
    for name in ['data','secrets','minio','caddy-data','caddy-config']:
        source=stage/name
        if not source.exists():private(root/name,True);continue
        # Caller has proved this is a fresh restore or explicitly stopped rollback.
        target=root/name
        if target.exists():
            retained=root/(name+'-before-restore-'+uuid.uuid4().hex)
            require(target.resolve().parent==root and not target.is_symlink(),'Refuse restoring an unowned path')
            target.rename(retained)
        source.rename(target);private(target,True)
def healthy(root,value):
    deadline=time.monotonic()+180
    while time.monotonic()<deadline:
        identifier=compose(root,value,'ps','-q','api',capture=True).decode().strip()
        if re.fullmatch(r'[a-f0-9]{12,64}',identifier):
            state=command(['docker','inspect','--format','{{if .State.Health}}{{.State.Health.Status}}{{end}}',identifier],capture=True).decode().strip()
            if state=='healthy' and 'worker' in running(root,value):return
            if state=='unhealthy':break
        time.sleep(2)
    raise ValueError('Owned API did not become healthy; backup checkpoint is retained')
def save_marker(root,value):
    temporary=root/('deployment-'+uuid.uuid4().hex+'.json');write_json(temporary,value);os.replace(temporary,root/'deployment.json');save_env(root,value)

def configure_external_s3(root,value,configuration):
    require(not value['s3'],'External S3 and local MinIO are separate deployment modes')
    ObjectBackup(configuration) # validate configuration/dependencies without network IO
    require(configuration.get('prefix','') in ('','cowork/v1/'),'The runtime requires the original cowork/v1 object-key namespace')
    current=dict(configuration)
    for name in ['access_key_file','secret_key_file']:
        source=pathlib.Path(configuration[name]);target=root/'secrets'/('s3-'+name+'-'+uuid.uuid4().hex)
        write_new(target,source.read_bytes());current[name]=str(target)
    require(not configuration.get('session_token_file'),'The daemon requires durable S3 access/secret keys')
    value['external_s3']=current;save_marker(root,value)
def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--root',type=pathlib.Path,required=True)
    commands=parser.add_subparsers(dest='command',required=True)
    p=commands.add_parser('init');p.add_argument('--site',required=True);p.add_argument('--image',default='localai-cowork-v2:local');p.add_argument('--local-s3',action='store_true');p.add_argument('--gvisor',action='store_true')
    p=commands.add_parser('build');p=commands.add_parser('start');p=commands.add_parser('stop');p=commands.add_parser('status')
    p=commands.add_parser('configure-team-tools');p.add_argument('--host-root',type=pathlib.Path,required=True);p.add_argument('--image',required=True);p.add_argument('--docker-gid',type=int,required=True);p.add_argument('--network',default='none');p.add_argument('--runtime')
    p=commands.add_parser('configure-external-s3');p.add_argument('--configuration',type=pathlib.Path,required=True)
    p=commands.add_parser('backup');p.add_argument('--recipient',required=True)
    p=commands.add_parser('restore');p.add_argument('--input',type=pathlib.Path,required=True);p.add_argument('--identity',type=pathlib.Path,required=True);p.add_argument('--site',required=True);p.add_argument('--image',required=True);p.add_argument('--local-s3',action='store_true');p.add_argument('--gvisor',action='store_true');p.add_argument('--external-s3-target',type=pathlib.Path);p.add_argument('--team-tool-host-root',type=pathlib.Path)
    p=commands.add_parser('update');p.add_argument('--image',required=True);p.add_argument('--recipient',required=True)
    p=commands.add_parser('rollback');p.add_argument('--checkpoint',type=pathlib.Path,required=True);p.add_argument('--identity',type=pathlib.Path,required=True);p.add_argument('--restore-database',action='store_true',required=True);p.add_argument('--external-s3-target',type=pathlib.Path)
    p=commands.add_parser('delete');p.add_argument('--deployment-id',required=True);p.add_argument('--data-policy',choices=['retain','purge'],required=True);p.add_argument('--backup',type=pathlib.Path);p.add_argument('--backup-sha256')
    p=commands.add_parser('render-kubernetes');p.add_argument('--namespace',default='localai-cowork');p.add_argument('--output',type=pathlib.Path,required=True)
    p.add_argument('--ha',action='store_true');p.add_argument('--database-host');p.add_argument('--s3-endpoint');p.add_argument('--s3-bucket');p.add_argument('--s3-region',default='us-east-1');p.add_argument('--tool-image');p.add_argument('--docker-gid',type=int)
    args=parser.parse_args();root=args.root
    if args.command=='init':value=init(root,args.site,args.image,args.local_s3,args.gvisor);print(json.dumps({'root':str(root),'project':value['project']}));return
    if args.command=='restore':
        value=init(root,args.site,args.image,args.local_s3,args.gvisor);root,value=load(root)
        with decoded(root,args.input,args.identity) as (stage,manifest):
            require(bool(manifest['deployment']['s3'])==value['s3'],'Choose the backup matching local-S3 mode')
            target_configuration=None
            if manifest.get('external_s3_objects_included'):
                require(args.external_s3_target is not None,'External S3 restore requires an explicit empty target configuration')
                target_configuration=json.loads(args.external_s3_target.read_bytes())
                require(target_configuration.get('prefix','')==manifest['deployment']['external_s3'].get('prefix',''),'S3 object-key prefix must remain unchanged for database references')
                require(not ObjectBackup(target_configuration).inventory(),'External S3 restore target must be empty')
            restore_workspace(root,value,stage,manifest,args.team_tool_host_root or (root/'team-workspaces') if manifest.get('team_workspace') else None)
            install_files(root,stage);compose(root,value,'up','-d','postgres')
            if manifest.get('external_s3_objects_included'):
                ObjectBackup(target_configuration).restore(stage/'external-s3')
                configure_external_s3(root,value,target_configuration)
            # pg_isready makes readiness explicit before logical restoration.
            for _ in range(60):
                try:compose(root,value,'exec','-T','postgres','pg_isready','-U','cowork','-d','cowork');break
                except ValueError:time.sleep(2)
            else:raise ValueError('Owned PostgreSQL did not become ready')
            with (stage/'database.dump').open('rb') as source:compose(root,value,'exec','-T','postgres','pg_restore','--exit-on-error','-U','cowork','-d','cowork',input_file=source)
        compose(root,value,'up','-d');healthy(root,value);print(json.dumps({'state':'restored','root':str(root)}));return
    root,value=load(root)
    require(value.get('state')!='deleted' or args.command=='status','Deleted installations cannot be restarted; initialize a new owned deployment')
    if args.command=='configure-external-s3':
        require(args.configuration.is_absolute() and args.configuration.is_file() and not args.configuration.is_symlink() and args.configuration.stat().st_size<16384,'Select a private S3 configuration JSON file')
        require(not running(root,value),'Stop the owned deployment before changing storage')
        configure_external_s3(root,value,json.loads(args.configuration.read_bytes()));print(json.dumps({'state':'configured','external_s3':True}))
    elif args.command=='configure-team-tools':
        require(not running(root,value),'Stop the owned deployment before changing team-tool workspaces')
        require(args.host_root.is_absolute() and args.host_root.is_dir() and not args.host_root.is_symlink(),'Select an existing private absolute Docker-host workspace root')
        require(',' not in str(args.host_root),'Docker-host path cannot contain a comma')
        for parent in [args.host_root,*args.host_root.parents]:require(not parent.is_symlink(),'Docker-host workspace namespace contains a symlink')
        if os.name=='posix':
            metadata=args.host_root.stat();require(metadata.st_uid==1000 and metadata.st_mode&0o077==0,'Team-tool workspace root must be private and owned by UID1000')
        require(re.fullmatch(r'[A-Za-z0-9._:/-]+@sha256:[a-f0-9]{64}',args.image),'Use the exact immutable team-tool image digest')
        require(0<=args.docker_gid<=2147483647,'Invalid Docker socket GID')
        require(re.fullmatch(r'[A-Za-z0-9_.-]{1,128}',args.network) and args.network!='host','Select an operator guest network; host is refused')
        require(args.runtime is None or re.fullmatch(r'[A-Za-z0-9_.-]{1,128}',args.runtime),'Invalid guest runtime')
        host_id=value.get('team_tools',{}).get('host_id') or str(uuid.uuid4())
        value['team_tools']={'host_root':str(args.host_root.resolve()),'image':args.image,'docker_gid':args.docker_gid,'network':args.network,'runtime':args.runtime,'host_id':host_id}
        workspaces.owned(args.host_root.resolve(),value['id'],True)
        save_marker(root,value);print(json.dumps({'state':'configured','project':value['project']}))
    elif args.command=='build':compose(root,value,'build')
    elif args.command=='start':compose(root,value,'up','-d');healthy(root,value)
    elif args.command=='stop':compose(root,value,'stop','--timeout','45');workspace_quiescence(root,value)
    elif args.command=='status':print(compose(root,value,'ps','--format','json',capture=True).decode())
    elif args.command=='backup':print(json.dumps(backup(root,value,args.recipient)))
    elif args.command=='delete':
        require(args.deployment_id==value['id'],'Deletion requires the exact owned deployment UUID')
        if args.data_policy=='purge':
            require(args.backup is not None and args.backup.is_absolute() and args.backup.is_file() and not args.backup.is_symlink() and sha(args.backup)==args.backup_sha256,'Data purge requires an explicitly pinned retained encrypted backup')
            require(not args.backup.resolve().is_relative_to(root),'Retain the purge backup outside the deployment directory')
            receipt_path=args.backup.with_suffix(args.backup.suffix+'.json')
            require(receipt_path.is_file() and not receipt_path.is_symlink(),'Retain the matching backup ownership receipt')
            receipt=json.loads(receipt_path.read_bytes());require(receipt['deployment_id']==value['id'] and receipt['sha256']==args.backup_sha256,'Purge backup belongs to another deployment')
        compose(root,value,'down','--timeout','45')
        workspace_quiescence(root,value)
        require(not compose(root,value,'ps','-a','-q',capture=True).strip(),'Owned containers still exist; retaining all files')
        value['state']='deleted';value['data_policy']=args.data_policy;save_marker(root,value)
        if args.data_policy=='purge':
            for name in ['data','postgres','minio','secrets','caddy-data','caddy-config']:
                target=root/name;require(target.resolve().parent==root and target.is_dir() and not target.is_symlink(),'Refusing an unowned purge target')
                for item in target.rglob('*'):require(not item.is_symlink(),'Refusing a symlink in purge target')
                shutil.rmtree(target)
        print(json.dumps({'state':'deleted','data_policy':args.data_policy,'deployment_id':value['id'],'external_s3_deleted':False,'team_workspace_retained':bool(value.get('team_tools'))}))
    elif args.command=='render-kubernetes':
        require(re.fullmatch(r'[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?',args.namespace),'Invalid namespace')
        substitutions={'COWORK_NAMESPACE':args.namespace,'COWORK_SITE':value['site'],'COWORK_IMAGE':value['image']}
        if args.ha:
            require(args.database_host and re.fullmatch(r'[a-z0-9.-]{1,253}',args.database_host),'Provide the operator external HA database DNS hostname')
            endpoint=urllib.parse.urlsplit(args.s3_endpoint or '')
            require(endpoint.scheme=='https' and endpoint.hostname and endpoint.path in {'','/'} and not endpoint.username and not endpoint.password and not endpoint.query and not endpoint.fragment,'HA S3 must use an explicit HTTPS origin without credentials')
            require(re.fullmatch(r'[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]',args.s3_bucket or ''),'Provide the external S3 bucket')
            require(re.fullmatch(r'[a-z0-9-]{1,64}',args.s3_region),'Invalid external S3 region')
            require(args.tool_image and re.fullmatch(r'[A-Za-z0-9._:/-]+@sha256:[a-f0-9]{64}',args.tool_image),'Use the immutable team-tool image digest')
            require(args.docker_gid is not None and 0<=args.docker_gid<=2147483647,'Provide the selected worker Docker socket GID')
            substitutions.update({'COWORK_HA_DATABASE_HOST':args.database_host,'COWORK_HA_S3_ENDPOINT':args.s3_endpoint,
                'COWORK_HA_S3_BUCKET':args.s3_bucket,'COWORK_HA_S3_REGION':args.s3_region,
                'COWORK_TEAM_TOOL_IMAGE':args.tool_image,'COWORK_DOCKER_GID':str(args.docker_gid)})
        raw=(HERE/('kubernetes.ha.yaml' if args.ha else 'kubernetes.yaml')).read_text()
        for key,item in substitutions.items():
            require('"' not in item and '\n' not in item and '\r' not in item,'Invalid manifest field')
            raw=raw.replace('${'+key+'}',item)
        require('${' not in raw,'Unresolved operator manifest field')
        require(args.output.is_absolute(),'Select an absolute NEW manifest output');write_new(args.output,raw.encode())
    elif args.command=='update':
        previous=value['image'];receipt=backup(root,value,args.recipient,resume=False)
        checkpoint={'format':FORMAT+'/update','deployment_id':value['id'],'previous_image':previous,'image':image(args.image),'backup':receipt,'state':'prepared'}
        path=root/'backups'/('update-'+uuid.uuid4().hex+'.json');write_json(path,checkpoint)
        value['image']=image(args.image);save_marker(root,value)
        prerequisites=[name for name in receipt['stopped_services'] if name not in ['api','worker']]
        if prerequisites:compose(root,value,'start',*prerequisites)
        compose(root,value,'up','-d','--no-deps','api','worker');healthy(root,value)
        checkpoint['state']='updated';temporary=path.with_suffix('.new');write_json(temporary,checkpoint);os.replace(temporary,path)
        print(json.dumps({'checkpoint':str(path),'state':'updated'}))
    elif args.command=='rollback':
        require(args.checkpoint.is_absolute() and not args.checkpoint.is_symlink() and args.checkpoint.stat().st_size<16384,'Select an update checkpoint')
        checkpoint=json.loads(args.checkpoint.read_bytes());require(checkpoint['format']==FORMAT+'/update' and checkpoint['deployment_id']==value['id'],'Checkpoint belongs to another deployment')
        backup_path=pathlib.Path(checkpoint['backup']['file']);require(sha(backup_path)==checkpoint['backup']['sha256'],'Rollback backup changed')
        writers=stop_writers(root,value)
        workspace_quiescence(root,value)
        with decoded(root,backup_path,args.identity) as (stage,manifest):
            require(manifest['deployment']['id']==value['id'],'Rollback backup belongs to another deployment')
            restore_workspace(root,value,stage,manifest)
            target_configuration=None
            if manifest.get('external_s3_objects_included'):
                require(args.external_s3_target is not None,'S3 rollback requires a selected new empty target; existing objects are retained')
                target_configuration=json.loads(args.external_s3_target.read_bytes())
                require(target_configuration.get('prefix','')==manifest['deployment']['external_s3'].get('prefix',''),'S3 rollback object-key prefix must remain unchanged')
                require(not ObjectBackup(target_configuration).inventory(),'S3 rollback target must be empty')
                ObjectBackup(target_configuration).restore(stage/'external-s3')
            compose(root,value,'exec','-T','postgres','psql','-U','cowork','-d','cowork','-v','ON_ERROR_STOP=1','-c','DROP SCHEMA public CASCADE; CREATE SCHEMA public AUTHORIZATION cowork;')
            with (stage/'database.dump').open('rb') as source:compose(root,value,'exec','-T','postgres','pg_restore','--clean','--if-exists','--exit-on-error','-U','cowork','-d','cowork',input_file=source)
            install_files(root,stage)
            if target_configuration:configure_external_s3(root,value,target_configuration)
        value['image']=image(checkpoint['previous_image']);save_marker(root,value)
        # Recreate mounts after restoring renamed private data directories.
        compose(root,value,'up','-d','--force-recreate');healthy(root,value)
        print(json.dumps({'state':'rolled_back','restored_database':True,'retained_newer_files':True}))
if __name__=='__main__':
    try:main()
    except (ValueError,OSError,subprocess.SubprocessError,KeyError) as error:
        print('Operator lifecycle refused: '+str(error),file=__import__('sys').stderr);raise SystemExit(1)
