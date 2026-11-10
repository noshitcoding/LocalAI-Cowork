//! Session/ACL fenced team-file metadata and immutable external-operation intents.
use super::*;
use cowork_identity::{ContentAction, ContentPermit, IdentityError, Principal};
use serde::{Deserialize, Serialize};

pub const TEAM_BLOB_MAX_BYTES: u64 = 32 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TeamBlobState {
    Uploading,
    Ready,
    Uncertain,
    Deleting,
    Deleted,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamBlob {
    pub id: Uuid,
    pub team_id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub media_type: String,
    pub size: u64,
    pub sha256: String,
    pub version: i64,
    pub revision: i64,
    pub state: TeamBlobState,
    pub created_at: i64,
    pub updated_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamBlobVersion {
    pub blob_id: Uuid,
    pub version: i64,
    pub size: u64,
    pub sha256: String,
    pub object_key: String,
    pub backend_id: String,
    pub state: TeamBlobState,
    pub created_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamBlobUpload {
    pub idempotency_key: String,
    pub name: String,
    pub media_type: String,
    pub blob_id: Option<Uuid>,
    pub expected_revision: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamBlobDelete {
    pub idempotency_key: String,
    pub expected_revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BlobIntent {
    id: String,
    team_id: Uuid,
    project_id: Uuid,
    blob_id: Uuid,
    action: String,
    versions: Vec<TeamBlobVersion>,
    request_hash: String,
    state: String,
    created_at: i64,
    #[serde(default)]
    result: Option<TeamBlob>,
}
/// Original operation receipt, scoped to its creator and current project access.
/// Object keys and storage credentials never enter the public sync protocol.
#[derive(Clone, Debug, Serialize)]
pub struct TeamBlobOperation {
    pub idempotency_key: String,
    pub blob_id: Uuid,
    pub action: String,
    pub state: String,
    pub sha256: Option<String>,
    pub size: Option<u64>,
    pub result: Option<TeamBlob>,
    pub current: TeamBlob,
}
/// Minted only after persisted Principal/ACL checks and SQL commit. Not deserializable.
pub struct TeamBlobPlan {
    intent: BlobIntent,
    blob: TeamBlob,
    send: bool,
}
impl TeamBlobPlan {
    pub fn is_upload(&self) -> bool {
        self.intent.action == "upload"
    }
    pub fn metadata(&self) -> &TeamBlob {
        &self.blob
    }
    pub fn versions(&self) -> &[TeamBlobVersion] {
        &self.intent.versions
    }
    pub fn should_send(&self) -> bool {
        self.send
    }
}
/// Read descriptor contains no bearer or credentials. Storage checks it again
/// after IO before bytes are released to the caller.
pub struct TeamBlobRead {
    blob: TeamBlob,
    version: TeamBlobVersion,
}
impl TeamBlobRead {
    pub fn metadata(&self) -> &TeamBlob {
        &self.blob
    }
    pub fn version(&self) -> &TeamBlobVersion {
        &self.version
    }
}
fn authority(e: IdentityError) -> Error {
    match e {
        IdentityError::Validation => invalid(),
        IdentityError::Forbidden | IdentityError::Unauthenticated => Error::Forbidden,
        IdentityError::NotFound => Error::NotFound,
        IdentityError::Conflict => Error::Conflict,
        _ => storage_error(e),
    }
}
fn invalid() -> Error {
    Error::Validation("Invalid team file request".into())
}
fn key(value: &str) -> Result<()> {
    valid_text(value, 200)?;
    if value.chars().any(char::is_control) {
        return Err(invalid());
    }
    Ok(())
}
async fn authorize(
    store: &Store,
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    actor: &Principal,
    team: Uuid,
    project: Uuid,
    action: ContentAction,
) -> Result<ContentPermit> {
    let permit = store
        .identity()
        .authorize_content_in_transaction(tx, actor, team, project, action)
        .await
        .map_err(authority)?;
    let row = sqlx::query("SELECT organization_id FROM tenant_projects WHERE id=$1")
        .bind(project.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
    if row
        .try_get::<String, _>("organization_id")
        .map_err(storage_error)?
        != team.to_string()
    {
        return Err(Error::Forbidden);
    }
    // Archived team projects allow reads, not additional file mutations.
    if action == ContentAction::Write
        && let Some(row) =
            sqlx::query("SELECT archived FROM team_project_profiles WHERE project_id=$1")
                .bind(project.to_string())
                .fetch_optional(&mut **tx)
                .await
                .map_err(storage_error)?
        && row.try_get::<i64, _>("archived").map_err(storage_error)? != 0
    {
        return Err(Error::Forbidden);
    }
    Ok(permit)
}
async fn read_blob(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    team: Uuid,
    project: Uuid,
    blob: Uuid,
) -> Result<TeamBlob> {
    let row =
        sqlx::query("SELECT payload FROM team_blobs WHERE id=$1 AND team_id=$2 AND project_id=$3")
            .bind(blob.to_string())
            .bind(team.to_string())
            .bind(project.to_string())
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?
            .ok_or(Error::NotFound)?;
    let value: TeamBlob = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    if value.id != blob || value.team_id != team || value.project_id != project {
        return Err(Error::Conflict);
    }
    Ok(value)
}
async fn lock_blob(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    team: Uuid,
    project: Uuid,
    blob: Uuid,
) -> Result<()> {
    sqlx::query(
        "UPDATE team_blobs SET revision=revision WHERE id=$1 AND team_id=$2 AND project_id=$3",
    )
    .bind(blob.to_string())
    .bind(team.to_string())
    .bind(project.to_string())
    .execute(&mut **tx)
    .await
    .map_err(storage_error)?;
    Ok(())
}
async fn save_blob(tx: &mut sqlx::Transaction<'_, sqlx::Any>, blob: &TeamBlob) -> Result<()> {
    sqlx::query(
        "UPDATE team_blobs SET revision=$1,payload=$2 WHERE id=$3 AND team_id=$4 AND project_id=$5",
    )
    .bind(blob.revision)
    .bind(json_string(blob)?)
    .bind(blob.id.to_string())
    .bind(blob.team_id.to_string())
    .bind(blob.project_id.to_string())
    .execute(&mut **tx)
    .await
    .map_err(storage_error)?;
    Ok(())
}
async fn save_version(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    version: &TeamBlobVersion,
) -> Result<()> {
    sqlx::query("INSERT INTO team_blob_versions(blob_id,version,payload) VALUES($1,$2,$3) ON CONFLICT(blob_id,version) DO UPDATE SET payload=excluded.payload").bind(version.blob_id.to_string()).bind(version.version).bind(json_string(version)?).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
async fn audit(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    actor: &Principal,
    blob: &TeamBlob,
    action: &str,
) -> Result<()> {
    let sequence: i64 =
        sqlx::query_scalar("SELECT COALESCE(MAX(sequence),0)+1 FROM identity_audit")
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
    sqlx::query("INSERT INTO identity_audit(id,sequence,organization_id,actor_id,action,target_id,observed_at) VALUES($1,$2,$3,$4,$5,$6,$7)").bind(id()).bind(sequence).bind(blob.team_id.to_string()).bind(actor.identity_id().to_string()).bind(action).bind(blob.id.to_string()).bind(now()).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
impl Store {
    pub async fn team_blob_operation(
        &self,
        actor: &Principal,
        team: Uuid,
        project: Uuid,
        operation: &str,
    ) -> Result<TeamBlobOperation> {
        key(operation)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let permit = authorize(self, &mut tx, actor, team, project, ContentAction::Read).await?;
        let row = sqlx::query("SELECT payload FROM team_blob_intents WHERE team_id=$1 AND project_id=$2 AND creator_id=$3 AND idempotency_key=$4")
            .bind(team.to_string()).bind(project.to_string()).bind(actor.identity_id().to_string()).bind(operation)
            .fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        let intent: BlobIntent =
            parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        if intent.team_id != team || intent.project_id != project {
            return Err(Error::Conflict);
        }
        let current = read_blob(&mut tx, team, project, intent.blob_id).await?;
        let version = if intent.action == "upload" {
            intent.versions.first()
        } else {
            None
        };
        let value = TeamBlobOperation {
            idempotency_key: operation.into(),
            blob_id: intent.blob_id,
            action: intent.action,
            state: intent.state,
            sha256: version.map(|v| v.sha256.clone()),
            size: version.map(|v| v.size),
            result: intent.result,
            current,
        };
        self.identity()
            .validate_content_in_transaction(&mut tx, &permit)
            .await
            .map_err(authority)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn team_blob_list(
        &self,
        actor: &Principal,
        team: Uuid,
        project: Uuid,
    ) -> Result<Vec<TeamBlob>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let permit = authorize(self, &mut tx, actor, team, project, ContentAction::Read).await?;
        let rows=sqlx::query("SELECT payload FROM team_blobs WHERE team_id=$1 AND project_id=$2 ORDER BY id LIMIT 2001").bind(team.to_string()).bind(project.to_string()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        if rows.len() > 2000 {
            return Err(Error::Validation("Team project file limit reached".into()));
        }
        let values = rows
            .into_iter()
            .map(|r| parse(&r.try_get::<String, _>("payload").map_err(storage_error)?))
            .collect::<Result<Vec<TeamBlob>>>()?;
        self.identity()
            .validate_content_in_transaction(&mut tx, &permit)
            .await
            .map_err(authority)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(values)
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "team_blob_prepare_upload: Keep upload actor/team/project, original metadata/digest/size and selected storage backend explicit"
    )]
    pub async fn team_blob_prepare_upload(
        &self,
        actor: &Principal,
        team: Uuid,
        project: Uuid,
        input: TeamBlobUpload,
        sha256: String,
        size: u64,
        backend_id: String,
    ) -> Result<TeamBlobPlan> {
        key(&input.idempotency_key)?;
        key(&input.name)?;
        key(&input.media_type)?;
        key(&backend_id)?;
        if size > TEAM_BLOB_MAX_BYTES
            || sha256.len() != 64
            || !sha256.bytes().all(|b| b.is_ascii_hexdigit())
            || input.media_type.len() > 120
            || !input.media_type.is_ascii()
        {
            return Err(invalid());
        }
        let fingerprint = canonical_hash(&(
            team,
            project,
            actor.identity_id(),
            &input,
            &sha256,
            size,
            &backend_id,
        ));
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let permit = authorize(self, &mut tx, actor, team, project, ContentAction::Write).await?;
        if let Some(row)=sqlx::query("SELECT payload,request_hash FROM team_blob_intents WHERE team_id=$1 AND creator_id=$2 AND idempotency_key=$3").bind(team.to_string()).bind(actor.identity_id().to_string()).bind(&input.idempotency_key).fetch_optional(&mut *tx).await.map_err(storage_error)? {
            if row.try_get::<String,_>("request_hash").map_err(storage_error)?!=fingerprint{return Err(Error::Conflict);}
            let intent:BlobIntent=parse(&row.try_get::<String,_>("payload").map_err(storage_error)?)?;
            if intent.state!="committed"{return Err(Error::UncertainEffect);}
            read_blob(&mut tx,team,project,intent.blob_id).await?;
            let blob=intent.result.clone().ok_or(Error::Conflict)?;
            self.identity().validate_content_in_transaction(&mut tx,&permit).await.map_err(authority)?;tx.commit().await.map_err(storage_error)?;return Ok(TeamBlobPlan{intent,blob,send:false});
        }
        let stamp = now();
        let existing = if let Some(id) = input.blob_id {
            sqlx::query_scalar::<_, String>(
                "SELECT payload FROM team_blobs WHERE id=$1 AND team_id=$2 AND project_id=$3",
            )
            .bind(id.to_string())
            .bind(team.to_string())
            .bind(project.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?
        } else {
            None
        };
        let mut blob = if let (Some(blob), Some(_)) = (input.blob_id, existing) {
            lock_blob(&mut tx, team, project, blob).await?;
            let current = read_blob(&mut tx, team, project, blob).await?;
            if current.state != TeamBlobState::Ready
                || input.expected_revision != Some(current.revision)
            {
                return Err(Error::Conflict);
            }
            current
        } else {
            if input.expected_revision.is_some()
                && !(input.blob_id.is_some() && input.expected_revision == Some(0))
            {
                return Err(invalid());
            }
            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM team_blobs WHERE team_id=$1 AND project_id=$2",
            )
            .bind(team.to_string())
            .bind(project.to_string())
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
            if count >= 2000 {
                return Err(Error::Validation("Team project file limit reached".into()));
            }
            let blob = TeamBlob {
                id: input.blob_id.unwrap_or_else(Uuid::new_v4),
                team_id: team,
                project_id: project,
                name: input.name.clone(),
                media_type: input.media_type.clone(),
                size: 0,
                sha256: String::new(),
                version: 0,
                revision: 0,
                state: TeamBlobState::Uploading,
                created_at: stamp,
                updated_at: stamp,
            };
            let n=sqlx::query("INSERT INTO team_blobs(id,team_id,project_id,revision,payload) VALUES($1,$2,$3,0,$4) ON CONFLICT(id) DO NOTHING").bind(blob.id.to_string()).bind(team.to_string()).bind(project.to_string()).bind(json_string(&blob)?).execute(&mut *tx).await.map_err(storage_error)?.rows_affected();
            if n != 1 {
                return Err(Error::Conflict);
            }
            blob
        };
        if blob.version >= 1000 {
            return Err(Error::Validation("File version limit reached".into()));
        }
        blob.version = blob.version.checked_add(1).ok_or(Error::Conflict)?;
        blob.revision = blob.revision.checked_add(1).ok_or(Error::Conflict)?;
        blob.name = input.name;
        blob.media_type = input.media_type;
        blob.size = size;
        blob.sha256 = sha256.clone();
        blob.state = TeamBlobState::Uploading;
        blob.updated_at = stamp;
        let version = TeamBlobVersion {
            blob_id: blob.id,
            version: blob.version,
            size,
            sha256: sha256.clone(),
            object_key: format!(
                "cowork/v1/{team}/{project}/{}/{}/{sha256}",
                blob.id, blob.version
            ),
            backend_id,
            state: TeamBlobState::Uploading,
            created_at: stamp,
        };
        let intent = BlobIntent {
            id: id(),
            team_id: team,
            project_id: project,
            blob_id: blob.id,
            action: "upload".into(),
            versions: vec![version.clone()],
            request_hash: fingerprint.clone(),
            state: "pending".into(),
            created_at: stamp,
            result: None,
        };
        save_blob(&mut tx, &blob).await?;
        save_version(&mut tx, &version).await?;
        sqlx::query("INSERT INTO team_blob_intents(id,team_id,project_id,blob_id,creator_id,idempotency_key,request_hash,state,payload) VALUES($1,$2,$3,$4,$5,$6,$7,'pending',$8)").bind(&intent.id).bind(team.to_string()).bind(project.to_string()).bind(blob.id.to_string()).bind(actor.identity_id().to_string()).bind(input.idempotency_key).bind(fingerprint).bind(json_string(&intent)?).execute(&mut *tx).await.map_err(storage_error)?;
        audit(&mut tx, actor, &blob, "team_file_upload_prepared").await?;
        self.identity()
            .validate_content_in_transaction(&mut tx, &permit)
            .await
            .map_err(authority)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(TeamBlobPlan {
            intent,
            blob,
            send: true,
        })
    }
    pub async fn team_blob_finish(
        &self,
        actor: &Principal,
        plan: &TeamBlobPlan,
    ) -> Result<TeamBlob> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let permit = authorize(
            self,
            &mut tx,
            actor,
            plan.blob.team_id,
            plan.blob.project_id,
            ContentAction::Write,
        )
        .await?;
        lock_blob(
            &mut tx,
            plan.blob.team_id,
            plan.blob.project_id,
            plan.blob.id,
        )
        .await?;
        let mut blob = read_blob(
            &mut tx,
            plan.blob.team_id,
            plan.blob.project_id,
            plan.blob.id,
        )
        .await?;
        let row = sqlx::query("SELECT payload FROM team_blob_intents WHERE id=$1")
            .bind(&plan.intent.id)
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
        let mut intent: BlobIntent =
            parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        if intent.request_hash != plan.intent.request_hash || intent.blob_id != blob.id {
            return Err(Error::Conflict);
        }
        if intent.state == "committed" {
            self.identity()
                .validate_content_in_transaction(&mut tx, &permit)
                .await
                .map_err(authority)?;
            tx.commit().await.map_err(storage_error)?;
            return intent.result.ok_or(Error::Conflict);
        }
        if !matches!(intent.state.as_str(), "pending" | "uncertain")
            || blob.version != plan.blob.version
        {
            return Err(Error::Conflict);
        }
        let final_state = if intent.action == "upload" {
            TeamBlobState::Ready
        } else {
            TeamBlobState::Deleted
        };
        blob.state = final_state.clone();
        blob.revision = blob.revision.checked_add(1).ok_or(Error::Conflict)?;
        blob.updated_at = now();
        save_blob(&mut tx, &blob).await?;
        for mut v in intent.versions.clone() {
            v.state = final_state.clone();
            save_version(&mut tx, &v).await?;
        }
        intent.state = "committed".into();
        intent.result = Some(blob.clone());
        sqlx::query("UPDATE team_blob_intents SET state='committed',payload=$1 WHERE id=$2")
            .bind(json_string(&intent)?)
            .bind(&intent.id)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        audit(
            &mut tx,
            actor,
            &blob,
            if intent.action == "upload" {
                "team_file_uploaded"
            } else {
                "team_file_deleted"
            },
        )
        .await?;
        self.identity()
            .validate_content_in_transaction(&mut tx, &permit)
            .await
            .map_err(authority)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(blob)
    }
    /// Preserves an existing intent after failed/lost IO or post-IO ACL expiry.
    /// Does not mint a read or execution permission and cannot change its content.
    pub async fn team_blob_uncertain(&self, plan: &TeamBlobPlan) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        sqlx::query("UPDATE identity_installation SET revision=revision WHERE id=1")
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        lock_blob(
            &mut tx,
            plan.blob.team_id,
            plan.blob.project_id,
            plan.blob.id,
        )
        .await?;
        let row = sqlx::query("SELECT payload FROM team_blob_intents WHERE id=$1")
            .bind(&plan.intent.id)
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
        let mut intent: BlobIntent =
            parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        if intent.request_hash != plan.intent.request_hash {
            return Err(Error::Conflict);
        }
        if !matches!(intent.state.as_str(), "pending" | "uncertain") {
            return Ok(());
        }
        intent.state = "uncertain".into();
        let mut blob = read_blob(
            &mut tx,
            plan.blob.team_id,
            plan.blob.project_id,
            plan.blob.id,
        )
        .await?;
        if blob.version != plan.blob.version {
            return Err(Error::Conflict);
        }
        blob.state = TeamBlobState::Uncertain;
        blob.revision = blob.revision.checked_add(1).ok_or(Error::Conflict)?;
        blob.updated_at = now();
        save_blob(&mut tx, &blob).await?;
        sqlx::query("UPDATE team_blob_intents SET state='uncertain',payload=$1 WHERE id=$2")
            .bind(json_string(&intent)?)
            .bind(&intent.id)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)
    }
    pub async fn team_blob_read(
        &self,
        actor: &Principal,
        team: Uuid,
        project: Uuid,
        blob: Uuid,
        version: Option<i64>,
    ) -> Result<TeamBlobRead> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let permit = authorize(self, &mut tx, actor, team, project, ContentAction::Read).await?;
        let blob = read_blob(&mut tx, team, project, blob).await?;
        if matches!(blob.state, TeamBlobState::Deleting | TeamBlobState::Deleted) {
            return Err(Error::NotFound);
        }
        let row =
            sqlx::query("SELECT payload FROM team_blob_versions WHERE blob_id=$1 AND version=$2")
                .bind(blob.id.to_string())
                .bind(version.unwrap_or(blob.version))
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?
                .ok_or(Error::NotFound)?;
        let v: TeamBlobVersion =
            parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        if v.state != TeamBlobState::Ready {
            return Err(Error::Conflict);
        }
        self.identity()
            .validate_content_in_transaction(&mut tx, &permit)
            .await
            .map_err(authority)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(TeamBlobRead { blob, version: v })
    }
    pub async fn team_blob_read_finish(
        &self,
        actor: &Principal,
        read: &TeamBlobRead,
    ) -> Result<()> {
        self.team_blob_read(
            actor,
            read.blob.team_id,
            read.blob.project_id,
            read.blob.id,
            Some(read.version.version),
        )
        .await
        .map(|_| ())
    }
    pub async fn team_blob_versions(
        &self,
        actor: &Principal,
        team: Uuid,
        project: Uuid,
        blob: Uuid,
    ) -> Result<Vec<TeamBlobVersion>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let permit = authorize(self, &mut tx, actor, team, project, ContentAction::Read).await?;
        read_blob(&mut tx, team, project, blob).await?;
        let rows =
            sqlx::query("SELECT payload FROM team_blob_versions WHERE blob_id=$1 ORDER BY version")
                .bind(blob.to_string())
                .fetch_all(&mut *tx)
                .await
                .map_err(storage_error)?;
        let versions = rows
            .into_iter()
            .map(|r| parse(&r.try_get::<String, _>("payload").map_err(storage_error)?))
            .collect::<Result<Vec<TeamBlobVersion>>>()?;
        self.identity()
            .validate_content_in_transaction(&mut tx, &permit)
            .await
            .map_err(authority)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(versions)
    }
    pub async fn team_blob_prepare_delete(
        &self,
        actor: &Principal,
        team: Uuid,
        project: Uuid,
        blob: Uuid,
        input: TeamBlobDelete,
    ) -> Result<TeamBlobPlan> {
        key(&input.idempotency_key)?;
        let fingerprint = canonical_hash(&(team, project, blob, actor.identity_id(), &input));
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let permit = authorize(self, &mut tx, actor, team, project, ContentAction::Write).await?;
        lock_blob(&mut tx, team, project, blob).await?;
        let mut value = read_blob(&mut tx, team, project, blob).await?;
        if let Some(row)=sqlx::query("SELECT payload,request_hash FROM team_blob_intents WHERE team_id=$1 AND creator_id=$2 AND idempotency_key=$3").bind(team.to_string()).bind(actor.identity_id().to_string()).bind(&input.idempotency_key).fetch_optional(&mut *tx).await.map_err(storage_error)? {
            if row.try_get::<String,_>("request_hash").map_err(storage_error)?!=fingerprint{return Err(Error::Conflict);}
            let intent:BlobIntent=parse(&row.try_get::<String,_>("payload").map_err(storage_error)?)?;
            if intent.state!="committed"{return Err(Error::UncertainEffect);}
            self.identity().validate_content_in_transaction(&mut tx,&permit).await.map_err(authority)?;tx.commit().await.map_err(storage_error)?;let receipt=intent.result.clone().ok_or(Error::Conflict)?;return Ok(TeamBlobPlan{intent,blob:receipt,send:false});
        }
        if value.revision != input.expected_revision
            || matches!(
                value.state,
                TeamBlobState::Uploading | TeamBlobState::Deleting | TeamBlobState::Deleted
            )
        {
            return Err(Error::Conflict);
        }
        let rows =
            sqlx::query("SELECT payload FROM team_blob_versions WHERE blob_id=$1 ORDER BY version")
                .bind(blob.to_string())
                .fetch_all(&mut *tx)
                .await
                .map_err(storage_error)?;
        let versions = rows
            .into_iter()
            .map(|r| parse(&r.try_get::<String, _>("payload").map_err(storage_error)?))
            .collect::<Result<Vec<TeamBlobVersion>>>()?;
        // Explicit delete supersedes uncertain uploads. Late replies may not restore metadata.
        for row in sqlx::query("SELECT payload FROM team_blob_intents WHERE blob_id=$1 AND state IN ('pending','uncertain')").bind(blob.to_string()).fetch_all(&mut *tx).await.map_err(storage_error)? {
            let mut old:BlobIntent=parse(&row.try_get::<String,_>("payload").map_err(storage_error)?)?;
            old.state="superseded".into();
            sqlx::query("UPDATE team_blob_intents SET state='superseded',payload=$1 WHERE id=$2").bind(json_string(&old)?).bind(old.id).execute(&mut *tx).await.map_err(storage_error)?;
        }
        let intent = BlobIntent {
            id: id(),
            team_id: team,
            project_id: project,
            blob_id: blob,
            action: "delete".into(),
            versions,
            request_hash: fingerprint.clone(),
            state: "pending".into(),
            created_at: now(),
            result: None,
        };
        value.state = TeamBlobState::Deleting;
        value.revision = value.revision.checked_add(1).ok_or(Error::Conflict)?;
        value.updated_at = now();
        save_blob(&mut tx, &value).await?;
        sqlx::query("INSERT INTO team_blob_intents(id,team_id,project_id,blob_id,creator_id,idempotency_key,request_hash,state,payload) VALUES($1,$2,$3,$4,$5,$6,$7,'pending',$8)").bind(&intent.id).bind(team.to_string()).bind(project.to_string()).bind(blob.to_string()).bind(actor.identity_id().to_string()).bind(input.idempotency_key).bind(fingerprint).bind(json_string(&intent)?).execute(&mut *tx).await.map_err(storage_error)?;
        audit(&mut tx, actor, &value, "team_file_delete_prepared").await?;
        self.identity()
            .validate_content_in_transaction(&mut tx, &permit)
            .await
            .map_err(authority)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(TeamBlobPlan {
            intent,
            blob: value,
            send: true,
        })
    }
    /// Loads an uncertain operation only for an explicit read/HEAD reconciliation.
    pub async fn team_blob_reconciliation_plan(
        &self,
        actor: &Principal,
        team: Uuid,
        project: Uuid,
        blob: Uuid,
        revision: i64,
    ) -> Result<TeamBlobPlan> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let permit = authorize(self, &mut tx, actor, team, project, ContentAction::Write).await?;
        lock_blob(&mut tx, team, project, blob).await?;
        let value = read_blob(&mut tx, team, project, blob).await?;
        if value.revision != revision
            || !matches!(
                value.state,
                TeamBlobState::Uploading | TeamBlobState::Deleting | TeamBlobState::Uncertain
            )
        {
            return Err(Error::Conflict);
        }
        let row=sqlx::query("SELECT payload FROM team_blob_intents WHERE blob_id=$1 AND state IN ('pending','uncertain') ORDER BY id DESC LIMIT 1").bind(blob.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        let intent: BlobIntent =
            parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        self.identity()
            .validate_content_in_transaction(&mut tx, &permit)
            .await
            .map_err(authority)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(TeamBlobPlan {
            intent,
            blob: value,
            send: false,
        })
    }
}
