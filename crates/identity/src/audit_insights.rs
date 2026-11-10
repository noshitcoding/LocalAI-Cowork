//! Immutable redacted execution facts. No reservation is reported as expenditure.
use crate::{IdentityError, Result, audit::hash};
use cowork_contracts::*;
use sqlx::{Any, Row, Transaction};
use std::collections::{BTreeMap, BTreeSet};

fn digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn number(s: &str) -> Result<u128> {
    if s.len() > 39
        || s.is_empty()
        || !s.bytes().all(|c| c.is_ascii_digit())
        || (s.len() > 1 && s.starts_with('0'))
    {
        return Err(IdentityError::Corrupt);
    }
    s.parse().map_err(|_| IdentityError::Corrupt)
}
fn validate(f: &AuditMetricFact) -> Result<()> {
    if f.id.is_empty()
        || f.id.len() > 512
        || f.id.chars().any(char::is_control)
        || f.run_id.is_empty()
        || f.run_id.len() > 128
        || !matches!(f.kind.as_str(), "model" | "run" | "cost")
        || !matches!(
            f.time_basis.as_str(),
            "server_recorded" | "run_created_legacy_fallback"
        )
        || f.observed_at <= 0
        || !digest(&f.source_sha256)
        || f.request_sha256.as_ref().is_some_and(|s| !digest(s))
        || f.skills.len() > 64
        || f.costs.len() > 8
    {
        return Err(IdentityError::Validation);
    }
    for s in [&f.input_tokens, &f.output_tokens, &f.duration_ms]
        .into_iter()
        .flatten()
    {
        number(s)?;
    }
    for skill in &f.skills {
        if skill.id.is_empty()
            || skill.id.len() > 128
            || skill.revision <= 0
            || !digest(&skill.instructions_sha256)
        {
            return Err(IdentityError::Validation);
        }
    }
    for c in &f.costs {
        number(&c.units)?;
        if c.scale > 9
            || !matches!(c.basis.as_str(), "price_calculated" | "provider_reported")
            || c.currency
                .as_ref()
                .is_some_and(|s| s.len() != 3 || !s.bytes().all(|b| b.is_ascii_uppercase()))
            || c.price_source_sha256.as_ref().is_some_and(|s| !digest(s))
        {
            return Err(IdentityError::Validation);
        }
    }
    Ok(())
}
async fn head(tx: &mut Transaction<'_, Any>, scope: &str) -> Result<i64> {
    if !(scope == "personal"
        || ["account:", "organization:"].iter().any(|p| {
            scope
                .strip_prefix(p)
                .is_some_and(|v| uuid::Uuid::parse_str(v).is_ok())
        }))
    {
        return Err(IdentityError::Validation);
    }
    sqlx::query(
        "INSERT INTO audit_metric_heads(scope,sequence) VALUES($1,0) ON CONFLICT(scope) DO NOTHING",
    )
    .bind(scope)
    .execute(&mut **tx)
    .await?;
    sqlx::query("UPDATE audit_metric_heads SET sequence=sequence WHERE scope=$1")
        .bind(scope)
        .execute(&mut **tx)
        .await?;
    Ok(
        sqlx::query_scalar("SELECT sequence FROM audit_metric_heads WHERE scope=$1")
            .bind(scope)
            .fetch_one(&mut **tx)
            .await?,
    )
}
pub async fn append(tx: &mut Transaction<'_, Any>, mut fact: AuditMetricFact) -> Result<()> {
    validate(&fact)?;
    let current = head(tx, &fact.scope).await?;
    if let Some(row) = sqlx::query(
        "SELECT sequence,payload,payload_sha256 FROM audit_metric_facts WHERE scope=$1 AND id=$2",
    )
    .bind(&fact.scope)
    .bind(&fact.id)
    .fetch_optional(&mut **tx)
    .await?
    {
        fact.sequence = row.try_get("sequence")?;
        let payload: String = row.try_get("payload")?;
        if hash(payload.as_bytes()) != row.try_get::<String, _>("payload_sha256")?
            || serde_json::from_str::<AuditMetricFact>(&payload)
                .map_err(|_| IdentityError::Corrupt)?
                != fact
        {
            return Err(IdentityError::Conflict);
        }
        return Ok(());
    }
    fact.sequence = current.checked_add(1).ok_or(IdentityError::QuotaExceeded)?;
    let payload = serde_json::to_string(&fact).map_err(|_| IdentityError::Corrupt)?;
    sqlx::query("INSERT INTO audit_metric_facts(scope,sequence,id,payload,payload_sha256,observed_at) VALUES($1,$2,$3,$4,$5,$6)")
        .bind(&fact.scope).bind(fact.sequence).bind(&fact.id).bind(&payload).bind(hash(payload.as_bytes())).bind(fact.observed_at).execute(&mut **tx).await?;
    sqlx::query("UPDATE audit_metric_heads SET sequence=$1 WHERE scope=$2")
        .bind(fact.sequence)
        .bind(&fact.scope)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
fn decode(row: &sqlx::any::AnyRow, scope: &str) -> Result<AuditMetricFact> {
    let payload: String = row.try_get("payload")?;
    if payload.len() > 32 * 1024
        || hash(payload.as_bytes()) != row.try_get::<String, _>("payload_sha256")?
    {
        return Err(IdentityError::Corrupt);
    }
    let f: AuditMetricFact = serde_json::from_str(&payload).map_err(|_| IdentityError::Corrupt)?;
    validate(&f)?;
    if f.scope != scope
        || f.sequence != row.try_get::<i64, _>("sequence")?
        || f.id != row.try_get::<String, _>("id")?
        || f.observed_at != row.try_get::<i64, _>("observed_at")?
    {
        return Err(IdentityError::Corrupt);
    }
    Ok(f)
}
fn totals(facts: &[AuditMetricFact], complete: bool) -> Result<AuditExecutionTotals> {
    let (
        mut models,
        mut input,
        mut output,
        mut incomplete,
        mut runs,
        mut duration,
        mut unknown_duration,
        mut skills,
        mut unknown_skills,
        mut missing_cost,
    ) = (
        0u128, 0u128, 0u128, 0u128, 0u128, 0u128, 0u128, 0u128, 0u128, 0u128,
    );
    let mut costs = BTreeMap::<(Option<String>, u8, String), u128>::new();
    let mut priced = BTreeSet::new();
    let (mut model_duration, mut unknown_model_duration, mut non_execution_timestamps) =
        (0u128, 0u128, 0u128);
    let mut latest_runs = BTreeMap::<&str, &AuditMetricFact>::new();
    for f in facts {
        if !f.costs.is_empty() {
            priced.insert(f.id.split_once(':').map_or(f.id.as_str(), |(_, key)| key));
        }
        for c in &f.costs {
            let slot = costs
                .entry((c.currency.clone(), c.scale, c.basis.clone()))
                .or_default();
            *slot = slot
                .checked_add(number(&c.units)?)
                .ok_or(IdentityError::Corrupt)?;
        }
    }
    for f in facts {
        match f.kind.as_str() {
            "model" => {
                models += 1;
                input = input
                    .checked_add(
                        f.input_tokens
                            .as_ref()
                            .map(|s| number(s))
                            .transpose()?
                            .unwrap_or(0),
                    )
                    .ok_or(IdentityError::Corrupt)?;
                output = output
                    .checked_add(
                        f.output_tokens
                            .as_ref()
                            .map(|s| number(s))
                            .transpose()?
                            .unwrap_or(0),
                    )
                    .ok_or(IdentityError::Corrupt)?;
                non_execution_timestamps += u128::from(f.time_basis != "server_recorded");
                if let Some(n) = &f.duration_ms {
                    model_duration = model_duration
                        .checked_add(number(n)?)
                        .ok_or(IdentityError::Corrupt)?;
                } else {
                    unknown_model_duration += 1;
                }
                incomplete += u128::from(!f.usage_complete);
                skills += f.skills.len() as u128;
                unknown_skills += u128::from(!f.skill_provenance_complete);
                missing_cost += u128::from(
                    !priced.contains(f.id.split_once(':').map_or(f.id.as_str(), |(_, key)| key)),
                );
            }
            "run" => {
                if latest_runs
                    .get(f.run_id.as_str())
                    .is_none_or(|old| old.observed_at < f.observed_at)
                {
                    latest_runs.insert(&f.run_id, f);
                }
            }
            _ => {}
        }
    }
    for f in latest_runs.values() {
        runs += 1;
        if let Some(n) = &f.duration_ms {
            duration = duration
                .checked_add(number(n)?)
                .ok_or(IdentityError::Corrupt)?;
        } else {
            unknown_duration += 1;
        }
    }
    Ok(AuditExecutionTotals {
        model_receipts: models.to_string(),
        input_tokens: input.to_string(),
        output_tokens: output.to_string(),
        incomplete_usage_receipts: incomplete.to_string(),
        run_receipts: runs.to_string(),
        duration_ms: duration.to_string(),
        unknown_duration_runs: unknown_duration.to_string(),
        skill_context_uses: skills.to_string(),
        unknown_skill_model_receipts: unknown_skills.to_string(),
        model_duration_ms: model_duration.to_string(),
        unknown_model_duration_receipts: unknown_model_duration.to_string(),
        non_execution_timestamp_receipts: non_execution_timestamps.to_string(),
        costs: costs
            .into_iter()
            .map(|((currency, scale, basis), n)| AuditCost {
                currency,
                scale,
                basis,
                units: n.to_string(),
                price_source_sha256: None,
            })
            .collect(),
        missing_cost_model_receipts: missing_cost.to_string(),
        facts: facts.len().to_string(),
        complete,
    })
}
pub async fn query(
    tx: &mut Transaction<'_, Any>,
    scope: &str,
    q: AuditInsightsQuery,
    now: i64,
) -> Result<AuditExecutionInsights> {
    let until = q.until_ms.unwrap_or(now);
    let from = q.from_ms.unwrap_or(until.saturating_sub(30 * 86_400_000));
    if from < 0
        || until <= from
        || until > now
        || until - from > 366 * 86_400_000
        || q.after_sequence < 0
        || !(1..=200).contains(&q.limit)
    {
        return Err(IdentityError::Validation);
    }
    let comparison = from.saturating_sub(until - from).max(0);
    let current = head(tx, scope).await?;
    let snapshot = q.snapshot_sequence.unwrap_or(current);
    if snapshot < 0 || snapshot > current || q.after_sequence > snapshot {
        return Err(IdentityError::Validation);
    }
    let mut all = Vec::new();
    let mut cursor = 0;
    let mut complete = true;
    loop {
        let rows=sqlx::query("SELECT * FROM audit_metric_facts WHERE scope=$1 AND sequence>$2 AND sequence<=$3 AND observed_at>=$4 AND observed_at<$5 ORDER BY sequence LIMIT 256")
        .bind(scope).bind(cursor).bind(snapshot).bind(comparison).bind(until).fetch_all(&mut **tx).await?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            let f = decode(&row, scope)?;
            cursor = f.sequence;
            all.push(f);
            if all.len() > 100_000 {
                all.pop();
                complete = false;
                break;
            }
        }
        if !complete {
            break;
        }
    }
    let selected: Vec<_> = all
        .iter()
        .filter(|f| f.observed_at >= from)
        .cloned()
        .collect();
    let previous: Vec<_> = all
        .iter()
        .filter(|f| f.observed_at < from)
        .cloned()
        .collect();
    let rows=sqlx::query("SELECT * FROM audit_metric_facts WHERE scope=$1 AND sequence>$2 AND sequence<=$3 AND observed_at>=$4 AND observed_at<$5 ORDER BY sequence LIMIT $6")
        .bind(scope).bind(q.after_sequence).bind(snapshot).bind(from).bind(until).bind(q.limit+1).fetch_all(&mut **tx).await?;
    let has_more = rows.len() > q.limit as usize;
    let facts = rows
        .iter()
        .take(q.limit as usize)
        .map(|r| decode(r, scope))
        .collect::<Result<Vec<_>>>()?;
    Ok(AuditExecutionInsights {
        from_ms: from,
        until_ms: until,
        comparison_from_ms: comparison,
        snapshot_sequence: snapshot,
        current: totals(&selected, complete)?,
        previous: totals(&previous, complete)?,
        next_sequence: facts.last().map_or(q.after_sequence, |f| f.sequence),
        facts,
        has_more,
    })
}

/// Parse only an explicit provider amount. Unknown currency stays unknown and
/// cannot be presented as an exact monetary bill. Reject scientific/overprecise
/// values rather than round or infer a price.
pub fn reported_cost(value: &serde_json::Value) -> Option<AuditCost> {
    let cost = value.get("cost").or_else(|| value.get("response_cost"))?;
    let raw = if let Some(raw) = cost.as_str() {
        raw.to_owned()
    } else {
        cost.as_number()?.to_string()
    };
    if raw.is_empty() || raw.len() > 48 {
        return None;
    }
    let (whole, fraction) = raw.split_once('.').unwrap_or((&raw, ""));
    if fraction.len() > 9
        || whole.starts_with('-')
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let units = format!("{whole}{fraction}")
        .parse::<u128>()
        .ok()?
        .to_string();
    let currency = value
        .get("currency")
        .and_then(|v| v.as_str())
        .filter(|s| s.len() == 3 && s.bytes().all(|b| b.is_ascii_uppercase()))
        .map(str::to_owned);
    Some(AuditCost {
        units,
        scale: fraction.len() as u8,
        currency,
        basis: "provider_reported".into(),
        price_source_sha256: None,
    })
}

/// Original completed-receive metadata; duplicate body/header observations are
/// one provider claim. Conflicting amounts/currencies stay unknown.
pub fn received_reported_cost(value: &serde_json::Value) -> Option<AuditCost> {
    if value["format"] != 1
        || value["provider_reported"] != true
        || value["completed_stream"] != true
    {
        return None;
    }
    let body = reported_cost(&value["body_usage"]);
    let header = reported_cost(
        &serde_json::json!({"cost":value["headers"]["x-litellm-response-cost"],"currency":value["headers"]["x-litellm-response-cost-currency"]}),
    );
    match (body, header) {
        (Some(mut a), Some(b)) => {
            let scale = a.scale.max(b.scale);
            let left = number(&a.units)
                .ok()?
                .checked_mul(10u128.checked_pow(u32::from(scale - a.scale))?)?;
            let right = number(&b.units)
                .ok()?
                .checked_mul(10u128.checked_pow(u32::from(scale - b.scale))?)?;
            if left != right
                || a.currency
                    .as_ref()
                    .zip(b.currency.as_ref())
                    .is_some_and(|(a, b)| a != b)
            {
                return None;
            }
            if a.currency.is_none() {
                a.currency = b.currency;
            }
            Some(a)
        }
        (Some(a), None) | (None, Some(a)) => Some(a),
        _ => None,
    }
}

/// Human reconciliation receipts do not prove actual provider usage.
pub async fn team_receipt(
    tx: &mut Transaction<'_, Any>,
    team: uuid::Uuid,
    run: uuid::Uuid,
    view: &serde_json::Value,
    receipt: &serde_json::Value,
    fallback_time: i64,
) -> Result<()> {
    if view["kind"] != "model" || receipt["outcome"] != "completed" {
        return Ok(());
    }
    let id = view["id"].as_str().ok_or(IdentityError::Corrupt)?;
    if receipt["effect_id"] != view["id"]
        || receipt["run_id"] != serde_json::json!(run)
        || receipt["request_sha256"] != view["request_sha256"]
        || receipt["scope_sha256"] != view["scope_sha256"]
    {
        return Err(IdentityError::Corrupt);
    }
    let insight = receipt.get("insights");
    let usage = &receipt["result"]["usage"];
    let input = usage["prompt_tokens"].as_u64().map(|n| n.to_string());
    let output = usage["completion_tokens"].as_u64().map(|n| n.to_string());
    let skills = insight
        .and_then(|i| i.get("skills"))
        .map(|v| {
            serde_json::from_value::<Vec<AuditSkillUse>>(v.clone())
                .map_err(|_| IdentityError::Corrupt)
        })
        .transpose()?
        .unwrap_or_default();
    append(
        tx,
        AuditMetricFact {
            sequence: 0,
            id: format!("model:{run}:{id}"),
            scope: format!("organization:{team}"),
            run_id: run.to_string(),
            kind: "model".into(),
            observed_at: insight
                .and_then(|i| i["observed_at"].as_i64())
                .unwrap_or(fallback_time),
            time_basis: if insight.is_some() {
                "server_recorded"
            } else {
                "run_created_legacy_fallback"
            }
            .into(),
            source_kind: "original_worker_model_ack".into(),
            source_sha256: hash(
                serde_json::to_string(receipt)
                    .map_err(|_| IdentityError::Corrupt)?
                    .as_bytes(),
            ),
            request_sha256: view["request_sha256"].as_str().map(str::to_owned),
            usage_complete: input.is_some() && output.is_some(),
            input_tokens: input,
            output_tokens: output,
            duration_ms: insight
                .and_then(|i| i["duration_ms"].as_u64())
                .map(|n| n.to_string()),
            costs: match insight.and_then(|i| i.get("received_cost_metadata")) {
                Some(metadata) => received_reported_cost(metadata),
                None => reported_cost(usage),
            }
            .into_iter()
            .collect(),
            skills,
            skill_provenance_complete: insight.is_some(),
        },
    )
    .await
}
pub async fn backfill_team(tx: &mut Transaction<'_, Any>, team: uuid::Uuid) -> Result<()> {
    let scope = format!("organization:{team}");
    let mut cursor: String = sqlx::query_scalar(
        "SELECT cursor FROM audit_metric_source_cursors WHERE scope=$1 AND source='team_receipts'",
    )
    .bind(&scope)
    .fetch_optional(&mut **tx)
    .await?
    .unwrap_or_default();
    loop {
        let rows=sqlx::query("SELECT i.id,i.run_id,i.payload AS intent_payload,r.payload AS receipt_payload,t.payload AS run_payload FROM team_effect_intents i JOIN team_effect_receipts r ON r.effect_id=i.id JOIN team_runs t ON t.id=i.run_id WHERE t.team_id=$1 AND i.id>$2 AND i.kind='model' AND r.outcome='completed' AND r.source='original_worker_ack' ORDER BY i.id LIMIT 128").bind(team.to_string()).bind(&cursor).fetch_all(&mut **tx).await?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            cursor = row.try_get("id")?;
            let intent: serde_json::Value =
                serde_json::from_str(&row.try_get::<String, _>("intent_payload")?)
                    .map_err(|_| IdentityError::Corrupt)?;
            let receipt: serde_json::Value =
                serde_json::from_str(&row.try_get::<String, _>("receipt_payload")?)
                    .map_err(|_| IdentityError::Corrupt)?;
            let run: TeamRun = serde_json::from_str(&row.try_get::<String, _>("run_payload")?)
                .map_err(|_| IdentityError::Corrupt)?;
            if run.team_id != team || run.id.to_string() != row.try_get::<String, _>("run_id")? {
                return Err(IdentityError::Corrupt);
            }
            team_receipt(tx, team, run.id, &intent["view"], &receipt, run.created_at).await?;
        }
    }
    sqlx::query("INSERT INTO audit_metric_source_cursors(scope,source,cursor) VALUES($1,'team_receipts',$2) ON CONFLICT(scope,source) DO UPDATE SET cursor=excluded.cursor").bind(scope).bind(cursor).execute(&mut **tx).await?;
    let scope = format!("organization:{team}");
    let mut cursor: String = sqlx::query_scalar(
        "SELECT cursor FROM audit_metric_source_cursors WHERE scope=$1 AND source='team_runs'",
    )
    .bind(&scope)
    .fetch_optional(&mut **tx)
    .await?
    .unwrap_or_default();
    loop {
        let rows = sqlx::query(
            "SELECT id,payload FROM team_runs WHERE team_id=$1 AND id>$2 ORDER BY id LIMIT 128",
        )
        .bind(team.to_string())
        .bind(&cursor)
        .fetch_all(&mut **tx)
        .await?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            cursor = row.try_get("id")?;
            let run: TeamRun = serde_json::from_str(&row.try_get::<String, _>("payload")?)
                .map_err(|_| IdentityError::Corrupt)?;
            if run.team_id != team || run.id.to_string() != cursor {
                return Err(IdentityError::Corrupt);
            }
            // Never invent a historical start from the queued/created timestamp.
            if run.status != TeamRunStatus::Running {
                team_run(tx, &run).await?;
            }
        }
    }
    sqlx::query("INSERT INTO audit_metric_source_cursors(scope,source,cursor) VALUES($1,'team_runs',$2) ON CONFLICT(scope,source) DO UPDATE SET cursor=excluded.cursor").bind(scope).bind(cursor).execute(&mut **tx).await?;
    Ok(())
}

pub async fn team_run(tx: &mut Transaction<'_, Any>, run: &TeamRun) -> Result<()> {
    let scope = format!("organization:{}", run.team_id);
    if run.status == TeamRunStatus::Running {
        sqlx::query("INSERT INTO audit_run_starts(scope,run_id,started_at) VALUES($1,$2,$3) ON CONFLICT(scope,run_id) DO NOTHING").bind(scope).bind(run.id.to_string()).bind(run.updated_at).execute(&mut **tx).await?;
        return Ok(());
    }
    if !matches!(
        run.status,
        TeamRunStatus::Completed | TeamRunStatus::Failed | TeamRunStatus::Cancelled
    ) {
        return Ok(());
    }
    let start: Option<i64> =
        sqlx::query_scalar("SELECT started_at FROM audit_run_starts WHERE scope=$1 AND run_id=$2")
            .bind(&scope)
            .bind(run.id.to_string())
            .fetch_optional(&mut **tx)
            .await?;
    append(
        tx,
        AuditMetricFact {
            sequence: 0,
            id: format!("run:{}:{}", run.id, run.updated_at),
            scope,
            run_id: run.id.to_string(),
            kind: "run".into(),
            observed_at: run.updated_at,
            time_basis: "server_recorded".into(),
            source_kind: "team_terminal_status".into(),
            source_sha256: hash(
                serde_json::to_vec(&(run.id, &run.status, run.updated_at))
                    .map_err(|_| IdentityError::Corrupt)?
                    .as_slice(),
            ),
            request_sha256: None,
            input_tokens: None,
            output_tokens: None,
            usage_complete: false,
            duration_ms: if run.status == TeamRunStatus::Cancelled {
                None
            } else {
                start
                    .and_then(|s| run.updated_at.checked_sub(s))
                    .filter(|n| *n >= 0)
                    .map(|n| n.to_string())
            },
            costs: Vec::new(),
            skills: Vec::new(),
            skill_provenance_complete: true,
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fact(id: &str, scope: &str, time: i64) -> AuditMetricFact {
        AuditMetricFact {
            sequence: 0,
            id: id.into(),
            scope: scope.into(),
            run_id: "run-1".into(),
            kind: "model".into(),
            observed_at: time,
            time_basis: "server_recorded".into(),
            source_kind: "fixture_received_usage".into(),
            source_sha256: "a".repeat(64),
            request_sha256: Some("b".repeat(64)),
            input_tokens: Some("12".into()),
            output_tokens: Some("3".into()),
            usage_complete: true,
            duration_ms: Some("10".into()),
            costs: vec![AuditCost {
                units: "7".into(),
                scale: 3,
                currency: Some("EUR".into()),
                basis: "provider_reported".into(),
                price_source_sha256: None,
            }],
            skills: vec![AuditSkillUse {
                id: "skill-1".into(),
                revision: 2,
                instructions_sha256: "c".repeat(64),
            }],
            skill_provenance_complete: true,
        }
    }
    #[test]
    fn explicit_costs_keep_unknown_currency_and_reject_rounding() {
        let v = reported_cost(&serde_json::json!({"response_cost":"0.000000123","currency":"EUR"}))
            .unwrap();
        assert_eq!(v.units, "123");
        assert_eq!(v.scale, 9);
        assert_eq!(v.currency.as_deref(), Some("EUR"));
        assert!(reported_cost(&serde_json::json!({"cost":"1e-3"})).is_none());
        assert!(reported_cost(&serde_json::json!({"cost":"0.0000000001"})).is_none());
        assert!(reported_cost(&serde_json::json!({"cost":"-1"})).is_none());
        assert_eq!(
            reported_cost(&serde_json::json!({"cost":"0.2"}))
                .unwrap()
                .currency,
            None
        );
        let mut metadata = serde_json::json!({"format":1,"completed_stream":true,"provider_reported":true,"body_usage":{"cost":"0.12","currency":"EUR"},"headers":{"x-litellm-response-cost":"0.120","x-litellm-response-cost-currency":"EUR"}});
        assert_eq!(received_reported_cost(&metadata).unwrap().units, "12");
        metadata["headers"]["x-litellm-response-cost"] = serde_json::json!("0.121");
        assert!(received_reported_cost(&metadata).is_none());
    }
    #[tokio::test]
    async fn scoped_receipts_snapshot_pagination_and_tamper_are_checked() {
        sqlx::any::install_default_drivers();
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        for statement in include_str!("audit_insights_schema.sql")
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            sqlx::query(statement).execute(&mut *tx).await.unwrap();
        }
        let org = format!("organization:{}", uuid::Uuid::from_u128(9));
        let first = fact("model:run-1:attempt-1", "personal", 120);
        append(&mut tx, first.clone()).await.unwrap();
        append(&mut tx, first.clone()).await.unwrap();
        let mut changed = first;
        changed.input_tokens = Some("13".into());
        assert!(matches!(
            append(&mut tx, changed).await,
            Err(IdentityError::Conflict)
        ));
        append(&mut tx, fact("model:run-1:attempt-0", "personal", 60))
            .await
            .unwrap();
        append(&mut tx, fact("model:run-2:attempt-1", "personal", 140))
            .await
            .unwrap();
        append(&mut tx, fact("model:other:attempt-1", &org, 120))
            .await
            .unwrap();
        let q = AuditInsightsQuery {
            from_ms: Some(100),
            until_ms: Some(150),
            limit: 1,
            ..Default::default()
        };
        let initial = query(&mut tx, "personal", q.clone(), 200).await.unwrap();
        assert_eq!(initial.current.model_receipts, "2");
        assert_eq!(initial.previous.model_receipts, "1");
        assert_eq!(initial.current.input_tokens, "24");
        assert_eq!(initial.current.skill_context_uses, "2");
        assert_eq!(initial.current.costs[0].units, "14");
        assert!(initial.has_more);
        append(&mut tx, fact("model:run-3:attempt-1", "personal", 145))
            .await
            .unwrap();
        let next = query(
            &mut tx,
            "personal",
            AuditInsightsQuery {
                after_sequence: initial.next_sequence,
                snapshot_sequence: Some(initial.snapshot_sequence),
                ..q.clone()
            },
            200,
        )
        .await
        .unwrap();
        assert_eq!(next.current.model_receipts, "2");
        assert_eq!(next.facts.len(), 1);
        assert!(!next.has_more);
        assert_eq!(
            query(&mut tx, &org, q.clone(), 200)
                .await
                .unwrap()
                .current
                .model_receipts,
            "1"
        );
        sqlx::query("UPDATE audit_metric_facts SET payload='secret-original-text' WHERE scope='personal' AND sequence=1").execute(&mut *tx).await.unwrap();
        assert!(matches!(
            query(&mut tx, "personal", q, 200).await,
            Err(IdentityError::Corrupt)
        ));
        tx.rollback().await.unwrap();
        pool.close().await;
    }
}
