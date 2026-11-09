"""Real PostgreSQL/HTTP broker accounting. Executors are simulated explicitly."""
import concurrent.futures
import hashlib
import hmac
import json
from enterprise_execution_coverage_acceptance import runtime
import uuid


def run(request, token, user, sql, broker_key, replica):
    uid = lambda: str(uuid.uuid4())
    people = [user(f'capacity-{n}@enterprise.test') for n in range(3)]
    orgs = [request('/management/v1/organisations', {'name': f'Capacity {n}'}, token)['id'] for n in range(2)]
    instances = []
    for org in orgs:
        base = f'/management/v1/organisations/{org}'
        for n in range(3):
            request(base + '/members', {'email': f'capacity-{n}@enterprise.test', 'role': 'member'}, token)
        group = []
        for n in range(2):
            instance = request(base + '/instances', {'name': f'Capacity instance {n}', 'backend': 'compose'}, people[n]['access_token'])['id']
            sql(f"UPDATE management_instances SET state='ready',execution_evidence=jsonb_build_object('active_epoch',execution_epoch,'runtime','{json.dumps(runtime())}'::jsonb) WHERE id='{instance}';")
            sql(f"UPDATE management_operations SET state='succeeded' WHERE instance_id='{instance}';")
            epoch = sql(f"SELECT execution_epoch FROM management_instances WHERE id='{instance}';").strip()
            key = hmac.new(broker_key.encode(), f'cowork-federation-v1\0{org}\0{instance}'.encode(), hashlib.sha256).hexdigest()
            for member in people:
                if member['user_id'] == people[n]['user_id']:
                    continue
                request(base + f'/instances/{instance}/members', {'user_id': member['user_id'], 'role': 'member'}, token)
            group.append({'organisation_id': org, 'instance_id': instance, 'execution_epoch': epoch, 'key': key})
        instances.append(group)
    def body(instance, person=0):
        return {k: v for k, v in instance.items() if k != 'key'} | {'run_id': uid(), 'user_id': people[person]['user_id'], 'target': {'kind': 'server_linux', 'pool_id': None}}
    def reserve(instance, value=None, person=0, **kwargs):
        value = value or body(instance, person)
        result = request('/management/runner/v1/execution-reserve', value, instance_key=instance['key'], **kwargs)
        return value, result
    def release(instance, value, decision, completion=None, **kwargs):
        completion = completion or {k: value[k] for k in ['organisation_id', 'instance_id', 'execution_epoch', 'run_id']} | {'reservation_id': decision['reservation_id'], 'completion_id': uid(), 'outcome': 'completed', 'execution_stopped': True}
        result = request('/management/runner/v1/execution-release', completion, instance_key=instance['key'], **kwargs)
        return completion, result
    def capacity(org, who=token, expected=200):
        return request(f'/management/v1/organisations/{org}/execution-capacity', token=who, expected=expected)
    def update(org, organisation=3, member=2, expected=200, **extra):
        return request(f'/management/v1/organisations/{org}/execution-capacity', {'expected_revision': capacity(org)['budget']['revision'], 'organisation_concurrent_runs': organisation, 'user_concurrent_runs': member, **extra}, token, 'PUT', expected=expected)
    a, b = instances[0]
    c, d = instances[1]
    capacity(orgs[0], people[0]['access_token'], 404)
    assert capacity(orgs[0])['budget']['revision'] == 0
    assert sql('SELECT count(*) FROM management_execution_limits;').strip() == '0', 'GET must not change budget state'
    update(orgs[0], organisation=999, expected=422)
    update(orgs[0], member=0, expected=422)
    changed = update(orgs[0])
    update(orgs[0], expected=409, expected_revision=changed['revision'] - 1)
    # Existing session credentials cannot substitute for a bound instance key.
    request('/management/runner/v1/execution-reserve', body(a), people[0]['access_token'], expected=401)
    request('/management/runner/v1/execution-reserve', body(c), instance_key=a['key'], expected=401)
    reserve(a, {**body(a), 'execution_epoch': uid()}, expected=409)
    reserve(a, {**body(a), 'user_id': uid()}, expected=404)
    reserve(a, {**body(a), 'expires_at': 'yesterday'}, expected=422)
    work = [(instances[0][n % 2], body(instances[0][n % 2], n % 2)) for n in range(40)]
    with concurrent.futures.ThreadPoolExecutor(max_workers=12) as pool:
        outcomes = list(pool.map(lambda pair: (pair[0], *reserve(*pair)), work))
    admitted = [row for row in outcomes if row[2]['admitted']]
    assert len(admitted) == 3, len(admitted)
    assert capacity(orgs[0])['held_runs'] == 3
    assert all(not row[2]['admitted'] or row[2]['reservation_id'] for row in outcomes)
    extra = reserve(c)
    assert extra[1]['admitted']
    assert reserve(d, person=1)[1]['reason'] == 'platform'
    replica({'revision':2,'platform_concurrent_runs':1,'organisation_concurrent_runs':1,'user_concurrent_runs':1}, 'outstanding reservations exceed')
    for instance, value, decision in admitted:
        assert reserve(instance, value)[1] == decision
        reserve(instance, {**value, 'user_id': people[2]['user_id']}, expected=409)
        reserve(instance, {**value, 'target': {'kind': 'personal_device', 'device_id': uid()}}, expected=409)
    update(orgs[0], organisation=2, expected=409)
    # Arbitrary age and client disconnect have no authority to free a slot.
    sql("UPDATE management_execution_reservations SET created_at=now()-interval '1 year' WHERE state='held';")
    assert capacity(orgs[0])['held_runs'] == 3
    assert reserve(d, person=1)[1]['reason'] == 'platform'
    for instance, value, decision in admitted + [(c, *extra)]:
        completion, _ = release(instance, value, decision)
        release(instance, value, decision, completion)
        release(instance, value, decision, {**completion, 'completion_id': uid()}, expected=409)
        reserve(instance, value, expected=409)
    assert capacity(orgs[0])['held_runs'] == 0
    print('execution_capacity_atomic_platform_organisation_limits_idempotency_and_no_timeout_release=ok')
    update(orgs[0], member=1)
    first = reserve(a)
    assert first[1]['admitted']
    assert reserve(b)[1]['reason'] == 'user'
    release(a, *first)
    update(orgs[0], member=2)
    first, second = reserve(a), reserve(a, person=1)
    assert first[1]['admitted'] and second[1]['admitted']
    assert reserve(a, person=2)[1]['reason'] == 'instance'
    # Stopping server infrastructure does not settle already assigned (possibly
    # offline) executions. Even a quiescent fleet must retain the old policy.
    org = orgs[0]
    sql(f"UPDATE management_operations SET state='succeeded' WHERE organisation_id='{org}'; UPDATE management_instances SET state='stopped' WHERE organisation_id='{org}';")
    policy = json.loads(sql(f"SELECT policy FROM management_organisations WHERE id='{org}';"))
    policy['revision'] += 1
    policy['limits']['concurrent_runs'] = 1
    request(f'/management/v1/organisations/{org}/policy', policy, token, 'PUT', expected=409)
    assert capacity(org)['held_runs'] == 2
    assert int(sql(f"SELECT (policy->>'revision')::bigint FROM management_organisations WHERE id='{org}';")) == policy['revision'] - 1
    sql(f"UPDATE management_instances SET state='ready' WHERE organisation_id='{org}';")
    completion = {k: first[0][k] for k in ['organisation_id', 'instance_id', 'execution_epoch', 'run_id']} | {'reservation_id': first[1]['reservation_id'], 'completion_id': uid(), 'outcome': 'completed', 'execution_stopped': True}
    release(a, *first, {**completion, 'execution_stopped': False}, expected=422)
    release(a, *first, {**completion, 'instance_id': b['instance_id']}, expected=401)
    release(a, *first, {**completion, 'reservation_id': uid()}, expected=404)
    # Revocation forbids fresh work; an old authenticated completion still settles
    # exactly its prior reservation after deletion or a generation switch.
    request(f'/management/v1/organisations/{orgs[0]}/members', {'email': 'capacity-0@enterprise.test', 'role': 'auditor'}, token)
    reserve(a, expected=404)
    sql(f"UPDATE management_instances SET state='deleted',execution_epoch='{uid()}' WHERE id='{a['instance_id']}';")
    release(a, *first, completion)
    release(a, *second)
    assert capacity(orgs[0])['held_runs'] == 0
    sql(f"UPDATE management_instances SET state='stopped' WHERE organisation_id='{org}' AND state<>'deleted';")
    request(f'/management/v1/organisations/{org}/policy', policy, token, 'PUT')
    sql(f"UPDATE management_instances SET state='ready' WHERE organisation_id='{org}' AND state<>'deleted';")
    print('stopped_fleet_policy_changes_wait_for_outstanding_execution_settlement=ok')
    # Ensure every scope other than this exact receipt remains intact.
    assert sql("SELECT count(*) FROM management_execution_reservations WHERE state='held';").strip() == '0'
    events = request(f'/management/v1/organisations/{orgs[0]}/audit', token=token)
    assert any(row['action'] == 'execution.reserve' for row in events)
    assert any(row['action'] == 'execution.release' for row in events)
    assert any(row['action'] == 'execution.budget.update' for row in events)
    print('execution_capacity_user_instance_boundaries_revocation_generation_fencing_and_scoped_release=ok')

    replica({'revision':1,'platform_concurrent_runs':5,'organisation_concurrent_runs':4,'user_concurrent_runs':2}, 'conflicting execution ceiling')
    replica({'revision':2,'platform_concurrent_runs':4,'organisation_concurrent_runs':3,'user_concurrent_runs':2})
    request('/readyz',expected=503)
    capacity(orgs[0], expected=503)
    reserve(b, person=1, expected=503)
    replica({'revision':1,'platform_concurrent_runs':4,'organisation_concurrent_runs':3,'user_concurrent_runs':2}, 'execution ceiling configuration is stale')
    print('management_replicas_reject_conflicting_stale_or_underreserved_ceiling_revisions=ok')
