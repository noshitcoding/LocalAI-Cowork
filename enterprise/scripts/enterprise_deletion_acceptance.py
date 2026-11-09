"""Real management HTTP/SQL checks; infrastructure receipts are explicitly simulated."""
import uuid


def run(request, token, user, sql):
    owner = user('deletion-owner@enterprise.test')['access_token']
    outsider = user('deletion-outsider@enterprise.test')['access_token']
    org = request('/management/v1/organisations', {'name': 'Disposable deletion acceptance'}, token)['id']
    base = f'/management/v1/organisations/{org}'
    request(base + '/members', {'email': 'deletion-owner@enterprise.test', 'role': 'member'}, token)
    fixture = request(base + '/instances', {'name': 'Deletion fixture', 'backend': 'compose'}, owner)
    instance = fixture['id']
    path = base + f'/instances/{instance}'
    def current():
        return next(row for row in request(base + '/instances', token=owner) if row['id'] == instance)
    def action(kind, **extra):
        return {'action': kind, 'expected_revision': current()['revision'], 'idempotency_key': str(uuid.uuid4()), **extra}
    def claim():
        return request('/management/runner/v1/claim', {'backend': 'compose'}, broker=True)
    def complete(step, succeeded=True, result=None, expected=200):
        return request(f"/management/runner/v1/operations/{step['id']}/complete", {'lease_token': step['lease_token'], 'succeeded': succeeded, 'result': result or {'receipt_id': step['id']}}, broker=True, expected=expected)
    assert 'delete' in current()['available_actions']
    consent = action('delete', confirmation_name='Deletion fixture', purge_backups=True)
    request(path + '/operations', consent, outsider, expected=404)
    for invalid in [{}, {'confirmation_name': 'Deletion fixture'}, {'confirmation_name': 'Deletion fixture ', 'purge_backups': True}, {'confirmation_name': 'Deletion fixture', 'purge_backups': True, 'release': 'x'}, {'confirmation_name': 'Deletion fixture', 'purge_backups': True, 'delete_backup_ids': []}]:
        request(path + '/operations', action('delete', **invalid), owner, expected=422)
    request(path + '/operations', action('provision', confirmation_name='Deletion fixture', purge_backups=True), owner, expected=422)
    request(path + '/operations', action('provision'), owner)
    complete(claim())
    request(path + '/operations', action('stop'), owner)
    complete(claim())
    request(path + '/operations', action('backup'), owner)
    backup = claim()
    complete(backup, result={'receipt_id': backup['id'], 'backup_id': backup['id'], 'backup_sha256': 'a' * 64, 'backup_bytes': 1024, 'manifest_version': 1})
    consent = action('delete', confirmation_name='Deletion fixture', purge_backups=True)
    requested = request(path + '/operations', consent, owner)
    assert request(path + '/operations', consent, owner)['id'] == requested['id']
    step = claim()
    assert step['arguments']['delete_backup_ids'] == [backup['id']]
    assert current()['state'] == 'deleting' and current()['available_actions'] == []
    request(path + '/launch', {}, owner, expected=404)
    complete(step, expected=422)
    complete(step, succeeded=False, result={'error_code': 'interrupted'}, expected=422)
    assert current()['state'] == 'deleting'
    report = f"/management/runner/v1/operations/{step['id']}/reconciliation"
    request(report, {'lease_token': step['lease_token'], 'reason': 'operation_completion_unproven'}, broker=True)
    recover = path + f"/operations/{step['id']}/recover"
    request(recover, {'mode': 'stop_if_unproven'}, owner, expected=422)
    request(recover, {'mode': 'continue_deletion', 'confirmation_name': 'wrong', 'purge_backups': True}, owner, expected=422)
    resume = {'mode': 'continue_deletion', 'confirmation_name': 'Deletion fixture', 'purge_backups': True}
    request(recover, resume, outsider, expected=404)
    def pending():
        return next(row for row in request('/management/runner/v1/pending', {'backend': 'compose'}, broker=True) if row['id'] == step['id'])
    assert pending()['recovery_authorized'] is False
    request(recover, resume, owner)
    # Revocation is checked again when dispatching an approved recovery.
    request(base + '/members', {'email': 'deletion-owner@enterprise.test', 'role': 'auditor'}, token)
    assert pending()['recovery_authorized'] is False
    request(recover, resume, owner, expected=404)
    request(base + '/members', {'email': 'deletion-owner@enterprise.test', 'role': 'member'}, token)
    request(recover, resume, owner)
    assert pending()['recovery_authorized'] is True
    assert pending()['recovery_authorized'] is False
    assert claim() is None
    request(recover, resume, owner)
    assert pending()['recovery_authorized'] is True
    assert pending()['recovery_authorized'] is False
    receipt = {'receipt_id': step['id'], 'instance_id': instance, 'deletion_verified': True, 'deletion_inventory_sha256': 'b' * 64, 'deleted_backup_count': 1}
    for invalid in [{'instance_id': str(uuid.uuid4())}, {'deleted_backup_count': 0}, {'deletion_verified': False}, {'deletion_inventory_sha256': 'invalid'}]:
        complete(step, result={**receipt, **invalid}, expected=422)
    complete(step, result=receipt)
    complete(step, result=receipt)
    assert not request(base + '/instances', token=owner)
    request(path + '/launch', {}, owner, expected=404)
    request(path + '/backups', token=owner, expected=404)
    assert sql(f"SELECT count(*) FROM management_backups WHERE instance_id='{instance}' AND purged_at IS NOT NULL;").strip() == '1'
    events = request(base + '/audit', token=token)
    assert any(e['payload'].get('metadata', {}).get('deletion') == receipt for e in events), 'deletion evidence missing from audit'
    print('deletion_http_name_consent_scoping_receipt_purge_and_one_shot_recovery=ok (simulated infrastructure receipts)')
