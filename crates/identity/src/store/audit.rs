use super::*;
use crate::audit::{self, AuditAuthority, MetadataEvent};
use cowork_contracts::{
    AuditInsights, AuditOrigin, AuditPage, AuditVerification, CaptureAuditArchive,
    PreserveAuditHistory, PreservedAuditHistory, SignedAuditArchive,
};

impl IdentityStore {
    pub async fn audit_trust_prepare(
        &self,
        actor: &Principal,
        org: Option<Uuid>,
        authority: &AuditAuthority,
        input: cowork_contracts::PrepareAuditTrustTransition,
    ) -> Result<cowork_contracts::AuditTrustTransitionProposal> {
        let mut tx = self.begin().await?;
        let scope = self.audit_authorize(&mut tx, actor, org).await?;
        let now = self.sample(&mut tx).await?;
        let value = crate::audit_trust::prepare(
            &mut tx,
            &scope,
            Some(actor.identity_id),
            authority,
            input,
            now,
        )
        .await?;
        self.audit_authorize(&mut tx, actor, org).await?;
        tx.commit().await?;
        Ok(value)
    }
    pub async fn audit_trust_proposal(
        &self,
        actor: &Principal,
        org: Option<Uuid>,
        id: Uuid,
    ) -> Result<cowork_contracts::AuditTrustTransitionProposal> {
        let mut tx = self.begin().await?;
        let scope = self.audit_authorize(&mut tx, actor, org).await?;
        let value = crate::audit_trust::proposal(&mut tx, &scope, id).await?;
        self.audit_authorize(&mut tx, actor, org).await?;
        tx.commit().await?;
        Ok(value)
    }
    pub async fn audit_trust_import(
        &self,
        actor: &Principal,
        org: Option<Uuid>,
        input: cowork_contracts::ImportAuditTrustTransition,
    ) -> Result<cowork_contracts::SignedAuditTrustTransition> {
        let mut tx = self.begin().await?;
        let scope = self.audit_authorize(&mut tx, actor, org).await?;
        let value = crate::audit_trust::import(&mut tx, &scope, input).await?;
        self.audit_authorize(&mut tx, actor, org).await?;
        tx.commit().await?;
        Ok(value)
    }
    pub async fn audit_trust_original(
        &self,
        actor: &Principal,
        org: Option<Uuid>,
        id: Uuid,
    ) -> Result<String> {
        let mut tx = self.begin().await?;
        let scope = self.audit_authorize(&mut tx, actor, org).await?;
        let value = crate::audit_trust::original(&mut tx, &scope, id).await?;
        self.audit_authorize(&mut tx, actor, org).await?;
        tx.commit().await?;
        Ok(value)
    }
    pub async fn audit_witness(
        &self,
        actor: &Principal,
        org: Option<Uuid>,
        authority: &AuditAuthority,
        input: cowork_contracts::ConfigureAuditWitness,
    ) -> Result<cowork_contracts::AuditWitnessBinding> {
        let mut tx = self.begin().await?;
        let scope = self.audit_authorize(&mut tx, actor, org).await?;
        let now = self.sample(&mut tx).await?;
        let value = crate::audit_checkpoints::configure(
            &mut tx,
            &scope,
            Some(actor.identity_id),
            authority,
            input,
            now,
        )
        .await?;
        self.audit_authorize(&mut tx, actor, org).await?;
        tx.commit().await?;
        Ok(value)
    }
    pub async fn audit_checkpoint_capture(
        &self,
        actor: &Principal,
        org: Option<Uuid>,
        authority: &AuditAuthority,
        input: cowork_contracts::CaptureAuditCheckpoint,
    ) -> Result<cowork_contracts::AuditCheckpointRequest> {
        let mut tx = self.begin().await?;
        let scope = self.audit_authorize(&mut tx, actor, org).await?;
        let now = self.sample(&mut tx).await?;
        let value =
            crate::audit_checkpoints::capture(&mut tx, &scope, authority, input, now).await?;
        self.audit_authorize(&mut tx, actor, org).await?;
        tx.commit().await?;
        Ok(value)
    }
    pub async fn audit_checkpoint_request(
        &self,
        actor: &Principal,
        org: Option<Uuid>,
        id: Uuid,
    ) -> Result<cowork_contracts::AuditCheckpointRequest> {
        let mut tx = self.begin().await?;
        let scope = self.audit_authorize(&mut tx, actor, org).await?;
        let value = crate::audit_checkpoints::request(&mut tx, &scope, id).await?;
        self.audit_authorize(&mut tx, actor, org).await?;
        tx.commit().await?;
        Ok(value)
    }
    pub async fn audit_checkpoint_import(
        &self,
        actor: &Principal,
        org: Option<Uuid>,
        input: cowork_contracts::ImportAuditCheckpoint,
    ) -> Result<cowork_contracts::SignedAuditCheckpoint> {
        let mut tx = self.begin().await?;
        let scope = self.audit_authorize(&mut tx, actor, org).await?;
        let value = crate::audit_checkpoints::import(&mut tx, &scope, input).await?;
        self.audit_authorize(&mut tx, actor, org).await?;
        tx.commit().await?;
        Ok(value)
    }
    pub async fn audit_checkpoint_status(
        &self,
        actor: &Principal,
        org: Option<Uuid>,
    ) -> Result<cowork_contracts::AuditCheckpointStatus> {
        let mut tx = self.begin().await?;
        let scope = self.audit_authorize(&mut tx, actor, org).await?;
        let value = crate::audit_checkpoints::status(&mut tx, &scope).await?;
        self.audit_authorize(&mut tx, actor, org).await?;
        tx.commit().await?;
        Ok(value)
    }
    pub async fn audit_checkpoint_history(
        &self,
        actor: &Principal,
        org: Option<Uuid>,
        after: i64,
        limit: i64,
    ) -> Result<Vec<cowork_contracts::SignedAuditCheckpoint>> {
        let mut tx = self.begin().await?;
        let scope = self.audit_authorize(&mut tx, actor, org).await?;
        let value = crate::audit_checkpoints::history(&mut tx, &scope, after, limit).await?;
        self.audit_authorize(&mut tx, actor, org).await?;
        tx.commit().await?;
        Ok(value)
    }
    pub async fn audit_checkpoint_original(
        &self,
        actor: &Principal,
        org: Option<Uuid>,
        id: Uuid,
    ) -> Result<String> {
        let mut tx = self.begin().await?;
        let scope = self.audit_authorize(&mut tx, actor, org).await?;
        let value = crate::audit_checkpoints::original(&mut tx, &scope, id).await?;
        self.audit_authorize(&mut tx, actor, org).await?;
        tx.commit().await?;
        Ok(value)
    }
    pub async fn audit_recovery_plan(
        &self,
        actor: &Principal,
        org: Option<Uuid>,
    ) -> Result<cowork_contracts::AuditRecoveryPlan> {
        let mut tx = self.begin().await?;
        let scope = self.audit_authorize(&mut tx, actor, org).await?;
        let v = crate::audit_recovery::plan(&mut tx, &scope).await?;
        self.audit_authorize(&mut tx, actor, org).await?;
        tx.commit().await?;
        Ok(v)
    }
    pub async fn audit_retire(
        &self,
        actor: &Principal,
        org: Option<Uuid>,
        authority: &AuditAuthority,
        input: cowork_contracts::ConfirmAuditRecovery,
    ) -> Result<cowork_contracts::AuditRecoveryReceipt> {
        let mut tx = self.begin().await?;
        let scope = self.audit_authorize(&mut tx, actor, org).await?;
        let time = self.sample(&mut tx).await?;
        let v = crate::audit_recovery::retire(
            &mut tx,
            &scope,
            Some(actor.identity_id),
            authority,
            input,
            time,
        )
        .await?;
        self.audit_authorize(&mut tx, actor, org).await?;
        tx.commit().await?;
        Ok(v)
    }
    pub async fn audit_retirements(
        &self,
        actor: &Principal,
        org: Option<Uuid>,
    ) -> Result<Vec<cowork_contracts::AuditRecoveryReceipt>> {
        let mut tx = self.begin().await?;
        let scope = self.audit_authorize(&mut tx, actor, org).await?;
        let v = crate::audit_recovery::receipts(&mut tx, &scope).await?;
        self.audit_authorize(&mut tx, actor, org).await?;
        tx.commit().await?;
        Ok(v)
    }
    pub async fn audit_retired_original(
        &self,
        actor: &Principal,
        org: Option<Uuid>,
        id: Uuid,
    ) -> Result<String> {
        let mut tx = self.begin().await?;
        let scope = self.audit_authorize(&mut tx, actor, org).await?;
        let v = crate::audit_recovery::original(&mut tx, &scope, id).await?;
        self.audit_authorize(&mut tx, actor, org).await?;
        tx.commit().await?;
        Ok(v)
    }
    pub(super) async fn audit_backfill(
        &self,
        tx: &mut Transaction<'_, Any>,
        scope: &str,
    ) -> Result<()> {
        let boundary: i64 =
            sqlx::query_scalar("SELECT identity_sequence FROM audit_legacy_boundary WHERE id=1")
                .fetch_one(&mut **tx)
                .await?;
        sqlx::query("INSERT INTO audit_source_cursors(scope,identity_sequence) VALUES($1,0) ON CONFLICT(scope) DO NOTHING").bind(scope).execute(&mut **tx).await?;
        let mut cursor: i64 =
            sqlx::query_scalar("SELECT identity_sequence FROM audit_source_cursors WHERE scope=$1")
                .bind(scope)
                .fetch_one(&mut **tx)
                .await?;
        let (query, id) = if let Some(id) = scope.strip_prefix("organization:") {
            (
                "SELECT * FROM identity_audit WHERE organization_id=$1 AND sequence>$2 AND sequence<=$3 ORDER BY sequence LIMIT 256",
                id,
            )
        } else if let Some(id) = scope.strip_prefix("account:") {
            (
                "SELECT * FROM identity_audit WHERE organization_id IS NULL AND (actor_id=$1 OR target_id=$1) AND sequence>$2 AND sequence<=$3 ORDER BY sequence LIMIT 256",
                id,
            )
        } else {
            return Err(IdentityError::Validation);
        };
        while cursor < boundary {
            let rows = sqlx::query(query)
                .bind(id)
                .bind(cursor)
                .bind(boundary)
                .fetch_all(&mut **tx)
                .await?;
            if rows.is_empty() {
                break;
            }
            for row in rows {
                cursor = row.try_get("sequence")?;
                let event_id: String = row.try_get("id")?;
                if sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM audit_retired_sources WHERE scope=$1 AND event_id=$2",
                )
                .bind(scope)
                .bind(&event_id)
                .fetch_one(&mut **tx)
                .await?
                    != 0
                {
                    continue;
                }
                let action: String = row.try_get("action")?;
                audit::append(
                    tx,
                    scope,
                    MetadataEvent {
                        event_id: &event_id,
                        action: &action,
                        actor_id: Some(row.try_get("actor_id")?),
                        target_id: Some(row.try_get("target_id")?),
                        observed_at: row.try_get("observed_at")?,
                        origin: AuditOrigin::LegacyUnsigned,
                        source_sha256: None,
                    },
                )
                .await?;
            }
        }
        sqlx::query("UPDATE audit_source_cursors SET identity_sequence=$1 WHERE scope=$2")
            .bind(boundary)
            .bind(scope)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }
    async fn audit_authorize(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
        org: Option<Uuid>,
    ) -> Result<String> {
        self.require_global_session_in_transaction(tx, actor)
            .await?;
        self.actor(tx, actor).await?;
        if let Some(org) = org {
            let (_, member) = self.org_member(tx, actor, org).await?;
            if !member.role.manages_members() {
                return Err(IdentityError::Forbidden);
            }
            Ok(format!("organization:{org}"))
        } else {
            Ok(format!("account:{}", actor.identity_id))
        }
    }
    async fn audit_prepare(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
        org: Option<Uuid>,
    ) -> Result<String> {
        let scope = self.audit_authorize(tx, actor, org).await?;
        self.audit_backfill(tx, &scope).await?;
        Ok(scope)
    }
    pub async fn audit_page(
        &self,
        actor: &Principal,
        org: Option<Uuid>,
        after: i64,
        through: Option<i64>,
        limit: i64,
    ) -> Result<AuditPage> {
        let mut tx = self.begin().await?;
        let scope = self.audit_prepare(&mut tx, actor, org).await?;
        let value = audit::page(&mut tx, &scope, after, through, limit).await?;
        self.audit_authorize(&mut tx, actor, org).await?;
        tx.commit().await?;
        Ok(value)
    }
    pub async fn audit_capture(
        &self,
        actor: &Principal,
        org: Option<Uuid>,
        authority: &AuditAuthority,
        input: CaptureAuditArchive,
    ) -> Result<SignedAuditArchive> {
        let mut tx = self.begin().await?;
        let scope = self.audit_prepare(&mut tx, actor, org).await?;
        let observed = self.sample(&mut tx).await?;
        let value = audit::capture(&mut tx, &scope, authority, input, observed).await?;
        self.audit_authorize(&mut tx, actor, org).await?;
        tx.commit().await?;
        Ok(value)
    }
    pub async fn audit_verification(
        &self,
        actor: &Principal,
        org: Option<Uuid>,
    ) -> Result<AuditVerification> {
        let mut tx = self.begin().await?;
        let scope = self.audit_prepare(&mut tx, actor, org).await?;
        let value = audit::verification(&mut tx, &scope).await?;
        self.audit_authorize(&mut tx, actor, org).await?;
        tx.commit().await?;
        Ok(value)
    }
    pub async fn audit_insights(
        &self,
        actor: &Principal,
        org: Option<Uuid>,
        authority: Option<&AuditAuthority>,
    ) -> Result<AuditInsights> {
        self.audit_insights_query(
            actor,
            org,
            authority,
            cowork_contracts::AuditInsightsQuery::default(),
        )
        .await
    }
    pub async fn audit_insights_query(
        &self,
        actor: &Principal,
        org: Option<Uuid>,
        authority: Option<&AuditAuthority>,
        query: cowork_contracts::AuditInsightsQuery,
    ) -> Result<AuditInsights> {
        let mut tx = self.begin().await?;
        let scope = self.audit_prepare(&mut tx, actor, org).await?;
        let mut value = audit::insights(&mut tx, &scope, authority).await?;
        if let Some(org) = org {
            let schema = match self.backend {
                BackendKind::Sqlite => {
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='team_runs'"
                }
                BackendKind::Postgres => {
                    "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema=current_schema() AND table_name='team_runs'"
                }
            };
            let exists: i64 = sqlx::query_scalar(schema).fetch_one(&mut *tx).await?;
            if exists == 1 {
                crate::audit_insights::backfill_team(&mut tx, org).await?;
                value.projects = Some(
                    sqlx::query_scalar(
                        "SELECT COUNT(*) FROM tenant_projects WHERE organization_id=$1",
                    )
                    .bind(org.to_string())
                    .fetch_one(&mut *tx)
                    .await?,
                );
                value.orders = Some(
                    sqlx::query_scalar("SELECT COUNT(*) FROM team_orders WHERE team_id=$1")
                        .bind(org.to_string())
                        .fetch_one(&mut *tx)
                        .await?,
                );
                value.runs = Some(
                    sqlx::query_scalar("SELECT COUNT(*) FROM team_runs WHERE team_id=$1")
                        .bind(org.to_string())
                        .fetch_one(&mut *tx)
                        .await?,
                );
                value.knowledge_documents = Some(
                    sqlx::query_scalar("SELECT COUNT(*) FROM team_documents WHERE team_id=$1")
                        .bind(org.to_string())
                        .fetch_one(&mut *tx)
                        .await?,
                );
                for row in sqlx::query("SELECT status,COUNT(*) AS total FROM team_runs WHERE team_id=$1 GROUP BY status ORDER BY status").bind(org.to_string()).fetch_all(&mut *tx).await? {
                    value.run_states.push(cowork_contracts::AuditCount { category: row.try_get("status")?, count: row.try_get("total")? });
                }
            }
        }
        let observed = self.sample(&mut tx).await?;
        value.execution =
            Some(crate::audit_insights::query(&mut tx, &scope, query, observed).await?);
        self.audit_authorize(&mut tx, actor, org).await?;
        tx.commit().await?;
        Ok(value)
    }
    pub async fn audit_preserve(
        &self,
        actor: &Principal,
        org: Option<Uuid>,
        input: PreserveAuditHistory,
    ) -> Result<PreservedAuditHistory> {
        let mut tx = self.begin().await?;
        let scope = self.audit_prepare(&mut tx, actor, org).await?;
        let observed = self.sample(&mut tx).await?;
        let value = audit::preserve(&mut tx, &scope, input, observed).await?;
        self.audit_authorize(&mut tx, actor, org).await?;
        tx.commit().await?;
        Ok(value)
    }
    pub async fn audit_preserved_history(
        &self,
        actor: &Principal,
        org: Option<Uuid>,
    ) -> Result<Vec<PreservedAuditHistory>> {
        let mut tx = self.begin().await?;
        let scope = self.audit_prepare(&mut tx, actor, org).await?;
        let value = audit::history_receipts(&mut tx, &scope).await?;
        self.audit_authorize(&mut tx, actor, org).await?;
        tx.commit().await?;
        Ok(value)
    }
    /// Explicit original-byte download; never included in diagnostics or insights.
    pub async fn audit_history_bytes(
        &self,
        actor: &Principal,
        org: Option<Uuid>,
        request: Uuid,
    ) -> Result<String> {
        let mut tx = self.begin().await?;
        let scope = self.audit_prepare(&mut tx, actor, org).await?;
        let value = audit::history(&mut tx, &scope, request).await?;
        self.audit_authorize(&mut tx, actor, org).await?;
        tx.commit().await?;
        Ok(value)
    }
}
