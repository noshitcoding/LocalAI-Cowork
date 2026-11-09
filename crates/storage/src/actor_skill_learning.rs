//! Actor-private observed outcomes. Proposals are inert encrypted content;
//! adoption uses the existing explicit skill revision/CAS request journal.
use super::*;
use crate::android_private_server::{id, row};
use crate::android_private_workspace as workspace;
use cowork_identity::{ContentAction, Principal};
use cowork_secrets::{KeyId, KeyStore};
use sqlx::{Any, Transaction, any::AnyRow};
use std::collections::{BTreeMap, BTreeSet};

struct Evidence {
    definition: Value,
    nodes: BTreeSet<String>,
    tools: BTreeSet<String>,
}
fn proposal(source: &str, state: &str, run: Uuid, tools: &[String]) -> Option<String> {
    let suffix = if state == "completed" && !tools.is_empty() {
        format!(
            "\n\nObserved execution from run {run} (review before adopting): tool receipts {}. Check the actual inputs and results before reporting completion. This history grants no tool permissions and is not an answer-quality assessment.",
            tools.join(", ")
        )
    } else if state == "failed" {
        format!(
            "\n\nBefore execution, check prerequisites and report unavailable capabilities explicitly. Run {run} failed. Stop and request missing information instead of repeating an uncertain operation."
        )
    } else {
        return None;
    };
    let value = format!("{source}{suffix}");
    (value.len() <= 64000).then_some(value)
}
async fn collect(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    cp: &Value,
    keys: &dyn KeyStore,
    all: &mut BTreeMap<(Uuid, i64), Evidence>,
) -> Result<()> {
    let spec: RunSpec = serde_json::from_value(cp["spec"].clone()).map_err(storage_error)?;
    let original: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
    let scoped = if let Some(context) = cp.get("frozen_role_context") {
        context.clone()
    } else {
        crate::android_private_workflows::scoped_runtime_context(
            &original["spec"]["input"]["frozen_runtime_context"],
            spec.agent_config.as_ref(),
        )?
    };
    let node = cp["workflow_node"]["node_id"].as_str();
    let prompt = cp["execution_goal"].as_str().unwrap_or(&spec.prompt);
    let prefix = node.map(|node| format!("workflow:{}:%", hash(node.as_bytes())));
    let steps=sqlx::query("SELECT sequence,intent,request_sha FROM android_private_loop_steps WHERE run_id=$1 AND kind!='model' AND state='received' AND ($2 IS NULL OR invocation_id LIKE $2) ORDER BY sequence LIMIT 4097").bind(id(r,"id")?.to_string()).bind(prefix).fetch_all(&mut **tx).await.map_err(storage_error)?;
    if steps.len() > 4096 {
        return Err(Error::BudgetExceeded);
    }
    let mut tools = BTreeSet::new();
    for step in steps {
        let sequence: i64 = step.try_get("sequence").map_err(storage_error)?;
        let intent: Value = serde_json::from_slice(&workspace::open_value(
            keys,
            r,
            &format!("intent:{sequence}"),
            &parse(&step.try_get::<String, _>("intent").map_err(storage_error)?)?,
        )?)
        .map_err(storage_error)?;
        if canonical_hash(&intent)
            != step
                .try_get::<String, _>("request_sha")
                .map_err(storage_error)?
        {
            return Err(Error::Forbidden);
        }
        if let Some(name) = intent["tool"]["name"]
            .as_str()
            .filter(|name| name.len() <= 200)
        {
            tools.insert(name.to_owned());
        }
    }
    for item in scoped["skill"].as_array().into_iter().flatten() {
        if item["definition"]["auto_learn"] != true
            || !crate::android_private_workflows::execution::actor_skill_applies(
                &item["definition"],
                prompt,
            )
        {
            continue;
        }
        let skill = crate::android_personal::wire_uuid(item, "entity_id")?;
        let revision = item["revision"]
            .as_i64()
            .filter(|r| *r > 0)
            .ok_or(Error::Forbidden)?;
        // Reauthenticate the immutable historical definition, even if the
        // current skill was edited or disabled while the original run worked.
        let raw:Option<String>=sqlx::query_scalar("SELECT payload FROM android_wire_sync_changes WHERE owner_id=$1 AND entity_type='skill' AND entity_id=$2 AND revision=$3 AND team_id IS NULL AND tombstone=0 ORDER BY cursor DESC LIMIT 1").bind(id(r,"owner_id")?.to_string()).bind(skill.to_string()).bind(revision).fetch_optional(&mut **tx).await.map_err(storage_error)?;
        let historical: Value = parse(&raw.ok_or(Error::Forbidden)?)?;
        if historical["etag"] != item["etag"]
            || crate::android_wire_sync::runtime_definition("skill", &historical["payload"])?
                != item["definition"]
        {
            return Err(Error::Forbidden);
        }
        let fragment = crate::android_private_workflows::render_actor_skills(
            &json!({"skill":[item]}),
            prompt,
        )?;
        if fragment.is_empty()
            || !cp["messages"].as_array().is_some_and(|messages| {
                messages.iter().any(|message| {
                    message["role"] == "system"
                        && message["content"]
                            .as_str()
                            .is_some_and(|content| content.contains(&fragment))
                })
            })
        {
            continue;
        }
        let entry = all.entry((skill, revision)).or_insert_with(|| Evidence {
            definition: item["definition"].clone(),
            nodes: BTreeSet::new(),
            tools: BTreeSet::new(),
        });
        if entry.definition != item["definition"] {
            return Err(Error::Forbidden);
        }
        if let Some(node) = node {
            entry.nodes.insert(node.into());
        }
        entry.tools.extend(tools.iter().cloned());
    }
    Ok(())
}
pub(crate) async fn capture(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    cp: &Value,
    state: &str,
    keys: &dyn KeyStore,
    key: KeyId,
    current: i64,
) -> Result<()> {
    if !matches!(state, "completed" | "failed" | "interrupted") {
        return Ok(());
    }
    let mut evidence = BTreeMap::new();
    if cp["execution_kind"] == "workflow" {
        let nodes:Vec<String>=sqlx::query_scalar("SELECT context_version_id FROM android_private_workflow_nodes WHERE run_id=$1 AND context_version_id IS NOT NULL ORDER BY node_id LIMIT 129").bind(id(r,"id")?.to_string()).fetch_all(&mut **tx).await.map_err(storage_error)?;
        if nodes.len() > 128 {
            return Err(Error::BudgetExceeded);
        }
        for version in nodes {
            let value: Value =
                serde_json::from_slice(&workspace::version_bytes(tx, r, &version, keys).await?)
                    .map_err(storage_error)?;
            collect(tx, r, &value, keys, &mut evidence).await?;
        }
    } else {
        collect(tx, r, cp, keys, &mut evidence).await?;
    }
    for ((skill, revision), entry) in evidence {
        let existing:i64=sqlx::query_scalar("SELECT COUNT(*) FROM actor_skill_learning WHERE owner_id=$1 AND skill_id=$2 AND skill_revision=$3 AND run_id=$4").bind(id(r,"owner_id")?.to_string()).bind(skill.to_string()).bind(revision).bind(id(r,"id")?.to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?;
        if existing != 0 {
            continue;
        }
        let tools: Vec<_> = entry.tools.into_iter().collect();
        let proposed = proposal(
            entry.definition["prompt_template"]
                .as_str()
                .ok_or(Error::Forbidden)?,
            state,
            id(r, "id")?,
            &tools,
        );
        let reason = format!(
            "Run ended as {state}. This records the selected frozen skill and observed tool receipts, not answer quality or proof of skill impact.{}",
            if proposed.is_none() {
                " No bounded improvement proposal was produced."
            } else {
                " Review the proposal before creating a new skill revision."
            }
        );
        let outcome = ActorSkillLearningOutcome {
            run_id: id(r, "id")?,
            project_id: id(r, "project_id")?,
            skill_id: skill,
            skill_revision: revision,
            node_ids: entry.nodes.into_iter().collect(),
            recorded_at: current,
            state: state.into(),
            reason,
            observed_tools: tools,
            source_definition: entry.definition,
            proposed_prompt_template: proposed,
        };
        let bytes = serde_json::to_vec(&outcome).map_err(storage_error)?;
        let (count,size):(i64,i64)=sqlx::query_as("SELECT COUNT(*),CAST(COALESCE(SUM(size),0) AS BIGINT) FROM android_private_loop_versions WHERE run_id=$1").bind(id(r,"id")?.to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?;
        if count >= 20000
            || size
                .checked_add(bytes.len() as i64)
                .is_none_or(|size| size > 2 * 1024 * 1024 * 1024)
        {
            return Err(Error::BudgetExceeded);
        }
        let version = workspace::add_version(
            tx,
            r,
            &format!(".cowork-system/skill-learning/v1/{skill}/{revision}"),
            "artifact",
            &bytes,
            keys,
            key,
            current,
        )
        .await?;
        sqlx::query("INSERT INTO actor_skill_learning(owner_id,project_id,skill_id,skill_revision,run_id,recorded_at,version_id,payload_sha256) VALUES($1,$2,$3,$4,$5,$6,$7,$8)").bind(id(r,"owner_id")?.to_string()).bind(id(r,"project_id")?.to_string()).bind(skill.to_string()).bind(revision).bind(id(r,"id")?.to_string()).bind(current).bind(version).bind(hash(&bytes)).execute(&mut **tx).await.map_err(storage_error)?;
    }
    Ok(())
}
impl Store {
    pub async fn actor_skill_learning(
        &self,
        a: &Principal,
        project: Uuid,
        skill: Uuid,
        keys: &dyn KeyStore,
    ) -> Result<ActorSkillLearningHistory> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .require_global_session_in_transaction(&mut tx, a)
            .await
            .map_err(team_service::identity_error)?;
        self.android_personal_permit(&mut tx, a, project, ContentAction::Read)
            .await?;
        let entries=sqlx::query("SELECT run_id,version_id,payload_sha256 FROM actor_skill_learning WHERE owner_id=$1 AND project_id=$2 AND skill_id=$3 ORDER BY recorded_at DESC,run_id LIMIT 20").bind(a.identity_id().to_string()).bind(project.to_string()).bind(skill.to_string()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let mut items = vec![];
        for entry in entries {
            let run = Uuid::parse_str(
                &entry
                    .try_get::<String, _>("run_id")
                    .map_err(storage_error)?,
            )
            .map_err(storage_error)?;
            let r = row(&mut tx, run).await?;
            if id(&r, "owner_id")? != a.identity_id() || id(&r, "project_id")? != project {
                return Err(Error::Forbidden);
            }
            let bytes = workspace::version_bytes(
                &mut tx,
                &r,
                &entry
                    .try_get::<String, _>("version_id")
                    .map_err(storage_error)?,
                keys,
            )
            .await?;
            if hash(&bytes)
                != entry
                    .try_get::<String, _>("payload_sha256")
                    .map_err(storage_error)?
            {
                return Err(Error::Forbidden);
            }
            let outcome: ActorSkillLearningOutcome =
                serde_json::from_slice(&bytes).map_err(storage_error)?;
            if outcome.run_id != run || outcome.project_id != project || outcome.skill_id != skill {
                return Err(Error::Forbidden);
            }
            items.push(outcome);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(ActorSkillLearningHistory { items })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_inert_flags_keep_their_projection_and_new_opt_in_is_explicit() {
        let baseline = json!({"name":"Legacy","prompt_template":"Original"});
        let mut old = baseline.clone();
        old["auto_learn"] = json!(true);
        assert_eq!(
            crate::android_wire_sync::runtime_definition("skill", &baseline).unwrap(),
            crate::android_wire_sync::runtime_definition("skill", &old).unwrap()
        );
        old["auto_learn_policy"] = json!("reviewed-outcomes/1");
        assert_eq!(
            crate::android_wire_sync::runtime_definition("skill", &old).unwrap()["auto_learn"],
            true
        );
        old["auto_learn"] = json!(false);
        assert_eq!(
            crate::android_wire_sync::runtime_definition("skill", &old).unwrap(),
            baseline
        );
    }
    #[test]
    fn proposals_keep_the_complete_template_and_never_invent_quality() {
        let run = Uuid::new_v4();
        let source = "Original {{input}}";
        let value = proposal(source, "completed", run, &["browser".into()]).unwrap();
        assert!(value.starts_with(source));
        assert!(value.contains("not an answer-quality assessment"));
        assert!(proposal(&"🧠".repeat(16000), "failed", run, &[]).is_none());
        assert!(proposal(source, "interrupted", run, &[]).is_none());
    }
}
