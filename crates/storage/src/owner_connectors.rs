use super::*;
use cowork_domain::mcp::McpServerRecord;

pub fn connector_request_hash(input: &SaveOwnerConnector) -> String {
    // No URL, bearer or unkeyed hash of secret material is persisted.
    canonical_hash(&json!([
        input.id,
        input.expected_revision,
        input.name,
        input.note,
        input.enabled,
        input.remove_bearer
    ]))
}
impl Store {
    pub async fn owner_connectors(&self, project: &str) -> Result<Vec<OwnerConnector>> {
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT payload FROM owner_connectors WHERE project_id=$1 ORDER BY id",
        )
        .bind(project)
        .fetch_all(&self.pool)
        .await
        .map_err(storage_error)?;
        rows.into_iter().map(|s| parse(&s)).collect()
    }
    pub async fn owner_connector(&self, project: &str, id: &str) -> Result<OwnerConnector> {
        let raw: String = sqlx::query_scalar(
            "SELECT payload FROM owner_connectors WHERE project_id=$1 AND id=$2",
        )
        .bind(project)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
        parse(&raw)
    }
    pub async fn owner_connector_receipt(
        &self,
        project: &str,
        key: &str,
        digest: &str,
    ) -> Result<Option<OwnerConnector>> {
        let row=sqlx::query("SELECT request_sha256,payload FROM owner_connector_requests WHERE project_id=$1 AND request_key=$2").bind(project).bind(key).fetch_optional(&self.pool).await.map_err(storage_error)?;
        row.map(|r| {
            if r.try_get::<String, _>("request_sha256")
                .map_err(storage_error)?
                != digest
            {
                return Err(Error::Conflict);
            }
            parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)
        })
        .transpose()
    }
    pub async fn is_connector_server(&self, server: &str) -> Result<bool> {
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM owner_connector_versions WHERE server_id=$1",
        )
        .bind(server)
        .fetch_one(&self.pool)
        .await
        .map_err(storage_error)?
            != 0)
    }
    pub async fn connector_server_selectable(&self, server: &str) -> Result<bool> {
        let raw:Option<String>=sqlx::query_scalar("SELECT c.payload FROM owner_connectors c JOIN owner_connector_versions v ON v.connector_id=c.id WHERE v.server_id=$1").bind(server).fetch_optional(&self.pool).await.map_err(storage_error)?;
        Ok(match raw {
            None => true,
            Some(raw) => {
                let c: OwnerConnector = parse(&raw)?;
                !c.deleted && c.enabled && c.server_id == server
            }
        })
    }
    pub async fn connector_version_ids(&self, project: &str, id: &str) -> Result<Vec<String>> {
        sqlx::query_scalar("SELECT server_id FROM owner_connector_versions WHERE project_id=$1 AND connector_id=$2 ORDER BY server_id").bind(project).bind(id).fetch_all(&self.pool).await.map_err(storage_error)
    }
    /// One transaction publishes metadata, the immutable catalog and its factory.
    /// Encrypted credentials may be staged beforehand; they have no authority alone.
    pub async fn publish_connector(
        &self,
        input: &SaveOwnerConnector,
        value: &OwnerConnector,
        factory: &NativeMcpFactory,
        catalog: Vec<Value>,
    ) -> Result<OwnerConnector> {
        if value.id != input.id
            || value.name != input.name
            || value.note != input.note
            || value.enabled != input.enabled
            || value.deleted
        {
            return Err(Error::Forbidden);
        }
        let digest = connector_request_hash(input);
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        if sqlx::query("UPDATE projects SET payload=payload WHERE id=$1")
            .bind(&value.project_id)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?
            .rows_affected()
            != 1
        {
            return Err(Error::NotFound);
        }
        if let Some(row)=sqlx::query("SELECT request_sha256,payload FROM owner_connector_requests WHERE project_id=$1 AND request_key=$2").bind(&value.project_id).bind(&input.idempotency_key).fetch_optional(&mut *tx).await.map_err(storage_error)?{
            if row.try_get::<String,_>("request_sha256").map_err(storage_error)?!=digest{return Err(Error::Conflict)}return parse(&row.try_get::<String,_>("payload").map_err(storage_error)?);
        }
        let previous: Option<String> =
            sqlx::query_scalar("SELECT payload FROM owner_connectors WHERE id=$1")
                .bind(&value.id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?;
        let previous: Option<OwnerConnector> = previous.map(|s| parse(&s)).transpose()?;
        if previous.as_ref().map_or(0, |c| c.revision) != input.expected_revision
            || previous.as_ref().is_some_and(|c| {
                c.deleted || c.native_only != value.native_only || c.project_id != value.project_id
            })
            || value.revision
                != input
                    .expected_revision
                    .checked_add(1)
                    .ok_or(Error::Conflict)?
        {
            return Err(Error::Conflict);
        }
        if value.server_id != factory.server_id
            || factory.project_id != value.project_id
            || factory.native_only != value.native_only
            || factory.revision != 1
            || factory.enabled != value.enabled
        {
            return Err(Error::Forbidden);
        }
        let configuration = McpServerConfiguration {
            name: value.name.clone(),
            endpoint: factory.endpoint.clone(),
            credential_reference: Some(factory.credential_reference.clone()),
            credential_revision: factory.credential_revision,
            catalog,
            selected_tools: vec!["send_json".into()],
            enabled: value.enabled,
        };
        let catalog = cowork_mcp::Catalog::from_definitions(configuration.catalog.clone())
            .map_err(|_| Error::Forbidden)?;
        let record = McpServerRecord {
            id: factory.server_id.clone(),
            project_id: value.project_id.clone(),
            revision: 1,
            configuration_sha256: canonical_hash(&configuration),
            catalog_sha256: catalog.sha256().into(),
            configuration,
            created_at: now(),
            updated_at: now(),
        };
        sqlx::query("INSERT INTO owner_connectors(id,project_id,revision,payload) VALUES($1,$2,$3,$4) ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,payload=excluded.payload").bind(&value.id).bind(&value.project_id).bind(value.revision).bind(json_string(value)?).execute(&mut *tx).await.map_err(storage_error)?;
        sqlx::query("UPDATE owner_connector_versions SET is_current=0 WHERE connector_id=$1 AND project_id=$2").bind(&value.id).bind(&value.project_id).execute(&mut *tx).await.map_err(storage_error)?;
        sqlx::query("INSERT INTO owner_connector_versions(server_id,connector_id,project_id,is_current) VALUES($1,$2,$3,1)").bind(&factory.server_id).bind(&value.id).bind(&value.project_id).execute(&mut *tx).await.map_err(storage_error)?;
        sqlx::query("INSERT INTO native_mcp_factories(server_id,project_id,revision,payload) VALUES($1,$2,1,$3)").bind(&factory.server_id).bind(&value.project_id).bind(json_string(factory)?).execute(&mut *tx).await.map_err(storage_error)?;
        sqlx::query("INSERT INTO mcp_servers(id,project_id,revision,payload) VALUES($1,$2,1,$3)")
            .bind(&factory.server_id)
            .bind(&value.project_id)
            .bind(json_string(&record)?)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        if !value.enabled {
            disable_versions(&mut tx, &value.project_id, &value.id, false).await?;
        }
        sqlx::query("INSERT INTO owner_connector_requests(project_id,request_key,request_sha256,payload) VALUES($1,$2,$3,$4)").bind(&value.project_id).bind(&input.idempotency_key).bind(digest).bind(json_string(value)?).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value.clone())
    }
    pub async fn delete_connector(
        &self,
        project: &str,
        id: &str,
        input: &DeleteOwnerConnector,
        native: bool,
    ) -> Result<OwnerConnector> {
        let digest = canonical_hash(&json!(["delete", id, input.expected_revision]));
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        sqlx::query("UPDATE projects SET payload=payload WHERE id=$1")
            .bind(project)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        if let Some(row)=sqlx::query("SELECT request_sha256,payload FROM owner_connector_requests WHERE project_id=$1 AND request_key=$2").bind(project).bind(&input.idempotency_key).fetch_optional(&mut *tx).await.map_err(storage_error)?{if row.try_get::<String,_>("request_sha256").map_err(storage_error)?!=digest{return Err(Error::Conflict)}return parse(&row.try_get::<String,_>("payload").map_err(storage_error)?);}
        let raw: String = sqlx::query_scalar(
            "SELECT payload FROM owner_connectors WHERE id=$1 AND project_id=$2",
        )
        .bind(id)
        .bind(project)
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
        let mut value: OwnerConnector = parse(&raw)?;
        if value.revision != input.expected_revision || value.deleted || value.native_only != native
        {
            return Err(Error::Conflict);
        }
        value.revision = value.revision.checked_add(1).ok_or(Error::Conflict)?;
        value.enabled = false;
        value.deleted = true;
        disable_versions(&mut tx, project, id, true).await?;
        sqlx::query(
            "UPDATE owner_connectors SET revision=$1,payload=$2 WHERE id=$3 AND project_id=$4",
        )
        .bind(value.revision)
        .bind(json_string(&value)?)
        .bind(id)
        .bind(project)
        .execute(&mut *tx)
        .await
        .map_err(storage_error)?;
        sqlx::query("INSERT INTO owner_connector_requests(project_id,request_key,request_sha256,payload) VALUES($1,$2,$3,$4)").bind(project).bind(&input.idempotency_key).bind(digest).bind(json_string(&value)?).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
}
async fn disable_versions(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    project: &str,
    id: &str,
    revoke: bool,
) -> Result<()> {
    let rows:Vec<String>=sqlx::query_scalar("SELECT f.payload FROM native_mcp_factories f JOIN owner_connector_versions v ON v.server_id=f.server_id WHERE v.connector_id=$1 AND v.project_id=$2").bind(id).bind(project).fetch_all(&mut **tx).await.map_err(storage_error)?;
    for raw in rows {
        let mut f: NativeMcpFactory = parse(&raw)?;
        if f.enabled {
            f.enabled = false;
            f.revision = f.revision.checked_add(1).ok_or(Error::Conflict)?;
            sqlx::query(
                "UPDATE native_mcp_factories SET revision=$1,payload=$2 WHERE server_id=$3",
            )
            .bind(f.revision)
            .bind(json_string(&f)?)
            .bind(&f.server_id)
            .execute(&mut **tx)
            .await
            .map_err(storage_error)?;
        }
        if revoke {
            let credentials: Vec<String> =
                sqlx::query_scalar("SELECT payload FROM mcp_credentials WHERE project_id=$1")
                    .bind(project)
                    .fetch_all(&mut **tx)
                    .await
                    .map_err(storage_error)?;
            for raw in credentials {
                let mut c: McpCredentialRecord = parse(&raw)?;
                if c.scope.server_id == f.server_id && !c.revoked {
                    c.revoked = true;
                    c.revision = c.revision.checked_add(1).ok_or(Error::Conflict)?;
                    c.sealed = None;
                    sqlx::query("UPDATE mcp_credentials SET revision=$1,payload=$2 WHERE project_id=$3 AND reference=$4").bind(c.revision).bind(json_string(&c)?).bind(project).bind(&c.scope.reference).execute(&mut **tx).await.map_err(storage_error)?;
                }
            }
        }
    }
    Ok(())
}
