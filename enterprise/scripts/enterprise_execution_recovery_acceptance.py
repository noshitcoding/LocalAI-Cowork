"""Real HTTP/PostgreSQL recovery boundaries; stop receipts here are simulated."""
import copy
from enterprise_execution_coverage_acceptance import runtime
import hashlib
import hmac
import uuid


def run(request,token,user,sql,broker_key,replica):
    uid=lambda:str(uuid.uuid4())
    owner=user('recovery-owner@enterprise.test'); admin=user('recovery-admin@enterprise.test')
    org=request('/management/v1/organisations',{'name':'Execution recovery'},token)['id']
    base='/management/v1/organisations/'+org
    for account,role in [('recovery-owner@enterprise.test','member'),('recovery-admin@enterprise.test','admin')]:
        request(base+'/members',{'email':account,'role':role},token)
    instance=request(base+'/instances',{'name':'Recovery instance','backend':'compose'},owner['access_token'])['id']
    path=base+'/instances/'+instance+'/operations'
    current=lambda:next(i for i in request(base+'/instances',token=token) if i['id']==instance)
    held=lambda:request(base+'/execution-capacity',token=token)['held_runs']
    key=hmac.new(broker_key.encode(),f'cowork-federation-v1\0{org}\0{instance}'.encode(),hashlib.sha256).hexdigest()
    def enqueue(action='stop',ids=None,actor=None,expected=200,**extra):
        value={'action':action,'expected_revision':current()['revision'],'idempotency_key':uid(),**extra}
        if ids is not None: value.update({'reconcile_reservation_ids':ids,'acknowledge_execution_stop':True,**extra})
        return request(path,value,actor or admin['access_token'],expected=expected)
    def claim(): return request('/management/runner/v1/claim',{'backend':'compose','execution_admission_version':1},broker=True)
    def receipt(op):
        result={'receipt_id':op['id'],'execution':{'binding':op['execution_binding'],'active_epoch':op['execution_binding']['source_epoch'],'runtime':runtime()}}
        if op.get('execution_recovery_binding'):
            result['execution_recovery']={'binding':op['execution_recovery_binding'],'stopped':True,'inventory_sha256':'a'*64,'verified_generations':2}
        return result
    def complete(op,result=None,expected=200):
        return request('/management/runner/v1/operations/'+op['id']+'/complete',
            {'lease_token':op['lease_token'],'succeeded':True,'result':result or receipt(op)},broker=True,expected=expected)
    enqueue('provision'); op=claim(); complete(op)
    epoch=op['execution_binding']['source_epoch']
    def reserve(target=None,run=None,expected=200):
        value={'organisation_id':org,'instance_id':instance,'execution_epoch':epoch,'run_id':run or uid(),
            'user_id':owner['user_id'],'target':target or {'kind':'server_linux','pool_id':None}}
        result=request('/management/runner/v1/execution-reserve',value,instance_key=key,expected=expected)
        return value,result
    def release(pair):
        value,result=pair
        body={k:value[k] for k in ['organisation_id','instance_id','execution_epoch','run_id']}
        body.update({'reservation_id':result['reservation_id'],'completion_id':uid(),'outcome':'completed','execution_stopped':True})
        request('/management/runner/v1/execution-release',body,instance_key=key)
    server=reserve(); remote=reserve({'kind':'personal_device','device_id':uid()})
    selected=[server[1]['reservation_id']]
    assert held()==2
    enqueue(ids=selected,actor=owner['access_token'],expected=404)
    enqueue(ids=selected,acknowledge_execution_stop=False,expected=422)
    enqueue(ids=[uid()],expected=404)
    enqueue(ids=[remote[1]['reservation_id']],expected=422)
    enqueue(ids=selected*2,expected=422)
    enqueue('start',ids=selected,expected=422)
    enqueue(ids=selected)
    request(base+'/members',{'email':'recovery-admin@enterprise.test','role':'member'},token)
    assert claim() is None and held()==2
    request(base+'/members',{'email':'recovery-admin@enterprise.test','role':'admin'},token)
    enqueue(ids=selected); op=claim()
    assert op['execution_recovery_binding']['reservations']==[{'reservation_id':selected[0],'execution_epoch':epoch,'run_id':server[0]['run_id']}]
    assert all('execution_recovery_binding' not in row for row in request(path,token=token))
    good=receipt(op)
    missing=copy.deepcopy(good); del missing['execution_recovery']; complete(op,missing,422)
    bad=copy.deepcopy(good); bad['execution_recovery']['stopped']=False; complete(op,bad,422)
    bad=copy.deepcopy(good); bad['execution_recovery']['binding']['reservations'][0]['reservation_id']=remote[1]['reservation_id']; complete(op,bad,422)
    assert held()==2
    sql("""CREATE FUNCTION reject_recovery_audit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected'; END $$;
        CREATE TRIGGER reject_recovery_audit BEFORE INSERT ON management_audit FOR EACH ROW WHEN (NEW.action='execution.recover') EXECUTE FUNCTION reject_recovery_audit();""")
    complete(op,good,500); assert held()==2 and current()['state']=='ready'
    sql('DROP TRIGGER reject_recovery_audit ON management_audit; DROP FUNCTION reject_recovery_audit();')
    complete(op,good); assert held()==1 and current()['state']=='stopped'
    stored=sql(f"SELECT completion::text FROM management_execution_reservations WHERE id='{selected[0]}';")
    release(server); release(server)
    assert held()==1 and sql(f"SELECT completion::text FROM management_execution_reservations WHERE id='{selected[0]}';")==stored
    complete(op,good); bad=copy.deepcopy(good); bad['execution_recovery']['inventory_sha256']='b'*64; complete(op,bad,409)
    assert sql("SELECT count(*) FROM management_audit WHERE action='execution.recover';").strip()=='1'
    print('recovery_is_admin_scoped_server_only_atomic_idempotent_and_preserves_late_completion_evidence=ok (simulated stop proof)')
    enqueue('start'); complete(claim()); reserve(run=server[0]['run_id'],expected=409)
    fresh=reserve(); assert fresh[1]['admitted']
    enqueue(ids=[fresh[1]['reservation_id']]); interrupted=claim()
    request('/management/runner/v1/operations/'+interrupted['id']+'/reconciliation',
        {'lease_token':interrupted['lease_token'],'reason':'operation_recovery_requires_review'},broker=True)
    assert held()==2 and claim() is None
    recover=path+'/'+interrupted['id']+'/recover'
    request(recover,{'mode':'stop_if_unproven'},owner['access_token'],expected=404)
    request(recover,{'mode':'stop_if_unproven'},admin['access_token'])
    request(base+'/members',{'email':'recovery-admin@enterprise.test','role':'member'},token)
    pending=request('/management/runner/v1/pending',{'backend':'compose','execution_admission_version':1},broker=True)
    assert not pending[0]['recovery_authorized'] and pending[0]['execution_recovery_binding']==interrupted['execution_recovery_binding']
    request(base+'/members',{'email':'recovery-admin@enterprise.test','role':'admin'},token)
    assert request('/management/runner/v1/pending',{'backend':'compose','execution_admission_version':1},broker=True)[0]['recovery_authorized']
    complete(interrupted); assert held()==1
    print('recovery_interruption_retains_holds_binding_and_requires_current_administrator_authority=ok (simulated stop proof)')
    enqueue('start'); complete(claim()); racing=reserve()
    enqueue(ids=[racing[1]['reservation_id']]); release(racing)
    racing_op=claim(); complete(racing_op)
    assert held()==1 and sql("SELECT count(*) FROM management_audit WHERE action='execution.recover';").strip()=='2'
    # Selecting remote work is still forbidden after the server is fully stopped.
    enqueue(ids=[remote[1]['reservation_id']],expected=422)
    enqueue('delete',confirmation_name='Recovery instance',purge_backups=True,expected=409)
    release(remote); assert held()==0
    print('completion_race_does_not_overrelease_and_offline_device_holds_survive_server_stop=ok (simulated device)')
    enqueue('start'); complete(claim()); stopped_work=reserve()
    enqueue('stop'); complete(claim())
    assert current()['state']=='stopped' and held()==1
    enqueue('delete',confirmation_name='Recovery instance',purge_backups=True,expected=409)
    enqueue(ids=[stopped_work[1]['reservation_id']]); complete(claim())
    assert current()['state']=='stopped' and held()==0
    enqueue('delete',confirmation_name='Recovery instance',purge_backups=True)
    assert claim()['action']=='delete'
    print('ordinary_stop_retains_holds_stopped_instance_can_be_verified_and_deletion_requires_settlement=ok (simulated stop proof)')
