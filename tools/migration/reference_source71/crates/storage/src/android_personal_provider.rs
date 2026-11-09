//! Identity-scoped server model profiles. Only ciphertext and keyed request receipts persist.
use super::*;
use crate::android_devices::{device_invalid, device_iso, device_writer};
use cowork_identity::{ContentAction, Principal};
use cowork_secrets::{
    CredentialScope, KeyId, KeyStore, SealedCredential, open_bearer, seal_bearer,
};
use sqlx::{Any, Transaction};
use team_service::identity_error;
use zeroize::Zeroizing;

pub fn credential_scope(profile: &Value) -> Result<CredentialScope> {
    Ok(CredentialScope {
        project_id: format!(
            "android-personal-providers/{}",
            crate::android_personal::wire_uuid(profile, "owner_user_id")?
        ),
        server_id: crate::android_personal::wire_uuid(profile, "id")?.to_string(),
        reference: "model-provider-bearer/v1".into(),
        revision: profile["revision"]
            .as_i64()
            .filter(|v| *v > 0)
            .ok_or_else(device_invalid)?,
        endpoint: profile["model_defaults"]["base_url"]
            .as_str()
            .ok_or_else(device_invalid)?
            .into(),
    })
}
pub fn binding(profile: &Value) -> Result<Value> {
    let scope = credential_scope(profile)?;
    Ok(
        json!({"id":profile["id"],"revision":scope.revision,"owner_user_id":profile["owner_user_id"],"endpoint":scope.endpoint,"sha256":hash(json_string(profile)?.as_bytes())}),
    )
}
fn contains_secret(v: &Value) -> bool {
    match v {
        Value::Object(o) => o.iter().any(|(k, v)| {
            matches!(
                k.to_ascii_lowercase().as_str(),
                "api_key"
                    | "token"
                    | "bearer"
                    | "authorization"
                    | "password"
                    | "secret"
                    | "credential"
                    | "access_token"
                    | "refresh_token"
            ) || contains_secret(v)
        }),
        Value::Array(a) => a.iter().any(contains_secret),
        _ => false,
    }
}
fn required(v: &Value, k: &str, max: usize) -> Result<String> {
    let s = v[k].as_str().ok_or_else(device_invalid)?.trim();
    if s.is_empty() || s.len() > max || s.chars().any(char::is_control) {
        return Err(device_invalid());
    }
    Ok(s.into())
}
pub fn normalized_defaults(mut v: Value) -> Result<Value> {
    if !v.is_object() || contains_secret(&v) || json_string(&v)?.len() > 64 * 1024 {
        return Err(device_invalid());
    }
    let url =
        reqwest::Url::parse(&required(&v, "base_url", 2048)?).map_err(|_| device_invalid())?;
    let local = matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "::1" | "[::1]")
    );
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || (url.scheme() != "https" && !(url.scheme() == "http" && local))
    {
        return Err(device_invalid());
    }
    let model = required(&v, "model", 200)?;
    let timeout = v["timeout_ms"].as_u64().unwrap_or(120_000);
    let steps = v["max_steps"].as_u64().unwrap_or(64);
    let context = v["context_tokens"].as_u64().unwrap_or(32_768);
    if !(1000..=1_200_000).contains(&timeout)
        || !(1..=128).contains(&steps)
        || !(1024..=2_000_000).contains(&context)
        || v.get("verify_tls_certificates")
            .is_some_and(|v| v != &json!(true))
    {
        return Err(device_invalid());
    }
    let auth = v["auth_mode"].as_str().unwrap_or("bearer");
    if !matches!(auth, "none" | "bearer") {
        return Err(device_invalid());
    }
    let auth = auth.to_owned();
    v["base_url"] = json!(url.as_str().trim_end_matches('/'));
    v["model"] = json!(model);
    v["timeout_ms"] = json!(timeout);
    v["max_steps"] = json!(steps);
    v["context_tokens"] = json!(context);
    v["auth_mode"] = json!(auth);
    v["verify_tls_certificates"] = json!(true);
    v["source"] = json!("server");
    v["endpoint_binding"] = json!("server");
    v["executor_kinds"] = json!(["server_linux"]);
    Ok(v)
}
async fn permit(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    action: ContentAction,
) -> Result<()> {
    store
        .identity()
        .require_global_session_in_transaction(tx, actor)
        .await
        .map_err(identity_error)?;
    store
        .identity()
        .require_session_action_in_transaction(tx, actor, action)
        .await
        .map_err(identity_error)
}
pub(crate) async fn current(tx: &mut Transaction<'_, Any>, owner: Uuid, id: Uuid) -> Result<Value> {
    let row=sqlx::query("SELECT payload FROM android_personal_provider_profiles WHERE id=$1 AND owner_id=$2 AND deleted=0").bind(id.to_string()).bind(owner.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
    parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)
}
fn request_mac(
    keys: &dyn KeyStore,
    id: KeyId,
    owner: Uuid,
    operation: &str,
    target: Option<Uuid>,
    body: &Value,
) -> Result<String> {
    let key = keys.load(&id).map_err(|_| Error::NativeKeyUnavailable)?;
    let bytes = Zeroizing::new(
        serde_json::to_vec(&(
            "android-personal-provider-request/v1",
            owner,
            operation,
            target,
            body,
        ))
        .map_err(storage_error)?,
    );
    Ok(ring::hmac::sign(
        &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key.as_ref()),
        bytes.as_slice(),
    )
    .as_ref()
    .iter()
    .map(|b| format!("{b:02x}"))
    .collect())
}
impl Store {
    pub async fn android_personal_providers(&self, actor: &Principal) -> Result<Vec<Value>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        permit(self, &mut tx, actor, ContentAction::Read).await?;
        let rows:Vec<String>=sqlx::query_scalar("SELECT payload FROM android_personal_provider_profiles WHERE owner_id=$1 AND deleted=0 ORDER BY id LIMIT 1001").bind(actor.identity_id().to_string()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        if rows.len() > 1000 {
            return Err(Error::BudgetExceeded);
        }
        let result = rows
            .iter()
            .map(|s| parse(s))
            .collect::<Result<Vec<Value>>>()?;
        permit(self, &mut tx, actor, ContentAction::Read).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn android_personal_provider(&self, actor: &Principal, id: Uuid) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        permit(self, &mut tx, actor, ContentAction::Read).await?;
        let p = current(&mut tx, actor.identity_id(), id).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(p)
    }
    /// Every mutation and its original acknowledgement commit atomically. An absent key uses a keyed exact-request identity for old clients.
    pub async fn android_personal_provider_mutate(
        &self,
        actor: &Principal,
        operation: &str,
        target: Option<Uuid>,
        mut body: Value,
        operation_key: Option<&str>,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<Value> {
        if !matches!(operation, "create" | "update" | "secret" | "delete") || !body.is_object() {
            return Err(device_invalid());
        }
        let request_key_id = keys
            .key_for_purpose(&key_id, cowork_secrets::KeyPurpose::RequestFingerprint)
            .map_err(|_| Error::NativeKeyUnavailable)?;
        let mac = request_mac(
            keys,
            request_key_id,
            actor.identity_id(),
            operation,
            target,
            &body,
        )?;
        let key = operation_key
            .map(str::to_owned)
            .unwrap_or_else(|| format!("legacy:{mac}"));
        valid_text(&key, 200)?;
        if key.chars().any(char::is_control) {
            return Err(device_invalid());
        }
        let allowed = match operation {
            "create" => &[
                "team_id",
                "name",
                "provider_kind",
                "model_defaults",
                "api_key",
            ][..],
            "update" => &[
                "expected_revision",
                "name",
                "provider_kind",
                "model_defaults",
                "api_key",
            ][..],
            "secret" => &["expected_revision", "api_key"][..],
            _ => &["expected_revision"][..],
        };
        if body
            .as_object()
            .ok_or_else(device_invalid)?
            .keys()
            .any(|k| !allowed.contains(&k.as_str()))
        {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let stamp = device_writer(&mut tx).await?;
        permit(self, &mut tx, actor, ContentAction::Write).await?;
        if let Some(row)=sqlx::query("SELECT request_mac,receipt FROM android_personal_provider_operations WHERE owner_id=$1 AND operation_key=$2").bind(actor.identity_id().to_string()).bind(&key).fetch_optional(&mut *tx).await.map_err(storage_error)?{if row.try_get::<String,_>("request_mac").map_err(storage_error)?!=mac{return Err(Error::Conflict)}let result=parse(&row.try_get::<String,_>("receipt").map_err(storage_error)?)?;tx.commit().await.map_err(storage_error)?;return Ok(result)}
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM android_personal_provider_operations WHERE owner_id=$1",
        )
        .bind(actor.identity_id().to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        if count >= 100_000 {
            return Err(Error::BudgetExceeded);
        }
        let id = target.unwrap_or_else(Uuid::new_v4);
        let mut profile = if operation == "create" {
            if target.is_some() || !body["team_id"].is_null() {
                return Err(device_invalid());
            }
            json!({"schema_version":2,"id":id,"revision":0,"etag":"","owner_user_id":actor.identity_id(),"team_id":null,"created_at":device_iso(stamp)?,"deleted_at":null,"has_secret":false})
        } else {
            let p = current(&mut tx, actor.identity_id(), id).await?;
            if body["expected_revision"].as_i64() != p["revision"].as_i64() {
                return Err(Error::Conflict);
            }
            p
        };
        let old_scope = if operation != "create" {
            Some(credential_scope(&profile)?)
        } else {
            None
        };
        let api_key = body
            .as_object_mut()
            .ok_or_else(device_invalid)?
            .remove("api_key");
        let old_sealed:Option<(String,String,i64)>=sqlx::query_as("SELECT scope,sealed,profile_revision FROM android_personal_provider_secrets WHERE profile_id=$1").bind(id.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        let mut old_key = None;
        if let Some((scope, sealed, revision)) = &old_sealed {
            let scope: CredentialScope = parse(scope)?;
            if Some(&scope) != old_scope.as_ref() || Some(*revision) != profile["revision"].as_i64()
            {
                return Err(Error::Forbidden);
            }
            if operation != "delete" && api_key.is_none() {
                let sealed: SealedCredential = parse(sealed)?;
                old_key = Some(
                    open_bearer(keys, &scope, &sealed).map_err(|_| Error::NativeKeyUnavailable)?,
                );
            }
        } else if profile["has_secret"] == json!(true) {
            return Err(Error::Forbidden);
        }
        let revision = profile["revision"]
            .as_i64()
            .ok_or_else(device_invalid)?
            .checked_add(1)
            .ok_or(Error::Conflict)?;
        if matches!(operation, "create" | "update") {
            let kind = required(&body, "provider_kind", 64)?;
            if !matches!(kind.as_str(), "openai_compatible" | "openai-compatible") {
                return Err(device_invalid());
            }
            profile["name"] = json!(required(&body, "name", 200)?);
            profile["provider_kind"] = json!("openai_compatible");
            profile["model_defaults"] = normalized_defaults(body["model_defaults"].clone())?;
        }
        let credential = match api_key {
            Some(Value::Null) => None,
            Some(Value::String(value)) => {
                let value = Zeroizing::new(value);
                cowork_secrets::validate_bearer(&value).map_err(|_| device_invalid())?;
                Some(value)
            }
            Some(_) => return Err(device_invalid()),
            None if operation == "secret" => return Err(device_invalid()),
            None => old_key,
        };
        profile["revision"] = json!(revision);
        profile["etag"] = json!(format!("{id}:{revision}"));
        profile["updated_at"] = json!(device_iso(stamp)?);
        profile["has_secret"] = json!(credential.is_some());
        if operation == "delete" {
            profile["deleted_at"] = json!(device_iso(stamp)?);
            profile["has_secret"] = json!(false)
        }
        let scope = credential_scope(&profile)?;
        let key_id = crate::android_private_workspace::writer_cipher(keys, key_id)?;
        let sealed = if operation == "delete" {
            None
        } else {
            credential
                .as_ref()
                .map(|v| seal_bearer(keys, key_id, &scope, v.as_str()))
                .transpose()
                .map_err(|_| Error::NativeKeyUnavailable)?
        };
        if let Some(cipher)=sealed.as_ref(){crate::server_keys::cipher_writer(&mut tx,cipher.key_id()).await?;}
        permit(self, &mut tx, actor, ContentAction::Write).await?;
        sqlx::query("INSERT INTO android_personal_provider_profiles(id,owner_id,revision,payload,deleted) VALUES($1,$2,$3,$4,$5) ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,payload=excluded.payload,deleted=excluded.deleted").bind(id.to_string()).bind(actor.identity_id().to_string()).bind(revision).bind(json_string(&profile)?).bind(i64::from(operation=="delete")).execute(&mut *tx).await.map_err(storage_error)?;
        sqlx::query("DELETE FROM android_personal_provider_secrets WHERE profile_id=$1")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        if let Some(sealed) = sealed {
            crate::server_keys::cipher_writer(&mut tx, sealed.key_id()).await?;
            sqlx::query("INSERT INTO android_personal_provider_secrets(profile_id,profile_revision,scope,sealed) VALUES($1,$2,$3,$4)").bind(id.to_string()).bind(revision).bind(json_string(&scope)?).bind(json_string(&sealed)?).execute(&mut *tx).await.map_err(storage_error)?;
        }
        crate::android_wire_sync::record(
            &mut tx,
            actor.identity_id(),
            "provider_profile",
            id,
            None,
            None,
            revision,
            &profile,
            operation == "delete",
            stamp,
        )
        .await?;
        sqlx::query("INSERT INTO android_personal_provider_operations(owner_id,operation_key,request_mac,receipt) VALUES($1,$2,$3,$4)").bind(actor.identity_id().to_string()).bind(key).bind(mac).bind(json_string(&profile)?).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(profile)
    }
    /// Dispatch uses an exact immutable binding, never the newest profile by ID.
    pub async fn android_personal_provider_dispatch(
        &self,
        actor: &Principal,
        frozen: &Value,
        keys: &dyn KeyStore,
    ) -> Result<(Value, Option<Zeroizing<String>>)> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        permit(self, &mut tx, actor, ContentAction::Execute).await?;
        let p = current(
            &mut tx,
            actor.identity_id(),
            crate::android_personal::wire_uuid(frozen, "id")?,
        )
        .await?;
        if binding(&p)? != *frozen {
            return Err(Error::Forbidden);
        }
        let scope = credential_scope(&p)?;
        let row:Option<(String,String,i64)>=sqlx::query_as("SELECT scope,sealed,profile_revision FROM android_personal_provider_secrets WHERE profile_id=$1").bind(p["id"].as_str().ok_or_else(device_invalid)?).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        let credential = if let Some((stored, sealed, rev)) = row {
            if parse::<CredentialScope>(&stored)? != scope || rev != scope.revision {
                return Err(Error::Forbidden);
            }
            if p["model_defaults"]["auth_mode"] == "none" {
                None
            } else {
                Some(
                    open_bearer(keys, &scope, &parse::<SealedCredential>(&sealed)?)
                        .map_err(|_| Error::NativeKeyUnavailable)?,
                )
            }
        } else {
            if p["has_secret"] == json!(true) || p["model_defaults"]["auth_mode"] != "none" {
                return Err(Error::Forbidden);
            }
            None
        };
        permit(self, &mut tx, actor, ContentAction::Execute).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok((p, credential))
    }
}
