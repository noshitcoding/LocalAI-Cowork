use super::*;
use cowork_workflow_contracts::ImportExecutionPreflightV2;

impl StoredExecutionRequestV2 {
    pub(super) fn validate_import(
        &self,
        saved: &StoredExecutionPlanV2,
        review: &ImportExecutionPreflightV2,
    ) -> Result<()> {
        let Self::Import(value) = self else {
            return Err(Error::ExecutionPlanChanged);
        };
        let input = &value.request;
        let spec = &saved.source_spec;
        let upload = &review.upload;
        valid_text(&input.idempotency_key, 128)?;
        valid_text(&input.prompt, 64_000)?;
        let tool = ToolCall {
            id: "import".into(),
            name: "import_file".into(),
            arguments: json!(input.import),
        };
        if input.base_revision <= 0
            || input.access_mode == AccessMode::ReadOnly
            || spec.prompt != input.prompt
            || spec.access_mode != input.access_mode
            || spec.budgets != Budgets::default()
            || spec.policy_revision != 1
            || spec.allowed_tools != ["import_file"]
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
            || canonical_hash(&review.import) != canonical_hash(&input.import)
            || upload.state != UploadState::Ready
            || upload.order_id != input.order_id
            || upload.project_id != spec.project_id
            || upload.id != input.import.upload_id
            || canonical_hash(&upload.file) != canonical_hash(&input.import.file)
            || !saved.spec.folders.iter().any(|f| {
                f.id == input.import.folder_id
                    && f.writable
                    && f.order_id.as_ref() == Some(&input.order_id)
            })
        {
            return Err(Error::ExecutionPlanChanged);
        }
        Ok(())
    }
}

impl Store {
    /// The original import intent can be retired before its input response arrives.
    pub async fn retire_import_execution_intent_v2(
        &self,
        input: &CreateImportRun,
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
            let StoredExecutionRequestV2::Import(request) = &saved.request else {
                return Err(Error::Conflict);
            };
            if canonical_hash(&request.request) != canonical_hash(input) {
                return Err(Error::Conflict);
            }
            return self.retire_execution_plan_v2_request(&saved.request).await;
        }
        let identity = json!({"import_intent":input});
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
