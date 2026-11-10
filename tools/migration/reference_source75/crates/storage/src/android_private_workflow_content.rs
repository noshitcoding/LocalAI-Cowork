//! Registry contents for project archives/forks. No runtime or publication grants are copied.
use super::*;
use cowork_contracts::AndroidWorkflowVersionPage;
pub(crate) struct RegistryContent {
    pub version_id: Uuid,
    pub workflow_id: Uuid,
    pub engine: AndroidWorkflowEngine,
    pub bytes: Zeroizing<Vec<u8>>,
    pub raw_sha256: String,
    pub canonical_sha256: String,
}
pub(crate) async fn content_page(
    tx: &mut Transaction<'_, Any>,
    store: &Store,
    actor: &Principal,
    project: Uuid,
    after: Option<Uuid>,
    keys: &dyn KeyStore,
) -> Result<(Option<RegistryContent>, Option<Uuid>)> {
    store
        .android_personal_permit(tx, actor, project, ContentAction::Read)
        .await?;
    let rows=sqlx::query("SELECT v.*,w.engine FROM android_private_workflow_versions v JOIN android_private_workflows w ON w.id=v.workflow_id AND w.owner_id=v.owner_id AND w.project_id=v.project_id WHERE v.owner_id=$1 AND v.project_id=$2 AND v.id>$3 ORDER BY v.id LIMIT 2").bind(actor.identity_id().to_string()).bind(project.to_string()).bind(after.map(|v|v.to_string()).unwrap_or_default()).fetch_all(&mut **tx).await.map_err(storage_error)?;
    let Some(r) = rows.first() else {
        return Ok((None, None));
    };
    let vid = uuid(r, "id")?;
    let bytes = cipher::read(
        tx,
        actor.identity_id(),
        project,
        vid,
        "version",
        r.try_get("definition_size").map_err(storage_error)?,
        keys,
    )
    .await?;
    let value: Value = serde_json::from_slice(&bytes).map_err(storage_error)?;
    let sha: String = r.try_get("definition_sha256").map_err(storage_error)?;
    if canonical_hash(&value) != sha {
        return Err(Error::Forbidden);
    }
    let kind = match r
        .try_get::<String, _>("engine")
        .map_err(storage_error)?
        .as_str()
    {
        "crew" => AndroidWorkflowEngine::Crew,
        "workflow" => AndroidWorkflowEngine::Workflow,
        _ => return Err(Error::Forbidden),
    };
    store
        .android_personal_permit(tx, actor, project, ContentAction::Read)
        .await?;
    let content = RegistryContent {
        version_id: vid,
        workflow_id: uuid(r, "workflow_id")?,
        engine: kind,
        raw_sha256: hash(&bytes),
        canonical_sha256: sha,
        bytes,
    };
    Ok((Some(content), if rows.len() > 1 { Some(vid) } else { None }))
}
pub(crate) async fn import_draft_with_id_in_tx(
    tx: &mut Transaction<'_, Any>,
    store: &Store,
    actor: &Principal,
    project: Uuid,
    raw: &[u8],
    engine_kind: &AndroidWorkflowEngine,
    keys: &dyn KeyStore,
    key: KeyId,
    current: i64,
    desired_id: Uuid,
) -> Result<Uuid> {
    if desired_id.is_nil() {
        return Err(device_invalid());
    }
    store
        .android_personal_permit(tx, actor, project, ContentAction::Write)
        .await?;
    let body: Value = serde_json::from_slice(raw).map_err(storage_error)?;
    definition::validate(engine_kind, &body)?;
    if raw.is_empty() || raw.len() > MAX_BODY {
        return Err(Error::BudgetExceeded);
    }
    quota(tx, actor.identity_id(), raw.len()).await?;
    let wid = desired_id;
    let vid = Uuid::new_v4();
    let name = body["name"]
        .as_str()
        .filter(|v| !v.is_empty() && v.len() <= 200)
        .ok_or_else(device_invalid)?;
    sqlx::query("INSERT INTO android_private_workflows(id,owner_id,project_id,name,engine,revision,revoked,published_version_id,source_entity_id,source_revision,source_legacy_id) VALUES($1,$2,$3,$4,$5,1,0,NULL,NULL,NULL,NULL)").bind(wid.to_string()).bind(actor.identity_id().to_string()).bind(project.to_string()).bind(name).bind(engine(engine_kind)).execute(&mut **tx).await.map_err(storage_error)?;
    sqlx::query("INSERT INTO android_private_workflow_versions(id,workflow_id,owner_id,project_id,revision,definition_sha256,definition_size,created_at) VALUES($1,$2,$3,$4,1,$5,$6,$7)").bind(vid.to_string()).bind(wid.to_string()).bind(actor.identity_id().to_string()).bind(project.to_string()).bind(canonical_hash(&body)).bind(raw.len() as i64).bind(current).execute(&mut **tx).await.map_err(storage_error)?;
    cipher::write(
        tx,
        actor.identity_id(),
        project,
        vid,
        "version",
        raw,
        keys,
        key,
    )
    .await?;
    store
        .android_personal_permit(tx, actor, project, ContentAction::Write)
        .await?;
    Ok(wid)
}
impl Store {
    pub async fn android_workflow_versions(
        &self,
        a: &Principal,
        project: Uuid,
        wid: Uuid,
        after: i64,
        limit: u16,
    ) -> Result<AndroidWorkflowVersionPage> {
        if after < 0 || !(1..=50).contains(&limit) {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.android_personal_permit(&mut tx, a, project, ContentAction::Read)
            .await?;
        let r = head(&mut tx, a.identity_id(), project, wid).await?;
        let ids=sqlx::query("SELECT id,revision FROM android_private_workflow_versions WHERE workflow_id=$1 AND owner_id=$2 AND project_id=$3 AND revision>$4 ORDER BY revision LIMIT $5").bind(wid.to_string()).bind(a.identity_id().to_string()).bind(project.to_string()).bind(after).bind(i64::from(limit)+1).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let mut versions = Vec::new();
        for v in ids.iter().take(limit as usize) {
            versions.push(version_ref(&mut tx, &r, uuid(v, "id")?).await?)
        }
        let next_revision = if ids.len() > limit as usize {
            versions.last().map(|v| v.revision)
        } else {
            None
        };
        tx.commit().await.map_err(storage_error)?;
        Ok(AndroidWorkflowVersionPage {
            versions,
            next_revision,
        })
    }
    pub async fn android_workflow_operation(
        &self,
        a: &Principal,
        project: Uuid,
        request: Uuid,
        keys: &dyn KeyStore,
    ) -> Result<AndroidWorkflowOperationReceipt> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.android_personal_permit(&mut tx, a, project, ContentAction::Read)
            .await?;
        let r=sqlx::query("SELECT * FROM android_private_workflow_operations WHERE owner_id=$1 AND request_id=$2 AND project_id=$3").bind(a.identity_id().to_string()).bind(request.to_string()).bind(project.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        let raw = cipher::read(
            &mut tx,
            a.identity_id(),
            project,
            request,
            "request",
            r.try_get("request_size").map_err(storage_error)?,
            keys,
        )
        .await?;
        let body: Value = serde_json::from_slice(&raw).map_err(storage_error)?;
        let receipt = original(
            &mut tx,
            a,
            project,
            request,
            &r.try_get::<String, _>("purpose").map_err(storage_error)?,
            &body,
            keys,
        )
        .await?
        .ok_or(Error::Forbidden)?;
        self.android_personal_permit(&mut tx, a, project, ContentAction::Read)
            .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
}

impl Store {
    pub async fn android_workflow_device_task(
        &self,
        a: &Principal,
        project: Uuid,
        config: &Value,
        keys: &dyn KeyStore,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.android_personal_permit(&mut tx, a, project, ContentAction::Execute)
            .await?;
        let selection = selection_for_task(&mut tx, a.identity_id(), project, config)
            .await?
            .ok_or(Error::Forbidden)?;
        let selected = selected(&mut tx, a.identity_id(), project, &selection, keys).await?;
        if selected.workflow.engine != AndroidWorkflowEngine::Crew {
            return Err(Error::Forbidden);
        }
        definition::validate(&selected.workflow.engine, &selected.definition)?;
        self.android_personal_permit(&mut tx, a, project, ContentAction::Execute)
            .await?;
        let again = head(&mut tx, a.identity_id(), project, selection.workflow_id).await?;
        if again.try_get::<i64, _>("revision").map_err(storage_error)?
            != selection.registry_revision
            || again.try_get::<i64, _>("revoked").map_err(storage_error)? != 0
        {
            return Err(Error::Forbidden);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(selected.definition)
    }
}
