//! Private request union. Untagged serialization preserves existing owner custody bytes.
use super::*;
use cowork_workflow_contracts::{
    PrepareImportExecutionPlanV2, PrepareRestoreExecutionPlanV2, RestoreExecutionPreflightV2,
};

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum StoredExecutionRequestV2 {
    Import(Box<PrepareImportExecutionPlanV2>),
    Restore(Box<PrepareRestoreExecutionPlanV2>),
    Owner(Box<PrepareOwnerExecutionPlanV2>),
}
impl From<PrepareOwnerExecutionPlanV2> for StoredExecutionRequestV2 {
    fn from(value: PrepareOwnerExecutionPlanV2) -> Self {
        Self::Owner(Box::new(value))
    }
}
impl From<PrepareRestoreExecutionPlanV2> for StoredExecutionRequestV2 {
    fn from(value: PrepareRestoreExecutionPlanV2) -> Self {
        Self::Restore(Box::new(value))
    }
}
impl From<PrepareImportExecutionPlanV2> for StoredExecutionRequestV2 {
    fn from(value: PrepareImportExecutionPlanV2) -> Self {
        Self::Import(Box::new(value))
    }
}
impl StoredExecutionRequestV2 {
    pub fn owner(&self) -> Option<&PrepareOwnerExecutionPlanV2> {
        match self {
            Self::Owner(value) => Some(value),
            Self::Restore(_) | Self::Import(_) => None,
        }
    }
    pub fn input(&self) -> &ExecutionInputRef {
        match self {
            Self::Owner(value) => &value.input,
            Self::Restore(value) => &value.input,
            Self::Import(value) => &value.input,
        }
    }
    pub fn fields(&self) -> (&str, &str, i64, &str) {
        match self {
            Self::Owner(value) => super::fields(&value.request),
            Self::Restore(value) => {
                let r = &value.request;
                (&r.order_id, &r.idempotency_key, r.base_revision, &r.prompt)
            }
            Self::Import(value) => {
                let r = &value.request;
                (&r.order_id, &r.idempotency_key, r.base_revision, &r.prompt)
            }
        }
    }
    pub(super) fn validate_restore(
        &self,
        saved: &StoredExecutionPlanV2,
        review: &RestoreExecutionPreflightV2,
    ) -> Result<()> {
        let Self::Restore(value) = self else {
            return Err(Error::ExecutionPlanChanged);
        };
        let input = &value.request;
        let spec = &saved.source_spec;
        valid_text(&input.idempotency_key, 128)?;
        valid_text(&input.prompt, 64_000)?;
        let mut restore = input.restore.clone();
        if restore
            .expected_source_sha256
            .as_ref()
            .is_some_and(|sha| sha != &review.content.sha256)
            || !cowork_domain::workspace_transfer::valid_sha256(&review.content.sha256)
        {
            return Err(Error::ExecutionPlanChanged);
        }
        restore.expected_source_sha256 = Some(review.content.sha256.clone());
        let tool = ToolCall {
            id: "restore".into(),
            name: "restore_file".into(),
            arguments: json!(restore),
        };
        if input.base_revision <= 0
            || input.access_mode == AccessMode::ReadOnly
            || spec.prompt != input.prompt
            || spec.access_mode != input.access_mode
            || spec.budgets != Budgets::default()
            || spec.policy_revision != 1
            || spec.allowed_tools != ["restore_file"]
            || spec.provider.is_some()
            || spec.model_binding.is_some()
            || spec.workflow.is_some()
            || spec.agent_config.is_some()
            || canonical_hash(&spec.file_action) != canonical_hash(&Some(tool))
            || !review.ready
            || review.executor != saved.spec.executor
            || review.folders != saved.spec.folders
            || review.allowed_tools != saved.spec.allowed_tools
            || review.access_mode != saved.spec.access_mode
            || review.budgets != saved.spec.budgets
            || canonical_hash(&review.restore) != canonical_hash(&restore)
            || !saved
                .spec
                .folders
                .iter()
                .any(|folder| folder.id == restore.folder_id && folder.writable)
        {
            return Err(Error::ExecutionPlanChanged);
        }
        Ok(())
    }
}

impl Store {
    /// A canceled restore intent fences its key even before input preparation
    /// has returned. No fake model request is used to borrow another namespace.
    pub async fn retire_restore_execution_intent_v2(
        &self,
        input: &CreateRestoreRun,
    ) -> Result<cowork_workflow_contracts::RetiredExecutionRequest> {
        valid_text(&input.idempotency_key, 128)?;
        let owner = self.order(&input.order_id).await?;
        let row = sqlx::query(
            "SELECT * FROM owner_execution_plans_v2 WHERE order_id=$1 AND idempotency_key=$2",
        )
        .bind(&input.order_id)
        .bind(&input.idempotency_key)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_error)?;
        if let Some(row) = row {
            let saved = super::checked(&row)?;
            let StoredExecutionRequestV2::Restore(request) = &saved.request else {
                return Err(Error::Conflict);
            };
            if canonical_hash(&request.request) != canonical_hash(input) {
                return Err(Error::Conflict);
            }
            return self.retire_execution_plan_v2_request(&saved.request).await;
        }
        let identity = json!({"restore_intent":input});
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        workflow_write_keys::lock(
            &mut tx,
            "execution-plan",
            &input.order_id,
            &input.idempotency_key,
            &owner.project_id,
            &identity,
        )
        .await?;
        let runs: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM runs WHERE order_id=$1 AND idempotency_key=$2",
        )
        .bind(&input.order_id)
        .bind(&input.idempotency_key)
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        if runs != 0 {
            return Err(Error::Conflict);
        }
        sqlx::query("UPDATE workflow_write_keys SET state='discarded' WHERE namespace='execution-plan' AND target_id=$1 AND request_key=$2")
            .bind(&input.order_id).bind(&input.idempotency_key).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(cowork_workflow_contracts::RetiredExecutionRequest {
            order_id: input.order_id.clone(),
            idempotency_key: input.idempotency_key.clone(),
            request_sha256: canonical_hash(&identity),
            retired: true,
        })
    }
}
