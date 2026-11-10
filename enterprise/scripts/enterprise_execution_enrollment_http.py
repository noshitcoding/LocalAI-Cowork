"""Real HTTP/PG enrollment transactions with simulated infrastructure receipts."""
import copy
import subprocess
import uuid
from enterprise_execution_coverage_acceptance import runtime


def run(request,token,user,sql,broker_key,replica):
    uid=lambda:str(uuid.uuid4())
    admin=user('enrollment-admin@enterprise.test');owner=user('enrollment-owner@enterprise.test')
    org=request('/management/v1/organisations',{'name':'Enrollment fixture'},token)['id']
    base='/management/v1/organisations/'+org
    for email,role in [('enrollment-admin@enterprise.test','admin'),('enrollment-owner@enterprise.test','member')]:
        request(base+'/members',{'email':email,'role':role},token)
    instance=request(base+'/instances',{'name':'Legacy fixture','backend':'compose'},owner['access_token'])['id']
    path=base+'/instances/'+instance+'/operations'
    def current(actor=None):return next(v for v in request(base+'/instances',token=actor or token) if v['id']==instance)
    def enqueue(action,expected=200,actor=None,**extra):
        return request(path,{'action':action,'expected_revision':current()['revision'],'idempotency_key':uid(),**extra},actor or admin['access_token'],expected=expected)
    def claim(expected=200,**contract):return request('/management/runner/v1/claim',{'backend':'compose',**contract},broker=True,expected=expected)
    def complete(op,result,succeeded=True,expected=200):
        return request('/management/runner/v1/operations/'+op['id']+'/complete',{'lease_token':op['lease_token'],'succeeded':succeeded,'result':result},broker=True,expected=expected)
    def receipt(op,enrolled=True):
        bound=op['execution_binding']
        value={'receipt_id':op['id'],'instance_state':'stopped','execution':{'binding':bound,'active_epoch':bound['source_epoch']},
            'execution_enrollment':{'binding':bound,'outcome':'enrolled' if enrolled else 'unchanged','source_manifest_sha256':'a'*64,
                'inventory_sha256':'b'*64 if enrolled else None,'verified_generations':2 if enrolled else 0}}
        if enrolled:
            value.update(backup_id=op['id'],backup_sha256='c'*64,backup_bytes=100,manifest_version=1)
            value['execution']['runtime']=runtime()
        else:value['error_code']='execution_enrollment_precheck_failed'
        return value
    def rejection(query):
        try:sql(query)
        except subprocess.CalledProcessError:return
        raise AssertionError('database bypass accepted')
    enqueue('provision');op=claim();complete(op,{'receipt_id':op['id']})
    enqueue('enroll_execution',expected=422,acknowledge_execution_enrollment=True)
    enqueue('stop');op=claim();complete(op,{'receipt_id':op['id']})
    assert 'enroll_execution' in current()['available_actions']
    assert 'enroll_execution' not in current(owner['access_token'])['available_actions']
    enqueue('enroll_execution',expected=404,actor=owner['access_token'],acknowledge_execution_enrollment=True)
    enqueue('enroll_execution',expected=422)
    enqueue('enroll_execution',expected=422,acknowledge_execution_enrollment=True,backup_id=uid())
    enqueue('enroll_execution',acknowledge_execution_enrollment=True)
    request(base+'/members',{'email':'enrollment-admin@enterprise.test','role':'member'},token)
    contract={'execution_admission_version':1,'execution_enrollment_version':1}
    claim(expected=503,execution_admission_version=1)
    assert claim(**contract) is None
    request(base+'/members',{'email':'enrollment-admin@enterprise.test','role':'admin'},token)
    queued=enqueue('enroll_execution',acknowledge_execution_enrollment=True)
    claim(expected=503);claim(expected=503,execution_admission_version=1,execution_enrollment_version=2)
    rejection(f"UPDATE management_operations SET state='running',lease_token='{uid()}' WHERE id='{queued['id']}';")
    rejection(f"UPDATE management_instances SET execution_required=true WHERE id='{instance}';")
    op=claim(**contract);assert op['execution_runtime_required'] and op['execution_binding']
    failed=receipt(op,False);complete(op,failed,False)
    assert current()['execution_required'] is False and current()['state']=='stopped'
    assert sql('SELECT count(*) FROM management_backups;').strip()=='0'
    print('enrollment_requires_stopped_legacy_admin_acknowledgement_and_versioned_operator_dispatch=ok')
    enqueue('enroll_execution',acknowledge_execution_enrollment=True);op=claim(**contract)
    good=receipt(op)
    for mutate in [lambda r:r.pop('execution_enrollment'),lambda r:r['execution_enrollment'].update(outcome='unchanged'),
        lambda r:r['execution_enrollment']['binding'].update(instance_id=uid()),lambda r:r['execution'].pop('runtime'),
        lambda r:r.update(backup_id=uid()),lambda r:r['execution_enrollment'].update(verified_generations=0)]:
        bad=copy.deepcopy(good);mutate(bad);complete(op,bad,expected=422)
    # A failed enrollment audit must roll back runtime proof, backup catalog and latch together.
    sql("CREATE FUNCTION reject_enrollment_audit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected'; END $$; CREATE TRIGGER reject_enrollment_audit BEFORE INSERT ON management_audit FOR EACH ROW WHEN (NEW.action='instance.execution.enrolled') EXECUTE FUNCTION reject_enrollment_audit();")
    complete(op,good,expected=500)
    assert not current()['execution_required'] and sql('SELECT count(*) FROM management_backups;').strip()=='0'
    sql('DROP TRIGGER reject_enrollment_audit ON management_audit; DROP FUNCTION reject_enrollment_audit();')
    request('/management/runner/v1/operations/'+op['id']+'/reconciliation',{'lease_token':op['lease_token'],'reason':'operation_recovery_requires_review'},broker=True)
    recover=path+'/'+op['id']+'/recover'
    request(recover,{'mode':'stop_if_unproven'},admin['access_token'],expected=422)
    request(recover,{'mode':'continue_enrollment'},owner['access_token'],expected=404)
    request(recover,{'mode':'continue_enrollment'},admin['access_token'])
    request(base+'/members',{'email':'enrollment-admin@enterprise.test','role':'member'},token)
    assert request('/management/runner/v1/pending',{'backend':'compose',**contract},broker=True)[0]['recovery_authorized'] is False
    request(base+'/members',{'email':'enrollment-admin@enterprise.test','role':'admin'},token)
    pending=request('/management/runner/v1/pending',{'backend':'compose',**contract},broker=True)[0]
    assert pending['recovery_authorized'] and pending['execution_binding']==op['execution_binding']
    assert not request('/management/runner/v1/pending',{'backend':'compose',**contract},broker=True)[0]['recovery_authorized']
    complete(op,good);complete(op,good)
    assert current()['execution_required'] and current()['state']=='stopped'
    assert 'start' not in current()['available_actions']
    enqueue('start',expected=503)
    rejection(f"UPDATE management_instances SET execution_enrollment=NULL WHERE id='{instance}';")
    rejection(f"UPDATE management_instances SET execution_required=false WHERE id='{instance}';")
    assert sql('SELECT count(*) FROM management_backups;').strip()=='1'
    print('enrollment_backup_runtime_latch_and_audit_are_atomic_and_recovery_is_one_explicit_admin_attempt=ok')
    ceiling={'revision':1,'platform_concurrent_runs':4,'organisation_concurrent_runs':3,'user_concurrent_runs':2}
    active=replica(ceiling,admission=True)
    ready=next(v for v in request(base+'/instances',token=token,origin_override=active) if v['id']==instance)
    assert 'start' in ready['available_actions'] and 'enroll_execution' not in ready['available_actions']
    assert 'execution_enrollment' not in ready and 'execution_evidence' not in ready
    replica(ceiling,'refusing a disabled management replica',admission=False)
    print('only_verified_enrollment_allows_global_activation_and_start_is_gated_until_then=ok')
