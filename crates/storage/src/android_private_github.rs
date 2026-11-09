//! Repository-specific credentials belonging to the actual private actor and project.
use super::*;
use crate::android_devices::{device_invalid, device_writer};
use cowork_identity::{ContentAction, Principal};
use cowork_secrets::{
    CredentialScope, KeyId, KeyStore, SealedCredential, open_bearer, seal_bearer,
};
use sqlx::{Any, Transaction};
use zeroize::Zeroizing;
struct SecretRequest(Value);
impl std::ops::Deref for SecretRequest {
    type Target = Value;
    fn deref(&self) -> &Value {
        &self.0
    }
}
impl Drop for SecretRequest {
    fn drop(&mut self) {
        if let Some(Value::String(token)) = self.0.get_mut("body").and_then(|v| v.get_mut("token"))
        {
            use zeroize::Zeroize;
            token.zeroize();
        }
    }
}

fn text(value: &Value, key: &str, limit: usize) -> Result<String> {
    let raw = value[key].as_str().ok_or_else(device_invalid)?;
    valid_text(raw, limit)?;
    if raw.chars().any(char::is_control) {
        return Err(device_invalid());
    }
    Ok(raw.into())
}
pub fn origin(raw: &str) -> Result<reqwest::Url> {
    let u = reqwest::Url::parse(raw).map_err(|_| device_invalid())?;
    let host = u.host_str().ok_or_else(device_invalid)?;
    if u.scheme() != "https"
        || u.port_or_known_default() != Some(443)
        || !u.username().is_empty()
        || u.password().is_some()
        || u.query().is_some()
        || u.fragment().is_some()
        || !host.contains('.')
        || host.ends_with('.')
        || host.parse::<std::net::IpAddr>().is_ok()
        || raw.len() > 2048
        || ["localhost", "local", "internal", "lan", "test", "invalid"]
            .iter()
            .any(|s| host == *s || host.ends_with(&format!(".{s}")))
    {
        return Err(Error::Forbidden);
    }
    Ok(u)
}
fn segment(raw: &str) -> Result<()> {
    if raw.is_empty()
        || raw.len() > 100
        || raw.starts_with('.')
        || raw.ends_with('.')
        || !raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        return Err(device_invalid());
    }
    Ok(())
}
fn profile(owner: Uuid, project: Uuid, id: Uuid, revision: i64, body: &Value) -> Result<Value> {
    let o = body.as_object().ok_or_else(device_invalid)?;
    if o.keys().any(|s| {
        !matches!(
            s.as_str(),
            "expected_revision"
                | "name"
                | "git_origin"
                | "api_root"
                | "owner"
                | "repository"
                | "author_name"
                | "author_email"
                | "token"
        )
    }) {
        return Err(device_invalid());
    }
    let git = origin(&text(body, "git_origin", 2048)?)?;
    if git.path() != "/" {
        return Err(device_invalid());
    }
    let api = origin(&text(body, "api_root", 2048)?)?;
    // Public GitHub uses its API origin; enterprise remains on the exact same TLS origin.
    if !(git.host_str() == Some("github.com") && api.as_str() == "https://api.github.com/"
        || git.origin() == api.origin() && api.path() == "/api/v3/")
    {
        return Err(Error::Forbidden);
    }
    let repository = text(body, "repository", 100)?;
    let namespace = text(body, "owner", 100)?;
    segment(&repository)?;
    segment(&namespace)?;
    if repository.ends_with(".git") {
        return Err(device_invalid());
    }
    let email = text(body, "author_email", 254)?;
    if !email.contains('@') || email.contains(['<', '>', ' ']) {
        return Err(device_invalid());
    }
    Ok(
        json!({"format":1,"id":id,"owner_user_id":owner,"project_id":project,"revision":revision,
        "name":text(body,"name",200)?,"git_origin":git.origin().ascii_serialization(),"api_root":api.as_str(),
        "owner":namespace,"repository":repository,"author_name":text(body,"author_name",200)?,"author_email":email}),
    )
}
pub fn credential_scope(p: &Value) -> Result<CredentialScope> {
    let mut metadata = p.clone();
    metadata
        .as_object_mut()
        .ok_or_else(device_invalid)?
        .remove("binding_sha256");
    let digest = canonical_hash(&metadata);
    if p["binding_sha256"] != digest {
        return Err(Error::Forbidden);
    }
    Ok(CredentialScope {
        project_id: format!(
            "private-github/{}",
            crate::android_personal::wire_uuid(p, "project_id")?
        ),
        server_id: crate::android_personal::wire_uuid(p, "id")?.to_string(),
        reference: format!(
            "actor/{}/sha/{digest}",
            crate::android_personal::wire_uuid(p, "owner_user_id")?
        ),
        revision: p["revision"]
            .as_i64()
            .filter(|n| *n > 0)
            .ok_or_else(device_invalid)?,
        endpoint: format!(
            "{}/{}/{}.git",
            p["git_origin"].as_str().ok_or_else(device_invalid)?,
            p["owner"].as_str().ok_or_else(device_invalid)?,
            p["repository"].as_str().ok_or_else(device_invalid)?
        ),
    })
}
fn mac(keys: &dyn KeyStore, id: KeyId, value: &Value) -> Result<String> {
    let key = keys.load(&id).map_err(|_| Error::NativeKeyUnavailable)?;
    let raw = Zeroizing::new(serde_json::to_vec(value).map_err(storage_error)?);
    let mut h =
        ring::hmac::Context::with_key(&ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key.as_ref()));
    h.update(b"android-private-github-mutation/v1\0");
    h.update(&raw);
    Ok(h.sign()
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
pub(crate) async fn current(
    tx: &mut Transaction<'_, Any>,
    owner: Uuid,
    project: Uuid,
    id: Uuid,
) -> Result<Value> {
    let raw:String=sqlx::query_scalar("SELECT payload FROM android_private_github_bindings WHERE id=$1 AND owner_id=$2 AND project_id=$3 AND deleted=0").bind(id.to_string()).bind(owner.to_string()).bind(project.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
    let value: Value = parse(&raw)?;
    credential_scope(&value)?;
    Ok(value)
}
pub(crate) async fn validate_binding(
    tx: &mut Transaction<'_, Any>,
    owner: Uuid,
    project: Uuid,
    input: &Value,
) -> Result<Option<Value>> {
    let Some(frozen) = input.get("frozen_github_binding") else {
        return Ok(None);
    };
    let id = crate::android_personal::wire_uuid(frozen, "id")?;
    let actual = current(tx, owner, project, id).await?;
    if actual != *frozen {
        return Err(Error::Forbidden);
    }
    Ok(Some(actual))
}
impl Store {
    pub async fn android_private_github_bindings(
        &self,
        actor: &Principal,
        project: Uuid,
    ) -> Result<Vec<Value>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.android_personal_permit(&mut tx, actor, project, ContentAction::Read)
            .await?;
        let rows:Vec<String>=sqlx::query_scalar("SELECT payload FROM android_private_github_bindings WHERE owner_id=$1 AND project_id=$2 AND deleted=0 ORDER BY id LIMIT 129").bind(actor.identity_id().to_string()).bind(project.to_string()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        if rows.len() > 128 {
            return Err(Error::BudgetExceeded);
        }
        let value = rows.iter().map(|s| parse(s)).collect::<Result<Vec<_>>>()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn android_private_github_freeze(
        &self,
        actor: &Principal,
        project: Uuid,
        id: Uuid,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.android_personal_permit(&mut tx, actor, project, ContentAction::Read)
            .await?;
        let value = current(&mut tx, actor.identity_id(), project, id).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "android_private_github_mutate: Keep actor/project/connection, original mutation operation and native key custody explicit"
    )]
    pub async fn android_private_github_mutate(
        &self,
        actor: &Principal,
        project: Uuid,
        id: Uuid,
        body: &Value,
        remove: bool,
        operation: Uuid,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<Value> {
        if operation.is_nil() || id.is_nil() {
            return Err(device_invalid());
        }
        let request = SecretRequest(
            json!({"actor":actor.identity_id(),"project":project,"id":id,"remove":remove,"body":body}),
        );
        let auth = keys
            .key_for_purpose(&key_id, cowork_secrets::KeyPurpose::RequestFingerprint)
            .map_err(|_| Error::NativeKeyUnavailable)?;
        let cipher = keys
            .key_for_purpose(&key_id, cowork_secrets::KeyPurpose::DataEncryption)
            .map_err(|_| Error::NativeKeyUnavailable)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        self.android_personal_permit(&mut tx, actor, project, ContentAction::Write)
            .await?;
        if let Some(r)=sqlx::query("SELECT request_key_id,request_mac,receipt FROM android_private_github_operations WHERE owner_id=$1 AND project_id=$2 AND operation_key=$3").bind(actor.identity_id().to_string()).bind(project.to_string()).bind(operation.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?{
            let original:KeyId=parse(&r.try_get::<String,_>("request_key_id").map_err(storage_error)?)?;
            if mac(keys,original,&request)?!=r.try_get::<String,_>("request_mac").map_err(storage_error)?{return Err(Error::Conflict)}
            let receipt=parse(&r.try_get::<String,_>("receipt").map_err(storage_error)?)?;tx.commit().await.map_err(storage_error)?;return Ok(receipt)
        }
        let old:Option<i64>=sqlx::query_scalar("SELECT revision FROM android_private_github_bindings WHERE id=$1 AND owner_id=$2 AND project_id=$3 AND deleted=0").bind(id.to_string()).bind(actor.identity_id().to_string()).bind(project.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        if body["expected_revision"].as_i64() != old || remove && old.is_none() {
            return Err(Error::Conflict);
        }
        let next = old
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(Error::BudgetExceeded)?;
        let receipt = if remove {
            if body.as_object().is_none_or(|v| v.len() != 1) {
                return Err(device_invalid());
            }
            if sqlx::query("UPDATE android_private_github_bindings SET deleted=1,revision=$1,sealed=NULL,updated_at=$2 WHERE id=$3 AND owner_id=$4 AND project_id=$5 AND revision=$6 AND deleted=0").bind(next).bind(now).bind(id.to_string()).bind(actor.identity_id().to_string()).bind(project.to_string()).bind(old.unwrap()).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict)}
            json!({"id":id,"revision":next,"deleted":true})
        } else {
            let mut p = profile(actor.identity_id(), project, id, next, body)?;
            p["binding_sha256"] = json!(canonical_hash(&p));
            let scope = credential_scope(&p)?;
            let token = Zeroizing::new(text(body, "token", 4096)?);
            let sealed = seal_bearer(keys, cipher, &scope, &token)
                .map_err(|_| Error::NativeKeyUnavailable)?;
            crate::server_keys::cipher_writer(&mut tx, cipher).await?;
            sqlx::query("INSERT INTO android_private_github_bindings(id,owner_id,project_id,revision,binding_sha,payload,sealed,deleted,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,0,$8) ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,binding_sha=excluded.binding_sha,payload=excluded.payload,sealed=excluded.sealed,deleted=0,updated_at=excluded.updated_at WHERE android_private_github_bindings.owner_id=excluded.owner_id AND android_private_github_bindings.project_id=excluded.project_id AND android_private_github_bindings.revision=$9 AND android_private_github_bindings.deleted=0").bind(id.to_string()).bind(actor.identity_id().to_string()).bind(project.to_string()).bind(next).bind(p["binding_sha256"].as_str().unwrap()).bind(json_string(&p)?).bind(json_string(&sealed)?).bind(now).bind(old.unwrap_or(0)).execute(&mut *tx).await.map_err(storage_error).and_then(|r|if r.rows_affected()==1{Ok(())}else{Err(Error::Conflict)})?;
            p
        };
        self.android_personal_permit(&mut tx, actor, project, ContentAction::Write)
            .await?;
        sqlx::query("INSERT INTO android_private_github_operations(owner_id,project_id,operation_key,request_key_id,request_mac,receipt) VALUES($1,$2,$3,$4,$5,$6)").bind(actor.identity_id().to_string()).bind(project.to_string()).bind(operation.to_string()).bind(json_string(&auth)?).bind(mac(keys,auth,&request)?).bind(json_string(&receipt)?).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
}
pub(crate) async fn rewrap(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    target: KeyId,
) -> Result<u64> {
    let rows=sqlx::query("SELECT id,payload,sealed FROM android_private_github_bindings WHERE sealed IS NOT NULL LIMIT 100001").fetch_all(&mut **tx).await.map_err(storage_error)?;
    if rows.len() > 100000 {
        return Err(Error::BudgetExceeded);
    }
    for row in &rows {
        let raw: String = row.try_get("sealed").map_err(storage_error)?;
        let p: Value = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        let scope = credential_scope(&p)?;
        let old: SealedCredential = parse(&raw)?;
        let token = open_bearer(keys, &scope, &old).map_err(|_| Error::NativeKeyUnavailable)?;
        let next =
            seal_bearer(keys, target, &scope, &token).map_err(|_| Error::NativeKeyUnavailable)?;
        if sqlx::query(
            "UPDATE android_private_github_bindings SET sealed=$1 WHERE id=$2 AND sealed=$3",
        )
        .bind(json_string(&next)?)
        .bind(row.try_get::<String, _>("id").map_err(storage_error)?)
        .bind(raw)
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?
        .rows_affected()
            != 1
        {
            return Err(Error::Conflict);
        }
    }
    Ok(rows.len() as u64)
}
pub(crate) async fn credential(
    tx: &mut Transaction<'_, Any>,
    p: &Value,
    keys: &dyn KeyStore,
) -> Result<Zeroizing<String>> {
    let actual = current(
        tx,
        crate::android_personal::wire_uuid(p, "owner_user_id")?,
        crate::android_personal::wire_uuid(p, "project_id")?,
        crate::android_personal::wire_uuid(p, "id")?,
    )
    .await?;
    if actual != *p {
        return Err(Error::Forbidden);
    }
    let raw:String=sqlx::query_scalar("SELECT sealed FROM android_private_github_bindings WHERE id=$1 AND revision=$2 AND deleted=0").bind(p["id"].as_str().unwrap()).bind(p["revision"].as_i64().unwrap()).fetch_one(&mut **tx).await.map_err(storage_error)?;
    open_bearer(
        keys,
        &credential_scope(p)?,
        &parse::<SealedCredential>(&raw)?,
    )
    .map_err(|_| Error::NativeKeyUnavailable)
}
