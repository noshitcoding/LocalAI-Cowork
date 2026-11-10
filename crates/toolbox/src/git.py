"""Structured Git operations; each mutation is bound to a reviewed repository state.

No shell, hooks, external diff/merge drivers, SSH commands or credential prompts.
All paths live inside the explicitly managed chat workspace.
"""
import hashlib,json,os,pathlib,re,stat,subprocess,sys,urllib.parse

def digest(value):return hashlib.sha256(value).hexdigest()
def safe_path(root,value,existing=False):
    assert isinstance(value,str) and len(value)<=4096 and not any(c in value for c in '\0\r\n:'),'Invalid repository path'
    relative=pathlib.Path(value);assert not relative.is_absolute() and '..' not in relative.parts,'Expected managed relative path'
    selected=root/relative
    for candidate in [selected,*selected.parents]:
        if candidate==root.parent:break
        if candidate.exists():assert not candidate.is_symlink() and not getattr(candidate.lstat(),'st_file_attributes',0)&0x400,'Repository path contains an alias'
    resolved=selected.resolve(strict=existing);assert resolved.is_relative_to(root),'Repository path escapes workspace'
    return resolved

class Repository:
    def __init__(self,root,git,cwd):
        self.root=pathlib.Path(root).resolve(strict=True);self.path=safe_path(self.root,cwd);self.git=str(pathlib.Path(git).resolve(strict=True))
        self.environment={key:value for key,value in os.environ.items() if not key.startswith('GIT_')}
        self.environment.update(GIT_CONFIG_NOSYSTEM='1',GIT_CONFIG_GLOBAL=os.devnull,GIT_TERMINAL_PROMPT='0',GIT_OPTIONAL_LOCKS='0',GIT_LFS_SKIP_SMUDGE='1')
    def call(self,*args,input=None,allow=False,env=None):
        command=[self.git,'--no-pager','-c','core.fsmonitor=false','-c','core.hooksPath='+os.devnull,'-c','credential.helper=','-c','core.askPass=','-c','diff.external=',*args]
        result=subprocess.run(command,cwd=self.path if self.path.exists() else self.root,input=input,stdout=subprocess.PIPE,stderr=subprocess.PIPE,env={**self.environment,**(env or {})},timeout=90)
        assert len(result.stdout)<=4*1024*1024 and len(result.stderr)<=256*1024,'Git output exceeds limit'
        if not allow and result.returncode:raise ValueError('Git operation refused: '+result.stderr.decode('utf-8','replace')[:4096])
        return result
    def validate(self):
        assert self.path.is_dir() and (self.path/'.git').is_dir(),'Choose a managed ordinary Git repository'
        safe_path(self.root,str((self.path/'.git').relative_to(self.root)),True)
        config=self.call('config','--local','--no-includes','--null','--list').stdout.decode('utf-8','strict')
        for entry in config.split('\0'):
            key=entry.split('\n',1)[0].lower()
            assert not (key.startswith(('include.','includeif.','filter.','credential.','url.')) or key in {'core.worktree','core.sshcommand'} or re.match(r'(diff\..*\.command|merge\..*\.driver|remote\..*\.(uploadpack|receivepack|proxy)|http\..*extraheader)',key)),'Repository contains an external command or redirected configuration'
        reported=pathlib.Path(self.call('rev-parse','--show-toplevel').stdout.decode().strip()).resolve(strict=True)
        assert reported.samefile(self.path),'Repository root does not match selection'
    def state(self):
        if not self.path.exists():return {'exists':False,'binding':digest(('absent:'+str(self.path.relative_to(self.root))).encode()),'entries':[],'branches':[],'head':None,'branch':None}
        if not (self.path/'.git').exists():
            files=[];total=0
            for item in sorted(self.path.rglob('*')):
                if item.is_dir():continue
                safe_path(self.root,str(item.relative_to(self.root)),True);assert item.is_file() and item.stat().st_nlink==1,'Unsupported repository input'
                total+=item.stat().st_size;assert len(files)<10000 and total<=128*1024*1024,'Repository input exceeds review limit'
                files.append([str(item.relative_to(self.path)),digest(item.read_bytes())])
            return {'exists':False,'binding':digest(json.dumps(files,separators=(',',':')).encode()),'entries':[],'branches':[],'head':None,'branch':None}
        self.validate();head=self.call('rev-parse','--verify','HEAD',allow=True).stdout.decode().strip() or None;branch=self.call('symbolic-ref','--quiet','--short','HEAD',allow=True).stdout.decode().strip() or None
        refs=self.call('for-each-ref','--format=%(refname) %(objectname)').stdout.decode('utf-8','strict').splitlines();raw=self.call('status','--porcelain=v1','-z','--untracked-files=all').stdout
        cached_modes={}
        for row in self.call('ls-files','--stage','-z').stdout.split(b'\0'):
            if row:
                metadata,name=row.decode('utf-8','strict').split('\t',1);mode,_,stage=metadata.split(' ')
                if stage=='0':cached_modes[name]=mode
        entries=[];parts=raw.split(b'\0');index=0;total=0
        while index<len(parts) and parts[index]:
            part=parts[index].decode('utf-8','strict');index+=1;status,path=part[:2],part[3:];entry={'status':status,'path':path}
            if 'R' in status or 'C' in status:entry['original_path']=parts[index].decode('utf-8','strict');index+=1
            target=safe_path(self.path,path);assert target.is_relative_to(self.root)
            if target.exists():
                assert target.is_file() and target.stat().st_nlink==1,'Only regular files can be reviewed'
                size=target.stat().st_size;total+=size;assert size<=32*1024*1024 and total<=128*1024*1024,'Changed files exceed review bound'
                mode=cached_modes.get(path,'100755' if os.name!='nt' and target.stat().st_mode&stat.S_IXUSR else '100644');assert mode in {'100644','100755'},'Only regular file index modes can be reviewed'
                entry.update(sha256=digest(target.read_bytes()),size=size,mode=mode)
            else:entry.update(sha256=None,size=0)
            entries.append(entry);assert len(entries)<=10000,'Too many changed files'
        index_path=self.path/'.git/index';index_hash=digest(index_path.read_bytes()) if index_path.exists() else None
        basis={'head':head,'branch':branch,'refs':refs,'index':index_hash,'entries':entries,'config':digest((self.path/'.git/config').read_bytes())}
        return {'exists':True,'binding':digest(json.dumps(basis,sort_keys=True,separators=(',',':')).encode()),'entries':entries,'branches':self.call('branch','--format=%(refname:short)').stdout.decode().splitlines(),'head':head,'branch':branch,'conflicts':bool(self.call('ls-files','-u').stdout)}
    def reference(self,value):
        assert isinstance(value,str) and len(value)<=200 and not value.startswith('-') and not any(c in value for c in '\0\r\n'),'Invalid branch reference'
        self.call('check-ref-format','--branch',value);return value
    def remote(self,value):
        assert isinstance(value,str) and len(value)<=4096,'Invalid remote'
        parsed=urllib.parse.urlsplit(value)
        if parsed.scheme=='https':assert parsed.hostname and not parsed.username and not parsed.password and not parsed.fragment and not parsed.query,'Use an HTTPS remote without embedded credentials or query'
        else:
            candidate=pathlib.Path(value)
            if candidate.is_absolute():assert candidate.resolve(strict=True).is_relative_to(self.root),'Local remote must stay in this workspace'
            else:safe_path(self.root,value,True)
        return value
    def execute(self,action):
        operation=action['operation'];before=self.state();paths=action.get('paths') or [];assert len(paths)<=256 and len(set(paths))==len(paths),'Invalid paths'
        for path in paths:assert path and not path.startswith('-');safe_path(self.path,path)
        reads={'status','diff','log','branches'}
        if operation not in reads:assert action.get('expected_state')==before['binding'],'Repository changed since review; refresh before applying'
        text='';reference=action.get('reference')
        if operation=='init':
            assert not before['exists'];self.path.mkdir(parents=True,exist_ok=True);self.call('init','--initial-branch=main')
        elif operation=='clone':
            assert not self.path.exists(),'Clone destination already exists';remote=self.remote(reference)
            self.call('clone','--no-recurse-submodules','--no-hardlinks','--',remote,str(self.path))
        elif operation=='status' and not before['exists']:pass
        elif not before['exists']:raise ValueError('Initialize or clone a repository first')
        elif operation in {'status','branches'}:pass
        elif operation=='diff':text=self.call('diff','--no-ext-diff','--no-textconv',*(['--cached'] if reference=='staged' else []),'--',*paths).stdout.decode('utf-8','replace')
        elif operation=='log':text=self.call('log','-50','--format=%H %ad %an%n%s','--date=iso-strict').stdout.decode('utf-8','replace') if before['head'] else ''
        elif operation=='stage':
            assert paths,'Select explicit files';changed={entry['path']:entry for entry in before['entries']}
            for path in paths:
                assert path in changed,'Selected file is not in the reviewed status';entry=changed[path]
                if entry['sha256'] is None:self.call('update-index','--force-remove','--',path)
                else:
                    content=safe_path(self.path,path,True).read_bytes();assert digest(content)==entry['sha256'],'File changed after review'
                    blob=self.call('hash-object','-w','--stdin',input=content).stdout.decode().strip()
                    self.call('update-index','--add','--cacheinfo',entry['mode'],blob,path)
        elif operation=='unstage':
            assert paths,'Select explicit files';self.call(*(['reset','--quiet','HEAD','--',*paths] if before['head'] else ['rm','--cached','--',*paths]))
        elif operation=='commit':
            assert before['head'] is None or not before.get('conflicts'),'Resolve conflicts first'
            payload=json.loads(action.get('text') or '{}');assert set(payload)<={'message','author_name','author_email'}
            message=payload.get('message');name=payload.get('author_name');email=payload.get('author_email')
            assert all(isinstance(v,str) and v.strip() for v in [message,name,email]) and len(message)<=40000 and len(name)<=200 and len(email)<=300 and not any(c in name+email for c in '\0\r\n'),'Provide a commit message and explicit author identity'
            self.call('commit','--no-verify','--no-gpg-sign','--file=-',input=message.encode(),env={'GIT_AUTHOR_NAME':name,'GIT_COMMITTER_NAME':name,'GIT_AUTHOR_EMAIL':email,'GIT_COMMITTER_EMAIL':email})
        elif operation=='create_branch':self.call('branch','--',self.reference(reference))
        elif operation=='switch_branch':
            assert not before['entries'],'Commit or unstage/resolve local changes before switching';self.call('switch','--no-guess',self.reference(reference))
        elif operation=='delete_branch':self.call('branch','-d','--',self.reference(reference))
        elif operation in {'fetch','push'}:
            remote=self.call('remote','get-url','origin').stdout.decode().strip();self.remote(remote)
            environment={};token=os.environ.get('COWORK_GIT_TOKEN')
            if token and urllib.parse.urlsplit(remote).hostname=='github.com':environment={'GIT_CONFIG_COUNT':'1','GIT_CONFIG_KEY_0':'http.https://github.com/.extraHeader','GIT_CONFIG_VALUE_0':'Authorization: Bearer '+token}
            if operation=='fetch':self.call('fetch','--no-recurse-submodules','origin',env=environment)
            else:self.call('push','--porcelain','origin','HEAD:refs/heads/'+self.reference(reference or before['branch']),env=environment)
        else:raise ValueError('Unsupported Git operation')
        return {'operation':operation,'repository':self.path.relative_to(self.root).as_posix(),'state':self.state(),'text':text}

if __name__=='__main__':
    root,git,encoded=sys.argv[1:4];action=json.loads(pathlib.Path(encoded[1:]).read_text(encoding='utf-8') if encoded.startswith('@') else encoded)
    print(json.dumps(Repository(root,git,action['cwd']).execute(action),ensure_ascii=False))
