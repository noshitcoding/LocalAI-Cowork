//! Private server-agent resume. No caller-supplied execution state or spec.
use axum::{
    extract::{Extension, Path, State},
    http::{header::AUTHORIZATION, HeaderMap},
    Json,
};
use cowork_contracts::{ExecutorTarget, ProjectRole, RunEventKind, RunRecord, RunState};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::{
    auth::Principal,
    db,
    error::ApiError,
    organization,
    runner_admission::{ExpectedAttempt, StopRequest, VerifiedStop},
    AppState,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResumeRequest {
    pub request_id: Uuid,
    pub expected_revision: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ResumeReceipt {
    pub request_id: Uuid,
    pub run_id: Uuid,
    pub checkpoint_id: Uuid,
    pub queued_revision: i64,
}

struct Snapshot {
    run: RunRecord,
    checkpoint_id: Uuid,
    checkpoint: Value,
    ledger: Value,
    stops: Vec<StopRequest>,
}

// Never serialize the supplied token or its digest into a checkpoint/receipt.
struct Authority {
    principal: Principal,
    token_hash: [u8; 32],
}

impl Authority {
    fn from_headers(principal: Principal, headers: &HeaderMap) -> Result<Self, ApiError> {
        let mut values = headers.get_all(AUTHORIZATION).iter();
        let supplied = values.next().ok_or_else(conflict)?;
        if values.next().is_some() || supplied.as_bytes().len() > 4096 {
            return Err(conflict());
        }
        let token = supplied
            .to_str()
            .ok()
            .and_then(|value| value.strip_prefix("Bearer "))
            .filter(|token| !token.is_empty())
            .ok_or_else(conflict)?;
        Ok(Self {
            principal,
            token_hash: crate::auth::opaque_token_hash(token),
        })
    }
}

fn conflict() -> ApiError {
    ApiError::Conflict("agent resume requires an unchanged trusted safe checkpoint and verified prior execution stop".into())
}

async fn current_auth_tx(
    tx: &mut Transaction<'_, Postgres>,
    authority: &Authority,
    run: &RunRecord,
) -> Result<(), ApiError> {
    let principal = &authority.principal;
    let session = principal
        .session_id
        .filter(|_| !principal.bootstrap)
        .ok_or_else(conflict)?;
    if principal.user_id != run.spec.creator_user_id {
        return Err(conflict());
    }
    let current: Option<Uuid> = sqlx::query_scalar("SELECT s.id FROM access_tokens a JOIN auth_sessions s ON s.id=a.session_id AND s.user_id=a.user_id JOIN users u ON u.id=a.user_id WHERE a.token_hash=$1 AND a.session_id=$2 AND a.user_id=$3 AND a.revoked_at IS NULL AND a.expires_at>clock_timestamp() AND s.revoked_at IS NULL AND s.expires_at>clock_timestamp() AND u.deleted_at IS NULL FOR SHARE OF a,s,u")
        .bind(authority.token_hash.as_slice()).bind(session).bind(principal.user_id).fetch_optional(&mut **tx).await?;
    if current.is_none() {
        return Err(conflict());
    }
    let project = sqlx::query(
        "SELECT owner_user_id,team_id FROM projects WHERE id=$1 AND deleted_at IS NULL FOR SHARE",
    )
    .bind(run.spec.project_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(conflict)?;
    let thread: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM threads WHERE id=$1 AND project_id=$2 AND deleted_at IS NULL FOR SHARE",
    )
    .bind(run.spec.thread_id)
    .bind(run.spec.project_id)
    .fetch_optional(&mut **tx)
    .await?;
    if thread.is_none() {
        return Err(conflict());
    }
    if project.try_get::<Uuid, _>("owner_user_id")? == principal.user_id {
        return Ok(());
    }
    let project_role: Option<String> = sqlx::query_scalar(
        "SELECT role FROM project_members WHERE project_id=$1 AND user_id=$2 FOR SHARE",
    )
    .bind(run.spec.project_id)
    .bind(principal.user_id)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(role) = project_role {
        return if matches!(role.as_str(), "runner" | "editor") {
            Ok(())
        } else {
            Err(conflict())
        };
    }
    if let Some(team) = project.try_get::<Option<Uuid>, _>("team_id")? {
        let role: Option<String> = sqlx::query_scalar(
            "SELECT role FROM team_members WHERE team_id=$1 AND user_id=$2 FOR SHARE",
        )
        .bind(team)
        .bind(principal.user_id)
        .fetch_optional(&mut **tx)
        .await?;
        if role.is_some_and(|role| matches!(role.as_str(), "owner" | "admin")) {
            return Ok(());
        }
    }
    Err(conflict())
}

async fn prior_receipt(
    tx: &mut Transaction<'_, Postgres>,
    run: Uuid,
    principal: &Principal,
    request: &ResumeRequest,
) -> Result<Option<ResumeReceipt>, ApiError> {
    let imported: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM run_instance_import_origins WHERE run_id=$1)",
    )
    .bind(run)
    .fetch_one(&mut **tx)
    .await?;
    if imported {
        return Err(conflict());
    }
    let row = sqlx::query("SELECT run_id,user_id,checkpoint_id,source_revision,queued_revision FROM run_resume_requests WHERE request_id=$1")
        .bind(request.request_id).fetch_optional(&mut **tx).await?;
    row.map(|row| {
        if row.try_get::<Uuid, _>("run_id")? != run
            || row.try_get::<Uuid, _>("user_id")? != principal.user_id
            || row.try_get::<i64, _>("source_revision")? != request.expected_revision
        {
            return Err(conflict());
        }
        Ok(ResumeReceipt {
            request_id: request.request_id,
            run_id: run,
            checkpoint_id: row.try_get("checkpoint_id")?,
            queued_revision: row.try_get("queued_revision")?,
        })
    })
    .transpose()
}

async fn attempts_tx(
    tx: &mut Transaction<'_, Postgres>,
    run: Uuid,
    lease: Uuid,
) -> Result<Vec<ExpectedAttempt>, ApiError> {
    let rows = sqlx::query("SELECT id,lease_token,request_sha256,workspace_id FROM run_runner_attempts WHERE run_id=$1 AND lease_token=$2 ORDER BY id LIMIT 2049")
        .bind(run).bind(lease).fetch_all(&mut **tx).await?;
    if rows.len() > 2048 {
        return Err(conflict());
    }
    rows.iter()
        .map(|row| {
            Ok(ExpectedAttempt {
                attempt_id: row.try_get("id")?,
                lease_token: row.try_get("lease_token")?,
                request_sha256: row.try_get("request_sha256")?,
                workspace_id: row.try_get("workspace_id")?,
            })
        })
        .collect()
}

async fn snapshot_tx(
    tx: &mut Transaction<'_, Postgres>,
    row: &sqlx::postgres::PgRow,
) -> Result<Snapshot, ApiError> {
    let run = db::row_to_run(row)?;
    if run.state != RunState::Interrupted
        || !matches!(run.spec.executor_target, ExecutorTarget::ServerLinux { .. })
        || row
            .try_get::<Option<Uuid>, _>("runner_cancel_id")?
            .is_some()
        || row.try_get::<Option<Uuid>, _>("lease_token")?.is_some()
        || row.try_get::<Option<Uuid>, _>("lease_owner")?.is_some()
    {
        return Err(conflict());
    }
    let checkpoint = sqlx::query("SELECT id,origin,safe_to_resume,executor_state,server_execution_attempt_id FROM run_checkpoints WHERE run_id=$1 ORDER BY sequence DESC LIMIT 1")
        .bind(run.spec.id).fetch_optional(&mut **tx).await?.ok_or_else(conflict)?;
    if checkpoint.try_get::<String, _>("origin")? != "server_agent_v1"
        || !checkpoint.try_get::<bool, _>("safe_to_resume")?
    {
        return Err(conflict());
    }
    let state: Value = checkpoint.try_get("executor_state")?;
    cowork_runtime::checkpoint::validate_for_run(&run.spec, &state).map_err(|_| conflict())?;
    let rows = sqlx::query("SELECT (to_jsonb(a)-'stop_receipt')||jsonb_build_object('stop_verified',stop_receipt IS NOT NULL) AS row,id,lease_token FROM run_server_execution_attempts a WHERE run_id=$1 ORDER BY id LIMIT 513")
        .bind(run.spec.id).fetch_all(&mut **tx).await?;
    if rows.is_empty()
        || rows.len() > 512
        || !rows.iter().any(|row| {
            row.get::<Uuid, _>("id") == checkpoint.get::<Uuid, _>("server_execution_attempt_id")
        })
    {
        return Err(conflict());
    }
    let unknown: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM run_runner_attempts r WHERE r.run_id=$1 AND NOT EXISTS(SELECT 1 FROM run_server_execution_attempts a WHERE a.run_id=r.run_id AND a.lease_token=r.lease_token)) OR EXISTS(SELECT 1 FROM desktop_sessions WHERE run_id=$1 AND state IN ('starting','agent_controlled','user_controlled','paused')) OR EXISTS(SELECT 1 FROM instance_execution_reservations WHERE run_id=$1 AND completion IS NOT NULL)")
        .bind(run.spec.id).fetch_one(&mut **tx).await?;
    if unknown {
        return Err(conflict());
    }
    let mut stops = Vec::new();
    let mut ledger = Vec::new();
    for row in rows {
        let lease = row.try_get("lease_token")?;
        stops.push(StopRequest {
            version: 1,
            run_id: run.spec.id,
            lease_token: Some(lease),
            cancel_lease: None,
            attempts: attempts_tx(tx, run.spec.id, lease).await?,
        });
        ledger.push(row.try_get::<Value, _>("row")?);
    }
    if serde_json::to_vec(&stops)?.len() > cowork_runtime::checkpoint::MAX_CHECKPOINT_BYTES {
        return Err(conflict());
    }
    Ok(Snapshot {
        run,
        checkpoint_id: checkpoint.try_get("id")?,
        checkpoint: state,
        ledger: Value::Array(ledger),
        stops,
    })
}

async fn begin(
    pool: &PgPool,
    run: Uuid,
    authority: &Authority,
    request: &ResumeRequest,
) -> Result<Result<Snapshot, ResumeReceipt>, ApiError> {
    let principal = &authority.principal;
    let mut tx = pool.begin().await?;
    let row = sqlx::query("SELECT * FROM runs WHERE id=$1 FOR UPDATE")
        .bind(run)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(conflict)?;
    let record = db::row_to_run(&row)?;
    current_auth_tx(&mut tx, authority, &record).await?;
    if let Some(receipt) = prior_receipt(&mut tx, run, principal, request).await? {
        tx.commit().await?;
        return Ok(Err(receipt));
    }
    if record.revision != request.expected_revision {
        return Err(conflict());
    }
    let snapshot = snapshot_tx(&mut tx, &row).await?;
    tx.commit().await?;
    Ok(Ok(snapshot))
}

async fn finish_tx(
    tx: &mut Transaction<'_, Postgres>,
    authority: &Authority,
    request: &ResumeRequest,
    snapshot: &Snapshot,
    proofs: &[VerifiedStop],
) -> Result<ResumeReceipt, ApiError> {
    let principal = &authority.principal;
    let row = sqlx::query("SELECT * FROM runs WHERE id=$1 FOR UPDATE")
        .bind(snapshot.run.spec.id)
        .fetch_one(&mut **tx)
        .await?;
    let run = db::row_to_run(&row)?;
    current_auth_tx(tx, authority, &run).await?;
    if let Some(receipt) = prior_receipt(tx, run.spec.id, principal, request).await? {
        return Ok(receipt);
    }
    if run.revision != request.expected_revision
        || serde_json::to_value(&run.spec)? != serde_json::to_value(&snapshot.run.spec)?
    {
        return Err(conflict());
    }
    let current = snapshot_tx(tx, &row).await?;
    if current.checkpoint_id != snapshot.checkpoint_id
        || current.checkpoint != snapshot.checkpoint
        || current.ledger != snapshot.ledger
        || current.stops.len() != snapshot.stops.len()
        || proofs.len() != current.stops.len()
    {
        return Err(conflict());
    }
    for ((current, original), proof) in current.stops.iter().zip(&snapshot.stops).zip(proofs) {
        if serde_json::to_value(current)? != serde_json::to_value(original)?
            || !proof.matches(run.spec.id, current.lease_token, None, &current.attempts)
        {
            return Err(conflict());
        }
    }
    for (stop, proof) in current.stops.iter().zip(proofs) {
        sqlx::query("UPDATE run_server_execution_attempts SET stop_receipt=$3 WHERE run_id=$1 AND lease_token=$2 AND stop_receipt IS NULL")
            .bind(run.spec.id).bind(stop.lease_token).bind(proof.receipt()).execute(&mut **tx).await?;
    }
    let revision = run.revision.checked_add(1).ok_or_else(conflict)?;
    sqlx::query("UPDATE runs SET state='queued',revision=$2,server_resume_checkpoint_id=$3,error=NULL,result=NULL,assigned_executor_id=NULL,finished_at=NULL,updated_at=clock_timestamp() WHERE id=$1")
        .bind(run.spec.id).bind(revision).bind(current.checkpoint_id).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO run_resume_requests(request_id,run_id,user_id,checkpoint_id,source_revision,queued_revision) VALUES($1,$2,$3,$4,$5,$6)")
        .bind(request.request_id).bind(run.spec.id).bind(principal.user_id).bind(current.checkpoint_id).bind(run.revision).bind(revision).execute(&mut **tx).await?;
    db::append_event_tx(tx, run.spec.id, RunEventKind::StateChanged, json!({"from":"interrupted","to":"queued",
        "reason":"trusted_agent_checkpoint_resume","checkpoint_id":current.checkpoint_id,"request_id":request.request_id})).await?;
    crate::organisation_mfa::bind_resume(
        tx,
        &run,
        principal.session_id.ok_or_else(conflict)?,
        request.request_id,
        revision,
    )
    .await?;
    // Retain admission charges. This same logical run has not completed.
    Ok(ResumeReceipt {
        request_id: request.request_id,
        run_id: run.spec.id,
        checkpoint_id: current.checkpoint_id,
        queued_revision: revision,
    })
}

pub async fn resume_agent_run(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
    headers: HeaderMap,
    Json(request): Json<ResumeRequest>,
) -> Result<Json<ResumeReceipt>, ApiError> {
    let authority = Authority::from_headers(principal, &headers)?;
    let principal = &authority.principal;
    if request.request_id.is_nil() || request.expected_revision < 1 || state.management_mode {
        return Err(conflict());
    }
    state
        .ensure_api_active()
        .await
        .map_err(ApiError::Internal)?;
    let run = db::get_run(&state.pool, run_id).await?;
    organization::ensure_thread_role(
        &state.pool,
        principal.user_id,
        run.spec.project_id,
        run.spec.thread_id,
        ProjectRole::Runner,
    )
    .await?;
    let snapshot = match begin(&state.pool, run_id, &authority, &request).await? {
        Ok(value) => value,
        Err(receipt) => return Ok(Json(receipt)),
    };
    let runner = state.runner.as_deref().ok_or_else(conflict)?;
    let proofs = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        let mut proofs = Vec::new();
        let mut bytes = 0_usize;
        for stop in &snapshot.stops {
            let proof = runner.stop_execution(stop).await.map_err(|_| conflict())?;
            bytes = bytes
                .checked_add(serde_json::to_vec(proof.receipt())?.len())
                .ok_or_else(conflict)?;
            if bytes > cowork_runtime::checkpoint::MAX_CHECKPOINT_BYTES {
                return Err(conflict());
            }
            proofs.push(proof);
        }
        Ok::<_, ApiError>(proofs)
    })
    .await
    .map_err(|_| conflict())??;
    state
        .ensure_api_active()
        .await
        .map_err(ApiError::Internal)?;
    organization::ensure_thread_role(
        &state.pool,
        principal.user_id,
        run.spec.project_id,
        run.spec.thread_id,
        ProjectRole::Runner,
    )
    .await?;
    let mut tx = state.pool.begin().await?;
    let receipt = finish_tx(&mut tx, &authority, &request, &snapshot, &proofs).await?;
    tx.commit().await?;
    Ok(Json(receipt))
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use cowork_runtime::{
        AgentRuntime, ModelConfig, RuntimeHost, ToolDefinition, ToolInvocation, ToolOutput,
    };
    use std::sync::Mutex;

    #[test]
    fn private_resume_dto_rejects_spec_and_checkpoint_overrides() {
        let request = json!({"request_id":Uuid::new_v4(),"expected_revision":2});
        assert!(serde_json::from_value::<ResumeRequest>(request.clone()).is_ok());
        for field in [
            "spec",
            "checkpoint",
            "checkpoint_id",
            "model",
            "tools",
            "input",
            "origin",
        ] {
            let mut changed = request.clone();
            changed[field] = json!({});
            assert!(serde_json::from_value::<ResumeRequest>(changed).is_err());
        }
    }

    #[test]
    fn resume_authority_requires_one_bounded_exact_bearer_token() {
        fn principal() -> Principal {
            Principal {
                user_id: Uuid::new_v4(),
                session_id: Some(Uuid::new_v4()),
                bootstrap: false,
            }
        }
        let mut headers = HeaderMap::new();
        assert!(Authority::from_headers(principal(), &headers).is_err());
        headers.insert(AUTHORIZATION, "Bearer fixture-secret".parse().unwrap());
        let authority = Authority::from_headers(principal(), &headers).unwrap();
        assert_eq!(
            authority.token_hash,
            crate::auth::opaque_token_hash("fixture-secret")
        );
        headers.append(AUTHORIZATION, "Bearer second-secret".parse().unwrap());
        assert!(Authority::from_headers(principal(), &headers).is_err());
        headers.clear();
        headers.insert(AUTHORIZATION, "Bearer ".parse().unwrap());
        assert!(Authority::from_headers(principal(), &headers).is_err());
        headers.insert(
            AUTHORIZATION,
            format!("Bearer {}", "a".repeat(4096)).parse().unwrap(),
        );
        assert!(Authority::from_headers(principal(), &headers).is_err());
    }

    struct Capture(Mutex<Option<Value>>);
    #[async_trait]
    impl RuntimeHost for Capture {
        fn tools(&self) -> Vec<ToolDefinition> {
            Vec::new()
        }
        async fn emit(&self, _: RunEventKind, _: Value) -> anyhow::Result<()> {
            anyhow::bail!("no model request permitted")
        }
        async fn execute_tool(&self, _: ToolInvocation) -> anyhow::Result<ToolOutput> {
            anyhow::bail!("no tool permitted")
        }
        async fn checkpoint(&self, state: Value, safe: bool) -> anyhow::Result<()> {
            assert!(safe);
            *self.0.lock().unwrap() = Some(state);
            anyhow::bail!("capture before first model request")
        }
    }

    async fn fixture(pool: PgPool) {
        let user = Uuid::new_v4();
        let session = Uuid::new_v4();
        let project = Uuid::new_v4();
        let thread = Uuid::new_v4();
        let run_id = Uuid::new_v4();
        let worker = Uuid::new_v4();
        let lease = Uuid::new_v4();
        let spec = cowork_contracts::RunSpec {
            schema_version: cowork_contracts::SCHEMA_VERSION,
            id: run_id,
            thread_id: thread,
            project_id: project,
            project: cowork_contracts::FrozenReference {
                id: project,
                revision: 1,
            },
            project_privacy: cowork_contracts::ProjectPrivacy::TeamManaged,
            task: None,
            creator_user_id: user,
            executor_target: ExecutorTarget::ServerLinux { pool_id: None },
            required_capabilities: Vec::new(),
            input: json!({"prompt":"isolated resume"}),
            model_profile_id: None,
            snapshot_id: None,
            idempotency_key: Uuid::new_v4().to_string(),
            created_at: chrono::Utc::now(),
        };
        sqlx::query("INSERT INTO users(id) VALUES($1)")
            .bind(user)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO auth_sessions(id,user_id,expires_at) VALUES($1,$2,clock_timestamp()+interval '1 hour')")
            .bind(session).bind(user).execute(&pool).await.unwrap();
        let token_hash = crate::auth::opaque_token_hash("private-resume-fixture-token");
        sqlx::query("INSERT INTO access_tokens(token_hash,session_id,user_id,expires_at) VALUES($1,$2,$3,clock_timestamp()+interval '1 hour')")
            .bind(token_hash.as_slice()).bind(session).bind(user).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO projects(id,owner_user_id) VALUES($1,$2)")
            .bind(project)
            .bind(user)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO threads(id,project_id) VALUES($1,$2)")
            .bind(thread)
            .bind(project)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO runs(id,spec,state,lease_owner,lease_token,lease_expires_at) VALUES($1,$2,'running',$3,$4,clock_timestamp()+interval '1 hour')")
            .bind(run_id).bind(serde_json::to_value(&spec).unwrap()).bind(worker).bind(lease).execute(&pool).await.unwrap();
        assert!(
            db::begin_server_agent_execution(&pool, run_id, worker, lease)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            db::begin_server_agent_execution(&pool, run_id, worker, lease)
                .await
                .is_err()
        );
        let capture = Capture(Mutex::new(None));
        let runtime = AgentRuntime::new(ModelConfig {
            base_url: "http://127.0.0.1:9/v1".into(),
            api_key: None,
            model: "test".into(),
            timeout: std::time::Duration::from_secs(1),
            max_steps: 3,
            verify_tls_certificates: true,
        })
        .unwrap();
        assert!(runtime.execute(&spec, &capture).await.is_err());
        let state = capture.0.lock().unwrap().take().unwrap();
        let checkpoint = crate::workflow::create_agent_worker_checkpoint(
            &pool,
            worker,
            run_id,
            lease,
            true,
            state.clone(),
        )
        .await
        .unwrap();
        sqlx::query("INSERT INTO run_runner_attempts(id,run_id,lease_token,request_sha256) VALUES($1,$2,$3,$4)")
            .bind(Uuid::new_v4()).bind(run_id).bind(lease).bind("a".repeat(64)).execute(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO instance_execution_reservations(run_id,completion) VALUES($1,NULL)",
        )
        .bind(run_id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("UPDATE runs SET state='interrupted',lease_owner=NULL,lease_token=NULL,lease_expires_at=NULL WHERE id=$1")
            .bind(run_id).execute(&pool).await.unwrap();
        let principal = Authority {
            token_hash,
            principal: Principal {
                user_id: user,
                session_id: Some(session),
                bootstrap: false,
            },
        };
        let request = ResumeRequest {
            request_id: Uuid::new_v4(),
            expected_revision: 1,
        };
        for invalid in [
            "UPDATE auth_sessions SET revoked_at=clock_timestamp()",
            "UPDATE access_tokens SET revoked_at=clock_timestamp()",
            "UPDATE access_tokens SET expires_at=clock_timestamp()-interval '1 second'",
            "UPDATE access_tokens SET user_id=gen_random_uuid()",
            "UPDATE runs SET runner_cancel_id=gen_random_uuid()",
            "UPDATE runs SET state='running'",
            "UPDATE instance_execution_reservations SET completion='{}'::jsonb",
        ] {
            sqlx::raw_sql(invalid).execute(&pool).await.unwrap();
            assert!(begin(&pool, run_id, &principal, &request).await.is_err());
            sqlx::raw_sql("UPDATE auth_sessions SET revoked_at=NULL;UPDATE access_tokens SET revoked_at=NULL,expires_at=clock_timestamp()+interval '1 hour';UPDATE runs SET runner_cancel_id=NULL,state='interrupted';UPDATE instance_execution_reservations SET completion=NULL")
                .execute(&pool).await.unwrap();
            sqlx::query("UPDATE access_tokens SET user_id=$1")
                .bind(user)
                .execute(&pool)
                .await
                .unwrap();
        }
        for safe in [true, false] {
            sqlx::query("INSERT INTO run_checkpoints(id,run_id,sequence,safe_to_resume,executor_state) VALUES($1,$2,2,$3,$4)")
                .bind(Uuid::new_v4()).bind(run_id).bind(safe).bind(&state).execute(&pool).await.unwrap();
            assert!(begin(&pool, run_id, &principal, &request).await.is_err());
            sqlx::query("DELETE FROM run_checkpoints WHERE sequence=2")
                .execute(&pool)
                .await
                .unwrap();
        }
        let snapshot = begin(&pool, run_id, &principal, &request)
            .await
            .unwrap()
            .ok()
            .unwrap();
        assert_eq!(snapshot.checkpoint_id, checkpoint.id);
        // A still-valid session cannot authorize a token revoked/expired during
        // the physical stop. The same final TX must reject before queueing.
        for invalid in [
            "UPDATE access_tokens SET revoked_at=clock_timestamp()",
            "UPDATE access_tokens SET expires_at=clock_timestamp()-interval '1 second'",
        ] {
            sqlx::raw_sql(invalid).execute(&pool).await.unwrap();
            let mut tx = pool.begin().await.unwrap();
            let proof = crate::runner_admission::verified_stop_for_test(&snapshot.stops[0]);
            assert!(
                finish_tx(&mut tx, &principal, &request, &snapshot, &[proof])
                    .await
                    .is_err()
            );
            tx.rollback().await.unwrap();
            assert_eq!(
                sqlx::query_scalar::<_, String>("SELECT state FROM runs WHERE id=$1")
                    .bind(run_id)
                    .fetch_one(&pool)
                    .await
                    .unwrap(),
                "interrupted"
            );
            sqlx::raw_sql("UPDATE access_tokens SET revoked_at=NULL,expires_at=clock_timestamp()+interval '1 hour'")
                .execute(&pool).await.unwrap();
        }
        let wrong = StopRequest {
            lease_token: Some(Uuid::new_v4()),
            ..snapshot.stops[0].clone()
        };
        let mut tx = pool.begin().await.unwrap();
        assert!(finish_tx(
            &mut tx,
            &principal,
            &request,
            &snapshot,
            &[crate::runner_admission::verified_stop_for_test(&wrong)]
        )
        .await
        .is_err());
        tx.rollback().await.unwrap();
        let added = Uuid::new_v4();
        sqlx::query("INSERT INTO run_runner_attempts(id,run_id,lease_token,request_sha256) VALUES($1,$2,$3,$4)")
            .bind(added).bind(run_id).bind(lease).bind("b".repeat(64)).execute(&pool).await.unwrap();
        let proof = crate::runner_admission::verified_stop_for_test(&snapshot.stops[0]);
        let mut tx = pool.begin().await.unwrap();
        assert!(
            finish_tx(&mut tx, &principal, &request, &snapshot, &[proof])
                .await
                .is_err()
        );
        tx.rollback().await.unwrap();
        sqlx::query("DELETE FROM run_runner_attempts WHERE id=$1")
            .bind(added)
            .execute(&pool)
            .await
            .unwrap();
        let proof = crate::runner_admission::verified_stop_for_test(&snapshot.stops[0]);
        let mut tx = pool.begin().await.unwrap();
        let receipt = finish_tx(&mut tx, &principal, &request, &snapshot, &[proof])
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(receipt.queued_revision, 2);
        let repeated = begin(&pool, run_id, &principal, &request)
            .await
            .unwrap()
            .err()
            .unwrap();
        assert_eq!(receipt, repeated);
        let row = sqlx::query("SELECT state,spec FROM runs WHERE id=$1")
            .bind(run_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.get::<String, _>("state"), "queued");
        assert_eq!(
            row.get::<Value, _>("spec"),
            serde_json::to_value(&spec).unwrap()
        );
        let completed: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM instance_execution_reservations WHERE completion IS NOT NULL",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(completed, 0);
        let stop_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM run_server_execution_attempts WHERE stop_receipt IS NOT NULL",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(stop_count, 1);
        assert!(
            sqlx::query("UPDATE run_checkpoints SET origin='untrusted' WHERE id=$1")
                .bind(checkpoint.id)
                .execute(&pool)
                .await
                .is_err()
        );
        let new_worker = Uuid::new_v4();
        let new_lease = Uuid::new_v4();
        sqlx::query("UPDATE runs SET state='running',lease_owner=$2,lease_token=$3,lease_expires_at=clock_timestamp()+interval '1 hour' WHERE id=$1")
            .bind(run_id).bind(new_worker).bind(new_lease).execute(&pool).await.unwrap();
        assert_eq!(
            db::begin_server_agent_execution(&pool, run_id, new_worker, new_lease)
                .await
                .unwrap(),
            Some(state.clone())
        );
        assert!(crate::workflow::create_agent_worker_checkpoint(
            &pool,
            worker,
            run_id,
            lease,
            true,
            state.clone()
        )
        .await
        .is_err());
        crate::workflow::create_agent_worker_checkpoint(
            &pool, new_worker, run_id, new_lease, true, state,
        )
        .await
        .unwrap();
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM run_server_execution_attempts")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 2);
        sqlx::query("INSERT INTO run_instance_import_origins(run_id) VALUES($1)")
            .bind(run_id)
            .execute(&pool)
            .await
            .unwrap();
        // Even a previously accepted receipt is historical at a new target.
        assert!(begin(&pool, run_id, &principal, &request).await.is_err());
    }

    #[tokio::test]
    #[ignore = "requires COWORK_TEST_DATABASE_URL; only a fresh UUID schema is changed"]
    async fn postgres_resume_fences_provenance_authority_attempt_recheck_and_idempotency() {
        let url = std::env::var("COWORK_TEST_DATABASE_URL").expect("isolated PostgreSQL test URL");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        let schema = format!("resume_{}", Uuid::new_v4().simple());
        sqlx::raw_sql(&format!("CREATE SCHEMA {schema};SET search_path TO {schema};
            CREATE TABLE users(id uuid PRIMARY KEY,deleted_at timestamptz);
            CREATE TABLE auth_sessions(id uuid PRIMARY KEY,user_id uuid,revoked_at timestamptz,expires_at timestamptz);
            CREATE TABLE access_tokens(token_hash bytea PRIMARY KEY,session_id uuid,user_id uuid,revoked_at timestamptz,expires_at timestamptz);
            CREATE TABLE projects(id uuid PRIMARY KEY,owner_user_id uuid,team_id uuid,deleted_at timestamptz);
            CREATE TABLE threads(id uuid PRIMARY KEY,project_id uuid,deleted_at timestamptz);
            CREATE TABLE project_members(project_id uuid,user_id uuid,role text);
            CREATE TABLE team_members(team_id uuid,user_id uuid,role text);
            CREATE TABLE runs(id uuid PRIMARY KEY,spec jsonb NOT NULL,state text,revision bigint DEFAULT 1,assigned_executor_id uuid,
                lease_owner uuid,lease_token uuid,lease_expires_at timestamptz,started_at timestamptz,finished_at timestamptz,
                result jsonb,error jsonb,runner_cancel_id uuid,updated_at timestamptz DEFAULT clock_timestamp(),next_event_sequence bigint DEFAULT 1);
            CREATE TABLE run_checkpoints(id uuid PRIMARY KEY,run_id uuid REFERENCES runs(id),sequence bigint,safe_to_resume boolean,
                executor_state jsonb,created_at timestamptz DEFAULT clock_timestamp());
            CREATE TABLE run_instance_import_origins(run_id uuid PRIMARY KEY REFERENCES runs(id));
            CREATE TABLE enterprise_workflows(id uuid PRIMARY KEY,state text,auth_session_id uuid);
            CREATE TABLE enterprise_workflow_steps(child_run_id uuid,workflow_id uuid,device_grant_id uuid,
                device_grant_revision bigint,request jsonb,dispatch_auth_session_id uuid);
            CREATE TABLE run_runner_attempts(id uuid PRIMARY KEY,run_id uuid,lease_token uuid,request_sha256 text,workspace_id uuid);
            CREATE TABLE desktop_sessions(run_id uuid,state text);
            CREATE TABLE instance_execution_reservations(run_id uuid,completion jsonb);
            CREATE TABLE run_events(run_id uuid,sequence bigint,event_id uuid PRIMARY KEY,kind text,payload jsonb,created_at timestamptz);"))
            .execute(&pool).await.unwrap();
        sqlx::raw_sql("CREATE TABLE run_mfa_continuations(run_id uuid REFERENCES runs(id),queued_revision bigint NOT NULL,owner_user_id uuid REFERENCES users(id),auth_session_id uuid REFERENCES auth_sessions(id),request_id uuid NOT NULL,spec_sha256 text NOT NULL,created_at timestamptz NOT NULL DEFAULT now(),PRIMARY KEY(run_id,queued_revision),UNIQUE(run_id,request_id))")
            .execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!(
            "../migrations/0059_agent_resume_provenance.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        let result = tokio::spawn(fixture(pool.clone())).await;
        sqlx::raw_sql(&format!(
            "SET search_path TO pg_catalog;DROP SCHEMA {schema} CASCADE"
        ))
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;
        result.unwrap();
    }
}
