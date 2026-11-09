//! Identity-private initial snapshots and file bytes, with scoped authenticated encryption.
use super::*;
use crate::android_devices::{device_invalid, device_iso, device_writer};
use base64::{engine::general_purpose::STANDARD, Engine};
use cowork_identity::{ContentAction, Principal};
use cowork_secrets::{KeyId, KeyStore};
use ring::{
    aead,
    rand::{SecureRandom, SystemRandom},
};
use sqlx::{Any, Transaction};
use std::collections::{BTreeMap, BTreeSet};
use zeroize::Zeroizing;
const CHUNK: usize = 4 * 1024 * 1024;
pub fn relative(path: &str) -> Result<()> {
    if path.is_empty()
        || path.len() > 2000
        || path.contains('\\')
        || path.contains(':')
        || path.chars().any(char::is_control)
        || path
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == "..")
    {
        return Err(device_invalid());
    }
    Ok(())
}
fn digest(v: &str) -> Result<()> {
    if v.len() != 64
        || !v
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(device_invalid());
    }
    Ok(())
}
fn chunks(manifest: &Value) -> Result<BTreeMap<String, usize>> {
    let files = manifest["files"]
        .as_array()
        .filter(|v| v.len() <= 2000)
        .ok_or_else(device_invalid)?;
    let mut names = BTreeSet::new();
    let mut all = BTreeMap::new();
    let mut total = 0u64;
    for file in files {
        let path = file["path"].as_str().ok_or_else(device_invalid)?;
        relative(path)?;
        let mode = file["mode"].as_u64().unwrap_or(0o644);
        if mode > u32::MAX as u64 || !matches!(mode & 0o170000, 0 | 0o100000) {
            return Err(device_invalid());
        }
        if file["modified_at"]
            .as_str()
            .is_some_and(|s| chrono::DateTime::parse_from_rfc3339(s).is_err())
        {
            return Err(device_invalid());
        }
        if !names.insert(path) {
            return Err(device_invalid());
        }
        let size = file["size"].as_u64().ok_or_else(device_invalid)?;
        let mut sum = 0u64;
        let entries = file["chunks"]
            .as_array()
            .filter(|v| v.len() <= 4096)
            .ok_or_else(device_invalid)?;
        for c in entries {
            let d = c["digest"].as_str().ok_or_else(device_invalid)?;
            digest(d)?;
            let n = c["plaintext_size"]
                .as_u64()
                .filter(|n| *n > 0 && *n <= CHUNK as u64)
                .ok_or_else(device_invalid)?;
            if all
                .insert(d.to_owned(), n as usize)
                .is_some_and(|old| old != n as usize)
            {
                return Err(device_invalid());
            }
            sum = sum.checked_add(n).ok_or(Error::BudgetExceeded)?;
        }
        if sum != size {
            return Err(device_invalid());
        }
        total = total.checked_add(size).ok_or(Error::BudgetExceeded)?;
        if total > 256 * 1024 * 1024 {
            return Err(Error::BudgetExceeded);
        }
    }
    if manifest["total_bytes"].as_u64() != Some(total) {
        return Err(device_invalid());
    }
    Ok(all)
}
fn aad(
    owner: Uuid,
    project: Uuid,
    snapshot: Uuid,
    digest: &str,
    size: usize,
    key: KeyId,
) -> Result<Vec<u8>> {
    serde_json::to_vec(&(
        "android-private-snapshot-chunk/v1",
        owner,
        project,
        snapshot,
        digest,
        size,
        key,
    ))
    .map_err(storage_error)
}
pub(crate) fn seal(
    keys: &dyn KeyStore,
    key_id: KeyId,
    owner: Uuid,
    project: Uuid,
    snapshot: Uuid,
    digest: &str,
    bytes: &[u8],
) -> Result<Value> {
    let master = keys
        .load(&key_id)
        .map_err(|_| Error::NativeKeyUnavailable)?;
    let key = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::CHACHA20_POLY1305, master.as_ref())
            .map_err(|_| Error::NativeKeyUnavailable)?,
    );
    let mut nonce = [0; 12];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| Error::NativeKeyUnavailable)?;
    let mut encrypted = Zeroizing::new(bytes.to_vec());
    key.seal_in_place_append_tag(
        aead::Nonce::assume_unique_for_key(nonce),
        aead::Aad::from(aad(owner, project, snapshot, digest, bytes.len(), key_id)?),
        &mut *encrypted,
    )
    .map_err(|_| Error::NativeKeyUnavailable)?;
    Ok(
        json!({"format":1,"key_id":key_id,"nonce":STANDARD.encode(nonce),"ciphertext":STANDARD.encode(encrypted.as_slice())}),
    )
}
pub(crate) fn open(
    keys: &dyn KeyStore,
    owner: Uuid,
    project: Uuid,
    snapshot: Uuid,
    digest: &str,
    size: usize,
    sealed: &Value,
) -> Result<Zeroizing<Vec<u8>>> {
    if sealed["format"] != 1 {
        return Err(Error::Forbidden);
    }
    let key_id: KeyId =
        serde_json::from_value(sealed["key_id"].clone()).map_err(|_| device_invalid())?;
    let master = keys
        .load(&key_id)
        .map_err(|_| Error::NativeKeyUnavailable)?;
    let nonce: [u8; 12] = STANDARD
        .decode(sealed["nonce"].as_str().ok_or_else(device_invalid)?)
        .map_err(|_| device_invalid())?
        .try_into()
        .map_err(|_| device_invalid())?;
    let mut bytes = Zeroizing::new(
        STANDARD
            .decode(sealed["ciphertext"].as_str().ok_or_else(device_invalid)?)
            .map_err(|_| device_invalid())?,
    );
    if bytes.len() != size + 16 {
        return Err(Error::Forbidden);
    }
    let key = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::CHACHA20_POLY1305, master.as_ref())
            .map_err(|_| Error::NativeKeyUnavailable)?,
    );
    let plain = key
        .open_in_place(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(aad(owner, project, snapshot, digest, size, key_id)?),
            &mut bytes,
        )
        .map_err(|_| Error::Forbidden)?
        .to_vec();
    if hash(&plain) != digest {
        return Err(Error::Forbidden);
    }
    Ok(Zeroizing::new(plain))
}
pub(crate) async fn manifest(
    tx: &mut Transaction<'_, Any>,
    owner: Uuid,
    project: Uuid,
    id: Uuid,
    ready: bool,
    current: i64,
) -> Result<Value> {
    let row=sqlx::query("SELECT payload,committed FROM android_private_snapshots WHERE id=$1 AND owner_id=$2 AND project_id=$3 AND deleted=0").bind(id.to_string()).bind(owner.to_string()).bind(project.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
    if ready && row.try_get::<i64, _>("committed").map_err(storage_error)? != 1 {
        return Err(Error::Conflict);
    }
    let v: Value = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    if v["expires_at"]
        .as_str()
        .map(|s| {
            chrono::DateTime::parse_from_rfc3339(s)
                .map(|d| d.timestamp_millis())
                .map_err(|_| device_invalid())
        })
        .transpose()?
        .is_some_and(|expiry| expiry <= current)
    {
        return Err(Error::Forbidden);
    }
    Ok(v)
}
pub(crate) async fn file_bytes(
    tx: &mut Transaction<'_, Any>,
    owner: Uuid,
    project: Uuid,
    snapshot: Uuid,
    path: &str,
    keys: &dyn KeyStore,
    current: i64,
) -> Result<Zeroizing<Vec<u8>>> {
    let m = manifest(tx, owner, project, snapshot, true, current).await?;
    let file = m["files"]
        .as_array()
        .ok_or_else(device_invalid)?
        .iter()
        .find(|f| f["path"] == path)
        .ok_or(Error::NotFound)?;
    let mut bytes = Zeroizing::new(Vec::new());
    for c in file["chunks"].as_array().ok_or_else(device_invalid)? {
        let digest = c["digest"].as_str().ok_or_else(device_invalid)?;
        let stored: String = sqlx::query_scalar(
            "SELECT sealed FROM android_private_snapshot_chunks WHERE snapshot_id=$1 AND digest=$2",
        )
        .bind(snapshot.to_string())
        .bind(digest)
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)?;
        let plain = open(
            keys,
            owner,
            project,
            snapshot,
            digest,
            c["plaintext_size"].as_u64().ok_or_else(device_invalid)? as usize,
            &parse(&stored)?,
        )?;
        bytes.extend_from_slice(&plain);
    }
    if bytes.len() as u64 != file["size"].as_u64().ok_or_else(device_invalid)? {
        return Err(Error::Forbidden);
    }
    Ok(bytes)
}
impl Store {
    pub async fn android_agent_private_snapshot(
        &self,
        a: &crate::AndroidAgentPrincipal,
        run: Uuid,
        lease: Uuid,
        digest: Option<&str>,
        keys: &dyn KeyStore,
    ) -> Result<Option<(Value, Vec<u8>)>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let r = crate::android_device_runs::device_lease_current(&mut tx, a, run, lease, current)
            .await?;
        if r.try_get::<Option<String>, _>("team_id")
            .map_err(storage_error)?
            .is_some()
        {
            return Ok(None);
        }
        let v: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
        let Some(snapshot) = v["spec"]["snapshot_id"]
            .as_str()
            .and_then(|s| Uuid::parse_str(s).ok())
        else {
            return Ok(None);
        };
        let owner = Uuid::parse_str(
            &r.try_get::<String, _>("creator_id")
                .map_err(storage_error)?,
        )
        .map_err(storage_error)?;
        let project = Uuid::parse_str(
            &r.try_get::<String, _>("project_id")
                .map_err(storage_error)?,
        )
        .map_err(storage_error)?;
        let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_snapshots WHERE id=$1 AND owner_id=$2 AND project_id=$3").bind(snapshot.to_string()).bind(owner.to_string()).bind(project.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if count == 0 {
            return Ok(None);
        }
        let m = manifest(&mut tx, owner, project, snapshot, true, current).await?;
        let bytes = if let Some(d) = digest {
            let expected = *chunks(&m)?.get(d).ok_or(Error::NotFound)?;
            let stored:String=sqlx::query_scalar("SELECT sealed FROM android_private_snapshot_chunks WHERE snapshot_id=$1 AND digest=$2").bind(snapshot.to_string()).bind(d).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
            open(
                keys,
                owner,
                project,
                snapshot,
                d,
                expected,
                &parse(&stored)?,
            )?
            .to_vec()
        } else {
            Vec::new()
        };
        crate::android_device_runs::device_lease_current(&mut tx, a, run, lease, current).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(Some((m, bytes)))
    }
    pub async fn android_private_snapshot_begin(
        &self,
        actor: &Principal,
        mut body: Value,
        operation: Option<&str>,
        key_id: KeyId,
    ) -> Result<Value> {
        if body.as_object().is_none_or(|o| {
            o.keys().any(|k| {
                !["project_id", "total_bytes", "files", "expires_at"].contains(&k.as_str())
            })
        }) {
            return Err(device_invalid());
        }
        let project = crate::android_personal::wire_uuid(&body, "project_id")?;
        chunks(&body)?;
        let digest = hash(json_string(&body)?.as_bytes());
        let operation = operation
            .map(str::to_owned)
            .unwrap_or_else(|| format!("legacy:{digest}"));
        valid_text(&operation, 200)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        self.android_personal_permit(&mut tx, actor, project, ContentAction::Execute)
            .await?;
        if let Some(row)=sqlx::query("SELECT id,request_sha FROM android_private_snapshots WHERE owner_id=$1 AND creation_key=$2 AND deleted=0").bind(actor.identity_id().to_string()).bind(&operation).fetch_optional(&mut *tx).await.map_err(storage_error)?{if row.try_get::<String,_>("request_sha").map_err(storage_error)?!=digest{return Err(Error::Conflict)}let id=Uuid::parse_str(&row.try_get::<String,_>("id").map_err(storage_error)?).map_err(storage_error)?;let result=status(&mut tx,actor.identity_id(),project,id,current).await?;tx.commit().await.map_err(storage_error)?;return Ok(result)}
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM android_private_snapshots WHERE owner_id=$1 AND deleted=0",
        )
        .bind(actor.identity_id().to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        if count >= 1000 {
            return Err(Error::BudgetExceeded);
        }
        let id = Uuid::new_v4();
        body["schema_version"] = json!(2);
        body["id"] = json!(id);
        body["encryption_key_id"] = json!(key_id
            .cipher_id()
            .map_err(|_| Error::NativeKeyUnavailable)?);
        body["created_at"] = json!(device_iso(current)?);
        if body["expires_at"].is_null() {
            body["expires_at"] = json!(device_iso(current + 7 * 24 * 60 * 60 * 1000)?)
        } else {
            let expiry = body["expires_at"]
                .as_str()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|v| v.timestamp_millis())
                .filter(|v| *v > current && *v <= current + 30 * 24 * 60 * 60 * 1000)
                .ok_or_else(device_invalid)?;
            body["expires_at"] = json!(device_iso(expiry)?)
        }
        sqlx::query("INSERT INTO android_private_snapshots(id,owner_id,project_id,creation_key,request_sha,payload) VALUES($1,$2,$3,$4,$5,$6)").bind(id.to_string()).bind(actor.identity_id().to_string()).bind(project.to_string()).bind(operation).bind(digest).bind(json_string(&body)?).execute(&mut *tx).await.map_err(storage_error)?;
        let result = status(&mut tx, actor.identity_id(), project, id, current).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn android_private_snapshot(
        &self,
        actor: &Principal,
        id: Uuid,
        upload: bool,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let project: String=sqlx::query_scalar("SELECT project_id FROM android_private_snapshots WHERE id=$1 AND owner_id=$2 AND deleted=0").bind(id.to_string()).bind(actor.identity_id().to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        let project = Uuid::parse_str(&project).map_err(storage_error)?;
        self.android_personal_permit(&mut tx, actor, project, ContentAction::Read)
            .await?;
        let result = if upload {
            status(&mut tx, actor.identity_id(), project, id, current).await?
        } else {
            manifest(&mut tx, actor.identity_id(), project, id, false, current).await?
        };
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn android_private_snapshot_chunk(
        &self,
        actor: &Principal,
        id: Uuid,
        d: &str,
        bytes: Option<&[u8]>,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<(Value, Vec<u8>)> {
        let key_id = crate::android_private_workspace::writer_cipher(keys, key_id)?;
        digest(d)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let project:String=sqlx::query_scalar("SELECT project_id FROM android_private_snapshots WHERE id=$1 AND owner_id=$2 AND deleted=0").bind(id.to_string()).bind(actor.identity_id().to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        let project = Uuid::parse_str(&project).map_err(storage_error)?;
        self.android_personal_permit(
            &mut tx,
            actor,
            project,
            if bytes.is_some() {
                ContentAction::Execute
            } else {
                ContentAction::Read
            },
        )
        .await?;
        let m = manifest(&mut tx, actor.identity_id(), project, id, false, current).await?;
        let expected = *chunks(&m)?.get(d).ok_or(Error::NotFound)?;
        let old: Option<String> = sqlx::query_scalar(
            "SELECT sealed FROM android_private_snapshot_chunks WHERE snapshot_id=$1 AND digest=$2",
        )
        .bind(id.to_string())
        .bind(d)
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?;
        let result = if let Some(bytes) = bytes {
            if bytes.len() != expected || hash(bytes) != d {
                return Err(device_invalid());
            }
            if old.is_none() {
                let sealed = seal(keys, key_id, actor.identity_id(), project, id, d, bytes)?;
                crate::server_keys::cipher_writer(&mut tx, key_id).await?;
                sqlx::query("INSERT INTO android_private_snapshot_chunks(snapshot_id,digest,size,sealed) VALUES($1,$2,$3,$4)").bind(id.to_string()).bind(d).bind(expected as i64).bind(json_string(&sealed)?).execute(&mut *tx).await.map_err(storage_error)?;
            }
            (
                json!({"schema_version":2,"manifest_id":id,"digest":d,"plaintext_size":expected,"ciphertext_size":expected+16,"deduplicated":old.is_some(),"warnings":[]}),
                Vec::new(),
            )
        } else {
            (
                Value::Null,
                open(
                    keys,
                    actor.identity_id(),
                    project,
                    id,
                    d,
                    expected,
                    &parse::<Value>(&old.ok_or(Error::NotFound)?)?,
                )?
                .to_vec(),
            )
        };
        self.android_personal_permit(
            &mut tx,
            actor,
            project,
            if bytes.is_some() {
                ContentAction::Execute
            } else {
                ContentAction::Read
            },
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn android_private_snapshot_commit(
        &self,
        actor: &Principal,
        id: Uuid,
        keys: &dyn KeyStore,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let project:String=sqlx::query_scalar("SELECT project_id FROM android_private_snapshots WHERE id=$1 AND owner_id=$2 AND deleted=0").bind(id.to_string()).bind(actor.identity_id().to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        let project = Uuid::parse_str(&project).map_err(storage_error)?;
        self.android_personal_permit(&mut tx, actor, project, ContentAction::Execute)
            .await?;
        let mut m = manifest(&mut tx, actor.identity_id(), project, id, false, current).await?;
        for (d, size) in chunks(&m)? {
            let sealed:String=sqlx::query_scalar("SELECT sealed FROM android_private_snapshot_chunks WHERE snapshot_id=$1 AND digest=$2").bind(id.to_string()).bind(&d).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Conflict)?;
            let _ = open(
                keys,
                actor.identity_id(),
                project,
                id,
                &d,
                size,
                &parse(&sealed)?,
            )?;
        }
        for file in m["files"].as_array_mut().ok_or_else(device_invalid)? {
            for chunk in file["chunks"].as_array_mut().ok_or_else(device_invalid)? {
                chunk["ciphertext_size"] = json!(
                    chunk["plaintext_size"]
                        .as_u64()
                        .ok_or_else(device_invalid)?
                        + 16
                )
            }
            let path = file["path"].as_str().ok_or_else(device_invalid)?;
            let id_string = hash(json_string(&(id, path))?.as_bytes());
            let file_id = Uuid::parse_str(&id_string[..32]).map_err(storage_error)?;
            let payload = json!({"schema_version":2,"id":file_id,"project_id":project,"snapshot_id":id,"name":path,"path":path,"size":file["size"],"size_bytes":file["size"],"revision":1,"deleted_at":null});
            sqlx::query("INSERT INTO android_private_files(id,owner_id,project_id,snapshot_id,path,payload) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(snapshot_id,path) DO NOTHING").bind(file_id.to_string()).bind(actor.identity_id().to_string()).bind(project.to_string()).bind(id.to_string()).bind(path).bind(json_string(&payload)?).execute(&mut *tx).await.map_err(storage_error)?;
        }
        sqlx::query("UPDATE android_private_snapshots SET committed=1,payload=$1 WHERE id=$2")
            .bind(json_string(&m)?)
            .bind(id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        crate::android_private_server::snapshot_ready(
            &mut tx,
            actor.identity_id(),
            project,
            id,
            current,
        )
        .await?;
        self.android_personal_permit(&mut tx, actor, project, ContentAction::Execute)
            .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(m)
    }
    pub async fn android_private_files_snapshot(
        &self,
        actor: &Principal,
        project: Uuid,
        ids: &[Uuid],
        run_key: &str,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<(Uuid, Vec<Value>)> {
        self.android_private_files_snapshot_selected(
            actor, project, ids, run_key, keys, key_id, None,
        )
        .await
    }
    pub async fn android_private_files_snapshot_selected(
        &self,
        actor: &Principal,
        project: Uuid,
        ids: &[Uuid],
        run_key: &str,
        keys: &dyn KeyStore,
        key_id: KeyId,
        expected_versions: Option<&Value>,
    ) -> Result<(Uuid, Vec<Value>)> {
        let key_id = crate::android_private_workspace::writer_cipher(keys, key_id)?;
        if ids.is_empty()
            || ids.len() > 2000
            || ids.iter().collect::<BTreeSet<_>>().len() != ids.len()
        {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        crate::server_keys::cipher_writer(&mut tx, key_id).await?;
        let current = device_writer(&mut tx).await?;
        self.android_personal_permit(&mut tx, actor, project, ContentAction::Execute)
            .await?;
        let selected = expected_versions
            .map(|v| v.as_object().ok_or_else(device_invalid))
            .transpose()?;
        if selected.is_some_and(|v| {
            v.len() != ids.len()
                || v.keys()
                    .any(|key| !ids.iter().any(|id| id.to_string() == *key))
        }) {
            return Err(Error::Conflict);
        }
        let mut files = Vec::new();
        let mut sources = Vec::new();
        let mut frozen = Vec::new();
        let mut names = BTreeSet::new();
        let mut total = 0u64;
        for id in ids {
            let r=sqlx::query("SELECT snapshot_id,path,payload FROM android_private_files WHERE id=$1 AND owner_id=$2 AND project_id=$3 AND deleted=0").bind(id.to_string()).bind(actor.identity_id().to_string()).bind(project.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
            let original_payload: Value =
                parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
            let revision = original_payload["revision"]
                .as_i64()
                .filter(|r| *r > 0)
                .ok_or(Error::Conflict)?;
            if original_payload["id"] != json!(id)
                || original_payload["project_id"] != json!(project)
                || selected.is_some_and(|versions| {
                    versions.get(&id.to_string()).and_then(Value::as_i64) != Some(revision)
                })
            {
                return Err(Error::Conflict);
            }
            let source = Uuid::parse_str(
                &r.try_get::<String, _>("snapshot_id")
                    .map_err(storage_error)?,
            )
            .map_err(storage_error)?;
            let path = r.try_get::<String, _>("path").map_err(storage_error)?;
            if !names.insert(path.to_lowercase()) {
                return Err(Error::Conflict);
            }
            if original_payload["snapshot_id"] != json!(source) || original_payload["path"] != path
            {
                return Err(Error::Conflict);
            }
            let m = manifest(&mut tx, actor.identity_id(), project, source, true, current).await?;
            let file = m["files"]
                .as_array()
                .ok_or_else(device_invalid)?
                .iter()
                .find(|f| f["path"] == path)
                .ok_or(Error::Forbidden)?
                .clone();
            let size = file["size"]
                .as_u64()
                .filter(|v| *v <= 32 * 1024 * 1024)
                .ok_or(Error::BudgetExceeded)?;
            total = total.checked_add(size).ok_or(Error::BudgetExceeded)?;
            if total > 256 * 1024 * 1024 {
                return Err(Error::BudgetExceeded);
            }
            let p: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
            frozen.push(json!({"id":id,"sha256":cowork_domain::canonical_hash(&p)}));
            sources.push(source);
            files.push(file);
        }
        let request_sha = hash(json_string(&(project, &frozen, &files))?.as_bytes());
        let operation = format!("run-files:{}", hash(run_key.as_bytes()));
        if let Some(r)=sqlx::query("SELECT id,request_sha FROM android_private_snapshots WHERE owner_id=$1 AND creation_key=$2 AND deleted=0 AND committed=1").bind(actor.identity_id().to_string()).bind(&operation).fetch_optional(&mut *tx).await.map_err(storage_error)?{if r.try_get::<String,_>("request_sha").map_err(storage_error)?!=request_sha{return Err(Error::Conflict)}let id=Uuid::parse_str(&r.try_get::<String,_>("id").map_err(storage_error)?).map_err(storage_error)?;self.android_personal_permit(&mut tx,actor,project,ContentAction::Execute).await?;tx.commit().await.map_err(storage_error)?;return Ok((id,frozen))}
        let snapshot = Uuid::new_v4();
        let payload = json!({"schema_version":2,"id":snapshot,"project_id":project,"total_bytes":total,"files":files,"encryption_key_id":key_id.cipher_id().map_err(|_|Error::NativeKeyUnavailable)?,"created_at":device_iso(current)?,"expires_at":null});
        sqlx::query("INSERT INTO android_private_snapshots(id,owner_id,project_id,creation_key,request_sha,payload,committed) VALUES($1,$2,$3,$4,$5,$6,1)").bind(snapshot.to_string()).bind(actor.identity_id().to_string()).bind(project.to_string()).bind(operation).bind(request_sha).bind(json_string(&payload)?).execute(&mut *tx).await.map_err(storage_error)?;
        let mut copied = BTreeSet::new();
        for (file, source) in files.iter().zip(sources) {
            use sha2::Digest;
            let mut file_hash = sha2::Sha256::new();
            let mut file_size = 0u64;
            for c in file["chunks"].as_array().ok_or_else(device_invalid)? {
                let d = c["digest"].as_str().ok_or_else(device_invalid)?;
                let size = c["plaintext_size"].as_u64().ok_or_else(device_invalid)? as usize;
                let encoded:String=sqlx::query_scalar("SELECT sealed FROM android_private_snapshot_chunks WHERE snapshot_id=$1 AND digest=$2").bind(source.to_string()).bind(d).fetch_one(&mut *tx).await.map_err(storage_error)?;
                let plain = open(
                    keys,
                    actor.identity_id(),
                    project,
                    source,
                    d,
                    size,
                    &parse(&encoded)?,
                )?;
                file_hash.update(plain.as_slice());
                file_size = file_size
                    .checked_add(plain.len() as u64)
                    .ok_or(Error::BudgetExceeded)?;
                if !copied.insert(d.to_owned()) {
                    continue;
                }
                let encrypted = seal(
                    keys,
                    key_id,
                    actor.identity_id(),
                    project,
                    snapshot,
                    d,
                    &plain,
                )?;
                crate::server_keys::cipher_writer(&mut tx, key_id).await?;
                sqlx::query("INSERT INTO android_private_snapshot_chunks(snapshot_id,digest,size,sealed) VALUES($1,$2,$3,$4)").bind(snapshot.to_string()).bind(d).bind(size as i64).bind(json_string(&encrypted)?).execute(&mut *tx).await.map_err(storage_error)?;
            }
            let actual = format!("{:x}", file_hash.finalize());
            if file["digest"].as_str() != Some(actual.as_str())
                || file["size"].as_u64() != Some(file_size)
            {
                return Err(Error::Conflict);
            }
        }
        self.android_personal_permit(&mut tx, actor, project, ContentAction::Execute)
            .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok((snapshot, frozen))
    }
    pub async fn android_private_files(
        &self,
        actor: &Principal,
        project: Uuid,
    ) -> Result<Vec<Value>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.android_personal_permit(&mut tx, actor, project, ContentAction::Read)
            .await?;
        let rows:Vec<String>=sqlx::query_scalar("SELECT f.payload FROM android_private_files f JOIN android_private_snapshots s ON s.id=f.snapshot_id WHERE f.owner_id=$1 AND f.project_id=$2 AND f.deleted=0 AND s.deleted=0 AND s.committed=1 ORDER BY f.id LIMIT 2001").bind(actor.identity_id().to_string()).bind(project.to_string()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        if rows.len() > 2000 {
            return Err(Error::BudgetExceeded);
        }
        let result = rows
            .iter()
            .map(|s| parse(s))
            .collect::<Result<Vec<Value>>>()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
}
pub(crate) async fn validate_files(
    tx: &mut Transaction<'_, Any>,
    owner: Uuid,
    project: Uuid,
    input: &Value,
    current: i64,
) -> Result<()> {
    let ids = input["project_file_ids"].as_array();
    let bindings = input["frozen_project_files"].as_array();
    if ids.is_none_or(|v| v.is_empty()) {
        if bindings.is_some_and(|v| !v.is_empty()) {
            return Err(Error::Forbidden);
        }
        return Ok(());
    }
    let ids = ids.ok_or_else(device_invalid)?;
    let bindings = bindings
        .filter(|b| b.len() == ids.len() && b.len() <= 2000)
        .ok_or(Error::Forbidden)?;
    for (id, frozen) in ids.iter().zip(bindings) {
        if frozen["id"] != *id {
            return Err(Error::Forbidden);
        }
        let id = id
            .as_str()
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or_else(device_invalid)?;
        let r=sqlx::query("SELECT snapshot_id,payload FROM android_private_files WHERE id=$1 AND owner_id=$2 AND project_id=$3 AND deleted=0").bind(id.to_string()).bind(owner.to_string()).bind(project.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        let p: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
        if frozen["sha256"] != json!(cowork_domain::canonical_hash(&p)) {
            return Err(Error::Forbidden);
        }
        let snapshot = Uuid::parse_str(
            &r.try_get::<String, _>("snapshot_id")
                .map_err(storage_error)?,
        )
        .map_err(storage_error)?;
        manifest(tx, owner, project, snapshot, true, current).await?;
    }
    Ok(())
}
async fn status(
    tx: &mut Transaction<'_, Any>,
    owner: Uuid,
    project: Uuid,
    id: Uuid,
    current: i64,
) -> Result<Value> {
    let m = manifest(tx, owner, project, id, false, current).await?;
    let existing: Vec<String> = sqlx::query_scalar(
        "SELECT digest FROM android_private_snapshot_chunks WHERE snapshot_id=$1",
    )
    .bind(id.to_string())
    .fetch_all(&mut **tx)
    .await
    .map_err(storage_error)?;
    let missing: Vec<_> = chunks(&m)?
        .keys()
        .filter(|d| !existing.contains(d))
        .cloned()
        .collect();
    Ok(
        json!({"schema_version":2,"manifest_id":id,"missing_chunks":missing,"max_chunk_bytes":CHUNK,"expires_at":m["expires_at"],"warnings":[]}),
    )
}
