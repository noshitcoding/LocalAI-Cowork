"""Verify an original local unsigned bundle; never execute packages or call GitHub."""
from pathlib import Path
import argparse,os,subprocess
import verify_review as verify
import local_origin
def main():
    p=argparse.ArgumentParser(description=__doc__)
    for name in ('operator-selection','source-root','output'):p.add_argument('--'+name,type=Path,required=True)
    p.add_argument('--expected-origin-sha256',required=True)
    a=p.parse_args()
    root=a.source_root.absolute()
    selected=verify.read_json(a.operator_selection.absolute())
    verify.require(verify.pin(a.operator_selection.absolute())['sha256']==a.expected_origin_sha256,'Independent operator selection digest mismatch')
    env=dict(os.environ);env['GIT_OPTIONAL_LOCKS']='0'
    def git(args):
        return subprocess.check_output(['git','--no-optional-locks','-c','core.fsmonitor=false','-c','core.untrackedCache=false',
            '-C',str(root),*args],env=env,text=True,encoding='utf-8').strip()
    def source():
        verify.require(git(['rev-parse','HEAD'])==selected['source_commit']
            and git(['rev-parse','HEAD^{tree}'])==selected['source_tree']
            and not git(['status','--porcelain','--untracked-files=no']),'Exact unchanged source checkout required')
    source()
    context=local_origin.checked_selection(a.operator_selection.absolute(),a.expected_origin_sha256,
        selected['source_commit'],selected['source_tree'],selected['platform'])
    selection={k:selected[k] for k in ('source_commit','source_tree','platform')}
    selection['archive_sha256']=context['archive']['sha256']
    archive=Path(selected['archive']['path'])
    output=a.output.absolute()
    verify.require(not output.exists() and output.parent.is_dir(),'New evidence output directory required')
    verify.require(root!=output and root not in output.parents,'Keep output outside selected source')
    output.mkdir(mode=0o700)
    verify.safe_extract(archive,output/'original',selection['archive_sha256'])
    record=verify.checked_local_tree(output/'original',root,selection,context)
    source()
    verify.require(verify.pin(archive)==context['archive']
        and verify.pin(a.operator_selection.absolute())['sha256']==a.expected_origin_sha256,'Original local origin/archive drift')
    record['archive']=context['archive']
    record['operator_selection']={k:selected[k] for k in ('source_commit','source_tree','platform')}
    import json
    with (output/'input-binding.json').open('x',encoding='utf-8',newline='\n') as f:json.dump(record,f,indent=2,sort_keys=True);f.write('\n')
    print(json.dumps({'input_binding':str(output/'input-binding.json'),'pin':verify.pin(output/'input-binding.json'),
        'installer_execution_performed':False,'signed':False,'published':False,'release_qualified':False}))
if __name__=='__main__':main()

