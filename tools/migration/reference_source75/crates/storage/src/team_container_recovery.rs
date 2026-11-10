//! Recovery admits a separate retirement, never another old tool execution.
use super::*;
use sha2::{Digest, Sha256};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Original {
    input: RetireTeamContainer,
    plan: TeamContainerRecoveryPlan,
}

fn digest(value: &impl Serialize) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).map_err(storage_error)?)
    ))
}
pub fn container_recovery_plan_digest(plan: &TeamContainerRecoveryPlan) -> Result<String> {
    let mut original = plan.clone();
    original.plan_sha256.clear();
    digest(&original)
}
fn sha(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Created only from an authenticated, persisted original admission. HTTP DTOs
/// cannot construct it. The configured runtime supplies the physical evidence.
#[derive(Clone)]
pub struct ContainerRecoveryPermit {
    service: TeamService,
    actor: Principal,
    original: Original,
    request_sha256: String,
}
impl ContainerRecoveryPermit {
    pub fn plan(&self) -> &TeamContainerRecoveryPlan {
        &self.original.plan
    }
    pub fn operation_id(&self) -> Uuid {
        self.original.input.idempotency_key
    }
    pub async fn revalidate(&self, completed: bool) -> Result<()> {
        let mut tx = self
            .service
            .store
            .pool
            .begin()
            .await
            .map_err(storage_error)?;
        self.service
            .container_recovery_authority(
                &mut tx,
                &self.actor,
                self.plan().team_id,
                self.plan().run_id,
            )
            .await?;
        let row = sqlx::query("SELECT actor_id,request_sha256,payload,state FROM team_container_recovery_operations WHERE run_id=$1 AND operation_id=$2")
            .bind(self.plan().run_id.to_string()).bind(self.operation_id().to_string())
            .fetch_one(&mut *tx).await.map_err(storage_error)?;
        if row
            .try_get::<String, _>("actor_id")
            .map_err(storage_error)?
            != self.actor.identity_id().to_string()
            || row
                .try_get::<String, _>("request_sha256")
                .map_err(storage_error)?
                != self.request_sha256
            || row.try_get::<String, _>("payload").map_err(storage_error)?
                != json_string(&self.original)?
            || row.try_get::<String, _>("state").map_err(storage_error)?
                != if completed { "completed" } else { "admitted" }
        {
            return Err(Error::Conflict);
        }
        tx.commit().await.map_err(storage_error)
    }
}

/// Not deserializable: only the operator-configured runtime returns this.
pub struct ContainerClosedEvidence {
    pub plan_sha256: String,
    pub record_sha256: String,
    pub container_id: String,
    pub host_id: Uuid,
    pub engine_id: String,
}

impl TeamService {
    pub(super) async fn container_recovery_authority(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
        team: Uuid,
        run: Uuid,
    ) -> Result<TeamRun> {
        let installation =
            sqlx::query("UPDATE identity_installation SET revision=revision WHERE id=1")
                .execute(&mut **tx)
                .await
                .map_err(storage_error)?;
        if installation.rows_affected() != 1 {
            return Err(Error::Forbidden);
        }
        sqlx::query("UPDATE team_runs SET lease_until=lease_until WHERE id=$1 AND team_id=$2")
            .bind(run.to_string())
            .bind(team.to_string())
            .execute(&mut **tx)
            .await
            .map_err(storage_error)?;
        let (value, _) = self.recovery_access(tx, actor, team, run, false).await?;
        self.recovery_owner(tx, actor, team, run).await?;
        let permit = self
            .project_permit(tx, actor, team, value.project_id, ContentAction::Execute)
            .await?;
        self.store
            .identity()
            .validate_content_in_transaction(tx, &permit)
            .await
            .map_err(identity_error)?;
        let row = sqlx::query("SELECT lease_owner,lease_until FROM team_runs WHERE id=$1")
            .bind(run.to_string())
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
        let current = db_now(&self.store, tx).await?;
        if !matches!(
            value.status,
            TeamRunStatus::Unknown
                | TeamRunStatus::Cancelled
                | TeamRunStatus::Completed
                | TeamRunStatus::Failed
        ) || row
            .try_get::<Option<String>, _>("lease_owner")
            .map_err(storage_error)?
            .is_some()
            || row
                .try_get::<i64, _>("lease_until")
                .map_err(storage_error)?
                > current
        {
            return Err(Error::Conflict);
        }
        Ok(value)
    }

    pub async fn container_recovery_plans(
        &self,
        actor: &Principal,
        team: Uuid,
        run: Uuid,
    ) -> Result<Vec<TeamContainerRecoveryPlan>> {
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        let value = self
            .container_recovery_authority(&mut tx, actor, team, run)
            .await?;
        tx.commit().await.map_err(storage_error)?;
        let mut plans = self
            .executor
            .container_recovery_plans(team, value.order_id, run)
            .await?;
        if plans.len() > 64 {
            return Err(Error::BudgetExceeded);
        }
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        self.container_recovery_authority(&mut tx, actor, team, run)
            .await?;
        let revision: i64 =
            sqlx::query_scalar("SELECT revision FROM team_container_recovery_runs WHERE run_id=$1")
                .bind(run.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?
                .unwrap_or(0);
        for plan in &mut plans {
            if plan.team_id != team
                || plan.run_id != run
                || plan.order_id != value.order_id
                || plan.nonce.is_nil()
                || plan.host_id.is_nil()
                || !sha(&plan.record_sha256)
                || !sha(&plan.container_id)
                || plan.engine_id.is_empty()
                || plan.engine_id.len() > 128
                || !matches!(plan.record_format, 1 | 2)
                || plan.legacy_location_adoption != (plan.record_format == 1)
                || plan.legacy_location_adoption && !plan.container_present
            {
                return Err(Error::UncertainEffect);
            }
            plan.revision = revision;
            plan.plan_sha256 = container_recovery_plan_digest(plan)?;
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(plans)
    }

    pub async fn retire_container(
        &self,
        actor: &Principal,
        team: Uuid,
        run: Uuid,
        input: RetireTeamContainer,
    ) -> Result<TeamContainerRecoveryReceipt> {
        if input.idempotency_key.is_nil()
            || input.nonce.is_nil()
            || input.expected_revision < 0
            || !sha(&input.plan_sha256)
        {
            return Err(invalid());
        }
        let request_sha256 = digest(&input)?;
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        let value = self
            .container_recovery_authority(&mut tx, actor, team, run)
            .await?;
        let old = sqlx::query("SELECT actor_id,request_sha256,payload,receipt,state FROM team_container_recovery_operations WHERE run_id=$1 AND operation_id=$2")
            .bind(run.to_string()).bind(input.idempotency_key.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        let (original, receipt) = if let Some(row) = old {
            if row
                .try_get::<String, _>("actor_id")
                .map_err(storage_error)?
                != actor.identity_id().to_string()
                || row
                    .try_get::<String, _>("request_sha256")
                    .map_err(storage_error)?
                    != request_sha256
            {
                return Err(Error::Conflict);
            }
            if row.try_get::<String, _>("state").map_err(storage_error)? == "rejected" {
                return Err(Error::Conflict);
            }
            let original: Original =
                parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
            let receipt = row
                .try_get::<Option<String>, _>("receipt")
                .map_err(storage_error)?
                .as_deref()
                .map(parse::<TeamContainerRecoveryReceipt>)
                .transpose()?;
            tx.commit().await.map_err(storage_error)?;
            (original, receipt)
        } else {
            tx.commit().await.map_err(storage_error)?;
            let plan = self
                .container_recovery_plans(actor, team, run)
                .await?
                .into_iter()
                .find(|p| p.nonce == input.nonce)
                .ok_or(Error::NotFound)?;
            if plan.plan_sha256 != input.plan_sha256
                || plan.revision != input.expected_revision
                || plan.legacy_location_adoption && !input.allow_legacy_location_adoption
            {
                return Err(Error::Conflict);
            }
            let original = Original {
                input: input.clone(),
                plan,
            };
            let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
            let fresh = self
                .container_recovery_authority(&mut tx, actor, team, run)
                .await?;
            if fresh.order_id != value.order_id {
                return Err(Error::Conflict);
            }
            sqlx::query("INSERT INTO team_container_recovery_runs(run_id,revision) VALUES($1,0) ON CONFLICT(run_id) DO NOTHING")
                .bind(run.to_string()).execute(&mut *tx).await.map_err(storage_error)?;
            if sqlx::query("UPDATE team_container_recovery_runs SET revision=revision+1 WHERE run_id=$1 AND revision=$2")
                .bind(run.to_string()).bind(input.expected_revision).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1
            { return Err(Error::Conflict); }
            let current = db_now(&self.store, &mut tx).await?;
            sqlx::query("INSERT INTO team_container_recovery_operations(run_id,operation_id,actor_id,request_sha256,plan_sha256,state,payload,created_at) VALUES($1,$2,$3,$4,$5,'admitted',$6,$7)")
                .bind(run.to_string()).bind(input.idempotency_key.to_string()).bind(actor.identity_id().to_string())
                .bind(&request_sha256).bind(&input.plan_sha256).bind(json_string(&original)?).bind(current)
                .execute(&mut *tx).await.map_err(storage_error)?;
            self.store
                .identity()
                .validate_principal_in_transaction(&mut tx, actor)
                .await
                .map_err(identity_error)?;
            tx.commit().await.map_err(storage_error)?;
            (original, None)
        };
        let permit = ContainerRecoveryPermit {
            service: self.clone(),
            actor: actor.clone(),
            original,
            request_sha256,
        };
        let receipt = if let Some(receipt) = receipt {
            receipt
        } else {
            permit.revalidate(false).await?;
            let evidence = self.executor.retire_container(permit.clone()).await?;
            let plan = permit.plan();
            if evidence.plan_sha256 != plan.plan_sha256
                || evidence.record_sha256 != plan.record_sha256
                || evidence.container_id != plan.container_id
                || evidence.host_id != plan.host_id
                || evidence.engine_id != plan.engine_id
            {
                return Err(Error::UncertainEffect);
            }
            // Original physical facts may be recorded after revocation. They
            // confer no new execution or record-closing authority.
            let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
            sqlx::query(
                "UPDATE team_container_recovery_runs SET revision=revision WHERE run_id=$1",
            )
            .bind(run.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
            let row = sqlx::query("SELECT payload,request_sha256,receipt FROM team_container_recovery_operations WHERE run_id=$1 AND operation_id=$2")
                .bind(run.to_string()).bind(input.idempotency_key.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
            if row.try_get::<String, _>("payload").map_err(storage_error)?
                != json_string(&permit.original)?
                || row
                    .try_get::<String, _>("request_sha256")
                    .map_err(storage_error)?
                    != permit.request_sha256
            {
                return Err(Error::Conflict);
            }
            let receipt = if let Some(raw) = row
                .try_get::<Option<String>, _>("receipt")
                .map_err(storage_error)?
            {
                parse(&raw)?
            } else {
                let revision: i64 = sqlx::query_scalar(
                    "SELECT revision FROM team_container_recovery_runs WHERE run_id=$1",
                )
                .bind(run.to_string())
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
                let next = revision.checked_add(1).ok_or(Error::Conflict)?;
                let receipt = TeamContainerRecoveryReceipt {
                    operation_id: input.idempotency_key,
                    run_id: run,
                    nonce: plan.nonce,
                    revision: next,
                    request_sha256: permit.request_sha256.clone(),
                    plan_sha256: plan.plan_sha256.clone(),
                    record_sha256: plan.record_sha256.clone(),
                    container_id: plan.container_id.clone(),
                    host_id: plan.host_id,
                    engine_id: plan.engine_id.clone(),
                    legacy_location_adoption: plan.legacy_location_adoption,
                    container_removed: true,
                    effects_unknown: true,
                };
                sqlx::query("UPDATE team_container_recovery_operations SET state='completed',receipt=$1 WHERE run_id=$2 AND operation_id=$3 AND state='admitted' AND receipt IS NULL")
                    .bind(json_string(&receipt)?).bind(run.to_string()).bind(input.idempotency_key.to_string())
                    .execute(&mut *tx).await.map_err(storage_error)?;
                sqlx::query("UPDATE team_container_recovery_runs SET revision=$1 WHERE run_id=$2")
                    .bind(next)
                    .bind(run.to_string())
                    .execute(&mut *tx)
                    .await
                    .map_err(storage_error)?;
                receipt
            };
            tx.commit().await.map_err(storage_error)?;
            receipt
        };
        permit.revalidate(true).await?;
        self.executor
            .close_container_record(permit, receipt.clone())
            .await?;
        Ok(receipt)
    }

    pub async fn container_recovery_receipt(
        &self,
        actor: &Principal,
        team: Uuid,
        run: Uuid,
        operation: Uuid,
    ) -> Result<Option<TeamContainerRecoveryReceipt>> {
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        self.recovery_access(&mut tx, actor, team, run, false)
            .await?;
        let raw: Option<String> = sqlx::query_scalar("SELECT receipt FROM team_container_recovery_operations WHERE run_id=$1 AND operation_id=$2 AND actor_id=$3")
            .bind(run.to_string()).bind(operation.to_string()).bind(actor.identity_id().to_string())
            .fetch_optional(&mut *tx).await.map_err(storage_error)?.flatten();
        let result = raw.as_deref().map(parse).transpose()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }

    pub async fn reject_container_recovery(
        &self,
        actor: &Principal,
        team: Uuid,
        run: Uuid,
        input: RetireTeamContainer,
    ) -> Result<Value> {
        if input.idempotency_key.is_nil()
            || input.nonce.is_nil()
            || input.expected_revision < 0
            || !sha(&input.plan_sha256)
        {
            return Err(invalid());
        }
        let request = digest(&input)?;
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        sqlx::query("UPDATE team_runs SET lease_until=lease_until WHERE id=$1 AND team_id=$2")
            .bind(run.to_string())
            .bind(team.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        self.recovery_access(&mut tx, actor, team, run, false)
            .await?;
        self.recovery_owner(&mut tx, actor, team, run).await?;
        let row = sqlx::query("SELECT actor_id,request_sha256,state FROM team_container_recovery_operations WHERE run_id=$1 AND operation_id=$2")
            .bind(run.to_string()).bind(input.idempotency_key.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        if let Some(row) = row {
            if row
                .try_get::<String, _>("actor_id")
                .map_err(storage_error)?
                != actor.identity_id().to_string()
                || row
                    .try_get::<String, _>("request_sha256")
                    .map_err(storage_error)?
                    != request
                || row.try_get::<String, _>("state").map_err(storage_error)? != "rejected"
            {
                return Err(Error::Conflict);
            }
        } else {
            let current = db_now(&self.store, &mut tx).await?;
            sqlx::query("INSERT INTO team_container_recovery_operations(run_id,operation_id,actor_id,request_sha256,plan_sha256,state,payload,created_at) VALUES($1,$2,$3,$4,$5,'rejected',$6,$7)")
                .bind(run.to_string()).bind(input.idempotency_key.to_string()).bind(actor.identity_id().to_string())
                .bind(&request).bind(&input.plan_sha256).bind(json_string(&input)?).bind(current).execute(&mut *tx).await.map_err(storage_error)?;
        }
        self.store
            .identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(
            json!({"operation_id":input.idempotency_key,"run_id":run,"request_sha256":request,"state":"rejected"}),
        )
    }
}
