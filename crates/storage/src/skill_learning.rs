use super::*;
use sqlx::{Any, Transaction};

/// Called in the same transaction as the terminal run snapshot. This records
/// observed outcomes, not inferred answer quality, and never rewrites a skill.
pub(crate) async fn record(tx: &mut Transaction<'_, Any>, run: &StoredRun) -> Result<()> {
    if !terminal(&run.record.state) {
        return Ok(());
    }
    let Some(row) =
        sqlx::query("SELECT payload,payload_sha256 FROM audit_run_skills WHERE run_id=$1")
            .bind(&run.record.id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?
    else {
        return Ok(());
    };
    let raw: String = row.try_get("payload").map_err(storage_error)?;
    if hash(raw.as_bytes())
        != row
            .try_get::<String, _>("payload_sha256")
            .map_err(storage_error)?
    {
        return Err(Error::Conflict);
    }
    let selected: Value = parse(&raw)?;
    for item in selected.as_array().into_iter().flatten() {
        let evidence = &item["evidence"];
        let (Some(skill_id), Some(revision)) =
            (evidence["id"].as_str(), evidence["revision"].as_i64())
        else {
            continue;
        };
        let Some(snapshot) = item.get("learning_snapshot").filter(|v| !v.is_null()) else {
            continue;
        };
        let skill: Skill = serde_json::from_value(snapshot.clone()).map_err(storage_error)?;
        if !skill.auto_learn || skill.id != skill_id || skill.revision != revision {
            continue;
        }
        let mut tools = std::collections::BTreeSet::new();
        for message in &run.checkpoint.messages {
            for tool in message["tool_calls"].as_array().into_iter().flatten() {
                if let Some(name) = tool["function"]["name"].as_str() {
                    tools.insert(name.to_owned());
                }
            }
        }
        let tools: Vec<_> = tools.into_iter().collect();
        let reason = if run.record.state == RunState::Completed {
            "Run completed. Completion is an execution outcome; answer quality has not been rated."
                .to_owned()
        } else {
            format!(
                "Run ended as {:?}; recorded error: {}. No automatic retry or quality score.",
                run.record.state,
                run.record.error_code.as_deref().unwrap_or("unknown")
            )
        };
        let proposal = if run.record.state == RunState::Completed && !tools.is_empty() {
            let base = skill.instructions.chars().take(12000).collect::<String>();
            Some(format!(
                "{base}\n\nObserved execution pattern from run {} (review before adopting): tools {}. Confirm task inputs and inspect the actual tool results before reporting completion. This historical pattern grants no tool permissions.",
                run.record.id,
                tools.join(", ")
            ))
        } else if run.record.state == RunState::Failed {
            Some(format!(
                "{}\n\nBefore execution, check prerequisites and report unavailable capabilities explicitly. A previous run ({}) failed with {}. Stop and request missing information instead of repeating an uncertain operation.",
                skill.instructions.chars().take(12000).collect::<String>(),
                run.record.id,
                run.record.error_code.as_deref().unwrap_or("unknown")
            ))
        } else {
            None
        };
        let outcome = SkillLearningOutcome {
            run_id: run.record.id.clone(),
            skill_id: skill_id.into(),
            skill_revision: revision,
            recorded_at: run.record.updated_at,
            state: run.record.state.clone(),
            reason,
            observed_tools: tools,
            proposed_instructions: proposal,
        };
        sqlx::query("INSERT INTO native_skill_learning(project_id,skill_id,skill_revision,run_id,recorded_at,payload) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(skill_id,skill_revision,run_id) DO NOTHING").bind(&run.record.spec.project_id).bind(skill_id).bind(revision).bind(&run.record.id).bind(outcome.recorded_at).bind(json_string(&outcome)?).execute(&mut **tx).await.map_err(storage_error)?;
    }
    Ok(())
}
impl Store {
    pub async fn skill_learning_history(
        &self,
        project: &str,
        skill: &str,
    ) -> Result<SkillLearningHistory> {
        let rows=sqlx::query("SELECT payload FROM native_skill_learning WHERE project_id=$1 AND skill_id=$2 ORDER BY recorded_at DESC,run_id LIMIT 100").bind(project).bind(skill).fetch_all(&self.pool).await.map_err(storage_error)?;
        Ok(SkillLearningHistory {
            items: rows
                .into_iter()
                .map(|r| parse(&r.try_get::<String, _>("payload").map_err(storage_error)?))
                .collect::<Result<_>>()?,
        })
    }
}
