//! Per-account, per-chat copied composer values with immutable retry receipts.
use super::*;
use crate::android_devices::{device_invalid, device_writer};
use cowork_identity::{ContentAction, Principal};
use sqlx::{Any, Transaction};

async fn permit(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    a: &Principal,
    thread: Uuid,
    action: ContentAction,
) -> Result<Uuid> {
    let project: Option<String> = sqlx::query_scalar(
        "SELECT project_id FROM android_personal_threads WHERE id=$1 AND deleted=0",
    )
    .bind(thread.to_string())
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage_error)?;
    let project = Uuid::parse_str(&project.ok_or(Error::NotFound)?).map_err(storage_error)?;
    store
        .android_personal_permit(tx, a, project, action)
        .await?;
    // Shared project content does not share private model/library preferences.
    store
        .identity()
        .require_global_session_in_transaction(tx, a)
        .await
        .map_err(team_service::identity_error)?;
    Ok(project)
}
fn validate(v: &ActorChatRunConfiguration) -> Result<()> {
    Uuid::parse_str(&v.model_profile_id).map_err(|_| device_invalid())?;
    if v.provider_revision < 1 {
        return Err(device_invalid());
    }
    validate_agent_values(&v.run_configuration.agent_config)?;
    validate_budgets(&v.run_configuration.budgets)?;
    for (ids, limit) in [
        (&v.skill_ids, 100),
        (&v.memory_ids, 100),
        (&v.mcp_metadata_ids, 16),
    ] {
        if ids.len() > limit
            || ids.iter().collect::<std::collections::BTreeSet<_>>().len() != ids.len()
        {
            return Err(device_invalid());
        }
        for id in ids {
            Uuid::parse_str(id).map_err(|_| device_invalid())?;
        }
    }
    if v.expected_mcp_revisions.len() > 16
        || v.expected_mcp_revisions
            .iter()
            .any(|(id, r)| *r < 1 || !v.mcp_metadata_ids.contains(id))
    {
        return Err(device_invalid());
    }
    if serde_json::to_vec(v).map_err(storage_error)?.len() > 384 * 1024 {
        return Err(Error::BudgetExceeded);
    }
    Ok(())
}
pub(crate) async fn check_revision(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    a: &Principal,
    thread: Uuid,
    expected: i64,
) -> Result<()> {
    permit(store, tx, a, thread, ContentAction::Execute).await?;
    let revision: Option<i64> = sqlx::query_scalar(
        "SELECT revision FROM actor_chat_configurations WHERE owner_id=$1 AND thread_id=$2",
    )
    .bind(a.identity_id().to_string())
    .bind(thread.to_string())
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage_error)?;
    if expected < 0 || revision.unwrap_or(0) != expected {
        return Err(Error::Conflict);
    }
    Ok(())
}
impl Store {
    /// Freeze policy defaults once at admission; workers never reread mutable preferences.
    pub async fn freeze_actor_run_defaults(&self, a: &Principal, input: &mut Value) -> Result<()> {
        let prefs = self
            .library_preferences(crate::library_config::LibraryOwner::Actor(a))
            .await?;
        let mut raw = input
            .get("run_configuration")
            .or_else(|| {
                input
                    .get("task_config")
                    .and_then(|v| v.get("run_configuration"))
            })
            .cloned()
            .unwrap_or_else(|| json!({}));
        let values = raw.as_object_mut().ok_or_else(device_invalid)?;
        values.entry("agent_config").or_insert_with(|| json!({}));
        values
            .entry("access_mode")
            .or_insert_with(|| json!(prefs.value.default_access_mode));
        values
            .entry("budgets")
            .or_insert_with(|| json!(prefs.value.default_budgets));
        let mut config: ActorRunConfiguration =
            serde_json::from_value(raw).map_err(|_| device_invalid())?;
        freeze_behavior_defaults(&mut config.agent_config, &prefs.value);
        if prefs.value.read_only_fs_mode {
            config.access_mode = AccessMode::ReadOnly;
        }
        let mut mode = config.agent_config.execution_mode;
        if let Some(skills) = input["frozen_runtime_context"]["skill"].as_array() {
            let parent =
                input["private_workflow"]["context_selection"]["parent_skill_ids"].as_array();
            for skill in skills {
                if !crate::android_private_workflows::execution::actor_skill_applies(
                    &skill["definition"],
                    input["prompt"]
                        .as_str()
                        .or_else(|| input["text"].as_str())
                        .unwrap_or(""),
                ) {
                    continue;
                }
                if parent.is_some_and(|ids| !ids.contains(&skill["entity_id"])) {
                    continue;
                }
                let selected = match skill["definition"]["run_mode"]
                    .as_str()
                    .unwrap_or("execute")
                {
                    "execute" => AgentExecutionMode::Execute,
                    "plan" => AgentExecutionMode::Plan,
                    "hybrid" => AgentExecutionMode::Hybrid,
                    _ => return Err(device_invalid()),
                };
                mode = Some(narrow_execution_mode(
                    mode.unwrap_or(AgentExecutionMode::Execute),
                    selected,
                ));
            }
        }
        config.agent_config.execution_mode = mode;
        validate_agent_values(&config.agent_config)?;
        input.as_object_mut().ok_or_else(device_invalid)?.insert(
            "run_configuration".into(),
            serde_json::to_value(config).map_err(storage_error)?,
        );
        Ok(())
    }
    pub async fn actor_chat_configuration(
        &self,
        a: &Principal,
        thread: Uuid,
    ) -> Result<ActorChatConfiguration> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let project = permit(self, &mut tx, a, thread, ContentAction::Read).await?;
        let payload: Option<String> = sqlx::query_scalar(
            "SELECT payload FROM actor_chat_configurations WHERE owner_id=$1 AND thread_id=$2",
        )
        .bind(a.identity_id().to_string())
        .bind(thread.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?;
        let result = payload
            .map(|p| parse(&p))
            .transpose()?
            .unwrap_or(ActorChatConfiguration {
                thread_id: thread.to_string(),
                project_id: project.to_string(),
                revision: 0,
                updated_at: 0,
                value: None,
            });
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn write_actor_chat_configuration(
        &self,
        a: &Principal,
        thread: Uuid,
        input: &WriteActorChatConfiguration,
    ) -> Result<ActorChatConfiguration> {
        validate(&input.value)?;
        if input.expected_revision < 0
            || input.idempotency_key.is_empty()
            || input.idempotency_key.len() > 200
            || input.idempotency_key.chars().any(char::is_control)
        {
            return Err(device_invalid());
        }
        let request_sha = canonical_hash(&(thread, input));
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let stamp = device_writer(&mut tx).await?;
        let project = permit(self, &mut tx, a, thread, ContentAction::Write).await?;
        if let Some((sha,raw))=sqlx::query_as::<_,(String,String)>("SELECT request_sha,receipt FROM actor_chat_configuration_receipts WHERE owner_id=$1 AND operation_key=$2").bind(a.identity_id().to_string()).bind(&input.idempotency_key).fetch_optional(&mut *tx).await.map_err(storage_error)?{
            if sha!=request_sha{return Err(Error::Conflict);}
            let result=parse(&raw)?;tx.commit().await.map_err(storage_error)?;return Ok(result);
        }
        check_revision(self, &mut tx, a, thread, input.expected_revision).await?;
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM actor_chat_configuration_receipts WHERE owner_id=$1",
        )
        .bind(a.identity_id().to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        if count >= 100000 {
            return Err(Error::BudgetExceeded);
        }
        let value = ActorChatConfiguration {
            thread_id: thread.to_string(),
            project_id: project.to_string(),
            revision: input
                .expected_revision
                .checked_add(1)
                .ok_or(Error::BudgetExceeded)?,
            updated_at: stamp,
            value: Some(input.value.clone()),
        };
        let raw = json_string(&value)?;
        sqlx::query("INSERT INTO actor_chat_configurations(owner_id,thread_id,project_id,revision,payload) VALUES($1,$2,$3,$4,$5) ON CONFLICT(owner_id,thread_id) DO UPDATE SET revision=excluded.revision,payload=excluded.payload").bind(a.identity_id().to_string()).bind(thread.to_string()).bind(project.to_string()).bind(value.revision).bind(&raw).execute(&mut *tx).await.map_err(storage_error)?;
        sqlx::query("INSERT INTO actor_chat_configuration_receipts(owner_id,operation_key,thread_id,request_sha,receipt) VALUES($1,$2,$3,$4,$5)").bind(a.identity_id().to_string()).bind(&input.idempotency_key).bind(thread.to_string()).bind(request_sha).bind(raw).execute(&mut *tx).await.map_err(storage_error)?;
        permit(self, &mut tx, a, thread, ContentAction::Write).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
}
