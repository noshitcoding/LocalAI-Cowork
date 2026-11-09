//! Durable, one-shot control-plane operations. A lost runner reply never grants
//! another dispatch. The same workspace gate is used by normal run admission.
use super::*;

#[derive(Clone, Debug)]
pub struct SandboxOperationRecord {
    pub id: String,
    pub resource_id: String,
    pub request: Value,
    pub result: Option<Value>,
    pub state: String,
}

pub(super) async fn lock_workspace(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    resource: &str,
) -> Result<()> {
    sqlx::query("INSERT INTO sandbox_workspace_guards(resource_id,revision) VALUES($1,1) ON CONFLICT(resource_id) DO UPDATE SET revision=sandbox_workspace_guards.revision+1")
        .bind(resource).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}

/// Called in the transaction that acquires folder_owners. It serializes that
/// admission with a lifecycle intent and blocks uncertain mutating operations.
pub(super) async fn admit_workspace(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    resource: &str,
) -> Result<()> {
    lock_workspace(tx, resource).await?;
    let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sandbox_operations WHERE resource_id=$1 AND operation <> 'inspect' AND state IN ('pending','dispatched')")
        .bind(resource).fetch_one(&mut **tx).await.map_err(storage_error)?;
    if pending != 0 {
        return Err(Error::UncertainEffect);
    }
    Ok(())
}

fn read(row: &sqlx::any::AnyRow) -> Result<SandboxOperationRecord> {
    let request: String = row.try_get("request").map_err(storage_error)?;
    let result: Option<String> = row.try_get("result").map_err(storage_error)?;
    Ok(SandboxOperationRecord {
        id: row.try_get("id").map_err(storage_error)?,
        resource_id: row.try_get("resource_id").map_err(storage_error)?,
        request: parse(&request)?,
        result: result.map(|v| parse(&v)).transpose()?,
        state: row.try_get("state").map_err(storage_error)?,
    })
}

impl Store {
    /// Recovery reads the original intent even after its environment is revoked
    /// or the broker filesystem is unavailable. It cannot authorize a dispatch.
    pub async fn sandbox_operation_by_key(
        &self,
        project: &str,
        order: &str,
        key: &str,
        request_hash: &str,
    ) -> Result<Option<SandboxOperationRecord>> {
        let row=sqlx::query("SELECT id,resource_id,request,result,state,request_hash FROM sandbox_operations WHERE project_id=$1 AND order_id=$2 AND idempotency_key=$3")
            .bind(project).bind(order).bind(key).fetch_optional(&self.pool).await.map_err(storage_error)?;
        row.map(|row| {
            if row
                .try_get::<String, _>("request_hash")
                .map_err(storage_error)?
                != request_hash
            {
                return Err(Error::Conflict);
            }
            read(&row)
        })
        .transpose()
    }
    pub async fn sandbox_operation_history(
        &self,
        project: &str,
        order: &str,
    ) -> Result<Vec<(String, String, String, Value)>> {
        // Keep unresolved mutations visible even after arbitrarily many later
        // inspections. Only recent terminal/read-only history is bounded.
        let rows=sqlx::query("SELECT id,idempotency_key,state,request,sequence FROM sandbox_operations WHERE project_id=$1 AND order_id=$2 AND operation <> 'inspect' AND state IN ('pending','dispatched') UNION ALL SELECT id,idempotency_key,state,request,sequence FROM (SELECT id,idempotency_key,state,request,sequence FROM sandbox_operations WHERE project_id=$1 AND order_id=$2 AND NOT (operation <> 'inspect' AND state IN ('pending','dispatched')) ORDER BY sequence DESC LIMIT 100) AS recent ORDER BY sequence DESC")
            .bind(project).bind(order).fetch_all(&self.pool).await.map_err(storage_error)?;
        rows.iter()
            .map(|r| {
                Ok((
                    r.try_get("id").map_err(storage_error)?,
                    r.try_get("idempotency_key").map_err(storage_error)?,
                    r.try_get("state").map_err(storage_error)?,
                    parse(&r.try_get::<String, _>("request").map_err(storage_error)?)?,
                ))
            })
            .collect()
    }
    pub async fn sandbox_session_result(
        &self,
        project: &str,
        order: &str,
    ) -> Result<Option<Value>> {
        // Inspection may finish while a preceding Stop is still terminating.
        // It never grants execution authority or overrides a lifecycle transition.
        let result:Option<String>=sqlx::query_scalar("SELECT result FROM sandbox_operations WHERE project_id=$1 AND order_id=$2 AND state='completed' AND operation <> 'inspect' ORDER BY sequence DESC LIMIT 1")
            .bind(project).bind(order).fetch_optional(&self.pool).await.map_err(storage_error)?;
        result.map(|v| parse(&v)).transpose()
    }
    #[allow(clippy::too_many_arguments)]
    pub async fn prepare_sandbox_operation(
        &self,
        project: &str,
        order: &str,
        resource: &str,
        key: &str,
        request_hash: &str,
        operation: SandboxLifecycleOperation,
        request: &Value,
    ) -> Result<SandboxOperationRecord> {
        valid_text(key, 128)?;
        if !cowork_domain::processes::valid_sha256(request_hash)
            || json_string(request)?.len() > 65_536
        {
            return Err(Error::Forbidden);
        }
        let operation_name = serde_json::to_value(operation).map_err(storage_error)?;
        let operation_name = operation_name.as_str().ok_or(Error::Forbidden)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        lock_workspace(&mut tx, resource).await?;
        let owned: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM orders WHERE id=$1 AND project_id=$2")
                .bind(order)
                .bind(project)
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        if owned != 1 {
            return Err(Error::NotFound);
        }
        if let Some(row)=sqlx::query("SELECT id,resource_id,request,result,state,request_hash FROM sandbox_operations WHERE project_id=$1 AND order_id=$2 AND idempotency_key=$3")
            .bind(project).bind(order).bind(key).fetch_optional(&mut *tx).await.map_err(storage_error)? {
            let prior_hash:String=row.try_get("request_hash").map_err(storage_error)?;
            if prior_hash!=request_hash {return Err(Error::Conflict);}
            let record=read(&row)?;tx.commit().await.map_err(storage_error)?;return Ok(record);
        }
        if operation != SandboxLifecycleOperation::Inspect {
            let owners: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM folder_owners WHERE resource_id=$1")
                    .bind(resource)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(storage_error)?;
            let pending:i64=sqlx::query_scalar("SELECT COUNT(*) FROM sandbox_operations WHERE resource_id=$1 AND operation <> 'inspect' AND state IN ('pending','dispatched')")
                .bind(resource).fetch_one(&mut *tx).await.map_err(storage_error)?;
            if owners != 0 || pending != 0 {
                return Err(Error::Conflict);
            }
        }
        let id = id();
        let sequence: i64 = sqlx::query_scalar(
            "SELECT revision FROM sandbox_workspace_guards WHERE resource_id=$1",
        )
        .bind(resource)
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        sqlx::query("INSERT INTO sandbox_operations(id,project_id,order_id,resource_id,idempotency_key,request_hash,operation,state,request,sequence,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,'pending',$8,$9,$10)")
            .bind(&id).bind(project).bind(order).bind(resource).bind(key).bind(request_hash)
            .bind(operation_name).bind(json_string(request)?).bind(sequence).bind(now()).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(SandboxOperationRecord {
            id,
            resource_id: resource.into(),
            request: request.clone(),
            result: None,
            state: "pending".into(),
        })
    }

    pub async fn dispatch_sandbox_operation(&self, id: &str) -> Result<bool> {
        Ok(sqlx::query(
            "UPDATE sandbox_operations SET state='dispatched' WHERE id=$1 AND state='pending'",
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(storage_error)?
        .rows_affected()
            == 1)
    }

    /// A pending intent has never granted transport dispatch. Expiring it is
    /// safe; dispatched operations can never be cleared this way.
    pub async fn expire_undispatched_sandbox_operation(&self, id: &str) -> Result<bool> {
        Ok(sqlx::query(
            "UPDATE sandbox_operations SET state='expired' WHERE id=$1 AND state='pending'",
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(storage_error)?
        .rows_affected()
            == 1)
    }

    pub async fn complete_sandbox_operation(&self, id: &str, result: &Value) -> Result<()> {
        if json_string(result)?.len() > 65_536 {
            return Err(Error::Forbidden);
        }
        let changed=sqlx::query("UPDATE sandbox_operations SET state='completed',result=$1 WHERE id=$2 AND state='dispatched'")
            .bind(json_string(result)?).bind(id).execute(&self.pool).await.map_err(storage_error)?.rows_affected();
        if changed != 1 {
            let prior: Option<String> = sqlx::query_scalar(
                "SELECT result FROM sandbox_operations WHERE id=$1 AND state='completed'",
            )
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(storage_error)?;
            if prior.as_deref() != Some(json_string(result)?.as_str()) {
                return Err(Error::Conflict);
            }
        }
        Ok(())
    }

    /// Only a definitive response to the original POST may prove non-admission.
    /// An absent later receipt or a disconnected transport cannot call this.
    pub async fn reject_unadmitted_sandbox_operation(&self, id: &str) -> Result<()> {
        if sqlx::query(
            "UPDATE sandbox_operations SET state='rejected' WHERE id=$1 AND state='dispatched'",
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(storage_error)?
        .rows_affected()
            != 1
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn contract(store: Store) {
        let project = store
            .create_project(CreateProject {
                name: "Sandbox lifecycle".into(),
            })
            .await
            .unwrap();
        let order = store
            .create_order(CreateOrder {
                project_id: project.id.clone(),
                title: "Workspace".into(),
            })
            .await
            .unwrap();
        let resource = id();
        let hash = "a".repeat(64);
        let request = json!({"payload":{"request_id":"original"},"signature":[]});
        let first = store
            .prepare_sandbox_operation(
                &project.id,
                &order.id,
                &resource,
                "key",
                &hash,
                SandboxLifecycleOperation::Ensure,
                &request,
            )
            .await
            .unwrap();
        let replay = store
            .prepare_sandbox_operation(
                &project.id,
                &order.id,
                &resource,
                "key",
                &hash,
                SandboxLifecycleOperation::Ensure,
                &json!({"new_clock":"must_not_replace_original"}),
            )
            .await
            .unwrap();
        assert_eq!(first.id, replay.id);
        assert_eq!(replay.request, request);
        let lookup = store
            .sandbox_operation_by_key(&project.id, &order.id, "key", &hash)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(lookup.resource_id, resource);
        assert_eq!(lookup.request, request);
        assert!(
            store
                .sandbox_operation_by_key(&project.id, &order.id, "key", &"b".repeat(64))
                .await
                .is_err()
        );
        assert!(
            store
                .sandbox_operation_by_key("foreign", &order.id, "key", &hash)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .prepare_sandbox_operation(
                    &project.id,
                    &order.id,
                    &resource,
                    "key",
                    &"b".repeat(64),
                    SandboxLifecycleOperation::Ensure,
                    &request
                )
                .await
                .is_err()
        );
        let mut tx = store.pool.begin().await.unwrap();
        assert!(admit_workspace(&mut tx, &resource).await.is_err());
        tx.rollback().await.unwrap();
        let (a, b) = tokio::join!(
            store.dispatch_sandbox_operation(&first.id),
            store.dispatch_sandbox_operation(&first.id)
        );
        assert_eq!(u8::from(a.unwrap()) + u8::from(b.unwrap()), 1);
        assert!(
            !store
                .expire_undispatched_sandbox_operation(&first.id)
                .await
                .unwrap()
        );
        // Read-only inspections must not hide the still-dispatched original
        // intent needed to recover its receipt in a new browser session.
        for n in 0..101 {
            let inspection = store
                .prepare_sandbox_operation(
                    &project.id,
                    &order.id,
                    &resource,
                    &format!("history-{n}"),
                    &hash,
                    SandboxLifecycleOperation::Inspect,
                    &request,
                )
                .await
                .unwrap();
            assert!(
                store
                    .dispatch_sandbox_operation(&inspection.id)
                    .await
                    .unwrap()
            );
            store
                .complete_sandbox_operation(&inspection.id, &json!({"state":"ready"}))
                .await
                .unwrap();
        }
        let history = store
            .sandbox_operation_history(&project.id, &order.id)
            .await
            .unwrap();
        assert_eq!(history.len(), 101);
        assert!(
            history
                .iter()
                .any(|(id, _, state, _)| id == &first.id && state == "dispatched")
        );
        assert!(
            store
                .prepare_sandbox_operation(
                    &project.id,
                    &order.id,
                    &resource,
                    "other",
                    &hash,
                    SandboxLifecycleOperation::Resume,
                    &request
                )
                .await
                .is_err()
        );
        // An explicit inspection remains possible while a mutation is uncertain.
        let inspection = store
            .prepare_sandbox_operation(
                &project.id,
                &order.id,
                &resource,
                "inspection",
                &hash,
                SandboxLifecycleOperation::Inspect,
                &request,
            )
            .await
            .unwrap();
        assert!(
            store
                .expire_undispatched_sandbox_operation(&inspection.id)
                .await
                .unwrap()
        );
        store
            .complete_sandbox_operation(&first.id, &json!({"signed_result":"original"}))
            .await
            .unwrap();
        let complete = store
            .prepare_sandbox_operation(
                &project.id,
                &order.id,
                &resource,
                "key",
                &hash,
                SandboxLifecycleOperation::Ensure,
                &request,
            )
            .await
            .unwrap();
        assert_eq!(complete.result, Some(json!({"signed_result":"original"})));
        assert!(!store.dispatch_sandbox_operation(&first.id).await.unwrap());
        store
            .complete_sandbox_operation(&first.id, &json!({"signed_result":"original"}))
            .await
            .unwrap();
        assert!(
            store
                .complete_sandbox_operation(&first.id, &json!({"signed_result":"different"}))
                .await
                .is_err()
        );
        // A later inspection can observe Ready while an earlier Stop is still
        // terminating. Its sequence must never supersede the completed Stop.
        let stop = store
            .prepare_sandbox_operation(
                &project.id,
                &order.id,
                &resource,
                "stop",
                &hash,
                SandboxLifecycleOperation::Stop,
                &request,
            )
            .await
            .unwrap();
        let inspect = store
            .prepare_sandbox_operation(
                &project.id,
                &order.id,
                &resource,
                "during-stop",
                &hash,
                SandboxLifecycleOperation::Inspect,
                &request,
            )
            .await
            .unwrap();
        assert!(store.dispatch_sandbox_operation(&inspect.id).await.unwrap());
        store
            .complete_sandbox_operation(&inspect.id, &json!({"state":"ready"}))
            .await
            .unwrap();
        assert!(store.dispatch_sandbox_operation(&stop.id).await.unwrap());
        store
            .complete_sandbox_operation(&stop.id, &json!({"state":"stopped"}))
            .await
            .unwrap();
        assert_eq!(
            store
                .sandbox_session_result(&project.id, &order.id)
                .await
                .unwrap(),
            Some(json!({"state":"stopped"}))
        );
        let mut tx = store.pool.begin().await.unwrap();
        admit_workspace(&mut tx, &resource).await.unwrap();
        tx.rollback().await.unwrap();
        let other = store
            .create_project(CreateProject {
                name: "Other owner".into(),
            })
            .await
            .unwrap();
        assert!(
            store
                .prepare_sandbox_operation(
                    &other.id,
                    &order.id,
                    &resource,
                    "foreign",
                    &hash,
                    SandboxLifecycleOperation::Inspect,
                    &request
                )
                .await
                .is_err()
        );
        // Competing fresh mutating intents for one workspace admit exactly one.
        let (a, b) = tokio::join!(
            store.prepare_sandbox_operation(
                &project.id,
                &order.id,
                &resource,
                "a",
                &hash,
                SandboxLifecycleOperation::Ensure,
                &request
            ),
            store.prepare_sandbox_operation(
                &project.id,
                &order.id,
                &resource,
                "b",
                &hash,
                SandboxLifecycleOperation::Ensure,
                &request
            )
        );
        assert_ne!(a.is_ok(), b.is_ok());
    }
    #[tokio::test]
    async fn sqlite_one_dispatch_and_workspace_ownership() {
        let dir = tempfile::tempdir().unwrap();
        contract(Store::sqlite(&dir.path().join("sandbox.db")).await.unwrap()).await;
    }
    #[tokio::test]
    #[ignore = "requires dedicated COWORK_TEST_POSTGRES"]
    async fn postgres_one_dispatch_and_workspace_ownership() {
        contract(
            Store::connect(&std::env::var("COWORK_TEST_POSTGRES").unwrap())
                .await
                .unwrap(),
        )
        .await;
    }
}
