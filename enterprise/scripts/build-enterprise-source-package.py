#!/usr/bin/env python3
"""Immutable standalone Server source/build-context delivery, without deployment.

Consumes actual current compiler/DDL receipts, builds the real web frontend and
copies an explicit public source closure. No Docker command, publication, Git
mutation, private operator material or inferred image/runtime qualification.
"""
import argparse,base64,hashlib,json,os,re,shlex,stat,subprocess,time,tomllib,uuid,zipfile
from datetime import datetime,timezone
from pathlib import Path
from urllib.parse import quote
ROOT=Path(__file__).resolve().parents[1]
TREES=('agents','crates','server','contracts','deploy','scripts','docs','brand','app','clients','.github','.cargo')
TOP=('Cargo.toml','Cargo.lock','rust-toolchain.toml','LICENSE','NOTICE','README.md','SECURITY.md','SUPPORT.md','CONTRIBUTING.md','DISCLAIMER.md','.dockerignore','.gitignore','.gitattributes')
SKIP={'node_modules','target','dist','coverage','__pycache__','resources','wheels','.git','.local','.internal','.tmp','.cache','.vite','test-results','playwright-report','playwright-report-web','playwright-report-mobile','gen'}
SECRET_SUFFIX={'.pem','.key','.p12','.pfx','.crt','.cer','.der','.db','.sqlite','.sqlite3','.duckdb','.dump','.backup','.bak','.log','.tmp','.tsbuildinfo','.pyc','.pyo','.exe','.dll','.pdb','.msi','.deb','.rpm','.whl','.a','.so','.rlib','.rmeta'}
def require(ok,message):
    if not ok:raise RuntimeError(message)
def canonical(value):return json.dumps(value,sort_keys=True,ensure_ascii=False,separators=(',',':')).encode()
def sha(raw):return hashlib.sha256(raw).hexdigest()
def regular(path):
    require(path.is_file() and not path.is_symlink() and all(not p.is_symlink() for p in path.parents),'regular source/artifact required')
    if os.name=='nt':require(not path.stat().st_file_attributes&0x400,'source reparse points forbidden')
    return path
def excluded(relative):
    parts=relative.parts;name=parts[-1];lower=name.lower()
    if any(p in SKIP or p.startswith(('.tmp-','.local-','.internal-')) for p in parts):return True
    if lower.startswith('.env') and not lower.endswith(('.example','.sample')):return True
    if lower.endswith('.local') or Path(name).suffix.lower() in SECRET_SUFFIX:return True
    if lower.startswith(('credentials.','secrets.')):return True
    if parts[:2]==('deploy','secrets') and name not in ('README.md','.gitignore'):return True
    if '/crew_runtime/runtime-bundle-manifest.json' in relative.as_posix():return True
    if relative.as_posix()=='app/src-tauri/python/crew_runtime/wheels.zip':return True
    return False
def sources():
    paths={ROOT/name for name in TOP if (ROOT/name).exists()}
    for name in TREES:
        selected=ROOT/name
        for base,folders,files in os.walk(selected,followlinks=False):
            kept=[]
            for n in sorted(folders):
                if n in SKIP or n.startswith(('.tmp-','.local-','.internal-')):continue
                p=Path(base)/n;require(not p.is_symlink() and (os.name!='nt' or not p.stat().st_file_attributes&0x400),'public source directory reparse point forbidden');kept.append(n)
            folders[:]=kept
            for filename in sorted(files):
                path=Path(base)/filename;relative=path.relative_to(ROOT)
                if not excluded(relative):regular(path);require(path.stat().st_size<=33554432,'generated/oversized source excluded by review: '+relative.as_posix());paths.add(path)
    result={p.relative_to(ROOT).as_posix():sha(regular(p).read_bytes()) for p in sorted(paths)}
    require(result['server/cowork-server/migrations/0064_organisation_mfa.sql']=='23cc60bb9d83d997c7ca3d62df164a97bfe43beaaf38e69ddd0455338c72a386','original64 migration changed')
    return result
def public_mode_files():
    # Local mode switches contain only these reviewed public constants. They
    # are recorded as actual build inputs and never copied into the package.
    allowed={'.env.web':b'VITE_COWORK_WEB=true','.env.android':b'VITE_COWORK_ANDROID=true'};result={}
    for p in (ROOT/'app').glob('.env*'):
        if p.name.endswith(('.example','.sample')):continue
        raw=regular(p).read_bytes();require(p.name in allowed and raw.strip()==allowed[p.name],'private Vite environment file must not enter a public web build')
        result[p.name]=sha(raw)
    return result
def receipt(path,kind,pins):
    raw=regular(Path(path)).read_bytes();value=json.loads(raw);require(value.get('passed') is True and value.get('exit_code')==0,'passing original '+kind+' receipt required')
    require(value.get('source_unchanged') is True,'unchanged '+kind+' inputs required')
    recorded=value.get('source_pins');require(isinstance(recorded,dict) and recorded and all(p in pins and pins[p]==digest for p,digest in recorded.items()),'current '+kind+' source differs from original receipt')
    if kind=='DDL':require({p:d for p,d in pins.items() if p.startswith('server/cowork-server/migrations/') and p.endswith('.sql')}==recorded,'DDL receipt must cover every current migration exactly')
    return raw,value
def original_log(path,value):
    folder=regular(Path(path)).parent
    matches=[p for p in folder.glob('*.log') if p.is_file() and p.stat().st_size<=33554432 and sha(regular(p).read_bytes())==value.get('log_sha256')]
    require(len(matches)==1,'one exact original terminal log required')
    return regular(matches[0]).read_bytes()
def public_evidence(raw,kind,replacements,structured=True):
    original_sha=sha(raw)
    def text(value):
        for private,role in sorted(replacements,key=lambda item:len(item[0]),reverse=True):
            if private:value=value.replace(private,role).replace(private.replace('\\','/'),role).replace(private.replace('/','\\'),role)
        value=re.sub(r'/home/[^/\s\"\']+|/mnt/[a-z]/Users/[^/\s\"\']+', '__OPERATOR_HOME__',value)
        return re.sub(r'[A-Za-z]:[\\/]Users[\\/][^\\/\s\"\']+', '__OPERATOR_HOME__',value,flags=re.I)
    scope='Derived public copy: known original build-output, source-root, toolchain, cache and operator-home paths replaced with explicit role placeholders; facts and relative source hashes retained. Private original bytes remain unchanged.'
    if structured:
        def visit(value):
            if isinstance(value,str):return text(value)
            if isinstance(value,list):return [visit(v) for v in value]
            if isinstance(value,dict):return {text(k):visit(v) for k,v in value.items()}
            return value
        return {'format':1,'evidence_kind':'derived-public-copy','material':kind,'original_bytes_sha256':original_sha,'transformation_scope':scope,'value':visit(json.loads(raw))}
    return ('Derived public terminal copy\nOriginal bytes SHA256: '+original_sha+'\nTransformation: '+scope+'\n\n'+text(raw.decode('utf-8',errors='replace'))).encode()
def write(path,data):
    path.parent.mkdir(parents=True,exist_ok=True);require(not path.exists() and not path.is_symlink(),'immutable output already exists')
    with path.open('xb') as f:f.write(data if isinstance(data,bytes) else canonical(data));f.flush();os.fsync(f.fileno())
def docker_closure(pins):
    result=[]
    for name in sorted(p for p in pins if 'Dockerfile' in Path(p).name and not p.endswith('.dockerignore')):
        text=(ROOT/name).read_text(encoding='utf-8');logical=re.sub(r'\\\r?\n',' ',text)
        for line in logical.splitlines():
            if not line.strip().upper().startswith('COPY '):continue
            words=shlex.split(line.strip()[5:]);options=[]
            while words and words[0].startswith('--'):options.append(words.pop(0))
            if any(w.startswith('--from') for w in options):continue
            if line.strip()[5:].lstrip().startswith('['):words=json.loads(line.strip()[5:])
            for source in words[:-1]:
                require(not source.startswith('/') and '..' not in Path(source).parts,'Docker COPY escaped public context')
                # Most shipped files use the repository context; sandbox Docker
                # files also document context-local assets. Record that choice.
                candidates=[source,(Path(name).parent/source).as_posix()]
                match=next((c for c in candidates if c in pins or any(p.startswith(c.rstrip('/')+'/') for p in pins)),None)
                require(match is not None,'Docker COPY input absent from package: '+name+' '+source)
                result.append({'dockerfile':name,'source':source,'packaged_source':match})
    return result
def sbom(pins,created):
    components={};notices=[]
    def add(ecosystem,name,version,checksum=None,license=None,development=False):
        purl='pkg:'+ecosystem+'/'+quote(name,safe='/')+'@'+quote(version,safe='.+-');component={'type':'library','bom-ref':purl,'name':name,'version':version,'purl':purl,'scope':'optional' if development else 'required'}
        if checksum:component['hashes']=[checksum]
        if license:component['licenses']=[{'license':{'name':license}}]
        components[purl]=component;notices.append({'ecosystem':ecosystem,'name':name,'version':version,'license':license,'development':development})
    lock=json.loads((ROOT/'app/package-lock.json').read_bytes())
    for path,entry in lock['packages'].items():
        if not path or not entry.get('version'):continue
        name=entry.get('name') or path.rsplit('node_modules/',1)[-1];checksum=None;integrity=entry.get('integrity','')
        if integrity.startswith('sha512-'):checksum={'alg':'SHA-512','content':base64.b64decode(integrity[7:]).hex()}
        add('npm',name,entry['version'],checksum,entry.get('license'),entry.get('dev',False))
    cargo=tomllib.loads((ROOT/'Cargo.lock').read_text(encoding='utf-8'))
    for entry in cargo['package']:
        checksum={'alg':'SHA-256','content':entry['checksum']} if entry.get('checksum') else None;add('cargo',entry['name'],entry['version'],checksum)
    version=tomllib.loads((ROOT/'Cargo.toml').read_text(encoding='utf-8'))['workspace']['package']['version'];material=sha(canonical(pins));identity=uuid.UUID(material[:32])
    value={'bomFormat':'CycloneDX','specVersion':'1.6','serialNumber':'urn:uuid:'+str(identity),'version':1,'metadata':{'timestamp':created,'component':{'type':'application','name':'open-cowork-enterprise-source','version':version},'properties':[{'name':'cowork:scope','value':'locked npm/Cargo source dependency inventory; OCI base/layer contents not built or attested'},{'name':'cowork:source-sha256','value':material}]},'components':sorted(components.values(),key=lambda c:c['bom-ref'])}
    return value,{'format':1,'scope':'source lock inventory; null licenses require operator review, not assumed approval','packages':notices}
def main():
    parser=argparse.ArgumentParser();parser.add_argument('--output',required=True);parser.add_argument('--native-receipt');parser.add_argument('--ddl-receipt');parser.add_argument('--python-receipt');parser.add_argument('--inspect-only',action='store_true');args=parser.parse_args()
    pins=sources();copies=docker_closure(pins)
    if args.inspect_only:print(json.dumps({'source_files':len(pins),'source_sha256':sha(canonical(pins)),'docker_copy_inputs':len(copies),'private_material_included':False}));return
    require(args.native_receipt and args.ddl_receipt,'actual current native and DDL receipts required')
    native,n=receipt(args.native_receipt,'native',pins);ddl,d=receipt(args.ddl_receipt,'DDL',pins)
    native_log=original_log(args.native_receipt,n);ddl_log=original_log(args.ddl_receipt,d)
    python_raw,python_value=receipt(args.python_receipt,'Python',pins) if args.python_receipt else (None,None)
    output=Path(args.output);require(output.is_absolute() and not output.exists() and all(not p.is_symlink() for p in output.parents),'new absolute immutable output directory required');output.mkdir(mode=0o700,parents=True)
    created=datetime.now(timezone.utc).isoformat();write(output/'OWNED-OUTPUT.json',{'format':1,'created_at':created,'source_sha256':sha(canonical(pins))})
    mode_pins=public_mode_files()
    env=dict(os.environ)
    for key in list(env):
        if key.startswith('VITE_'):env.pop(key)
    destination=output/'web-dist';require(not destination.exists() and destination.resolve().parent==output.resolve(),'owned build output changed')
    command=['npm.cmd' if os.name=='nt' else 'npm','run','build:web','--','--outDir',str(destination)];started=time.monotonic()
    with (output/'frontend-build.log').open('xb') as log:build=subprocess.run(command,cwd=ROOT/'app',env=env,stdout=log,stderr=subprocess.STDOUT)
    unchanged=sources()==pins and public_mode_files()==mode_pins;files={p.relative_to(output).as_posix():sha(regular(p).read_bytes()) for p in sorted(destination.rglob('*')) if p.is_file()} if destination.exists() else {}
    result={'format':1,'passed':build.returncode==0 and unchanged and bool(files),'exit_code':build.returncode,'source_unchanged':unchanged,'command':command,'node_version':subprocess.check_output(['node','--version'],text=True).strip(),'npm_version':subprocess.check_output(['npm.cmd' if os.name=='nt' else 'npm','--version'],text=True).strip(),'seconds':round(time.monotonic()-started,3),'source_pins':pins,'public_mode_file_pins':mode_pins,'dist_files':files,'log_sha256':sha((output/'frontend-build.log').read_bytes()),'built':build.returncode==0,'installed':False,'published':False}
    write(output/'frontend-build-receipt.json',result);require(result['passed'],'actual web build/current source check failed; retain original failure output')
    index=regular(destination/'index.html').read_text(encoding='utf-8')
    for reference in re.findall(r'(?:src|href)=["\']([^"\']+)["\']',index):
        if reference.startswith(('https://','http://','data:','#')):continue
        name=reference.split('?',1)[0].split('#',1)[0].lstrip('/');require('..' not in Path(name).parts and (destination/name).is_file(),'built HTML references an absent/dist-escaping asset')
    package=output/'open-cowork-enterprise';context=package/'source';context.mkdir(parents=True)
    for name,digest in pins.items():
        raw=regular(ROOT/name).read_bytes();require(sha(raw)==digest,'source changed during package copy');write(context/name,raw)
    for p in sorted(destination.rglob('*')):
        if p.is_file():write(package/'web-dist'/p.relative_to(destination),regular(p).read_bytes())
    replacements=[(str(output),'__DELIVERY_OUTPUT__'),(os.path.relpath(output,ROOT/'app'),'__DELIVERY_OUTPUT__'),
        (str(ROOT),'__SOURCE_ROOT__'),('/mnt/c/'+str(ROOT)[3:].replace('\\','/'),'__SOURCE_ROOT__') if os.name=='nt' else ('',''),
        (str(n.get('target_directory','')),'__CARGO_TARGET__'),(str(n.get('rustc','')),'__TOOLCHAIN__/rustc'),(str(n.get('rustdoc','')),'__TOOLCHAIN__/rustdoc'),
        (str(n['command'][0]),'__TOOLCHAIN__/cargo'),(str(Path.home()),'__OPERATOR_HOME__')]
    write(package/'evidence'/'native-check.json',public_evidence(native,'native compiler receipt',replacements))
    write(package/'evidence'/'ddl-check.json',public_evidence(ddl,'fresh DDL receipt',replacements))
    write(package/'evidence'/'frontend-build.json',public_evidence(canonical(result),'web build receipt',replacements))
    write(package/'evidence'/'native-terminal.log',public_evidence(native_log,'native terminal',replacements,False))
    write(package/'evidence'/'ddl-terminal.log',public_evidence(ddl_log,'DDL terminal',replacements,False))
    write(package/'evidence'/'frontend-terminal.log',public_evidence((output/'frontend-build.log').read_bytes(),'web terminal',replacements,False))
    if python_raw is not None:write(package/'evidence'/'python-syntax.json',public_evidence(python_raw,'Python syntax receipt',replacements))
    bom,licenses=sbom(pins,created);write(package/'SBOM.cdx.json',bom);write(package/'THIRD_PARTY_NOTICES.json',licenses)
    write(package/'SOURCE-MANIFEST.json',{'format':1,'source_sha256':sha(canonical(pins)),'sources':pins,'docker_copy_closure':copies})
    write(package/'BUILD-SCOPE.json',{'format':1,'source_sha256':sha(canonical(pins)),'native_compile_scope':n.get('compiler'),'native_original_receipt_sha256':sha(native),'ddl_original_receipt_sha256':sha(ddl),'migrations':d['migrations'],'web_original_build_receipt_sha256':sha(canonical(result)),'python_original_receipt_sha256':sha(python_raw) if python_raw else None,'public_evidence_kind':'explicitly transformed derived copies with original byte hashes; private originals are not shipped','linux_compile_checked':False,'unix_disk_quota_branch_compiled':False,'docker_images_built':False,'installed':False,'runtime_qualified':False,'production_qualified':False,'published':False})
    write(package/'INSTALL.txt',('Open_Cowork Enterprise source delivery\n\nBuild/Compose installation runs only on your own reviewed server. Read source/deploy/enterprise/README.md and source/docs/SERVER_IMPLEMENTATION_STATUS.md first.\n\nFrom source/:\n  docker build -f server/Dockerfile -t YOUR_LOCAL_SERVER_TAG .\n  docker build -f server/cowork-runner/Dockerfile -t YOUR_LOCAL_RUNNER_TAG .\n  docker build -f server/cowork-provisioner/Dockerfile -t YOUR_LOCAL_PROVISIONER_TAG .\n  docker build -f app/Dockerfile.web -t YOUR_LOCAL_WEB_TAG .\n\nSupply actual built immutable image digests, private keys/credentials, reviewed physical capacity, DNS/TLS, PostgreSQL/S3, original VM/Kubernetes identities and current operator configuration. Use prepare-enterprise-gateway.py and the documented original lifecycle producers. The included web-dist is the genuine recorded current build and can be served with source/deploy/web/nginx.conf. No credentials or live operator configs are included.\n\nRecorded native check scope: '+str(n.get('compiler','see original native receipt'))+'. This producer did not run a Linux compile or build/load Docker images. The Unix-only disk-quota statvfs branch is not checked by a Windows/MSVC compile; the Linux Docker build remains an operator prerequisite. Native check, source DDL and frontend build do not qualify a physical deployment. No installation/publication occurred.\n').encode())
    package_files={p.relative_to(package).as_posix():sha(regular(p).read_bytes()) for p in sorted(package.rglob('*')) if p.is_file()};write(package/'SHA256SUMS',''.join(digest+'  '+name+'\n' for name,digest in package_files.items()).encode())
    archive=output/'open-cowork-enterprise-source.zip'
    with zipfile.ZipFile(archive,'x',compression=zipfile.ZIP_DEFLATED,compresslevel=6) as zip:
        for p in sorted(package.rglob('*')):
            if p.is_file():
                raw=regular(p).read_bytes();info=zipfile.ZipInfo.from_file(p,'open-cowork-enterprise/'+p.relative_to(package).as_posix());info.create_system=3
                info.external_attr=(stat.S_IFREG|(0o555 if raw.startswith(b'#!') else 0o444))<<16;info.compress_type=zipfile.ZIP_DEFLATED
                zip.writestr(info,raw,compress_type=zipfile.ZIP_DEFLATED,compresslevel=6)
    require(sources()==pins,'source changed before final package receipt');require(all(not excluded(Path(name)) for name in pins),'private source path entered package')
    final={'format':'cowork-enterprise-current-delivery-v1','created_at':created,'source_sha256':sha(canonical(pins)),'source_files':len(pins),'archive':archive.name,'archive_sha256':sha(archive.read_bytes()),'archive_bytes':archive.stat().st_size,'package_files':package_files,'frontend_build_receipt_sha256':sha((output/'frontend-build-receipt.json').read_bytes()),'native_receipt_sha256':sha(native),'native_compile_scope':n.get('compiler'),'ddl_receipt_sha256':sha(ddl),'python_receipt_sha256':sha(python_raw) if python_raw else None,'migrations':d['migrations'],'linux_compile_checked':False,'unix_disk_quota_branch_compiled':False,'docker_images_built':False,'installed':False,'runtime_qualified':False,'production_qualified':False,'published':False}
    write(output/'delivery.json',final);write(output/'SHA256SUMS',(final['archive_sha256']+'  '+archive.name+'\n'+sha((output/'delivery.json').read_bytes())+'  delivery.json\n').encode())
    for p in output.rglob('*'):
        if p.is_file():os.chmod(p,0o555 if p.read_bytes().startswith(b'#!') else 0o444)
    print(json.dumps({k:v for k,v in final.items() if k!='package_files'}))
if __name__=='__main__':main()
