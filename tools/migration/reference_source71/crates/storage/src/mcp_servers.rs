use super::*;
use cowork_domain::mcp::*;

impl Store {
    pub async fn mcp_servers(&self, project: &str) -> Result<Vec<McpServer>> {
        let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM projects WHERE id=$1")
            .bind(project)
            .fetch_one(&self.pool)
            .await
            .map_err(storage_error)?;
        if exists != 1 {
            return Err(Error::NotFound);
        }
        let rows = sqlx::query("SELECT id,project_id,revision,payload FROM mcp_servers WHERE project_id=$1 ORDER BY id")
            .bind(project).fetch_all(&self.pool).await.map_err(storage_error)?;
        rows.iter()
            .map(|r| Ok(checked_server(r)?.public_view()))
            .collect()
    }

    pub async fn select_mcp_tools(
        &self,
        project: &str,
        selections: &[McpToolSelection],
    ) -> Result<Vec<McpToolBinding>> {
        if selections.len() > 128 {
            return Err(Error::Validation(
                "At most 128 MCP tools may be selected".into(),
            ));
        }
        let mut seen = std::collections::HashSet::new();
        let mut bindings = Vec::new();
        for selection in selections {
            if !seen.insert((&selection.server_id, &selection.tool_name)) {
                return Err(Error::Conflict);
            }
            let server = self.mcp_server(project, &selection.server_id).await?;
            if server.revision != selection.server_revision {
                return Err(Error::Conflict);
            }
            self.mcp_server_credential(&server).await?;
            let binding = server
                .bindings()?
                .into_iter()
                .find(|b| b.definition["name"].as_str() == Some(&selection.tool_name))
                .ok_or(Error::Forbidden)?;
            bindings.push(binding);
        }
        Ok(bindings)
    }
    /// Private owner/control-plane operation. This is not a model tool or browser route.
    /// Request receipts are immutable; replaying an old enable never revives a revoked server.
    pub async fn configure_mcp_server(
        &self,
        project: &str,
        server: &str,
        expected_revision: i64,
        key: &str,
        configuration: &McpServerConfiguration,
    ) -> Result<McpServerRecord> {
        for value in [project, server, key] {
            valid_text(value, 128)?;
        }
        if expected_revision < 0 || expected_revision == i64::MAX {
            return Err(Error::Conflict);
        }
        let mut configuration = configuration.clone();
        configuration.endpoint = cowork_mcp::Endpoint::new(&configuration.endpoint, None)
            .map_err(|_| Error::Validation("Invalid MCP endpoint".into()))?
            .url()
            .into();
        let catalog = validate_configuration(&configuration)?;
        let request_sha256 =
            canonical_hash(&json!([project, server, expected_revision, configuration]));
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        // Serialize owner changes and idempotency receipts for this project before reading.
        if sqlx::query("UPDATE projects SET payload=payload WHERE id=$1")
            .bind(project)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?
            .rows_affected()
            != 1
        {
            return Err(Error::NotFound);
        }
        if let Some(row)=sqlx::query("SELECT request_sha256,payload FROM mcp_server_requests WHERE project_id=$1 AND request_key=$2")
            .bind(project).bind(key).fetch_optional(&mut *tx).await.map_err(storage_error)? {
            if row.try_get::<String,_>("request_sha256").map_err(storage_error)?!=request_sha256 {return Err(Error::Conflict);}
            let record:McpServerRecord=parse(&row.try_get::<String,_>("payload").map_err(storage_error)?)?;
            validate_record(&record)?;
            if record.project_id!=project || record.id!=server || record.revision!=expected_revision+1
                || record.configuration!=configuration {return Err(Error::Forbidden);}
            tx.commit().await.map_err(storage_error)?;
            return Ok(record);
        }
        let existing = sqlx::query(
            "SELECT id,project_id,revision,payload FROM mcp_servers WHERE id=$1 AND project_id=$2",
        )
        .bind(server)
        .bind(project)
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?;
        let previous = existing.as_ref().map(checked_server).transpose()?;
        if previous.as_ref().map_or(0, |s| s.revision) != expected_revision {
            return Err(Error::Conflict);
        }
        let timestamp = now();
        let record = McpServerRecord {
            id: server.into(),
            project_id: project.into(),
            revision: expected_revision + 1,
            configuration_sha256: canonical_hash(&configuration),
            catalog_sha256: catalog.sha256().into(),
            configuration,
            created_at: previous.as_ref().map_or(timestamp, |s| s.created_at),
            updated_at: timestamp,
        };
        validate_record(&record)?;
        let affected = if expected_revision == 0 {
            sqlx::query("INSERT INTO mcp_servers(id,project_id,revision,payload) VALUES($1,$2,1,$3) ON CONFLICT(id) DO NOTHING")
                .bind(server).bind(project).bind(json_string(&record)?).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()
        } else {
            sqlx::query("UPDATE mcp_servers SET revision=$1,payload=$2 WHERE id=$3 AND project_id=$4 AND revision=$5")
                .bind(record.revision).bind(json_string(&record)?).bind(server).bind(project).bind(expected_revision)
                .execute(&mut *tx).await.map_err(storage_error)?.rows_affected()
        };
        if affected != 1 {
            return Err(Error::Conflict);
        }
        sqlx::query("INSERT INTO mcp_server_requests(project_id,request_key,request_sha256,payload) VALUES($1,$2,$3,$4)")
            .bind(project).bind(key).bind(request_sha256).bind(json_string(&record)?)
            .execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(record)
    }
    pub async fn mcp_server(&self, project: &str, server: &str) -> Result<McpServerRecord> {
        let row = sqlx::query(
            "SELECT id,project_id,revision,payload FROM mcp_servers WHERE id=$1 AND project_id=$2",
        )
        .bind(server)
        .bind(project)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
        checked_server(&row)
    }
}
fn validate_configuration(config: &McpServerConfiguration) -> Result<cowork_mcp::Catalog> {
    valid_text(&config.name, 200)?;
    let endpoint =
        cowork_mcp::Endpoint::new(&config.endpoint, None).map_err(|_| Error::Forbidden)?;
    if endpoint.url() != config.endpoint {
        return Err(Error::Forbidden);
    }
    if let Some(reference) = &config.credential_reference {
        valid_text(reference, 128)?;
        if config.credential_revision <= 0 {
            return Err(Error::Forbidden);
        }
    } else if config.credential_revision != 0 {
        return Err(Error::Forbidden);
    }
    let catalog = cowork_mcp::Catalog::from_definitions(config.catalog.clone())
        .map_err(|_| Error::Validation("Invalid or unsupported MCP catalogue".into()))?;
    let mut seen = std::collections::HashSet::new();
    for name in &config.selected_tools {
        if !seen.insert(name) || !catalog.tools().any(|(n, _)| n == name) {
            return Err(Error::Forbidden);
        }
    }
    Ok(catalog)
}
fn validate_record(record: &McpServerRecord) -> Result<()> {
    for id in [&record.id, &record.project_id] {
        valid_text(id, 128)?;
    }
    let catalog = validate_configuration(&record.configuration)?;
    if record.revision < 1
        || record.created_at <= 0
        || record.updated_at < record.created_at
        || catalog.sha256() != record.catalog_sha256
        || canonical_hash(&record.configuration) != record.configuration_sha256
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}
pub(super) fn checked_server(row: &sqlx::any::AnyRow) -> Result<McpServerRecord> {
    let record: McpServerRecord =
        parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    validate_record(&record)?;
    if record.id != row.try_get::<String, _>("id").map_err(storage_error)?
        || record.project_id
            != row
                .try_get::<String, _>("project_id")
                .map_err(storage_error)?
        || record.revision != row.try_get::<i64, _>("revision").map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    Ok(record)
}
/// The row write fence is shared with reconfiguration/revocation until admission commits.
pub(super) async fn check_binding(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    run: &RunRecord,
    binding: &McpToolBinding,
) -> Result<()> {
    let locked = sqlx::query(
        "UPDATE mcp_servers SET revision=revision WHERE id=$1 AND project_id=$2 AND revision=$3",
    )
    .bind(&binding.server_id)
    .bind(&run.spec.project_id)
    .bind(binding.server_revision)
    .execute(&mut **tx)
    .await
    .map_err(storage_error)?
    .rows_affected();
    if locked != 1 {
        return Err(Error::Forbidden);
    }
    let row = sqlx::query(
        "SELECT id,project_id,revision,payload FROM mcp_servers WHERE id=$1 AND project_id=$2",
    )
    .bind(&binding.server_id)
    .bind(&run.spec.project_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage_error)?;
    let server = checked_server(&row)?;
    if !server.bindings()?.contains(binding) {
        return Err(Error::Forbidden);
    }
    mcp_credentials::check_server(tx, &server).await?;
    Ok(())
}
