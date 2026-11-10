-- Only an explicit authenticated link or creation of a new account establishes
-- an external identity. A matching email is not evidence of account ownership.
ALTER TABLE oidc_identities ADD COLUMN binding_method TEXT NOT NULL DEFAULT 'legacy_unverified'
    CHECK (binding_method IN ('legacy_unverified','provisioned','authenticated_session'));

UPDATE oidc_identities i SET binding_method='authenticated_session'
WHERE EXISTS (
    SELECT 1 FROM audit_events a
    WHERE a.actor_user_id=i.user_id AND a.action='auth.oidc.link'
      AND a.metadata->>'issuer'=i.issuer
      AND a.metadata->>'subject_hash'=encode(sha256(convert_to(i.subject,'UTF8')),'hex')
);
-- The old provisioning path inserted both rows in one transaction, using the
-- same transaction timestamp. It never changed the user's creation timestamp.
UPDATE oidc_identities i SET binding_method='provisioned'
FROM users u WHERE u.id=i.user_id AND u.created_at=i.created_at
    AND i.binding_method='legacy_unverified';

-- Old email-based links without evidence cannot retain already-issued access.
UPDATE auth_sessions SET revoked_at=now(),revoke_reason='oidc_binding_requires_verification'
WHERE revoked_at IS NULL AND user_id IN (
    SELECT user_id FROM oidc_identities WHERE binding_method='legacy_unverified'
);
DELETE FROM native_authorization_codes WHERE user_id IN (
    SELECT user_id FROM oidc_identities WHERE binding_method='legacy_unverified'
);

-- Old pending links have no session proof and must be started again.
ALTER TABLE oidc_authorizations ADD COLUMN link_session_id UUID
    REFERENCES auth_sessions(id) ON DELETE CASCADE;
UPDATE oidc_authorizations SET consumed_at=now()
WHERE link_user_id IS NOT NULL AND consumed_at IS NULL;
