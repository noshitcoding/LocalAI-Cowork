//! Personal scope only. Tenant audit authority is checked in cowork-identity.
use super::*;
use cowork_identity::audit::{self, AuditAuthority, MetadataEvent};
use sqlx::{Any, Transaction};

fn identity_error(error: cowork_identity::IdentityError) -> Error {
    Error::Storage(Box::new(error))
}
pub(crate) async fn record_event(
    tx: &mut Transaction<'_, Any>,
    event: &RunEvent,
    origin: AuditOrigin,
) -> Result<()> {
    // This allowlist prevents externally supplied event text becoming diagnostics.
    let action = match event.kind.as_str() {
        "run_created" | "run_started" | "run_completed" | "run_failed" | "run_cancelled"
        | "approval_requested" | "approval_resolved" | "model_requested" | "model_completed"
        | "tool_requested" | "tool_completed" | "tool_failed" | "input_requested"
        | "input_submitted" | "file_written" | "artifact_created" | "context_compacted" => {
            event.kind.as_str()
        }
        _ => "runtime_event",
    };
    let source = json_string(&(
        event.id.as_str(),
        event.run_id.as_str(),
        event.sequence,
        event.created_at,
    ))?;
    audit::append(
        tx,
        "personal",
        MetadataEvent {
            event_id: &event.id,
            action,
            actor_id: None,
            target_id: Uuid::parse_str(&event.run_id).ok().map(|v| v.to_string()),
            observed_at: event.created_at,
            origin: origin.clone(),
            source_sha256: Some(audit::hash(source.as_bytes())),
        },
    )
    .await
    .map_err(identity_error)?;
    if origin == AuditOrigin::NativeV2 {
        crate::audit_execution::personal_event(tx, event).await?;
    }
    Ok(())
}
async fn prepare(tx: &mut Transaction<'_, Any>) -> Result<()> {
    // Read immutable source records in bounded pages. The migration's frozen ID
    // set distinguishes actual old rows from all events subsequently appended.
    loop {
        let rows = sqlx::query("SELECT e.payload FROM events e JOIN audit_personal_legacy_events l ON l.event_id=e.id WHERE NOT EXISTS(SELECT 1 FROM audit_records a WHERE a.scope='personal' AND a.event_id=e.id) AND NOT EXISTS(SELECT 1 FROM audit_retired_sources r WHERE r.scope='personal' AND r.event_id=e.id) ORDER BY e.run_id,e.sequence LIMIT 256")
            .fetch_all(&mut **tx).await.map_err(storage_error)?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            let event: RunEvent =
                parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
            record_event(tx, &event, AuditOrigin::LegacyUnsigned).await?;
        }
    }
    Ok(())
}
impl Store {
    pub async fn personal_audit_trust_prepare(
        &self,
        authority: &AuditAuthority,
        input: PrepareAuditTrustTransition,
    ) -> Result<AuditTrustTransitionProposal> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let value = cowork_identity::audit_trust::prepare(
            &mut tx,
            "personal",
            None,
            authority,
            input,
            now(),
        )
        .await
        .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn personal_audit_trust_proposal(
        &self,
        id: Uuid,
    ) -> Result<AuditTrustTransitionProposal> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let value = cowork_identity::audit_trust::proposal(&mut tx, "personal", id)
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn personal_audit_trust_import(
        &self,
        input: ImportAuditTrustTransition,
    ) -> Result<SignedAuditTrustTransition> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let value = cowork_identity::audit_trust::import(&mut tx, "personal", input)
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn personal_audit_trust_original(&self, id: Uuid) -> Result<String> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let value = cowork_identity::audit_trust::original(&mut tx, "personal", id)
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn personal_audit_witness(
        &self,
        authority: &AuditAuthority,
        input: ConfigureAuditWitness,
    ) -> Result<AuditWitnessBinding> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let value = cowork_identity::audit_checkpoints::configure(
            &mut tx,
            "personal",
            None,
            authority,
            input,
            now(),
        )
        .await
        .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn personal_audit_checkpoint_capture(
        &self,
        authority: &AuditAuthority,
        input: CaptureAuditCheckpoint,
    ) -> Result<AuditCheckpointRequest> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let value = cowork_identity::audit_checkpoints::capture(
            &mut tx,
            "personal",
            authority,
            input,
            now(),
        )
        .await
        .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn personal_audit_checkpoint_request(
        &self,
        id: Uuid,
    ) -> Result<AuditCheckpointRequest> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let value = cowork_identity::audit_checkpoints::request(&mut tx, "personal", id)
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn personal_audit_checkpoint_import(
        &self,
        input: ImportAuditCheckpoint,
    ) -> Result<SignedAuditCheckpoint> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let value = cowork_identity::audit_checkpoints::import(&mut tx, "personal", input)
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn personal_audit_checkpoint_status(&self) -> Result<AuditCheckpointStatus> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let value = cowork_identity::audit_checkpoints::status(&mut tx, "personal")
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn personal_audit_checkpoint_history(
        &self,
        after: i64,
        limit: i64,
    ) -> Result<Vec<SignedAuditCheckpoint>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let value = cowork_identity::audit_checkpoints::history(&mut tx, "personal", after, limit)
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn personal_audit_checkpoint_original(&self, id: Uuid) -> Result<String> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let value = cowork_identity::audit_checkpoints::original(&mut tx, "personal", id)
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn personal_audit_verify_v1(
        &self,
        authority: &AuditAuthority,
        input: VerifyAuditV1,
    ) -> Result<AuditV1Verification> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let value = cowork_identity::audit_v1::capture(&mut tx, authority, input, now())
            .await
            .map_err(|e| {
                if matches!(e, cowork_identity::IdentityError::NativeAuditKeyUnavailable) {
                    Error::NativeKeyUnavailable
                } else {
                    identity_error(e)
                }
            })?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn personal_audit_recovery_plan(&self) -> Result<AuditRecoveryPlan> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let v = cowork_identity::audit_recovery::plan(&mut tx, "personal")
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn personal_audit_retire(
        &self,
        authority: &AuditAuthority,
        input: ConfirmAuditRecovery,
    ) -> Result<AuditRecoveryReceipt> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let v = cowork_identity::audit_recovery::retire(
            &mut tx,
            "personal",
            None,
            authority,
            input,
            now(),
        )
        .await
        .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn personal_audit_retired_original(&self, id: Uuid) -> Result<String> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let v = cowork_identity::audit_recovery::original(&mut tx, "personal", id)
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn personal_audit_retirements(&self) -> Result<Vec<AuditRecoveryReceipt>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let v = cowork_identity::audit_recovery::receipts(&mut tx, "personal")
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn personal_audit_page(
        &self,
        after: i64,
        through: Option<i64>,
        limit: i64,
    ) -> Result<AuditPage> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        prepare(&mut tx).await?;
        let value = audit::page(&mut tx, "personal", after, through, limit)
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn personal_audit_capture(
        &self,
        authority: &AuditAuthority,
        input: CaptureAuditArchive,
    ) -> Result<SignedAuditArchive> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        prepare(&mut tx).await?;
        let value = audit::capture(&mut tx, "personal", authority, input, now())
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn personal_audit_insights(
        &self,
        authority: Option<&AuditAuthority>,
    ) -> Result<AuditInsights> {
        self.personal_audit_insights_query(authority, AuditInsightsQuery::default())
            .await
    }
    pub async fn personal_audit_insights_query(
        &self,
        authority: Option<&AuditAuthority>,
        query: AuditInsightsQuery,
    ) -> Result<AuditInsights> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        prepare(&mut tx).await?;
        crate::audit_execution::backfill_personal(&mut tx).await?;
        let mut value = audit::insights(&mut tx, "personal", authority)
            .await
            .map_err(identity_error)?;
        value.execution = Some(
            cowork_identity::audit_insights::query(&mut tx, "personal", query, now())
                .await
                .map_err(identity_error)?,
        );
        value.projects = Some(
            sqlx::query_scalar("SELECT COUNT(*) FROM projects")
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?,
        );
        value.orders = Some(
            sqlx::query_scalar("SELECT COUNT(*) FROM orders")
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?,
        );
        value.runs = Some(
            sqlx::query_scalar("SELECT COUNT(*) FROM runs")
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?,
        );
        value.messages = Some(
            sqlx::query_scalar("SELECT COUNT(*) FROM messages")
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?,
        );
        value.knowledge_documents = Some(
            sqlx::query_scalar("SELECT COUNT(*) FROM productivity_objects WHERE kind='knowledge'")
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?,
        );
        value.skills = Some(
            sqlx::query_scalar("SELECT COUNT(*) FROM productivity_objects WHERE kind='skill'")
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?,
        );
        for row in
            sqlx::query("SELECT state,COUNT(*) AS total FROM runs GROUP BY state ORDER BY state")
                .fetch_all(&mut *tx)
                .await
                .map_err(storage_error)?
        {
            value.run_states.push(AuditCount {
                category: row.try_get("state").map_err(storage_error)?,
                count: row.try_get("total").map_err(storage_error)?,
            });
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn personal_audit_preserve(
        &self,
        input: PreserveAuditHistory,
    ) -> Result<PreservedAuditHistory> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        prepare(&mut tx).await?;
        let value = audit::preserve(&mut tx, "personal", input, now())
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn personal_audit_history(&self) -> Result<Vec<PreservedAuditHistory>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let value = audit::history_receipts(&mut tx, "personal")
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn personal_audit_original(&self, request: Uuid) -> Result<String> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let value = audit::history(&mut tx, "personal", request)
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
}
