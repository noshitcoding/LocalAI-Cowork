"""GitHub PR workbench: reviewed writes bind the exact server head SHA."""
import json,pathlib,re,subprocess,sys,tempfile

def execute(gh,action):
    repository=action['repository'];number=action['number'];operation=action['operation'];expected=action.get('expected_head')
    assert re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9-]{0,38}/[A-Za-z0-9_.-]{1,100}',repository) and type(number) is int and 0<number<1000000000,'Invalid pull request identity'
    def api(endpoint,method='GET',body=None,accept=None):
        command=[gh,'api',endpoint,'--method',method]
        if body is not None:command+=['--input','-']
        if accept:command+=['--header','Accept: '+accept]
        with tempfile.TemporaryFile() as out,tempfile.TemporaryFile() as err:
            child=subprocess.Popen(command,stdin=subprocess.PIPE if body is not None else subprocess.DEVNULL,stdout=out,stderr=err)
            child.communicate(input=json.dumps(body).encode() if body is not None else None,timeout=75)
            err.seek(0);error=err.read(4096).decode('utf-8','replace');assert child.returncode==0,'GitHub refused the pinned request: '+error
            assert out.tell()<=4*1024*1024,'GitHub response exceeds workbench limit';out.seek(0);raw=out.read()
        return raw.decode('utf-8','strict') if accept else json.loads(raw)
    endpoint=f'repos/{repository}/pulls/{number}';pull=api(endpoint);head=pull['head']['sha'];base=pull['base']['sha']
    assert re.fullmatch(r'[0-9a-f]{40}|[0-9a-f]{64}',head) and re.fullmatch(r'[0-9a-f]{40}|[0-9a-f]{64}',base),'Invalid server commit identity'
    if operation!='inspect':assert expected==head,'Pull request head changed since review; inspect again'
    result={'repository':repository,'number':number,'head':head,'base':base,'title':pull['title'],'state':pull['state'],'url':pull['html_url']}
    if operation=='inspect':result['pull']=pull
    elif operation=='diff':result['diff']=api(f'repos/{repository}/compare/{base}...{head}',accept='application/vnd.github.v3.diff')
    elif operation=='reviews':result['reviews']=api(endpoint+'/reviews?per_page=100')
    elif operation=='review':
        event=action.get('event');body=action.get('body') or '';assert event in {'APPROVE','REQUEST_CHANGES','COMMENT'} and isinstance(body,str) and len(body)<=40000 and (event=='APPROVE' or body.strip()),'Choose a review event and description'
        result['review']=api(endpoint+'/reviews','POST',{'commit_id':head,'event':event,'body':body})
    elif operation=='merge':
        method=action.get('merge_method');assert method in {'merge','squash','rebase'},'Choose a merge method'
        assert pull['state']=='open' and not pull.get('draft'),'Only an open ready pull request can be merged'
        result['merge']=api(endpoint+'/merge','PUT',{'sha':head,'merge_method':method})
        assert result['merge'].get('merged') is True,'GitHub did not merge this exact head'
    else:raise ValueError('Unsupported pull request operation')
    return result

if __name__=='__main__':
    gh,encoded=sys.argv[1:3];action=json.loads(pathlib.Path(encoded[1:]).read_text(encoding='utf-8') if encoded.startswith('@') else encoded)
    print(json.dumps(execute(gh,action),ensure_ascii=False))
