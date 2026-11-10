//! Device identities are convenience records, never bearer tokens or ACL grants.
//! The offline queue contains only explicit personal content edits.
use super::productivity::*;
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProductivityPlatform {
    Windows,
    Linux,
    Android,
    Web,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductivityDevice {
    pub id: String,
    pub name: String,
    pub platform: ProductivityPlatform,
    pub protocol_version: u32,
    pub revision: i64,
    pub revoked: bool,
    pub created_at: i64,
    pub last_seen_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterProductivityDevice {
    pub idempotency_key: String,
    pub name: String,
    pub platform: ProductivityPlatform,
    pub protocol_version: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProductivityOfflineAction {
    WriteSkill { value: WriteSkill },
    WriteKnowledge { value: WriteKnowledge },
    DeleteSkill { id: String, expected_revision: i64 },
    DeleteKnowledge { id: String, expected_revision: i64 },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteProductivityOfflineOperation {
    pub idempotency_key: String,
    pub project_id: String,
    pub action: ProductivityOfflineAction,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProductivityOfflineStatus {
    Pending,
    Committing,
    Committed,
    Conflict,
    Rejected,
    Uncertain,
    Revoked,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductivityOfflineOperation {
    pub id: String,
    pub device_id: String,
    pub project_id: String,
    pub action: ProductivityOfflineAction,
    pub status: ProductivityOfflineStatus,
    pub revision: i64,
    pub result: Option<Value>,
    pub error_code: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductivityProtocol {
    pub protocol_version: u32,
    pub minimum_version: u32,
    pub maximum_version: u32,
    pub capabilities: Vec<String>,
    pub max_pending_operations: u32,
    pub max_pending_bytes: u32,
    pub android_client_available: bool,
}
pub fn productivity_protocol() -> ProductivityProtocol {
    ProductivityProtocol {
        protocol_version: 1,
        minimum_version: 1,
        maximum_version: 1,
        capabilities: vec![
            "project_content".into(),
            "versioned_zip".into(),
            "revision_sync".into(),
            "explicit_offline_commit".into(),
        ],
        max_pending_operations: 100,
        max_pending_bytes: 4 * 1024 * 1024,
        android_client_available: false,
    }
}
fn offline_state(value: &ProductivityOfflineStatus) -> &'static str {
    match value {
        ProductivityOfflineStatus::Pending => "pending",
        ProductivityOfflineStatus::Committing => "committing",
        ProductivityOfflineStatus::Committed => "committed",
        ProductivityOfflineStatus::Conflict => "conflict",
        ProductivityOfflineStatus::Rejected => "rejected",
        ProductivityOfflineStatus::Uncertain => "uncertain",
        ProductivityOfflineStatus::Revoked => "revoked",
    }
}
async fn read_device(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    device: &str,
) -> Result<ProductivityDevice> {
    let row = sqlx::query("SELECT payload FROM productivity_devices WHERE id=$1")
        .bind(device)
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
    parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)
}
async fn read_operation(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    device: &str,
    operation: &str,
) -> Result<ProductivityOfflineOperation> {
    let row =
        sqlx::query("SELECT payload FROM productivity_offline_queue WHERE device_id=$1 AND id=$2")
            .bind(device)
            .bind(operation)
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?
            .ok_or(Error::NotFound)?;
    parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)
}
async fn save_operation(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    operation: &ProductivityOfflineOperation,
) -> Result<()> {
    sqlx::query("UPDATE productivity_offline_queue SET status=$1,revision=$2,payload=$3 WHERE id=$4 AND device_id=$5").bind(offline_state(&operation.status)).bind(operation.revision).bind(json_string(operation)?).bind(&operation.id).bind(&operation.device_id).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
impl Store {
    pub async fn productivity_devices(&self) -> Result<Vec<ProductivityDevice>> {
        sqlx::query("SELECT payload FROM productivity_devices ORDER BY id LIMIT 65")
            .fetch_all(&self.pool)
            .await
            .map_err(storage_error)?
            .into_iter()
            .map(|r| parse(&r.try_get::<String, _>("payload").map_err(storage_error)?))
            .collect()
    }
    pub async fn productivity_register_device(
        &self,
        input: RegisterProductivityDevice,
    ) -> Result<ProductivityDevice> {
        valid_text(&input.name, 200)?;
        valid_text(&input.idempotency_key, 200)?;
        if input.protocol_version != 1 {
            return Err(Error::Validation(
                "Unsupported device protocol version; negotiate /devices/protocol".into(),
            ));
        }
        let fingerprint = canonical_hash(&input);
        let mut tx = begin_productivity(self).await?;
        if let Some(row) = sqlx::query(
            "SELECT request_hash,payload FROM productivity_devices WHERE idempotency_key=$1",
        )
        .bind(&input.idempotency_key)
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?
        {
            if row
                .try_get::<String, _>("request_hash")
                .map_err(storage_error)?
                != fingerprint
            {
                return Err(Error::Conflict);
            }
            return parse(&row.try_get::<String, _>("payload").map_err(storage_error)?);
        }
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM productivity_devices")
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
        if count >= 64 {
            return Err(Error::Validation(
                "Installation device limit reached".into(),
            ));
        }
        let stamp = now();
        let device = ProductivityDevice {
            id: id(),
            name: input.name.trim().into(),
            platform: input.platform,
            protocol_version: 1,
            revision: 1,
            revoked: false,
            created_at: stamp,
            last_seen_at: stamp,
        };
        sqlx::query("INSERT INTO productivity_devices(id,idempotency_key,request_hash,revoked,payload) VALUES($1,$2,$3,0,$4)").bind(&device.id).bind(input.idempotency_key).bind(fingerprint).bind(json_string(&device)?).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(device)
    }
    pub async fn productivity_revoke_device(&self, key: &str) -> Result<ProductivityDevice> {
        let mut tx = begin_productivity(self).await?;
        let mut device = read_device(&mut tx, key).await?;
        if device.revoked {
            return Ok(device);
        }
        device.revoked = true;
        device.revision += 1;
        device.last_seen_at = now();
        sqlx::query("UPDATE productivity_devices SET revoked=1,payload=$1 WHERE id=$2")
            .bind(json_string(&device)?)
            .bind(key)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        let rows=sqlx::query("SELECT payload FROM productivity_offline_queue WHERE device_id=$1 AND status='pending'").bind(key).fetch_all(&mut *tx).await.map_err(storage_error)?;
        for row in rows {
            let mut operation: ProductivityOfflineOperation =
                parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
            operation.status = ProductivityOfflineStatus::Revoked;
            operation.revision += 1;
            operation.updated_at = now();
            save_operation(&mut tx, &operation).await?;
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(device)
    }
    pub async fn productivity_offline_queue(
        &self,
        device: &str,
    ) -> Result<Vec<ProductivityOfflineOperation>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        read_device(&mut tx, device).await?;
        sqlx::query("SELECT payload FROM productivity_offline_queue WHERE device_id=$1 ORDER BY id LIMIT 501").bind(device).fetch_all(&mut *tx).await.map_err(storage_error)?.into_iter().map(|r|parse(&r.try_get::<String,_>("payload").map_err(storage_error)?)).collect()
    }
    pub async fn productivity_enqueue_offline(
        &self,
        device: &str,
        input: WriteProductivityOfflineOperation,
    ) -> Result<ProductivityOfflineOperation> {
        valid_text(&input.idempotency_key, 200)?;
        match &input.action {
            ProductivityOfflineAction::WriteSkill { value } => validate_skill(value)?,
            ProductivityOfflineAction::WriteKnowledge { value } => validate_knowledge(value)?,
            ProductivityOfflineAction::DeleteSkill {
                id,
                expected_revision,
            }
            | ProductivityOfflineAction::DeleteKnowledge {
                id,
                expected_revision,
            } => {
                valid_text(id, 200)?;
                if *expected_revision < 1 {
                    return Err(Error::Validation("Invalid offline revision".into()));
                }
            }
        }
        let fingerprint = canonical_hash(&input);
        let mut tx = begin_productivity(self).await?;
        let mut current = read_device(&mut tx, device).await?;
        if current.revoked {
            return Err(Error::Forbidden);
        }
        product_project(&mut tx, &input.project_id).await?;
        if let Some(row)=sqlx::query("SELECT request_hash,payload FROM productivity_offline_queue WHERE device_id=$1 AND idempotency_key=$2").bind(device).bind(&input.idempotency_key).fetch_optional(&mut *tx).await.map_err(storage_error)? {
            if row.try_get::<String,_>("request_hash").map_err(storage_error)?!=fingerprint{return Err(Error::Conflict);}
            return parse(&row.try_get::<String,_>("payload").map_err(storage_error)?);
        }
        let rows=sqlx::query("SELECT payload FROM productivity_offline_queue WHERE device_id=$1 AND status IN ('pending','committing','uncertain')").bind(device).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let pending_bytes: usize = rows
            .iter()
            .map(|r| {
                r.try_get::<String, _>("payload")
                    .map(|s| s.len())
                    .map_err(storage_error)
            })
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .sum();
        let stamp = now();
        let operation = ProductivityOfflineOperation {
            id: id(),
            device_id: device.into(),
            project_id: input.project_id,
            action: input.action,
            status: ProductivityOfflineStatus::Pending,
            revision: 1,
            result: None,
            error_code: None,
            created_at: stamp,
            updated_at: stamp,
        };
        let body = json_string(&operation)?;
        let total: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM productivity_offline_queue WHERE device_id=$1",
        )
        .bind(device)
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        if rows.len() >= 100
            || pending_bytes.saturating_add(body.len()) > 4 * 1024 * 1024
            || total >= 500
        {
            return Err(Error::Validation(
                "Device offline queue capacity reached".into(),
            ));
        }
        sqlx::query("INSERT INTO productivity_offline_queue(id,device_id,project_id,idempotency_key,request_hash,status,revision,payload) VALUES($1,$2,$3,$4,$5,'pending',1,$6)").bind(&operation.id).bind(device).bind(&operation.project_id).bind(input.idempotency_key).bind(fingerprint).bind(body).execute(&mut *tx).await.map_err(storage_error)?;
        current.last_seen_at = stamp;
        sqlx::query("UPDATE productivity_devices SET payload=$1 WHERE id=$2")
            .bind(json_string(&current)?)
            .bind(device)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(operation)
    }
    pub async fn productivity_commit_offline(
        &self,
        device: &str,
        key: &str,
        revision: i64,
    ) -> Result<ProductivityOfflineOperation> {
        let mut tx = begin_productivity(self).await?;
        let current = read_device(&mut tx, device).await?;
        if current.revoked {
            return Err(Error::Forbidden);
        }
        let mut operation = read_operation(&mut tx, device, key).await?;
        if operation.status == ProductivityOfflineStatus::Committed {
            return Ok(operation);
        }
        if operation.revision != revision || operation.status != ProductivityOfflineStatus::Pending
        {
            return Err(Error::Conflict);
        }
        operation.status = ProductivityOfflineStatus::Committing;
        operation.revision += 1;
        operation.updated_at = now();
        save_operation(&mut tx, &operation).await?;
        // Queue status, local content and its cursor event share one transaction.
        // No external effect is expressible; a lost commit ACK is resolved by the
        // same queue identity and its already durable committed result.
        sqlx::query("SAVEPOINT offline_content")
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        let outcome: Result<Value> = match operation.action.clone() {
            ProductivityOfflineAction::WriteSkill { value } => {
                write_skill_in_tx(&mut tx, &operation.project_id, value)
                    .await
                    .and_then(|v| serde_json::to_value(v).map_err(storage_error))
            }
            ProductivityOfflineAction::WriteKnowledge { value } => {
                write_knowledge_in_tx(&mut tx, &operation.project_id, value)
                    .await
                    .and_then(|v| serde_json::to_value(v).map_err(storage_error))
            }
            ProductivityOfflineAction::DeleteSkill {
                id,
                expected_revision,
            } => delete_object_in_tx(
                &mut tx,
                &operation.project_id,
                &id,
                "skill",
                expected_revision,
            )
            .await
            .map(|()| json!({"deleted":true})),
            ProductivityOfflineAction::DeleteKnowledge {
                id,
                expected_revision,
            } => delete_object_in_tx(
                &mut tx,
                &operation.project_id,
                &id,
                "knowledge",
                expected_revision,
            )
            .await
            .map(|()| json!({"deleted":true})),
        };
        match outcome {
            Ok(value) => {
                operation.status = ProductivityOfflineStatus::Committed;
                operation.result = Some(value);
                operation.error_code = None;
            }
            Err(Error::Conflict) => {
                sqlx::query("ROLLBACK TO SAVEPOINT offline_content")
                    .execute(&mut *tx)
                    .await
                    .map_err(storage_error)?;
                operation.status = ProductivityOfflineStatus::Conflict;
                operation.error_code = Some("revision_conflict".into());
            }
            Err(Error::Validation(_) | Error::NotFound | Error::Forbidden) => {
                sqlx::query("ROLLBACK TO SAVEPOINT offline_content")
                    .execute(&mut *tx)
                    .await
                    .map_err(storage_error)?;
                operation.status = ProductivityOfflineStatus::Rejected;
                operation.error_code = Some("invalid_or_unavailable_content".into());
            }
            // SQL failures may abort PostgreSQL's whole transaction. Propagate
            // and roll back rather than trying to commit a partial mutation.
            Err(error) => return Err(error),
        }
        sqlx::query("RELEASE SAVEPOINT offline_content")
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        operation.revision += 1;
        operation.updated_at = now();
        save_operation(&mut tx, &operation).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(operation)
    }
    /// Explicitly discard a pending/conflicted/uncertain queue entry after review.
    /// Discarding never reverses an already committed content mutation.
    pub async fn productivity_discard_offline(
        &self,
        device: &str,
        key: &str,
        revision: i64,
    ) -> Result<()> {
        let mut tx = begin_productivity(self).await?;
        read_device(&mut tx, device).await?;
        let operation = read_operation(&mut tx, device, key).await?;
        if operation.revision != revision
            || operation.status == ProductivityOfflineStatus::Committing
        {
            return Err(Error::Conflict);
        }
        sqlx::query("DELETE FROM productivity_offline_queue WHERE device_id=$1 AND id=$2")
            .bind(device)
            .bind(key)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)
    }
}
