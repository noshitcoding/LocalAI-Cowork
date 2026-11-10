//! Lease-bound device approvals, durable messages and inert content. No filesystem paths are grants.
use super::*;
use crate::android_device_registry::AndroidAgentPrincipal;
use crate::android_device_runs::{
    device_event, device_lease_current, device_run_permit, device_run_row,
};
use crate::android_devices::{device_invalid, device_iso, device_writer};
use base64::{Engine, engine::general_purpose::STANDARD};
use cowork_identity::{ContentAction, Principal};
use sqlx::{Any, Transaction};
use team_service::identity_error;
fn text(v: &Value, key: &str) -> Result<String> {
    v[key]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(device_invalid)
}
fn chunk_digest(d: &str) -> Result<()> {
    if d.len() == 64 && d.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(device_invalid())
    }
}
pub(crate) fn relative(v: &str) -> Result<()> {
    if v.is_empty()
        || v.len() > 1000
        || v.starts_with('/')
        || v.contains('\\')
        || v.contains(':')
        || v.split('/').any(|c| c.is_empty() || c == "." || c == "..")
        || v.chars().any(char::is_control)
    {
        Err(device_invalid())
    } else {
        Ok(())
    }
}
async fn store_artifact(
    tx: &mut Transaction<'_, Any>,
    run: Uuid,
    path: &str,
    source: &str,
    source_event: Option<Uuid>,
    media: &str,
    bytes: &[u8],
    current: i64,
) -> Result<Value> {
    relative(path)?;
    if bytes.is_empty()
        || bytes.len() > 8 * 1024 * 1024
        || source.len() > 100
        || media.len() > 200
        || media.chars().any(char::is_control)
    {
        return Err(device_invalid());
    }
    let digest = hash(bytes);
    let request = hash(json_string(&(run, path, source, source_event, media, &digest))?.as_bytes());
    let key = if let Some(source) = source_event {
        hash(json_string(&(run, "event", source))?.as_bytes())
    } else {
        request.clone()
    };
    let id = Uuid::parse_str(&key[..32]).map_err(storage_error)?;
    if let Some(row) =
        sqlx::query("SELECT payload FROM android_scoped_artifacts WHERE id=$1 AND run_id=$2")
            .bind(id.to_string())
            .bind(run.to_string())
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?
    {
        let v: Value = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        if v["metadata"]["request_sha256"].as_str() != Some(request.as_str()) {
            return Err(Error::Conflict);
        }
        return Ok(v);
    }
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM android_scoped_artifacts WHERE run_id=$1")
            .bind(run.to_string())
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
    if count >= 100 {
        return Err(Error::BudgetExceeded);
    }
    let v = json!({"schema_version":2,"id":id,"run_id":run,"revision":1,"kind":"file","name":path,"media_type":media,"digest":digest,"size_bytes":bytes.len(),"metadata":{"source":source,"source_event_id":source_event,"request_sha256":request},"created_at":device_iso(current)?,"deleted_at":null});
    sqlx::query(
        "INSERT INTO android_scoped_artifacts(id,run_id,payload,bytes_base64) VALUES($1,$2,$3,$4)",
    )
    .bind(id.to_string())
    .bind(run.to_string())
    .bind(json_string(&v)?)
    .bind(STANDARD.encode(bytes))
    .execute(&mut **tx)
    .await
    .map_err(storage_error)?;
    device_event(tx, run, None, "artifact_created", &v, current).await?;
    Ok(v)
}
impl Store {
    pub async fn android_device_snapshot(
        &self,
        actor: &Principal,
        run: Uuid,
        input: bool,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let r = device_run_row(&mut tx, run).await?;
        device_run_permit(self, &mut tx, actor, &r, ContentAction::Read).await?;
        let payload: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
        let id = if input {
            &payload["spec"]["snapshot_id"]
        } else {
            &payload["result_snapshot_manifest_id"]
        };
        let id = id.as_str().ok_or(Error::NotFound)?;
        if input&&r.try_get::<Option<String>,_>("team_id").map_err(storage_error)?.is_none(){let owner=Uuid::parse_str(&r.try_get::<String,_>("creator_id").map_err(storage_error)?).map_err(storage_error)?;let project=Uuid::parse_str(&r.try_get::<String,_>("project_id").map_err(storage_error)?).map_err(storage_error)?;let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_snapshots WHERE id=$1 AND owner_id=$2 AND project_id=$3").bind(id).bind(owner.to_string()).bind(project.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;if count==1{let current=device_writer(&mut tx).await?;let m=crate::android_private_snapshots::manifest(&mut tx,owner,project,Uuid::parse_str(id).map_err(storage_error)?,true,current).await?;tx.commit().await.map_err(storage_error)?;return Ok(m)}}
        let raw: String = sqlx::query_scalar("SELECT s.payload FROM android_scoped_snapshots s JOIN android_scoped_runs r ON r.id=s.run_id WHERE s.id=$1 AND s.committed=1 AND r.project_id=$2").bind(id).bind(r.try_get::<String,_>("project_id").map_err(storage_error)?).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        let manifest = parse(&raw)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(manifest)
    }
    pub async fn android_device_snapshot_file(
        &self,
        actor: &Principal,
        run: Uuid,
        input: bool,
        path: &str,
    ) -> Result<Vec<u8>> {
        relative(path)?;
        let manifest = self.android_device_snapshot(actor, run, input).await?;
        let file = manifest["files"]
            .as_array()
            .ok_or(Error::Conflict)?
            .iter()
            .find(|file| file["path"].as_str() == Some(path))
            .ok_or(Error::NotFound)?;
        let size = file["size"]
            .as_u64()
            .filter(|n| *n <= 32 * 1024 * 1024)
            .ok_or(Error::Conflict)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let r = device_run_row(&mut tx, run).await?;
        device_run_permit(self, &mut tx, actor, &r, ContentAction::Read).await?;
        let mut bytes = Vec::new();
        for chunk in file["chunks"].as_array().ok_or(Error::Conflict)? {
            let digest = text(chunk, "digest")?;
            let raw: String = sqlx::query_scalar("SELECT bytes_base64 FROM android_scoped_snapshot_chunks WHERE manifest_id=$1 AND digest=$2").bind(text(&manifest,"id")?).bind(&digest).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
            let decoded = STANDARD.decode(raw).map_err(storage_error)?;
            if hash(&decoded) != digest
                || chunk["plaintext_size"].as_u64() != Some(decoded.len() as u64)
            {
                return Err(Error::Conflict);
            }
            bytes.extend_from_slice(&decoded);
            if bytes.len() as u64 > size {
                return Err(Error::Conflict);
            }
        }
        if bytes.len() as u64 != size
            || file["digest"]
                .as_str()
                .is_some_and(|sha| hash(&bytes) != sha)
        {
            return Err(Error::Conflict);
        }
        device_run_permit(self, &mut tx, actor, &r, ContentAction::Read).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(bytes)
    }
    pub async fn android_agent_request(
        &self,
        a: &AndroidAgentPrincipal,
        run: Uuid,
        token: Uuid,
        kind: &str,
        source: Option<Uuid>,
        content: Value,
        expiry: Option<i64>,
    ) -> Result<Value> {
        if !matches!(kind, "approval" | "input") || json_string(&content)?.len() > 128 * 1024 {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        device_lease_current(&mut tx, a, run, token, current).await?;
        let digest = hash(json_string(&(kind, &content, expiry))?.as_bytes());
        if let Some(source) = source {
            if let Some(r)=sqlx::query("SELECT payload,request_sha256 FROM android_scoped_requests WHERE run_id=$1 AND kind=$2 AND source_id=$3").bind(run.to_string()).bind(kind).bind(source.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?{if r.try_get::<String,_>("request_sha256").map_err(storage_error)?!=digest{return Err(Error::Conflict)}let v=parse(&r.try_get::<String,_>("payload").map_err(storage_error)?)?;tx.commit().await.map_err(storage_error)?;return Ok(v)}
        }
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM android_scoped_requests WHERE run_id=$1")
                .bind(run.to_string())
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        if count >= 1000 {
            return Err(Error::BudgetExceeded);
        }
        let expiry = expiry
            .unwrap_or(current + 30 * 60_000)
            .min(current + 24 * 60 * 60_000);
        if expiry <= current {
            return Err(device_invalid());
        }
        let id = Uuid::new_v4();
        let mut v = json!({"schema_version":2,"id":id,"run_id":run,"revision":1,"etag":format!("{id}:1"),"state":"pending","requested_at":device_iso(current)?,"expires_at":device_iso(expiry)?});
        if kind == "approval" {
            v["requested_action"] = content;
            v["resolved_by_user_id"] = Value::Null;
            v["resolved_at"] = Value::Null
        } else {
            v["prompt"] = content;
            v["response"] = Value::Null;
            v["responded_by_user_id"] = Value::Null;
            v["responded_at"] = Value::Null
        }
        sqlx::query("INSERT INTO android_scoped_requests(id,run_id,kind,source_id,request_sha256,payload,state,revision,expires_at) VALUES($1,$2,$3,$4,$5,$6,'pending',1,$7)").bind(id.to_string()).bind(run.to_string()).bind(kind).bind(source.map(|s|s.to_string())).bind(digest).bind(json_string(&v)?).bind(expiry).execute(&mut *tx).await.map_err(storage_error)?;
        device_event(
            &mut tx,
            run,
            None,
            if kind == "approval" {
                "approval_requested"
            } else {
                "input_requested"
            },
            &json!({"request_id":id}),
            current,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn android_agent_request_status(
        &self,
        a: &AndroidAgentPrincipal,
        run: Uuid,
        id: Uuid,
        kind: &str,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let token: String =
            sqlx::query_scalar("SELECT lease_token FROM android_scoped_runs WHERE id=$1")
                .bind(run.to_string())
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        device_lease_current(
            &mut tx,
            a,
            run,
            Uuid::parse_str(&token).map_err(storage_error)?,
            current,
        )
        .await?;
        let r=sqlx::query("SELECT payload,state,expires_at FROM android_scoped_requests WHERE id=$1 AND run_id=$2 AND kind=$3").bind(id.to_string()).bind(run.to_string()).bind(kind).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        let mut v: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
        if r.try_get::<i64, _>("expires_at").map_err(storage_error)? <= current {
            v["state"] = json!("expired")
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn android_device_requests(
        &self,
        actor: &Principal,
        run: Uuid,
        kind: &str,
    ) -> Result<Vec<Value>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        let r = device_run_row(&mut tx, run).await?;
        device_run_permit(self, &mut tx, actor, &r, ContentAction::Read).await?;
        let rows=sqlx::query("SELECT payload,expires_at FROM android_scoped_requests WHERE run_id=$1 AND kind=$2 ORDER BY id LIMIT 1000").bind(run.to_string()).bind(kind).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let v = rows
            .iter()
            .map(|r| {
                let mut v: Value =
                    parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
                if r.try_get::<i64, _>("expires_at").map_err(storage_error)? <= now() {
                    v["state"] = json!("expired")
                }
                Ok(v)
            })
            .collect::<Result<Vec<_>>>()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn android_device_resolve(
        &self,
        actor: &Principal,
        run: Uuid,
        id: Uuid,
        kind: &str,
        revision: i64,
        response: Value,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        let current = device_writer(&mut tx).await?;
        let r = device_run_row(&mut tx, run).await?;
        device_run_permit(self, &mut tx, actor, &r, ContentAction::Execute).await?;
        crate::android_device_runs::device_run_authority(&mut tx, &r, current).await?;
        if !matches!(
            r.try_get::<String, _>("state")
                .map_err(storage_error)?
                .as_str(),
            "running" | "leased"
        ) {
            return Err(Error::Conflict);
        }
        let old=sqlx::query("SELECT payload,revision,state,expires_at FROM android_scoped_requests WHERE id=$1 AND run_id=$2 AND kind=$3").bind(id.to_string()).bind(run.to_string()).bind(kind).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        if old.try_get::<i64, _>("revision").map_err(storage_error)? != revision
            || old.try_get::<String, _>("state").map_err(storage_error)? != "pending"
            || old.try_get::<i64, _>("expires_at").map_err(storage_error)? <= current
        {
            return Err(Error::Conflict);
        }
        let mut v: Value = parse(&old.try_get::<String, _>("payload").map_err(storage_error)?)?;
        let state = if kind == "approval" {
            let state = response
                .as_str()
                .filter(|s| matches!(*s, "approved" | "rejected"))
                .ok_or_else(device_invalid)?;
            v["resolved_by_user_id"] = json!(actor.identity_id());
            v["resolved_at"] = json!(device_iso(current)?);
            state.to_owned()
        } else if kind == "input" {
            if json_string(&response)?.len() > 128 * 1024 {
                return Err(device_invalid());
            }
            v["response"] = response;
            v["responded_by_user_id"] = json!(actor.identity_id());
            v["responded_at"] = json!(device_iso(current)?);
            "submitted".into()
        } else {
            return Err(device_invalid());
        };
        v["state"] = json!(state);
        v["revision"] = json!(revision + 1);
        v["etag"] = json!(format!("{id}:{}", revision + 1));
        sqlx::query("UPDATE android_scoped_requests SET state=$1,payload=$2,revision=revision+1 WHERE id=$3 AND run_id=$4 AND revision=$5 AND state='pending'").bind(state).bind(json_string(&v)?).bind(id.to_string()).bind(run.to_string()).bind(revision).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn android_agent_checkpoint(
        &self,
        a: &AndroidAgentPrincipal,
        run: Uuid,
        token: Uuid,
        source: Uuid,
        safe: bool,
        state: Value,
    ) -> Result<Value> {
        if json_string(&state)?.len() > 512 * 1024 {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        device_lease_current(&mut tx, a, run, token, current).await?;
        let digest = hash(json_string(&(safe, &state))?.as_bytes());
        if let Some(r)=sqlx::query("SELECT payload,request_sha256 FROM android_scoped_checkpoints WHERE run_id=$1 AND source_id=$2").bind(run.to_string()).bind(source.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?{if r.try_get::<String,_>("request_sha256").map_err(storage_error)?!=digest{return Err(Error::Conflict)}let v=parse(&r.try_get::<String,_>("payload").map_err(storage_error)?)?;tx.commit().await.map_err(storage_error)?;return Ok(v)}
        let id = Uuid::new_v4();
        let sequence: i64 =
            sqlx::query_scalar("SELECT COUNT(*)+1 FROM android_scoped_checkpoints WHERE run_id=$1")
                .bind(run.to_string())
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        if sequence > 1000 {
            return Err(Error::BudgetExceeded);
        }
        let v = json!({"schema_version":2,"id":id,"run_id":run,"sequence":sequence,"safe_to_resume":safe,"executor_state":state,"created_at":device_iso(current)?});
        sqlx::query("INSERT INTO android_scoped_checkpoints(id,run_id,source_id,request_sha256,payload) VALUES($1,$2,$3,$4,$5)").bind(id.to_string()).bind(run.to_string()).bind(source.to_string()).bind(digest).bind(json_string(&v)?).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn android_agent_artifact(
        &self,
        a: &AndroidAgentPrincipal,
        run: Uuid,
        token: Uuid,
        path: &str,
        source: &str,
        source_event: Option<Uuid>,
        bytes: &[u8],
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        device_lease_current(&mut tx, a, run, token, current).await?;
        let v = store_artifact(
            &mut tx,
            run,
            path,
            source,
            source_event,
            "application/octet-stream",
            bytes,
            current,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn android_device_upload_attachment(
        &self,
        actor: &Principal,
        run: Uuid,
        path: &str,
        media: &str,
        bytes: &[u8],
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        let current = device_writer(&mut tx).await?;
        let r = device_run_row(&mut tx, run).await?;
        device_run_permit(self, &mut tx, actor, &r, ContentAction::Write).await?;
        let v = store_artifact(
            &mut tx,
            run,
            path,
            "attachment",
            None,
            media,
            bytes,
            current,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn android_device_artifacts(
        &self,
        actor: &Principal,
        run: Uuid,
    ) -> Result<Vec<Value>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        let r = device_run_row(&mut tx, run).await?;
        device_run_permit(self, &mut tx, actor, &r, ContentAction::Read).await?;
        let rows = sqlx::query(
            "SELECT payload FROM android_scoped_artifacts WHERE run_id=$1 ORDER BY id LIMIT 100",
        )
        .bind(run.to_string())
        .fetch_all(&mut *tx)
        .await
        .map_err(storage_error)?;
        let v = rows
            .iter()
            .map(|r| parse(&r.try_get::<String, _>("payload").map_err(storage_error)?))
            .collect::<Result<Vec<_>>>()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn android_device_artifact_bytes(
        &self,
        actor: &Principal,
        run: Uuid,
        id: Uuid,
    ) -> Result<Vec<u8>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        let r = device_run_row(&mut tx, run).await?;
        device_run_permit(self, &mut tx, actor, &r, ContentAction::Read).await?;
        let raw: String = sqlx::query_scalar(
            "SELECT bytes_base64 FROM android_scoped_artifacts WHERE run_id=$1 AND id=$2",
        )
        .bind(run.to_string())
        .bind(id.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
        let bytes = STANDARD.decode(raw).map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(bytes)
    }
    pub async fn android_agent_begin_snapshot(
        &self,
        a: &AndroidAgentPrincipal,
        run: Uuid,
        token: Uuid,
        v: Value,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let r = device_lease_current(&mut tx, a, run, token, current).await?;
        if text(&v, "project_id")?
            != r.try_get::<String, _>("project_id")
                .map_err(storage_error)?
        {
            return Err(Error::Forbidden);
        }
        let total = v["total_bytes"]
            .as_u64()
            .filter(|n| *n <= 256 * 1024 * 1024)
            .ok_or_else(device_invalid)?;
        let files = v["files"]
            .as_array()
            .filter(|f| f.len() <= 2000)
            .ok_or_else(device_invalid)?;
        let mut measured = 0u64;
        let mut missing = std::collections::BTreeSet::new();
        let mut paths = std::collections::BTreeSet::new();
        let mut public = Vec::new();
        for file in files {
            let path = text(file, "path")?;
            relative(&path)?;
            if !paths.insert(path) {
                return Err(device_invalid());
            }
            let size = file["size"].as_u64().ok_or_else(device_invalid)?;
            measured = measured.checked_add(size).ok_or(Error::BudgetExceeded)?;
            let chunks = file["chunks"]
                .as_array()
                .filter(|c| c.len() <= 2000)
                .ok_or_else(device_invalid)?;
            let mut ftotal = 0u64;
            let mut out = Vec::new();
            for c in chunks {
                let digest = text(c, "digest")?;
                chunk_digest(&digest)?;
                let size = c["plaintext_size"]
                    .as_u64()
                    .filter(|n| *n <= 4 * 1024 * 1024)
                    .ok_or_else(device_invalid)?;
                ftotal = ftotal.checked_add(size).ok_or(Error::BudgetExceeded)?;
                missing.insert(digest.clone());
                out.push(json!({"digest":digest,"plaintext_size":size,"ciphertext_size":size}))
            }
            if ftotal != size {
                return Err(device_invalid());
            }
            let mut f = file.clone();
            f["chunks"] = json!(out);
            public.push(f)
        }
        if measured != total || missing.len() > 4000 {
            return Err(device_invalid());
        }
        let id = Uuid::new_v4();
        let manifest = json!({"schema_version":2,"id":id,"project_id":v["project_id"],"total_bytes":total,"files":public,"encryption_key_id":"none","created_at":device_iso(current)?,"expires_at":null});
        sqlx::query(
            "INSERT INTO android_scoped_snapshots(id,run_id,payload,committed) VALUES($1,$2,$3,0)",
        )
        .bind(id.to_string())
        .bind(run.to_string())
        .bind(json_string(&manifest)?)
        .execute(&mut *tx)
        .await
        .map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(
            json!({"schema_version":2,"manifest_id":id,"missing_chunks":missing,"max_chunk_bytes":4*1024*1024,"expires_at":null,"warnings":[]}),
        )
    }
    pub async fn android_agent_snapshot_chunk(
        &self,
        a: &AndroidAgentPrincipal,
        run: Uuid,
        token: Uuid,
        manifest: Uuid,
        digest: &str,
        bytes: &[u8],
    ) -> Result<Value> {
        chunk_digest(digest)?;
        if bytes.len() > 4 * 1024 * 1024 || hash(bytes) != digest {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        device_lease_current(&mut tx, a, run, token, current).await?;
        let row = sqlx::query(
            "SELECT payload,committed FROM android_scoped_snapshots WHERE id=$1 AND run_id=$2",
        )
        .bind(manifest.to_string())
        .bind(run.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
        let m: Value = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        let expected = m["files"]
            .as_array()
            .ok_or(Error::Conflict)?
            .iter()
            .flat_map(|f| f["chunks"].as_array().into_iter().flatten())
            .filter(|c| c["digest"].as_str() == Some(digest))
            .all(|c| c["plaintext_size"].as_u64() == Some(bytes.len() as u64));
        let exists = m["files"]
            .as_array()
            .ok_or(Error::Conflict)?
            .iter()
            .any(|f| {
                f["chunks"]
                    .as_array()
                    .is_some_and(|cs| cs.iter().any(|c| c["digest"].as_str() == Some(digest)))
            });
        if !expected || !exists {
            return Err(Error::Conflict);
        }
        let old:Option<String>=sqlx::query_scalar("SELECT bytes_base64 FROM android_scoped_snapshot_chunks WHERE manifest_id=$1 AND digest=$2").bind(manifest.to_string()).bind(digest).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        let encoded = STANDARD.encode(bytes);
        if old.as_ref().is_some_and(|v| v != &encoded) {
            return Err(Error::Conflict);
        }
        if old.is_none() && row.try_get::<i64, _>("committed").map_err(storage_error)? != 0 {
            return Err(Error::Conflict);
        }
        sqlx::query("INSERT INTO android_scoped_snapshot_chunks(manifest_id,digest,bytes_base64) VALUES($1,$2,$3) ON CONFLICT(manifest_id,digest) DO NOTHING").bind(manifest.to_string()).bind(digest).bind(encoded).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(
            json!({"schema_version":2,"manifest_id":manifest,"digest":digest,"plaintext_size":bytes.len(),"ciphertext_size":bytes.len(),"deduplicated":old.is_some(),"warnings":[]}),
        )
    }
    pub async fn android_agent_commit_snapshot(
        &self,
        a: &AndroidAgentPrincipal,
        run: Uuid,
        token: Uuid,
        id: Uuid,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        device_lease_current(&mut tx, a, run, token, current).await?;
        let raw: String = sqlx::query_scalar(
            "SELECT payload FROM android_scoped_snapshots WHERE id=$1 AND run_id=$2",
        )
        .bind(id.to_string())
        .bind(run.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
        let m: Value = parse(&raw)?;
        for f in m["files"].as_array().ok_or(Error::Conflict)? {
            for c in f["chunks"].as_array().ok_or(Error::Conflict)? {
                let digest = text(c, "digest")?;
                let raw:String=sqlx::query_scalar("SELECT bytes_base64 FROM android_scoped_snapshot_chunks WHERE manifest_id=$1 AND digest=$2").bind(id.to_string()).bind(&digest).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Conflict)?;
                let bytes = STANDARD.decode(raw).map_err(storage_error)?;
                if hash(&bytes) != digest
                    || c["plaintext_size"].as_u64() != Some(bytes.len() as u64)
                {
                    return Err(Error::Conflict);
                }
            }
        }
        sqlx::query("UPDATE android_scoped_snapshots SET committed=1 WHERE id=$1 AND run_id=$2")
            .bind(id.to_string())
            .bind(run.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(m)
    }
    pub async fn android_agent_get_snapshot(
        &self,
        a: &AndroidAgentPrincipal,
        run: Uuid,
        token: Uuid,
        input: bool,
    ) -> Result<Option<Value>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let r = device_lease_current(&mut tx, a, run, token, current).await?;
        let raw: Option<String> = if input {
            let v: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
            let id = v["spec"]["snapshot_id"].as_str().ok_or(Error::NotFound)?;
            sqlx::query_scalar("SELECT s.payload FROM android_scoped_snapshots s JOIN android_scoped_runs r ON r.id=s.run_id WHERE s.id=$1 AND r.project_id=$2 AND s.committed=1").bind(id).bind(r.try_get::<String,_>("project_id").map_err(storage_error)?).fetch_optional(&mut *tx).await.map_err(storage_error)?
        } else {
            sqlx::query_scalar("SELECT payload FROM android_scoped_snapshots WHERE run_id=$1 AND committed=1 ORDER BY id DESC LIMIT 1").bind(run.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?
        };
        let v = raw.map(|r| parse(&r)).transpose()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn android_agent_get_chunk(
        &self,
        a: &AndroidAgentPrincipal,
        run: Uuid,
        token: Uuid,
        digest: &str,
    ) -> Result<Vec<u8>> {
        chunk_digest(digest)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let r = device_lease_current(&mut tx, a, run, token, current).await?;
        let v: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
        let id = v["spec"]["snapshot_id"].as_str().ok_or(Error::NotFound)?;
        let raw:String=sqlx::query_scalar("SELECT c.bytes_base64 FROM android_scoped_snapshot_chunks c JOIN android_scoped_snapshots s ON s.id=c.manifest_id JOIN android_scoped_runs r ON r.id=s.run_id WHERE c.manifest_id=$1 AND c.digest=$2 AND s.committed=1 AND r.project_id=$3").bind(id).bind(digest).bind(r.try_get::<String,_>("project_id").map_err(storage_error)?).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        let bytes = STANDARD.decode(raw).map_err(storage_error)?;
        if hash(&bytes) != digest {
            return Err(Error::Conflict);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(bytes)
    }
}
