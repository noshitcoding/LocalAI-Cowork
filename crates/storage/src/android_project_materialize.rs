//! Atomic content publication after original object IO; no source execution authority is copied.
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use zeroize::Zeroizing;

fn mapped(p: &copy::CopyPlan, kind: &str, id: Uuid) -> Result<Uuid> {
    p.mappings
        .iter()
        .find(|m| m.kind == kind && m.source_id == id)
        .map(|m| m.destination_id)
        .ok_or(Error::Conflict)
}
fn references(value: &mut Value, p: &copy::CopyPlan) -> Result<()> {
    match value {
        Value::Object(o) => {
            for (name, v) in o {
                let kind = match name.as_str() {
                    "project_id" => Some("project"),
                    "thread_id" | "order_id" | "forked_from_thread_id" => Some("thread"),
                    "message_id" | "forked_from_message_id" => Some("message"),
                    "task_id" => Some("task"),
                    "crew_id" => Some("crew"),
                    "skill_id" | "skill_ids" | "parent_skill_id" => Some("skill"),
                    "memory_id" | "memory_ids" => Some("memory"),
                    "file_id" | "file_ids" => Some("file"),
                    "model_profile_id" | "provider_profile_id" => Some("model_configuration"),
                    "mcp_metadata_ids" => Some("mcp_metadata"),
                    _ => None,
                };
                if let Some(kind) = kind {
                    map_ids(v, p, kind)?;
                } else {
                    references(v, p)?;
                }
            }
        }
        Value::Array(a) => {
            for v in a {
                references(v, p)?;
            }
        }
        _ => {}
    }
    Ok(())
}
fn map_ids(value: &mut Value, p: &copy::CopyPlan, kind: &str) -> Result<()> {
    match value {
        Value::String(s) => {
            if let Ok(id) = Uuid::parse_str(s) {
                if kind == "project" && id == p.source_project {
                    *s = p.target_project.to_string();
                } else if let Some(m) = p
                    .mappings
                    .iter()
                    .find(|m| m.kind == kind && m.source_id == id)
                {
                    *s = m.destination_id.to_string();
                }
                // An external/inert reference remains provenance and cannot mint a binding.
            }
        }
        Value::Array(a) => {
            for v in a {
                map_ids(v, p, kind)?;
            }
        }
        _ => {}
    }
    Ok(())
}
fn content_value(
    original: &Value,
    p: &copy::CopyPlan,
    m: &AndroidForkObjectMapping,
    current: i64,
) -> Result<Value> {
    let mut v = original.clone();
    if !v.is_object() {
        return Err(Error::Conflict);
    }
    references(&mut v, p)?;
    v["owner_user_id"] = if p.target_team.is_none() {
        json!(p.owner)
    } else {
        Value::Null
    };
    v["team_id"] = json!(p.target_team);
    if v.get("has_secret").is_some() {
        v["has_secret"] = json!(false)
    }
    if v.get("credential_configured").is_some() {
        v["credential_configured"] = json!(false)
    }
    v["schema_version"] = json!(2);
    v["id"] = json!(m.destination_id);
    v["project_id"] = json!(p.target_project);
    v["revision"] = json!(1);
    v["unconfigured"] = json!(true);
    v["etag"] = json!(format!("{}:1", m.destination_id));
    v["created_at"] = json!(device_iso(current)?);
    v["updated_at"] = json!(device_iso(current)?);
    v["deleted_at"] = Value::Null;
    // Identity/run/lease/approval bindings are deliberately absent from the destination.
    for field in [
        "run_id",
        "created_by_run_id",
        "author_user_id",
        "lease_token",
        "lease_id",
        "approval_id",
        "provider_binding",
        "model_binding",
        "private_runner_authority",
        "private_workflow",
        "released_version_id",
        "published_version_id",
        "executor_id",
        "preferred_executor_target",
    ] {
        if v.get(field).is_some() {
            v[field] = Value::Null;
        }
    }
    if let Some(config) = v.get_mut("config").and_then(Value::as_object_mut) {
        for field in [
            "private_workflow",
            "provider_binding",
            "model_binding",
            "private_runner_authority",
            "workflow_grant",
            "schedule_grant",
        ] {
            config.remove(field);
        }
    }
    v["copied_content_provenance"] = json!({"format":"android-project-content-origin/1","source_project_id":p.source_project,"source_version_id":p.source_version,"source_id":m.source_id,"source_kind":m.kind,"original_sha256":m.original_sha256,"execution_authority_copied":false});
    Ok(v)
}

pub(super) async fn objects(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    p: &copy::CopyPlan,
    keys: &dyn KeyStore,
    key: KeyId,
    current: i64,
) -> Result<()> {
    let (_, source, _) =
        version_custody(tx, p.source_project, p.source_version, p.kind == "merge").await?;
    // Parents precede children regardless of catalogue lexical ordering.
    for kind in [
        "thread",
        "document",
        "model_configuration",
        "crew",
        "skill",
        "memory",
        "mcp_metadata",
        "workflow_registry",
        "task",
        "message",
    ] {
        for m in p.mappings.iter().filter(|m| m.kind == kind) {
            let original = content::read_object(tx, keys, &source, kind, m.source_id).await?;
            if hash(json_string(&original)?.as_bytes()) != m.original_sha256 {
                return Err(Error::Conflict);
            }
            let mut v = content_value(&original, p, m, current)?;
            match kind {
                "thread" => {
                    if let Some(team) = p.target_team {
                        let title = original["title"]
                            .as_str()
                            .filter(|s| !s.trim().is_empty() && s.len() <= 200)
                            .ok_or(Error::Conflict)?;
                        let order = TeamOrder {
                            id: m.destination_id,
                            team_id: team,
                            project_id: p.target_project,
                            revision: 1,
                            title: title.into(),
                            prompt: original["prompt"].as_str().unwrap_or_default().into(),
                            archived: false,
                            created_at: current,
                        };
                        let encoded = json_string(&order)?;
                        sqlx::query("INSERT INTO team_orders(id,team_id,project_id,creator_id,creation_key,request_sha,initial_receipt,payload) VALUES($1,$2,$3,$4,$5,$6,$7,$7)").bind(order.id.to_string()).bind(team.to_string()).bind(p.target_project.to_string()).bind(actor.identity_id().to_string()).bind(format!("copy:{}:{}",p.id,m.source_id)).bind(hash(encoded.as_bytes())).bind(encoded).execute(&mut **tx).await.map_err(storage_error)?;
                        crate::team_service::project_sync_record(
                            tx,
                            team,
                            p.target_project,
                            "order",
                            order.id,
                            &serde_json::to_value(&order).map_err(storage_error)?,
                            false,
                        )
                        .await?;
                    } else {
                        v["owner_user_id"] = json!(actor.identity_id());
                        sqlx::query("INSERT INTO android_personal_threads(id,project_id,owner_id,revision,payload,deleted,creation_key,request_sha256) VALUES($1,$2,$3,1,$4,0,$5,$6)").bind(m.destination_id.to_string()).bind(p.target_project.to_string()).bind(actor.identity_id().to_string()).bind(json_string(&v)?).bind(format!("copy:{}:{}",p.id,m.source_id)).bind(m.original_sha256.clone()).execute(&mut **tx).await.map_err(storage_error)?;
                    }
                }
                "message" => {
                    let source_thread = crate::android_personal::wire_uuid(&original, "thread_id")?;
                    let thread = mapped(p, "thread", source_thread)?;
                    v["thread_id"] = json!(thread);
                    for field in ["created_at", "updated_at"] {
                        if original[field].as_str().is_some() {
                            v[field] = original[field].clone();
                        }
                    }
                    v["run_id"] = Value::Null;
                    v["author_user_id"] = Value::Null;
                    v["inert_history"] = json!(true);
                    if p.target_team.is_none() {
                        sqlx::query("INSERT INTO android_personal_messages(id,owner_id,project_id,thread_id,revision,payload,deleted) VALUES($1,$2,$3,$4,1,$5,0)").bind(m.destination_id.to_string()).bind(actor.identity_id().to_string()).bind(p.target_project.to_string()).bind(thread.to_string()).bind(json_string(&v)?).execute(&mut **tx).await.map_err(storage_error)?;
                    }
                    sqlx::query("INSERT INTO android_project_thread_history(id,project_id,thread_id,team_id,creator_id,payload,source_id,original_sha256) VALUES($1,$2,$3,$4,$5,$6,$7,$8)").bind(m.destination_id.to_string()).bind(p.target_project.to_string()).bind(thread.to_string()).bind(p.target_team.map(|x|x.to_string())).bind(actor.identity_id().to_string()).bind(json_string(&v)?).bind(m.source_id.to_string()).bind(&m.original_sha256).execute(&mut **tx).await.map_err(storage_error)?;
                }
                "document" if p.target_team.is_some() => {
                    let team = p.target_team.ok_or(Error::Conflict)?;
                    let name = original["name"].as_str().ok_or(Error::Conflict)?;
                    let text = original["content"].as_str().ok_or(Error::Conflict)?;
                    if name.len() > 200 || text.len() > 128 * 1024 {
                        return Err(Error::BudgetExceeded);
                    }
                    let provenance = KnowledgeProvenance {
                        format: 1,
                        origin: KnowledgeOrigin::Imported,
                        sources: vec![KnowledgeSource {
                            format: 1,
                            source_uri: Some(format!(
                                "urn:cowork:project-version:{}/{}",
                                p.source_version, m.source_id
                            )),
                            file_id: None,
                            artifact_id: None,
                            revision: "1".into(),
                            sha256: m.original_sha256.clone(),
                            locator: "original document content".into(),
                            verification: KnowledgeSourceVerification::Declared,
                        }],
                    };
                    let document = TeamDocument {
                        provenance,
                        id: m.destination_id,
                        team_id: team,
                        project_id: p.target_project,
                        kind: serde_json::from_value(original["kind"].clone())
                            .map_err(storage_error)?,
                        name: name.into(),
                        content: text.into(),
                        revision: 1,
                        archived: false,
                    };
                    let body = serde_json::to_value(&document).map_err(storage_error)?;
                    sqlx::query("INSERT INTO team_documents(id,team_id,project_id,payload) VALUES($1,$2,$3,$4)").bind(m.destination_id.to_string()).bind(team.to_string()).bind(p.target_project.to_string()).bind(json_string(&document)?).execute(&mut **tx).await.map_err(storage_error)?;
                    crate::knowledge_provenance::record(
                        tx,
                        &crate::knowledge_provenance::scope(
                            &p.target_project.to_string(),
                            Some(team),
                        ),
                        &m.destination_id.to_string(),
                        1,
                        &document.content,
                        &document.provenance,
                        &document,
                    )
                    .await?;
                    crate::team_service::project_sync_record(
                        tx,
                        team,
                        p.target_project,
                        "document",
                        document.id,
                        &body,
                        false,
                    )
                    .await?;
                    v["provenance"] = body["provenance"].clone();
                }
                "task" => {
                    v["released"] = json!(false);
                    v["released_at"] = Value::Null;
                    v["unconfigured"] = json!(true);
                    sqlx::query("INSERT INTO android_task_versions(id,revision,project_id,payload,released,deleted) VALUES($1,1,$2,$3,0,0)").bind(m.destination_id.to_string()).bind(p.target_project.to_string()).bind(json_string(&v)?).execute(&mut **tx).await.map_err(storage_error)?;
                }
                "workflow_registry" if p.target_team.is_none() => {
                    let raw = Zeroizing::new(
                        STANDARD
                            .decode(
                                original["original_base64"]
                                    .as_str()
                                    .ok_or(Error::Conflict)?,
                            )
                            .map_err(|_| Error::Conflict)?,
                    );
                    if hash(&raw) != original["raw_sha256"].as_str().ok_or(Error::Conflict)? {
                        return Err(Error::Conflict);
                    }
                    let engine = serde_json::from_value(original["engine"].clone())
                        .map_err(storage_error)?;
                    crate::android_private_workflows::import_draft_with_id_in_tx(
                        tx,
                        store,
                        actor,
                        p.target_project,
                        &raw,
                        &engine,
                        keys,
                        key,
                        current,
                        m.destination_id,
                    )
                    .await?;
                    v["unconfigured"] = json!(true);
                    v["published_version_id"] = Value::Null;
                    // The actual definition lives only in the new encrypted private registry.
                    // Replication exposes metadata, never a second plaintext DSL copy.
                    for field in [
                        "original_base64",
                        "definition",
                        "raw_sha256",
                        "canonical_sha256",
                    ] {
                        v.as_object_mut().ok_or(Error::Conflict)?.remove(field);
                    }
                }
                "model_configuration"
                | "crew"
                | "skill"
                | "memory"
                | "mcp_metadata"
                | "workflow_registry" => {
                    v["unconfigured"] = json!(true);
                    v["enabled"] = json!(false);
                    v["auto_run"] = json!(false);
                    v["published_version_id"] = Value::Null;
                }
                _ => {}
            }
            crate::android_wire_sync::record(
                tx,
                actor.identity_id(),
                kind,
                m.destination_id,
                p.target_team,
                Some(p.target_project),
                1,
                &v,
                false,
                current,
            )
            .await?;
        }
    }
    Ok(())
}

pub(super) async fn files(
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    p: &copy::CopyPlan,
    keys: &dyn KeyStore,
    key: KeyId,
    current: i64,
) -> Result<()> {
    let (_, source, _) =
        version_custody(tx, p.source_project, p.source_version, p.kind == "merge").await?;
    let dest = content::Scope {
        owner: actor.identity_id(),
        project: p.target_project,
        version: p.snapshot,
    };
    let mut manifest = json!({"schema_version":2,"id":p.snapshot,"project_id":p.target_project,"encryption_key_id":key.cipher_id().map_err(|_|Error::NativeKeyUnavailable)?,"total_bytes":0,"files":[],"created_at":device_iso(current)?,"expires_at":null});
    let mut total = 0u64;
    if let Some(team) = p.target_team {
        sqlx::query("INSERT INTO android_project_snapshots(id,project_id,team_id,creator_id,creation_key,request_sha256,payload,committed) VALUES($1,$2,$3,$4,$5,$6,$7,0)").bind(p.snapshot.to_string()).bind(p.target_project.to_string()).bind(team.to_string()).bind(actor.identity_id().to_string()).bind(format!("copy:{}",p.id)).bind(&p.request_sha256).bind(json_string(&manifest)?).execute(&mut **tx).await.map_err(storage_error)?;
    } else {
        sqlx::query("INSERT INTO android_private_snapshots(id,owner_id,project_id,creation_key,request_sha,payload,committed,deleted) VALUES($1,$2,$3,$4,$5,$6,0,0)").bind(p.snapshot.to_string()).bind(actor.identity_id().to_string()).bind(p.target_project.to_string()).bind(format!("copy:{}",p.id)).bind(&p.request_sha256).bind(json_string(&manifest)?).execute(&mut **tx).await.map_err(storage_error)?;
    }
    for f in &p.files {
        crate::android_private_snapshots::relative(&f.path)?;
        total = total.checked_add(f.size).ok_or(Error::BudgetExceeded)?;
        if f.size > TEAM_BLOB_MAX_BYTES || total > content::MAX_CONTENT {
            return Err(Error::BudgetExceeded);
        }
        let mut hash_file = Sha256::new();
        let mut observed = 0u64;
        let mut chunks = Vec::new();
        for ordinal in 0..f.size.div_ceil(content::PAGE as u64) {
            let bytes = content::read_page(
                tx,
                keys,
                &source,
                &format!("file:{}", f.source_id),
                ordinal as i64,
            )
            .await?;
            observed = observed
                .checked_add(bytes.len() as u64)
                .ok_or(Error::BudgetExceeded)?;
            hash_file.update(bytes.as_slice());
            let digest = hash(&bytes);
            if p.target_team.is_some() {
                let sealed =
                    content::seal(keys, key, &dest, &format!("source:{digest}"), 0, &bytes)?;
                sqlx::query("INSERT INTO android_project_snapshot_chunks(snapshot_id,digest,ordinal,page_digest,size,sealed) VALUES($1,$2,0,$2,$3,$4) ON CONFLICT(snapshot_id,digest,ordinal) DO NOTHING").bind(p.snapshot.to_string()).bind(&digest).bind(bytes.len()as i64).bind(json_string(&sealed)?).execute(&mut **tx).await.map_err(storage_error)?;
            } else {
                let sealed = crate::android_private_snapshots::seal(
                    keys,
                    key,
                    actor.identity_id(),
                    p.target_project,
                    p.snapshot,
                    &digest,
                    &bytes,
                )?;
                sqlx::query("INSERT INTO android_private_snapshot_chunks(snapshot_id,digest,size,sealed) VALUES($1,$2,$3,$4) ON CONFLICT(snapshot_id,digest) DO NOTHING").bind(p.snapshot.to_string()).bind(&digest).bind(bytes.len()as i64).bind(json_string(&sealed)?).execute(&mut **tx).await.map_err(storage_error)?;
            }
            chunks.push(json!({"digest":digest,"plaintext_size":bytes.len(),"ciphertext_size":bytes.len()+16}));
        }
        if observed != f.size || format!("{:x}", hash_file.finalize()) != f.sha256 {
            return Err(Error::Conflict);
        }
        manifest["files"]
            .as_array_mut()
            .ok_or(Error::Conflict)?
            .push(
                json!({"path":f.path,"size":f.size,"digest":f.sha256,"mode":420,"chunks":chunks}),
            );
        if let Some(team) = p.target_team {
            let effect=sqlx::query("SELECT state,target,original_sha256,receipt,receipt_sha256 FROM android_project_copy_effects WHERE plan_id=$1 AND path=$2").bind(p.id.to_string()).bind(&f.path).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Conflict)?;
            let target = f.target.as_ref().ok_or(Error::Conflict)?;
            let encoded: String = effect
                .try_get::<Option<String>, _>("receipt")
                .map_err(storage_error)?
                .ok_or(Error::UncertainEffect)?;
            if effect
                .try_get::<String, _>("state")
                .map_err(storage_error)?
                != "received"
                || effect
                    .try_get::<String, _>("target")
                    .map_err(storage_error)?
                    != json_string(target)?
                || effect
                    .try_get::<String, _>("original_sha256")
                    .map_err(storage_error)?
                    != f.sha256
                || effect
                    .try_get::<Option<String>, _>("receipt_sha256")
                    .map_err(storage_error)?
                    .as_deref()
                    != Some(hash(encoded.as_bytes()).as_str())
            {
                return Err(Error::UncertainEffect);
            }
            let actual: Value = parse(&encoded)?;
            if !matches!(
                actual["format"].as_str(),
                Some("project-version-object-put/1" | "project-version-object-read/1")
            ) || actual["backend_id"] != target.backend_id
                || actual["object_key"] != target.object_key
                || actual["sha256"] != target.sha256
                || actual["size"] != target.size
            {
                return Err(Error::Conflict);
            }
            let mut blob = f.blob.clone().ok_or(Error::Conflict)?;
            blob.state = TeamBlobState::Ready;
            let mut actual_version = target.clone();
            actual_version.state = TeamBlobState::Ready;
            sqlx::query("INSERT INTO team_blobs(id,team_id,project_id,revision,payload) VALUES($1,$2,$3,$4,$5)").bind(blob.id.to_string()).bind(team.to_string()).bind(p.target_project.to_string()).bind(blob.revision).bind(json_string(&blob)?).execute(&mut **tx).await.map_err(storage_error)?;
            sqlx::query("INSERT INTO team_blob_versions(blob_id,version,payload) VALUES($1,$2,$3)")
                .bind(blob.id.to_string())
                .bind(blob.version)
                .bind(json_string(&actual_version)?)
                .execute(&mut **tx)
                .await
                .map_err(storage_error)?;
        } else {
            let payload = json!({"schema_version":2,"id":f.destination_id,"project_id":p.target_project,"snapshot_id":p.snapshot,"name":f.path,"path":f.path,"size":f.size,"size_bytes":f.size,"sha256":f.sha256,"revision":1,"deleted_at":null});
            sqlx::query("INSERT INTO android_private_files(id,owner_id,project_id,snapshot_id,path,payload) VALUES($1,$2,$3,$4,$5,$6)").bind(f.destination_id.to_string()).bind(actor.identity_id().to_string()).bind(p.target_project.to_string()).bind(p.snapshot.to_string()).bind(&f.path).bind(json_string(&payload)?).execute(&mut **tx).await.map_err(storage_error)?;
        }
    }
    manifest["total_bytes"] = json!(total);
    let query = if p.target_team.is_some() {
        "UPDATE android_project_snapshots SET committed=1,payload=$1 WHERE id=$2 AND committed=0"
    } else {
        "UPDATE android_private_snapshots SET committed=1,payload=$1 WHERE id=$2 AND committed=0"
    };
    if sqlx::query(query)
        .bind(json_string(&manifest)?)
        .bind(p.snapshot.to_string())
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?
        .rows_affected()
        != 1
    {
        return Err(Error::Conflict);
    }
    Ok(())
}

impl Store {
    pub async fn android_project_fork_commit(
        &self,
        actor: &Principal,
        id: Uuid,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<AndroidProjectForkReceipt> {
        let key = crate::android_private_workspace::writer_cipher(keys, key)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        crate::server_keys::cipher_writer(&mut tx, key).await?;
        let current = device_writer(&mut tx).await?;
        let (p, state, old) = copy::load(&mut tx, id).await?;
        if p.kind != "fork" {
            return Err(Error::Conflict);
        }
        copy::rights(self, &mut tx, actor, &p, false).await?;
        if let Some(old) = old {
            if state != "committed" {
                return Err(Error::Conflict);
            }
            let v = serde_json::from_value(old).map_err(storage_error)?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(v);
        }
        if state == "retired" {
            return Err(Error::Conflict);
        }
        copy::rights(self, &mut tx, actor, &p, true).await?;
        let src = scope(self, &mut tx, actor, p.source_project, ContentAction::Read).await?;
        if src.revision != p.source_project_revision {
            return Err(Error::Conflict);
        }
        let (_, source, _) = version(&mut tx, p.source_project, p.source_version).await?;
        let original =
            content::read_object(&mut tx, keys, &source, "project", p.source_project).await?;
        if let Some(team) = p.target_team {
            let actual = self
                .create_tenant_project_in_transaction(
                    &mut tx,
                    actor,
                    team,
                    CreateTenantProject {
                        name: p.target_name.clone(),
                        idempotency_key: format!("fork:{}", hash(p.key.as_bytes())),
                    },
                )
                .await?;
            if actual.id != p.target_project {
                return Err(Error::Conflict);
            }
            sqlx::query("INSERT INTO team_project_profiles(project_id,revision,name,archived) VALUES($1,1,$2,0)").bind(p.target_project.to_string()).bind(&p.target_name).execute(&mut *tx).await.map_err(storage_error)?;
        } else {
            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM android_personal_projects WHERE owner_id=$1 AND deleted=0",
            )
            .bind(actor.identity_id().to_string())
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
            if count >= 2000 {
                return Err(Error::BudgetExceeded);
            }
            let value = json!({"schema_version":2,"id":p.target_project,"revision":1,"etag":format!("{}:1",p.target_project),"owner_user_id":actor.identity_id(),"team_id":null,"privacy":"private_local","name":p.target_name,"description":original["description"].as_str().unwrap_or_default(),"preferred_executor_target":null,"current_version_id":null,"policy":original["policy"],"created_at":device_iso(current)?,"updated_at":device_iso(current)?,"deleted_at":null,"forked_from_project_id":p.source_project,"forked_from_version_id":p.source_version});
            sqlx::query("INSERT INTO android_personal_projects(id,owner_id,revision,payload,deleted) VALUES($1,$2,1,$3,0)").bind(p.target_project.to_string()).bind(actor.identity_id().to_string()).bind(json_string(&value)?).execute(&mut *tx).await.map_err(storage_error)?;
        }
        scope(self, &mut tx, actor, p.target_project, ContentAction::Write).await?;
        objects(self, &mut tx, actor, &p, keys, key, current).await?;
        files(&mut tx, actor, &p, keys, key, current).await?;
        let destination_version_id = Uuid::new_v4();
        let version_value = AndroidProjectVersion {
            schema_version: 2,
            id: destination_version_id,
            project_id: p.target_project,
            revision: 1,
            parent_version_id: None,
            merge_base_version_id: None,
            snapshot_manifest_id: p.snapshot,
            created_by_run_id: None,
            created_at: device_iso(current)?,
        };
        let version_encoded = json_string(&version_value)?;
        sqlx::query("INSERT INTO android_project_versions(id,project_id,team_id,creator_id,revision,payload,payload_sha256,content_sha256,created_at) VALUES($1,$2,$3,$4,1,$5,$6,$7,$8)").bind(destination_version_id.to_string()).bind(p.target_project.to_string()).bind(p.target_team.map(|v|v.to_string())).bind(actor.identity_id().to_string()).bind(&version_encoded).bind(hash(version_encoded.as_bytes())).bind(hash(b"")).bind(current).execute(&mut *tx).await.map_err(storage_error)?;
        sqlx::query("INSERT INTO android_project_version_heads(project_id,team_id,current_version_id,active_snapshot_id,revision) VALUES($1,$2,$3,$4,1)").bind(p.target_project.to_string()).bind(p.target_team.map(|v|v.to_string())).bind(destination_version_id.to_string()).bind(p.snapshot.to_string()).execute(&mut *tx).await.map_err(storage_error)?;
        if p.target_team.is_none() {
            let mut project = scope(self, &mut tx, actor, p.target_project, ContentAction::Write)
                .await?
                .payload;
            project["current_version_id"] = json!(destination_version_id);
            if sqlx::query("UPDATE android_personal_projects SET payload=$1 WHERE id=$2 AND owner_id=$3 AND revision=1 AND deleted=0").bind(json_string(&project)?).bind(p.target_project.to_string()).bind(actor.identity_id().to_string()).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict)}
        }
        let value = AndroidProjectForkReceipt {
            format: "android-project-fork-receipt/1".into(),
            idempotency_key: p.key.clone(),
            source_project_id: p.source_project,
            source_version_id: p.source_version,
            destination_project_id: p.target_project,
            destination_version_id: Some(destination_version_id),
            destination_team_id: p.target_team,
            mappings: p.mappings.clone(),
            original_content_sha256: p.source_content_sha256.clone(),
            execution_authority_copied: false,
            committed_at: device_iso(current)?,
        };
        let encoded = json_string(&value)?;
        if encoded.len() > 3 * 1024 * 1024 {
            return Err(Error::BudgetExceeded);
        }
        if sqlx::query("UPDATE android_project_copy_plans SET state='committed',receipt=$1,receipt_sha256=$2 WHERE id=$3 AND state IN('prepared','receiving','uncertain') AND receipt IS NULL AND payload_sha256=$4").bind(&encoded).bind(hash(encoded.as_bytes())).bind(id.to_string()).bind(hash(json_string(&p)?.as_bytes())).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict)}
        copy::rights(self, &mut tx, actor, &p, true).await?;
        let actual = scope(self, &mut tx, actor, p.target_project, ContentAction::Write).await?;
        crate::android_wire_sync::record(
            &mut tx,
            actor.identity_id(),
            "project",
            p.target_project,
            p.target_team,
            Some(p.target_project),
            1,
            &actual.payload,
            false,
            current,
        )
        .await?;
        receipt(
            &mut tx,
            actor.identity_id(),
            p.source_project,
            &p.key,
            "fork",
            &p.request_sha256,
            &serde_json::to_value(&value).map_err(storage_error)?,
        )
        .await?;
        // Archive the destination's actual independent current content in the same original
        // commit, so source deletion/revocation cannot strand its version/configuration history.
        let project = scope(self, &mut tx, actor, p.target_project, ContentAction::Write).await?;
        let mut capture = AndroidVersionCapture {
            tx,
            actor: actor.clone(),
            project,
            scope: content::Scope {
                owner: actor.identity_id(),
                project: p.target_project,
                version: destination_version_id,
            },
            value: version_value,
            key: format!("fork-version:{}", p.id),
            request_sha: hash(
                json_string(&(
                    "android-project-fork-version/1",
                    p.id,
                    p.target_project,
                    p.snapshot,
                ))?
                .as_bytes(),
            ),
            source_snapshot: p.snapshot,
            proofs: Vec::new(),
            object_bytes: 0,
            file_bytes: 0,
            files: BTreeSet::new(),
            file_progress: BTreeMap::new(),
            source_snapshot_sha: None,
            references: BTreeMap::new(),
        };
        capture.capture_objects(self, keys, key).await?;
        let (_, original_scope, _) =
            version(&mut capture.tx, p.source_project, p.source_version).await?;
        for mapping in p.mappings.iter().filter(|m| m.kind != "file") {
            let raw = content::read_object(
                &mut capture.tx,
                keys,
                &original_scope,
                &mapping.kind,
                mapping.source_id,
            )
            .await?;
            if hash(json_string(&raw)?.as_bytes()) != mapping.original_sha256 {
                return Err(Error::Conflict);
            }
            let kind = format!("origin:{}", mapping.kind);
            // Historical source bytes use separate original-content objects, not the projected
            // editable-content budget. No plaintext page, history or individual object cap rises.
            content::write_object(
                &mut capture.tx,
                keys,
                key,
                &capture.scope,
                &kind,
                mapping.source_id,
                &raw,
            )
            .await?;
        }
        let mut history_after = (String::new(), String::new());
        loop {
            let row=sqlx::query("SELECT kind,source_id,payload_sha256 FROM android_project_version_objects WHERE version_id=$1 AND kind LIKE 'origin:%' AND (kind>$2 OR (kind=$2 AND source_id>$3)) ORDER BY kind,source_id LIMIT 1").bind(p.source_version.to_string()).bind(&history_after.0).bind(&history_after.1).fetch_optional(&mut *capture.tx).await.map_err(storage_error)?;
            let Some(row) = row else { break };
            let kind: String = row.try_get("kind").map_err(storage_error)?;
            let id = row_id(&row, "source_id")?;
            history_after = (kind.clone(), id.to_string());
            let raw =
                content::read_object(&mut capture.tx, keys, &original_scope, &kind, id).await?;
            if hash(json_string(&raw)?.as_bytes())
                != row
                    .try_get::<String, _>("payload_sha256")
                    .map_err(storage_error)?
            {
                return Err(Error::Conflict);
            }
            content::write_object(&mut capture.tx, keys, key, &capture.scope, &kind, id, &raw)
                .await?;
        }
        capture.capture_snapshot_files(self, keys, key).await?;
        capture.finish(self, keys).await?;
        Ok(value)
    }
}
