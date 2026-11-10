"""Organisation SSO scenarios for the disposable, signed-provider OIDC harness."""
import json
import hashlib
import hmac
import time
from concurrent.futures import ThreadPoolExecutor


def run(root, env, start, stop, call, sql, flow, finish, admin, first, provider_factory, binary, recovery_code):
    operator = admin['access_token']
    org = call('/management/v1/organisations', {'name': 'SSO Alpha'}, operator)['id']
    other = call('/management/v1/organisations', {'name': 'SSO Beta'}, operator)['id']
    # Keep the real password/TOTP enrollment, but model a restored session whose
    # MFA proof is no longer current. Admin membership alone cannot save SSO.
    sql(f"UPDATE auth_sessions SET second_factor_verified_at=NULL WHERE id='{admin['session_id']}'")
    call(f'/management/v1/organisations/{org}/sso', dict(expected_revision=0, enabled=False,
         provider_id=None, auto_provision=False, group_mappings=[]), operator, 403, method='PUT')
    assert sql(f"SELECT count(*) FROM management_sso WHERE organisation_id='{org}'") == '0'
    assert call('/api/v1/auth/assurance/verify', dict(code=recovery_code), operator)['session_verified']
    print('unverified_platform_admin_cannot_configure_organisation_sso=ok')
    second_root = root / 'second-provider'
    second_root.mkdir()
    second = provider_factory(second_root)
    catalog = root / 'providers.json'

    def entry(provider, provider_id, organisation, claim):
        secret = root / (provider_id + '-secret')
        secret.write_text(provider.secret)
        return dict(id=provider_id, label=provider_id, organisation_ids=[organisation], issuer=provider.issuer,
                    client_id=provider.client, client_secret_file=str(secret), public_origin=env['COWORK_PUBLIC_ORIGIN'], groups_claim=claim, scopes=['groups'])

    entries = [entry(first, 'alpha', org, 'groups'), entry(second, 'beta', other, 'teams')]
    catalog.write_text(json.dumps(entries))
    env['COWORK_ORGANISATION_OIDC_PROVIDERS_FILE'] = str(catalog)
    stop()
    start(binary)

    def path(organisation=org):
        return f'/management/v1/organisations/{organisation}/sso'

    def save(organisation=org, **overrides):
        old = call(path(organisation), token=operator)
        body = {key: old[key] for key in ['enabled', 'provider_id', 'auto_provision', 'group_mappings']}
        body.update(expected_revision=old['revision'])
        body.update(overrides)
        return call(path(organisation), body, operator, method='PUT')

    def login(subject, groups, organisation=org, provider=first, **kwargs):
        return flow(dict(sub=subject, claims={('groups' if provider is first else 'teams'): groups}), org=organisation, identity_provider=provider, **kwargs)

    def role(user, organisation=org):
        return sql(f"SELECT role FROM management_members WHERE organisation_id='{organisation}' AND user_id='{user}'")

    try:
        assert [p['id'] for p in call(path(), token=operator)['providers']] == ['alpha']
        assert [p['id'] for p in call(path(other), token=operator)['providers']] == ['beta']
        body = dict(expected_revision=0, enabled=True, provider_id='beta', auto_provision=True, group_mappings=[])
        call(path(), body, operator, 422, method='PUT')
        call(path(), {**body, 'provider_id': 'alpha', 'issuer': 'http://host.internal'}, operator, 422, method='PUT')
        mappings = [dict(group='staff', role='member'), dict(group='audit', role='auditor'), dict(group='admins', role='admin')]
        config = save(enabled=True, provider_id='alpha', auto_provision=True, group_mappings=mappings)
        save(other, enabled=True, provider_id='beta', auto_provision=True, group_mappings=[dict(group='beta-staff', role='member')])
        public = call(f'/api/v1/auth/organisations/{org}/oidc/config')
        assert public['organisation_id'] == org and public['enabled']
        assert 'client_secret' not in json.dumps(config)
        assert first.secret not in json.dumps(config)
        call(path(), {**body, 'provider_id': 'alpha'}, operator, 409, method='PUT')
        print('operator_catalog_is_organisation_scoped_and_revision_checked=ok')

        finish(login('unmapped', []), 401)
        assert sql("SELECT count(*) FROM users WHERE email='unmapped@example.test'") == '0'
        finish(flow(dict(sub='forged', claims={'groups': ['admins']}, bad_signature=True), org=org), 401)
        assert sql("SELECT count(*) FROM users WHERE email='forged@example.test'") == '0'
        user = finish(login('sso-staff', ['staff'], native=True))
        token, uid = user['access_token'], user['user_id']
        assert role(uid) == 'member'
        assert sql(f"SELECT platform_admin FROM users WHERE id='{uid}'") == 'f'
        assert sql(f"SELECT content_admin FROM management_members WHERE user_id='{uid}'") == 'f'
        assert [o['id'] for o in call('/management/v1/organisations', token=token)['items']] == [org]
        call(path(), token=token, expected=404)
        call(path(), dict(expected_revision=config['revision'], enabled=False, provider_id=None,
             auto_provision=False, group_mappings=[]), token, 404, method='PUT')
        call(f'/management/v1/organisations/{other}/members', token=token, expected=404)
        beta = finish(login('beta-user', ['beta-staff'], other, second))
        assert role(beta['user_id'], other) == 'member'
        assert role(beta['user_id']) == ''
        # A same-email identity from another issuer must not take over Alpha.
        finish(flow(dict(sub='foreign', email='sso-staff@example.test', claims={'teams': ['beta-staff']}), org=other, identity_provider=second), 409)
        print('signed_groups_admit_only_the_selected_organisation_without_email_linking=ok')

        instance = call(f'/management/v1/organisations/{org}/instances', dict(name='Membership check', backend='compose'), token)['id']
        # Mark the metadata ready to exercise the actual broker ACL, without Docker.
        sql(f"UPDATE management_instances SET state='ready' WHERE id='{instance}'")
        broker = (root / 'broker-key').read_text()
        access = dict(organisation_id=org, instance_id=instance, user_id=uid)
        instance_key = hmac.new(broker.encode(), f'cowork-federation-v1\0{org}\0{instance}'.encode(), hashlib.sha256).hexdigest()
        broker_headers = {'x-cowork-instance-key': instance_key}
        assert call('/management/runner/v1/instance-access', access, extra_headers=broker_headers)['allowed']
        finish(login('sso-staff', []), 401)
        assert role(uid) == 'suspended'
        assert not call('/management/runner/v1/instance-access', access, extra_headers=broker_headers)['allowed']
        call(f'/management/v1/organisations/{org}/instances', token=token, expected=404)
        assert call('/management/v1/organisations', token=token)['items'] == []
        finish(login('sso-staff', ['audit', 'staff', 'admins']))
        assert role(uid) == 'admin'
        finish(login('sso-staff', ['audit']))
        assert role(uid) == 'auditor'
        assert not call('/management/runner/v1/instance-access', access, extra_headers=broker_headers)['allowed']
        call(f'/management/v1/organisations/{org}/instances', dict(name='Denied', backend='compose'), token, 404)
        finish(login('sso-staff', 'staff'), 401)
        assert role(uid) == 'suspended'
        print('group_removal_and_downgrade_revoke_existing_session_and_broker_access=ok')

        finish(login('sso-staff', ['staff']))
        pending = login('pending-policy', ['staff'])
        save(auto_provision=False)
        assert role(uid) == 'suspended'
        finish(pending, 401)
        finish(login('no-jit', ['staff']), 401)
        assert sql("SELECT count(*) FROM users WHERE email='no-jit@example.test'") == '0'
        finish(login('sso-staff', ['staff']))
        assert role(uid) == 'member'
        save(enabled=False)
        assert role(uid) == 'suspended'
        call(f'/api/v1/auth/organisations/{org}/oidc/config', expected=404)
        save(enabled=True, auto_provision=True)
        print('policy_changes_suspend_managed_memberships_and_invalidate_pending_callbacks=ok')

        # Explicit manual assignment is the only way to leave group management.
        call(f'/management/v1/organisations/{org}/members', dict(email='sso-staff@example.test', role='member'), operator)
        assert sql(f"SELECT sso_provider_id IS NULL FROM management_members WHERE user_id='{uid}'") == 't'
        finish(login('sso-staff', ['admins']))
        assert role(uid) == 'member'
        finish(login('sso-staff', []))
        assert role(uid) == 'member'
        managed = finish(login('managed-admin', ['admins']))
        # Do not let the last manual administrator disappear behind an SSO admin.
        call(f'/management/v1/organisations/{org}/members', dict(email='admin@example.test', role='member'), operator, 422)
        assert role(admin['user_id']) == 'admin'
        save(auto_provision=True)
        assert role(uid) == 'member' and role(managed['user_id']) == 'suspended'
        print('manual_roles_are_preserved_and_manual_admin_recovery_cannot_be_removed=ok')

        finish(login('managed-admin', ['admins']))
        # Explicit links preserve manual membership and still require the original session.
        invite = call(f'/management/v1/organisations/{org}/invitations', dict(email='manual@example.test', role='member'), operator, 201)
        manual = call('/api/v1/auth/organisation-invitations/register', dict(token=invite['token'],display_name='Manual',password='Manual-password-long-42!',device_id=str(__import__('uuid').uuid4())), expected=201)
        linked = finish(flow(dict(sub='manual-external',email='different@example.test',claims={'groups':['admins']}),manual['access_token'],org=org))
        assert linked['user_id']==manual['user_id'] and role(manual['user_id'])=='member'
        pending_link = flow(dict(sub='expired-link',claims={'groups':[]}),manual['access_token'],org=org)
        call('/api/v1/auth/logout',{},manual['access_token'],204)
        finish(pending_link,401)
        assert sql("SELECT count(*) FROM oidc_identities WHERE subject='expired-link'")=='0'
        assert role(manual['user_id'])=='member'
        # A suspended managed account cannot reactivate itself through an invitation.
        finish(login('managed-admin',[]),401)
        invite = call(f'/management/v1/organisations/{org}/invitations',dict(email='managed-admin@example.test',role='admin'),operator,201)
        call('/management/v1/invitations/accept',dict(token=invite['token']),managed['access_token'],409)
        assert role(managed['user_id'])=='suspended'
        # Failed security audit writes must roll back membership and new identity together.
        sql("CREATE FUNCTION fail_sso_audit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.action LIKE 'sso.%' THEN RAISE EXCEPTION 'acceptance audit unavailable'; END IF; RETURN NEW; END $$; CREATE TRIGGER acceptance_sso_audit BEFORE INSERT ON management_audit FOR EACH ROW EXECUTE FUNCTION fail_sso_audit();")
        finish(login('audit-blocked',['staff']),500)
        assert sql("SELECT count(*) FROM users WHERE email='audit-blocked@example.test'")=='0'
        assert sql("SELECT count(*) FROM oidc_identities WHERE subject='audit-blocked'")=='0'
        current=call(path(),token=operator)
        body={key:current[key] for key in ['enabled','provider_id','auto_provision','group_mappings']}
        call(path(),{**body,'expected_revision':current['revision'],'enabled':False},operator,500,method='PUT')
        assert call(path(),token=operator)['revision']==current['revision']
        sql('DROP TRIGGER acceptance_sso_audit ON management_audit; DROP FUNCTION fail_sso_audit();')
        finish(login('managed-admin',['admins']))
        # Requests already waiting on the organisation lock must lose authority
        # when the revocation commits; a stale preflight check is insufficient.
        with ThreadPoolExecutor(max_workers=1) as executor:
            revocation=executor.submit(sql,f"BEGIN; SELECT id FROM management_organisations WHERE id='{org}' FOR UPDATE; UPDATE management_members SET role='suspended' WHERE organisation_id='{org}' AND user_id='{managed['user_id']}'; SELECT pg_sleep(1.0) /* sso-race */; COMMIT;")
            for _ in range(100):
                if sql("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE wait_event='PgSleep' AND query LIKE '%sso-race%')")=='t': break
                time.sleep(.01)
            else: raise AssertionError('revocation lock not observed')
            call(f'/management/v1/organisations/{org}/members',dict(email='managed-admin@example.test',role='admin'),managed['access_token'],404)
            revocation.result()
        assert role(managed['user_id'])=='suspended'
        finish(login('managed-admin',['admins']))
        print('queued_membership_mutation_cannot_outlive_sso_revocation=ok')
        print('account_linking_invitation_suspension_and_audit_atomicity=ok')
        pending = login('provider-changed', ['staff'])
        stop()
        catalog.write_text(json.dumps([entries[1]]))
        start(binary)
        assert role(managed['user_id']) == 'suspended' and role(uid) == 'member'
        assert call(path(), token=operator)['available'] is False
        assert call(path(), token=operator)['enabled'] is False
        assert call(path(other), token=operator)['available'] is True
        finish(pending, 404)
        assert role(beta['user_id'], other) == 'member'
        print('provider_withdrawal_revokes_only_its_managed_memberships_before_serving=ok')

        stop()
        catalog.write_text(json.dumps(entries))
        start(binary)
        assert role(managed['user_id']) == 'suspended'
        call(f'/api/v1/auth/organisations/{org}/oidc/config', expected=404)
        save(enabled=True)
        finish(login('managed-admin', ['admins']))
        assert role(managed['user_id']) == 'admin'
        # Ensure provider secrets, raw subjects and signed group claims do not leak.
        audit = sql('SELECT payload FROM management_audit')
        assert first.secret not in audit and second.secret not in audit
        assert 'managed-admin@example.test' not in audit
        print('organisation_sso_database_acceptance=passed')
    finally:
        second.close()
