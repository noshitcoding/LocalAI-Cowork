use super::*;
impl Store {
    /// Stores an immutable definition without resolving models, secrets or tools.
    pub async fn create_workflow(
        &self,
        project: &str,
        key: &str,
        definition: &WorkflowDefinition,
    ) -> Result<WorkflowLibraryEntry> {
        let original = cowork_workflow_contracts::WorkflowOperationRequest::Create {
            request: cowork_workflow_contracts::CreateWorkflow {
                idempotency_key: key.into(),
                definition: definition.clone(),
            },
        };
        let identity = super::super::workflow_operations::action_identity(project, &original);
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        super::super::workflow_drafts::require_project(&mut tx, project).await?;
        let reservation = super::super::workflow_write_keys::lock(
            &mut tx,
            "workflow-create",
            project,
            key,
            project,
            &identity,
        )
        .await?;
        super::super::workflow_write_keys::permit(&reservation, None)?;
        let result = create_in_tx(&mut tx, project, key, definition).await?;
        let first = &result.versions[0];
        let receipt = cowork_workflow_contracts::WorkflowOperationReceipt::Created {
            publication: WorkflowPublication {
                workflow_id: result.id.clone(),
                project_id: project.into(),
                revision: 1,
                version_id: Some(first.id.clone()),
                revoked: false,
                created_at: first.created_at,
            },
        };
        super::super::workflow_write_keys::accept(
            &mut tx,
            "workflow-create",
            project,
            key,
            &serde_json::to_value(receipt).map_err(storage_error)?,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn workflow(&self, project: &str, workflow: &str) -> Result<WorkflowLibraryEntry> {
        let mut tx = self.snapshot_transaction().await?;
        let entry = read_entry(&mut tx, project, workflow).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(entry)
    }
    pub async fn workflow_version(
        &self,
        project: &str,
        workflow: &str,
        version: &str,
    ) -> Result<WorkflowVersion> {
        let mut tx = self.snapshot_transaction().await?;
        read_entry(&mut tx, project, workflow).await?;
        let result = read_version(&mut tx, project, workflow, version).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn publish_workflow(
        &self,
        project: &str,
        workflow: &str,
        expected: i64,
        key: &str,
        definition: &WorkflowDefinition,
    ) -> Result<WorkflowPublication> {
        check_definition(definition)?;
        self.change_workflow(project, workflow, expected, key, Some(definition))
            .await
    }
    pub async fn revoke_workflow(
        &self,
        project: &str,
        workflow: &str,
        expected: i64,
        key: &str,
    ) -> Result<WorkflowPublication> {
        self.change_workflow(project, workflow, expected, key, None)
            .await
    }
    async fn change_workflow(
        &self,
        project: &str,
        workflow: &str,
        expected: i64,
        key: &str,
        definition: Option<&WorkflowDefinition>,
    ) -> Result<WorkflowPublication> {
        let original = match definition {
            Some(definition) => cowork_workflow_contracts::WorkflowOperationRequest::Publish {
                workflow_id: workflow.into(),
                request: cowork_workflow_contracts::PublishWorkflow {
                    base_revision: expected,
                    idempotency_key: key.into(),
                    definition: definition.clone(),
                },
            },
            None => cowork_workflow_contracts::WorkflowOperationRequest::Revoke {
                workflow_id: workflow.into(),
                request: cowork_workflow_contracts::RevokeWorkflow {
                    base_revision: expected,
                    idempotency_key: key.into(),
                },
            },
        };
        let identity = super::super::workflow_operations::action_identity(project, &original);
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        super::super::workflow_drafts::require_project(&mut tx, project).await?;
        let reservation = super::super::workflow_write_keys::lock(
            &mut tx,
            "workflow-change",
            workflow,
            key,
            project,
            &identity,
        )
        .await?;
        super::super::workflow_write_keys::permit(&reservation, None)?;
        let result = change_in_tx(&mut tx, project, workflow, expected, key, definition).await?;
        let receipt = match definition {
            Some(_) => cowork_workflow_contracts::WorkflowOperationReceipt::Published {
                publication: result.clone(),
            },
            None => cowork_workflow_contracts::WorkflowOperationReceipt::Revoked {
                publication: result.clone(),
            },
        };
        super::super::workflow_write_keys::accept(
            &mut tx,
            "workflow-change",
            workflow,
            key,
            &serde_json::to_value(receipt).map_err(storage_error)?,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
}
pub(crate) async fn lock_head(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    project: &str,
    workflow: &str,
) -> Result<()> {
    if sqlx::query("UPDATE workflows SET revision=revision WHERE id=$1 AND project_id=$2")
        .bind(workflow)
        .bind(project)
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?
        .rows_affected()
        != 1
    {
        return Err(Error::NotFound);
    }
    Ok(())
}
pub(crate) async fn read_entry(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    project: &str,
    workflow: &str,
) -> Result<WorkflowLibraryEntry> {
    let row =
        sqlx::query("SELECT name,revision,revoked FROM workflows WHERE id=$1 AND project_id=$2")
            .bind(workflow)
            .bind(project)
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?
            .ok_or(Error::NotFound)?;
    let mut entry = WorkflowLibraryEntry {
        id: workflow.into(),
        project_id: project.into(),
        name: row.try_get("name").map_err(storage_error)?,
        revision: row.try_get("revision").map_err(storage_error)?,
        revoked: row.try_get::<i64, _>("revoked").map_err(storage_error)? == 1,
        versions: vec![],
    };
    for row in sqlx::query("SELECT id,revision,name,sha256,created_at FROM workflow_versions WHERE workflow_id=$1 AND project_id=$2 ORDER BY revision").bind(workflow).bind(project).fetch_all(&mut **tx).await.map_err(storage_error)? {
        entry.versions.push(WorkflowVersionRef {id:row.try_get("id").map_err(storage_error)?,revision:row.try_get("revision").map_err(storage_error)?,name:row.try_get("name").map_err(storage_error)?,sha256:row.try_get("sha256").map_err(storage_error)?,created_at:row.try_get("created_at").map_err(storage_error)?});
    }
    valid_text(&entry.name, 200)?;
    if entry.versions.is_empty()
        || entry.versions.len() > 128
        || entry.versions[0].revision != 1
        || entry.versions.last().is_none_or(|v| v.name != entry.name)
    {
        return Err(Error::Forbidden);
    }
    let changes = sqlx::query(
        "SELECT revision,payload FROM workflow_changes WHERE workflow_id=$1 ORDER BY revision",
    )
    .bind(workflow)
    .fetch_all(&mut **tx)
    .await
    .map_err(storage_error)?;
    if entry.revision != changes.len() as i64 + 1 || changes.len() > 128 {
        return Err(Error::Forbidden);
    }
    let mut published = 1;
    let mut revoked = false;
    for (index, row) in changes.iter().enumerate() {
        let receipt: WorkflowPublication =
            parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        if revoked
            || receipt.project_id != project
            || receipt.workflow_id != workflow
            || receipt.revision != index as i64 + 2
            || receipt.revision != row.try_get::<i64, _>("revision").map_err(storage_error)?
        {
            return Err(Error::Forbidden);
        }
        match (&receipt.version_id, receipt.revoked) {
            (Some(id), false)
                if entry.versions.iter().any(|v| {
                    &v.id == id
                        && v.revision == receipt.revision
                        && v.created_at == receipt.created_at
                }) =>
            {
                published += 1
            }
            (None, true) => revoked = true,
            _ => return Err(Error::Forbidden),
        }
    }
    if revoked != entry.revoked || published != entry.versions.len() {
        return Err(Error::Forbidden);
    }
    for v in &entry.versions {
        valid_text(&v.id, 128)?;
        valid_text(&v.name, 200)?;
        if v.sha256.len() != 64
            || !v.sha256.bytes().all(|b| b.is_ascii_hexdigit())
            || v.created_at < 0
        {
            return Err(Error::Forbidden);
        }
    }
    Ok(entry)
}
async fn insert_version(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    v: &WorkflowVersion,
) -> Result<()> {
    sqlx::query("INSERT INTO workflow_versions(id,workflow_id,project_id,revision,name,sha256,created_at,payload) VALUES($1,$2,$3,$4,$5,$6,$7,$8)").bind(&v.reference.id).bind(&v.workflow_id).bind(&v.project_id).bind(v.reference.revision).bind(&v.reference.name).bind(&v.reference.sha256).bind(v.reference.created_at).bind(json_string(v)?).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
pub(crate) async fn read_version(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    project: &str,
    workflow: &str,
    version: &str,
) -> Result<WorkflowVersion> {
    let row=sqlx::query("SELECT revision,name,sha256,created_at,payload FROM workflow_versions WHERE id=$1 AND workflow_id=$2 AND project_id=$3").bind(version).bind(workflow).bind(project).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
    let v: WorkflowVersion = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    check_definition(&v.definition)?;
    if v.workflow_id != workflow
        || v.project_id != project
        || v.reference.id != version
        || v.reference.sha256 != canonical_hash(&v.definition)
        || v.reference.name != v.definition.name
        || v.reference.sha256 != row.try_get::<String, _>("sha256").map_err(storage_error)?
        || v.reference.revision != row.try_get::<i64, _>("revision").map_err(storage_error)?
        || v.reference.name != row.try_get::<String, _>("name").map_err(storage_error)?
        || v.reference.created_at != row.try_get::<i64, _>("created_at").map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    Ok(v)
}

pub(crate) async fn create_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    project: &str,
    key: &str,
    definition: &WorkflowDefinition,
) -> Result<WorkflowLibraryEntry> {
    valid_text(key, 128)?;
    check_definition(definition)?;
    let request_sha256 = canonical_hash(&(project, definition));
    let workflow_id = id();
    let version_id = id();
    let timestamp = seconds();
    let inserted=sqlx::query("INSERT INTO workflows(id,project_id,name,revision,revoked,request_key,request_sha256) SELECT $1,$2,$3,1,0,$4,$5 WHERE EXISTS(SELECT id FROM projects WHERE id=$2) ON CONFLICT(project_id,request_key) DO NOTHING")
            .bind(&workflow_id).bind(project).bind(&definition.name).bind(key).bind(&request_sha256).execute(&mut **tx).await.map_err(storage_error)?.rows_affected();
    let row = sqlx::query(
        "SELECT id,request_sha256 FROM workflows WHERE project_id=$1 AND request_key=$2",
    )
    .bind(project)
    .bind(key)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage_error)?
    .ok_or(Error::NotFound)?;
    if row
        .try_get::<String, _>("request_sha256")
        .map_err(storage_error)?
        != request_sha256
    {
        return Err(Error::Conflict);
    }
    let existing_id: String = row.try_get("id").map_err(storage_error)?;
    if inserted == 1 {
        insert_version(
            tx,
            &WorkflowVersion {
                workflow_id: workflow_id.clone(),
                project_id: project.into(),
                reference: WorkflowVersionRef {
                    id: version_id,
                    revision: 1,
                    name: definition.name.clone(),
                    sha256: canonical_hash(definition),
                    created_at: timestamp,
                },
                definition: definition.clone(),
            },
        )
        .await?;
    }
    lock_head(tx, project, &existing_id).await?;
    let result = read_entry(tx, project, &existing_id).await?;
    Ok(result)
}
pub(crate) async fn change_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    project: &str,
    workflow: &str,
    expected: i64,
    key: &str,
    definition: Option<&WorkflowDefinition>,
) -> Result<WorkflowPublication> {
    if let Some(definition) = definition {
        check_definition(definition)?;
    }
    valid_text(key, 128)?;
    let digest = canonical_hash(&(project, workflow, definition.map(canonical_hash)));
    lock_head(tx, project, workflow).await?;
    if let Some(row)=sqlx::query("SELECT request_sha256,payload FROM workflow_changes WHERE workflow_id=$1 AND request_key=$2").bind(workflow).bind(key).fetch_optional(&mut **tx).await.map_err(storage_error)? {
            if row.try_get::<String,_>("request_sha256").map_err(storage_error)?!=digest { return Err(Error::Conflict); }
            let result:WorkflowPublication=parse(&row.try_get::<String,_>("payload").map_err(storage_error)?)?;
            if result.revision != expected + 1 { return Err(Error::Conflict); }
            read_entry(tx,project,workflow).await?;
            return Ok(result);
        }
    let before = read_entry(tx, project, workflow).await?;
    if before.revision != expected {
        return Err(Error::Conflict);
    }
    if before.revoked {
        return Err(Error::Forbidden);
    }
    if definition.is_some() && before.versions.len() >= 128 {
        return Err(Error::BudgetExceeded);
    }
    let timestamp = seconds();
    if definition.is_some()
        && before
            .versions
            .last()
            .is_some_and(|v| timestamp < v.created_at)
    {
        return Err(Error::Forbidden);
    }
    let publication = WorkflowPublication {
        workflow_id: workflow.into(),
        project_id: project.into(),
        revision: expected + 1,
        version_id: definition.map(|_| id()),
        revoked: definition.is_none(),
        created_at: timestamp,
    };
    let name = definition.map(|d| d.name.as_str()).unwrap_or(&before.name);
    if sqlx::query("UPDATE workflows SET name=$1,revision=$2,revoked=$3 WHERE project_id=$4 AND id=$5 AND revision=$6").bind(name).bind(publication.revision).bind(i64::from(publication.revoked)).bind(project).bind(workflow).bind(expected).execute(&mut **tx).await.map_err(storage_error)?.rows_affected()!=1 { return Err(Error::Conflict); }
    if let Some(definition) = definition {
        insert_version(
            tx,
            &WorkflowVersion {
                workflow_id: workflow.into(),
                project_id: project.into(),
                reference: WorkflowVersionRef {
                    id: publication.version_id.clone().expect("publication"),
                    revision: publication.revision,
                    name: definition.name.clone(),
                    sha256: canonical_hash(definition),
                    created_at: timestamp,
                },
                definition: definition.clone(),
            },
        )
        .await?;
    }
    sqlx::query("INSERT INTO workflow_changes(workflow_id,request_key,request_sha256,revision,payload) VALUES($1,$2,$3,$4,$5)").bind(workflow).bind(key).bind(&digest).bind(publication.revision).bind(json_string(&publication)?).execute(&mut **tx).await.map_err(storage_error)?;
    read_entry(tx, project, workflow).await?;
    if publication.revoked {
        sqlx::query("UPDATE workflow_attention SET actionable_count=0,valid_until=NULL WHERE run_id IN (SELECT run_id FROM workflow_runs WHERE workflow_id=$1 AND project_id=$2)")
                .bind(workflow).bind(project).execute(&mut **tx).await.map_err(storage_error)?;
    }
    Ok(publication)
}
