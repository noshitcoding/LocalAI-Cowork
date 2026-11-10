//! Once-only original object copy admission. Unknown storage IO cannot mint another send permit.
use super::*;
use serde::{Deserialize, Serialize};
pub(super) const MAX_PLAN: usize = 16 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct CopyFile {
    pub path: String,
    pub source_id: Uuid,
    pub size: u64,
    pub sha256: String,
    pub media_type: String,
    pub destination_id: Uuid,
    pub blob: Option<TeamBlob>,
    pub target: Option<TeamBlobVersion>,
    pub expected_revision: Option<i64>,
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct CopyPlan {
    pub format: String,
    pub id: Uuid,
    pub owner: Uuid,
    pub key: String,
    pub kind: String,
    pub source_project: Uuid,
    pub source_version: Uuid,
    pub source_content_sha256: String,
    pub source_project_revision: i64,
    pub target_project: Uuid,
    pub target_team: Option<Uuid>,
    pub target_name: String,
    pub target_revision: Option<i64>,
    pub target_current_version: Option<Uuid>,
    pub snapshot: Uuid,
    pub mappings: Vec<AndroidForkObjectMapping>,
    pub files: Vec<CopyFile>,
    pub request_sha256: String,
    pub created_at: i64,
    #[serde(default)]
    pub target_content_sha256: Option<String>,
    #[serde(default)]
    pub merge_base: Option<Uuid>,
}
pub struct AndroidProjectCopy {
    plan: CopyPlan,
    state: String,
    receipt: Option<Value>,
}
impl AndroidProjectCopy {
    pub(super) fn from_parts(plan: CopyPlan, state: String, receipt: Option<Value>) -> Self {
        Self {
            plan,
            state,
            receipt,
        }
    }
    pub fn id(&self) -> Uuid {
        self.plan.id
    }
    pub fn state(&self) -> &str {
        &self.state
    }
    pub fn original_receipt(&self) -> Option<&Value> {
        self.receipt.as_ref()
    }
    pub fn target_is_team(&self) -> bool {
        self.plan.target_team.is_some()
    }
    pub fn target_project(&self) -> Uuid {
        self.plan.target_project
    }
    pub fn files(&self) -> Vec<(String, TeamBlobVersion)> {
        self.plan
            .files
            .iter()
            .filter_map(|f| {
                f.target
                    .as_ref()
                    .map(|target| (f.path.clone(), target.clone()))
            })
            .collect()
    }
}
pub struct AndroidProjectCopyPermit {
    plan: Uuid,
    path: String,
    nonce: Uuid,
    dispatch_sha256: String,
    target: TeamBlobVersion,
}
pub struct AndroidProjectCopyReadWitness {
    plan: Uuid,
    path: String,
    nonce: String,
    dispatch_sha256: String,
    target: TeamBlobVersion,
    observed: u64,
    hash: Sha256,
}
impl AndroidProjectCopyReadWitness {
    pub fn target(&self) -> &TeamBlobVersion {
        &self.target
    }
    pub fn observe(&mut self, bytes: &[u8]) -> Result<()> {
        if bytes.is_empty() || bytes.len() > content::PAGE {
            return Err(Error::Conflict);
        }
        self.observed = self
            .observed
            .checked_add(bytes.len() as u64)
            .ok_or(Error::BudgetExceeded)?;
        if self.observed > self.target.size {
            return Err(Error::Conflict);
        }
        self.hash.update(bytes);
        Ok(())
    }
}
impl AndroidProjectCopyPermit {
    pub fn target(&self) -> &TeamBlobVersion {
        &self.target
    }
}
pub(super) async fn load(
    tx: &mut Transaction<'_, Any>,
    id: Uuid,
) -> Result<(CopyPlan, String, Option<Value>)> {
    let r=sqlx::query("SELECT owner_id,source_project_id,source_version_id,target_project_id,request_sha256,payload,payload_sha256,state,receipt,receipt_sha256 FROM android_project_copy_plans WHERE id=$1").bind(id.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
    let bytes: String = r.try_get("payload").map_err(storage_error)?;
    if bytes.len() > MAX_PLAN
        || hash(bytes.as_bytes())
            != r.try_get::<String, _>("payload_sha256")
                .map_err(storage_error)?
    {
        return Err(Error::Conflict);
    }
    let p: CopyPlan = parse(&bytes)?;
    if p.format != "android-project-copy-plan/1"
        || p.id != id
        || p.owner != row_id(&r, "owner_id")?
        || p.source_project != row_id(&r, "source_project_id")?
        || p.source_version != row_id(&r, "source_version_id")?
        || p.target_project != row_id(&r, "target_project_id")?
        || p.request_sha256
            != r.try_get::<String, _>("request_sha256")
                .map_err(storage_error)?
    {
        return Err(Error::Conflict);
    }
    let receipt = if let Some(encoded) = r
        .try_get::<Option<String>, _>("receipt")
        .map_err(storage_error)?
    {
        if encoded.len() > 3 * 1024 * 1024
            || r.try_get::<Option<String>, _>("receipt_sha256")
                .map_err(storage_error)?
                .as_deref()
                != Some(hash(encoded.as_bytes()).as_str())
        {
            return Err(Error::Conflict);
        }
        Some(parse(&encoded)?)
    } else {
        None
    };
    Ok((p, r.try_get("state").map_err(storage_error)?, receipt))
}
pub(super) async fn rights(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    p: &CopyPlan,
    write: bool,
) -> Result<()> {
    if p.owner != actor.identity_id() {
        return Err(Error::Forbidden);
    }
    scope(store, tx, actor, p.source_project, ContentAction::Read).await?;
    let (_, original, sha) =
        version_custody(tx, p.source_project, p.source_version, p.kind == "merge").await?;
    if p.kind == "merge" && original.owner != p.owner {
        return Err(Error::Forbidden);
    }
    if sha != p.source_content_sha256 {
        return Err(Error::Conflict);
    }
    let actual:i64 = sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM tenant_projects WHERE id=$1 UNION ALL SELECT COUNT(*) FROM android_personal_projects WHERE id=$1").bind(p.target_project.to_string()).fetch_all(&mut **tx).await.map_err(storage_error)?.into_iter().sum();
    if let Some(team) = p.target_team {
        if p.kind == "fork" && p.target_revision.is_none() && actual == 0 {
            let permit = store
                .identity()
                .authorize_content_in_transaction(
                    tx,
                    actor,
                    team,
                    p.target_project,
                    if write {
                        ContentAction::Write
                    } else {
                        ContentAction::Read
                    },
                )
                .await
                .map_err(identity_error)?;
            store
                .identity()
                .validate_content_in_transaction(tx, &permit)
                .await
                .map_err(identity_error)?;
        } else {
            scope(
                store,
                tx,
                actor,
                p.target_project,
                if write {
                    ContentAction::Write
                } else {
                    ContentAction::Read
                },
            )
            .await?;
        }
    } else if p.kind == "fork" && p.target_revision.is_none() && actual == 0 {
        store
            .identity()
            .require_global_session_in_transaction(tx, actor)
            .await
            .map_err(identity_error)?;
        store
            .identity()
            .require_session_action_in_transaction(
                tx,
                actor,
                if write {
                    ContentAction::Write
                } else {
                    ContentAction::Read
                },
            )
            .await
            .map_err(identity_error)?;
    } else {
        scope(
            store,
            tx,
            actor,
            p.target_project,
            if write {
                ContentAction::Write
            } else {
                ContentAction::Read
            },
        )
        .await?;
    }
    Ok(())
}
impl Store {
    pub async fn android_project_copy_summary(
        &self,
        actor: &Principal,
        id: Uuid,
        after: Option<&str>,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        let (p, state, original) = load(&mut tx, id).await?;
        rights(self, &mut tx, actor, &p, false).await?;
        if after.is_some_and(|a| !p.files.iter().any(|f| f.path == a)) {
            return Err(Error::Conflict);
        }
        let mut ordered: Vec<_> = p.files.iter().collect();
        ordered.sort_by(|a, b| a.path.cmp(&b.path));
        let selected: Vec<_> = ordered
            .into_iter()
            .filter(|f| after.is_none_or(|a| f.path.as_str() > a))
            .take(51)
            .collect();
        let mut files = Vec::new();
        for file in selected.iter().take(50) {
            let effect: Option<String> = sqlx::query_scalar(
                "SELECT state FROM android_project_copy_effects WHERE plan_id=$1 AND path=$2",
            )
            .bind(id.to_string())
            .bind(&file.path)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?;
            files.push(json!({"path":file.path,"size":file.size,"sha256":file.sha256,"state":effect.unwrap_or_else(||"sql_content_only".into())}));
        }
        let next = if selected.len() > 50 {
            selected.get(49).map(|f| f.path.clone())
        } else {
            None
        };
        let unknown:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_project_copy_effects WHERE plan_id=$1 AND state IN('dispatched','uncertain')").bind(id.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let state = if !matches!(state.as_str(), "committed" | "retired") && unknown > 0 {
            "uncertain"
        } else {
            state.as_str()
        };
        let receipt_sha256 = original
            .as_ref()
            .map(|value| json_string(value).map(|bytes| hash(bytes.as_bytes())))
            .transpose()?;
        let mut value = json!({"id":id,"state":state,"kind":p.kind,"original_operation_key":p.key,"original_request_sha256":p.request_sha256,"source_project_id":p.source_project,"source_version_id":p.source_version,"target_project_id":p.target_project,"original_receipt":original,"original_receipt_sha256":receipt_sha256,"files":files,"next_cursor":next,"file_count":p.files.len(),"unknown_files":unknown});
        // Large original acknowledgements stay available through the unchanged original
        // POST. Summary metadata and a file page must remain within the public limit.
        if json_string(&value)?.len() > 3 * 1024 * 1024 {
            value["original_receipt"] = Value::Null;
        }
        if json_string(&value)?.len() > 3 * 1024 * 1024 {
            return Err(Error::BudgetExceeded);
        }
        rights(self, &mut tx, actor, &p, false).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn android_project_copy_original(
        &self,
        actor: &Principal,
        project: Uuid,
        key: &str,
    ) -> Result<Uuid> {
        operation_key(key)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        scope(self, &mut tx, actor, project, ContentAction::Read).await?;
        let id:String=sqlx::query_scalar("SELECT id FROM android_project_copy_plans WHERE owner_id=$1 AND operation_key=$2 AND source_project_id=$3").bind(actor.identity_id().to_string()).bind(key).bind(project.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        let id = Uuid::parse_str(&id).map_err(storage_error)?;
        let (p, _, _) = load(&mut tx, id).await?;
        rights(self, &mut tx, actor, &p, false).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(id)
    }
    pub async fn android_project_copy_retire(&self, actor: &Principal, id: Uuid) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let (p, state, original) = load(&mut tx, id).await?;
        rights(self, &mut tx, actor, &p, true).await?;
        if state == "retired" {
            let v = original.ok_or(Error::Conflict)?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(v);
        }
        if state == "committed" {
            return Err(Error::Conflict);
        }
        let unknown:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_project_copy_effects WHERE plan_id=$1 AND state NOT IN('prepared','received')").bind(id.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if unknown != 0 {
            return Err(Error::UncertainEffect);
        }
        let value = json!({"format":"android-project-copy-retirement/1","plan_id":id,"operation_key":p.key,"original_request_sha256":p.request_sha256,"source_project_id":p.source_project,"source_version_id":p.source_version,"target_project_id":p.target_project,"effect_disposition":"only_original_unsent_or_received","external_objects_deleted":false,"execution_adopted":false,"closed_at":device_iso(current)?});
        let bytes = json_string(&value)?;
        if sqlx::query("UPDATE android_project_copy_plans SET state='retired',receipt=$1,receipt_sha256=$2 WHERE id=$3 AND state IN('prepared','receiving','uncertain') AND receipt IS NULL AND payload_sha256=$4").bind(&bytes).bind(hash(bytes.as_bytes())).bind(id.to_string()).bind(hash(json_string(&p)?.as_bytes())).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict)}
        rights(self, &mut tx, actor, &p, true).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn android_project_copy_read_witness(
        &self,
        actor: &Principal,
        id: Uuid,
        path: &str,
    ) -> Result<AndroidProjectCopyReadWitness> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        let (p, state, _) = load(&mut tx, id).await?;
        if matches!(state.as_str(), "retired" | "committed") {
            return Err(Error::Conflict);
        }
        rights(self, &mut tx, actor, &p, true).await?;
        let file = p
            .files
            .iter()
            .find(|f| f.path == path)
            .ok_or(Error::NotFound)?;
        let target = file.target.clone().ok_or(Error::Forbidden)?;
        let row=sqlx::query("SELECT nonce,dispatch_sha256,state,target FROM android_project_copy_effects WHERE plan_id=$1 AND path=$2").bind(id.to_string()).bind(path).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        if !matches!(
            row.try_get::<String, _>("state")
                .map_err(storage_error)?
                .as_str(),
            "dispatched" | "uncertain" | "received"
        ) || row.try_get::<String, _>("target").map_err(storage_error)? != json_string(&target)?
        {
            return Err(Error::Conflict);
        }
        let nonce: String = row
            .try_get::<Option<String>, _>("nonce")
            .map_err(storage_error)?
            .ok_or(Error::Conflict)?;
        let dispatch_sha256: String = row
            .try_get::<Option<String>, _>("dispatch_sha256")
            .map_err(storage_error)?
            .ok_or(Error::Conflict)?;
        rights(self, &mut tx, actor, &p, true).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(AndroidProjectCopyReadWitness {
            plan: id,
            path: path.into(),
            nonce,
            dispatch_sha256,
            target,
            observed: 0,
            hash: Sha256::new(),
        })
    }
    /// Called only after the actual operator-bound object stream reached EOF.
    pub async fn android_project_copy_confirm_read(
        &self,
        actor: &Principal,
        witness: AndroidProjectCopyReadWitness,
    ) -> Result<()> {
        if witness.observed != witness.target.size
            || format!("{:x}", witness.hash.finalize()) != witness.target.sha256
        {
            return Err(Error::Conflict);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let (p, _, _) = load(&mut tx, witness.plan).await?;
        rights(self, &mut tx, actor, &p, true).await?;
        let value = json!({"format":"project-version-object-read/1","plan_id":witness.plan,"path":witness.path,"nonce":witness.nonce,"dispatch_sha256":witness.dispatch_sha256,"backend_id":witness.target.backend_id,"object_key":witness.target.object_key,"size":witness.target.size,"sha256":witness.target.sha256,"original_only":true,"eof_observed":true,"confirmed_at":device_iso(current)?});
        let encoded = json_string(&value)?;
        // Keep the independent original PUT reply if it already exists. Read confirmation is
        // append-only evidence, and also safely settles a lost sender ACK without another PUT.
        let inserted=sqlx::query("INSERT INTO android_project_copy_receipts(plan_id,path,kind,receipt,receipt_sha256) VALUES($1,$2,'original_read',$3,$4) ON CONFLICT(plan_id,path,kind) DO NOTHING").bind(witness.plan.to_string()).bind(&witness.path).bind(&encoded).bind(hash(encoded.as_bytes())).execute(&mut *tx).await.map_err(storage_error)?.rows_affected();
        if inserted == 0 {
            let row=sqlx::query("SELECT receipt,receipt_sha256 FROM android_project_copy_receipts WHERE plan_id=$1 AND path=$2 AND kind='original_read'").bind(witness.plan.to_string()).bind(&witness.path).fetch_one(&mut *tx).await.map_err(storage_error)?;
            let existing: String = row.try_get("receipt").map_err(storage_error)?;
            if existing.len() > 64 * 1024
                || hash(existing.as_bytes())
                    != row
                        .try_get::<String, _>("receipt_sha256")
                        .map_err(storage_error)?
            {
                return Err(Error::Conflict);
            }
            let v: Value = parse(&existing)?;
            for key in [
                "plan_id",
                "path",
                "nonce",
                "dispatch_sha256",
                "backend_id",
                "object_key",
                "size",
                "sha256",
            ] {
                if v[key] != value[key] {
                    return Err(Error::Conflict);
                }
            }
        }
        let changed=sqlx::query("UPDATE android_project_copy_effects SET state='received',receipt=COALESCE(receipt,$1),receipt_sha256=COALESCE(receipt_sha256,$2) WHERE plan_id=$3 AND path=$4 AND state IN('dispatched','uncertain','received') AND nonce=$5 AND dispatch_sha256=$6 AND target=$7").bind(&encoded).bind(hash(encoded.as_bytes())).bind(witness.plan.to_string()).bind(witness.path).bind(witness.nonce).bind(witness.dispatch_sha256).bind(json_string(&witness.target)?).execute(&mut *tx).await.map_err(storage_error)?.rows_affected();
        if changed != 1 {
            return Err(Error::Conflict);
        }
        rights(self, &mut tx, actor, &p, true).await?;
        tx.commit().await.map_err(storage_error)
    }
    pub async fn android_project_fork_prepare(
        &self,
        actor: &Principal,
        source_project: Uuid,
        input: AndroidForkProject,
        backend_id: &str,
        active_key: KeyId,
    ) -> Result<AndroidProjectCopy> {
        operation_key(&input.idempotency_key)?;
        if backend_id.is_empty() || backend_id.len() > 200 {
            return Err(device_invalid());
        }
        let request =
            hash(json_string(&("android-project-fork/1", source_project, &input))?.as_bytes());
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        scope(self, &mut tx, actor, source_project, ContentAction::Read).await?;
        if let Some(r)=sqlx::query("SELECT id,request_sha256 FROM android_project_copy_plans WHERE owner_id=$1 AND operation_key=$2").bind(actor.identity_id().to_string()).bind(&input.idempotency_key).fetch_optional(&mut *tx).await.map_err(storage_error)?{
            if r.try_get::<String,_>("request_sha256").map_err(storage_error)?!=request{return Err(Error::Conflict)}let (p,state,receipt)=load(&mut tx,row_id(&r,"id")?).await?;rights(self,&mut tx,actor,&p,false).await?;tx.commit().await.map_err(storage_error)?;return Ok(AndroidProjectCopy{plan:p,state,receipt})
        }
        crate::server_keys::server_admission(&mut tx, active_key).await?;
        let source = scope(self, &mut tx, actor, source_project, ContentAction::Read).await?;
        if source.revision != input.expected_source_project_revision {
            return Err(Error::Conflict);
        }
        let (_, _, content_sha) = version(&mut tx, source_project, input.source_version_id).await?;
        let (target_team, name) = match input.destination {
            AndroidForkDestination::PrivateLocal { name } => (None, name),
            AndroidForkDestination::TeamManaged { team_id, name } => (Some(team_id), name),
        };
        if name.trim().is_empty() || name.len() > 200 || name.chars().any(char::is_control) {
            return Err(device_invalid());
        }
        let target = if let Some(team) = target_team {
            let creation = format!("fork:{}", hash(input.idempotency_key.as_bytes()));
            let resource_key = format!("tenant_project.v1:{}", hash(creation.as_bytes()));
            self.identity()
                .create_resource_in_transaction(
                    &mut tx,
                    actor,
                    team,
                    cowork_identity::ContentKind::Project,
                    &resource_key,
                )
                .await
                .map_err(identity_error)?
                .id
        } else {
            self.identity()
                .require_global_session_in_transaction(&mut tx, actor)
                .await
                .map_err(identity_error)?;
            self.identity()
                .require_session_action_in_transaction(&mut tx, actor, ContentAction::Write)
                .await
                .map_err(identity_error)?;
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
            Uuid::new_v4()
        };
        let id = Uuid::new_v4();
        let snapshot = Uuid::new_v4();
        let mut mappings = Vec::new();
        let objects=sqlx::query("SELECT kind,source_id,payload_sha256 FROM android_project_version_objects WHERE version_id=$1 AND kind NOT LIKE 'origin:%' ORDER BY kind,source_id LIMIT 10001").bind(input.source_version_id.to_string()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        if objects.len() > 10000 {
            return Err(Error::BudgetExceeded);
        }
        for r in objects {
            let kind: String = r.try_get("kind").map_err(storage_error)?;
            let source_id = row_id(&r, "source_id")?;
            mappings.push(AndroidForkObjectMapping {
                destination_id: if kind == "project" {
                    target
                } else {
                    Uuid::new_v4()
                },
                kind,
                source_id,
                original_sha256: r.try_get("payload_sha256").map_err(storage_error)?,
            });
        }
        let rows=sqlx::query("SELECT path,source_id,size,sha256,media_type FROM android_project_version_files WHERE version_id=$1 ORDER BY path LIMIT 2001").bind(input.source_version_id.to_string()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        if rows.len() > 2000 {
            return Err(Error::BudgetExceeded);
        }
        let mut files = Vec::new();
        let mut total = 0u64;
        for r in rows {
            let path: String = r.try_get("path").map_err(storage_error)?;
            let size = u64::try_from(r.try_get::<i64, _>("size").map_err(storage_error)?)
                .map_err(|_| Error::Conflict)?;
            if size > TEAM_BLOB_MAX_BYTES {
                return Err(Error::BudgetExceeded);
            }
            total = total.checked_add(size).ok_or(Error::BudgetExceeded)?;
            if total > content::MAX_CONTENT {
                return Err(Error::BudgetExceeded);
            }
            let sha256: String = r.try_get("sha256").map_err(storage_error)?;
            let source_id = row_id(&r, "source_id")?;
            let blob_id = if target_team.is_some() {
                Uuid::new_v4()
            } else {
                let sha = hash(json_string(&(snapshot, &path))?.as_bytes());
                Uuid::parse_str(&sha[..32]).map_err(storage_error)?
            };
            mappings.push(AndroidForkObjectMapping {
                kind: "file".into(),
                source_id,
                destination_id: blob_id,
                original_sha256: sha256.clone(),
            });
            let media_type: String = r.try_get("media_type").map_err(storage_error)?;
            let blob = target_team.map(|team| TeamBlob {
                id: blob_id,
                team_id: team,
                project_id: target,
                name: path.clone(),
                media_type: media_type.clone(),
                size,
                sha256: sha256.clone(),
                version: 1,
                revision: 1,
                state: TeamBlobState::Uploading,
                created_at: current,
                updated_at: current,
            });
            let target_descriptor = target_team.map(|team| TeamBlobVersion {
                blob_id,
                version: 1,
                size,
                sha256: sha256.clone(),
                object_key: format!("cowork/v1/{team}/{target}/{blob_id}/1/copy-{id}/{sha256}"),
                backend_id: backend_id.into(),
                state: TeamBlobState::Uploading,
                created_at: current,
            });
            files.push(CopyFile {
                path,
                source_id,
                size,
                sha256,
                media_type,
                destination_id: blob_id,
                blob,
                target: target_descriptor,
                expected_revision: None,
            });
        }
        if let Some(team) = target_team {
            let existing: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM team_blobs WHERE team_id=$1 AND project_id=$2",
            )
            .bind(team.to_string())
            .bind(target.to_string())
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
            if existing
                .checked_add(files.len() as i64)
                .is_none_or(|n| n > 2000)
            {
                return Err(Error::BudgetExceeded);
            }
        }
        let p = CopyPlan {
            format: "android-project-copy-plan/1".into(),
            id,
            owner: actor.identity_id(),
            key: input.idempotency_key,
            kind: "fork".into(),
            source_project,
            source_version: input.source_version_id,
            source_content_sha256: content_sha,
            source_project_revision: source.revision,
            target_project: target,
            target_team,
            target_name: name,
            target_revision: None,
            target_current_version: None,
            snapshot,
            mappings,
            files,
            request_sha256: request,
            created_at: current,
            target_content_sha256: None,
            merge_base: None,
        };
        let payload = json_string(&p)?;
        if payload.len() > MAX_PLAN {
            return Err(Error::BudgetExceeded);
        }
        sqlx::query("INSERT INTO android_project_copy_plans(id,owner_id,operation_key,source_project_id,source_version_id,target_project_id,request_sha256,payload,payload_sha256,state,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,'prepared',$10)").bind(id.to_string()).bind(actor.identity_id().to_string()).bind(&p.key).bind(source_project.to_string()).bind(p.source_version.to_string()).bind(target.to_string()).bind(&p.request_sha256).bind(&payload).bind(hash(payload.as_bytes())).bind(current).execute(&mut *tx).await.map_err(storage_error)?;
        if p.target_team.is_some() {
            for f in &p.files {
                let fingerprint = hash(
                    json_string(&(
                        id,
                        &p.request_sha256,
                        &f.path,
                        f.source_id,
                        &f.sha256,
                        &f.target,
                    ))?
                    .as_bytes(),
                );
                sqlx::query("INSERT INTO android_project_copy_effects(plan_id,path,original_sha256,target,request_sha256,state) VALUES($1,$2,$3,$4,$5,'prepared')").bind(id.to_string()).bind(&f.path).bind(&f.sha256).bind(json_string(&f.target)?).bind(fingerprint).execute(&mut *tx).await.map_err(storage_error)?;
            }
        }
        rights(self, &mut tx, actor, &p, true).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(AndroidProjectCopy {
            plan: p,
            state: "prepared".into(),
            receipt: None,
        })
    }
    pub async fn android_project_copy(
        &self,
        actor: &Principal,
        id: Uuid,
    ) -> Result<AndroidProjectCopy> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        let (p, state, receipt) = load(&mut tx, id).await?;
        rights(self, &mut tx, actor, &p, false).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(AndroidProjectCopy {
            plan: p,
            state,
            receipt,
        })
    }
    pub async fn android_project_copy_admit(
        &self,
        actor: &Principal,
        id: Uuid,
        path: &str,
        active_key: KeyId,
    ) -> Result<Option<AndroidProjectCopyPermit>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        crate::server_keys::server_admission(&mut tx, active_key).await?;
        device_writer(&mut tx).await?;
        let (p, state, _) = load(&mut tx, id).await?;
        rights(self, &mut tx, actor, &p, true).await?;
        if !matches!(state.as_str(), "prepared" | "receiving" | "uncertain") {
            return Err(Error::Conflict);
        }
        let file = p
            .files
            .iter()
            .find(|f| f.path == path)
            .ok_or(Error::NotFound)?;
        let target = file.target.as_ref().ok_or(Error::Forbidden)?;
        let r=sqlx::query("SELECT state,target,request_sha256 FROM android_project_copy_effects WHERE plan_id=$1 AND path=$2").bind(id.to_string()).bind(path).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        if r.try_get::<String, _>("target").map_err(storage_error)? != json_string(target)? {
            return Err(Error::Conflict);
        }
        let effect = r.try_get::<String, _>("state").map_err(storage_error)?;
        if effect == "received" {
            rights(self, &mut tx, actor, &p, true).await?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(None);
        }
        if effect != "prepared" {
            return Err(Error::UncertainEffect);
        }
        let nonce = Uuid::new_v4();
        let dispatch = hash(
            json_string(&(
                "android-project-copy-dispatch/1",
                id,
                path,
                nonce,
                r.try_get::<String, _>("request_sha256")
                    .map_err(storage_error)?,
                target,
            ))?
            .as_bytes(),
        );
        let changed=sqlx::query("UPDATE android_project_copy_effects SET state='dispatched',nonce=$1,dispatch_sha256=$2 WHERE plan_id=$3 AND path=$4 AND state='prepared' AND nonce IS NULL AND dispatch_sha256 IS NULL").bind(nonce.to_string()).bind(&dispatch).bind(id.to_string()).bind(path).execute(&mut *tx).await.map_err(storage_error)?.rows_affected();
        if changed != 1 {
            return Err(Error::Conflict);
        }
        rights(self, &mut tx, actor, &p, true).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(Some(AndroidProjectCopyPermit {
            plan: id,
            path: path.into(),
            nonce,
            dispatch_sha256: dispatch,
            target: target.clone(),
        }))
    }
    /// Persists the actual sender's original reply even if actor rights changed after IO.
    pub async fn android_project_copy_received(
        &self,
        permit: AndroidProjectCopyPermit,
        actual: &Value,
    ) -> Result<()> {
        if actual["format"] != "project-version-object-put/1"
            || actual["backend_id"] != permit.target.backend_id
            || actual["object_key"] != permit.target.object_key
            || actual["sha256"] != permit.target.sha256
            || actual["size"] != permit.target.size
        {
            return Err(Error::Conflict);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        let encoded = json_string(actual)?;
        sqlx::query("INSERT INTO android_project_copy_receipts(plan_id,path,kind,receipt,receipt_sha256) VALUES($1,$2,'original_put',$3,$4)").bind(permit.plan.to_string()).bind(&permit.path).bind(&encoded).bind(hash(encoded.as_bytes())).execute(&mut *tx).await.map_err(storage_error)?;
        let changed=sqlx::query("UPDATE android_project_copy_effects SET state='received',receipt=COALESCE(receipt,$1),receipt_sha256=COALESCE(receipt_sha256,$2) WHERE plan_id=$3 AND path=$4 AND state IN('dispatched','uncertain','received') AND nonce=$5 AND dispatch_sha256=$6 AND target=$7").bind(&encoded).bind(hash(encoded.as_bytes())).bind(permit.plan.to_string()).bind(permit.path).bind(permit.nonce.to_string()).bind(permit.dispatch_sha256).bind(json_string(&permit.target)?).execute(&mut *tx).await.map_err(storage_error)?.rows_affected();
        if changed != 1 {
            return Err(Error::Conflict);
        }
        tx.commit().await.map_err(storage_error)
    }
    /// One original authenticated page; SourceRead and TargetWrite are fresh on every SQL wait.
    pub async fn android_project_copy_page(
        &self,
        actor: &Principal,
        id: Uuid,
        path: &str,
        ordinal: u32,
        keys: &dyn KeyStore,
    ) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let (p, state, _) = load(&mut tx, id).await?;
        if matches!(state.as_str(), "committed" | "retired") {
            return Err(Error::Conflict);
        }
        rights(self, &mut tx, actor, &p, true).await?;
        let f = p
            .files
            .iter()
            .find(|f| f.path == path)
            .ok_or(Error::NotFound)?;
        if ordinal as u64 >= f.size.div_ceil(content::PAGE as u64) {
            rights(self, &mut tx, actor, &p, true).await?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(None);
        }
        let (_, s, _) = version_custody(
            &mut tx,
            p.source_project,
            p.source_version,
            p.kind == "merge",
        )
        .await?;
        let page = content::read_page(
            &mut tx,
            keys,
            &s,
            &format!("file:{}", f.source_id),
            ordinal as i64,
        )
        .await?;
        rights(self, &mut tx, actor, &p, true).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(Some(page))
    }
}
