//! Durable batch updates. Transactions only schedule metadata; no host actions.
use super::*;

pub(super) async fn lock(tx: &mut Transaction<'_, Postgres>) -> Result<(), ApiError> {
    // The same short-lived gate orders reservations, operation dispatch and
    // completion across management replicas. It is never held during execution.
    sqlx::query("SELECT pg_advisory_xact_lock(7182483622)")
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub(super) async fn reserved(
    tx: &mut Transaction<'_, Postgres>,
    instance: Uuid,
) -> Result<bool, ApiError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM management_rollout_targets WHERE instance_id=$1 AND reserved)",
    )
    .bind(instance)
    .fetch_one(&mut **tx)
    .await?)
}

async fn authority(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
    actor: Uuid,
) -> Result<bool, ApiError> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users u LEFT JOIN management_members m ON m.user_id=u.id AND m.organisation_id=$2 WHERE u.id=$1 AND u.deleted_at IS NULL AND (u.platform_admin OR m.role='admin'))")
        .bind(actor).bind(org).fetch_one(&mut **tx).await?)
}

pub(super) async fn dispatch_allowed(
    tx: &mut Transaction<'_, Postgres>,
    operation: &Value,
) -> Result<bool, ApiError> {
    if operation["rollout_id"].is_null() {
        return Ok(true);
    }
    let id: Uuid = serde_json::from_value(operation["rollout_id"].clone())?;
    let org: Uuid = serde_json::from_value(operation["organisation_id"].clone())?;
    let actor: Uuid = serde_json::from_value(operation["actor_user_id"].clone())?;
    let transition: Option<(String,String)> = sqlx::query_as("SELECT r.release,t.source_release FROM management_rollouts r JOIN management_rollout_targets t ON t.rollout_id=r.id WHERE r.id=$1 AND r.organisation_id=$2 AND r.state='running' AND t.operation_id=$3 AND t.reserved AND t.state='active'")
        .bind(id).bind(org).bind(serde_json::from_value::<Uuid>(operation["id"].clone())?).fetch_optional(&mut **tx).await?;
    let Some((release, source)) = transition else {
        return Ok(false);
    };
    let instance = serde_json::from_value(operation["instance_id"].clone())?;
    let host = executor_hosts::instance_host(tx, instance).await?;
    Ok(authority(tx, org, actor).await?
        && releases::selection_allowed_profiled(
            tx,
            &release,
            &source,
            false,
            host,
            operation.get("backend_profile").filter(|v| !v.is_null()),
        )
        .await?)
}

pub(super) async fn list_rollouts(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(org): Path<Uuid>,
) -> Result<Json<Vec<Rollout>>, ApiError> {
    let role = membership(&state, &principal, org, false).await?;
    if !["admin", "auditor"].contains(&role.as_str()) {
        return Err(denied());
    }
    let rows: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(r)-'request_digest'-'idempotency_key'||jsonb_build_object('targets',COALESCE((SELECT jsonb_agg(to_jsonb(t)-'reserved'||jsonb_build_object('instance_name',i.name) ORDER BY t.ordinal) FROM management_rollout_targets t JOIN management_instances i ON i.id=t.instance_id WHERE t.rollout_id=r.id),'[]'::jsonb)) FROM management_rollouts r WHERE organisation_id=$1 ORDER BY created_at DESC LIMIT 100")
        .bind(org).fetch_all(&state.pool).await?;
    Ok(Json(
        rows.into_iter()
            .map(serde_json::from_value)
            .collect::<Result<_, _>>()?,
    ))
}

pub(super) async fn create_rollout(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(org): Path<Uuid>,
    Json(request): Json<CreateRollout>,
) -> Result<Json<Value>, ApiError> {
    membership(&state, &principal, org, true).await?;
    if request.name.trim().is_empty()
        || request.name.chars().count() > 120
        || request.name.chars().any(char::is_control)
        || !(1..=10).contains(&request.batch_size)
        || request.targets.is_empty()
        || request.targets.len() > 40
        || request
            .targets
            .iter()
            .map(|t| t.instance_id)
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != request.targets.len()
        || !releases::digest(&request.release)
    {
        return Err(invalid("invalid rollout specification"));
    }
    let digest = hex::encode(Sha256::digest(serde_json::to_vec(
        &json!({"organisation_id":org,"request":request}),
    )?));
    let mut tx = state.pool.begin().await?;
    lock(&mut tx).await?;
    sso::authorize(&mut tx, org, &principal).await?;
    let previous: Option<Value>=sqlx::query_scalar("SELECT to_jsonb(r) FROM management_rollouts r WHERE actor_user_id=$1 AND idempotency_key=$2")
        .bind(principal.user_id).bind(request.idempotency_key).fetch_optional(&mut *tx).await?;
    if let Some(previous) = previous {
        if previous["request_digest"] != digest {
            return Err(ApiError::Conflict(
                "idempotency key reused for a different rollout".into(),
            ));
        }
        return Ok(Json(json!({"id":previous["id"],"state":previous["state"]})));
    }
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO management_rollouts(id,organisation_id,actor_user_id,name,release,batch_size,idempotency_key,request_digest) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
        .bind(id).bind(org).bind(principal.user_id).bind(&request.name).bind(&request.release).bind(request.batch_size).bind(request.idempotency_key).bind(digest).execute(&mut *tx).await?;
    for (ordinal, target) in request.targets.iter().enumerate() {
        let value: Option<Value>=sqlx::query_scalar("SELECT to_jsonb(i) FROM management_instances i WHERE id=$1 AND organisation_id=$2 FOR UPDATE")
            .bind(target.instance_id).bind(org).fetch_optional(&mut *tx).await?;
        let instance: Instance = serde_json::from_value(value.ok_or_else(denied)?)?;
        if instance.revision != target.expected_revision {
            return Err(ApiError::Conflict(
                "rollout instance changed; reload before retrying".into(),
            ));
        }
        let busy: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_operations WHERE instance_id=$1 AND state IN ('queued','running','reconciling'))")
            .bind(instance.id).fetch_one(&mut *tx).await?;
        if busy || reserved(&mut tx, instance.id).await? {
            return Err(ApiError::Conflict(
                "rollout instance is already reserved or busy".into(),
            ));
        }
        let mut stopped = instance.clone();
        environments::instance_unlocked(&mut tx, instance.id).await?;
        vm_snapshots::unlocked(&mut tx, instance.id).await?;
        stopped.state = InstanceState::Stopped;
        let profile = backend_profiles::current(&mut tx, org, instance.id).await?;
        if !matches!(
            instance.state,
            InstanceState::Ready | InstanceState::Stopped
        ) || !(operation_available(&stopped, InstanceAction::Upgrade)
            || backend_profiles::available(&stopped, InstanceAction::Upgrade, profile.as_ref()))
        {
            return Err(invalid(
                "rollout requires ready or stopped instances on their current bound backend",
            ));
        }
        let source = instance
            .current_release
            .as_deref()
            .ok_or_else(|| invalid("installed release is unknown"))?;
        let host = executor_hosts::instance_host(&mut tx, instance.id).await?;
        releases::validate_selection_profiled(
            &mut tx,
            &request.release,
            source,
            false,
            host,
            profile.as_ref(),
        )
        .await?;
        sqlx::query("INSERT INTO management_rollout_targets(organisation_id,rollout_id,instance_id,ordinal,source_release,initial_state,expected_revision,next_action) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
            .bind(org).bind(id).bind(instance.id).bind(ordinal as i32).bind(source).bind(tag(instance.state)?).bind(instance.revision+1)
            .bind(if instance.state==InstanceState::Ready {"stop"} else {"upgrade"}).execute(&mut *tx).await?;
        sqlx::query("UPDATE management_instances SET revision=revision+1 WHERE id=$1")
            .bind(instance.id)
            .execute(&mut *tx)
            .await?;
    }
    append_audit(&mut tx,org,principal.user_id,"rollout.created",id,json!({"release":request.release,"batch_size":request.batch_size,"instances":request.targets.iter().map(|t| t.instance_id).collect::<Vec<_>>()})).await?;
    tx.commit().await?;
    Ok(Json(json!({"id":id,"state":"running"})))
}

pub(super) async fn control_rollout(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, id)): Path<(Uuid, Uuid)>,
    Json(request): Json<ControlRollout>,
) -> Result<Json<Value>, ApiError> {
    membership(&state, &principal, org, true).await?;
    let mut tx = state.pool.begin().await?;
    lock(&mut tx).await?;
    sso::authorize(&mut tx, org, &principal).await?;
    let row = sqlx::query(
        "SELECT * FROM management_rollouts WHERE id=$1 AND organisation_id=$2 FOR UPDATE",
    )
    .bind(id)
    .bind(org)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(denied)?;
    if row.try_get::<i64, _>("revision")? != request.expected_revision {
        return Err(ApiError::Conflict(
            "rollout changed; reload before retrying".into(),
        ));
    }
    let current: String = row.try_get("state")?;
    let next = match request.action {
        RolloutControl::Pause if current == "running" => "paused",
        RolloutControl::Resume if current == "paused" => {
            let failed: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_rollout_targets t LEFT JOIN management_operations o ON o.id=t.operation_id WHERE t.rollout_id=$1 AND (t.state='failed' OR o.state IN ('failed','reconciling')))").bind(id).fetch_one(&mut *tx).await?;
            if failed || !authority(&mut tx, org, row.try_get("actor_user_id")?).await? {
                return Err(invalid(
                    "failed rollout requires review and cancellation before creating a new plan",
                ));
            }
            "running"
        }
        RolloutControl::Cancel if current == "running" || current == "paused" => "canceling",
        _ => return Err(invalid("rollout control is not valid in the current state")),
    };
    sqlx::query(
        "UPDATE management_rollouts SET state=$2,revision=revision+1,updated_at=now() WHERE id=$1",
    )
    .bind(id)
    .bind(next)
    .execute(&mut *tx)
    .await?;
    append_audit(
        &mut tx,
        org,
        principal.user_id,
        "rollout.control",
        id,
        json!({"action":request.action}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"id":id,"state":next})))
}

async fn fail(
    tx: &mut Transaction<'_, Postgres>,
    rollout: Uuid,
    instance: Uuid,
    code: &str,
) -> Result<(), ApiError> {
    sqlx::query("UPDATE management_rollout_targets SET state='failed',error_code=$3 WHERE rollout_id=$1 AND instance_id=$2").bind(rollout).bind(instance).bind(code).execute(&mut **tx).await?;
    sqlx::query("UPDATE management_rollouts SET state='paused',error_code=$2,revision=revision+1,updated_at=now() WHERE id=$1").bind(rollout).bind(code).execute(&mut **tx).await?;
    let (org, actor): (Uuid, Uuid) =
        sqlx::query_as("SELECT organisation_id,actor_user_id FROM management_rollouts WHERE id=$1")
            .bind(rollout)
            .fetch_one(&mut **tx)
            .await?;
    append_audit(
        tx,
        org,
        actor,
        "rollout.paused",
        rollout,
        json!({"instance_id":instance,"reason":code}),
    )
    .await?;
    Ok(())
}

/// Called by authenticated provisioner polling. Restarting either service reads
/// these rows and original operation IDs; it cannot replay an uncertain action.
pub(super) async fn advance(state: &AppState) -> Result<(), ApiError> {
    let mut tx = state.pool.begin().await?;
    lock(&mut tx).await?;
    sqlx::query("UPDATE management_operations SET state='reconciling',updated_at=now() WHERE state='running' AND lease_expires_at<now()").execute(&mut *tx).await?;
    let plans: Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(r) FROM management_rollouts r WHERE state IN ('running','paused','canceling') ORDER BY created_at FOR UPDATE").fetch_all(&mut *tx).await?;
    for plan in plans {
        let id: Uuid = serde_json::from_value(plan["id"].clone())?;
        let org: Uuid = serde_json::from_value(plan["organisation_id"].clone())?;
        let actor: Uuid = serde_json::from_value(plan["actor_user_id"].clone())?;
        let canceling = plan["state"] == "canceling";
        let mut running = plan["state"] == "running";
        let mut changed = false;
        if canceling {
            sqlx::query("UPDATE management_operations SET state='failed',result=jsonb_build_object('error_code','rollout_canceled_before_dispatch'),updated_at=now() WHERE rollout_id=$1 AND state='queued'").bind(id).execute(&mut *tx).await?;
        }
        let targets:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(t)||jsonb_build_object('operation',to_jsonb(o)) FROM management_rollout_targets t LEFT JOIN management_operations o ON o.id=t.operation_id WHERE t.rollout_id=$1 AND t.reserved ORDER BY t.ordinal").bind(id).fetch_all(&mut *tx).await?;
        for target in targets {
            let instance: Uuid = serde_json::from_value(target["instance_id"].clone())?;
            let op = &target["operation"];
            if canceling && !matches!(op["state"].as_str(), Some("running" | "reconciling")) {
                sqlx::query("UPDATE management_rollout_targets SET state='canceled',reserved=FALSE,next_action=NULL WHERE rollout_id=$1 AND instance_id=$2").bind(id).bind(instance).execute(&mut *tx).await?;
                sqlx::query("UPDATE management_instances SET revision=revision+1 WHERE id=$1")
                    .bind(instance)
                    .execute(&mut *tx)
                    .await?;
                changed = true;
                continue;
            }
            if target["state"] != "active" {
                continue;
            }
            if matches!(op["state"].as_str(), Some("failed" | "reconciling")) {
                if !canceling {
                    fail(
                        &mut tx,
                        id,
                        instance,
                        if op["state"] == "reconciling" {
                            "operation_requires_reconciliation"
                        } else {
                            "instance_operation_failed"
                        },
                    )
                    .await?;
                    running = false;
                    changed = true;
                }
            } else if op["state"] == "succeeded" {
                let next = match op["action"].as_str() {
                    Some("stop") => Some("upgrade"),
                    Some("upgrade") if target["initial_state"] == "ready" => Some("start"),
                    _ => None,
                };
                sqlx::query("UPDATE management_rollout_targets t SET state=$3,next_action=$4,reserved=$5,expected_revision=i.revision FROM management_instances i WHERE t.rollout_id=$1 AND t.instance_id=$2 AND i.id=t.instance_id")
                    .bind(id).bind(instance).bind(if next.is_some(){"waiting"}else{"succeeded"}).bind(next).bind(next.is_some()).execute(&mut *tx).await?;
                changed = true;
            }
        }
        let remaining: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM management_rollout_targets WHERE rollout_id=$1 AND reserved",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        if remaining == 0 {
            let final_state = if canceling { "canceled" } else { "succeeded" };
            sqlx::query("UPDATE management_rollouts SET state=$2,revision=revision+1,updated_at=now() WHERE id=$1").bind(id).bind(final_state).execute(&mut *tx).await?;
            append_audit(
                &mut tx,
                org,
                actor,
                "rollout.completed",
                id,
                json!({"state":final_state}),
            )
            .await?;
            continue;
        }
        if running {
            let candidates:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(t) FROM management_rollout_targets t WHERE rollout_id=$1 AND state='waiting' AND ordinal/$2=(SELECT min(ordinal)/$2 FROM management_rollout_targets WHERE rollout_id=$1 AND reserved) ORDER BY ordinal")
                .bind(id).bind(plan["batch_size"].as_i64().unwrap_or(1) as i32).fetch_all(&mut *tx).await?;
            for target in candidates {
                let instance: Uuid = serde_json::from_value(target["instance_id"].clone())?;
                let current: Value = sqlx::query_scalar(
                    "SELECT to_jsonb(i) FROM management_instances i WHERE id=$1 FOR UPDATE",
                )
                .bind(instance)
                .fetch_one(&mut *tx)
                .await?;
                let action = target["next_action"]
                    .as_str()
                    .ok_or_else(|| invalid("invalid rollout step"))?;
                let parsed: Instance = serde_json::from_value(current.clone())?;
                let parsed_action: InstanceAction = serde_json::from_value(json!(action))?;
                let expected_release = if action == "start" {
                    &plan["release"]
                } else {
                    &target["source_release"]
                };
                let host = serde_json::from_value(current["executor_host_id"].clone())?;
                let backend = backend_profiles::current(&mut tx, org, instance).await?;
                let authorized = authority(&mut tx, org, actor).await?
                    && releases::selection_allowed_profiled(
                        &mut tx,
                        plan["release"].as_str().unwrap_or(""),
                        target["source_release"].as_str().unwrap_or(""),
                        false,
                        host,
                        backend.as_ref(),
                    )
                    .await?;
                let mut profile = parsed.clone();
                profile.state = InstanceState::Stopped;
                if !authorized
                    || !(operation_available(&profile, InstanceAction::Upgrade)
                        || backend_profiles::available(
                            &profile,
                            InstanceAction::Upgrade,
                            backend.as_ref(),
                        ))
                    || current["revision"] != target["expected_revision"]
                    || current["current_release"] != *expected_release
                    || !parsed.state.accepts(parsed_action)
                    || !(operation_available(&parsed, parsed_action)
                        || backend_profiles::available(&parsed, parsed_action, backend.as_ref()))
                {
                    fail(
                        &mut tx,
                        id,
                        instance,
                        if !authorized {
                            "rollout_authorization_revoked"
                        } else {
                            "rollout_instance_changed"
                        },
                    )
                    .await?;
                    changed = true;
                    break;
                }
                let operation = Uuid::new_v4();
                let arguments = if action == "upgrade" {
                    json!({"action":"upgrade","release":plan["release"],"source_release":target["source_release"],"qualification_only":false})
                } else {
                    json!({"action":action})
                };
                sqlx::query("INSERT INTO management_operations(id,organisation_id,instance_id,actor_user_id,action,arguments,idempotency_key,request_digest,rollout_id,backend_profile) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
                    .bind(operation).bind(org).bind(instance).bind(actor).bind(action).bind(&arguments).bind(operation.to_string()).bind(hex::encode(Sha256::digest(serde_json::to_vec(&arguments)?))).bind(id).bind(backend).execute(&mut *tx).await?;
                sqlx::query("UPDATE management_instances SET revision=revision+1 WHERE id=$1")
                    .bind(instance)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("UPDATE management_rollout_targets SET state='active',operation_id=$3 WHERE rollout_id=$1 AND instance_id=$2").bind(id).bind(instance).bind(operation).execute(&mut *tx).await?;
                append_audit(
                    &mut tx,
                    org,
                    actor,
                    "rollout.operation.queued",
                    instance,
                    json!({"rollout_id":id,"operation_id":operation,"action":action}),
                )
                .await?;
                changed = true;
            }
        }
        if changed {
            sqlx::query(
                "UPDATE management_rollouts SET revision=revision+1,updated_at=now() WHERE id=$1",
            )
            .bind(id)
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}
