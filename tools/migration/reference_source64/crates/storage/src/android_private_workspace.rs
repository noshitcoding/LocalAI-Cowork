//! Database-backed private workspaces. No host paths or owner folder grants exist here.
use super::*;
use crate::android_devices::{device_invalid, device_iso};
use crate::android_private_server::{id, row};
use base64::{Engine, engine::general_purpose::STANDARD};
use cowork_secrets::{KeyId, KeyStore};
use ring::{
    aead,
    rand::{SecureRandom, SystemRandom},
};
use sqlx::{Any, Transaction, any::AnyRow};
use zeroize::Zeroizing;

const FILE_LIMIT: usize = 32 * 1024 * 1024;
const TREE_LIMIT: u64 = 256 * 1024 * 1024;
pub(crate) fn writer_cipher(keys: &dyn KeyStore, id: KeyId) -> Result<KeyId> {
    keys.key_for_purpose(&id, cowork_secrets::KeyPurpose::DataEncryption)
        .map_err(|_| Error::NativeKeyUnavailable)
}

pub(crate) fn seal_value(
    keys: &dyn KeyStore,
    key_id: KeyId,
    r: &AnyRow,
    locator: &str,
    bytes: &[u8],
) -> Result<Value> {
    key_id.validate().map_err(|_| Error::NativeKeyUnavailable)?;
    if bytes.len() > FILE_LIMIT {
        return Err(Error::BudgetExceeded);
    }
    let master = keys
        .load(&key_id)
        .map_err(|_| Error::NativeKeyUnavailable)?;
    let key = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::CHACHA20_POLY1305, master.as_ref())
            .map_err(|_| Error::NativeKeyUnavailable)?,
    );
    let digest = hash(bytes);
    let aad = json_string(&(
        "android-private-loop/v1",
        id(r, "owner_id")?,
        id(r, "project_id")?,
        id(r, "id")?,
        locator,
        &digest,
        bytes.len(),
        key_id,
    ))?;
    let mut nonce = [0; 12];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| Error::NativeKeyUnavailable)?;
    let mut raw = Zeroizing::new(bytes.to_vec());
    key.seal_in_place_append_tag(
        aead::Nonce::assume_unique_for_key(nonce),
        aead::Aad::from(aad.as_bytes()),
        &mut *raw,
    )
    .map_err(|_| Error::NativeKeyUnavailable)?;
    Ok(
        json!({"format":1,"key_id":key_id,"sha256":digest,"size":bytes.len(),"nonce":STANDARD.encode(nonce),"ciphertext":STANDARD.encode(raw.as_slice())}),
    )
}
pub(crate) fn open_value(
    keys: &dyn KeyStore,
    r: &AnyRow,
    locator: &str,
    v: &Value,
) -> Result<Zeroizing<Vec<u8>>> {
    if v["format"] != 1 {
        return Err(Error::Forbidden);
    }
    let key_id: KeyId =
        serde_json::from_value(v["key_id"].clone()).map_err(|_| device_invalid())?;
    let size = v["size"]
        .as_u64()
        .filter(|n| *n <= FILE_LIMIT as u64)
        .ok_or(Error::BudgetExceeded)? as usize;
    let digest = v["sha256"].as_str().ok_or_else(device_invalid)?;
    let master = keys
        .load(&key_id)
        .map_err(|_| Error::NativeKeyUnavailable)?;
    let key = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::CHACHA20_POLY1305, master.as_ref())
            .map_err(|_| Error::NativeKeyUnavailable)?,
    );
    let nonce: [u8; 12] = STANDARD
        .decode(v["nonce"].as_str().ok_or_else(device_invalid)?)
        .map_err(|_| device_invalid())?
        .try_into()
        .map_err(|_| device_invalid())?;
    let mut raw = Zeroizing::new(
        STANDARD
            .decode(v["ciphertext"].as_str().ok_or_else(device_invalid)?)
            .map_err(|_| device_invalid())?,
    );
    if raw.len() != size + 16 {
        return Err(Error::Forbidden);
    }
    let aad = json_string(&(
        "android-private-loop/v1",
        id(r, "owner_id")?,
        id(r, "project_id")?,
        id(r, "id")?,
        locator,
        digest,
        size,
        key_id,
    ))?;
    let plain = key
        .open_in_place(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(aad.as_bytes()),
            &mut raw,
        )
        .map_err(|_| Error::Forbidden)?;
    if hash(plain) != digest {
        return Err(Error::Forbidden);
    }
    Ok(Zeroizing::new(plain.to_vec()))
}
pub(crate) async fn entries(tx: &mut Transaction<'_, Any>, run: Uuid) -> Result<Vec<Value>> {
    let rows=sqlx::query("SELECT e.path,v.id,v.kind,v.digest,v.size FROM android_private_loop_entries e JOIN android_private_loop_versions v ON v.id=e.version_id WHERE e.run_id=$1 ORDER BY e.path LIMIT 10001").bind(run.to_string()).fetch_all(&mut **tx).await.map_err(storage_error)?;
    if rows.len() > 10000 {
        return Err(Error::BudgetExceeded);
    }
    rows.iter().map(|v|Ok(json!({"path":v.try_get::<String,_>("path").map_err(storage_error)?,"version_id":v.try_get::<String,_>("id").map_err(storage_error)?,"kind":v.try_get::<String,_>("kind").map_err(storage_error)?,"sha256":v.try_get::<String,_>("digest").map_err(storage_error)?,"size":v.try_get::<i64,_>("size").map_err(storage_error)?}))).collect()
}
pub(crate) async fn version_bytes(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    version: &str,
    keys: &dyn KeyStore,
) -> Result<Zeroizing<Vec<u8>>> {
    let v = sqlx::query("SELECT * FROM android_private_loop_versions WHERE id=$1 AND run_id=$2")
        .bind(version)
        .bind(id(r, "id")?.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
    let sealed: Value = parse(&v.try_get::<String, _>("sealed").map_err(storage_error)?)?;
    let plain = open_value(keys, r, &format!("file-version:{version}"), &sealed)?;
    if plain.len() as i64 != v.try_get::<i64, _>("size").map_err(storage_error)?
        || hash(&plain) != v.try_get::<String, _>("digest").map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    Ok(plain)
}
pub(crate) async fn add_version(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    path: &str,
    kind: &str,
    bytes: &[u8],
    keys: &dyn KeyStore,
    key_id: KeyId,
    current: i64,
) -> Result<String> {
    crate::android_private_snapshots::relative(path)?;
    let version = Uuid::new_v4().to_string();
    let sealed = seal_value(keys, key_id, r, &format!("file-version:{version}"), bytes)?;
    crate::server_keys::cipher_writer(tx, key_id).await?;
    sqlx::query("INSERT INTO android_private_loop_versions(id,run_id,path,kind,digest,size,sealed,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8)").bind(&version).bind(id(r,"id")?.to_string()).bind(path).bind(kind).bind(hash(bytes)).bind(bytes.len() as i64).bind(json_string(&sealed)?).bind(current).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(version)
}
pub(crate) async fn put_entry(
    tx: &mut Transaction<'_, Any>,
    run: Uuid,
    path: &str,
    version: &str,
) -> Result<()> {
    sqlx::query("INSERT INTO android_private_loop_entries(run_id,path,version_id) VALUES($1,$2,$3) ON CONFLICT(run_id,path) DO UPDATE SET version_id=excluded.version_id").bind(run.to_string()).bind(path).bind(version).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
fn at<'a>(all: &'a [Value], path: &str) -> Option<&'a Value> {
    all.iter().find(|v| v["path"] == path)
}
fn parent(path: &str) -> Option<&str> {
    path.rsplit_once('/').map(|(p, _)| p)
}
fn assert_parent(all: &[Value], path: &str) -> Result<()> {
    if parent(path).is_some_and(|p| at(all, p).is_none_or(|v| v["kind"] != "directory")) {
        return Err(Error::Conflict);
    }
    Ok(())
}
fn tree(all: &[Value], path: &str) -> Vec<Value> {
    all.iter()
        .filter(|v| {
            v["path"] == path
                || v["path"]
                    .as_str()
                    .is_some_and(|p| p.starts_with(&format!("{path}/")))
        })
        .cloned()
        .collect()
}
fn tree_sha(all: &[Value], path: &str) -> Result<String> {
    let item = at(all, path).ok_or(Error::NotFound)?;
    if item["kind"] == "file" {
        return item["sha256"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(device_invalid);
    }
    Ok(canonical_hash(&tree(all,path).iter().map(|v|json!({"path":v["path"],"kind":v["kind"],"sha256":v["sha256"],"size":v["size"]})).collect::<Vec<_>>()))
}
pub(crate) async fn materialize(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    keys: &dyn KeyStore,
    key_id: KeyId,
    current: i64,
) -> Result<()> {
    let Some(snapshot) = r
        .try_get::<Option<String>, _>("snapshot_id")
        .map_err(storage_error)?
    else {
        return Ok(());
    };
    let snapshot = Uuid::parse_str(&snapshot).map_err(storage_error)?;
    let manifest = crate::android_private_snapshots::manifest(
        tx,
        id(r, "owner_id")?,
        id(r, "project_id")?,
        snapshot,
        true,
        current,
    )
    .await?;
    let mut names = std::collections::BTreeSet::new();
    let mut dirs = std::collections::BTreeSet::new();
    if let Some(values) = manifest.get("directories") {
        let values = values
            .as_array()
            .filter(|v| v.len() <= 10000)
            .ok_or_else(device_invalid)?;
        for value in values {
            let path = value.as_str().ok_or_else(device_invalid)?;
            crate::android_private_snapshots::relative(path)?;
            dirs.insert(path.to_owned());
            let mut p = parent(path);
            while let Some(path) = p {
                dirs.insert(path.to_owned());
                p = parent(path);
            }
        }
    }
    for f in manifest["files"].as_array().ok_or_else(device_invalid)? {
        let path = f["path"].as_str().ok_or_else(device_invalid)?;
        if !names.insert(path.to_lowercase()) {
            return Err(Error::Conflict);
        }
        let mut p = parent(path);
        while let Some(value) = p {
            dirs.insert(value.to_owned());
            p = parent(value);
        }
    }
    if dirs.iter().any(|p| names.contains(&p.to_lowercase())) {
        return Err(Error::Conflict);
    }
    for path in dirs {
        let version = add_version(tx, r, &path, "directory", b"", keys, key_id, current).await?;
        put_entry(tx, id(r, "id")?, &path, &version).await?;
    }
    for f in manifest["files"].as_array().ok_or_else(device_invalid)? {
        let path = f["path"].as_str().ok_or_else(device_invalid)?;
        let bytes = crate::android_private_snapshots::file_bytes(
            tx,
            id(r, "owner_id")?,
            id(r, "project_id")?,
            snapshot,
            path,
            keys,
            current,
        )
        .await?;
        let version = add_version(tx, r, path, "file", &bytes, keys, key_id, current).await?;
        put_entry(tx, id(r, "id")?, path, &version).await?;
    }
    bounds(tx, id(r, "id")?).await
}
pub(crate) async fn bounds(tx: &mut Transaction<'_, Any>, run: Uuid) -> Result<()> {
    let all = entries(tx, run).await?;
    let mut paths = std::collections::BTreeSet::new();
    for entry in &all {
        if !paths.insert(field(entry, "path")?.to_lowercase()) {
            return Err(Error::Conflict);
        }
    }
    let total = all.iter().try_fold(0u64, |a, v| {
        a.checked_add(v["size"].as_u64().ok_or_else(device_invalid)?)
            .ok_or(Error::BudgetExceeded)
    })?;
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM android_private_loop_versions WHERE run_id=$1")
            .bind(run.to_string())
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
    let retained: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(size),0) FROM android_private_loop_versions WHERE run_id=$1",
    )
    .bind(run.to_string())
    .fetch_one(&mut **tx)
    .await
    .map_err(storage_error)?;
    if total > TREE_LIMIT || count > 20000 || retained > 2 * 1024 * 1024 * 1024 {
        return Err(Error::BudgetExceeded);
    }
    Ok(())
}
fn field<'a>(v: &'a Value, name: &str) -> Result<&'a str> {
    v[name]
        .as_str()
        .filter(|s| !s.contains('\0'))
        .ok_or_else(device_invalid)
}
fn shape(v: &Value, allowed: &[&str], required: &[&str]) -> Result<()> {
    let o = v.as_object().ok_or_else(device_invalid)?;
    if o.keys().any(|k| !allowed.contains(&k.as_str()))
        || required.iter().any(|k| !o.contains_key(*k))
    {
        return Err(device_invalid());
    }
    Ok(())
}
pub(crate) fn mutation(name: &str) -> bool {
    matches!(
        name,
        "write_text_file"
            | "create_directory"
            | "rename_file"
            | "remove_file"
            | "restore_file"
            | "create_artifact"
    )
}
pub(crate) fn validate_tool(tool: &ToolCall, folder: &str) -> Result<()> {
    let v = &tool.arguments;
    match tool.name.as_str() {
        "list_files" | "read_text_file" | "stat_file" | "create_directory" => {
            shape(v, &["folder_id", "path"], &["folder_id", "path"])?
        }
        "write_text_file" => shape(
            v,
            &["folder_id", "path", "content", "expected_sha256"],
            &["folder_id", "path", "content", "expected_sha256"],
        )?,
        "rename_file" => shape(
            v,
            &["folder_id", "path", "destination", "expected_sha256"],
            &["folder_id", "path", "destination", "expected_sha256"],
        )?,
        "remove_file" => shape(
            v,
            &["folder_id", "path", "expected_sha256"],
            &["folder_id", "path", "expected_sha256"],
        )?,
        "restore_file" => {
            shape(
                v,
                &["folder_id", "path", "source"],
                &["folder_id", "path", "source"],
            )?;
            shape(
                &v["source"],
                &["run_id", "tool_call_id", "version"],
                &["run_id", "tool_call_id", "version"],
            )?;
        }
        "create_artifact" => shape(v, &["name", "content"], &["name", "content"])?,
        "list_artifacts" => shape(v, &[], &[])?,
        "read_artifact" => {
            shape(v, &["id"], &["id"])?;
            Uuid::parse_str(field(v, "id")?).map_err(|_| device_invalid())?;
        }
        "request_input" => {
            shape(v, &["question"], &["question"])?;
            valid_text(field(v, "question")?, 16 * 1024)?;
        }
        _ => return Err(Error::Forbidden),
    }
    if let Some(path) = v.get("path") {
        if v["folder_id"] != folder {
            return Err(Error::Forbidden);
        }
        let path = path.as_str().ok_or_else(device_invalid)?;
        if !(tool.name == "list_files" && path == ".") {
            crate::android_private_snapshots::relative(path)?;
        }
    }
    for k in ["name", "destination"] {
        if let Some(p) = v.get(k) {
            crate::android_private_snapshots::relative(p.as_str().ok_or_else(device_invalid)?)?;
        }
    }
    if let Some(c) = v.get("content") {
        if !c.is_string() || c.as_str().unwrap().len() > 1024 * 1024 {
            return Err(Error::BudgetExceeded);
        }
    }
    Ok(())
}
pub(crate) async fn execute(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    tool: &ToolCall,
    keys: &dyn KeyStore,
    key_id: KeyId,
    current: i64,
) -> Result<Value> {
    let run = id(r, "id")?;
    let all = entries(tx, run).await?;
    let v = &tool.arguments;
    let path = v["path"].as_str().unwrap_or("");
    let result = match tool.name.as_str() {
        "list_files" => {
            if path != "." && at(&all, path).is_none_or(|v| v["kind"] != "directory") {
                return Err(Error::NotFound);
            }
            let prefix = if path == "." {
                String::new()
            } else {
                format!("{path}/")
            };
            json!({"entries":all.iter().filter(|v|v["path"].as_str().is_some_and(|p|p.strip_prefix(&prefix).is_some_and(|tail|!tail.contains('/')))).collect::<Vec<_>>()})
        }
        "read_text_file" => {
            let f = at(&all, path)
                .filter(|v| v["kind"] == "file")
                .ok_or(Error::NotFound)?;
            if f["size"].as_u64().ok_or_else(device_invalid)? > 1024 * 1024 {
                return Err(Error::BudgetExceeded);
            }
            let bytes = version_bytes(tx, r, field(f, "version_id")?, keys).await?;
            json!({"path":path,"sha256":f["sha256"],"content":std::str::from_utf8(&bytes).map_err(|_|device_invalid())?})
        }
        "stat_file" => {
            let f = at(&all, path).ok_or(Error::NotFound)?;
            json!({"path":path,"kind":f["kind"],"size":f["size"],"sha256":tree_sha(&all,path)?})
        }
        "write_text_file" => {
            assert_parent(&all, path)?;
            let old = at(&all, path);
            if old.is_some_and(|f| f["kind"] != "file")
                || match old {
                    Some(f) => v["expected_sha256"] != f["sha256"],
                    None => !v["expected_sha256"].is_null(),
                }
            {
                return Err(Error::Conflict);
            }
            let version = add_version(
                tx,
                r,
                path,
                "file",
                field(v, "content")?.as_bytes(),
                keys,
                key_id,
                current,
            )
            .await?;
            put_entry(tx, run, path, &version).await?;
            json!({"path":path,"sha256":hash(field(v,"content")?.as_bytes()),"retained_before":old.map(|f|vec![f.clone()]).unwrap_or_default(),"retained_after":[{"path":path,"version_id":version,"kind":"file"}]})
        }
        "create_directory" => {
            assert_parent(&all, path)?;
            if at(&all, path).is_some() {
                return Err(Error::Conflict);
            }
            let version = add_version(tx, r, path, "directory", b"", keys, key_id, current).await?;
            put_entry(tx, run, path, &version).await?;
            json!({"path":path,"retained_after":[{"path":path,"version_id":version,"kind":"directory"}]})
        }
        "remove_file" | "rename_file" => {
            if v["expected_sha256"] != json!(tree_sha(&all, path)?) {
                return Err(Error::Conflict);
            }
            let retained = tree(&all, path);
            let destination = if tool.name == "rename_file" {
                Some(field(v, "destination")?)
            } else {
                None
            };
            if let Some(d) = destination {
                assert_parent(&all, d)?;
                if at(&all, d).is_some() || d.starts_with(&format!("{path}/")) {
                    return Err(Error::Conflict);
                }
            }
            let mut after = Vec::new();
            for entry in &retained {
                let source = field(entry, "path")?;
                sqlx::query("DELETE FROM android_private_loop_entries WHERE run_id=$1 AND path=$2")
                    .bind(run.to_string())
                    .bind(source)
                    .execute(&mut **tx)
                    .await
                    .map_err(storage_error)?;
                if let Some(d) = destination {
                    let target = format!(
                        "{d}{}",
                        source.strip_prefix(path).ok_or_else(device_invalid)?
                    );
                    put_entry(tx, run, &target, field(entry, "version_id")?).await?;
                    let mut e = entry.clone();
                    e["path"] = json!(target);
                    after.push(e);
                }
            }
            json!({"path":path,"destination":destination,"retained_before":retained,"retained_after":after})
        }
        "restore_file" => {
            assert_parent(&all, path)?;
            if at(&all, path).is_some() {
                return Err(Error::Conflict);
            }
            let source_run =
                Uuid::parse_str(field(&v["source"], "run_id")?).map_err(|_| device_invalid())?;
            let source = row(tx, source_run).await?;
            if id(&source, "owner_id")? != id(r, "owner_id")?
                || id(&source, "project_id")? != id(r, "project_id")?
                || id(&source, "thread_id")? != id(r, "thread_id")?
            {
                return Err(Error::Forbidden);
            }
            let retained = crate::android_private_tools::retained_tool_version(
                tx,
                &source,
                field(&v["source"], "tool_call_id")?,
                field(&v["source"], "version")?,
                keys,
            )
            .await?;
            let original = retained
                .first()
                .and_then(|v| v["path"].as_str())
                .ok_or(Error::NotFound)?;
            let mut after = Vec::new();
            for e in &retained {
                let source_path = field(e, "path")?;
                let target = format!(
                    "{path}{}",
                    source_path
                        .strip_prefix(original)
                        .ok_or_else(device_invalid)?
                );
                if at(&all, &target).is_some() {
                    return Err(Error::Conflict);
                }
                let bytes = version_bytes(tx, &source, field(e, "version_id")?, keys).await?;
                let version = add_version(
                    tx,
                    r,
                    &target,
                    field(e, "kind")?,
                    &bytes,
                    keys,
                    key_id,
                    current,
                )
                .await?;
                put_entry(tx, run, &target, &version).await?;
                after.push(json!({"path":target,"version_id":version,"kind":e["kind"]}));
            }
            json!({"path":path,"source":v["source"],"retained_after":after})
        }
        "create_artifact" => {
            let artifact = Uuid::new_v4();
            let name = field(v, "name")?;
            let bytes = field(v, "content")?.as_bytes();
            let version = add_version(
                tx,
                r,
                &format!("artifacts/{artifact}/{name}"),
                "artifact",
                bytes,
                keys,
                key_id,
                current,
            )
            .await?;
            let metadata = json!({"schema_version":2,"id":artifact,"run_id":run,"revision":1,"kind":"result","media_type":"text/plain; charset=utf-8","name":name,"digest":hash(bytes),"size_bytes":bytes.len(),"metadata":{"adapter":"android_private_loop"},"created_at":device_iso(current)?,"deleted_at":null});
            sqlx::query("INSERT INTO android_private_loop_artifacts(id,run_id,version_id,payload) VALUES($1,$2,$3,$4)").bind(artifact.to_string()).bind(run.to_string()).bind(version).bind(json_string(&metadata)?).execute(&mut **tx).await.map_err(storage_error)?;
            metadata
        }
        "list_artifacts" => {
            let rows:Vec<String>=sqlx::query_scalar("SELECT payload FROM android_private_loop_artifacts WHERE run_id=$1 ORDER BY id LIMIT 1000").bind(run.to_string()).fetch_all(&mut **tx).await.map_err(storage_error)?;
            json!({"artifacts":rows.iter().map(|s|parse::<Value>(s)).collect::<Result<Vec<_>>>()?})
        }
        "read_artifact" => {
            let value=sqlx::query("SELECT version_id,payload FROM android_private_loop_artifacts WHERE id=$1 AND run_id=$2").bind(field(v,"id")?).bind(run.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
            let bytes = version_bytes(
                tx,
                r,
                &value
                    .try_get::<String, _>("version_id")
                    .map_err(storage_error)?,
                keys,
            )
            .await?;
            json!({"metadata":parse::<Value>(&value.try_get::<String,_>("payload").map_err(storage_error)?)?,"content":std::str::from_utf8(&bytes).map_err(|_|device_invalid())?})
        }
        _ => return Err(Error::Forbidden),
    };
    bounds(tx, run).await?;
    Ok(result)
}
pub(crate) async fn snapshot(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    keys: &dyn KeyStore,
    key_id: KeyId,
    current: i64,
) -> Result<(Uuid, Value)> {
    let run = id(r, "id")?;
    let snapshot = Uuid::new_v4();
    let all = entries(tx, run).await?;
    let mut files = Vec::new();
    let mut total = 0usize;
    let mut emitted = std::collections::BTreeSet::new();
    let owner = id(r, "owner_id")?;
    let project = id(r, "project_id")?;
    // Insert the snapshot before chunks to preserve SQL referential integrity. Its bytes are private until committed in this same transaction.
    sqlx::query("INSERT INTO android_private_snapshots(id,owner_id,project_id,creation_key,request_sha,payload,committed) VALUES($1,$2,$3,$4,$5,'{}',0)").bind(snapshot.to_string()).bind(owner.to_string()).bind(project.to_string()).bind(format!("private-result:{run}")).bind(canonical_hash(&all)).execute(&mut **tx).await.map_err(storage_error)?;
    for f in all.iter().filter(|f| f["kind"] == "file") {
        let bytes = version_bytes(tx, r, field(f, "version_id")?, keys).await?;
        total = total
            .checked_add(bytes.len())
            .ok_or(Error::BudgetExceeded)?;
        let mut chunks = Vec::new();
        for bytes in bytes.chunks(4 * 1024 * 1024) {
            let digest = hash(bytes);
            chunks.push(json!({"digest":digest,"plaintext_size":bytes.len(),"ciphertext_size":bytes.len()+16}));
            if emitted.insert(digest.clone()) {
                let sealed = crate::android_private_snapshots::seal(
                    keys, key_id, owner, project, snapshot, &digest, bytes,
                )?;
                crate::server_keys::cipher_writer(tx, key_id).await?;
                sqlx::query("INSERT INTO android_private_snapshot_chunks(snapshot_id,digest,size,sealed) VALUES($1,$2,$3,$4)").bind(snapshot.to_string()).bind(digest).bind(bytes.len() as i64).bind(json_string(&sealed)?).execute(&mut **tx).await.map_err(storage_error)?;
            }
        }
        files.push(json!({"path":f["path"],"size":bytes.len(),"mode":0o644,"modified_at":device_iso(current)?,"chunks":chunks}));
    }
    if files.len() > 2000 || total as u64 > TREE_LIMIT {
        return Err(Error::BudgetExceeded);
    }
    let manifest = json!({"schema_version":2,"id":snapshot,"project_id":project,"total_bytes":total,"files":files,"directories":all.iter().filter(|f|f["kind"]=="directory").map(|f|f["path"].clone()).collect::<Vec<_>>(),"encryption_key_id":key_id.cipher_id().map_err(|_|Error::NativeKeyUnavailable)?,"created_at":device_iso(current)?,"expires_at":null});
    sqlx::query(
        "UPDATE android_private_snapshots SET payload=$1,committed=1 WHERE id=$2 AND committed=0",
    )
    .bind(json_string(&manifest)?)
    .bind(snapshot.to_string())
    .execute(&mut **tx)
    .await
    .map_err(storage_error)?;
    for f in files {
        let path = field(&f, "path")?;
        let fid = Uuid::new_v4();
        let payload = json!({"schema_version":2,"id":fid,"project_id":project,"snapshot_id":snapshot,"name":path,"path":path,"size":f["size"],"size_bytes":f["size"],"revision":1,"deleted_at":null});
        sqlx::query("INSERT INTO android_private_files(id,owner_id,project_id,snapshot_id,path,payload) VALUES($1,$2,$3,$4,$5,$6)").bind(fid.to_string()).bind(owner.to_string()).bind(project.to_string()).bind(snapshot.to_string()).bind(path).bind(json_string(&payload)?).execute(&mut **tx).await.map_err(storage_error)?;
    }
    Ok((snapshot, manifest))
}
