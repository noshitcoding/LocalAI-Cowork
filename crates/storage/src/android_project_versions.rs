//! Actor/team content version custody, with current authority and immutable original replies.
use super::*;
use crate::android_devices::{device_invalid, device_iso, device_writer};
use crate::team_service::identity_error;
use cowork_identity::{ContentAction, Principal};
use cowork_secrets::{KeyId, KeyStore};
use sha2::{Digest, Sha256};
use sqlx::{Any, Transaction, any::AnyRow};
use std::collections::{BTreeMap, BTreeSet};

#[path = "android_project_version_apply.rs"]
mod apply;
#[path = "android_project_version_content.rs"]
mod content;
#[path = "android_project_copy.rs"]
mod copy;
#[path = "android_project_forks.rs"]
mod forks;
#[path = "android_project_materialize.rs"]
mod materialize;
#[path = "android_project_template_content.rs"]
mod templates;
pub(crate) use apply::quiescence;
#[path = "android_project_version_merge.rs"]
mod merge;
#[path = "android_project_snapshots.rs"]
mod snapshots;
pub(crate) use content::{ciphertext_preflight, required_keys, rewrap_helper};
pub use copy::{AndroidProjectCopy, AndroidProjectCopyPermit, AndroidProjectCopyReadWitness};

pub(super) fn row_id(row: &AnyRow, name: &str) -> Result<Uuid> {
    Uuid::parse_str(&row.try_get::<String, _>(name).map_err(storage_error)?).map_err(storage_error)
}
pub(super) struct ProjectScope {
    pub team: Option<Uuid>,
    pub project: Uuid,
    pub revision: i64,
    pub payload: Value,
}
pub(super) async fn scope(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    project: Uuid,
    action: ContentAction,
) -> Result<ProjectScope> {
    store
        .identity()
        .validate_principal_in_transaction(tx, actor)
        .await
        .map_err(identity_error)?;
    if let Some(row) = sqlx::query("SELECT revision FROM android_personal_projects WHERE id=$1")
        .bind(project.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?
    {
        let value = store
            .android_personal_permit(tx, actor, project, action)
            .await?;
        let revision = row.try_get::<i64, _>("revision").map_err(storage_error)?;
        if value["id"] != json!(project) || value["revision"] != revision {
            return Err(Error::Conflict);
        }
        return Ok(ProjectScope {
            team: None,
            project,
            revision,
            payload: value,
        });
    }
    let row=sqlx::query("SELECT p.organization_id,p.payload,COALESCE(f.revision,1) AS revision,COALESCE(f.archived,0) AS archived,f.name AS profile_name FROM tenant_projects p LEFT JOIN team_project_profiles f ON f.project_id=p.id WHERE p.id=$1").bind(project.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
    let team = row_id(&row, "organization_id")?;
    let permit = store
        .identity()
        .authorize_content_in_transaction(tx, actor, team, project, action)
        .await
        .map_err(identity_error)?;
    if matches!(action, ContentAction::Write | ContentAction::Execute)
        && row.try_get::<i64, _>("archived").map_err(storage_error)? != 0
    {
        return Err(Error::Conflict);
    }
    let revision = row.try_get::<i64, _>("revision").map_err(storage_error)?;
    let tenant: TenantProject =
        parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    if tenant.id != project || tenant.organization_id != team {
        return Err(Error::Conflict);
    }
    let current_version = apply::head(tx, project).await?;
    let value = json!({"schema_version":2,"id":project,"team_id":team,"owner_user_id":null,"privacy":"team_managed","revision":revision,"name":row.try_get::<Option<String>,_>("profile_name").map_err(storage_error)?.unwrap_or(tenant.name),"description":"","policy":{},"preferred_executor_target":null,"current_version_id":current_version,"created_at":device_iso(tenant.created_at)?,"deleted_at":null});
    store
        .identity()
        .validate_content_in_transaction(tx, &permit)
        .await
        .map_err(identity_error)?;
    Ok(ProjectScope {
        team: Some(team),
        project,
        revision,
        payload: value,
    })
}
pub(super) fn operation_key(key: &str) -> Result<()> {
    if key.is_empty() || key.len() > 200 || key.chars().any(char::is_control) {
        Err(device_invalid())
    } else {
        Ok(())
    }
}
pub(super) async fn original(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    project: Uuid,
    key: &str,
    kind: &str,
    sha: &str,
) -> Result<Option<Value>> {
    operation_key(key)?;
    scope(store, tx, actor, project, ContentAction::Read).await?;
    let row=sqlx::query("SELECT project_id,kind,request_sha256,receipt,receipt_sha256 FROM android_project_version_operations WHERE owner_id=$1 AND operation_key=$2").bind(actor.identity_id().to_string()).bind(key).fetch_optional(&mut **tx).await.map_err(storage_error)?;
    let Some(row) = row else { return Ok(None) };
    if row
        .try_get::<String, _>("project_id")
        .map_err(storage_error)?
        != project.to_string()
        || row.try_get::<String, _>("kind").map_err(storage_error)? != kind
        || row
            .try_get::<String, _>("request_sha256")
            .map_err(storage_error)?
            != sha
    {
        return Err(Error::Conflict);
    }
    let bytes = row.try_get::<String, _>("receipt").map_err(storage_error)?;
    if bytes.len() > 3 * 1024 * 1024
        || hash(bytes.as_bytes())
            != row
                .try_get::<String, _>("receipt_sha256")
                .map_err(storage_error)?
    {
        return Err(Error::Conflict);
    }
    let value = parse(&bytes)?;
    scope(store, tx, actor, project, ContentAction::Read).await?;
    Ok(Some(value))
}
pub(super) async fn receipt(
    tx: &mut Transaction<'_, Any>,
    actor: Uuid,
    project: Uuid,
    key: &str,
    kind: &str,
    sha: &str,
    value: &Value,
) -> Result<()> {
    let encoded = json_string(value)?;
    if encoded.len() > 3 * 1024 * 1024 {
        return Err(Error::BudgetExceeded);
    }
    sqlx::query("INSERT INTO android_project_version_operations(owner_id,operation_key,project_id,kind,request_sha256,receipt,receipt_sha256) VALUES($1,$2,$3,$4,$5,$6,$7)").bind(actor.to_string()).bind(key).bind(project.to_string()).bind(kind).bind(sha).bind(&encoded).bind(hash(encoded.as_bytes())).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
async fn version(
    tx: &mut Transaction<'_, Any>,
    project: Uuid,
    id: Uuid,
) -> Result<(AndroidProjectVersion, content::Scope, String)> {
    version_custody(tx, project, id, false).await
}
async fn version_custody(
    tx: &mut Transaction<'_, Any>,
    project: Uuid,
    id: Uuid,
    staged: bool,
) -> Result<(AndroidProjectVersion, content::Scope, String)> {
    let row = sqlx::query("SELECT * FROM android_project_versions WHERE id=$1 AND project_id=$2")
        .bind(id.to_string())
        .bind(project.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
    if row.try_get::<i64, _>("ready").map_err(storage_error)? != 1 && !staged {
        return Err(Error::NotFound);
    }
    let encoded: String = row.try_get("payload").map_err(storage_error)?;
    if hash(encoded.as_bytes())
        != row
            .try_get::<String, _>("payload_sha256")
            .map_err(storage_error)?
    {
        return Err(Error::Conflict);
    }
    let value: AndroidProjectVersion = parse(&encoded)?;
    if value.id != id
        || value.project_id != project
        || value.revision != row.try_get::<i64, _>("revision").map_err(storage_error)?
    {
        return Err(Error::Conflict);
    }
    Ok((
        value,
        content::Scope {
            owner: row_id(&row, "creator_id")?,
            project,
            version: id,
        },
        row.try_get("content_sha256").map_err(storage_error)?,
    ))
}

/// A held original capture transaction. No caller JSON can create this writer.
pub struct AndroidVersionCapture {
    tx: Transaction<'static, Any>,
    actor: Principal,
    project: ProjectScope,
    scope: content::Scope,
    value: AndroidProjectVersion,
    key: String,
    request_sha: String,
    source_snapshot: Uuid,
    proofs: Vec<(String, Uuid, String)>,
    object_bytes: u64,
    file_bytes: u64,
    files: BTreeSet<String>,
    file_progress: BTreeMap<Uuid, (u32, u64, Sha256)>,
    source_snapshot_sha: Option<String>,
    references: BTreeMap<String, BTreeSet<Uuid>>,
}
pub enum AndroidVersionAdmission {
    Original(AndroidProjectVersion),
    Capture(Box<AndroidVersionCapture>),
}
impl Store {
    pub async fn android_project_version_state(
        &self,
        actor: &Principal,
        project: Uuid,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let p = scope(self, &mut tx, actor, project, ContentAction::Read).await?;
        let snapshot: Option<String> = sqlx::query_scalar(
            "SELECT active_snapshot_id FROM android_project_version_heads WHERE project_id=$1",
        )
        .bind(project.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?
        .flatten();
        let value = json!({"project":p.payload,"current_version_id":apply::head(&mut tx,project).await?,"active_snapshot_id":snapshot});
        scope(self, &mut tx, actor, project, ContentAction::Read).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn android_project_version_files(
        &self,
        actor: &Principal,
        project: Uuid,
        id: Uuid,
    ) -> Result<Vec<Value>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        scope(self, &mut tx, actor, project, ContentAction::Read).await?;
        version(&mut tx, project, id).await?;
        let rows=sqlx::query("SELECT path,source_id,size,sha256,media_type FROM android_project_version_files WHERE version_id=$1 ORDER BY path LIMIT 2001").bind(id.to_string()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        if rows.len() > 2000 {
            return Err(Error::BudgetExceeded);
        }
        let values=rows.iter().map(|r|Ok(json!({"id":row_id(r,"source_id")?,"path":r.try_get::<String,_>("path").map_err(storage_error)?,"size":r.try_get::<i64,_>("size").map_err(storage_error)?,"sha256":r.try_get::<String,_>("sha256").map_err(storage_error)?,"media_type":r.try_get::<String,_>("media_type").map_err(storage_error)?,"page_bytes":content::PAGE}))).collect::<Result<Vec<Value>>>()?;
        scope(self, &mut tx, actor, project, ContentAction::Read).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(values)
    }
    pub async fn android_project_version_file_page(
        &self,
        actor: &Principal,
        project: Uuid,
        id: Uuid,
        file: Uuid,
        ordinal: u32,
        keys: &dyn KeyStore,
    ) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        scope(self, &mut tx, actor, project, ContentAction::Read).await?;
        let (_, s, _) = version(&mut tx, project, id).await?;
        let size: i64 = sqlx::query_scalar(
            "SELECT size FROM android_project_version_files WHERE version_id=$1 AND source_id=$2",
        )
        .bind(id.to_string())
        .bind(file.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
        let size = u64::try_from(size).map_err(|_| Error::Forbidden)?;
        let page = if ordinal as u64 >= size.div_ceil(content::PAGE as u64) {
            None
        } else {
            Some(
                content::read_page(&mut tx, keys, &s, &format!("file:{file}"), ordinal as i64)
                    .await?,
            )
        };
        scope(self, &mut tx, actor, project, ContentAction::Read).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(page)
    }
    pub async fn android_project_versions(
        &self,
        actor: &Principal,
        project: Uuid,
    ) -> Result<Vec<AndroidProjectVersion>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        scope(self, &mut tx, actor, project, ContentAction::Read).await?;
        let rows=sqlx::query("SELECT id FROM android_project_versions WHERE project_id=$1 AND ready=1 ORDER BY revision DESC LIMIT 1001").bind(project.to_string()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        if rows.len() > 1000 {
            return Err(Error::BudgetExceeded);
        }
        let mut values = Vec::new();
        for r in rows {
            values.push(version(&mut tx, project, row_id(&r, "id")?).await?.0);
        }
        scope(self, &mut tx, actor, project, ContentAction::Read).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(values)
    }
    pub async fn android_project_version_begin(
        &self,
        actor: &Principal,
        project: Uuid,
        input: AndroidCreateProjectVersion,
        key: &str,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<AndroidVersionAdmission> {
        operation_key(key)?;
        if json_string(&input.diff_summary)?.len() > 128 * 1024 {
            return Err(device_invalid());
        }
        let key_id = crate::android_private_workspace::writer_cipher(keys, key_id)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        crate::server_keys::cipher_writer(&mut tx, key_id).await?;
        let current = device_writer(&mut tx).await?;
        let sha =
            hash(json_string(&("android-project-version-create/1", project, &input))?.as_bytes());
        if let Some(old) = original(self, &mut tx, actor, project, key, "create", &sha).await? {
            tx.commit().await.map_err(storage_error)?;
            return Ok(AndroidVersionAdmission::Original(
                serde_json::from_value(old).map_err(storage_error)?,
            ));
        }
        let project_scope = scope(self, &mut tx, actor, project, ContentAction::Write).await?;
        for parent in [input.parent_version_id, input.merge_base_version_id]
            .into_iter()
            .flatten()
        {
            version(&mut tx, project, parent).await?;
        }
        if let Some(run) = input.created_by_run_id {
            let count:i64=sqlx::query_scalar("SELECT (SELECT COUNT(*) FROM android_private_server_runs WHERE id=$1 AND project_id=$2 AND owner_id=$3 AND state='completed')+(SELECT COUNT(*) FROM android_scoped_runs WHERE id=$1 AND project_id=$2 AND state='completed')+(SELECT COUNT(*) FROM team_runs WHERE id=$1 AND project_id=$2 AND status='completed')").bind(run.to_string()).bind(project.to_string()).bind(actor.identity_id().to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
            if count != 1 {
                return Err(Error::Conflict);
            }
        }
        let revision: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(revision),0)+1 FROM android_project_versions WHERE project_id=$1",
        )
        .bind(project.to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        if revision > 1000 {
            return Err(Error::BudgetExceeded);
        }
        let id = Uuid::new_v4();
        let value = AndroidProjectVersion {
            schema_version: 2,
            id,
            project_id: project,
            revision,
            parent_version_id: input.parent_version_id,
            merge_base_version_id: input.merge_base_version_id,
            snapshot_manifest_id: input.snapshot_manifest_id,
            created_by_run_id: input.created_by_run_id,
            created_at: device_iso(current)?,
        };
        let encoded = json_string(&value)?;
        sqlx::query("INSERT INTO android_project_versions(id,project_id,team_id,creator_id,revision,payload,payload_sha256,content_sha256,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)").bind(id.to_string()).bind(project.to_string()).bind(project_scope.team.map(|v|v.to_string())).bind(actor.identity_id().to_string()).bind(revision).bind(&encoded).bind(hash(encoded.as_bytes())).bind(hash(b"" )).bind(current).execute(&mut *tx).await.map_err(storage_error)?;
        let mut capture = AndroidVersionCapture {
            tx,
            actor: actor.clone(),
            project: project_scope,
            scope: content::Scope {
                owner: actor.identity_id(),
                project,
                version: id,
            },
            value,
            key: key.into(),
            request_sha: sha,
            source_snapshot: input.snapshot_manifest_id,
            proofs: Vec::new(),
            object_bytes: 0,
            file_bytes: 0,
            files: BTreeSet::new(),
            file_progress: BTreeMap::new(),
            source_snapshot_sha: None,
            references: BTreeMap::new(),
        };
        capture.capture_objects(self, keys, key_id).await?;
        Ok(AndroidVersionAdmission::Capture(Box::new(capture)))
    }
    pub async fn android_project_version_object(
        &self,
        actor: &Principal,
        project: Uuid,
        id: Uuid,
        kind: &str,
        source: Uuid,
        keys: &dyn KeyStore,
    ) -> Result<AndroidProjectVersionContent> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        scope(self, &mut tx, actor, project, ContentAction::Read).await?;
        let (_, s, _) = version(&mut tx, project, id).await?;
        let payload = content::read_object(&mut tx, keys, &s, kind, source).await?;
        let original_sha256 = hash(
            serde_json::to_vec(&payload)
                .map_err(storage_error)?
                .as_slice(),
        );
        scope(self, &mut tx, actor, project, ContentAction::Read).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(AndroidProjectVersionContent {
            kind: kind.into(),
            source_id: source,
            payload,
            original_sha256,
        })
    }
}

impl AndroidVersionCapture {
    pub fn project_id(&self) -> Uuid {
        self.scope.project
    }
    pub fn source_snapshot(&self) -> Uuid {
        self.source_snapshot
    }
    pub fn team_id(&self) -> Option<Uuid> {
        self.project.team
    }
    async fn object(
        &mut self,
        keys: &dyn KeyStore,
        key: KeyId,
        kind: &str,
        id: Uuid,
        value: &Value,
    ) -> Result<()> {
        let encoded = json_string(value)?;
        if let Some((_, _, original)) = self
            .proofs
            .iter()
            .find(|(k, source, _)| k == kind && *source == id)
        {
            if original != &hash(encoded.as_bytes()) {
                return Err(Error::Conflict);
            }
            return Ok(());
        }
        collect_references(value, &mut self.references)?;
        self.object_bytes = self
            .object_bytes
            .checked_add(encoded.len() as u64)
            .ok_or(Error::BudgetExceeded)?;
        if self.object_bytes > content::MAX_CONTENT || self.proofs.len() >= 10000 {
            return Err(Error::BudgetExceeded);
        }
        content::write_object(&mut self.tx, keys, key, &self.scope, kind, id, value).await?;
        self.proofs
            .push((kind.into(), id, hash(encoded.as_bytes())));
        Ok(())
    }
    async fn capture_objects(
        &mut self,
        store: &Store,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<()> {
        self.object(
            keys,
            key,
            "project",
            self.scope.project,
            &self.project.payload.clone(),
        )
        .await?;
        // Current real canonical content is captured once per source ID, not a whole-project RAM dump.
        let tables: Vec<(&str, &str)> = if self.project.team.is_none() {
            vec![(
                "thread",
                "SELECT id,payload FROM android_personal_threads WHERE project_id=$1 AND deleted=0 AND id>$2 ORDER BY id LIMIT 1",
            )]
        } else {
            vec![
                (
                    "thread",
                    "SELECT id,payload FROM team_orders WHERE project_id=$1 AND id>$2 ORDER BY id LIMIT 1",
                ),
                (
                    "document",
                    "SELECT id,payload FROM team_documents WHERE project_id=$1 AND id>$2 ORDER BY id LIMIT 1",
                ),
            ]
        };
        for (kind, sql) in tables {
            let mut after = String::new();
            loop {
                let row = sqlx::query(sql)
                    .bind(self.scope.project.to_string())
                    .bind(&after)
                    .fetch_optional(&mut *self.tx)
                    .await
                    .map_err(storage_error)?;
                let Some(row) = row else { break };
                let id = row_id(&row, "id")?;
                after = id.to_string();
                let value: Value = if kind == "document" {
                    current_object(
                        store,
                        &mut self.tx,
                        &self.actor,
                        &self.project,
                        kind,
                        id,
                        keys,
                    )
                    .await?
                } else {
                    parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?
                };
                self.object(keys, key, kind, id, &value).await?;
                if kind == "thread" {
                    let messages =
                        forks::thread_messages(&mut self.tx, &self.actor, &self.project, id)
                            .await?;
                    for message in messages {
                        let message_id = crate::android_personal::wire_uuid(&message, "id")?;
                        self.object(keys, key, "message", message_id, &message)
                            .await?;
                    }
                }
            }
        }
        if self.project.team.is_none() {
            let mut after = None;
            loop {
                let (item, next) = crate::android_private_workflows::content_page(
                    &mut self.tx,
                    store,
                    &self.actor,
                    self.scope.project,
                    after,
                    keys,
                )
                .await?;
                let Some(item) = item else { break };
                let value = workflow_content(&item)?;
                self.object(keys, key, "workflow_registry", item.version_id, &value)
                    .await?;
                let Some(next) = next else { break };
                after = Some(next);
            }
        }
        let mut after = String::new();
        loop {
            let row=sqlx::query("SELECT id,payload FROM android_task_versions v WHERE project_id=$1 AND deleted=0 AND revision=(SELECT MAX(w.revision) FROM android_task_versions w WHERE w.id=v.id) AND id>$2 ORDER BY id LIMIT 1").bind(self.scope.project.to_string()).bind(&after).fetch_optional(&mut *self.tx).await.map_err(storage_error)?;
            let Some(row) = row else { break };
            let id = row_id(&row, "id")?;
            after = id.to_string();
            let value: Value = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
            self.object(keys, key, "task", id, &value).await?;
        }
        for kind in [
            "crew",
            "skill",
            "memory",
            "mcp_metadata",
            "document",
            "model_configuration",
        ] {
            let mut after = String::new();
            loop {
                let rows=sqlx::query("SELECT owner_id,entity_id,payload FROM android_wire_sync_current WHERE entity_type=$1 AND project_id=$2 AND tombstone=0 AND entity_id>$3 AND ((team_id IS NULL AND owner_id=$4) OR team_id=$5) ORDER BY entity_id LIMIT 2").bind(kind).bind(self.scope.project.to_string()).bind(&after).bind(self.actor.identity_id().to_string()).bind(self.project.team.map(|v|v.to_string())).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
                let Some(row) = rows.first() else { break };
                let id = row_id(row, "entity_id")?;
                if rows
                    .get(1)
                    .is_some_and(|next| row_id(next, "entity_id").ok() == Some(id))
                {
                    return Err(Error::Conflict);
                }
                after = id.to_string();
                if self
                    .proofs
                    .iter()
                    .any(|(k, source, _)| k == kind && *source == id)
                {
                    continue;
                }
                let entity: Value =
                    parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
                let value = entity.get("payload").cloned().ok_or(Error::Conflict)?;
                self.object(keys, key, kind, id, &value).await?;
            }
        }
        // Include referenced actor-global library definitions as content, without reading any
        // provider secret, MCP credential or external runner authorization table.
        loop {
            let mut pending = Vec::new();
            for (kind, ids) in &self.references {
                for id in ids {
                    if !self
                        .proofs
                        .iter()
                        .any(|(k, source, _)| k == kind && source == id)
                    {
                        pending.push((kind.clone(), *id));
                    }
                }
            }
            if pending.is_empty() {
                break;
            }
            for (kind, id) in pending {
                let value = current_object(
                    store,
                    &mut self.tx,
                    &self.actor,
                    &self.project,
                    &kind,
                    id,
                    keys,
                )
                .await?;
                self.object(keys, key, &kind, id, &value).await?;
            }
        }
        if let Some(team) = self.project.team
            && let Some(value) = team_model_configuration(&mut self.tx, team).await?
        {
            self.object(keys, key, "model_configuration", team, &value)
                .await?;
        }
        scope(
            store,
            &mut self.tx,
            &self.actor,
            self.scope.project,
            ContentAction::Read,
        )
        .await?;
        Ok(())
    }
    /// Stream one authenticated original file page; this does not expose raw bytes in metadata.
    pub async fn write_file_page(
        &mut self,
        keys: &dyn KeyStore,
        key: KeyId,
        file: Uuid,
        ordinal: u32,
        bytes: &[u8],
    ) -> Result<()> {
        let key = crate::android_private_workspace::writer_cipher(keys, key)?;
        let progress = self
            .file_progress
            .entry(file)
            .or_insert_with(|| (0, 0, Sha256::new()));
        if progress.0 != ordinal
            || (progress.1 > 0 && !progress.1.is_multiple_of(content::PAGE as u64))
            || bytes.is_empty()
            || bytes.len() > content::PAGE
        {
            return Err(Error::Conflict);
        }
        progress.1 = progress
            .1
            .checked_add(bytes.len() as u64)
            .ok_or(Error::BudgetExceeded)?;
        if progress.1 > TEAM_BLOB_MAX_BYTES {
            return Err(Error::BudgetExceeded);
        }
        progress.2.update(bytes);
        progress.0 = progress.0.checked_add(1).ok_or(Error::BudgetExceeded)?;
        content::write_page(
            &mut self.tx,
            keys,
            key,
            &self.scope,
            &format!("file:{file}"),
            i64::from(ordinal),
            bytes,
        )
        .await
    }
    pub async fn capture_snapshot_files(
        &mut self,
        store: &Store,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<()> {
        let current = device_writer(&mut self.tx).await?;
        scope(
            store,
            &mut self.tx,
            &self.actor,
            self.scope.project,
            ContentAction::Read,
        )
        .await?;
        let manifest = self.source_manifest(current).await?;
        self.source_snapshot_sha = Some(hash(json_string(&manifest)?.as_bytes()));
        snapshots::declared_chunks(&manifest)?;
        for original in manifest["files"].as_array().ok_or_else(device_invalid)? {
            let path = original["path"].as_str().ok_or_else(device_invalid)?;
            let identity = hash(json_string(&(self.source_snapshot, path))?.as_bytes());
            let file = Uuid::parse_str(&identity[..32]).map_err(storage_error)?;
            let mut ordinal = 0u32;
            let mut buffer = zeroize::Zeroizing::new(Vec::with_capacity(content::PAGE));
            let mut complete = Sha256::new();
            for part in original["chunks"].as_array().ok_or_else(device_invalid)? {
                let digest = part["digest"].as_str().ok_or_else(device_invalid)?;
                let size =
                    usize::try_from(part["plaintext_size"].as_u64().ok_or_else(device_invalid)?)
                        .map_err(|_| Error::BudgetExceeded)?;
                let bytes = if self.project.team.is_none() {
                    let encoded:String=sqlx::query_scalar("SELECT sealed FROM android_private_snapshot_chunks WHERE snapshot_id=$1 AND digest=$2").bind(self.source_snapshot.to_string()).bind(digest).fetch_one(&mut *self.tx).await.map_err(storage_error)?;
                    crate::android_private_snapshots::open(
                        keys,
                        self.actor.identity_id(),
                        self.scope.project,
                        self.source_snapshot,
                        digest,
                        size,
                        &parse(&encoded)?,
                    )?
                } else if let Some(row) = snapshots::row(&mut self.tx, self.source_snapshot).await?
                {
                    snapshots::read_chunk(
                        &mut self.tx,
                        keys,
                        &snapshots::snapshot_scope(&row)?,
                        digest,
                        size,
                    )
                    .await?
                } else {
                    use base64::Engine;
                    let encoded:String=sqlx::query_scalar("SELECT bytes_base64 FROM android_scoped_snapshot_chunks WHERE manifest_id=$1 AND digest=$2").bind(self.source_snapshot.to_string()).bind(digest).fetch_one(&mut *self.tx).await.map_err(storage_error)?;
                    let value = zeroize::Zeroizing::new(
                        base64::engine::general_purpose::STANDARD
                            .decode(encoded)
                            .map_err(|_| Error::Conflict)?,
                    );
                    if value.len() != size || hash(&value) != digest {
                        return Err(Error::Conflict);
                    }
                    value
                };
                complete.update(bytes.as_slice());
                let mut offset = 0;
                while offset < bytes.len() {
                    let n = (content::PAGE - buffer.len()).min(bytes.len() - offset);
                    buffer.extend_from_slice(&bytes[offset..offset + n]);
                    offset += n;
                    if buffer.len() == content::PAGE {
                        self.write_file_page(keys, key, file, ordinal, &buffer)
                            .await?;
                        ordinal = ordinal.checked_add(1).ok_or(Error::BudgetExceeded)?;
                        buffer.clear();
                    }
                }
            }
            if !buffer.is_empty() {
                self.write_file_page(keys, key, file, ordinal, &buffer)
                    .await?;
            }
            let digest = format!("{:x}", complete.finalize());
            if original
                .get("digest")
                .is_some_and(|v| v.as_str() != Some(digest.as_str()))
            {
                return Err(Error::Conflict);
            }
            self.finish_file(
                path,
                file,
                original["size"].as_u64().ok_or_else(device_invalid)?,
                &digest,
                "application/octet-stream",
                original,
            )
            .await?;
        }
        scope(
            store,
            &mut self.tx,
            &self.actor,
            self.scope.project,
            ContentAction::Read,
        )
        .await?;
        Ok(())
    }
    async fn source_manifest(&mut self, current: i64) -> Result<Value> {
        if self.project.team.is_none() {
            return crate::android_private_snapshots::manifest(
                &mut self.tx,
                self.actor.identity_id(),
                self.scope.project,
                self.source_snapshot,
                true,
                current,
            )
            .await;
        }
        if let Some(row) = snapshots::row(&mut self.tx, self.source_snapshot).await? {
            if row_id(&row, "project_id")? != self.scope.project
                || row_id(&row, "team_id")? != self.project.team.ok_or(Error::Forbidden)?
                || row.try_get::<i64, _>("committed").map_err(storage_error)? != 1
            {
                return Err(Error::Conflict);
            }
            return snapshots::manifest(&row, current);
        }
        let row=sqlx::query("SELECT s.payload FROM android_scoped_snapshots s JOIN android_scoped_runs r ON r.id=s.run_id WHERE s.id=$1 AND s.committed=1 AND r.project_id=$2 AND r.team_id=$3").bind(self.source_snapshot.to_string()).bind(self.scope.project.to_string()).bind(self.project.team.map(|v|v.to_string())).fetch_optional(&mut *self.tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        let value: Value = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        if value["id"] != json!(self.source_snapshot)
            || value["project_id"] != json!(self.scope.project)
        {
            return Err(Error::Conflict);
        }
        if value["expires_at"]
            .as_str()
            .map(|s| {
                chrono::DateTime::parse_from_rfc3339(s)
                    .map(|v| v.timestamp_millis())
                    .map_err(storage_error)
            })
            .transpose()?
            .is_some_and(|v| v <= current)
        {
            return Err(Error::Forbidden);
        }
        Ok(value)
    }
    pub async fn finish_file(
        &mut self,
        path: &str,
        file: Uuid,
        size: u64,
        sha256: &str,
        media_type: &str,
        original: &Value,
    ) -> Result<()> {
        crate::android_private_snapshots::relative(path)?;
        if !self.files.insert(path.to_lowercase())
            || self.files.len() > 2000
            || size > TEAM_BLOB_MAX_BYTES
            || sha256.len() != 64
            || !sha256.bytes().all(|b| b.is_ascii_hexdigit())
            || media_type.len() > 120
        {
            return Err(device_invalid());
        }
        self.file_bytes = self
            .file_bytes
            .checked_add(size)
            .ok_or(Error::BudgetExceeded)?;
        if self.file_bytes > content::MAX_CONTENT {
            return Err(Error::BudgetExceeded);
        }
        let progress = self
            .file_progress
            .remove(&file)
            .unwrap_or_else(|| (0, 0, Sha256::new()));
        if progress.1 != size || format!("{:x}", progress.2.finalize()) != sha256 {
            return Err(Error::Conflict);
        }
        let total:i64=sqlx::query_scalar("SELECT COALESCE(SUM(size),0) FROM android_project_version_parts WHERE version_id=$1 AND piece_key=$2").bind(self.scope.version.to_string()).bind(format!("file:{file}")).fetch_one(&mut *self.tx).await.map_err(storage_error)?;
        if total != size as i64 {
            return Err(Error::Conflict);
        }
        sqlx::query("INSERT INTO android_project_version_files(version_id,path,source_id,size,sha256,media_type,payload) VALUES($1,$2,$3,$4,$5,$6,$7)").bind(self.scope.version.to_string()).bind(path).bind(file.to_string()).bind(size as i64).bind(sha256).bind(media_type).bind(json_string(original)?).execute(&mut *self.tx).await.map_err(storage_error)?;
        Ok(())
    }
    pub async fn finish(
        mut self,
        store: &Store,
        keys: &dyn KeyStore,
    ) -> Result<AndroidProjectVersion> {
        if !self.file_progress.is_empty() {
            return Err(Error::Conflict);
        }
        let observed = device_writer(&mut self.tx).await?;
        let original_manifest = self.source_manifest(observed).await?;
        if self.source_snapshot_sha.as_deref()
            != Some(hash(json_string(&original_manifest)?.as_bytes()).as_str())
        {
            return Err(Error::Conflict);
        }
        let current = scope(
            store,
            &mut self.tx,
            &self.actor,
            self.scope.project,
            ContentAction::Write,
        )
        .await?;
        if current.team != self.project.team || current.revision != self.project.revision {
            return Err(Error::Conflict);
        }
        // Every archive page is independently authenticated; ordered original descriptors form the immutable catalogue digest.
        let mut h = Sha256::new();
        let mut message_cache: Option<(Uuid, Vec<Value>)> = None;
        for (kind, id, sha) in &self.proofs {
            let payload = content::read_object(&mut self.tx, keys, &self.scope, kind, *id).await?;
            if hash(json_string(&payload)?.as_bytes()) != *sha {
                return Err(Error::Conflict);
            }
            let current = if kind == "message" {
                let thread = crate::android_personal::wire_uuid(&payload, "thread_id")?;
                if message_cache
                    .as_ref()
                    .is_none_or(|(cached, _)| *cached != thread)
                {
                    message_cache = Some((
                        thread,
                        forks::thread_messages(&mut self.tx, &self.actor, &self.project, thread)
                            .await?,
                    ));
                }
                message_cache
                    .as_ref()
                    .and_then(|(_, items)| items.iter().find(|v| v["id"] == json!(id)))
                    .cloned()
                    .ok_or(Error::Conflict)?
            } else {
                current_object(
                    store,
                    &mut self.tx,
                    &self.actor,
                    &self.project,
                    kind,
                    *id,
                    keys,
                )
                .await?
            };
            if hash(json_string(&current)?.as_bytes()) != *sha {
                return Err(Error::Conflict);
            }
            h.update(json_string(&(kind, id, sha))?.as_bytes());
        }
        let mut origin_after = (String::new(), String::new());
        loop {
            let row=sqlx::query("SELECT kind,source_id,payload_sha256 FROM android_project_version_objects WHERE version_id=$1 AND kind LIKE 'origin:%' AND (kind>$2 OR (kind=$2 AND source_id>$3)) ORDER BY kind,source_id LIMIT 1").bind(self.scope.version.to_string()).bind(&origin_after.0).bind(&origin_after.1).fetch_optional(&mut *self.tx).await.map_err(storage_error)?;
            let Some(row) = row else { break };
            let kind: String = row.try_get("kind").map_err(storage_error)?;
            let id = row_id(&row, "source_id")?;
            let sha: String = row.try_get("payload_sha256").map_err(storage_error)?;
            origin_after = (kind.clone(), id.to_string());
            let payload = content::read_object(&mut self.tx, keys, &self.scope, &kind, id).await?;
            if hash(json_string(&payload)?.as_bytes()) != sha {
                return Err(Error::Conflict);
            }
            h.update(json_string(&(&kind, id, &sha))?.as_bytes());
        }
        let rows=sqlx::query("SELECT path,source_id,size,sha256 FROM android_project_version_files WHERE version_id=$1 ORDER BY path").bind(self.scope.version.to_string()).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        for row in rows {
            h.update(
                json_string(&(
                    row.try_get::<String, _>("path").map_err(storage_error)?,
                    row.try_get::<String, _>("source_id")
                        .map_err(storage_error)?,
                    row.try_get::<i64, _>("size").map_err(storage_error)?,
                    row.try_get::<String, _>("sha256").map_err(storage_error)?,
                ))?
                .as_bytes(),
            );
        }
        if sqlx::query("UPDATE android_project_versions SET content_sha256=$1 WHERE id=$2 AND content_sha256=$3").bind(format!("{:x}",h.finalize())).bind(self.scope.version.to_string()).bind(hash(b"")).execute(&mut *self.tx).await.map_err(storage_error)?.rows_affected() != 1 { return Err(Error::Conflict) }
        let value = serde_json::to_value(&self.value).map_err(storage_error)?;
        receipt(
            &mut self.tx,
            self.actor.identity_id(),
            self.scope.project,
            &self.key,
            "create",
            &self.request_sha,
            &value,
        )
        .await?;
        scope(
            store,
            &mut self.tx,
            &self.actor,
            self.scope.project,
            ContentAction::Write,
        )
        .await?;
        crate::android_wire_sync::record(
            &mut self.tx,
            self.actor.identity_id(),
            "project_version",
            self.value.id,
            self.project.team,
            Some(self.scope.project),
            self.value.revision,
            &value,
            false,
            observed,
        )
        .await?;
        self.tx.commit().await.map_err(storage_error)?;
        Ok(self.value)
    }
}

async fn current_object(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    project: &ProjectScope,
    kind: &str,
    id: Uuid,
    keys: &dyn KeyStore,
) -> Result<Value> {
    if kind == "document" && project.team.is_none() {
        return templates::read_optional(tx, actor, project, kind, id)
            .await?
            .map(|v| v.0)
            .ok_or(Error::Conflict);
    }
    if kind == "workflow_registry" {
        let prior = id.as_u128().checked_sub(1).map(Uuid::from_u128);
        let (item, _) = crate::android_private_workflows::content_page(
            tx,
            store,
            actor,
            project.project,
            prior,
            keys,
        )
        .await?;
        let item = item.filter(|v| v.version_id == id).ok_or(Error::Conflict)?;
        return workflow_content(&item);
    }
    if kind == "project" {
        return Ok(
            scope(store, tx, actor, project.project, ContentAction::Read)
                .await?
                .payload,
        );
    }
    if kind == "model_configuration" {
        if let Some(value) = templates::read_optional(tx, actor, project, kind, id).await? {
            return Ok(value.0);
        }
        if project.team == Some(id) {
            return team_model_configuration(tx, id)
                .await?
                .ok_or(Error::Conflict);
        }
        store
            .identity()
            .require_global_session_in_transaction(tx, actor)
            .await
            .map_err(identity_error)?;
        return crate::android_personal_provider::current(tx, actor.identity_id(), id).await;
    }
    let sql = match kind {
        "thread" if project.team.is_none() => {
            "SELECT payload FROM android_personal_threads WHERE id=$1 AND project_id=$2 AND deleted=0"
        }
        "thread" => "SELECT payload FROM team_orders WHERE id=$1 AND project_id=$2",
        "message" => {
            "SELECT payload FROM android_personal_messages WHERE id=$1 AND project_id=$2 AND deleted=0"
        }
        "document" => "SELECT payload FROM team_documents WHERE id=$1 AND project_id=$2",
        "task" => {
            "SELECT payload FROM android_task_versions WHERE id=$1 AND project_id=$2 AND deleted=0 ORDER BY revision DESC LIMIT 1"
        }
        _ => "",
    };
    if !sql.is_empty() {
        let encoded: String = sqlx::query_scalar(sql)
            .bind(id.to_string())
            .bind(project.project.to_string())
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?
            .ok_or(Error::Conflict)?;
        let mut value: Value = parse(&encoded)?;
        if kind == "document"
            && let Some((mut source, _, _)) =
                templates::read_optional(tx, actor, project, kind, id).await?
        {
            if let (Some(source), Some(actual)) = (source.as_object_mut(), value.as_object()) {
                source.extend(actual.clone());
            }
            value = source;
        }
        return Ok(value);
    }
    let rows:Vec<String>=sqlx::query_scalar("SELECT payload FROM android_wire_sync_current WHERE entity_type=$1 AND entity_id=$2 AND (project_id=$3 OR project_id IS NULL) AND tombstone=0 AND ((team_id IS NULL AND owner_id=$4) OR team_id=$5) LIMIT 2").bind(kind).bind(id.to_string()).bind(project.project.to_string()).bind(actor.identity_id().to_string()).bind(project.team.map(|v|v.to_string())).fetch_all(&mut **tx).await.map_err(storage_error)?;
    if rows.len() != 1 {
        return Err(Error::Conflict);
    }
    let value: Value = parse(&rows[0])?;
    value.get("payload").cloned().ok_or(Error::Conflict)
}

fn workflow_content(item: &crate::android_private_workflows::RegistryContent) -> Result<Value> {
    use base64::Engine;
    Ok(
        json!({"workflow_id":item.workflow_id,"version_id":item.version_id,"engine":item.engine,"definition":serde_json::from_slice::<Value>(&item.bytes).map_err(storage_error)?,"original_base64":base64::engine::general_purpose::STANDARD.encode(item.bytes.as_slice()),"raw_sha256":item.raw_sha256,"canonical_sha256":item.canonical_sha256}),
    )
}

fn collect_references(value: &Value, refs: &mut BTreeMap<String, BTreeSet<Uuid>>) -> Result<()> {
    match value {
        Value::Object(map) => {
            for (name, value) in map {
                let kind = match name.as_str() {
                    "crew_id" => Some("crew"),
                    "skill_id" | "skill_ids" | "parent_skill_id" => Some("skill"),
                    "memory_id" | "memory_ids" => Some("memory"),
                    "mcp_metadata_ids" => Some("mcp_metadata"),
                    "model_profile_id" | "provider_profile_id" => Some("model_configuration"),
                    _ => None,
                };
                if let Some(kind) = kind {
                    for id in match value {
                        Value::Array(values) => values.iter().collect::<Vec<_>>(),
                        _ => vec![value],
                    } {
                        if let Some(id) = id.as_str().and_then(|s| Uuid::parse_str(s).ok()) {
                            refs.entry(kind.into()).or_default().insert(id);
                        }
                    }
                }
                collect_references(value, refs)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_references(value, refs)?;
            }
        }
        _ => (),
    }
    if refs.values().map(BTreeSet::len).sum::<usize>() > 10000 {
        return Err(Error::BudgetExceeded);
    }
    Ok(())
}
async fn team_model_configuration(
    tx: &mut Transaction<'_, Any>,
    team: Uuid,
) -> Result<Option<Value>> {
    let row = sqlx::query("SELECT revision,base_url,model FROM team_provider WHERE team_id=$1")
        .bind(team.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(
        json!({"schema_version":2,"id":team,"team_id":team,"owner_user_id":null,"revision":row.try_get::<i64,_>("revision").map_err(storage_error)?,"provider_kind":"openai_compatible","model_defaults":{"base_url":row.try_get::<String,_>("base_url").map_err(storage_error)?,"model":row.try_get::<String,_>("model").map_err(storage_error)?,"auth_mode":"bearer"},"credentials_copied":false}),
    ))
}
