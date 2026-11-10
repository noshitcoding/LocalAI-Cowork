//! Local input custody and original preparation receipt. No execution authority.
use super::*;
use cowork_domain::execution_inputs::{self as inputs, CapturedExecutionInput};

#[derive(Clone)]
pub struct StoredExecutionInput {
    pub capture: CapturedExecutionInput,
    pub retired: bool,
}
impl StoredExecutionInput {
    pub fn view(&self) -> PreparedExecutionInput {
        PreparedExecutionInput {
            snapshot: self.capture.snapshot.clone(),
            sha256: self.capture.sha256.clone(),
            retired: self.retired,
        }
    }
}

pub(super) fn checked(row: &sqlx::any::AnyRow, order: &str) -> Result<StoredExecutionInput> {
    let capture: CapturedExecutionInput =
        parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    let request: PrepareExecutionInput = parse(
        &row.try_get::<String, _>("request_payload")
            .map_err(storage_error)?,
    )?;
    capture.validate_request(&request)?;
    let retired: i64 = row.try_get("retired").map_err(storage_error)?;
    if capture.snapshot.id != row.try_get::<String, _>("id").map_err(storage_error)?
        || capture.snapshot.order_id != order
        || row
            .try_get::<String, _>("order_id")
            .map_err(storage_error)?
            != order
        || capture.snapshot.project_id
            != row
                .try_get::<String, _>("project_id")
                .map_err(storage_error)?
        || request.idempotency_key
            != row
                .try_get::<String, _>("idempotency_key")
                .map_err(storage_error)?
        || capture.snapshot.request_sha256
            != row
                .try_get::<String, _>("request_sha256")
                .map_err(storage_error)?
        || capture.sha256
            != row
                .try_get::<String, _>("snapshot_sha256")
                .map_err(storage_error)?
        || canonical_hash(&capture)
            != row
                .try_get::<String, _>("custody_sha256")
                .map_err(storage_error)?
        || !matches!(retired, 0 | 1)
    {
        return Err(Error::Conflict);
    }
    Ok(StoredExecutionInput {
        capture,
        retired: retired == 1,
    })
}

impl Store {
    pub async fn existing_execution_input(
        &self,
        order: &str,
        request: &PrepareExecutionInput,
    ) -> Result<Option<StoredExecutionInput>> {
        inputs::validate_request(request)?;
        let row = sqlx::query(
            "SELECT * FROM owner_execution_inputs WHERE order_id=$1 AND idempotency_key=$2",
        )
        .bind(order)
        .bind(&request.idempotency_key)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_error)?;
        row.map(|row| {
            let saved = checked(&row, order)?;
            if saved.capture.snapshot.request_sha256 != inputs::request_hash(order, request) {
                return Err(Error::Conflict);
            }
            Ok(saved)
        })
        .transpose()
    }

    /// Caller keeps all native reader/revocation locks until this transaction
    /// commits. A racing identical preparation returns the original receipt.
    pub async fn retain_execution_input(
        &self,
        request: &PrepareExecutionInput,
        capture: &CapturedExecutionInput,
    ) -> Result<StoredExecutionInput> {
        capture.validate_request(request)?;
        let order = &capture.snapshot.order_id;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        if sqlx::query("UPDATE orders SET revision=revision WHERE id=$1")
            .bind(order)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?
            .rows_affected()
            != 1
        {
            return Err(Error::NotFound);
        }
        if let Some(row) = sqlx::query(
            "SELECT * FROM owner_execution_inputs WHERE order_id=$1 AND idempotency_key=$2",
        )
        .bind(order)
        .bind(&request.idempotency_key)
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?
        {
            let saved = checked(&row, order)?;
            if saved.capture.snapshot.request_sha256 != capture.snapshot.request_sha256 {
                return Err(Error::Conflict);
            }
            tx.commit().await.map_err(storage_error)?;
            return Ok(saved);
        }
        let owner = sqlx::query("SELECT revision,project_id,payload FROM orders WHERE id=$1")
            .bind(order)
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
        let value: Order = parse(
            &owner
                .try_get::<String, _>("payload")
                .map_err(storage_error)?,
        )?;
        if value.id != *order
            || value.project_id != capture.snapshot.project_id
            || value.revision != request.base_revision
            || owner
                .try_get::<String, _>("project_id")
                .map_err(storage_error)?
                != value.project_id
            || owner.try_get::<i64, _>("revision").map_err(storage_error)? != value.revision
        {
            return Err(Error::Conflict);
        }
        sqlx::query("INSERT INTO owner_execution_inputs(id,order_id,project_id,idempotency_key,request_sha256,request_payload,snapshot_sha256,custody_sha256,payload) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)")
            .bind(&capture.snapshot.id).bind(order).bind(&capture.snapshot.project_id).bind(&request.idempotency_key)
            .bind(&capture.snapshot.request_sha256).bind(json_string(request)?).bind(&capture.sha256).bind(canonical_hash(capture)).bind(json_string(capture)?)
            .execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(StoredExecutionInput {
            capture: capture.clone(),
            retired: false,
        })
    }

    pub async fn execution_input(
        &self,
        order: &str,
        snapshot: &str,
    ) -> Result<StoredExecutionInput> {
        let row = sqlx::query("SELECT * FROM owner_execution_inputs WHERE order_id=$1 AND id=$2")
            .bind(order)
            .bind(snapshot)
            .fetch_optional(&self.pool)
            .await
            .map_err(storage_error)?
            .ok_or(Error::NotFound)?;
        checked(&row, order)
    }

    pub async fn retire_execution_input(
        &self,
        order: &str,
        snapshot: &str,
        sha256: &str,
    ) -> Result<StoredExecutionInput> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        if sqlx::query(
            "UPDATE owner_execution_inputs SET retired=retired WHERE order_id=$1 AND id=$2",
        )
        .bind(order)
        .bind(snapshot)
        .execute(&mut *tx)
        .await
        .map_err(storage_error)?
        .rows_affected()
            != 1
        {
            return Err(Error::NotFound);
        }
        let row = sqlx::query("SELECT * FROM owner_execution_inputs WHERE order_id=$1 AND id=$2")
            .bind(order)
            .bind(snapshot)
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
        let mut saved = checked(&row, order)?;
        if saved.capture.sha256 != sha256 {
            return Err(Error::Conflict);
        }
        sqlx::query("UPDATE owner_execution_inputs SET retired=1 WHERE id=$1")
            .bind(snapshot)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        saved.retired = true;
        Ok(saved)
    }
}
