//! Existing Android MCP binding contract, with independent actor/project encryption custody.
use super::*;
use crate::android_devices::{device_invalid, device_iso, device_writer};
use base64::{Engine, engine::general_purpose::STANDARD};
use cowork_identity::{ContentAction, Principal};
use cowork_secrets::{KeyId, KeyStore};
use ring::{
    aead, hmac,
    rand::{SecureRandom, SystemRandom},
};
use serde::Deserialize;
use sqlx::{Any, Transaction, any::AnyRow};
use std::collections::BTreeMap;
use zeroize::Zeroizing;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    pub expected_revision: Option<i64>,
    pub name: String,
    pub transport: String,
    pub command: String,
    pub args: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub url: String,
    pub headers: BTreeMap<String, String>,
}
impl Drop for Configuration {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        for v in self.environment.values_mut() {
            v.zeroize();
        }
        for v in self.headers.values_mut() {
            v.zeroize();
        }
    }
}
fn bounded(values: &BTreeMap<String, String>, headers: bool) -> Result<()> {
    if values.len() > 64 || values.iter().map(|(k, v)| k.len() + v.len()).sum::<usize>() > 32768 {
        return Err(Error::BudgetExceeded);
    }
    for (k, v) in values {
        if k.is_empty()
            || k.len() > 128
            || v.contains('\0')
            || v.len() > 8192
            || if headers {
                k.bytes()
                    .any(|b| !b.is_ascii_alphanumeric() && !b"-!#$%&'*+.^_`|~".contains(&b))
                    || v.chars().any(char::is_control)
                    || [
                        "host",
                        "content-length",
                        "transfer-encoding",
                        "connection",
                        "accept",
                        "content-type",
                        "mcp-session-id",
                        "mcp-protocol-version",
                        "proxy-authorization",
                        "proxy-connection",
                        "te",
                        "trailer",
                        "upgrade",
                    ]
                    .contains(&k.to_ascii_lowercase().as_str())
            } else {
                k.bytes().any(|b| !b.is_ascii_alphanumeric() && b != b'_')
                    || k.starts_with(|c: char| c.is_ascii_digit())
            }
        {
            return Err(device_invalid());
        }
    }
    if headers {
        let mut unique = std::collections::BTreeSet::new();
        if values
            .keys()
            .any(|k| !unique.insert(k.to_ascii_lowercase()))
        {
            return Err(device_invalid());
        }
    }
    Ok(())
}
pub fn configuration(value: &Value) -> Result<Configuration> {
    let c: Configuration = serde_json::from_value(value.clone()).map_err(|_| device_invalid())?;
    valid_text(&c.name, 200)?;
    if c.expected_revision.is_some_and(|v| v <= 0)
        || c.args.len() > 128
        || c.args.iter().map(String::len).sum::<usize>() > 32768
        || c.args.iter().any(|s| s.contains('\0'))
    {
        return Err(device_invalid());
    }
    bounded(&c.environment, false)?;
    bounded(&c.headers, true)?;
    match c.transport.as_str() {
        "stdio" => {
            if !c.url.is_empty()
                || !c.headers.is_empty()
                || !c.command.starts_with('/')
                || c.command.len() > 2000
                || c.command.contains(['\0', '\\', ':'])
                || c.command
                    .split('/')
                    .skip(1)
                    .any(|s| s.is_empty() || s == "." || s == "..")
            {
                return Err(device_invalid());
            }
        }
        "streamable_http" | "sse" => {
            if !c.command.is_empty() || !c.args.is_empty() || !c.environment.is_empty() {
                return Err(device_invalid());
            }
            let url = reqwest::Url::parse(&c.url).map_err(|_| device_invalid())?;
            let host = url.host_str().ok_or_else(device_invalid)?;
            if url.scheme() != "https"
                || url.port_or_known_default() != Some(443)
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
                || host.parse::<std::net::IpAddr>().is_ok()
                || !host.contains('.')
                || host.ends_with('.')
                || ["localhost", "local", "internal", "lan", "test", "invalid"]
                    .iter()
                    .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))
            {
                return Err(device_invalid());
            }
        }
        _ => return Err(device_invalid()),
    }
    Ok(c)
}
async fn metadata(
    tx: &mut Transaction<'_, Any>,
    owner: Uuid,
    project: Uuid,
    entity: Uuid,
) -> Result<(i64, String)> {
    let r=sqlx::query("SELECT project_id,revision,digest FROM android_wire_sync_current WHERE owner_id=$1 AND entity_type='mcp_metadata' AND entity_id=$2 AND team_id IS NULL AND tombstone=0").bind(owner.to_string()).bind(entity.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
    if r.try_get::<Option<String>, _>("project_id")
        .map_err(storage_error)?
        .is_some_and(|p| p != project.to_string())
    {
        return Err(Error::Forbidden);
    }
    Ok((
        r.try_get("revision").map_err(storage_error)?,
        r.try_get("digest").map_err(storage_error)?,
    ))
}
fn scope(
    owner: Uuid,
    project: Uuid,
    entity: Uuid,
    revision: i64,
    sha: &str,
    key: KeyId,
) -> Result<String> {
    json_string(&(
        "android-private-mcp-binding/v1",
        owner,
        project,
        entity,
        revision,
        sha,
        key,
    ))
}
fn seal(
    keys: &dyn KeyStore,
    key_id: KeyId,
    owner: Uuid,
    project: Uuid,
    entity: Uuid,
    revision: i64,
    sha: &str,
    value: &Value,
) -> Result<Value> {
    let cipher = keys
        .key_for_purpose(&key_id, cowork_secrets::KeyPurpose::DataEncryption)
        .map_err(|_| Error::NativeKeyUnavailable)?;
    seal_with_auth(
        keys, cipher, key_id, owner, project, entity, revision, sha, value,
    )
}
fn scope_v2(
    owner: Uuid,
    project: Uuid,
    entity: Uuid,
    revision: i64,
    sha: &str,
    cipher: KeyId,
    auth: KeyId,
) -> Result<String> {
    json_string(&(
        "android-private-mcp-binding/v2",
        owner,
        project,
        entity,
        revision,
        sha,
        cipher,
        auth,
    ))
}
fn seal_with_auth(
    keys: &dyn KeyStore,
    key_id: KeyId,
    auth_key_id: KeyId,
    owner: Uuid,
    project: Uuid,
    entity: Uuid,
    revision: i64,
    sha: &str,
    value: &Value,
) -> Result<Value> {
    key_id.validate().map_err(|_| Error::NativeKeyUnavailable)?;
    auth_key_id
        .validate()
        .map_err(|_| Error::NativeKeyUnavailable)?;
    let mut bytes = Zeroizing::new(serde_json::to_vec(value).map_err(storage_error)?);
    if bytes.len() > 128 * 1024 {
        return Err(Error::BudgetExceeded);
    }
    let key = keys
        .load(&key_id)
        .map_err(|_| Error::NativeKeyUnavailable)?;
    let key = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::CHACHA20_POLY1305, key.as_ref())
            .map_err(|_| Error::NativeKeyUnavailable)?,
    );
    let mut nonce = [0; 12];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| Error::NativeKeyUnavailable)?;
    key.seal_in_place_append_tag(
        aead::Nonce::assume_unique_for_key(nonce),
        aead::Aad::from(
            scope_v2(owner, project, entity, revision, sha, key_id, auth_key_id)?.as_bytes(),
        ),
        &mut *bytes,
    )
    .map_err(|_| Error::NativeKeyUnavailable)?;
    Ok(
        json!({"format":2,"key_id":key_id,"auth_key_id":auth_key_id,"nonce":STANDARD.encode(nonce),"ciphertext":STANDARD.encode(bytes.as_slice())}),
    )
}
fn open(
    keys: &dyn KeyStore,
    owner: Uuid,
    project: Uuid,
    entity: Uuid,
    r: &AnyRow,
) -> Result<Value> {
    let v: Value = parse(&r.try_get::<String, _>("sealed").map_err(storage_error)?)?;
    if !matches!(v["format"].as_u64(), Some(1 | 2)) {
        return Err(Error::Forbidden);
    }
    let key_id: KeyId =
        serde_json::from_value(v["key_id"].clone()).map_err(|_| device_invalid())?;
    let auth_key_id: KeyId = if v["format"] == 1 {
        key_id
    } else {
        serde_json::from_value(v["auth_key_id"].clone()).map_err(|_| device_invalid())?
    };
    let sha = r
        .try_get::<String, _>("binding_sha")
        .map_err(storage_error)?;
    let revision = r.try_get::<i64, _>("revision").map_err(storage_error)?;
    let key = keys
        .load(&key_id)
        .map_err(|_| Error::NativeKeyUnavailable)?;
    let key = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::CHACHA20_POLY1305, key.as_ref())
            .map_err(|_| Error::NativeKeyUnavailable)?,
    );
    let nonce: [u8; 12] = STANDARD
        .decode(v["nonce"].as_str().ok_or_else(device_invalid)?)
        .map_err(|_| device_invalid())?
        .try_into()
        .map_err(|_| device_invalid())?;
    let mut bytes = Zeroizing::new(
        STANDARD
            .decode(v["ciphertext"].as_str().ok_or_else(device_invalid)?)
            .map_err(|_| device_invalid())?,
    );
    if bytes.len() > 128 * 1024 + 16 {
        return Err(Error::BudgetExceeded);
    }
    let plain = key
        .open_in_place(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(
                if v["format"] == 1 {
                    scope(owner, project, entity, revision, &sha, key_id)?
                } else {
                    scope_v2(owner, project, entity, revision, &sha, key_id, auth_key_id)?
                }
                .as_bytes(),
            ),
            &mut bytes,
        )
        .map_err(|_| Error::Forbidden)?;
    let value: Value = serde_json::from_slice(plain).map_err(storage_error)?;
    // Rewrap changes only cipher custody; original HMAC authority remains explicitly bound.
    if request_mac(
        keys,
        auth_key_id,
        &json!({"owner":owner,"project":project,"entity":entity,"revision":revision,"configuration":value}),
    )? != sha
    {
        return Err(Error::Forbidden);
    }
    if r.try_get::<i64, _>("deleted").map_err(storage_error)? == 0 {
        configuration(&value)?;
    } else if !value.as_object().is_some_and(|v| v.len() == 1)
        || value["expected_revision"].as_i64().is_none_or(|v| v <= 0)
    {
        return Err(Error::Forbidden);
    }
    Ok(value)
}
/// KMS caller must CAS the original row/envelope in its current authenticated transaction.
/// This validates original cipher+HMAC first and never changes business revision, MAC or operation receipts.
pub(crate) fn rewrap_private_mcp_binding(
    keys: &dyn KeyStore,
    target: KeyId,
    owner: Uuid,
    project: Uuid,
    entity: Uuid,
    r: &AnyRow,
) -> Result<Value> {
    let original: Value = parse(&r.try_get::<String, _>("sealed").map_err(storage_error)?)?;
    let cipher: KeyId =
        serde_json::from_value(original["key_id"].clone()).map_err(|_| device_invalid())?;
    let auth: KeyId = if original["format"] == 1 {
        cipher
    } else {
        serde_json::from_value(original["auth_key_id"].clone()).map_err(|_| device_invalid())?
    };
    let value = open(keys, owner, project, entity, r)?;
    if target == cipher {
        return Ok(original);
    }
    let revision = r.try_get::<i64, _>("revision").map_err(storage_error)?;
    let binding = r
        .try_get::<String, _>("binding_sha")
        .map_err(storage_error)?;
    seal_with_auth(
        keys, target, auth, owner, project, entity, revision, &binding, &value,
    )
}
pub(crate) async fn rewrap_android_server_secrets(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    target: KeyId,
) -> Result<u64> {
    crate::android_secret_rotation::rewrap(tx, keys, target).await
}
fn request_mac(keys: &dyn KeyStore, key_id: KeyId, value: &Value) -> Result<String> {
    let key = keys
        .load(&key_id)
        .map_err(|_| Error::NativeKeyUnavailable)?;
    let raw = Zeroizing::new(serde_json::to_vec(value).map_err(storage_error)?);
    let mut context = hmac::Context::with_key(&hmac::Key::new(hmac::HMAC_SHA256, key.as_ref()));
    context.update(b"android-private-mcp-request/v1\0");
    context.update(&raw);
    Ok(context
        .sign()
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
pub(crate) async fn validate_bindings(
    tx: &mut Transaction<'_, Any>,
    owner: Uuid,
    project: Uuid,
    input: &Value,
) -> Result<()> {
    let Some(frozen) = input.get("frozen_mcp_bindings") else {
        return Ok(());
    };
    let frozen = frozen
        .as_array()
        .filter(|v| v.len() <= 16)
        .ok_or_else(device_invalid)?;
    let mut seen = std::collections::BTreeSet::new();
    for binding in frozen {
        let entity = crate::android_personal::wire_uuid(binding, "entity_id")?;
        if !seen.insert(entity) {
            return Err(device_invalid());
        }
        let current=sqlx::query("SELECT revision,metadata_revision,metadata_sha,binding_sha FROM android_private_mcp_bindings WHERE owner_id=$1 AND project_id=$2 AND entity_id=$3 AND deleted=0").bind(owner.to_string()).bind(project.to_string()).bind(entity.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        let observed = metadata(tx, owner, project, entity).await?;
        if binding["revision"].as_i64()
            != Some(
                current
                    .try_get::<i64, _>("revision")
                    .map_err(storage_error)?,
            )
            || binding["binding_sha256"]
                != json!(
                    current
                        .try_get::<String, _>("binding_sha")
                        .map_err(storage_error)?
                )
            || binding["metadata_revision"].as_i64() != Some(observed.0)
            || binding["metadata_sha256"] != json!(observed.1.clone())
            || observed
                != (
                    current
                        .try_get("metadata_revision")
                        .map_err(storage_error)?,
                    current.try_get("metadata_sha").map_err(storage_error)?,
                )
        {
            return Err(Error::Forbidden);
        }
    }
    Ok(())
}
impl Store {
    pub async fn android_private_mcp_bindings(
        &self,
        actor: &Principal,
        project: Uuid,
    ) -> Result<Vec<Value>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.android_personal_permit(&mut tx, actor, project, ContentAction::Read)
            .await?;
        let rows:Vec<String>=sqlx::query_scalar("SELECT payload FROM android_private_mcp_bindings WHERE owner_id=$1 AND project_id=$2 AND deleted=0 ORDER BY entity_id LIMIT 129").bind(actor.identity_id().to_string()).bind(project.to_string()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        if rows.len() > 128 {
            return Err(Error::BudgetExceeded);
        }
        let values = rows.iter().map(|s| parse(s)).collect::<Result<Vec<_>>>()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(values)
    }
    pub async fn android_private_mcp_mutate(
        &self,
        actor: &Principal,
        project: Uuid,
        entity: Uuid,
        body: &Value,
        remove: bool,
        operation: Option<&str>,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<Value> {
        let c = if remove {
            None
        } else {
            Some(configuration(body)?)
        };
        if remove
            && (!body.as_object().is_some_and(|v| v.len() == 1)
                || body["expected_revision"].as_i64().is_none_or(|v| v <= 0))
        {
            return Err(device_invalid());
        }
        let digest = request_mac(
            keys,
            key_id,
            &json!({"owner":actor.identity_id(),"project":project,"entity":entity,"remove":remove,"request":body}),
        )?;
        let operation = if let Some(operation) = operation {
            operation.to_owned()
        } else {
            let stable = keys
                .key_for_purpose(&key_id, cowork_secrets::KeyPurpose::RequestFingerprint)
                .map_err(|_| Error::NativeKeyUnavailable)?;
            format!(
                "legacy:{}",
                request_mac(
                    keys,
                    stable,
                    &json!({"owner":actor.identity_id(),"project":project,"entity":entity,"remove":remove,"request":body})
                )?
            )
        };
        valid_text(&operation, 200)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let stamp = device_writer(&mut tx).await?;
        self.android_personal_permit(&mut tx, actor, project, ContentAction::Write)
            .await?;
        if let Some(old)=sqlx::query("SELECT request_mac,request_key_id,receipt FROM android_private_mcp_operations WHERE owner_id=$1 AND project_id=$2 AND operation_key=$3").bind(actor.identity_id().to_string()).bind(project.to_string()).bind(&operation).fetch_optional(&mut *tx).await.map_err(storage_error)?{let original_key:KeyId=parse(&old.try_get::<String,_>("request_key_id").map_err(storage_error)?)?;let original_digest=request_mac(keys,original_key,&json!({"owner":actor.identity_id(),"project":project,"entity":entity,"remove":remove,"request":body}))?;if old.try_get::<String,_>("request_mac").map_err(storage_error)?!=original_digest{return Err(Error::Conflict);}let receipt=parse(&old.try_get::<String,_>("receipt").map_err(storage_error)?)?;tx.commit().await.map_err(storage_error)?;return Ok(receipt);}
        let old=sqlx::query("SELECT revision,payload,deleted,metadata_revision,metadata_sha FROM android_private_mcp_bindings WHERE owner_id=$1 AND project_id=$2 AND entity_id=$3").bind(actor.identity_id().to_string()).bind(project.to_string()).bind(entity.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        let (metadata_revision, metadata_sha) = if remove {
            let old = old.as_ref().ok_or(Error::NotFound)?;
            (
                old.try_get::<i64, _>("metadata_revision")
                    .map_err(storage_error)?,
                old.try_get::<String, _>("metadata_sha")
                    .map_err(storage_error)?,
            )
        } else {
            metadata(&mut tx, actor.identity_id(), project, entity).await?
        };
        let expected = body["expected_revision"].as_i64();
        let old_revision = old
            .as_ref()
            .map(|r| r.try_get::<i64, _>("revision").map_err(storage_error))
            .transpose()?;
        let live_revision = if old
            .as_ref()
            .is_some_and(|r| r.try_get::<i64, _>("deleted").ok() == Some(0))
        {
            old_revision
        } else {
            None
        };
        if expected != live_revision || remove && live_revision.is_none() {
            return Err(Error::Conflict);
        }
        let revision = old_revision
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(Error::Conflict)?;
        let created =
            old.as_ref()
                .map(|r| {
                    parse::<Value>(&r.try_get::<String, _>("payload").map_err(storage_error)?)
                        .and_then(|v| {
                            v["created_at"]
                                .as_str()
                                .map(str::to_owned)
                                .ok_or_else(device_invalid)
                        })
                })
                .transpose()?
                .unwrap_or(device_iso(stamp)?);
        let receipt = if let Some(c) = c {
            json!({"schema_version":2,"project_id":project,"mcp_entity_id":entity,"revision":revision,"etag":format!("{entity}:{revision}"),"name":c.name,"transport":c.transport,"executable_hint":if c.transport=="stdio"{c.command.rsplit('/').next().unwrap_or("")}else{"HTTPS endpoint"},"argument_count":c.args.len(),"environment_keys":if c.transport=="stdio"{c.environment.keys().cloned().collect::<Vec<_>>()}else{c.headers.keys().cloned().collect::<Vec<_>>()},"created_at":created,"updated_at":device_iso(stamp)?})
        } else {
            let mut value: Value = parse(
                &old.as_ref()
                    .ok_or(Error::NotFound)?
                    .try_get::<String, _>("payload")
                    .map_err(storage_error)?,
            )?;
            value["deleted"] = json!(true);
            value["revision"] = json!(revision);
            value["etag"] = json!(format!("{entity}:{revision}"));
            value["updated_at"] = json!(device_iso(stamp)?);
            value
        };
        let configuration = body.clone();
        let binding = request_mac(
            keys,
            key_id,
            &json!({"owner":actor.identity_id(),"project":project,"entity":entity,"revision":revision,"configuration":configuration}),
        )?;
        let sealed = seal(
            keys,
            key_id,
            actor.identity_id(),
            project,
            entity,
            revision,
            &binding,
            &configuration,
        )?;
        device_writer(&mut tx).await?;
        let cipher: KeyId = serde_json::from_value(sealed["key_id"].clone())
            .map_err(|_| Error::NativeKeyUnavailable)?;
        crate::server_keys::cipher_writer(&mut tx, cipher).await?;
        self.android_personal_permit(&mut tx, actor, project, ContentAction::Write)
            .await?;
        if !remove
            && metadata(&mut tx, actor.identity_id(), project, entity).await?
                != (metadata_revision, metadata_sha.clone())
        {
            return Err(Error::Forbidden);
        }
        sqlx::query("INSERT INTO android_private_mcp_bindings(owner_id,project_id,entity_id,revision,metadata_revision,metadata_sha,payload,binding_sha,sealed,deleted) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) ON CONFLICT(owner_id,project_id,entity_id) DO UPDATE SET revision=excluded.revision,metadata_revision=excluded.metadata_revision,metadata_sha=excluded.metadata_sha,payload=excluded.payload,binding_sha=excluded.binding_sha,sealed=excluded.sealed,deleted=excluded.deleted").bind(actor.identity_id().to_string()).bind(project.to_string()).bind(entity.to_string()).bind(revision).bind(metadata_revision).bind(metadata_sha).bind(json_string(&receipt)?).bind(binding).bind(json_string(&sealed)?).bind(i64::from(remove)).execute(&mut *tx).await.map_err(storage_error)?;
        sqlx::query("INSERT INTO android_private_mcp_operations(owner_id,project_id,operation_key,request_mac,request_key_id,receipt) VALUES($1,$2,$3,$4,$5,$6)").bind(actor.identity_id().to_string()).bind(project.to_string()).bind(operation).bind(digest).bind(json_string(&key_id)?).bind(json_string(&receipt)?).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
    pub async fn android_private_mcp_freeze(
        &self,
        actor: &Principal,
        project: Uuid,
        ids: &[Uuid],
    ) -> Result<Vec<Value>> {
        if ids.len() > 16
            || ids.iter().collect::<std::collections::BTreeSet<_>>().len() != ids.len()
        {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.android_personal_permit(&mut tx, actor, project, ContentAction::Execute)
            .await?;
        let mut values = Vec::new();
        for entity in ids {
            let r=sqlx::query("SELECT revision,metadata_revision,metadata_sha,binding_sha FROM android_private_mcp_bindings WHERE owner_id=$1 AND project_id=$2 AND entity_id=$3 AND deleted=0").bind(actor.identity_id().to_string()).bind(project.to_string()).bind(entity.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
            let observed = metadata(&mut tx, actor.identity_id(), project, *entity).await?;
            if observed
                != (
                    r.try_get("metadata_revision").map_err(storage_error)?,
                    r.try_get("metadata_sha").map_err(storage_error)?,
                )
            {
                return Err(Error::Forbidden);
            }
            values.push(json!({"entity_id":entity,"revision":r.try_get::<i64,_>("revision").map_err(storage_error)?,"metadata_revision":observed.0,"metadata_sha256":observed.1,"binding_sha256":r.try_get::<String,_>("binding_sha").map_err(storage_error)?}));
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(values)
    }
    pub async fn android_private_mcp_open_for_run(
        &self,
        run: Uuid,
        lease: Uuid,
        frozen: &Value,
        keys: &dyn KeyStore,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let stamp = device_writer(&mut tx).await?;
        let r = crate::android_private_server::leased(&mut tx, run, lease, stamp).await?;
        let owner = crate::android_private_server::id(&r, "owner_id")?;
        let project = crate::android_private_server::id(&r, "project_id")?;
        let entity = crate::android_personal::wire_uuid(frozen, "entity_id")?;
        let stored: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
        if !stored["spec"]["input"]["frozen_mcp_bindings"]
            .as_array()
            .is_some_and(|v| v.contains(frozen))
        {
            return Err(Error::Forbidden);
        }
        validate_bindings(&mut tx, owner, project, &stored["spec"]["input"]).await?;
        let binding=sqlx::query("SELECT * FROM android_private_mcp_bindings WHERE owner_id=$1 AND project_id=$2 AND entity_id=$3 AND deleted=0").bind(owner.to_string()).bind(project.to_string()).bind(entity.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        if frozen["revision"].as_i64()
            != Some(
                binding
                    .try_get::<i64, _>("revision")
                    .map_err(storage_error)?,
            )
            || frozen["binding_sha256"]
                != json!(
                    binding
                        .try_get::<String, _>("binding_sha")
                        .map_err(storage_error)?
                )
            || metadata(&mut tx, owner, project, entity).await?
                != (
                    binding
                        .try_get("metadata_revision")
                        .map_err(storage_error)?,
                    binding.try_get("metadata_sha").map_err(storage_error)?,
                )
        {
            return Err(Error::Forbidden);
        }
        let value = open(keys, owner, project, entity, &binding)?;
        let stamp = device_writer(&mut tx).await?;
        crate::android_private_server::leased(&mut tx, run, lease, stamp).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
}
