//! One durable namespace shared by direct writes and journal operations.
//! The row is locked until the enclosing effect transaction commits.
use super::*;

pub(super) struct WriteKey {
    pub operation_id: Option<String>,
    pub state: String,
    pub receipt: Option<Value>,
}
pub(super) async fn lock(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    namespace: &str,
    target: &str,
    key: &str,
    project: &str,
    request: &Value,
) -> Result<WriteKey> {
    valid_text(key, 128)?;
    let sha256 = canonical_hash(request);
    sqlx::query("INSERT INTO workflow_write_keys(namespace,target_id,request_key,project_id,request_sha256,request_payload,state) VALUES($1,$2,$3,$4,$5,$6,'reserved') ON CONFLICT(namespace,target_id,request_key) DO NOTHING")
        .bind(namespace).bind(target).bind(key).bind(project).bind(&sha256).bind(json_string(request)?)
        .execute(&mut **tx).await.map_err(storage_error)?;
    // UPDATE obtains the PostgreSQL row lock; SQLite serializes the transaction.
    sqlx::query("UPDATE workflow_write_keys SET request_sha256=request_sha256 WHERE namespace=$1 AND target_id=$2 AND request_key=$3")
        .bind(namespace).bind(target).bind(key).execute(&mut **tx).await.map_err(storage_error)?;
    let row = sqlx::query("SELECT project_id,request_sha256,request_payload,operation_id,state,receipt FROM workflow_write_keys WHERE namespace=$1 AND target_id=$2 AND request_key=$3")
        .bind(namespace).bind(target).bind(key).fetch_one(&mut **tx).await.map_err(storage_error)?;
    let stored: Value = parse(
        &row.try_get::<String, _>("request_payload")
            .map_err(storage_error)?,
    )?;
    if row
        .try_get::<String, _>("project_id")
        .map_err(storage_error)?
        != project
    {
        return Err(Error::NotFound);
    }
    if row
        .try_get::<String, _>("request_sha256")
        .map_err(storage_error)?
        != sha256
        || canonical_hash(&stored) != sha256
    {
        return Err(Error::Conflict);
    }
    Ok(WriteKey {
        operation_id: row.try_get("operation_id").map_err(storage_error)?,
        state: row.try_get("state").map_err(storage_error)?,
        receipt: row
            .try_get::<Option<String>, _>("receipt")
            .map_err(storage_error)?
            .map(|s| parse(&s))
            .transpose()?,
    })
}
pub(super) fn permit(key: &WriteKey, operation: Option<&str>) -> Result<()> {
    if matches!(key.state.as_str(), "discarded" | "rejected")
        || (key.state != "accepted"
            && key
                .operation_id
                .as_deref()
                .is_some_and(|owner| Some(owner) != operation))
    {
        return Err(Error::Conflict);
    }
    Ok(())
}
pub(super) async fn accept(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    namespace: &str,
    target: &str,
    key: &str,
    receipt: &Value,
) -> Result<()> {
    if sqlx::query("UPDATE workflow_write_keys SET state='accepted',receipt=$1 WHERE namespace=$2 AND target_id=$3 AND request_key=$4 AND state IN ('reserved','accepted')")
        .bind(json_string(receipt)?).bind(namespace).bind(target).bind(key)
        .execute(&mut **tx).await.map_err(storage_error)?.rows_affected() != 1 {
        return Err(Error::Conflict);
    }
    Ok(())
}
