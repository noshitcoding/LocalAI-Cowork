//! Actual native original file custody and conservative CAS/conflict recovery.
use super::*;
use crate::enterprise_policy::{digest, indirect, Custody, Pin};
use std::io::Write;
mod original_bytes {
    use super::*;
    pub fn serialize<S: serde::Serializer>(v: &[u8], s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&BASE64.encode(v))
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        d: D,
    ) -> std::result::Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        BASE64.decode(s).map_err(serde::de::Error::custom)
    }
}
pub(super) async fn chunk(daemon: &Daemon, params: Value) -> Result<Value> {
    let id: Uuid = serde_json::from_value(params["transfer_id"].clone())?;
    let offset = params["offset"]
        .as_u64()
        .context("original offset missing")? as usize;
    let size = params["size"]
        .as_u64()
        .filter(|s| *s <= 1024 * 1024)
        .context("bounded native chunk required")? as usize;
    let db = daemon.database.lock().await;
    let raw: String = db.query_row(
        "SELECT encrypted_original FROM daemon_enterprise_file_transfers WHERE id=?1",
        [id.to_string()],
        |r| r.get(0),
    )?;
    let o: Original = decode(daemon, &raw)?;
    if !o.source || o.id != id || offset > o.bytes.len() {
        bail!("original native source chunk differs")
    };
    let end = offset
        .checked_add(size)
        .context("source chunk overflow")?
        .min(o.bytes.len());
    let hex = o.bytes[offset..end]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    Ok(json!({"transfer_id":id,"offset":offset,"hex":hex,"source_sha256":hash(&o.bytes)}))
}
pub(super) async fn accept_chunk(daemon: &Daemon, params: Value) -> Result<Value> {
    let v = &params["transfer"];
    custody(daemon, v, false).await?;
    let id: Uuid = serde_json::from_value(v["id"].clone())?;
    let offset = params["offset"]
        .as_u64()
        .filter(|o| *o <= 16 * 1024 * 1024)
        .context("bounded original chunk offset required")?;
    let text = params["hex"]
        .as_str()
        .filter(|s| {
            s.len() <= 2 * 1024 * 1024
                && s.len() % 2 == 0
                && s.bytes().all(|b| b.is_ascii_hexdigit())
        })
        .context("bounded original native chunk required")?;
    let bytes = (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let meta = digest(&immutable(v))?;
    let db = daemon.database.lock().await;
    db.execute_batch(SCHEMA)?;
    if let Some((old,sha))=db.query_row("SELECT encrypted_original,metadata_sha256 FROM daemon_enterprise_transfer_chunks WHERE id=?1 AND offset=?2",params![id.to_string(),offset],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).optional()? {if sha!=meta||decrypt_secret_payload(&old,&daemon.config.model_secret_key)?!=bytes{bail!("different immutable downloaded chunk")};}else{let count:i64=db.query_row("SELECT COUNT(*) FROM daemon_enterprise_transfer_chunks WHERE id=?1",[id.to_string()],|r|r.get(0))?;if count>=16{bail!("original file chunk count exceeded")};let used:i64=db.query_row("SELECT COALESCE(SUM(length(encrypted_original)),0) FROM daemon_enterprise_transfer_chunks",[],|r|r.get(0))?;if used>128*1024*1024{bail!("encrypted native transfer chunk capacity reached")};db.execute("INSERT INTO daemon_enterprise_transfer_chunks(id,offset,encrypted_original,metadata_sha256) VALUES(?1,?2,?3,?4)",params![id.to_string(),offset,encrypt_secret_payload(&bytes,&daemon.config.model_secret_key)?,meta])?;}
    Ok(json!({"transfer_id":id,"offset":offset,"size":bytes.len()}))
}
async fn assembled(daemon: &Daemon, v: &Value) -> Result<Vec<u8>> {
    let id: Uuid = serde_json::from_value(v["id"].clone())?;
    let db = daemon.database.lock().await;
    db.execute_batch(SCHEMA)?;
    let mut s=db.prepare("SELECT offset,encrypted_original,metadata_sha256 FROM daemon_enterprise_transfer_chunks WHERE id=?1 ORDER BY offset")?;
    let rows = s
        .query_map([id.to_string()], |r| {
            Ok((
                r.get::<_, usize>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let meta = digest(&immutable(v))?;
    let mut bytes = Vec::new();
    for (offset, cipher, sha) in rows {
        if offset != bytes.len() || sha != meta {
            bail!("original native download chunk gap or changed metadata")
        };
        let chunk = decrypt_secret_payload(&cipher, &daemon.config.model_secret_key)?;
        if bytes.len() + chunk.len() > 16 * 1024 * 1024 {
            bail!("original native file exceeds limit")
        };
        bytes.extend_from_slice(&chunk)
    }
    Ok(bytes)
}
const SCHEMA:&str="CREATE TABLE IF NOT EXISTS daemon_enterprise_file_transfers(id TEXT PRIMARY KEY,encrypted_original TEXT NOT NULL,phase TEXT NOT NULL,encrypted_receipt TEXT,encrypted_server_receipt TEXT);CREATE TABLE IF NOT EXISTS daemon_enterprise_transfer_chunks(id TEXT NOT NULL,offset INTEGER NOT NULL,encrypted_original TEXT NOT NULL,metadata_sha256 TEXT NOT NULL,PRIMARY KEY(id,offset));";
#[derive(Serialize, Deserialize)]
struct Original {
    format: u16,
    id: Uuid,
    source: bool,
    server_url: String,
    metadata: Value,
    grant_id: Uuid,
    request_id: Uuid,
    path: PathBuf,
    stage: PathBuf,
    backup: PathBuf,
    #[serde(with = "original_bytes")]
    bytes: Vec<u8>,
    before: Option<String>,
    baseline: Option<String>,
    stage_identity: Option<String>,
    conflict: bool,
}
#[derive(Serialize, Deserialize)]
struct Ack {
    request_id: Uuid,
    source_sha256: String,
    destination_before_sha256: Option<String>,
    destination_after_sha256: String,
    conflict: bool,
    local_receipt_sha256: String,
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn immutable(v: &Value) -> Value {
    json!({"id":v["id"],"owner_user_id":v["owner_user_id"],"grant_id":v["grant_id"],"grant_revision":v["grant_revision"],"relative_path":v["relative_path"],"size":v["size"],"sha256":v["sha256"],"original_destination_sha256":v["original_destination_sha256"],"grant":v["grant"]})
}
fn seal(daemon: &Daemon, v: &impl Serialize) -> Result<String> {
    encrypt_secret_payload(&serde_json::to_vec(v)?, &daemon.config.model_secret_key)
}
fn decode<T: serde::de::DeserializeOwned>(daemon: &Daemon, s: &str) -> Result<T> {
    Ok(serde_json::from_slice(&decrypt_secret_payload(
        s,
        &daemon.config.model_secret_key,
    )?)?)
}
fn read(path: &Path) -> Result<Vec<u8>> {
    let m = fs::symlink_metadata(path)?;
    if indirect(&m) || !m.is_file() || m.len() > 16 * 1024 * 1024 {
        bail!("native transfer requires a regular original file within 16 MiB")
    };
    let b = fs::read(path)?;
    if b.len() > 16 * 1024 * 1024 {
        bail!("native original file exceeds transfer bound")
    };
    Ok(b)
}
fn before(path: &Path) -> Result<Option<String>> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(Some(hash(&read(path)?))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn identity(file: &File) -> Result<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = file.metadata()?;
        return Ok(format!("{}:{}", m.dev(), m.ino()));
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        #[repr(C)]
        struct Info {
            attrs: u32,
            creation: [u32; 2],
            access: [u32; 2],
            write: [u32; 2],
            volume: u32,
            high_size: u32,
            low_size: u32,
            links: u32,
            high: u32,
            low: u32,
        }
        #[link(name = "kernel32")]
        extern "system" {
            fn GetFileInformationByHandle(handle: *mut std::ffi::c_void, info: *mut Info) -> i32;
        }
        let mut info: Info = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        return Ok(format!("{}:{}:{}", info.volume, info.high, info.low));
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        bail!("native filesystem identity is unsupported")
    }
}
async fn custody(daemon: &Daemon, v: &Value, source: bool) -> Result<Custody> {
    let endpoint = &v["grant"][if source { "source" } else { "destination" }];
    if endpoint["kind"] != "device" || endpoint["id"] != json!(daemon.config.device_id) {
        bail!("original transfer endpoint is not this native device")
    };
    let id: Uuid = serde_json::from_value(endpoint["grant_id"].clone())?;
    let db = daemon.database.lock().await;
    db.execute_batch(SCHEMA)?;
    let(raw,seen):(String,String)=db.query_row("SELECT encrypted_original,last_seen_at FROM daemon_enterprise_grants WHERE grant_id=?1 AND stopped=0 AND confirmation IS NOT NULL",[id.to_string()],|r|Ok((r.get(0)?,r.get(1)?)))?;
    let c: Custody = decode(daemon, &raw)?;
    let at = enterprise_local::clock(&db)?;
    let seen = DateTime::parse_from_rfc3339(&seen)?.with_timezone(&Utc);
    if c.expires_at <= at
        || at.signed_duration_since(seen).num_seconds() > 15
        || v["owner_user_id"] != c.original_grant["owner_user_id"]
        || v["grant"]["chat_id"] != c.original_grant["chat_id"]
        || !c.policy.capabilities.contains("files")
        || matches!(
            c.policy.access,
            cowork_contracts::enterprise::LaptopAccess::None
                | cowork_contracts::enterprise::LaptopAccess::Sandbox
        )
    {
        bail!("current actual native file custody unavailable")
    };
    for p in c.roots.values() {
        crate::enterprise_policy::known_root(p)?;
    }
    Ok(c)
}
fn pin(c: &Custody) -> Pin {
    Pin {
        grant_id: c.grant_id,
        revision: c.pending_revision + 1,
        policy_sha256: c.policy_sha256.clone(),
        access: c.policy.access,
        read_roots: c.policy.read_roots.clone(),
        write_roots: c.policy.write_roots.clone(),
        capabilities: c.policy.capabilities.clone(),
        offline_seconds: c.policy.offline_seconds,
        max_offline_events: c.policy.max_offline_events,
    }
}
pub(super) async fn binding(daemon: &Daemon, params: Value) -> Result<Value> {
    let source = params["source"]
        .as_bool()
        .context("actual transfer direction missing")?;
    let endpoint = &params["transfer"]["grant"][if source { "source" } else { "destination" }];
    let id: Uuid = serde_json::from_value(endpoint["grant_id"].clone())?;
    let db = daemon.database.lock().await;
    let raw:String=db.query_row("SELECT encrypted_original FROM daemon_enterprise_grants WHERE grant_id=?1 AND stopped=0 AND confirmation IS NOT NULL",[id.to_string()],|r|r.get(0))?;
    let c: Custody = decode(daemon, &raw)?;
    if endpoint["kind"] != "device"
        || endpoint["id"] != json!(c.device_id)
        || c.device_id != daemon.config.device_id
    {
        bail!("native original transfer binding differs")
    };
    Ok(json!({"pin":pin(&c)}))
}
pub(super) async fn original(daemon: &Daemon, params: Value) -> Result<Value> {
    let v = &params["transfer"];
    let source = params["source"]
        .as_bool()
        .context("native transfer direction missing")?;
    let c = custody(daemon, v, source).await?;
    let server = params["server_url"]
        .as_str()
        .context("actual server origin missing")?;
    if server.trim_end_matches('/') != c.server_url {
        bail!("native transfer server origin differs")
    };
    let id: Uuid = serde_json::from_value(v["id"].clone())?;
    let rel = v["relative_path"]
        .as_str()
        .filter(|p| cowork_contracts::enterprise::safe_relative_path(p))
        .context("safe original transfer path required")?;
    let permitted = v["grant"]["relative_paths"].as_array().is_some_and(|a| {
        a.iter()
            .filter_map(Value::as_str)
            .any(|p| rel == p || rel.starts_with(&(p.to_owned() + "/")))
    });
    if !permitted {
        bail!("path exceeds original transfer prefix")
    };
    let aliases = if source {
        &c.policy.read_roots
    } else {
        &c.policy.write_roots
    };
    let (alias, root) = aliases
        .iter()
        .filter_map(|a| c.roots.get(a).map(|r| (a, r)))
        .filter(|(a, _)| rel.starts_with(&(a.to_string() + "/")))
        .max_by_key(|(a, _)| a.len())
        .context("transfer path has no actual locally selected root alias")?;
    let path = safe_workspace_path_no_symlinks(root, &rel[alias.len() + 1..], false)?;
    {
        let db = daemon.database.lock().await;
        db.execute_batch(SCHEMA)?;
        if let Some((s,phase,receipt))=db.query_row("SELECT encrypted_original,phase,encrypted_receipt FROM daemon_enterprise_file_transfers WHERE id=?1",[id.to_string()],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?))).optional()?{let old:Original=decode(daemon,&s)?;if old.id!=id||old.source!=source||old.metadata!=immutable(v)||old.server_url!=server||old.path!=path||old.grant_id!=c.grant_id{bail!("different immutable native file transfer")};return Ok(json!({"phase":phase,"size":old.bytes.len(),"local_receipt":receipt.map(|s|decode::<Value>(daemon,&s)).transpose()?}));}
    }
    let bytes = if source {
        read(&path)?
    } else {
        assembled(daemon, v).await?
    };
    if bytes.len() > 16 * 1024 * 1024
        || v["size"].as_u64() != Some(bytes.len() as u64)
        || v["sha256"] != json!(hash(&bytes))
    {
        bail!("actual native original file differs from transfer size/SHA")
    };
    let before = if source { None } else { before(&path)? };
    let baseline: Option<String> =
        serde_json::from_value(v["original_destination_sha256"].clone())?;
    if !source && !v["grant"]["writable"].as_bool().unwrap_or(false) && before.is_some() {
        bail!("original transfer grant cannot replace an existing file")
    };
    let conflict = !source && before != baseline;
    let parent = path.parent().context("native transfer parent missing")?;
    if !parent.is_dir() {
        bail!("destination parent must already be locally present")
    };
    let stage = parent.join(format!(".cowork-transfer-{id}.original"));
    let backup = parent.join(format!(".cowork-transfer-{id}.before"));
    let o = Original {
        format: 1,
        id,
        source,
        server_url: server.into(),
        metadata: immutable(v),
        grant_id: c.grant_id,
        request_id: Uuid::new_v4(),
        path,
        stage,
        backup,
        bytes,
        before,
        baseline,
        stage_identity: None,
        conflict,
    };
    let db = daemon.database.lock().await;
    let count: i64 = db.query_row(
        "SELECT COUNT(*) FROM daemon_enterprise_file_transfers",
        [],
        |r| r.get(0),
    )?;
    if count >= 1000 {
        bail!("native immutable file transfer custody bound reached")
    };
    let capacity: i64 = db.query_row(
        "SELECT COALESCE(SUM(length(encrypted_original)),0) FROM daemon_enterprise_file_transfers",
        [],
        |r| r.get(0),
    )?;
    if capacity + ((o.bytes.len() as i64) * 2) > 256 * 1024 * 1024 {
        bail!("encrypted original native transfer capacity reached");
    }
    db.execute("INSERT INTO daemon_enterprise_file_transfers(id,encrypted_original,phase) VALUES(?1,?2,'prepared')",params![id.to_string(),seal(daemon,&o)?])?;
    Ok(json!({"phase":"prepared","size":o.bytes.len()}))
}
pub(super) async fn apply(daemon: &Daemon, params: Value) -> Result<Value> {
    let v = &params["transfer"];
    let c = custody(daemon, v, false).await?;
    let id: Uuid = serde_json::from_value(v["id"].clone())?;
    let db = daemon.database.lock().await;
    let(raw,phase,receipt):(String,String,Option<String>)=db.query_row("SELECT encrypted_original,phase,encrypted_receipt FROM daemon_enterprise_file_transfers WHERE id=?1",[id.to_string()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    let mut o: Original = decode(daemon, &raw)?;
    if o.source
        || o.format != 1
        || o.id != id
        || o.metadata != immutable(v)
        || o.grant_id != c.grant_id
    {
        bail!("native original destination plan differs")
    };
    if let Some(r) = receipt {
        return Ok(decode::<Value>(daemon, &r)?);
    };
    if phase == "prepared" {
        if before(&o.path)? != o.before {
            o.conflict = true;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&o.stage)?;
        file.write_all(&o.bytes)?;
        file.sync_all()?;
        o.stage_identity = Some(identity(&file)?);
        db.execute("UPDATE daemon_enterprise_file_transfers SET encrypted_original=?1,phase='dispatched' WHERE id=?2 AND phase='prepared'",params![seal(daemon,&o)?,id.to_string()])?;
        if !o.conflict {
            if o.before.is_some() {
                if o.backup.exists() {
                    bail!("original destination backup path already exists; no overwrite");
                }
                fs::rename(&o.path, &o.backup)?;
                if before(&o.backup)? != o.baseline {
                    o.conflict = true;
                    let _ = fs::hard_link(&o.backup, &o.path);
                }
            }
            if !o.conflict && fs::hard_link(&o.stage, &o.path).is_err() {
                o.conflict = true;
            }
        }
        db.execute("UPDATE daemon_enterprise_file_transfers SET encrypted_original=?1 WHERE id=?2 AND phase='dispatched'",params![seal(daemon,&o)?,id.to_string()])?;
    } else if phase != "dispatched" {
        bail!("native original transfer phase unavailable")
    };
    // A current matching digest alone is insufficient: the destination must be the original staged inode.
    let proven = if let Ok(file) = File::open(&o.path) {
        Some(identity(&file)?) == o.stage_identity && before(&o.path)? == Some(hash(&o.bytes))
    } else {
        false
    };
    if !o.conflict && !proven {
        bail!("original native file write outcome remains unknown; original stage/backup retained; no replay")
    };
    let target = if o.conflict { &o.stage } else { &o.path };
    if before(target)? != Some(hash(&o.bytes)) {
        bail!("original native staged bytes changed")
    };
    let fact = json!({"format":1,"transfer_id":id,"request_id":o.request_id,"source_sha256":hash(&o.bytes),"original_baseline_sha256":o.baseline,"actual_destination_before_sha256":o.before,"actual_saved_path":target,"actual_saved_sha256":hash(&o.bytes),"original_stage_identity":o.stage_identity,"conflict":o.conflict,"backup_path":o.backup,"file_operation_replayed":false});
    let ack = Ack {
        request_id: o.request_id,
        source_sha256: hash(&o.bytes),
        destination_before_sha256: o.baseline.clone(),
        destination_after_sha256: hash(&o.bytes),
        conflict: o.conflict,
        local_receipt_sha256: digest(&fact)?,
    };
    let receipt = json!({"fact":fact,"acknowledge":ack});
    db.execute("UPDATE daemon_enterprise_file_transfers SET encrypted_receipt=?1,phase='applied' WHERE id=?2 AND encrypted_receipt IS NULL",params![seal(daemon,&receipt)?,id.to_string()])?;
    Ok(receipt)
}
pub(super) async fn ack(daemon: &Daemon, params: Value) -> Result<Value> {
    let id: Uuid = serde_json::from_value(params["transfer_id"].clone())?;
    let actual = &params["receipt"];
    let db = daemon.database.lock().await;
    let raw:String=db.query_row("SELECT encrypted_receipt FROM daemon_enterprise_file_transfers WHERE id=?1 AND phase IN ('applied','acknowledged')",[id.to_string()],|r|r.get(0))?;
    let local: Value = decode(daemon, &raw)?;
    let ack: Ack = serde_json::from_value(local["acknowledge"].clone())?;
    if actual["id"] != json!(id)
        || actual["request_id"] != json!(ack.request_id)
        || actual["request_sha256"] != json!(digest(&ack)?)
        || actual["source_sha256"] != json!(ack.source_sha256)
        || actual["destination_before_sha256"] != json!(ack.destination_before_sha256)
        || actual["destination_after_sha256"] != json!(ack.destination_after_sha256)
        || actual["local_receipt_sha256"] != json!(ack.local_receipt_sha256)
        || actual["destination_device_id"] != json!(daemon.config.device_id)
        || actual["state"]
            != if ack.conflict {
                json!("conflict")
            } else {
                json!("completed")
            }
    {
        bail!("actual original server file transfer ACK differs")
    };
    let old: Option<String> = db.query_row(
        "SELECT encrypted_server_receipt FROM daemon_enterprise_file_transfers WHERE id=?1",
        [id.to_string()],
        |r| r.get(0),
    )?;
    if let Some(old) = old {
        if decode::<Value>(daemon, &old)? != *actual {
            bail!("different original server transfer receipt");
        }
    }
    db.execute("UPDATE daemon_enterprise_file_transfers SET phase='acknowledged',encrypted_server_receipt=?2 WHERE id=?1",params![id.to_string(),seal(daemon,actual)?])?;
    Ok(actual.clone())
}
pub(super) async fn source_ack(daemon: &Daemon, params: Value) -> Result<Value> {
    let id: Uuid = serde_json::from_value(params["transfer_id"].clone())?;
    let receipt = &params["receipt"];
    let db = daemon.database.lock().await;
    let(raw,old):(String,Option<String>)=db.query_row("SELECT encrypted_original,encrypted_server_receipt FROM daemon_enterprise_file_transfers WHERE id=?1",[id.to_string()],|r|Ok((r.get(0)?,r.get(1)?)))?;
    let o: Original = decode(daemon, &raw)?;
    if !o.source
        || o.id != id
        || receipt["id"] != json!(id)
        || receipt["sha256"] != json!(hash(&o.bytes))
        || receipt["size"] != json!(o.bytes.len())
        || receipt["source_device_id"] != json!(daemon.config.device_id)
        || receipt["source_bytes_retained"] != true
    {
        bail!("actual source upload receipt differs from native original bytes")
    };
    if let Some(old) = old {
        if decode::<Value>(daemon, &old)? != *receipt {
            bail!("different original source upload receipt")
        }
    }
    db.execute("UPDATE daemon_enterprise_file_transfers SET encrypted_server_receipt=?2,phase='source_acknowledged' WHERE id=?1",params![id.to_string(),seal(daemon,receipt)?])?;
    Ok(receipt.clone())
}
pub(super) async fn pending(daemon: &Daemon, params: Value) -> Result<Value> {
    let server = params["server_url"]
        .as_str()
        .context("actual source server missing")?;
    let db = daemon.database.lock().await;
    db.execute_batch(SCHEMA)?;
    let mut q=db.prepare("SELECT encrypted_original,phase FROM daemon_enterprise_file_transfers WHERE phase NOT IN ('acknowledged','source_acknowledged') ORDER BY id LIMIT 1000")?;
    let rows = q
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut out = Vec::new();
    for (s, phase) in rows {
        let o: Original = decode(daemon, &s)?;
        if o.server_url == server.trim_end_matches('/') {
            out.push(json!({"id":o.id,"source":o.source,"phase":phase}))
        }
    }
    Ok(json!(out))
}
