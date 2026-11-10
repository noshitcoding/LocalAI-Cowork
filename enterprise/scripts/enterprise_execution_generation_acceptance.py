"""Real lifecycle API/database; infrastructure generation receipts are simulated."""
import copy
from enterprise_execution_coverage_acceptance import runtime
import hashlib
import hmac
import json
import uuid


def run(request, token, user, sql, broker_key, replica):
    uid = lambda: str(uuid.uuid4())
    owner = user('generation-owner@enterprise.test')
    org = request('/management/v1/organisations', {'name':'Execution lifecycle'}, token)['id']
    base = f'/management/v1/organisations/{org}'
    request(base+'/members', {'email':'generation-owner@enterprise.test','role':'member'},token)
    instance = request(base+'/instances', {'name':'Generation fixture','backend':'compose'},owner['access_token'])['id']
    path = base+f'/instances/{instance}/operations'
    epoch = lambda:sql(f"SELECT execution_epoch FROM management_instances WHERE id='{instance}';").strip()
    state = lambda:sql(f"SELECT state FROM management_instances WHERE id='{instance}';").strip()
    held = lambda:request(base+'/execution-capacity',token=token)['held_runs']
    key = hmac.new(broker_key.encode(),f'cowork-federation-v1\0{org}\0{instance}'.encode(),hashlib.sha256).hexdigest()
    def reserve(e, run_id=None, expected=200):
        value={'organisation_id':org,'instance_id':instance,'execution_epoch':e,'run_id':run_id or uid(),'user_id':owner['user_id'],'target':{'kind':'server_linux','pool_id':None}}
        decision=request('/management/runner/v1/execution-reserve',value,instance_key=key,expected=expected)
        return value, decision
    def settle(pair):
        value,decision=pair
        request('/management/runner/v1/execution-release',{k:value[k] for k in ['organisation_id','instance_id','execution_epoch','run_id']} | {'reservation_id':decision['reservation_id'],'completion_id':uid(),'outcome':'completed','execution_stopped':True},instance_key=key)
    def claim():
        return request('/management/runner/v1/claim',{'backend':'compose','execution_admission_version':1},broker=True)
    def enqueue(action, **extra):
        current=next(row for row in request(base+'/instances',token=token) if row['id']==instance)
        return request(path,{'action':action,'expected_revision':current['revision'],'idempotency_key':uid(),**extra},owner['access_token'])['id']
    def receipt(op, active=None, **extra):
        return {'receipt_id':op['id'],'execution':{'binding':op['execution_binding'],'active_epoch':active or op['execution_binding']['source_epoch'],'runtime':runtime()},**extra}
    def complete(op, result=None, succeeded=True, expected=200):
        return request(f"/management/runner/v1/operations/{op['id']}/complete",{'lease_token':op['lease_token'],'succeeded':succeeded,'result':result or receipt(op)},broker=True,expected=expected)
    def lifecycle(action, **extra):
        op_id=enqueue(action,**extra)
        op=claim()
        assert op['id']==op_id
        assert op['execution_binding']['source_epoch']==epoch()
        return op
    def backup_fields(op):
        return {'backup_id':op['id'],'backup_sha256':'c'*64,'backup_bytes':1234,'manifest_version':1}
    source=epoch()
    provision=lifecycle('provision')
    assert all('execution_epoch' not in value for value in request(base+'/instances',token=token))
    assert all('execution_binding' not in value for value in request(path,token=token))
    assert provision['execution_binding']['source_epoch']==source==provision['execution_binding']['candidate_epoch']
    complete(provision,{'receipt_id':provision['id']},expected=422)
    wrong=copy.deepcopy(receipt(provision)); wrong['execution']['binding']['instance_id']=uid()
    complete(provision,wrong,expected=422)
    complete(provision)
    outstanding=reserve(source)
    assert outstanding[1]['admitted'] and held()==1
    enqueue('stop')
    reserve(source,expected=404)  # A queued lifecycle action already fences new work.
    stop=claim()
    sql(f"UPDATE management_operations SET lease_expires_at=now()-interval '1 hour' WHERE id='{stop['id']}';")
    pending=request('/management/runner/v1/pending',{'backend':'compose','execution_admission_version':1},broker=True)
    assert pending[0]['execution_binding']==stop['execution_binding'] and not pending[0]['recovery_authorized']
    assert claim() is None
    complete(stop)  # The exact delayed original receipt still settles.
    assert epoch()==source and state()=='stopped' and held()==1
    backup=lifecycle('backup'); complete(backup,receipt(backup,**backup_fields(backup)))
    restore=lifecycle('restore',backup_id=backup['id'])
    candidate=restore['execution_binding']['candidate_epoch']
    assert candidate!=source and epoch()==source
    complete(restore,receipt(restore,backup_id=backup['id']),expected=422)
    sql("""CREATE FUNCTION reject_epoch_audit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected epoch audit failure'; END $$;
        CREATE TRIGGER reject_epoch_audit BEFORE INSERT ON management_audit FOR EACH ROW WHEN (NEW.action='instance.execution.epoch') EXECUTE FUNCTION reject_epoch_audit();""")
    restored=receipt(restore,candidate,backup_id=backup['id'])
    complete(restore,restored,expected=500)
    assert epoch()==source and held()==1
    assert sql(f"SELECT state FROM management_operations WHERE id='{restore['id']}';").strip()=='running'
    sql('DROP TRIGGER reject_epoch_audit ON management_audit; DROP FUNCTION reject_epoch_audit();')
    complete(restore,restored)
    assert epoch()==candidate and held()==1 and state()=='stopped'
    complete(lifecycle('start'))
    reserve(source,expected=409)
    reserve(source,outstanding[0]['run_id'],expected=409)
    settle(outstanding)
    fresh=reserve(candidate); assert fresh[1]['admitted']; settle(fresh)
    complete(restore,restored)  # Replay cannot rotate a later generation again.
    assert epoch()==candidate and held()==0
    print('lifecycle_epoch_receipts_are_atomic_scoped_durable_and_fence_old_admission=ok (simulated infrastructure receipts)')
    complete(lifecycle('stop'))
    failed=lifecycle('restore',backup_id=backup['id'])
    complete(failed,receipt(failed,error_code='fixture_source_preserved',instance_state='stopped'),succeeded=False)
    assert epoch()==candidate
    interrupted=lifecycle('restore',backup_id=backup['id'])
    sql(f"UPDATE management_operations SET lease_expires_at=now()-interval '1 hour' WHERE id='{interrupted['id']}';")
    request(path,token=token)
    recover_result=receipt(interrupted,interrupted['execution_binding']['candidate_epoch'],error_code='operation_interrupted_resources_stopped',instance_state='stopped')
    complete(interrupted,recover_result,succeeded=False,expected=422)
    request(path+f"/{interrupted['id']}/recover",{'mode':'stop_if_unproven'},owner['access_token'])
    pending=request('/management/runner/v1/pending',{'backend':'compose','execution_admission_version':1},broker=True)
    assert pending[0]['recovery_authorized'] and pending[0]['execution_binding']==interrupted['execution_binding']
    complete(interrupted,recover_result,succeeded=False)
    assert epoch()==interrupted['execution_binding']['candidate_epoch'] and state()=='stopped'
    print('failed_restore_retains_source_and_explicit_recovery_can_settle_switched_pointer=ok (simulated infrastructure receipts)')
    def images(letter): return {k:f'registry.test/{k}@sha256:'+letter*64 for k in ['server','runner','sandbox_core','sandbox_gui','sandbox_crew','egress_proxy']}
    digest=lambda value:hashlib.sha256(json.dumps(value,sort_keys=True,separators=(',',':')).encode()).hexdigest()
    source_release,target_release=digest(images('a')),digest(images('b'))
    request('/management/runner/v1/releases/sync',{'releases':[
        {'version':1,'id':source_release,'name':'Source','images':images('a'),'supported_sources':[target_release]},
        {'version':1,'id':target_release,'name':'Target','images':images('b'),'supported_sources':[source_release]}],
        'instances':[{'organisation_id':org,'instance_id':instance,'release':source_release}]},broker=True)
    old=epoch()
    canary=lifecycle('upgrade',release=target_release,qualification_only=True)
    common={'instance_id':instance,'source_release':source_release,'release':target_release,'instance_state':'stopped'}
    complete(canary,receipt(canary,canary['execution_binding']['candidate_epoch'],**common,**backup_fields(canary),qualification_only=True,rollback_verified=True),expected=422)
    complete(canary,receipt(canary,**common,**backup_fields(canary),qualification_only=True,rollback_verified=True))
    assert epoch()==old
    promotion=lifecycle('upgrade',release=target_release)
    promoted=promotion['execution_binding']['candidate_epoch']
    complete(promotion,receipt(promotion,promoted,**common,**backup_fields(promotion),qualification_only=False,generation=promotion['id']))
    assert epoch()==promoted and promoted!=old
    rollback=lifecycle('rollback',rollback_operation_id=promotion['id'])
    rolled_back=rollback['execution_binding']['candidate_epoch']
    complete(rollback,receipt(rollback,rolled_back,instance_id=instance,source_release=target_release,release=source_release,instance_state='stopped',**backup_fields(rollback),generation=rollback['id'],rollback_operation_id=promotion['id'],restored_backup_id=promotion['id']))
    assert epoch()==rolled_back and rolled_back not in [old,promoted,source,candidate]
    complete(promotion,receipt(promotion,promoted,**common,**backup_fields(promotion),qualification_only=False,generation=promotion['id']))
    assert epoch()==rolled_back
    events=request(base+'/audit',token=token)
    assert sum(event['action']=='instance.execution.epoch' for event in events)==4
    print('canary_retains_source_promotion_and_rollback_use_distinct_fresh_execution_epochs=ok (simulated infrastructure receipts)')
