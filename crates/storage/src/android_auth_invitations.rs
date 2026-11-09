use super::*;
async fn tokens(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    key: KeyId,
    issued: &SessionIssued,
    actor: &Principal,
    device: &str,
    current: i64,
) -> Result<Value> {
    let secret = Zeroizing::new(format!(
        "cw2a_{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    ));
    let grant_id = Uuid::new_v4();
    let challenge: Option<String> = None;
    let scope = CredentialScope {
        project_id: format!("android-auth:{grant_id}"),
        server_id: "android-compat/v1".into(),
        reference: hash(
            &serde_json::to_vec(&(
                "refresh",
                device,
                issued.expires_at,
                actor.identity_id(),
                actor.session_id(),
                &challenge,
            ))
            .map_err(storage_error)?,
        ),
        revision: 1,
        endpoint: "localai-cowork://android/auth".into(),
    };
    crate::server_keys::cipher_writer(tx, key).await?;
    let sealed =
        seal_bearer(keys, key, &scope, &issued.token).map_err(|_| Error::NativeKeyUnavailable)?;
    let payload = json!({"user_id":actor.identity_id(),"session_id":actor.session_id(),"challenge":challenge,"sealed":sealed});
    sqlx::query("DELETE FROM android_auth_grants WHERE expires_at<=$1")
        .bind(current)
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM android_auth_grants")
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)?;
    if count >= 10_000 {
        return Err(Error::BudgetExceeded);
    }
    sqlx::query("INSERT INTO android_auth_grants(id,kind,token_sha256,device_id,expires_at,consumed,payload) VALUES($1,'refresh',$2,$3,$4,0,$5)").bind(grant_id.to_string()).bind(digest(&secret)).bind(device).bind(issued.expires_at).bind(json_string(&payload)?).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(
        json!({"schema_version":2,"access_token":issued.token,"access_expires_at":device_iso(issued.expires_at)?,"refresh_token":*secret,"refresh_expires_at":device_iso(issued.expires_at)?,"user_id":actor.identity_id(),"session_id":actor.session_id()}),
    )
}
/// Read the original encrypted response, then authenticate its actual still-live
/// bearer in this transaction. A saved operation cannot revive a rotated session.
async fn original_login(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    owner: Uuid,
    operation: Uuid,
    purpose: &str,
    request: &str,
) -> Result<Option<Value>> {
    let r = sqlx::query(
        "SELECT scope,sealed FROM android_auth_operations WHERE owner_id=$1 AND operation_id=$2",
    )
    .bind(owner.to_string())
    .bind(operation.to_string())
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage_error)?;
    let Some(r) = r else { return Ok(None) };
    let scope: CredentialScope = parse(&r.try_get::<String, _>("scope").map_err(storage_error)?)?;
    if serde_json::to_value(&scope).map_err(storage_error)?
        != serde_json::to_value(operation_scope(owner, operation)).map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    let sealed: SealedCredential =
        parse(&r.try_get::<String, _>("sealed").map_err(storage_error)?)?;
    let encoded = open_bearer(keys, &scope, &sealed).map_err(|_| Error::NativeKeyUnavailable)?;
    let bytes = Zeroizing::new(
        STANDARD
            .decode(encoded.as_bytes())
            .map_err(|_| Error::Forbidden)?,
    );
    let old: Original = serde_json::from_slice(&bytes).map_err(|_| Error::Forbidden)?;
    if old.request != request {
        return Err(Error::Conflict);
    }
    let response: Value = parse(&old.response)?;
    let token = Zeroizing::new(
        response["access_token"]
            .as_str()
            .ok_or(Error::Forbidden)?
            .to_owned(),
    );
    let actor = store
        .identity()
        .authenticate_android_token_in_transaction(tx, &token)
        .await
        .map_err(identity_error)?;
    if actor.identity_id() != owner {
        return Err(Error::Forbidden);
    }
    authority(store, tx, &actor).await?;
    let original = replay(tx, keys, &actor, operation, purpose, request).await?;
    authority(store, tx, &actor).await?;
    Ok(original)
}
impl Store {
    #[expect(
        clippy::too_many_arguments,
        reason = "android_security_login_tokens: Keep original login/invitation operation, device, credentials and native session key custody explicit"
    )]
    pub async fn android_security_login_tokens(
        &self,
        keys: &dyn KeyStore,
        key: KeyId,
        email: &str,
        password: &str,
        device: &str,
        code: Option<&str>,
        ttl: i64,
        operation: Uuid,
    ) -> Result<Value> {
        if operation.is_nil() {
            return Err(invalid());
        }
        let key = crate::android_private_workspace::writer_cipher(keys, key)?;
        let request=Zeroizing::new(serde_json::to_string(&json!({"email":email,"password":password,"device_id":device,"second_factor":code,"ttl":ttl})).map_err(storage_error)?);
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        let witness = self
            .identity()
            .verify_android_password_in_transaction(&mut tx, email, password)
            .await
            .map_err(identity_error)?;
        if let Some(v) = original_login(
            self,
            &mut tx,
            keys,
            witness.identity().id,
            operation,
            "password_login",
            &request,
        )
        .await?
        {
            tx.commit().await.map_err(storage_error)?;
            return Ok(v);
        }
        // Take the rotation guard before any factor counter/recovery CAS.
        crate::server_keys::cipher_writer(&mut tx, key).await?;
        let current = device_writer(&mut tx).await?;
        let verified =
            match verify_factor(&mut tx, keys, witness.identity().id, code, current, None).await? {
                Verification::Accepted(v) => v,
                Verification::Denied => {
                    tx.commit().await.map_err(storage_error)?;
                    return Err(Error::Forbidden);
                }
            };
        let issued = self
            .identity()
            .issue_android_password_session_in_transaction(&mut tx, &witness, ttl)
            .await
            .map_err(identity_error)?;
        self.identity()
            .record_android_session_in_transaction(
                &mut tx,
                &issued,
                device,
                verified.map(|p| (p.revision, p.method)),
            )
            .await
            .map_err(identity_error)?;
        let actor = self
            .identity()
            .authenticate_android_token_in_transaction(&mut tx, &issued.token)
            .await
            .map_err(identity_error)?;
        let value = tokens(&mut tx, keys, key, &issued, &actor, device, current).await?;
        remember(
            &mut tx,
            keys,
            key,
            &actor,
            operation,
            "password_login",
            &request,
            &value,
            current,
        )
        .await?;
        audit(
            &mut tx,
            &actor,
            "auth.password.login",
            actor.session_id(),
            current,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn android_security_create_invitation(
        &self,
        keys: &dyn KeyStore,
        key: KeyId,
        actor: &Principal,
        email: &str,
        expires: Option<i64>,
        operation: Uuid,
    ) -> Result<Value> {
        if operation.is_nil() {
            return Err(invalid());
        }
        let key = crate::android_private_workspace::writer_cipher(keys, key)?;
        let email = email.trim().to_lowercase();
        if email.is_empty()
            || email.len() > 200
            || !email.contains('@')
            || email.starts_with('@')
            || email.ends_with('@')
            || email.chars().any(char::is_control)
        {
            return Err(invalid());
        }
        let request = Zeroizing::new(
            serde_json::to_string(&json!({"email":email,"expires_at":expires}))
                .map_err(storage_error)?,
        );
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let actual = authority(self, &mut tx, actor).await?;
        if !actual.platform_admin {
            return Err(Error::Forbidden);
        }
        self.identity()
            .require_android_management_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        if let Some(v) = replay(
            &mut tx,
            keys,
            actor,
            operation,
            "invitation_create",
            &request,
        )
        .await?
        {
            tx.commit().await.map_err(storage_error)?;
            return Ok(v);
        }
        let expires = expires.unwrap_or(current.saturating_add(7 * 24 * 60 * 60 * 1000));
        if expires <= current || expires > current.saturating_add(30 * 24 * 60 * 60 * 1000) {
            return Err(invalid());
        }
        let exists: i64 =
            sqlx::query_scalar("SELECT (SELECT COUNT(*) FROM identity_subjects WHERE username=$1)+(SELECT COUNT(*) FROM android_auth_invitations WHERE email=$1 AND accepted_at>0 AND accepted_user_id IS NOT NULL)")
                .bind(&email)
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        if exists != 0 {
            return Err(Error::Conflict);
        }
        let retained: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM android_auth_invitations WHERE issuer_id=$1")
                .bind(actor.identity_id().to_string())
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        if retained >= 4096 {
            return Err(Error::BudgetExceeded);
        }
        let token = Zeroizing::new(format!("cw2i_{}", base32(&*random::<32>()?)));
        let invitation = Uuid::new_v4();
        sqlx::query("INSERT INTO android_auth_invitations(id,issuer_id,issuer_revision,email,token_digest,created_at,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7)").bind(invitation.to_string()).bind(actor.identity_id().to_string()).bind(actual.revision).bind(&email).bind(digest(&token)).bind(current).bind(expires).execute(&mut *tx).await.map_err(storage_error)?;
        let value = json!({"schema_version":2,"invitation_id":invitation,"email":email,"token":*token,"expires_at":device_iso(expires)?});
        remember(
            &mut tx,
            keys,
            key,
            actor,
            operation,
            "invitation_create",
            &request,
            &value,
            current,
        )
        .await?;
        audit(
            &mut tx,
            actor,
            "auth.invitation.created",
            invitation,
            current,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "android_security_accept_invitation: Keep original login/invitation operation, device, credentials and native session key custody explicit"
    )]
    pub async fn android_security_accept_invitation(
        &self,
        keys: &dyn KeyStore,
        key: KeyId,
        token: &str,
        display: &str,
        password: &str,
        device: Uuid,
        ttl: i64,
        operation: Uuid,
    ) -> Result<Value> {
        if operation.is_nil()
            || device.is_nil()
            || token.len() != 57
            || !token.starts_with("cw2i_")
            || display.trim().is_empty()
            || display.len() > 200
            || display.chars().any(char::is_control)
        {
            return Err(invalid());
        }
        let key = crate::android_private_workspace::writer_cipher(keys, key)?;
        let request=Zeroizing::new(serde_json::to_string(&json!({"token":token,"display_name":display,"password":password,"device_id":device,"ttl":ttl})).map_err(storage_error)?);
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let row=sqlx::query("SELECT id,email,accepted_at,accepted_user_id,accept_operation_id FROM android_auth_invitations WHERE token_digest=$1 AND revoked_at=0").bind(digest(token)).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        let invitation = uuid(&row.try_get::<String, _>("id").map_err(storage_error)?)?;
        if row
            .try_get::<i64, _>("accepted_at")
            .map_err(storage_error)?
            > 0
        {
            if row
                .try_get::<Option<String>, _>("accept_operation_id")
                .map_err(storage_error)?
                .as_deref()
                != Some(operation.to_string().as_str())
            {
                return Err(Error::Conflict);
            }
            let email: String = row.try_get("email").map_err(storage_error)?;
            let witness = self
                .identity()
                .verify_android_password_in_transaction(&mut tx, &email, password)
                .await
                .map_err(identity_error)?;
            if row
                .try_get::<Option<String>, _>("accepted_user_id")
                .map_err(storage_error)?
                .as_deref()
                != Some(witness.identity().id.to_string().as_str())
            {
                return Err(Error::Forbidden);
            }
            let value = original_login(
                self,
                &mut tx,
                keys,
                witness.identity().id,
                operation,
                "invitation_accept",
                &request,
            )
            .await?
            .ok_or(Error::Conflict)?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(value);
        }
        let issued = self
            .identity()
            .accept_android_invitation_in_transaction(
                &mut tx,
                invitation,
                &digest(token),
                display,
                password,
                ttl,
            )
            .await
            .map_err(identity_error)?;
        self.identity()
            .record_android_session_in_transaction(&mut tx, &issued, &device.to_string(), None)
            .await
            .map_err(identity_error)?;
        let actor = self
            .identity()
            .authenticate_android_token_in_transaction(&mut tx, &issued.token)
            .await
            .map_err(identity_error)?;
        one(sqlx::query("UPDATE android_auth_invitations SET accept_operation_id=$1 WHERE id=$2 AND accepted_user_id=$3 AND accept_operation_id IS NULL").bind(operation.to_string()).bind(invitation.to_string()).bind(actor.identity_id().to_string()).execute(&mut *tx).await.map_err(storage_error)?.rows_affected())?;
        let value = tokens(
            &mut tx,
            keys,
            key,
            &issued,
            &actor,
            &device.to_string(),
            current,
        )
        .await?;
        remember(
            &mut tx,
            keys,
            key,
            &actor,
            operation,
            "invitation_accept",
            &request,
            &value,
            current,
        )
        .await?;
        audit(
            &mut tx,
            &actor,
            "auth.invitation.accepted",
            invitation,
            current,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
}
