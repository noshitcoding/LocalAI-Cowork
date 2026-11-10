//! Trusted adapter hooks: actual password verification and durable invitation admission.
use super::*;

/// Not deserializable or caller-constructible. The password was checked against
/// this actual subject while holding the same installation writer transaction.
pub struct VerifiedPassword {
    identity: Identity,
}
impl VerifiedPassword {
    pub fn identity(&self) -> &Identity {
        &self.identity
    }
}
impl IdentityStore {
    async fn android_login_row(
        &self,
        tx: &mut Transaction<'_, Any>,
        name: &str,
    ) -> Result<Option<sqlx::any::AnyRow>> {
        self.lock_installation(tx).await?;
        if self.android_auth_security && name.contains('@') {
            let email = name.trim().to_lowercase();
            if !(3..=200).contains(&email.len())
                || email.starts_with('@')
                || email.ends_with('@')
                || email.chars().any(char::is_control)
            {
                return Err(IdentityError::Unauthenticated);
            }
            // Only the original, consumed server invitation binds an email to
            // its newly created subject. A caller UUID or existing username is
            // never adopted as an email account.
            let mut rows=sqlx::query("SELECT s.* FROM identity_subjects s WHERE s.id IN (SELECT accepted_user_id FROM android_auth_invitations WHERE email=$1 AND accepted_at>0 AND accepted_user_id IS NOT NULL) LIMIT 2")
                .bind(email).fetch_all(&mut **tx).await?;
            if rows.len() > 1 {
                return Err(IdentityError::Corrupt);
            }
            return Ok(rows.pop());
        }
        let name = username(name).map_err(|_| IdentityError::Unauthenticated)?;
        Ok(
            sqlx::query("SELECT * FROM identity_subjects WHERE username=$1")
                .bind(name)
                .fetch_optional(&mut **tx)
                .await?,
        )
    }
    /// Metadata only. Real passkey challenge verification and credential CAS
    /// must precede any session issue; knowing this subject is not proof.
    pub async fn android_subject_for_login_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        name: &str,
    ) -> Result<Identity> {
        let row = self
            .android_login_row(tx, name)
            .await?
            .ok_or(IdentityError::Unauthenticated)?;
        let actual = identity(&row)?;
        if !actual.active {
            return Err(IdentityError::Unauthenticated);
        }
        Ok(actual)
    }
    pub async fn android_identity_email_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
    ) -> Result<String> {
        let actual = self.validate_principal_in_transaction(tx, actor).await?;
        if !self.android_auth_security {
            return Ok(actual.username);
        }
        let rows=sqlx::query_scalar::<_,String>("SELECT DISTINCT email FROM android_auth_invitations WHERE accepted_user_id=$1 AND accepted_at>0 LIMIT 2")
            .bind(actual.id.to_string()).fetch_all(&mut **tx).await?;
        if rows.len() > 1 {
            return Err(IdentityError::Corrupt);
        }
        Ok(rows.into_iter().next().unwrap_or(actual.username))
    }
    pub(super) async fn revoke_android_metadata(
        &self,
        tx: &mut Transaction<'_, Any>,
        owner: Uuid,
        reason: &str,
    ) -> Result<()> {
        if !self.android_auth_security {
            return Ok(());
        }
        let current = self.sample(tx).await?;
        sqlx::query("UPDATE android_auth_session_metadata SET revoked_at=$1,revoke_reason=$2 WHERE owner_id=$3 AND revoked_at=0 AND session_id IN (SELECT id FROM identity_sessions WHERE identity_id=$3 AND revoked=1)")
            .bind(current).bind(reason).bind(owner.to_string()).execute(&mut **tx).await?;
        Ok(())
    }
    pub(super) async fn require_android_factor_verified(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
    ) -> Result<()> {
        if !self.android_auth_security {
            return Ok(());
        }
        let current = self.sample(tx).await?;
        let enabled: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM android_auth_totp WHERE owner_id=$1 AND enabled=1",
        )
        .bind(actor.identity_id.to_string())
        .fetch_one(&mut **tx)
        .await?;
        if enabled == 0 {
            return Ok(());
        }
        let proven:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_auth_session_metadata m WHERE m.owner_id=$1 AND m.session_id=$2 AND m.revoked_at=0 AND m.verified_at<=$3 AND m.verified_at>$4 AND ((m.factor_method IN ('totp','recovery') AND EXISTS(SELECT 1 FROM android_auth_totp t WHERE t.owner_id=$1 AND t.enabled=1 AND t.revision=m.factor_revision)) OR (m.factor_method='passkey' AND EXISTS(SELECT 1 FROM android_auth_passkey_proofs p JOIN android_passkeys k ON k.id=p.passkey_id AND k.identity_id=p.owner_id WHERE p.session_id=m.session_id AND p.owner_id=m.owner_id AND p.credential_revision=m.factor_revision AND k.revision>=p.credential_revision AND p.verified_at=m.verified_at)))")
            .bind(actor.identity_id.to_string()).bind(actor.session_id.to_string()).bind(current).bind(current.saturating_sub(12*60*60*1000)).fetch_one(&mut **tx).await?;
        if proven != 1 {
            return Err(IdentityError::Unauthenticated);
        }
        Ok(())
    }
    pub async fn authenticate_android_token_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        token: &str,
    ) -> Result<Principal> {
        self.lock_installation(tx).await?;
        if token.len() != 69
            || !token.starts_with("cw2s_")
            || !token[5..].bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(IdentityError::Unauthenticated);
        }
        let row = sqlx::query(
            "SELECT id,identity_id,revision FROM identity_sessions WHERE token_sha256=$1",
        )
        .bind(secret_hash("session-v1", token))
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(IdentityError::Unauthenticated)?;
        let actor = Principal {
            identity_id: identifier(row.try_get("identity_id")?)?,
            session_id: identifier(row.try_get("id")?)?,
            session_revision: row.try_get("revision")?,
        };
        self.actor(tx, &actor).await?;
        Ok(actor)
    }
    /// Records the actual newly issued session under the same writer. This
    /// cannot be used with a caller-selected session ID or identity.
    pub async fn record_android_session_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        issued: &SessionIssued,
        device: &str,
        verified: Option<(i64, &str)>,
    ) -> Result<Uuid> {
        self.lock_installation(tx).await?;
        if device.is_empty() || device.len() > 200 || device.chars().any(char::is_control) {
            return Err(IdentityError::Validation);
        }
        let current = self.sample(tx).await?;
        let sid:String=sqlx::query_scalar("SELECT id FROM identity_sessions WHERE token_sha256=$1 AND identity_id=$2 AND revoked=0 AND expires_at>$3")
            .bind(secret_hash("session-v1",&issued.token)).bind(issued.identity.id.to_string()).bind(current).fetch_one(&mut **tx).await?;
        let (revision, method) = verified.unwrap_or((0, ""));
        if revision < 0 || !matches!(method, "" | "totp" | "recovery" | "passkey") {
            return Err(IdentityError::Validation);
        }
        let changed=sqlx::query("UPDATE android_auth_session_metadata SET device_id=$1,last_used_at=$2,verified_at=CASE WHEN $3='' THEN verified_at ELSE $2 END,factor_revision=CASE WHEN $3='' THEN factor_revision ELSE $4 END,factor_method=CASE WHEN $3='' THEN factor_method ELSE $3 END WHERE session_id=$5 AND owner_id=$6 AND revoked_at=0 AND (device_id='' OR device_id=$1)")
            .bind(device).bind(current).bind(method).bind(revision).bind(&sid).bind(issued.identity.id.to_string()).execute(&mut **tx).await?;
        if changed.rows_affected() != 1 {
            return Err(IdentityError::Conflict);
        }
        identifier(sid)
    }
    pub(super) async fn inherit_android_assurance(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
        issued: &SessionIssued,
    ) -> Result<()> {
        let sid:String=sqlx::query_scalar("SELECT id FROM identity_sessions WHERE token_sha256=$1 AND identity_id=$2 AND revoked=0")
            .bind(secret_hash("session-v1",&issued.token)).bind(actor.identity_id.to_string()).fetch_one(&mut **tx).await?;
        // Copy the original proof time, never the current refresh time. A later
        // assurance read checks the live factor revision and the original window.
        sqlx::query("UPDATE android_auth_session_metadata SET device_id=COALESCE((SELECT device_id FROM android_auth_session_metadata WHERE session_id=$1 AND owner_id=$2 AND revoked_at=0),''),verified_at=COALESCE((SELECT verified_at FROM android_auth_session_metadata WHERE session_id=$1 AND owner_id=$2 AND revoked_at=0),0),factor_revision=COALESCE((SELECT factor_revision FROM android_auth_session_metadata WHERE session_id=$1 AND owner_id=$2 AND revoked_at=0),0),factor_method=COALESCE((SELECT factor_method FROM android_auth_session_metadata WHERE session_id=$1 AND owner_id=$2 AND revoked_at=0),'') WHERE session_id=$3 AND owner_id=$2")
            .bind(actor.session_id.to_string()).bind(actor.identity_id.to_string()).bind(&sid).execute(&mut **tx).await?;
        sqlx::query("INSERT INTO android_auth_passkey_proofs(session_id,owner_id,passkey_id,credential_revision,verified_at) SELECT $1,owner_id,passkey_id,credential_revision,verified_at FROM android_auth_passkey_proofs WHERE session_id=$2 AND owner_id=$3")
            .bind(sid).bind(actor.session_id.to_string()).bind(actor.identity_id.to_string()).execute(&mut **tx).await?;
        Ok(())
    }
    pub async fn verify_android_password_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        name: &str,
        password: &str,
    ) -> Result<VerifiedPassword> {
        self.lock_installation(tx).await?;
        if password.len() > 1024 {
            return Err(IdentityError::Unauthenticated);
        }
        let row = self.android_login_row(tx, name).await?;
        let Some(row) = row else {
            let _ = verify_password(
                "00000000000000000000000000000000",
                password,
                &"00".repeat(32),
            );
            return Err(IdentityError::Unauthenticated);
        };
        let value = identity(&row)?;
        if !value.active
            || !verify_password(
                &row.try_get::<String, _>("password_salt")?,
                password,
                &row.try_get::<String, _>("password_hash")?,
            )
        {
            return Err(IdentityError::Unauthenticated);
        }
        Ok(VerifiedPassword { identity: value })
    }
    pub async fn issue_android_password_session_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        witness: &VerifiedPassword,
        ttl: i64,
    ) -> Result<SessionIssued> {
        self.lock_installation(tx).await?;
        let value = self
            .identity_version(tx, witness.identity.id, witness.identity.revision)
            .await?;
        self.session(tx, value, ttl).await
    }
    pub async fn verify_current_password_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
        password: &str,
    ) -> Result<()> {
        let actual = self.validate_principal_in_transaction(tx, actor).await?;
        let verified = self
            .verify_android_password_in_transaction(tx, &actual.username, password)
            .await?;
        if verified.identity.id != actor.identity_id()
            || verified.identity.revision != actual.revision
        {
            return Err(IdentityError::Unauthenticated);
        }
        Ok(())
    }
    /// Consumes the actual server invitation and creates a new unprivileged
    /// identity. It never adopts an existing account, organization or private data.
    pub async fn accept_android_invitation_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        invitation: Uuid,
        token_digest: &str,
        display: &str,
        password: &str,
        ttl: i64,
    ) -> Result<SessionIssued> {
        self.lock_installation(tx).await?;
        let current = self.sample(tx).await?;
        let row=sqlx::query("SELECT v.email FROM android_auth_invitations v JOIN identity_subjects i ON i.id=v.issuer_id WHERE v.id=$1 AND v.token_digest=$2 AND v.accepted_at=0 AND v.revoked_at=0 AND v.expires_at>$3 AND i.active=1 AND i.platform_admin=1 AND i.revision=v.issuer_revision")
            .bind(invitation.to_string()).bind(token_digest).bind(current).fetch_optional(&mut **tx).await?.ok_or(IdentityError::Unauthenticated)?;
        let email: String = row.try_get("email")?;
        let accepted:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_auth_invitations WHERE email=$1 AND accepted_at>0 AND accepted_user_id IS NOT NULL").bind(&email).fetch_one(&mut **tx).await?;
        if accepted != 0 {
            return Err(IdentityError::Conflict);
        }
        let internal_name = format!("android-{}", Uuid::new_v4().simple());
        let value = self
            .insert_identity(tx, &internal_name, display, password, false)
            .await?;
        self.audit_event(tx, None, value.id, "identity_registered", value.id)
            .await?;
        let issued = self.session(tx, value.clone(), ttl).await?;
        let session: String = sqlx::query_scalar(
            "SELECT id FROM identity_sessions WHERE token_sha256=$1 AND identity_id=$2",
        )
        .bind(secret_hash("session-v1", &issued.token))
        .bind(value.id.to_string())
        .fetch_one(&mut **tx)
        .await?;
        if sqlx::query("UPDATE android_auth_invitations SET accepted_at=$1,accepted_user_id=$2,accepted_session_id=$3 WHERE id=$4 AND accepted_at=0 AND revoked_at=0 AND expires_at>$1")
            .bind(current).bind(value.id.to_string()).bind(session).bind(invitation.to_string()).execute(&mut **tx).await?.rows_affected()!=1 {return Err(IdentityError::Conflict);}
        Ok(issued)
    }
    async fn identity_version(
        &self,
        tx: &mut Transaction<'_, Any>,
        user: Uuid,
        revision: i64,
    ) -> Result<Identity> {
        let row =
            sqlx::query("SELECT * FROM identity_subjects WHERE id=$1 AND active=1 AND revision=$2")
                .bind(user.to_string())
                .bind(revision)
                .fetch_optional(&mut **tx)
                .await?
                .ok_or(IdentityError::Unauthenticated)?;
        identity(&row)
    }
}
