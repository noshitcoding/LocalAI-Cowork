//! Trusted transaction hooks; never count a selected catalogue item as execution.
use super::*;
use cowork_identity::audit_insights as metrics;
use serde::{Deserialize, Serialize};
use sqlx::{Any, Transaction};

fn identity(e: cowork_identity::IdentityError) -> Error {
    storage_error(e)
}
#[derive(Serialize, Deserialize)]
struct SelectedSkill {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    learning_snapshot: Option<Skill>,
    evidence: AuditSkillUse,
    fragment_sha256: String,
    fragment_bytes: usize,
}
fn has_fragment(messages: &[Value], bytes: usize, digest: &str) -> bool {
    messages
        .iter()
        .filter_map(|m| {
            m["content"].as_str().filter(|text| {
                m["role"] == "system"
                    || (m["role"] == "user"
                        && text.starts_with(crate::product_context::PROJECT_CONTEXT_PREFIX))
            })
        })
        .any(|text| {
            text.match_indices("Skill ").any(|(start, _)| {
                start
                    .checked_add(bytes)
                    .and_then(|end| text.get(start..end))
                    .is_some_and(|candidate| hash(candidate.as_bytes()) == digest)
            })
        })
}
pub(crate) async fn freeze_skills(
    tx: &mut Transaction<'_, Any>,
    run: &RunRecord,
    messages: &[Value],
) -> Result<()> {
    let mut selected = Vec::new();
    let mut captured = messages.to_vec();
    if let Some(binding) = &run.spec.workflow {
        captured.extend(crate::workflow_role_contexts::audit_messages(
            messages,
            &binding.plan_sha256,
        )?);
    }
    if run.spec.file_action.is_none() {
        for row in sqlx::query("SELECT kind,payload FROM productivity_objects WHERE project_id=$1 AND kind='skill' ORDER BY id LIMIT 1001").bind(&run.spec.project_id).fetch_all(&mut **tx).await.map_err(storage_error)?{
            if row.try_get::<String,_>("kind").map_err(storage_error)?!="skill"{continue;}
            let skill:Skill=parse(&row.try_get::<String,_>("payload").map_err(storage_error)?)?;
            if !skill.enabled{continue;}
            let Some(fragment)=crate::product_context::skill_fragment(&skill,&run.spec.prompt)else{continue;};
            let fragment_sha256=hash(fragment.as_bytes());
            if has_fragment(&captured,fragment.len(),&fragment_sha256){selected.push(SelectedSkill{learning_snapshot:skill.auto_learn.then(||skill.clone()),evidence:AuditSkillUse{id:skill.id,revision:skill.revision,instructions_sha256:hash(skill.instructions.as_bytes())},fragment_bytes:fragment.len(),fragment_sha256});}
        }
    }
    let payload = json_string(&selected)?;
    sqlx::query("INSERT INTO audit_run_skills(run_id,payload,payload_sha256) VALUES($1,$2,$3)")
        .bind(&run.id)
        .bind(&payload)
        .bind(hash(payload.as_bytes()))
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?;
    Ok(())
}
async fn selected(
    tx: &mut Transaction<'_, Any>,
    run: &str,
    messages: &[Value],
) -> Result<Option<Vec<AuditSkillUse>>> {
    let Some(row) =
        sqlx::query("SELECT payload,payload_sha256 FROM audit_run_skills WHERE run_id=$1")
            .bind(run)
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?
    else {
        return Ok(None);
    };
    let payload: String = row.try_get("payload").map_err(storage_error)?;
    if hash(payload.as_bytes())
        != row
            .try_get::<String, _>("payload_sha256")
            .map_err(storage_error)?
    {
        return Err(Error::Conflict);
    }
    let chosen: Vec<SelectedSkill> = parse(&payload)?;
    Ok(Some(
        chosen
            .into_iter()
            .filter(|s| {
                s.fragment_bytes <= 32_768
                    && has_fragment(messages, s.fragment_bytes, &s.fragment_sha256)
            })
            .map(|s| s.evidence)
            .collect(),
    ))
}
pub(crate) async fn personal_snapshot(
    tx: &mut Transaction<'_, Any>,
    run: &StoredRun,
) -> Result<()> {
    crate::skill_learning::record(tx, run).await?;
    let workflow = if run.record.spec.workflow.is_some() {
        sqlx::query_scalar::<_, String>("SELECT payload FROM workflow_runs WHERE run_id=$1")
            .bind(&run.record.id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?
            .map(|raw| parse::<StoredWorkflow>(&raw))
            .transpose()?
    } else {
        None
    };
    for attempt in &run.record.model_attempts {
        if attempt.request_sha256.is_none() {
            continue;
        }
        let actual_messages = if run.record.spec.workflow.is_none() {
            Some(run.checkpoint.messages.as_slice())
        } else {
            workflow.as_ref().and_then(|w| {
                w.agents
                    .values()
                    .flat_map(|a| &a.turns)
                    .find(|turn| {
                        turn.id == attempt.id
                            && Some(&turn.context.request_sha256) == attempt.request_sha256.as_ref()
                    })
                    .map(|turn| turn.context.messages.as_slice())
            })
        };
        if attempt.state == ModelAttemptState::InFlight || run.record.spec.workflow.is_some() {
            if let Some(skills) = if let Some(messages) = actual_messages {
                selected(tx, &run.record.id, messages).await?
            } else {
                None
            } {
                let payload = json_string(&skills)?;
                sqlx::query("INSERT INTO audit_attempt_skills(run_id,attempt_id,payload,payload_sha256) VALUES($1,$2,$3,$4) ON CONFLICT(run_id,attempt_id) DO NOTHING")
                    .bind(&run.record.id).bind(&attempt.id).bind(&payload).bind(hash(payload.as_bytes())).execute(&mut **tx).await.map_err(storage_error)?;
            }
            if attempt.state == ModelAttemptState::InFlight {
                continue;
            }
        }
        let Some(finished) = attempt.finished_at else {
            continue;
        };
        let row=sqlx::query("SELECT payload,payload_sha256 FROM audit_attempt_skills WHERE run_id=$1 AND attempt_id=$2").bind(&run.record.id).bind(&attempt.id).fetch_optional(&mut **tx).await.map_err(storage_error)?;
        let received_context = attempt.state != ModelAttemptState::Interrupted
            || attempt.usage.input_tokens.is_some()
            || attempt.usage.output_tokens.is_some();
        let skills = if let Some(row) = &row {
            let raw: String = row.try_get("payload").map_err(storage_error)?;
            if hash(raw.as_bytes())
                != row
                    .try_get::<String, _>("payload_sha256")
                    .map_err(storage_error)?
            {
                return Err(Error::Conflict);
            }
            if received_context {
                parse(&raw)?
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        // Interrupted receipts expose only observed lower bounds, never reserved tokens.
        let fact = AuditMetricFact {
            sequence: 0,
            id: format!("model:{}:{}", run.record.id, attempt.id),
            scope: "personal".into(),
            run_id: run.record.id.clone(),
            kind: "model".into(),
            observed_at: finished,
            time_basis: "server_recorded".into(),
            source_kind: "model_attempt_checkpoint".into(),
            source_sha256: canonical_hash(attempt),
            request_sha256: attempt.request_sha256.clone(),
            input_tokens: attempt.usage.input_tokens.map(|n| n.to_string()),
            output_tokens: attempt.usage.output_tokens.map(|n| n.to_string()),
            usage_complete: attempt.state == ModelAttemptState::Measured
                && !attempt.usage.incomplete
                && attempt.usage.input_tokens.is_some()
                && attempt.usage.output_tokens.is_some(),
            duration_ms: finished
                .checked_sub(attempt.started_at)
                .filter(|n| *n >= 0)
                .map(|n| n.to_string()),
            costs: Vec::new(),
            skills,
            skill_provenance_complete: row.is_some() && received_context,
        };
        metrics::append(tx, fact).await.map_err(identity)?;
    }
    if matches!(
        run.record.state,
        RunState::Completed | RunState::Failed | RunState::Canceled | RunState::Expired
    ) {
        metrics::append(
            tx,
            AuditMetricFact {
                sequence: 0,
                id: format!("run:{}:{}", run.record.id, run.record.updated_at),
                scope: "personal".into(),
                run_id: run.record.id.clone(),
                kind: "run".into(),
                observed_at: run.record.updated_at,
                time_basis: "server_recorded".into(),
                source_kind: "terminal_run_checkpoint".into(),
                source_sha256: canonical_hash(&(
                    run.record.id.as_str(),
                    &run.record.state,
                    run.record.started_at,
                    run.record.updated_at,
                )),
                request_sha256: None,
                input_tokens: None,
                output_tokens: None,
                usage_complete: false,
                duration_ms: if run.record.state == RunState::Canceled {
                    None
                } else {
                    run.record
                        .started_at
                        .and_then(|start| run.record.updated_at.checked_sub(start))
                        .filter(|n| *n >= 0)
                        .map(|n| n.to_string())
                },
                costs: Vec::new(),
                skills: Vec::new(),
                skill_provenance_complete: true,
            },
        )
        .await
        .map_err(identity)?;
    }
    Ok(())
}
pub(crate) async fn personal_event(tx: &mut Transaction<'_, Any>, event: &RunEvent) -> Result<()> {
    let raw: String = sqlx::query_scalar("SELECT payload FROM runs WHERE id=$1")
        .bind(&event.run_id)
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)?;
    let run = parse::<StoredRun>(&raw)?;
    personal_snapshot(tx, &run).await?;
    // Live original receive events only. Backfill deliberately does not accept
    // historical or imported response claims as provider billing evidence.
    if matches!(
        event.kind.as_str(),
        "model_completed" | "model_response_rejected" | "workflow_model_completed"
    ) && let Some(attempt) = event.payload["attempt_id"].as_str().and_then(|id| {
        run.record.model_attempts.iter().find(|a| {
            a.id == id
                && a.finished_at.is_some()
                && a.state != ModelAttemptState::InFlight
                && a.state != ModelAttemptState::Interrupted
        })
    }) && let Some(cost) =
        metrics::received_reported_cost(&event.payload["received_cost_metadata"])
    {
        metrics::append(
            tx,
            AuditMetricFact {
                sequence: 0,
                id: format!("provider-cost:{}:{}", event.run_id, attempt.id),
                scope: "personal".into(),
                run_id: event.run_id.clone(),
                kind: "cost".into(),
                observed_at: event.created_at,
                time_basis: "server_recorded".into(),
                source_kind: "original_completed_provider_cost".into(),
                source_sha256: canonical_hash(&(
                    event.id.as_str(),
                    &event.payload["received_cost_metadata"],
                    attempt.request_sha256.as_ref(),
                )),
                request_sha256: attempt.request_sha256.clone(),
                input_tokens: None,
                output_tokens: None,
                usage_complete: !attempt.usage.incomplete,
                duration_ms: None,
                costs: vec![cost],
                skills: Vec::new(),
                skill_provenance_complete: true,
            },
        )
        .await
        .map_err(identity)?;
    }
    if event.kind == "monetary_model_response" && event.payload["complete_usage"] == true {
        let v = &event.payload;
        let Some(attempt) = v["attempt_id"].as_str() else {
            return Ok(());
        };
        let price = &v["reservation"]["price"];
        let Some(units) = v["charged_units"].as_u64() else {
            return Ok(());
        };
        let (Some(currency), Some(scale), Some(source)) = (
            price["currency"]["code"].as_str(),
            price["currency"]["scale"].as_u64(),
            price["source_sha256"].as_str(),
        ) else {
            return Ok(());
        };
        let request = v["received"]["dispatch"]["logical_sha256"]
            .as_str()
            .map(str::to_owned);
        metrics::append(
            tx,
            AuditMetricFact {
                sequence: 0,
                id: format!("cost:{}:{attempt}", event.run_id),
                scope: "personal".into(),
                run_id: event.run_id.clone(),
                kind: "cost".into(),
                observed_at: event.created_at,
                time_basis: "server_recorded".into(),
                source_kind: "completed_received_price_receipt".into(),
                source_sha256: canonical_hash(v),
                request_sha256: request,
                input_tokens: None,
                output_tokens: None,
                usage_complete: true,
                duration_ms: None,
                costs: vec![AuditCost {
                    units: units.to_string(),
                    scale: u8::try_from(scale).map_err(storage_error)?,
                    currency: Some(currency.into()),
                    basis: "price_calculated".into(),
                    price_source_sha256: Some(source.into()),
                }],
                skills: Vec::new(),
                skill_provenance_complete: true,
            },
        )
        .await
        .map_err(identity)?;
    }
    Ok(())
}
pub(crate) async fn backfill_personal(tx: &mut Transaction<'_, Any>) -> Result<()> {
    let mut cursor: String = sqlx::query_scalar(
        "SELECT cursor FROM audit_metric_source_cursors WHERE scope='personal' AND source='runs'",
    )
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage_error)?
    .unwrap_or_default();
    loop {
        let rows = sqlx::query("SELECT id,payload FROM runs WHERE id>$1 ORDER BY id LIMIT 128")
            .bind(&cursor)
            .fetch_all(&mut **tx)
            .await
            .map_err(storage_error)?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            cursor = row.try_get("id").map_err(storage_error)?;
            personal_snapshot(
                tx,
                &parse::<StoredRun>(&row.try_get::<String, _>("payload").map_err(storage_error)?)?,
            )
            .await?;
            for e in sqlx::query("SELECT payload FROM events WHERE run_id=$1")
                .bind(&cursor)
                .fetch_all(&mut **tx)
                .await
                .map_err(storage_error)?
            {
                let event: RunEvent =
                    parse(&e.try_get::<String, _>("payload").map_err(storage_error)?)?;
                if event.kind == "monetary_model_response" {
                    personal_event(tx, &event).await?;
                }
            }
        }
    }
    sqlx::query("INSERT INTO audit_metric_source_cursors(scope,source,cursor) VALUES('personal','runs',$1) ON CONFLICT(scope,source) DO UPDATE SET cursor=excluded.cursor").bind(cursor).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
pub(crate) async fn team_run(tx: &mut Transaction<'_, Any>, run: &TeamRun) -> Result<()> {
    metrics::team_run(tx, run).await.map_err(identity)
}
