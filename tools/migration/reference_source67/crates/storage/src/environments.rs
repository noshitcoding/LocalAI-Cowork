use super::*;
use cowork_domain::environments::*;

impl Store {
    pub async fn environments(
        &self,
        project: &str,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<EnvironmentRecord>> {
        if !(1..=501).contains(&limit) || after.is_some_and(|value| value.len() > 128) {
            return Err(Error::Validation("Invalid environment page".into()));
        }
        sqlx::query_scalar::<_, String>("SELECT id FROM projects WHERE id=$1")
            .bind(project)
            .fetch_optional(&self.pool)
            .await
            .map_err(storage_error)?
            .ok_or(Error::NotFound)?;
        let rows=sqlx::query("SELECT id,revision,payload FROM environments WHERE project_id=$1 AND id>$2 ORDER BY id LIMIT $3")
            .bind(project).bind(after.unwrap_or("")).bind(i64::from(limit)).fetch_all(&self.pool).await.map_err(storage_error)?;
        rows.iter()
            .map(|row| {
                let id: String = row.try_get("id").map_err(storage_error)?;
                checked(row, project, &id)
            })
            .collect()
    }
    /// Private control-plane operation. No HTTP/MCP route accepts environment authority.
    pub async fn create_environment(
        &self,
        project: &str,
        name: &str,
        key: &str,
    ) -> Result<EnvironmentRecord> {
        valid_text(name, 200)?;
        valid_text(key, 128)?;
        let record = EnvironmentRecord {
            format_version: 1,
            id: id(),
            project_id: project.into(),
            name: name.into(),
            revision: 1,
            active_version_id: None,
            revoked: false,
            versions: vec![],
            receipts: vec![],
        };
        record.validate()?;
        let request_hash = canonical_hash(&(project, name));
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        sqlx::query("INSERT INTO environments(id,project_id,idempotency_key,request_hash,revision,payload) SELECT $1,$2,$3,$4,1,$5 WHERE EXISTS(SELECT id FROM projects WHERE id=$2) ON CONFLICT(project_id,idempotency_key) DO NOTHING")
            .bind(&record.id).bind(project).bind(key).bind(&request_hash).bind(json_string(&record)?)
            .execute(&mut *tx).await.map_err(storage_error)?;
        let row = sqlx::query("SELECT request_hash,payload FROM environments WHERE project_id=$1 AND idempotency_key=$2")
            .bind(project).bind(key).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        if row
            .try_get::<String, _>("request_hash")
            .map_err(storage_error)?
            != request_hash
        {
            return Err(Error::Conflict);
        }
        let record: EnvironmentRecord =
            parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        record.validate()?;
        if record.project_id != project || record.name != name {
            return Err(Error::Forbidden);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(record)
    }
    pub async fn environment(&self, project: &str, environment: &str) -> Result<EnvironmentRecord> {
        let row =
            sqlx::query("SELECT revision,payload FROM environments WHERE project_id=$1 AND id=$2")
                .bind(project)
                .bind(environment)
                .fetch_optional(&self.pool)
                .await
                .map_err(storage_error)?
                .ok_or(Error::NotFound)?;
        checked(&row, project, environment)
    }
    /// Records only trusted adapter evidence. It does not install, execute, authenticate or
    /// qualify an adapter. Those operations must precede their corresponding transition.
    pub async fn change_environment(
        &self,
        project: &str,
        environment: &str,
        expected_revision: i64,
        key: &str,
        change: &EnvironmentChange,
    ) -> Result<EnvironmentReceipt> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        // Acquire the write lock before reading, including across independent SQLite pools.
        let locked =
            sqlx::query("UPDATE environments SET revision=revision WHERE project_id=$1 AND id=$2")
                .bind(project)
                .bind(environment)
                .execute(&mut *tx)
                .await
                .map_err(storage_error)?
                .rows_affected();
        if locked != 1 {
            return Err(Error::NotFound);
        }
        let row =
            sqlx::query("SELECT revision,payload FROM environments WHERE project_id=$1 AND id=$2")
                .bind(project)
                .bind(environment)
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        let mut record = checked(&row, project, environment)?;
        let before = record.revision;
        let receipt = record.apply(expected_revision, key, change, &id(), now())?;
        if record.revision != before {
            let changed = sqlx::query("UPDATE environments SET revision=$1,payload=$2 WHERE project_id=$3 AND id=$4 AND revision=$5")
                .bind(record.revision).bind(json_string(&record)?).bind(project).bind(environment).bind(before)
                .execute(&mut *tx).await.map_err(storage_error)?.rows_affected();
            if changed != 1 {
                return Err(Error::Conflict);
            }
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
    pub async fn pin_environment(
        &self,
        project: &str,
        environment: &str,
        observation: &EnvironmentObservation,
    ) -> Result<EnvironmentPin> {
        self.environment(project, environment)
            .await?
            .pin(observation, now())
    }
    pub async fn check_environment_pin(
        &self,
        project: &str,
        pin: &EnvironmentPin,
        observation: &EnvironmentObservation,
    ) -> Result<()> {
        if pin.project_id != project {
            return Err(Error::Forbidden);
        }
        self.environment(project, &pin.environment_id)
            .await?
            .resolve_pin(pin, observation, now())
    }
}
pub(super) fn checked(
    row: &sqlx::any::AnyRow,
    project: &str,
    id: &str,
) -> Result<EnvironmentRecord> {
    let record: EnvironmentRecord =
        parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    record.validate()?;
    if record.project_id != project
        || record.id != id
        || record.revision != row.try_get::<i64, _>("revision").map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    Ok(record)
}
