"""Central resource acceptance inside the caller's validated disposable fleet.

Real management/instance APIs, workers, model proxy and sandbox MCP process are
used. The model and MCP server are controlled local protocol fixtures.
"""
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import threading
import uuid


def verify_managed_resources(request,wait,passed,management,base,admin,owner,item,tenant_sql):
    resources=[]
    requests=[]
    release=threading.Event()
    entered=threading.Event()
    mode={'value':'success'}
    mcp_id=None
    marker='CENTRAL_MCP_EFFECT_OK'
    role_marker='CENTRAL_ROLE_CONTEXT_OK'
    skill_marker='CENTRAL_SKILL_CONTEXT_OK'

    class Model(BaseHTTPRequestHandler):
        def log_message(self,*_): pass
        def do_POST(self):
            body=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            requests.append(body)
            if mode['value']=='revoke':
                entered.set()
                if not release.wait(90):
                    self.send_error(504)
                    return
            outputs=[m for m in body.get('messages',[]) if m.get('role')=='tool']
            if outputs:
                message={'role':'assistant','content':'CENTRAL_RESOURCES_OK'}
                finish='stop'
            else:
                message={'role':'assistant','content':None,'tool_calls':[{'id':'central-mcp','type':'function','function':{
                    'name':'MCPTool','arguments':json.dumps({'server_name':'managed_'+uuid.UUID(mcp_id).hex,
                        'tool_name':'effect','arguments':{'text':marker}})}}]}
                finish='tool_calls'
            result=json.dumps({'choices':[{'index':0,'message':message,'finish_reason':finish}],
                'usage':{'prompt_tokens':1,'completion_tokens':1,'total_tokens':2}}).encode()
            self.send_response(200)
            self.send_header('Content-Type','application/json')
            self.send_header('Content-Length',str(len(result)))
            self.end_headers()
            self.wfile.write(result)

    model=ThreadingHTTPServer(('0.0.0.0',9002),Model)
    threading.Thread(target=model.serve_forever,daemon=True).start()
    def catalog(kind,name,configuration,**extra):
        resource=request(management,'/management/v1/resource-catalog',admin['access_token'],
            {'kind':kind,'name':name,'configuration':configuration,'secret_reference':None,'enabled':True,**extra},expected=201)
        resources.append(resource)
        return resource['id']
    assignments=base+'/members/'+owner['user_id']+'/resource-assignments'
    def assign(ids):
        current=request(management,assignments,admin['access_token'])
        return request(management,assignments,admin['access_token'],{'expected_revision':current['revision'],
            'assignments':[{'resource_id':rid,'instance_id':item['id'],'environment_id':None} for rid in ids]},method='PUT')
    def listed():
        return request(item['origin'],'/api/v1/managed-resources',item['token'])
    def start(ids,expected=201):
        thread=request(item['origin'],'/api/v1/threads',item['token'],
            {'project_id':item['project']['id'],'title':'Central resource acceptance'},expected=201)
        return request(item['origin'],'/api/v1/runs',item['token'],{
            'project_id':item['project']['id'],'thread_id':thread['id'],
            'project_revision':item['project']['revision'],'project_privacy':'team_managed',
            'executor_target':{'kind':'server_linux','pool_id':None},'model_profile_id':None,
            'required_capabilities':['model.external','tool.mcp.invoke'],
            'input':{'prompt':'Exercise the assigned central MCP tool once.','managed_resource_ids':ids},
            'idempotency_key':str(uuid.uuid4())},expected=expected)
    def state(run):
        return tenant_sql(item,f"SELECT state FROM runs WHERE id='{run}';")
    def terminal(run):
        return state(run) in ['completed','failed','cancelled','interrupted']

    # The process can only write in its own mounted sandbox workspace. It never
    # contacts the network, uses a real credential, or accesses host documents.
    program='''import json,sys,pathlib
for line in sys.stdin:
 value=json.loads(line)
 if 'id' not in value: continue
 method=value.get('method')
 if method=='initialize': result={'protocolVersion':value['params']['protocolVersion'],'capabilities':{'tools':{}},'serverInfo':{'name':'central-fixture','version':'1'}}
 elif method=='tools/list': result={'tools':[{'name':'effect','description':'Record a local synthetic test effect','inputSchema':{'type':'object','properties':{'text':{'type':'string'}},'required':['text']}}]}
 elif method=='tools/call':
  text=value['params']['arguments']['text']
  pathlib.Path('central-acceptance-effect.txt').write_text(text)
  result={'content':[{'type':'text','text':text}],'isError':False}
 else: result={}
 print(json.dumps({'jsonrpc':'2.0','id':value['id'],'result':result}),flush=True)
'''
    try:
        endpoint=catalog('endpoint','Central acceptance endpoint',{'base_url':'http://fixture-model:9002/v1','protocol':'openai_compatible'})
        model_id=catalog('model','Central acceptance model',{'model_id':'fixture','endpoint_resource_id':endpoint,
            'context_window':131072,'max_output_tokens':32768})
        skill=catalog('skill','Central acceptance skill',{'instructions':skill_marker})
        mcp_id=catalog('mcp','Central acceptance MCP',{'transport':'stdio','command':'python3','args':['-u','-c',program]})
        role=catalog('agent_role','Central acceptance role',{'system_prompt':role_marker,'resource_ids':[model_id,skill,mcp_id]})
        assign([role])
        assert next(r for r in listed() if r['id']==role)['available'] is False
        start([role],expected=409)
        assert not requests
        passed('central_role_does_not_grant_unassigned_dependencies_over_actual_http')

        granted=[role,model_id,skill,mcp_id]
        assign(granted)
        assert all(r['available'] for r in listed())
        result=start([role])
        assert result['spec']['model_profile_id']==model_id
        run=result['spec']['id']
        wait(lambda:terminal(run),'central MCP run did not finish',seconds=120)
        assert state(run)=='completed', 'central resource worker failed: '+state(run)
        assert len(requests)==2, 'central MCP must execute once and complete with one follow-up model request'
        context=json.dumps(requests[0]['messages'])
        assert role_marker in context and skill_marker in context
        output=json.dumps([m for m in requests[-1]['messages'] if m['role']=='tool'])
        assert marker in output
        passed('central_role_skill_model_proxy_and_real_sandbox_mcp_are_consumed')

        before=len(requests)
        mode['value']='revoke'
        revoked=start([role])['spec']['id']
        wait(entered.is_set,'central model did not enter the controlled revocation window')
        assign([role,model_id,skill])
        release.set()
        wait(lambda:terminal(revoked),'revoked central run did not terminate',seconds=120)
        assert state(revoked)=='failed'
        assert len(requests)==before+1
        events=tenant_sql(item,f"SELECT COALESCE(json_agg(e),'[]'::json) FROM run_events e WHERE run_id='{revoked}';")
        # The model-completed event legitimately includes requested arguments.
        # Test dispatch events instead of mistaking those arguments for an effect.
        assert not any(e['kind'] in ['tool_started','tool_completed','tool_failed'] for e in json.loads(events))
        start([role],expected=409)
        passed('central_revocation_after_model_prevents_actual_tool_dispatch_and_new_preparation')
        return {'completed_run_id':run,'revoked_run_id':revoked,'model_requests':len(requests),
            'completed_mcp_calls':1,'revoked_tool_dispatches':0,'real_model':False,
            'model_proxy':'actual management proxy to controlled loopback fixture',
            'mcp':'actual sandbox process and workspace write'}
    finally:
        release.set()
        model.shutdown()
