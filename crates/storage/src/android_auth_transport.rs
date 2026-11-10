//! Original authentication exchanges. No consume/rotate/receipt boundary commits separately.
use super::*;
use crate::android_compat::AndroidAuthGrant;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use cowork_secrets::KeyPurpose;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    user_id: Uuid,
    session_id: Uuid,
    challenge: Option<String>,
    sealed: SealedCredential,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OriginalTransport {
    session_token: String,
    response: String,
}
impl Drop for OriginalTransport {
    fn drop(&mut self) {
        self.session_token.zeroize();
        self.response.zeroize();
    }
}
fn token(value: &str) -> Result<()> {
    if value.len() != 69
        || !value.starts_with("cw2a_")
        || !value[5..].bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(invalid());
    }
    Ok(())
}
fn request_mac(keys: &dyn KeyStore, key: KeyId, value: &Value) -> Result<String> {
    let id = keys
        .key_for_purpose(&key, KeyPurpose::RequestFingerprint)
        .map_err(|_| Error::NativeKeyUnavailable)?;
    let secret = keys.load(&id).map_err(|_| Error::NativeKeyUnavailable)?;
    let bytes = Zeroizing::new(serde_json::to_vec(value).map_err(storage_error)?);
    let mut context = hmac::Context::with_key(&hmac::Key::new(hmac::HMAC_SHA256, secret.as_ref()));
    context.update(b"android-auth-transport-request/v1\0");
    context.update(&bytes);
    Ok(context
        .sign()
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
fn grant_scope(g: &AndroidAuthGrant, p: &Payload) -> Result<CredentialScope> {
    Ok(CredentialScope {
        project_id: format!("android-auth:{}", g.id),
        server_id: "android-compat/v1".into(),
        reference: hash(
            &serde_json::to_vec(&(
                &g.kind,
                &g.device_id,
                g.expires_at,
                p.user_id,
                p.session_id,
                &p.challenge,
            ))
            .map_err(storage_error)?,
        ),
        revision: 1,
        endpoint: "localai-cowork://android/auth".into(),
    })
}
fn receipt_scope(r: &sqlx::any::AnyRow) -> Result<CredentialScope> {
    let owner: String = r.try_get("owner_id").map_err(storage_error)?;
    let operation: String = r.try_get("operation_id").map_err(storage_error)?;
    let binding = json!({"purpose":r.try_get::<String,_>("purpose").map_err(storage_error)?,"owner":owner,"operation":operation,"source_session":r.try_get::<String,_>("source_session_id").map_err(storage_error)?,"source_revision":r.try_get::<i64,_>("source_session_revision").map_err(storage_error)?,"target_session":r.try_get::<String,_>("target_session_id").map_err(storage_error)?,"target_revision":r.try_get::<i64,_>("target_session_revision").map_err(storage_error)?,"grant":r.try_get::<Option<String>,_>("source_grant_id").map_err(storage_error)?,"device":r.try_get::<String,_>("device_id").map_err(storage_error)?,"source_token":r.try_get::<String,_>("source_token_sha256").map_err(storage_error)?,"request":r.try_get::<String,_>("request_mac").map_err(storage_error)?,"until":r.try_get::<i64,_>("replay_until").map_err(storage_error)?,"created":r.try_get::<i64,_>("created_at").map_err(storage_error)?});
    Ok(CredentialScope {
        project_id: format!("android-auth-transport/{owner}"),
        server_id: operation,
        reference: format!(
            "original-transport-operation/v1:{}",
            hash(json_string(&binding)?.as_bytes())
        ),
        revision: 1,
        endpoint: "localai-cowork://android/auth/transport".into(),
    })
}
async fn configuration(tx: &mut Transaction<'_, Any>, binding: &Value) -> Result<()> {
    let expected = binding["configuration_revision"]
        .as_i64()
        .filter(|v| *v >= 0)
        .ok_or(Error::Forbidden)?;
    let actual: i64 = sqlx::query_scalar(
        "SELECT revision FROM operator_device_configuration WHERE id='devices-v1'",
    )
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage_error)?
    .unwrap_or(0);
    if expected != actual {
        return Err(Error::Forbidden);
    }
    Ok(())
}
async fn revision(tx: &mut Transaction<'_, Any>, a: &Principal) -> Result<i64> {
    sqlx::query_scalar(
        "SELECT revision FROM identity_sessions WHERE id=$1 AND identity_id=$2 AND revoked=0",
    )
    .bind(a.session_id().to_string())
    .bind(a.identity_id().to_string())
    .fetch_one(&mut **tx)
    .await
    .map_err(storage_error)
}
#[expect(
    clippy::too_many_arguments,
    reason = "grant: Keep original transport operation, actor/session/device, request/response pins and expiry explicit"
)]
pub(crate) async fn grant(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    key: KeyId,
    issued: &SessionIssued,
    actor: &Principal,
    device: &str,
    kind: &str,
    challenge: Option<String>,
    expiry: i64,
) -> Result<(String, String)> {
    if device.is_empty()
        || device.len() > 200
        || device.chars().any(char::is_control)
        || !matches!(kind, "code" | "refresh")
    {
        return Err(invalid());
    }
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM android_auth_grants")
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)?;
    if n >= 10_000 {
        return Err(Error::BudgetExceeded);
    }
    let secret = Zeroizing::new(format!(
        "cw2a_{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    ));
    let mut g = AndroidAuthGrant {
        id: Uuid::new_v4().to_string(),
        kind: kind.into(),
        device_id: device.into(),
        expires_at: expiry,
        payload: String::new(),
    };
    let scope = CredentialScope {
        project_id: "initial".into(),
        server_id: "initial".into(),
        reference: "initial".into(),
        revision: 1,
        endpoint: "localai-cowork://android/auth".into(),
    };
    let mut p = Payload {
        user_id: actor.identity_id(),
        session_id: actor.session_id(),
        challenge,
        sealed: seal_bearer(keys, key, &scope, &issued.token)
            .map_err(|_| Error::NativeKeyUnavailable)?,
    };
    p.sealed = seal_bearer(keys, key, &grant_scope(&g, &p)?, &issued.token)
        .map_err(|_| Error::NativeKeyUnavailable)?;
    g.payload = json_string(&p)?;
    sqlx::query("INSERT INTO android_auth_grants(id,kind,token_sha256,device_id,expires_at,consumed,payload) VALUES($1,$2,$3,$4,$5,0,$6)")
        .bind(&g.id).bind(kind).bind(digest(&secret)).bind(device).bind(expiry).bind(&g.payload).execute(&mut **tx).await.map_err(storage_error)?;
    Ok((secret.to_string(), g.id))
}
async fn tokens(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    key: KeyId,
    issued: &SessionIssued,
    device: &str,
) -> Result<Value> {
    store
        .identity()
        .record_android_session_in_transaction(tx, issued, device, None)
        .await
        .map_err(identity_error)?;
    let actor = store
        .identity()
        .authenticate_android_token_in_transaction(tx, &issued.token)
        .await
        .map_err(identity_error)?;
    if let Ok(device) = Uuid::parse_str(device) {
        if device.is_nil() {
            return Err(invalid());
        }
        sqlx::query("INSERT INTO android_wire_session_devices(session_id,owner_id,device_id) VALUES($1,$2,$3) ON CONFLICT(session_id) DO NOTHING")
            .bind(actor.session_id().to_string()).bind(actor.identity_id().to_string()).bind(device.to_string()).execute(&mut **tx).await.map_err(storage_error)?;
        let n:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_wire_session_devices WHERE session_id=$1 AND owner_id=$2 AND device_id=$3")
            .bind(actor.session_id().to_string()).bind(actor.identity_id().to_string()).bind(device.to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?;
        if n != 1 {
            return Err(Error::Conflict);
        }
    }
    let (refresh, _) = grant(
        tx,
        keys,
        key,
        issued,
        &actor,
        device,
        "refresh",
        None,
        issued.expires_at,
    )
    .await?;
    Ok(
        json!({"schema_version":2,"access_token":issued.token,"access_expires_at":device_iso(issued.expires_at)?,"refresh_token":refresh,"refresh_expires_at":device_iso(issued.expires_at)?,"user_id":actor.identity_id(),"session_id":actor.session_id()}),
    )
}
#[expect(
    clippy::too_many_arguments,
    reason = "remember_transport: Keep original transport operation, actor/session/device, request/response pins and expiry explicit"
)]
async fn remember_transport(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    key: KeyId,
    operation: Uuid,
    purpose: &str,
    source: &Principal,
    source_revision: i64,
    source_grant: Option<&str>,
    source_token: &str,
    device: &str,
    request: &str,
    issued: &SessionIssued,
    response: &Value,
    until: i64,
    current: i64,
) -> Result<()> {
    let target = store
        .identity()
        .authenticate_android_token_in_transaction(tx, &issued.token)
        .await
        .map_err(identity_error)?;
    if source.identity_id() != target.identity_id() {
        return Err(Error::Forbidden);
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM android_auth_transport_operations WHERE owner_id=$1",
    )
    .bind(source.identity_id().to_string())
    .fetch_one(&mut **tx)
    .await
    .map_err(storage_error)?;
    if count >= 4096 {
        return Err(Error::BudgetExceeded);
    }
    let rev = revision(tx, &target).await?;
    sqlx::query("INSERT INTO android_auth_transport_operations(operation_id,purpose,owner_id,source_session_id,source_session_revision,target_session_id,target_session_revision,source_grant_id,device_id,source_token_sha256,request_mac,replay_until,scope,sealed,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,'','',$13)")
        .bind(operation.to_string()).bind(purpose).bind(source.identity_id().to_string()).bind(source.session_id().to_string()).bind(source_revision).bind(target.session_id().to_string()).bind(rev).bind(source_grant).bind(device).bind(source_token).bind(request).bind(until.min(issued.expires_at)).bind(current).execute(&mut **tx).await.map_err(storage_error)?;
    let r = sqlx::query("SELECT * FROM android_auth_transport_operations WHERE operation_id=$1")
        .bind(operation.to_string())
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)?;
    let scope = receipt_scope(&r)?;
    let value = OriginalTransport {
        session_token: issued.token.clone(),
        response: json_string(response)?,
    };
    let bytes = Zeroizing::new(serde_json::to_vec(&value).map_err(storage_error)?);
    let encoded = Zeroizing::new(STANDARD.encode(&*bytes));
    if encoded.len() > 4096 {
        return Err(Error::BudgetExceeded);
    }
    let sealed =
        seal_bearer(keys, key, &scope, &encoded).map_err(|_| Error::NativeKeyUnavailable)?;
    one(sqlx::query("UPDATE android_auth_transport_operations SET scope=$1,sealed=$2 WHERE operation_id=$3 AND scope='' AND sealed=''").bind(json_string(&scope)?).bind(json_string(&sealed)?).bind(operation.to_string()).execute(&mut **tx).await.map_err(storage_error)?.rows_affected())?;
    Ok(())
}
async fn replay_transport(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    operation: Uuid,
    purpose: &str,
    request: &str,
    current: i64,
) -> Result<Option<Value>> {
    let row = sqlx::query("SELECT * FROM android_auth_transport_operations WHERE operation_id=$1")
        .bind(operation.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?;
    let Some(r) = row else { return Ok(None) };
    if r.try_get::<String, _>("purpose").map_err(storage_error)? != purpose
        || r.try_get::<String, _>("request_mac")
            .map_err(storage_error)?
            != request
    {
        return Err(Error::Conflict);
    }
    if r.try_get::<i64, _>("replay_until").map_err(storage_error)? <= current {
        return Err(Error::Forbidden);
    }
    let scope: CredentialScope = parse(&r.try_get::<String, _>("scope").map_err(storage_error)?)?;
    if scope != receipt_scope(&r)? {
        return Err(Error::Forbidden);
    }
    let sealed: SealedCredential =
        parse(&r.try_get::<String, _>("sealed").map_err(storage_error)?)?;
    let plain = open_bearer(keys, &scope, &sealed).map_err(|_| Error::NativeKeyUnavailable)?;
    let bytes = Zeroizing::new(
        STANDARD
            .decode(plain.as_bytes())
            .map_err(|_| Error::Forbidden)?,
    );
    let original: OriginalTransport =
        serde_json::from_slice(&bytes).map_err(|_| Error::Forbidden)?;
    let actor = store
        .identity()
        .authenticate_android_token_in_transaction(tx, &original.session_token)
        .await
        .map_err(identity_error)?;
    if actor.identity_id().to_string()
        != r.try_get::<String, _>("owner_id").map_err(storage_error)?
        || actor.session_id().to_string()
            != r.try_get::<String, _>("target_session_id")
                .map_err(storage_error)?
        || revision(tx, &actor).await?
            != r.try_get::<i64, _>("target_session_revision")
                .map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    let actual_device:String=sqlx::query_scalar("SELECT device_id FROM android_auth_session_metadata WHERE session_id=$1 AND owner_id=$2 AND revoked_at=0")
        .bind(actor.session_id().to_string()).bind(actor.identity_id().to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?;
    if actual_device != r.try_get::<String, _>("device_id").map_err(storage_error)? {
        return Err(Error::Forbidden);
    }
    if matches!(purpose, "passkey_tokens" | "passkey_native_code") {
        let proven:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_auth_passkey_proofs p JOIN android_passkeys k ON k.id=p.passkey_id AND k.identity_id=p.owner_id WHERE p.session_id=$1 AND p.owner_id=$2 AND k.revision>=p.credential_revision")
            .bind(actor.session_id().to_string()).bind(actor.identity_id().to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?;
        if proven != 1 {
            return Err(Error::Forbidden);
        }
    }
    Ok(Some(parse(&original.response)?))
}
async fn revoke_descendants(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    owner: Uuid,
    source_grant: &str,
    current: i64,
) -> Result<()> {
    // The caller has proved the actual original high-entropy token. Only that
    // exchange's existing descendants are revoked, never a caller-supplied user.
    let rows=sqlx::query("WITH RECURSIVE descendants(id) AS (SELECT target_session_id FROM android_auth_transport_operations WHERE source_grant_id=$1 AND owner_id=$2 UNION SELECT o.target_session_id FROM android_auth_transport_operations o JOIN descendants d ON o.source_session_id=d.id WHERE o.owner_id=$2) SELECT o.* FROM android_auth_transport_operations o WHERE o.owner_id=$2 AND (o.source_grant_id=$1 OR o.source_session_id IN (SELECT id FROM descendants)) LIMIT 4097")
        .bind(source_grant).bind(owner.to_string()).fetch_all(&mut **tx).await.map_err(storage_error)?;
    if rows.len() > 4096 {
        return Err(Error::BudgetExceeded);
    }
    for r in rows {
        let scope: CredentialScope =
            parse(&r.try_get::<String, _>("scope").map_err(storage_error)?)?;
        if scope != receipt_scope(&r)? {
            return Err(Error::Forbidden);
        }
        let sealed: SealedCredential =
            parse(&r.try_get::<String, _>("sealed").map_err(storage_error)?)?;
        let plain = open_bearer(keys, &scope, &sealed).map_err(|_| Error::NativeKeyUnavailable)?;
        let bytes = Zeroizing::new(
            STANDARD
                .decode(plain.as_bytes())
                .map_err(|_| Error::Forbidden)?,
        );
        let _: OriginalTransport = serde_json::from_slice(&bytes).map_err(|_| Error::Forbidden)?;
        let session: String = r.try_get("target_session_id").map_err(storage_error)?;
        sqlx::query("UPDATE identity_sessions SET revoked=1,revision=revision+1 WHERE id=$1 AND identity_id=$2 AND revoked=0").bind(&session).bind(owner.to_string()).execute(&mut **tx).await.map_err(storage_error)?;
        sqlx::query("UPDATE android_auth_session_metadata SET revoked_at=$1,revoke_reason='auth_grant_reused' WHERE session_id=$2 AND owner_id=$3 AND revoked_at=0").bind(current).bind(&session).bind(owner.to_string()).execute(&mut **tx).await.map_err(storage_error)?;
    }
    Ok(())
}
impl Store {
    /// OIDC source session/claims were committed by the SDK consumer. This
    /// adapter atomically records the original native code or refresh envelope;
    /// an unchanged lost acknowledgement cannot create another grant.
    pub async fn android_transport_oidc_finish(
        &self,
        keys: std::sync::Arc<dyn KeyStore>,
        key: KeyId,
        q: &AndroidOidcFinish,
        browser_revision: Option<i64>,
    ) -> Result<Value> {
        if q.request_id.is_nil()
            || q.device_id.is_empty()
            || q.device_id.len() > 200
            || q.native_code_challenge.as_ref().is_some_and(|v| {
                v.len() != 43
                    || !v
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
            })
        {
            return Err(invalid());
        }
        let binding =
            json!({"purpose":"oidc_finish","request":q,"browser_revision":browser_revision});
        let fingerprint = request_mac(keys.as_ref(), key, &binding)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        if let Some(expected) = browser_revision {
            configuration(&mut tx, &json!({"configuration_revision":expected})).await?;
        }
        if let Some(original) = replay_transport(
            self,
            &mut tx,
            keys.as_ref(),
            q.request_id,
            "oidc_finish",
            &fingerprint,
            current,
        )
        .await?
        {
            if let Some(expected) = browser_revision {
                configuration(&mut tx, &json!({"configuration_revision":expected})).await?;
            }
            tx.commit().await.map_err(storage_error)?;
            return Ok(original);
        }
        tx.commit().await.map_err(storage_error)?;
        let receipt = self
            .identity()
            .android_oidc_complete(&q.flow, keys.clone(), key)
            .await
            .map_err(identity_error)?;
        let recovery = self
            .identity()
            .android_oidc_recover(
                &AndroidOidcRecover {
                    flow_id: q.flow.flow_id,
                    resume_secret: q.flow.resume_secret.clone(),
                },
                keys.clone(),
            )
            .await
            .map_err(identity_error)?;
        if recovery["native_code_challenge"] != json!(q.native_code_challenge) {
            return Err(Error::Conflict);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        if let Some(expected) = browser_revision {
            configuration(&mut tx, &json!({"configuration_revision":expected})).await?;
        }
        if let Some(original) = replay_transport(
            self,
            &mut tx,
            keys.as_ref(),
            q.request_id,
            "oidc_finish",
            &fingerprint,
            current,
        )
        .await?
        {
            tx.commit().await.map_err(storage_error)?;
            return Ok(original);
        }
        let actor = self
            .identity()
            .authenticate_android_token_in_transaction(&mut tx, receipt.access_token())
            .await
            .map_err(identity_error)?;
        if actor.identity_id() != receipt.identity_id()
            || actor.session_id() != receipt.session_id()
        {
            return Err(Error::Forbidden);
        }
        let device:String=sqlx::query_scalar("SELECT device_id FROM android_auth_session_metadata WHERE session_id=$1 AND owner_id=$2 AND revoked_at=0").bind(actor.session_id().to_string()).bind(actor.identity_id().to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if device != q.device_id {
            return Err(Error::Forbidden);
        }
        // The native challenge is read from the immutable original flow, not
        // adopted from a later callback. Recovery is read-only here.
        let key = crate::android_private_workspace::writer_cipher(keys.as_ref(), key)?;
        crate::server_keys::cipher_writer(&mut tx, key).await?;
        self.identity()
            .validate_principal_in_transaction(&mut tx, &actor)
            .await
            .map_err(identity_error)?;
        let actual = self
            .identity()
            .validate_principal_in_transaction(&mut tx, &actor)
            .await
            .map_err(identity_error)?;
        let expiry: i64 = sqlx::query_scalar(
            "SELECT expires_at FROM identity_sessions WHERE id=$1 AND identity_id=$2 AND revoked=0",
        )
        .bind(actor.session_id().to_string())
        .bind(actor.identity_id().to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        let issued = SessionIssued {
            token: receipt.access_token().into(),
            identity: actual,
            expires_at: expiry,
        };
        let response = if let Some(challenge) = &q.native_code_challenge {
            let expiry = current.saturating_add(120000).min(issued.expires_at);
            let (code, _) = grant(
                &mut tx,
                keys.as_ref(),
                key,
                &issued,
                &actor,
                &device,
                "code",
                Some(challenge.clone()),
                expiry,
            )
            .await?;
            json!({"schema_version":2,"code":code,"expires_at":device_iso(expiry)?})
        } else {
            tokens(self, &mut tx, keys.as_ref(), key, &issued, &device).await?
        };
        let original_revision = revision(&mut tx, &actor).await?;
        remember_transport(
            self,
            &mut tx,
            keys.as_ref(),
            key,
            q.request_id,
            "oidc_finish",
            &actor,
            original_revision,
            None,
            "",
            &device,
            &fingerprint,
            &issued,
            &response,
            issued.expires_at,
            current,
        )
        .await?;
        if let Some(expected) = browser_revision {
            configuration(&mut tx, &json!({"configuration_revision":expected})).await?;
        }
        self.identity()
            .validate_principal_in_transaction(&mut tx, &actor)
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(response)
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "android_transport_registration_start: Keep original transport operation, actor/session/device, request/response pins and expiry explicit"
    )]
    pub async fn android_transport_registration_start(
        &self,
        keys: &dyn KeyStore,
        key: KeyId,
        actor: &Principal,
        bearer: &str,
        operation: Uuid,
        request: &Value,
        challenge: Option<&crate::AndroidPasskeyState>,
        metadata: Option<&Value>,
    ) -> Result<Option<Value>> {
        if operation.is_nil() {
            return Err(invalid());
        }
        let mac = request_mac(
            keys,
            key,
            &json!({"purpose":"native_registration_start","owner":actor.identity_id(),"session":actor.session_id(),"request":request}),
        )?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        configuration(&mut tx, &request["binding"]).await?;
        self.identity()
            .require_global_session_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        self.identity()
            .require_android_management_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        let actual_actor = self
            .identity()
            .authenticate_android_token_in_transaction(&mut tx, bearer)
            .await
            .map_err(identity_error)?;
        if actual_actor.identity_id() != actor.identity_id()
            || actual_actor.session_id() != actor.session_id()
        {
            return Err(Error::Forbidden);
        }
        if let Some(reply) = replay_transport(
            self,
            &mut tx,
            keys,
            operation,
            "native_registration_start",
            &mac,
            current,
        )
        .await?
        {
            tx.commit().await.map_err(storage_error)?;
            return Ok(Some(reply));
        }
        let (Some(challenge), Some(metadata)) = (challenge, metadata) else {
            tx.commit().await.map_err(storage_error)?;
            return Ok(None);
        };
        if metadata["purpose"] != "native_passkey_registration"
            || metadata["challenge_id"] != json!(challenge.id)
            || metadata["device_id"] != request["device_id"]
            || metadata["code_challenge"] != request["code_challenge"]
            || metadata["state"] != request["state"]
            || metadata["redirect_uri"] != request["redirect_uri"]
            || metadata["label"] != request["label"]
            || challenge.kind != "registration"
            || challenge.binding != request["binding"]
        {
            return Err(Error::Forbidden);
        }
        let serialized = json_string(metadata)?;
        if serialized.len() > 12000 {
            return Err(Error::BudgetExceeded);
        }
        let key = crate::android_private_workspace::writer_cipher(keys, key)?;
        crate::server_keys::cipher_writer(&mut tx, key).await?;
        let current = device_writer(&mut tx).await?;
        self.android_passkey_create_challenge_in_transaction(&mut tx, Some(actor), challenge)
            .await?;
        let actual = self
            .identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        let expiry: i64 = sqlx::query_scalar(
            "SELECT expires_at FROM identity_sessions WHERE id=$1 AND identity_id=$2 AND revoked=0",
        )
        .bind(actor.session_id().to_string())
        .bind(actor.identity_id().to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        let until = current
            .saturating_add(120000)
            .min(expiry)
            .min(challenge.expires_at);
        let issued = SessionIssued {
            token: bearer.into(),
            identity: actual,
            expires_at: expiry,
        };
        let device = request["device_id"].as_str().ok_or_else(invalid)?;
        let (ticket, _) = grant(
            &mut tx,
            keys,
            key,
            &issued,
            actor,
            device,
            "code",
            Some(serialized),
            until,
        )
        .await?;
        let response = json!({"ticket":ticket,"expires_at":device_iso(until)?});
        let actual_device:String=sqlx::query_scalar("SELECT device_id FROM android_auth_session_metadata WHERE session_id=$1 AND owner_id=$2 AND revoked_at=0").bind(actor.session_id().to_string()).bind(actor.identity_id().to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let source_revision = revision(&mut tx, actor).await?;
        remember_transport(
            self,
            &mut tx,
            keys,
            key,
            operation,
            "native_registration_start",
            actor,
            source_revision,
            None,
            "",
            &actual_device,
            &mac,
            &issued,
            &response,
            until,
            current,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(Some(response))
    }
    pub async fn android_transport_remove_passkey(
        &self,
        actor: &Principal,
        id: Uuid,
        password: &str,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        self.identity()
            .require_global_session_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        self.identity()
            .require_android_management_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        self.identity()
            .verify_current_password_in_transaction(&mut tx, actor, password)
            .await
            .map_err(identity_error)?;
        let actual = self
            .identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        let required: i64 = sqlx::query_scalar(
            "SELECT admin_required FROM android_auth_security_policy WHERE id=1",
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        if actual.platform_admin && required == 1 {
            let other:i64=sqlx::query_scalar("SELECT (SELECT COUNT(*) FROM android_passkeys WHERE identity_id=$1 AND id<>$2)+(SELECT COUNT(*) FROM android_auth_totp WHERE owner_id=$1 AND enabled=1)").bind(actor.identity_id().to_string()).bind(id.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
            if other == 0 {
                return Err(Error::Forbidden);
            }
        }
        one(
            sqlx::query("DELETE FROM android_passkeys WHERE id=$1 AND identity_id=$2")
                .bind(id.to_string())
                .bind(actor.identity_id().to_string())
                .execute(&mut *tx)
                .await
                .map_err(storage_error)?
                .rows_affected(),
        )?;
        tx.commit().await.map_err(storage_error)
    }
    pub async fn android_transport_registration_ticket(
        &self,
        keys: &dyn KeyStore,
        secret: &str,
    ) -> Result<Value> {
        token(secret)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let r=sqlx::query("SELECT id,kind,device_id,expires_at,payload FROM android_auth_grants WHERE kind='code' AND token_sha256=$1 AND expires_at>$2").bind(digest(secret)).bind(current).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        let g = AndroidAuthGrant {
            id: r.try_get("id").map_err(storage_error)?,
            kind: r.try_get("kind").map_err(storage_error)?,
            device_id: r.try_get("device_id").map_err(storage_error)?,
            expires_at: r.try_get("expires_at").map_err(storage_error)?,
            payload: r.try_get("payload").map_err(storage_error)?,
        };
        let p: Payload = parse(&g.payload)?;
        let bearer = open_bearer(keys, &grant_scope(&g, &p)?, &p.sealed)
            .map_err(|_| Error::NativeKeyUnavailable)?;
        let actor = self
            .identity()
            .authenticate_android_token_in_transaction(&mut tx, &bearer)
            .await
            .map_err(identity_error)?;
        if actor.identity_id() != p.user_id || actor.session_id() != p.session_id {
            return Err(Error::Forbidden);
        }
        self.identity()
            .require_android_management_in_transaction(&mut tx, &actor)
            .await
            .map_err(identity_error)?;
        let metadata: Value = parse(p.challenge.as_deref().ok_or(Error::Forbidden)?)?;
        if metadata["purpose"] != "native_passkey_registration"
            || metadata["device_id"].as_str() != Some(g.device_id.as_str())
        {
            return Err(Error::Forbidden);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(metadata)
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "android_transport_registration_finish: Keep original transport operation, actor/session/device, request/response pins and expiry explicit"
    )]
    pub async fn android_transport_registration_finish<F>(
        &self,
        keys: &dyn KeyStore,
        key: KeyId,
        operation: Uuid,
        secret: &str,
        request: &Value,
        expected: &Value,
        verify: F,
    ) -> Result<Value>
    where
        F: FnOnce(
            &crate::AndroidPasskeyState,
            &[crate::AndroidStoredPasskey],
        ) -> Result<crate::AndroidVerifiedPasskey>,
    {
        token(secret)?;
        if operation.is_nil() {
            return Err(invalid());
        }
        let mac = request_mac(
            keys,
            key,
            &json!({"purpose":"native_registration_finish","ticket":secret,"request":request,"metadata":expected}),
        )?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        configuration(&mut tx, &request["binding"]).await?;
        if let Some(v) = replay_transport(
            self,
            &mut tx,
            keys,
            operation,
            "native_registration_finish",
            &mac,
            current,
        )
        .await?
        {
            tx.commit().await.map_err(storage_error)?;
            return Ok(v);
        }
        let key = crate::android_private_workspace::writer_cipher(keys, key)?;
        crate::server_keys::cipher_writer(&mut tx, key).await?;
        let current = device_writer(&mut tx).await?;
        let r=sqlx::query("SELECT id,kind,device_id,expires_at,payload FROM android_auth_grants WHERE kind='code' AND token_sha256=$1 AND consumed=0 AND expires_at>$2").bind(digest(secret)).bind(current).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        let g = AndroidAuthGrant {
            id: r.try_get("id").map_err(storage_error)?,
            kind: r.try_get("kind").map_err(storage_error)?,
            device_id: r.try_get("device_id").map_err(storage_error)?,
            expires_at: r.try_get("expires_at").map_err(storage_error)?,
            payload: r.try_get("payload").map_err(storage_error)?,
        };
        let p: Payload = parse(&g.payload)?;
        let metadata: Value = parse(p.challenge.as_deref().ok_or(Error::Forbidden)?)?;
        if &metadata != expected
            || metadata["purpose"] != "native_passkey_registration"
            || metadata["device_id"].as_str() != Some(g.device_id.as_str())
        {
            return Err(Error::Forbidden);
        }
        let bearer = open_bearer(keys, &grant_scope(&g, &p)?, &p.sealed)
            .map_err(|_| Error::NativeKeyUnavailable)?;
        let actor = self
            .identity()
            .authenticate_android_token_in_transaction(&mut tx, &bearer)
            .await
            .map_err(identity_error)?;
        if actor.identity_id() != p.user_id || actor.session_id() != p.session_id {
            return Err(Error::Forbidden);
        }
        self.identity()
            .require_android_management_in_transaction(&mut tx, &actor)
            .await
            .map_err(identity_error)?;
        let original_revision = revision(&mut tx, &actor).await?;
        let challenge = uuid(metadata["challenge_id"].as_str().ok_or(Error::Forbidden)?)?;
        let finished = self
            .android_passkey_finish_in_transaction(
                &mut tx,
                Some(&actor),
                challenge,
                "registration",
                verify,
            )
            .await?;
        let crate::AndroidPasskeyFinished::Registered(registered) = finished else {
            return Err(Error::Forbidden);
        };
        one(sqlx::query("UPDATE android_auth_grants SET consumed=1 WHERE id=$1 AND token_sha256=$2 AND payload=$3 AND expires_at=$4 AND consumed=0 AND expires_at>$5").bind(&g.id).bind(digest(secret)).bind(&g.payload).bind(g.expires_at).bind(current).execute(&mut *tx).await.map_err(storage_error)?.rows_affected())?;
        let actual = self
            .identity()
            .validate_principal_in_transaction(&mut tx, &actor)
            .await
            .map_err(identity_error)?;
        let actual_expiry: i64 = sqlx::query_scalar(
            "SELECT expires_at FROM identity_sessions WHERE id=$1 AND identity_id=$2 AND revoked=0",
        )
        .bind(actor.session_id().to_string())
        .bind(actor.identity_id().to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        let issued = SessionIssued {
            token: bearer.as_str().to_owned(),
            identity: actual,
            expires_at: actual_expiry,
        };
        let until = current
            .saturating_add(120_000)
            .min(g.expires_at)
            .min(actual_expiry);
        let challenge = metadata["code_challenge"]
            .as_str()
            .ok_or(Error::Forbidden)?;
        let (code, _) = grant(
            &mut tx,
            keys,
            key,
            &issued,
            &actor,
            &g.device_id,
            "code",
            Some(format!("native_registered:{challenge}:{}", registered.id)),
            until,
        )
        .await?;
        let mut redirect =
            reqwest::Url::parse(metadata["redirect_uri"].as_str().ok_or(Error::Forbidden)?)
                .map_err(|_| invalid())?;
        redirect
            .query_pairs_mut()
            .append_pair("code", &code)
            .append_pair("state", metadata["state"].as_str().ok_or(Error::Forbidden)?);
        let response = json!({"redirect_url":redirect.as_str(),"expires_at":device_iso(until)?});
        let actual_device:String=sqlx::query_scalar("SELECT device_id FROM android_auth_session_metadata WHERE session_id=$1 AND owner_id=$2 AND revoked_at=0").bind(actor.session_id().to_string()).bind(actor.identity_id().to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        remember_transport(
            self,
            &mut tx,
            keys,
            key,
            operation,
            "native_registration_finish",
            &actor,
            original_revision,
            Some(&g.id),
            &digest(secret),
            &actual_device,
            &mac,
            &issued,
            &response,
            until,
            current,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(response)
    }
    pub async fn android_transport_registration_ack(
        &self,
        keys: &dyn KeyStore,
        key: KeyId,
        operation: Uuid,
        secret: &str,
        verifier: &str,
        device: &str,
    ) -> Result<Value> {
        token(secret)?;
        if operation.is_nil()
            || !(43..=128).contains(&verifier.len())
            || !verifier
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
        {
            return Err(invalid());
        }
        let mac = request_mac(
            keys,
            key,
            &json!({"purpose":"native_registration_ack","code":secret,"verifier":verifier,"device":device}),
        )?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        if let Some(v) = replay_transport(
            self,
            &mut tx,
            keys,
            operation,
            "native_registration_ack",
            &mac,
            current,
        )
        .await?
        {
            tx.commit().await.map_err(storage_error)?;
            return Ok(v);
        }
        let key = crate::android_private_workspace::writer_cipher(keys, key)?;
        crate::server_keys::cipher_writer(&mut tx, key).await?;
        let current = device_writer(&mut tx).await?;
        let r=sqlx::query("SELECT id,kind,device_id,expires_at,payload FROM android_auth_grants WHERE kind='code' AND token_sha256=$1 AND consumed=0 AND expires_at>$2").bind(digest(secret)).bind(current).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        let g = AndroidAuthGrant {
            id: r.try_get("id").map_err(storage_error)?,
            kind: r.try_get("kind").map_err(storage_error)?,
            device_id: r.try_get("device_id").map_err(storage_error)?,
            expires_at: r.try_get("expires_at").map_err(storage_error)?,
            payload: r.try_get("payload").map_err(storage_error)?,
        };
        if g.device_id != device {
            return Err(Error::Forbidden);
        }
        let p: Payload = parse(&g.payload)?;
        let parts = p
            .challenge
            .as_deref()
            .ok_or(Error::Forbidden)?
            .split(':')
            .collect::<Vec<_>>();
        if parts.len() != 3
            || parts[0] != "native_registered"
            || parts[1] != URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
        {
            return Err(Error::Forbidden);
        }
        let registered = uuid(parts[2])?;
        let bearer = open_bearer(keys, &grant_scope(&g, &p)?, &p.sealed)
            .map_err(|_| Error::NativeKeyUnavailable)?;
        let actor = self
            .identity()
            .authenticate_android_token_in_transaction(&mut tx, &bearer)
            .await
            .map_err(identity_error)?;
        if actor.identity_id() != p.user_id || actor.session_id() != p.session_id {
            return Err(Error::Forbidden);
        }
        self.identity()
            .require_android_management_in_transaction(&mut tx, &actor)
            .await
            .map_err(identity_error)?;
        let present: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM android_passkeys WHERE id=$1 AND identity_id=$2",
        )
        .bind(registered.to_string())
        .bind(actor.identity_id().to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        if present != 1 {
            return Err(Error::Forbidden);
        }
        let rev = revision(&mut tx, &actor).await?;
        one(sqlx::query("UPDATE android_auth_grants SET consumed=1 WHERE id=$1 AND token_sha256=$2 AND device_id=$3 AND payload=$4 AND expires_at=$5 AND consumed=0 AND expires_at>$6").bind(&g.id).bind(digest(secret)).bind(device).bind(&g.payload).bind(g.expires_at).bind(current).execute(&mut *tx).await.map_err(storage_error)?.rows_affected())?;
        let actual = self
            .identity()
            .validate_principal_in_transaction(&mut tx, &actor)
            .await
            .map_err(identity_error)?;
        let expiry: i64 = sqlx::query_scalar(
            "SELECT expires_at FROM identity_sessions WHERE id=$1 AND identity_id=$2 AND revoked=0",
        )
        .bind(actor.session_id().to_string())
        .bind(actor.identity_id().to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        let issued = SessionIssued {
            token: bearer.as_str().to_owned(),
            identity: actual,
            expires_at: expiry,
        };
        let actual_device:String=sqlx::query_scalar("SELECT device_id FROM android_auth_session_metadata WHERE session_id=$1 AND owner_id=$2 AND revoked_at=0").bind(actor.session_id().to_string()).bind(actor.identity_id().to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let response = json!({"registered":true,"passkey_id":registered});
        remember_transport(
            self,
            &mut tx,
            keys,
            key,
            operation,
            "native_registration_ack",
            &actor,
            rev,
            Some(&g.id),
            &digest(secret),
            &actual_device,
            &mac,
            &issued,
            &response,
            g.expires_at,
            current,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(response)
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "android_transport_passkey_finish: Keep original transport operation, actor/session/device, request/response pins and expiry explicit"
    )]
    pub async fn android_transport_passkey_finish<F>(
        &self,
        keys: &dyn KeyStore,
        key: KeyId,
        operation: Uuid,
        id: Uuid,
        kind: &str,
        request: &Value,
        verify: F,
    ) -> Result<Value>
    where
        F: FnOnce(
            &crate::AndroidPasskeyState,
            &[crate::AndroidStoredPasskey],
        ) -> Result<crate::AndroidVerifiedPasskey>,
    {
        if operation.is_nil()
            || id.is_nil()
            || !matches!(kind, "authentication" | "native_authentication")
        {
            return Err(invalid());
        }
        let purpose = if kind == "authentication" {
            "passkey_tokens"
        } else {
            "passkey_native_code"
        };
        let mac = request_mac(
            keys,
            key,
            &json!({"purpose":purpose,"challenge":id,"request":request}),
        )?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        configuration(&mut tx, &request["binding"]).await?;
        if let Some(v) =
            replay_transport(self, &mut tx, keys, operation, purpose, &mac, current).await?
        {
            tx.commit().await.map_err(storage_error)?;
            return Ok(v);
        }
        let key = crate::android_private_workspace::writer_cipher(keys, key)?;
        crate::server_keys::cipher_writer(&mut tx, key).await?;
        let current = device_writer(&mut tx).await?;
        let expiry:i64=sqlx::query_scalar("SELECT expires_at FROM android_passkey_challenges WHERE id=$1 AND kind=$2 AND consumed=0 AND expires_at>$3")
            .bind(id.to_string()).bind(kind).bind(current).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let finished = self
            .android_passkey_finish_in_transaction(&mut tx, None, id, kind, verify)
            .await?;
        let crate::AndroidPasskeyFinished::Authenticated {
            issued,
            device_id,
            binding,
        } = finished
        else {
            return Err(Error::Forbidden);
        };
        let device = device_id.to_string();
        let actor = self
            .identity()
            .authenticate_android_token_in_transaction(&mut tx, &issued.token)
            .await
            .map_err(identity_error)?;
        let until = if kind == "native_authentication" {
            current.saturating_add(120_000).min(expiry)
        } else {
            expiry
        };
        let response = if kind == "native_authentication" {
            let challenge = binding["challenge"]
                .as_str()
                .filter(|v| {
                    v.len() == 43
                        && v.bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
                })
                .ok_or(Error::Forbidden)?;
            let state = binding["client_state"].as_str().ok_or(Error::Forbidden)?;
            let uri = binding["redirect_uri"].as_str().ok_or(Error::Forbidden)?;
            let (code, _) = grant(
                &mut tx,
                keys,
                key,
                &issued,
                &actor,
                &device,
                "code",
                Some(challenge.into()),
                until,
            )
            .await?;
            let mut redirect = reqwest::Url::parse(uri).map_err(|_| invalid())?;
            redirect
                .query_pairs_mut()
                .append_pair("code", &code)
                .append_pair("state", state);
            json!({"schema_version":2,"redirect_url":redirect.as_str(),"expires_at":device_iso(until)?})
        } else {
            tokens(self, &mut tx, keys, key, &issued, &device).await?
        };
        let rev = revision(&mut tx, &actor).await?;
        remember_transport(
            self, &mut tx, keys, key, operation, purpose, &actor, rev, None, "", &device, &mac,
            &issued, &response, until, current,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(response)
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "android_transport_authorize: Keep original transport operation, actor/session/device, request/response pins and expiry explicit"
    )]
    pub async fn android_transport_authorize(
        &self,
        keys: &dyn KeyStore,
        key: KeyId,
        operation: Uuid,
        email: &str,
        password: &str,
        device: &str,
        code: Option<&str>,
        challenge: &str,
        ttl: i64,
    ) -> Result<Value> {
        if operation.is_nil()
            || challenge.len() != 43
            || !challenge
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
        {
            return Err(invalid());
        }
        let mac = request_mac(
            keys,
            key,
            &json!({"purpose":"native_authorize","email":email,"password":password,"device":device,"factor":code,"challenge":challenge}),
        )?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let witness = self
            .identity()
            .verify_android_password_in_transaction(&mut tx, email, password)
            .await
            .map_err(identity_error)?;
        if let Some(v) = replay_transport(
            self,
            &mut tx,
            keys,
            operation,
            "native_authorize",
            &mac,
            current,
        )
        .await?
        {
            tx.commit().await.map_err(storage_error)?;
            return Ok(v);
        }
        let key = crate::android_private_workspace::writer_cipher(keys, key)?;
        crate::server_keys::cipher_writer(&mut tx, key).await?;
        let current = device_writer(&mut tx).await?;
        let proof =
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
                proof.map(|p| (p.revision, p.method)),
            )
            .await
            .map_err(identity_error)?;
        let actor = self
            .identity()
            .authenticate_android_token_in_transaction(&mut tx, &issued.token)
            .await
            .map_err(identity_error)?;
        let expiry = current.saturating_add(120_000).min(issued.expires_at);
        let (secret, _) = grant(
            &mut tx,
            keys,
            key,
            &issued,
            &actor,
            device,
            "code",
            Some(challenge.into()),
            expiry,
        )
        .await?;
        let response = json!({"schema_version":2,"code":secret,"expires_at":device_iso(expiry)?});
        let source_revision = revision(&mut tx, &actor).await?;
        remember_transport(
            self,
            &mut tx,
            keys,
            key,
            operation,
            "native_authorize",
            &actor,
            source_revision,
            None,
            "",
            device,
            &mac,
            &issued,
            &response,
            expiry,
            current,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(response)
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "android_transport_exchange: Keep original transport operation, actor/session/device, request/response pins and expiry explicit"
    )]
    pub async fn android_transport_exchange(
        &self,
        keys: &dyn KeyStore,
        key: KeyId,
        operation: Uuid,
        kind: &str,
        secret: &str,
        verifier: Option<&str>,
        device: Option<&str>,
        browser: bool,
        configuration_revision: Option<i64>,
        ttl: i64,
    ) -> Result<Value> {
        token(secret)?;
        if operation.is_nil()
            || !matches!(kind, "code" | "refresh")
            || (browser && kind != "refresh")
        {
            return Err(invalid());
        }
        if kind == "code"
            && verifier.is_none_or(|v| {
                !(43..=128).contains(&v.len())
                    || !v
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
            })
        {
            return Err(invalid());
        }
        let purpose = if browser {
            "browser_refresh"
        } else if kind == "code" {
            "native_token"
        } else {
            "native_refresh"
        };
        if browser != configuration_revision.is_some() {
            return Err(invalid());
        }
        let mut mac = request_mac(
            keys,
            key,
            &json!({"purpose":purpose,"token_sha256":digest(secret),"verifier":verifier,"device":device,"configuration_revision":configuration_revision}),
        )?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        if let Some(revision) = configuration_revision {
            configuration(&mut tx, &json!({"configuration_revision":revision})).await?;
        }
        if browser
            && let Some(r)=sqlx::query("SELECT source_token_sha256,request_mac FROM android_auth_transport_operations WHERE operation_id=$1 AND purpose='browser_refresh'").bind(operation.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)? {
                let source:String=r.try_get("source_token_sha256").map_err(storage_error)?;
                if source!=digest(secret){
                    // A browser may have applied Set-Cookie before losing the
                    // response body. Its exact server-issued successor cookie
                    // can retrieve this SAME receipt without another rotation.
                    let original_mac:String=r.try_get("request_mac").map_err(storage_error)?;
                    let v=replay_transport(self,&mut tx,keys,operation,purpose,&original_mac,current).await?.ok_or(Error::Forbidden)?;
                    if v["refresh_token"].as_str().is_none_or(|t|digest(t)!=digest(secret)){return Err(Error::Conflict);}
                    let live:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_auth_grants WHERE kind='refresh' AND token_sha256=$1 AND consumed=0 AND expires_at>$2").bind(digest(secret)).bind(current).fetch_one(&mut *tx).await.map_err(storage_error)?;
                    if live!=1{return Err(Error::Forbidden);}
                    mac=request_mac(keys,key,&json!({"purpose":purpose,"token_sha256":source,"verifier":verifier,"device":device,"configuration_revision":configuration_revision}))?;
                }
            }
        if let Some(v) =
            replay_transport(self, &mut tx, keys, operation, purpose, &mac, current).await?
        {
            tx.commit().await.map_err(storage_error)?;
            return Ok(v);
        }
        let key = crate::android_private_workspace::writer_cipher(keys, key)?;
        crate::server_keys::cipher_writer(&mut tx, key).await?;
        let current = device_writer(&mut tx).await?;
        let row=sqlx::query("SELECT id,kind,device_id,expires_at,payload,consumed FROM android_auth_grants WHERE token_sha256=$1 AND kind=$2")
            .bind(digest(secret)).bind(kind).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        if row.try_get::<i64, _>("expires_at").map_err(storage_error)? <= current {
            return Err(Error::Forbidden);
        }
        let consumed = row.try_get::<i64, _>("consumed").map_err(storage_error)? != 0;
        let g = AndroidAuthGrant {
            id: row.try_get("id").map_err(storage_error)?,
            kind: row.try_get("kind").map_err(storage_error)?,
            device_id: row.try_get("device_id").map_err(storage_error)?,
            expires_at: row.try_get("expires_at").map_err(storage_error)?,
            payload: row.try_get("payload").map_err(storage_error)?,
        };
        if device.is_some_and(|v| v != g.device_id) {
            return Err(Error::Forbidden);
        }
        let p: Payload = parse(&g.payload)?;
        if kind == "code"
            && p.challenge.as_deref()
                != Some(
                    URL_SAFE_NO_PAD
                        .encode(Sha256::digest(verifier.ok_or_else(invalid)?.as_bytes()))
                        .as_str(),
                )
        {
            return Err(Error::Forbidden);
        }
        let bearer = open_bearer(keys, &grant_scope(&g, &p)?, &p.sealed)
            .map_err(|_| Error::NativeKeyUnavailable)?;
        if consumed {
            if kind == "refresh" {
                revoke_descendants(&mut tx, keys, p.user_id, &g.id, current).await?;
                tx.commit().await.map_err(storage_error)?;
            }
            return Err(Error::Forbidden);
        }
        let actor = self
            .identity()
            .authenticate_android_token_in_transaction(&mut tx, &bearer)
            .await
            .map_err(identity_error)?;
        if actor.identity_id() != p.user_id || actor.session_id() != p.session_id {
            return Err(Error::Forbidden);
        }
        let source_revision = revision(&mut tx, &actor).await?;
        one(sqlx::query("UPDATE android_auth_grants SET consumed=1 WHERE id=$1 AND kind=$2 AND token_sha256=$3 AND device_id=$4 AND payload=$5 AND expires_at=$6 AND consumed=0 AND expires_at>$7")
            .bind(&g.id).bind(kind).bind(digest(secret)).bind(&g.device_id).bind(&g.payload).bind(g.expires_at).bind(current).execute(&mut *tx).await.map_err(storage_error)?.rows_affected())?;
        let issued = self
            .identity()
            .rotate_android_session_in_transaction(&mut tx, &actor, ttl)
            .await
            .map_err(identity_error)?;
        let response = tokens(self, &mut tx, keys, key, &issued, &g.device_id).await?;
        remember_transport(
            self,
            &mut tx,
            keys,
            key,
            operation,
            purpose,
            &actor,
            source_revision,
            Some(&g.id),
            &digest(secret),
            &g.device_id,
            &mac,
            &issued,
            &response,
            g.expires_at,
            current,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(response)
    }
}
pub(crate) async fn rewrap_helper(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    target: KeyId,
) -> Result<u64> {
    target.validate().map_err(|_| Error::NativeKeyUnavailable)?;
    keys.load(&target)
        .map_err(|_| Error::NativeKeyUnavailable)?;
    let rows = sqlx::query(
        "SELECT * FROM android_auth_transport_operations ORDER BY operation_id LIMIT 100001",
    )
    .fetch_all(&mut **tx)
    .await
    .map_err(storage_error)?;
    if rows.len() > ROW_LIMIT {
        return Err(Error::BudgetExceeded);
    }
    let mut n = 0;
    for r in rows {
        let scope: CredentialScope =
            parse(&r.try_get::<String, _>("scope").map_err(storage_error)?)?;
        if scope != receipt_scope(&r)? {
            return Err(Error::Forbidden);
        }
        let raw: String = r.try_get("sealed").map_err(storage_error)?;
        let sealed: SealedCredential = parse(&raw)?;
        if sealed.key_id().instance != target.instance || sealed.key_id() == target {
            continue;
        }
        let plain = open_bearer(keys, &scope, &sealed).map_err(|_| Error::NativeKeyUnavailable)?;
        let bytes = Zeroizing::new(
            STANDARD
                .decode(plain.as_bytes())
                .map_err(|_| Error::Forbidden)?,
        );
        let _: OriginalTransport = serde_json::from_slice(&bytes).map_err(|_| Error::Forbidden)?;
        let next =
            seal_bearer(keys, target, &scope, &plain).map_err(|_| Error::NativeKeyUnavailable)?;
        one(sqlx::query("UPDATE android_auth_transport_operations SET sealed=$1 WHERE operation_id=$2 AND scope=$3 AND sealed=$4 AND request_mac=$5 AND source_session_revision=$6 AND target_session_revision=$7")
        .bind(json_string(&next)?).bind(r.try_get::<String,_>("operation_id").map_err(storage_error)?).bind(json_string(&scope)?).bind(raw).bind(r.try_get::<String,_>("request_mac").map_err(storage_error)?).bind(r.try_get::<i64,_>("source_session_revision").map_err(storage_error)?).bind(r.try_get::<i64,_>("target_session_revision").map_err(storage_error)?).execute(&mut **tx).await.map_err(storage_error)?.rows_affected())?;
        n += 1;
    }
    Ok(n)
}
