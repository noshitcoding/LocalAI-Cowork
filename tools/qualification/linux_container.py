"""Build an isolated source snapshot and run the Linux core acceptance suite.

No daemon data, credentials, repository history or host directories are mounted.
The container/image are retained for inspection; this script does not prune Docker.
"""
import argparse
import hashlib
import json
import pathlib
import shutil
import subprocess
import uuid

root = pathlib.Path(__file__).resolve().parents[2]
parser = argparse.ArgumentParser()
parser.add_argument('--suite', choices=['core', 'executor', 'environments'], default='core')
args = parser.parse_args()
run_id = uuid.uuid4().hex
destination = root / '.local/linux-qualification' / run_id
context = destination / 'context'
context.mkdir(parents=True)
allowed = {'.rs', '.toml', '.lock', '.sql', '.py', '.mjs', '.json'}
sources = [root/name for name in ['Cargo.toml','Cargo.lock','rust-toolchain.toml']]
for directory in ['crates','services','tools/contracts','tools/qualification','tests/system']:
    for path in (root/directory).rglob('*'):
        if {'target','__pycache__','node_modules'} & set(path.relative_to(root).parts):
            continue
        if path.is_file() and (path.suffix in allowed or path.name == 'Dockerfile'):
            assert not path.is_symlink(), f'Source symlink requires explicit review: {path}'
            sources.append(path)
manifest = {}
for source in sorted(set(sources)):
    relative = source.relative_to(root)
    target = context/relative
    target.parent.mkdir(parents=True,exist_ok=True)
    shutil.copyfile(source,target)
    manifest[relative.as_posix()] = hashlib.sha256(target.read_bytes()).hexdigest()
(context/'source-manifest.json').write_text(json.dumps(manifest,indent=2)+'\n',encoding='utf-8')
image = 'localai-cowork-v2-qualification:'+run_id
container = 'localai-cowork-v2-qualification-'+run_id
label = 'localai.cowork.scope=v2-linux-core-qualification'
def run(arguments, **kwargs):
    return subprocess.run(arguments, check=True, **kwargs)
metadata = {'run_id':run_id,'image':image,'container':container,'suite':args.suite,'source_files':len(manifest),'limits':{'cpus':4,'memory':'6g','pids':512},'network':'none','source_manifest_sha256':hashlib.sha256((context/'source-manifest.json').read_bytes()).hexdigest()}
(destination/'host.json').write_text(json.dumps(metadata,indent=2)+'\n',encoding='utf-8')
print(json.dumps(metadata),flush=True)
run(['docker','build','--label',label,'-t',image,'-f',str(context/'tools/qualification/Dockerfile'),str(context)])
metadata['image_inspect'] = json.loads(subprocess.check_output(['docker','image','inspect',image],text=True))[0]['Id']
result = subprocess.run(['docker','run','--name',container,'--label',label,'--network','none','--cap-drop','ALL','--security-opt','no-new-privileges:true','--cpus','4','--memory','6g','--pids-limit','512',image,'python3','tools/qualification/run_linux.py','--suite',args.suite])
metadata['exit_code'] = result.returncode
run(['docker','cp',container+':/home/qualification/evidence',str(destination/'evidence')])
(destination/'host.json').write_text(json.dumps(metadata,indent=2)+'\n',encoding='utf-8')
print('Evidence: '+str(destination),flush=True)
raise SystemExit(result.returncode)
