use super::*;
pub use cowork_contracts::NativeMcpFactory;
impl Store {
    pub async fn native_mcp_factory(&self, id: &str) -> Result<Option<NativeMcpFactory>> {
        sqlx::query_scalar::<_, String>(
            "SELECT payload FROM native_mcp_factories WHERE server_id=$1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_error)?
        .map(|s| parse(&s))
        .transpose()
    }
    pub async fn native_mcp_factories(&self, project: &str) -> Result<Vec<NativeMcpFactory>> {
        let rows:Vec<String>=sqlx::query_scalar("SELECT payload FROM native_mcp_factories WHERE project_id=$1 AND NOT EXISTS(SELECT 1 FROM owner_connector_versions v WHERE v.server_id=native_mcp_factories.server_id AND v.is_current=0) ORDER BY server_id LIMIT 65").bind(project).fetch_all(&self.pool).await.map_err(storage_error)?;
        if rows.len() > 64 {
            return Err(Error::Conflict);
        }
        rows.into_iter().map(|s| parse(&s)).collect()
    }
    pub async fn put_native_mcp_factory(
        &self,
        value: &NativeMcpFactory,
        expected: i64,
    ) -> Result<NativeMcpFactory> {
        if value.revision != expected.checked_add(1).ok_or(Error::Conflict)? {
            return Err(Error::Conflict);
        }
        Uuid::parse_str(&value.server_id).map_err(|_| Error::Forbidden)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let n = sqlx::query("UPDATE projects SET payload=payload WHERE id=$1")
            .bind(&value.project_id)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?
            .rows_affected();
        if n != 1 {
            return Err(Error::NotFound);
        }
        let old: Option<String> =
            sqlx::query_scalar("SELECT payload FROM native_mcp_factories WHERE server_id=$1")
                .bind(&value.server_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?;
        if let Some(old) = old {
            let old: NativeMcpFactory = parse(&old)?;
            if old.project_id != value.project_id
                || old.endpoint != value.endpoint
                || old.credential_reference != value.credential_reference
                || old.revision != expected
            {
                return Err(Error::Conflict);
            }
        } else if expected != 0 {
            return Err(Error::Conflict);
        }
        sqlx::query("INSERT INTO native_mcp_factories(server_id,project_id,revision,payload) VALUES($1,$2,$3,$4) ON CONFLICT(server_id) DO UPDATE SET revision=excluded.revision,payload=excluded.payload")
            .bind(&value.server_id).bind(&value.project_id).bind(value.revision).bind(json_string(value)?).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value.clone())
    }
}
