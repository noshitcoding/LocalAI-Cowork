"""Activation/replica barriers over real HTTP and PostgreSQL; runtime receipts simulated."""
import copy
import json
import subprocess
import uuid


def runtime():
    return {'version':1,'server_image':'cowork-server@sha256:'+'a'*64,'manifest_sha256':'b'*64}


def run(request,token,user,sql,broker_key,replica):
    uid=lambda:str(uuid.uuid4())
    ceiling={'revision':1,'platform_concurrent_runs':4,'organisation_concurrent_runs':3,'user_concurrent_runs':2}
    org=request('/management/v1/organisations',{'name':'Coverage fixture'},token)['id']
    base='/management/v1/organisations/'+org
    legacy=request(base+'/instances',{'name':'Legacy fixture','backend':'compose'},token)['id']
    def current(instance,origin=None):return next(row for row in request(base+'/instances',token=token,origin_override=origin) if row['id']==instance)
    def enqueue(instance,action,origin=None):
        return request(base+'/instances/'+instance+'/operations',{'action':action,'expected_revision':current(instance,origin)['revision'],'idempotency_key':uid()},token,origin_override=origin)
    def claim(origin=None,expected=200,version=None):
        return request('/management/runner/v1/claim',{'backend':'compose',**({'execution_admission_version':version} if version is not None else {})},broker=True,origin_override=origin,expected=expected)
    def complete(op,result,origin=None,expected=200):
        return request('/management/runner/v1/operations/'+op['id']+'/complete',{'lease_token':op['lease_token'],'succeeded':True,'result':result},broker=True,origin_override=origin,expected=expected)
    enqueue(legacy,'provision'); op=claim(); assert op.get('execution_binding') is None
    complete(op,{'receipt_id':op['id']})
    assert current(legacy)['state']=='ready'
    replica(ceiling,'requires verified enrollment of 1',admission=True)
    assert sql('SELECT count(*) FROM management_execution_activation;').strip()=='0'
    assert sql('SELECT count(*) FROM management_execution_ceiling_revisions;').strip()=='0'
    enqueue(legacy,'stop'); op=claim(); complete(op,{'receipt_id':op['id']})
    replica(ceiling,'requires verified enrollment of 1',admission=True)
    # Only this disposable metadata fixture is retired, without claiming a real deletion.
    sql(f"UPDATE management_instances SET state='deleted' WHERE id='{legacy}';")
    active=replica(ceiling,admission=True)
    request('/readyz',expected=503)  # A warm disabled replica leaves load-balancer readiness.
    assert sql('SELECT count(*) FROM management_execution_activation;').strip()=='1'
    print('activation_rejects_running_and_stopped_legacy_instances_without_partial_latch=ok')

    # The already-running disabled replica cannot create a legacy instance after activation.
    instance=request(base+'/instances',{'name':'Warm replica fixture','backend':'compose'},token)['id']
    assert sql(f"SELECT execution_required FROM management_instances WHERE id='{instance}';").strip()=='t'
    queued=enqueue(instance,'provision')
    claim(expected=503); claim(active,expected=503); claim(active,expected=503,version=2)
    assert sql(f"SELECT state FROM management_operations WHERE id='{queued['id']}';").strip()=='queued'
    def rejected_sql(query):
        try:sql(query)
        except subprocess.CalledProcessError:return
        raise AssertionError('database bypass unexpectedly succeeded')
    rejected_sql(f"UPDATE management_operations SET state='running',lease_token='{uid()}',lease_expires_at=now()+interval '1 hour' WHERE id='{queued['id']}';")
    assert sql(f"SELECT state FROM management_operations WHERE id='{queued['id']}';").strip()=='queued'
    op=claim(version=1)  # Warm replica derives requirements from durable instance metadata.
    assert op['execution_runtime_required'] and op['execution_binding']
    result={'receipt_id':op['id'],'execution':{'binding':op['execution_binding'],'active_epoch':op['execution_binding']['source_epoch']}}
    complete(op,result,active,422)
    result['execution']['runtime']=runtime()
    bad=copy.deepcopy(result);bad['execution']['runtime']['server_image']='cowork-server:mutable';complete(op,bad,active,422)
    bad=copy.deepcopy(result);bad['execution']['runtime']['version']=2;complete(op,bad,active,422)
    bad=copy.deepcopy(result);bad['execution']['runtime']['manifest_sha256']='unproven';complete(op,bad,active,422)
    assert current(instance,active)['state']=='provisioning'
    sql("""CREATE FUNCTION reject_runtime_audit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected'; END $$;
        CREATE TRIGGER reject_runtime_audit BEFORE INSERT ON management_audit FOR EACH ROW WHEN (NEW.action='instance.execution.runtime') EXECUTE FUNCTION reject_runtime_audit();""")
    complete(op,result,active,500)
    assert current(instance,active)['state']=='provisioning'
    assert sql(f"SELECT execution_evidence IS NULL FROM management_instances WHERE id='{instance}';").strip()=='t'
    sql('DROP TRIGGER reject_runtime_audit ON management_audit; DROP FUNCTION reject_runtime_audit();')
    complete(op,result,active);complete(op,result,active)
    assert current(instance,active)['state']=='ready' and 'execution_evidence' not in current(instance,active)
    evidence=json.loads(sql(f"SELECT execution_evidence FROM management_instances WHERE id='{instance}';"))
    assert evidence==result['execution']
    rejected_sql(f"UPDATE management_instances SET execution_required=false WHERE id='{instance}';")
    rejected_sql(f"UPDATE management_instances SET execution_evidence=NULL WHERE id='{instance}';")
    rejected_sql(f"UPDATE management_instances SET execution_epoch='{uid()}' WHERE id='{instance}';")
    rejected_sql('DELETE FROM management_execution_activation;')
    assert json.loads(sql(f"SELECT execution_evidence FROM management_instances WHERE id='{instance}';"))==evidence
    print('warm_replica_creation_and_legacy_dispatch_are_fenced_by_database_contract=ok')
    replica(ceiling,admission=True)
    replica(ceiling,'refusing a disabled management replica',admission=False)
    print('verified_runtime_receipts_bind_readiness_and_restart_cannot_disable_admission=ok (simulated runtime proof)')
