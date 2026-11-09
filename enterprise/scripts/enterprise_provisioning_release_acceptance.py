"""Real HTTP/PostgreSQL selection boundaries; infrastructure receipts are simulated here."""
import hashlib
import json
import subprocess
import uuid


def run(request, token, user, sql, broker_key, replica):
    uid = lambda: str(uuid.uuid4())
    owner, outsider = user('release-owner@enterprise.test'), user('release-outsider@enterprise.test')
    org = request('/management/v1/organisations', {'name':'Selected releases'}, token)['id']
    base = '/management/v1/organisations/' + org
    request(base+'/members', {'email':'release-owner@enterprise.test','role':'member'}, token)
    images = {key:f'registry.test/{key}@sha256:'+'a'*64 for key in ['server','runner','sandbox_core','sandbox_gui','sandbox_crew','egress_proxy']}
    release = hashlib.sha256(json.dumps(images,sort_keys=True,separators=(',',':')).encode()).hexdigest()
    manifest = dict(version=1,id=release,name='Approved initial version',images=images,supported_sources=['b'*64])
    def sync(active=True): request('/management/runner/v1/releases/sync',{'releases':[manifest] if active else [],'instances':[]},broker=True)
    def create(name='Selected',expected=200,**extra):
        return request(base+'/instances',dict(name=name,backend='compose',release=release,**extra),owner['access_token'],expected=expected)
    def current(i): return next(row for row in request(base+'/instances',token=token) if row['id']==i)
    def path(i): return base+'/instances/'+i+'/operations'
    def enqueue(i,expected=200,**extra):
        return request(path(i),dict(action='provision',expected_revision=current(i)['revision'],idempotency_key=uid(),**extra),owner['access_token'],expected=expected)
    def claim(expected=200,**contract): return request('/management/runner/v1/claim',{'backend':'compose',**contract},broker=True,expected=expected)
    def complete(op,result,succeeded=True,expected=200):
        return request('/management/runner/v1/operations/'+op['id']+'/complete',dict(lease_token=op['lease_token'],succeeded=succeeded,result=result),broker=True,expected=expected)
    def rejected(query):
        try: sql(query)
        except subprocess.CalledProcessError: return
        raise AssertionError('database accepted a changed provisioning binding')
    contract={'provisioning_release_version':1}
    sync()
    assert request('/management/v1/organisations',token=token)['release_selection_enabled'] is True
    assert request(base+'/provisioning-releases',token=owner['access_token'])==[{'id':release,'name':manifest['name']}]
    request(base+'/provisioning-releases',token=outsider['access_token'],expected=404)
    request(base+'/instances',{'name':'Foreign','backend':'compose','release':release},outsider['access_token'],expected=404)
    request(base+'/members',{'email':'release-owner@enterprise.test','role':'auditor'},token)
    request(base+'/provisioning-releases',token=owner['access_token'],expected=404)
    create(expected=404)
    request(base+'/members',{'email':'release-owner@enterprise.test','role':'member'},token)
    for bad in ['latest','c'*64]:
        request(base+'/instances',{'name':'Invalid','backend':'compose','release':bad},owner['access_token'],expected=422)
    request(base+'/instances',{'name':'Unsupported','backend':'kubernetes','release':release},owner['access_token'],expected=422)
    print('initial_release_catalog_is_scoped_and_rejects_unknown_versions_and_profiles=ok')

    disabled = replica({'platform_concurrent_runs':4,'organisation_concurrent_runs':3,'user_concurrent_runs':2},release_selection=False)
    assert request(base+'/provisioning-releases',token=owner['access_token'],origin_override=disabled)==[]
    request(base+'/instances',{'name':'Disabled','backend':'compose','release':release},owner['access_token'],origin_override=disabled,expected=422)
    selected=create()['id']
    assert current(selected)['provisioning_release']==release and current(selected)['current_release'] is None
    rejected(f"UPDATE management_instances SET provisioning_release=NULL WHERE id='{selected}'")
    sql("UPDATE management_releases SET refreshed_at=now()-interval '6 minutes'")
    assert request(base+'/provisioning-releases',token=token)==[]
    create(expected=422);enqueue(selected,expected=422)
    sync()
    enqueue(selected,expected=422,release=release)
    queued=enqueue(selected)
    rejected(f"UPDATE management_operations SET arguments=arguments-'release' WHERE id='{queued['id']}'")
    rejected(f"UPDATE management_operations SET state='running',lease_token='{uid()}' WHERE id='{queued['id']}'")
    claim(expected=422);claim(expected=422,provisioning_release_version=2)
    assert request(path(selected),token=token)[0]['state']=='queued'
    sync(False)
    assert claim(**contract) is None
    assert request(path(selected),token=token)[0]['result']['error_code']=='release_authorization_revoked'
    assert current(selected)['state']=='pending'
    print('initial_release_is_frozen_and_dispatch_rechecks_freshness_withdrawal_and_operator_version=ok')

    sync(); queued=enqueue(selected); op=claim(**contract)
    assert op['id']==queued['id'] and op['arguments']['release']==release and op['instance']['provisioning_release']==release
    good=dict(receipt_id=op['id'],instance_id=selected,release=release)
    audit_before=len(request(base+'/audit',token=token))
    for bad in [{}, {**good,'release':'f'*64}, {**good,'instance_id':uid()}, {**good,'receipt_id':uid()}]:
        complete(op,bad,expected=422)
    assert len(request(base+'/audit',token=token))==audit_before
    assert current(selected)['current_release'] is None and current(selected)['state']=='provisioning'
    sync(False)
    complete(op,good);complete(op,good)
    complete(op,{**good,'release':'f'*64},expected=409)
    assert current(selected)['current_release']==release and current(selected)['state']=='ready'
    assert sql('SELECT count(*) FROM management_release_qualifications').strip()=='0'
    print('initial_release_completion_is_atomic_idempotent_and_grants_no_update_qualification=ok')

    sync(); interrupted=create('Interrupted')['id'];enqueue(interrupted);op=claim(**contract)
    sql(f"UPDATE management_operations SET lease_expires_at=now()-interval '1 minute' WHERE id='{op['id']}'")
    request('/management/runner/v1/pending',{'backend':'compose'},broker=True,expected=422)
    pending=request('/management/runner/v1/pending',{'backend':'compose',**contract},broker=True)
    assert pending[0]['lease_token']==op['lease_token'] and pending[0]['arguments']==op['arguments']
    request(path(interrupted)+'/'+op['id']+'/recover',{'mode':'stop_if_unproven'},token)
    sync(False)
    pending=request('/management/runner/v1/pending',{'backend':'compose',**contract},broker=True)
    assert pending[0]['recovery_authorized'] is True
    assert claim(**contract) is None
    complete(op,{'error_code':'operation_interrupted_resources_stopped','instance_state':'stopped'},succeeded=False)
    assert current(interrupted)['state']=='stopped' and current(interrupted)['current_release'] is None
    print('withdrawal_allows_explicit_stop_recovery_without_unsafe_replay=ok')

    legacy=request(base+'/instances',{'name':'Legacy client','backend':'compose'},owner['access_token'])['id']
    enqueue(legacy);op=claim();complete(op,{'receipt_id':op['id']})
    assert current(legacy)['state']=='ready' and current(legacy)['provisioning_release'] is None
    print('optional_initial_release_preserves_legacy_clients_and_operators=ok')
    print('enterprise_provisioning_release_http_acceptance=passed')
