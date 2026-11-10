//! Explicit installation-owner MFA policy, durable CAS and original public receipts.
use super::*;
impl Store {
    pub async fn android_management_policy(&self) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        let r = sqlx::query(
            "SELECT revision,admin_required FROM android_auth_security_policy WHERE id=1",
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        let result = json!({"schema_version":2,"revision":r.try_get::<i64,_>("revision").map_err(storage_error)?,"admin_required":r.try_get::<i64,_>("admin_required").map_err(storage_error)?==1});
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    /// This is an installation-owner API. Tenant routers must never expose it.
    pub async fn android_management_set_policy(
        &self,
        operation: Uuid,
        expected: i64,
        required: bool,
    ) -> Result<Value> {
        if operation.is_nil() || expected <= 0 {
            return Err(invalid());
        }
        let request = hash(json_string(&(expected, required))?.as_bytes());
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        if let Some(r)=sqlx::query("SELECT request_sha256,receipt FROM android_auth_management_operations WHERE operation_id=$1").bind(operation.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?{
            if r.try_get::<String,_>("request_sha256").map_err(storage_error)?!=request {return Err(Error::Conflict);}
            let value=parse(&r.try_get::<String,_>("receipt").map_err(storage_error)?)?;
            tx.commit().await.map_err(storage_error)?;return Ok(value);
        }
        let next = expected.checked_add(1).ok_or(Error::Conflict)?;
        one(sqlx::query("UPDATE android_auth_security_policy SET revision=$1,admin_required=$2 WHERE id=1 AND revision=$3")
            .bind(next).bind(i64::from(required)).bind(expected).execute(&mut *tx).await.map_err(storage_error)?.rows_affected())?;
        let receipt = json!({"schema_version":2,"revision":next,"admin_required":required});
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM android_auth_management_operations")
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
        if n >= 4096 {
            return Err(Error::BudgetExceeded);
        }
        sqlx::query("INSERT INTO android_auth_management_operations(operation_id,request_sha256,receipt,created_at) VALUES($1,$2,$3,$4)")
            .bind(operation.to_string()).bind(request).bind(json_string(&receipt)?).bind(current).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
}
