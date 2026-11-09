//! Private user-selected cache intake. Old account IDs, outbox intents and run states confer no authority.
use super::*;
use crate::android_devices::{device_invalid, device_iso, device_writer};
use base64::{Engine, engine::general_purpose::STANDARD};
use cowork_identity::{ContentAction, Principal};
use cowork_secrets::{KeyId, KeyStore};
use sqlx::{Any, Transaction, any::AnyRow};
use zeroize::Zeroizing;
#[path = "android_mobile_cache_cipher.rs"]
pub(crate) mod cipher;
pub const CACHE_PART: usize = 1024 * 1024;
pub const CACHE_ARCHIVE_MAX: u64 = 24 * 1024 * 1024;
const PAYLOAD_MAX: usize = 8 * 1024 * 1024;
async fn authorize(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    action: ContentAction,
) -> Result<()> {
    store
        .identity()
        .require_global_session_in_transaction(tx, actor)
        .await
        .map_err(team_service::identity_error)?;
    store
        .identity()
        .require_session_action_in_transaction(tx, actor, action)
        .await
        .map_err(team_service::identity_error)
}
async fn row(tx: &mut Transaction<'_, Any>, owner: Uuid, id: Uuid) -> Result<AnyRow> {
    sqlx::query("SELECT * FROM android_mobile_cache_imports WHERE id=$1 AND owner_id=$2")
        .bind(id.to_string())
        .bind(owner.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)
}
fn view(r: &AnyRow) -> Result<Value> {
    Ok(
        json!({"schema_version":2,"id":r.try_get::<String,_>("id").map_err(storage_error)?,"revision":r.try_get::<i64,_>("revision").map_err(storage_error)?,"state":r.try_get::<String,_>("state").map_err(storage_error)?,"archive_sha256":r.try_get::<String,_>("archive_sha256").map_err(storage_error)?,"archive_size":r.try_get::<i64,_>("archive_size").map_err(storage_error)?,"entry_count":r.try_get::<i64,_>("entry_count").map_err(storage_error)?,"payload_sha256":r.try_get::<Option<String>,_>("payload_sha256").map_err(storage_error)?,"source":r.try_get::<Option<String>,_>("source").map_err(storage_error)?.map(|v|parse::<Value>(&v)).transpose()?,"error":r.try_get::<Option<String>,_>("error").map_err(storage_error)?,"original_account_authority":false,"outbox_auto_dispatch":false}),
    )
}
async fn bytes(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    owner: Uuid,
    id: Uuid,
    stream: &str,
    max: usize,
) -> Result<Zeroizing<Vec<u8>>> {
    let mut output = Zeroizing::new(Vec::new());
    let mut ordinal = 0i64;
    loop {
        let Some(r)=sqlx::query("SELECT * FROM android_mobile_cache_chunks WHERE import_id=$1 AND stream=$2 AND ordinal=$3").bind(id.to_string()).bind(stream).bind(ordinal).fetch_optional(&mut **tx).await.map_err(storage_error)?else{break};
        let part = cipher::open(keys, owner, &r)?;
        if output.len().checked_add(part.len()).is_none_or(|n| n > max) {
            return Err(Error::BudgetExceeded);
        }
        output.extend_from_slice(&part);
        ordinal += 1;
    }
    Ok(output)
}
async fn put(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    keys: &dyn KeyStore,
    key: KeyId,
    id: Uuid,
    stream: &str,
    raw: &[u8],
) -> Result<()> {
    for (ordinal, part) in raw.chunks(CACHE_PART).enumerate() {
        let sealed = cipher::seal(
            keys,
            key,
            actor.identity_id(),
            id,
            stream,
            ordinal as i64,
            part,
        )?;
        crate::server_keys::cipher_writer(tx, key).await?;
        authorize(store, tx, actor, ContentAction::Write).await?;
        let encoded = json_string(&sealed)?;
        crate::android_administration_control::charge(
            tx,
            actor.identity_id(),
            encoded.len() as i64 + 512,
            0,
        )
        .await?;
        sqlx::query("INSERT INTO android_mobile_cache_chunks(import_id,stream,ordinal,digest,size,sealed) VALUES($1,$2,$3,$4,$5,$6)").bind(id.to_string()).bind(stream).bind(ordinal as i64).bind(hash(part)).bind(part.len() as i64).bind(encoded).execute(&mut **tx).await.map_err(storage_error)?;
    }
    Ok(())
}
fn exported(archive: &[u8]) -> Result<(Zeroizing<Vec<u8>>, Value, Value)> {
    let wrapper: Value = serde_json::from_slice(archive).map_err(|_| device_invalid())?;
    if wrapper["format"] != "localai-cowork-mobile-cache-export/1"
        || wrapper.as_object().is_none_or(|o| {
            o.keys().any(|k| {
                ![
                    "format",
                    "payload_base64",
                    "payload_sha256",
                    "payload_size",
                    "ciphertext_base64",
                    "ciphertext_sha256",
                    "ciphertext_size",
                    "source",
                ]
                .contains(&k.as_str())
            })
        })
    {
        return Err(device_invalid());
    }
    let raw = Zeroizing::new(
        STANDARD
            .decode(
                wrapper["payload_base64"]
                    .as_str()
                    .filter(|s| s.len() <= PAYLOAD_MAX.div_ceil(3) * 4)
                    .ok_or_else(device_invalid)?,
            )
            .map_err(|_| device_invalid())?,
    );
    let cipher = Zeroizing::new(
        STANDARD
            .decode(
                wrapper["ciphertext_base64"]
                    .as_str()
                    .filter(|s| s.len() <= (PAYLOAD_MAX + 28).div_ceil(3) * 4)
                    .ok_or_else(device_invalid)?,
            )
            .map_err(|_| device_invalid())?,
    );
    if raw.len() > PAYLOAD_MAX
        || cipher.len() != raw.len() + 28
        || wrapper["payload_size"].as_u64() != Some(raw.len() as u64)
        || wrapper["ciphertext_size"].as_u64() != Some(cipher.len() as u64)
        || wrapper["payload_sha256"] != hash(&raw)
        || wrapper["ciphertext_sha256"] != hash(&cipher)
        || STANDARD.encode(raw.as_slice()) != wrapper["payload_base64"]
        || STANDARD.encode(cipher.as_slice()) != wrapper["ciphertext_base64"]
    {
        return Err(Error::Forbidden);
    }
    let source = &wrapper["source"];
    if source["keystore_namespace"] != "offline_cache"
        || source["keystore_key"] != "aes_gcm_key_v1"
        || source["key_never_exported"] != true
        || source.as_object().is_none_or(|o| {
            o.keys().any(|k| {
                ![
                    "storage_key",
                    "aad_base64",
                    "keystore_namespace",
                    "keystore_key",
                    "key_never_exported",
                ]
                .contains(&k.as_str())
            })
        })
    {
        return Err(device_invalid());
    }
    let payload: Value = serde_json::from_slice(&raw).map_err(|_| device_invalid())?;
    if !matches!(payload["schemaVersion"].as_u64(), Some(1 | 2))
        || !payload["runs"].is_array()
        || !payload["outbox"].is_array()
        || !payload["events"].is_object()
        || !payload["messages"].is_object()
    {
        return Err(device_invalid());
    }
    let aad = STANDARD
        .decode(
            source["aad_base64"]
                .as_str()
                .filter(|s| s.len() <= 8192)
                .ok_or_else(device_invalid)?,
        )
        .map_err(|_| device_invalid())?;
    if source["storage_key"] == "open-cowork-mobile-cache-v1" {
        if aad != b"open-cowork-mobile-cache-v1"
            || payload.get("binding").is_some_and(|v| !v.is_null())
        {
            return Err(Error::Forbidden);
        }
    } else {
        let binding = &payload["binding"];
        let origin = binding["serverOrigin"]
            .as_str()
            .ok_or_else(device_invalid)?;
        let user = Uuid::parse_str(binding["userId"].as_str().ok_or_else(device_invalid)?)
            .map_err(|_| device_invalid())?;
        let parsed = reqwest::Url::parse(origin).map_err(|_| device_invalid())?;
        if !matches!(parsed.scheme(), "https" | "http")
            || parsed.origin().ascii_serialization() != origin
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || payload["schemaVersion"] != 2
        {
            return Err(Error::Forbidden);
        }
        let actual: Value = serde_json::from_slice(&aad).map_err(|_| device_invalid())?;
        if actual
            != json!({"format":"open-cowork-mobile-cache-v2","serverOrigin":origin,"userId":user})
        {
            return Err(Error::Forbidden);
        }
        use sha2::Digest;
        let suffix = STANDARD
            .encode(sha2::Sha256::digest(&aad))
            .replace('/', "_")
            .replace('+', "-")
            .replace('=', "");
        if source["storage_key"] != format!("open-cowork-mobile-cache-v2:{suffix}") {
            return Err(Error::Forbidden);
        }
    }
    let metadata = json!({"format":wrapper["format"],"source":source,"payload_size":raw.len(),"payload_sha256":wrapper["payload_sha256"],"ciphertext_size":cipher.len(),"ciphertext_sha256":wrapper["ciphertext_sha256"],"original_binding":payload.get("binding").cloned(),"authority":"user_selected_inert_content","cryptographic_android_attestation":false});
    Ok((raw, payload, metadata))
}
impl Store {
    pub async fn android_cache_begin(
        &self,
        actor: &Principal,
        request: Uuid,
        size: u64,
        digest: &str,
    ) -> Result<Value> {
        if request.is_nil()
            || size == 0
            || size > CACHE_ARCHIVE_MAX
            || digest.len() != 64
            || !digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        authorize(self, &mut tx, actor, ContentAction::Read).await?;
        let sha = canonical_hash(
            &json!({"request_id":request,"archive_size":size,"archive_sha256":digest}),
        );
        let operation = format!("cache-begin:{request}");
        if let Some(v) = crate::android_administration_control::original(
            &mut tx,
            actor.identity_id(),
            &operation,
            "cache_begin",
            &sha,
        )
        .await?
        {
            authorize(self, &mut tx, actor, ContentAction::Read).await?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(v);
        }
        authorize(self, &mut tx, actor, ContentAction::Write).await?;
        if let Some(old) = sqlx::query(
            "SELECT * FROM android_mobile_cache_imports WHERE owner_id=$1 AND request_id=$2",
        )
        .bind(actor.identity_id().to_string())
        .bind(request.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?
        {
            if old
                .try_get::<String, _>("archive_sha256")
                .map_err(storage_error)?
                != digest
                || old
                    .try_get::<i64, _>("archive_size")
                    .map_err(storage_error)?
                    != size as i64
            {
                return Err(Error::Conflict);
            }
            // A row without its original receipt cannot be adopted as a historical ACK.
            return Err(Error::Conflict);
        }
        let reserved = (size as i64).checked_mul(2).ok_or(Error::BudgetExceeded)?;
        crate::android_administration_control::charge(&mut tx, actor.identity_id(), 0, reserved)
            .await?;
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO android_mobile_cache_imports(id,owner_id,request_id,archive_sha256,archive_size,revision,state,reserved_bytes,entry_count,created_at) VALUES($1,$2,$3,$4,$5,1,'receiving',$6,0,$7)").bind(id.to_string()).bind(actor.identity_id().to_string()).bind(request.to_string()).bind(digest).bind(size as i64).bind(reserved).bind(current).execute(&mut *tx).await.map_err(storage_error)?;
        let mut v = view(&row(&mut tx, actor.identity_id(), id).await?)?;
        v["request_sha256"] = json!(sha);
        v["operation_key"] = json!(operation);
        crate::android_administration_control::remember(
            &mut tx,
            actor.identity_id(),
            &operation,
            "cache_begin",
            &sha,
            &v,
            current,
        )
        .await?;
        authorize(self, &mut tx, actor, ContentAction::Write).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn android_cache_status(&self, actor: &Principal, id: Uuid) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        authorize(self, &mut tx, actor, ContentAction::Read).await?;
        let mut v = view(&row(&mut tx, actor.identity_id(), id).await?)?;
        let parts=sqlx::query("SELECT ordinal,size,digest FROM android_mobile_cache_chunks WHERE import_id=$1 AND stream='archive' ORDER BY ordinal LIMIT 25").bind(id.to_string()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        if parts.len() > 24 {
            return Err(Error::BudgetExceeded);
        }
        let mut items = Vec::new();
        for p in parts {
            items.push(json!({"ordinal":p.try_get::<i64,_>("ordinal").map_err(storage_error)?,"size":p.try_get::<i64,_>("size").map_err(storage_error)?,"sha256":p.try_get::<String,_>("digest").map_err(storage_error)?}));
        }
        v["archive_parts"] = json!(items);
        authorize(self, &mut tx, actor, ContentAction::Read).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn android_cache_list(
        &self,
        actor: &Principal,
        after: Option<Uuid>,
        limit: u32,
    ) -> Result<Value> {
        if !(1..=100).contains(&limit) {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        authorize(self, &mut tx, actor, ContentAction::Read).await?;
        let rows=sqlx::query("SELECT * FROM android_mobile_cache_imports WHERE owner_id=$1 AND id>$2 ORDER BY id LIMIT $3").bind(actor.identity_id().to_string()).bind(after.map(|v|v.to_string()).unwrap_or_default()).bind(i64::from(limit)+1).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let more = rows.len() > limit as usize;
        let items = rows
            .iter()
            .take(limit as usize)
            .map(view)
            .collect::<Result<Vec<_>>>()?;
        let next = if more {
            items.last().map(|v| v["id"].clone())
        } else {
            None
        };
        authorize(self, &mut tx, actor, ContentAction::Read).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(json!({"items":items,"next_cursor":next}))
    }
    pub async fn android_cache_upload(
        &self,
        actor: &Principal,
        id: Uuid,
        ordinal: u64,
        raw: &[u8],
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<Value> {
        let key = crate::android_private_workspace::writer_cipher(keys, key)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        crate::server_keys::cipher_writer(&mut tx, key).await?;
        device_writer(&mut tx).await?;
        authorize(self, &mut tx, actor, ContentAction::Write).await?;
        let r = row(&mut tx, actor.identity_id(), id).await?;
        let total = r.try_get::<i64, _>("archive_size").map_err(storage_error)? as u64;
        let offset = ordinal
            .checked_mul(CACHE_PART as u64)
            .ok_or_else(device_invalid)?;
        if offset >= total || raw.len() != (total - offset).min(CACHE_PART as u64) as usize {
            return Err(device_invalid());
        }
        if let Some(old)=sqlx::query("SELECT * FROM android_mobile_cache_chunks WHERE import_id=$1 AND stream='archive' AND ordinal=$2").bind(id.to_string()).bind(ordinal as i64).fetch_optional(&mut *tx).await.map_err(storage_error)?{if cipher::open(keys,actor.identity_id(),&old)?.as_slice()!=raw{return Err(Error::Conflict)}let v=view(&r)?;tx.commit().await.map_err(storage_error)?;return Ok(v)}
        if r.try_get::<String, _>("state").map_err(storage_error)? != "receiving" {
            return Err(Error::Conflict);
        }
        let sealed = cipher::seal(
            keys,
            key,
            actor.identity_id(),
            id,
            "archive",
            ordinal as i64,
            raw,
        )?;
        crate::server_keys::cipher_writer(&mut tx, key).await?;
        authorize(self, &mut tx, actor, ContentAction::Write).await?;
        let encoded = json_string(&sealed)?;
        crate::android_administration_control::charge(
            &mut tx,
            actor.identity_id(),
            encoded.len() as i64 + 512,
            0,
        )
        .await?;
        sqlx::query("INSERT INTO android_mobile_cache_chunks(import_id,stream,ordinal,digest,size,sealed) VALUES($1,'archive',$2,$3,$4,$5)").bind(id.to_string()).bind(ordinal as i64).bind(hash(raw)).bind(raw.len() as i64).bind(encoded).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(view(&r)?)
    }
    pub async fn android_cache_index(
        &self,
        actor: &Principal,
        id: Uuid,
        expected: i64,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<Value> {
        let key = crate::android_private_workspace::writer_cipher(keys, key)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        crate::server_keys::cipher_writer(&mut tx, key).await?;
        device_writer(&mut tx).await?;
        authorize(self, &mut tx, actor, ContentAction::Write).await?;
        let r = row(&mut tx, actor.identity_id(), id).await?;
        let op = format!("cache-index:{id}:{expected}");
        let sha = canonical_hash(
            &json!({"import_id":id,"expected_revision":expected,"archive_sha256":r.try_get::<String,_>("archive_sha256").map_err(storage_error)?}),
        );
        if let Some(original) = crate::android_administration_control::original(
            &mut tx,
            actor.identity_id(),
            &op,
            "cache_index",
            &sha,
        )
        .await?
        {
            tx.commit().await.map_err(storage_error)?;
            return Ok(original);
        }
        if r.try_get::<String, _>("state").map_err(storage_error)? != "receiving" {
            return Err(Error::Conflict);
        }
        if r.try_get::<i64, _>("revision").map_err(storage_error)? != expected {
            return Err(Error::Conflict);
        }
        let archive = bytes(
            &mut tx,
            keys,
            actor.identity_id(),
            id,
            "archive",
            CACHE_ARCHIVE_MAX as usize,
        )
        .await?;
        if archive.len() as i64 != r.try_get::<i64, _>("archive_size").map_err(storage_error)?
            || hash(&archive)
                != r.try_get::<String, _>("archive_sha256")
                    .map_err(storage_error)?
        {
            return Err(Error::Conflict);
        }
        let (payload_raw, payload, metadata) = exported(&archive)?;
        put(self, &mut tx, actor, keys, key, id, "payload", &payload_raw).await?;
        let mut ordinal = 0i64;
        let roots = std::iter::once((&payload, "".to_owned())).chain(
            payload
                .get("legacy")
                .filter(|v| v.is_object())
                .map(|v| (v, "legacy:".to_owned())),
        );
        for (state, prefix) in roots {
            for (family, field) in [
                ("run", "runs"),
                ("outbox", "outbox"),
                ("event", "events"),
                ("message", "messages"),
            ] {
                let mut groups: Vec<(String, &[Value])> = Vec::new();
                if matches!(family, "run" | "outbox") {
                    if let Some(a) = state[field].as_array() {
                        groups.push((prefix.clone(), a.as_slice()));
                    }
                } else if let Some(map) = state[field].as_object() {
                    for (k, v) in map {
                        if k.len() > 512 {
                            return Err(device_invalid());
                        }
                        let a = v.as_array().ok_or_else(device_invalid)?;
                        groups.push((format!("{prefix}{k}"), a.as_slice()));
                    }
                }
                for (group, values) in groups {
                    for value in values {
                        ordinal = ordinal.checked_add(1).ok_or(Error::BudgetExceeded)?;
                        let raw = Zeroizing::new(serde_json::to_vec(value).map_err(storage_error)?);
                        let stream = format!("entry:{ordinal}");
                        put(self, &mut tx, actor, keys, key, id, &stream, &raw).await?;
                        crate::android_administration_control::charge(
                            &mut tx,
                            actor.identity_id(),
                            1024,
                            0,
                        )
                        .await?;
                        let source_id = value
                            .get("id")
                            .or_else(|| value.pointer("/spec/id"))
                            .and_then(Value::as_str)
                            .filter(|s| s.len() <= 200)
                            .map(str::to_owned);
                        sqlx::query("INSERT INTO android_mobile_cache_entries(import_id,ordinal,family,source_group,source_id,size,digest) VALUES($1,$2,$3,$4,$5,$6,$7)").bind(id.to_string()).bind(ordinal).bind(family).bind(&group).bind(source_id).bind(raw.len() as i64).bind(hash(&raw)).execute(&mut *tx).await.map_err(storage_error)?;
                    }
                }
            }
        }
        authorize(self, &mut tx, actor, ContentAction::Write).await?;
        crate::android_administration_control::charge(
            &mut tx,
            actor.identity_id(),
            0,
            -r.try_get::<i64, _>("reserved_bytes")
                .map_err(storage_error)?,
        )
        .await?;
        if sqlx::query("UPDATE android_mobile_cache_imports SET revision=revision+1,state='indexed',entry_count=$1,payload_sha256=$2,source=$3,reserved_bytes=0 WHERE id=$4 AND owner_id=$5 AND revision=$6 AND state='receiving'").bind(ordinal).bind(hash(&payload_raw)).bind(json_string(&metadata)?).bind(id.to_string()).bind(actor.identity_id().to_string()).bind(expected).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict)}
        let mut v = view(&row(&mut tx, actor.identity_id(), id).await?)?;
        v["request_sha256"] = json!(sha);
        v["operation_key"] = json!(op);
        let current = device_writer(&mut tx).await?;
        crate::android_administration_control::remember(
            &mut tx,
            actor.identity_id(),
            &op,
            "cache_index",
            &sha,
            &v,
            current,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn android_cache_entries(
        &self,
        actor: &Principal,
        id: Uuid,
        after: i64,
        limit: u32,
    ) -> Result<Value> {
        if after < 0 || !(1..=100).contains(&limit) {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        authorize(self, &mut tx, actor, ContentAction::Read).await?;
        let _ = row(&mut tx, actor.identity_id(), id).await?;
        let rows=sqlx::query("SELECT * FROM android_mobile_cache_entries WHERE import_id=$1 AND ordinal>$2 ORDER BY ordinal LIMIT $3").bind(id.to_string()).bind(after).bind(i64::from(limit)+1).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let more = rows.len() > limit as usize;
        let mut items = Vec::new();
        let mut next = after;
        for r in rows.iter().take(limit as usize) {
            next = r.try_get("ordinal").map_err(storage_error)?;
            items.push(json!({"ordinal":next,"family":r.try_get::<String,_>("family").map_err(storage_error)?,"source_group":r.try_get::<String,_>("source_group").map_err(storage_error)?,"source_id":r.try_get::<Option<String>,_>("source_id").map_err(storage_error)?,"size":r.try_get::<i64,_>("size").map_err(storage_error)?,"sha256":r.try_get::<String,_>("digest").map_err(storage_error)?,"activation_receipt_sha256":r.try_get::<Option<String>,_>("activation_receipt_sha256").map_err(storage_error)?,"target_message_id":r.try_get::<Option<String>,_>("target_message_id").map_err(storage_error)?,"activatable_as_inert_message":r.try_get::<String,_>("family").map_err(storage_error)?=="message"}));
        }
        authorize(self, &mut tx, actor, ContentAction::Read).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(json!({"items":items,"next_cursor":if more{Some(next)}else{None}}))
    }
    pub async fn android_cache_bytes(
        &self,
        actor: &Principal,
        id: Uuid,
        stream: &str,
        ordinal: i64,
        keys: &dyn KeyStore,
    ) -> Result<(Value, Vec<u8>)> {
        if ordinal < 0
            || !(stream == "archive"
                || stream == "payload"
                || stream
                    .strip_prefix("entry:")
                    .is_some_and(|s| s.parse::<u64>().is_ok_and(|v| v > 0)))
        {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        authorize(self, &mut tx, actor, ContentAction::Read).await?;
        let source = row(&mut tx, actor.identity_id(), id).await?;
        let r=sqlx::query("SELECT * FROM android_mobile_cache_chunks WHERE import_id=$1 AND stream=$2 AND ordinal=$3").bind(id.to_string()).bind(stream).bind(ordinal).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        let data = cipher::open(keys, actor.identity_id(), &r)?;
        let v = json!({"import_id":id,"archive_sha256":source.try_get::<String,_>("archive_sha256").map_err(storage_error)?,"stream":stream,"ordinal":ordinal,"size":data.len(),"sha256":hash(&data)});
        authorize(self, &mut tx, actor, ContentAction::Read).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok((v, data.to_vec()))
    }
    pub async fn android_cache_activate(
        &self,
        actor: &Principal,
        id: Uuid,
        request: Uuid,
        expected: i64,
        thread: Uuid,
        ordinals: &[i64],
        reason: &str,
        keys: &dyn KeyStore,
    ) -> Result<Value> {
        if request.is_nil()
            || ordinals.is_empty()
            || ordinals.len() > 100
            || ordinals.iter().any(|v| *v <= 0)
            || ordinals
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != ordinals.len()
            || reason.trim().is_empty()
            || reason.len() > 2048
        {
            return Err(device_invalid());
        }
        let body = json!({"import_id":id,"request_id":request,"expected_revision":expected,"thread_id":thread,"entries":ordinals,"history_only_reason":reason});
        let sha = canonical_hash(&body);
        let op = format!("cache-activate:{request}");
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        authorize(self, &mut tx, actor, ContentAction::Read).await?;
        let r = row(&mut tx, actor.identity_id(), id).await?;
        if let Some(v) = crate::android_administration_control::original(
            &mut tx,
            actor.identity_id(),
            &op,
            "cache_activate",
            &sha,
        )
        .await?
        {
            tx.commit().await.map_err(storage_error)?;
            return Ok(v);
        }
        authorize(self, &mut tx, actor, ContentAction::Write).await?;
        if r.try_get::<String, _>("state").map_err(storage_error)? != "indexed"
            || r.try_get::<i64, _>("revision").map_err(storage_error)? != expected
        {
            return Err(Error::Conflict);
        }
        let project: String = sqlx::query_scalar(
            "SELECT project_id FROM android_personal_threads WHERE id=$1 AND deleted=0",
        )
        .bind(thread.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
        let project = Uuid::parse_str(&project).map_err(storage_error)?;
        self.android_personal_permit(&mut tx, actor, project, ContentAction::Write)
            .await?;
        let owner = crate::android_private_sharing::source_owner(&mut tx, project).await?;
        let mut messages = Vec::new();
        for ordinal in ordinals {
            let e = sqlx::query(
                "SELECT * FROM android_mobile_cache_entries WHERE import_id=$1 AND ordinal=$2",
            )
            .bind(id.to_string())
            .bind(ordinal)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?
            .ok_or(Error::NotFound)?;
            if e.try_get::<String, _>("family").map_err(storage_error)? != "message"
                || e.try_get::<Option<String>, _>("activation_receipt_sha256")
                    .map_err(storage_error)?
                    .is_some()
            {
                return Err(Error::Conflict);
            }
            let raw = bytes(
                &mut tx,
                keys,
                actor.identity_id(),
                id,
                &format!("entry:{ordinal}"),
                1024 * 1024,
            )
            .await?;
            if hash(&raw) != e.try_get::<String, _>("digest").map_err(storage_error)?
                || raw.len() as i64 != e.try_get::<i64, _>("size").map_err(storage_error)?
            {
                return Err(Error::Forbidden);
            }
            let original: Value = serde_json::from_slice(&raw).map_err(storage_error)?;
            let content = original
                .get("content")
                .cloned()
                .ok_or_else(device_invalid)?;
            if json_string(&content)?.len() > 512 * 1024 {
                return Err(Error::BudgetExceeded);
            }
            let message = Uuid::new_v4();
            let role = match original["role"].as_str() {
                Some("user") => "user",
                _ => "assistant",
            };
            let value = json!({"schema_version":2,"id":message,"revision":1,"thread_id":thread,"project_id":project,"role":role,"content":content,"run_id":null,"created_at":device_iso(current)?,"deleted_at":null,"metadata":{"source":"legacy_mobile_cache_inert","import_id":id,"source_ordinal":ordinal,"source_sha256":e.try_get::<String,_>("digest").map_err(storage_error)?,"original_role":original["role"],"current_actor":actor.identity_id()}});
            let encoded = json_string(&value)?;
            crate::android_administration_control::charge(
                &mut tx,
                actor.identity_id(),
                encoded.len() as i64 + 512,
                0,
            )
            .await?;
            self.android_personal_permit(&mut tx, actor, project, ContentAction::Write)
                .await?;
            sqlx::query("INSERT INTO android_personal_messages(id,owner_id,project_id,thread_id,revision,payload,deleted) VALUES($1,$2,$3,$4,1,$5,0)").bind(message.to_string()).bind(owner.to_string()).bind(project.to_string()).bind(thread.to_string()).bind(encoded).execute(&mut *tx).await.map_err(storage_error)?;
            crate::android_wire_sync::record(
                &mut tx,
                owner,
                "message",
                message,
                None,
                Some(project),
                1,
                &value,
                false,
                current,
            )
            .await?;
            messages.push(
                json!({"ordinal":ordinal,"message_id":message,"sha256":canonical_hash(&value)}),
            );
        }
        let receipt = json!({"schema_version":2,"import_id":id,"request_id":request,"request_sha256":sha,"revision":expected+1,"current_actor":actor.identity_id(),"project_id":project,"thread_id":thread,"messages":messages,"remainder_history_only_reason":reason,"outbox_dispatched":false,"runs_created":false});
        let receipt_sha = canonical_hash(&receipt);
        for m in &messages {
            if sqlx::query("UPDATE android_mobile_cache_entries SET activation_receipt_sha256=$1,target_message_id=$2 WHERE import_id=$3 AND ordinal=$4 AND activation_receipt_sha256 IS NULL").bind(&receipt_sha).bind(m["message_id"].as_str()).bind(id.to_string()).bind(m["ordinal"].as_i64()).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict)}
        }
        if sqlx::query("UPDATE android_mobile_cache_imports SET revision=revision+1 WHERE id=$1 AND owner_id=$2 AND revision=$3 AND state='indexed'").bind(id.to_string()).bind(actor.identity_id().to_string()).bind(expected).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict)}
        self.android_personal_permit(&mut tx, actor, project, ContentAction::Write)
            .await?;
        crate::android_administration_control::remember(
            &mut tx,
            actor.identity_id(),
            &op,
            "cache_activate",
            &sha,
            &receipt,
            current,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
    pub async fn android_cache_retained(
        &self,
        actor: &Principal,
        id: Uuid,
        expected: i64,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        authorize(self, &mut tx, actor, ContentAction::Write).await?;
        let r = row(&mut tx, actor.identity_id(), id).await?;
        let op = format!("cache-retain:{id}:{expected}");
        let sha = canonical_hash(
            &json!({"import_id":id,"expected_revision":expected,"archive_sha256":r.try_get::<String,_>("archive_sha256").map_err(storage_error)?}),
        );
        if let Some(v) = crate::android_administration_control::original(
            &mut tx,
            actor.identity_id(),
            &op,
            "cache_retain",
            &sha,
        )
        .await?
        {
            authorize(self, &mut tx, actor, ContentAction::Read).await?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(v);
        }
        if r.try_get::<String, _>("state").map_err(storage_error)? != "receiving" {
            return Err(Error::Conflict);
        }
        if r.try_get::<String, _>("state").map_err(storage_error)? == "receiving" {
            if r.try_get::<i64, _>("revision").map_err(storage_error)? != expected {
                return Err(Error::Conflict);
            }
            crate::android_administration_control::charge(
                &mut tx,
                actor.identity_id(),
                0,
                -r.try_get::<i64, _>("reserved_bytes")
                    .map_err(storage_error)?,
            )
            .await?;
            sqlx::query("UPDATE android_mobile_cache_imports SET state='retained',revision=revision+1,reserved_bytes=0 WHERE id=$1 AND revision=$2 AND state='receiving'").bind(id.to_string()).bind(expected).execute(&mut *tx).await.map_err(storage_error)?;
        }
        let mut v = view(&row(&mut tx, actor.identity_id(), id).await?)?;
        v["operation_key"] = json!(op);
        v["request_sha256"] = json!(sha);
        crate::android_administration_control::remember(
            &mut tx,
            actor.identity_id(),
            &op,
            "cache_retain",
            &sha,
            &v,
            current,
        )
        .await?;
        authorize(self, &mut tx, actor, ContentAction::Write).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
}
