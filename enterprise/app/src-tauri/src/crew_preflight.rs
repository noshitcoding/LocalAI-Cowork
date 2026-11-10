//! Pure, installation-independent Crew validation used by diagnostics and execution.
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use crate::{db::{ApiProfileRow, Database}, provider_model_catalog};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CrewReadinessIssue {
    pub code: String,
    pub severity: String,
    pub message: String,
    pub subject_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CrewReadinessReport {
    pub crew_id: String,
    pub definition_revision: String,
    pub checked_at: String,
    pub state: String,
    pub can_run: bool,
    pub runtime_ready: bool,
    pub issues: Vec<CrewReadinessIssue>,
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("").trim()
}

pub fn requires_python_runtime(payload: &Value) -> bool {
    !payload.get("agents").and_then(Value::as_array).is_some_and(|agents| agents.iter().any(|agent|
        agent.get("enabled").and_then(Value::as_bool).unwrap_or(true)
        && agent.get("backendSelection").is_some_and(|selection| text(selection, "backend") == "codex")))
}

/// Shared by preflight and the native API executor, including empty-override fallback.
pub fn resolve_api_execution_profile(database: &Database, profile_id: &str, model_override: Option<&str>) -> Result<ApiProfileRow, String> {
    let mut profile = database.get_api_profile(profile_id).map_err(|error| error.to_string())?
        .ok_or_else(|| format!("Scheduled API profile '{profile_id}' does not exist"))?;
    profile.model = model_override.map(str::trim).filter(|value| !value.is_empty())
        .unwrap_or(profile.model.trim()).to_string();
    Ok(profile)
}

/// Resolve the same saved profiles used at execution; never trust frontend secrets.
pub fn hydrate_provider_context(payload: &mut Value, database: &Database, mut has_key: impl FnMut(&str) -> Result<bool, String>) -> Result<(), String> {
    if let Some(configs) = payload.get_mut("providerConfigs").and_then(Value::as_object_mut) {
        for config in configs.values_mut().filter(|value| value.is_object()) {
            let id = text(config, "profileId").to_string();
            let profile = if id.is_empty() { None } else { database.get_api_profile(&id).map_err(|error| error.to_string())? };
            provider_model_catalog::attach(config, profile.as_ref());
            if !id.is_empty() {
                config["profileMissing"] = Value::Bool(profile.is_none());
                config["apiKey"] = Value::String(String::new());
                if let Some(profile) = profile {
                    config["authMode"] = Value::String(profile.auth_mode.clone());
                    config["hasApiKey"] = Value::Bool(profile.auth_mode != "bearer" || has_key(&id)?);
                } else { config["hasApiKey"] = Value::Bool(false); }
            }
        }
    }
    if requires_python_runtime(payload) { return Ok(()); }
    if let Some(agents) = payload.get_mut("agents").and_then(Value::as_array_mut) {
        for agent in agents {
            let Some(selection) = agent.get("backendSelection") else { continue; };
            if text(selection, "backend") != "openai-compatible" { continue; }
            let id = text(selection, "profileId").to_string();
            let override_model = agent.get("modelOverride").and_then(Value::as_str)
                .or_else(|| selection.get("model").and_then(Value::as_str));
            let mut config = match resolve_api_execution_profile(database, &id, override_model) {
                Ok(profile) => {
                    let mut config = serde_json::json!({"profileId":profile.id,"model":profile.model,
                        "baseUrl":profile.base_url,"authMode":profile.auth_mode,
                        "hasApiKey":profile.auth_mode != "bearer" || has_key(&id)?});
                    provider_model_catalog::attach(&mut config, Some(&profile));
                    config
                }
                Err(_) => serde_json::json!({"profileId":id,"profileMissing":true}),
            };
            config["apiKey"] = Value::String(String::new());
            agent["resolvedBackend"] = config;
        }
    }
    Ok(())
}

pub fn inspect(payload: &Value, runtime_ready: bool) -> CrewReadinessReport {
    let mut issues = Vec::new();
    let mut issue = |code: &str, warning: bool, message: &str, subject: Option<&str>| {
        issues.push(CrewReadinessIssue { code: code.to_string(),
            severity: if warning { "warning" } else { "error" }.to_string(),
            message: message.to_string(), subject_id: subject.map(str::to_string) });
    };
    let empty = Vec::new();
    let agents = payload.get("agents").and_then(Value::as_array).unwrap_or(&empty);
    let tasks = payload.get("tasks").and_then(Value::as_array).unwrap_or(&empty);
    let active: HashMap<&str, &Value> = agents.iter()
        .filter(|agent| agent.get("enabled").and_then(Value::as_bool).unwrap_or(true))
        .map(|agent| (text(agent, "id"), agent)).collect();
    let hierarchical = text(payload, "process").eq_ignore_ascii_case("hierarchical");
    let manager = text(payload, "managerAgentId");
    if tasks.is_empty() { issue("tasks.empty", false, "At least one task is required.", None); }
    if active.is_empty() { issue("agents.none_active", false, "At least one active agent is required.", None); }
    if hierarchical {
        if manager.is_empty() { issue("manager.required", false, "Hierarchical crew requires a manager agent.", None); }
        else if !active.contains_key(manager) { issue("manager.inactive", false, "The selected manager agent is disabled or missing.", Some(manager)); }
        if !active.keys().any(|id| *id != manager) {
            issue("agents.worker_required", false, "Crew requires at least one active non-manager agent.", None);
        }
        if active.get(manager).is_some_and(|agent| ["tools", "mcpServerNames"].iter()
            .any(|key| agent.get(key).and_then(Value::as_array).is_some_and(|values| !values.is_empty()))) {
            issue("manager.tools_ignored", true, "Manager tools and MCP access are ignored during orchestration.", Some(manager));
        }
    }
    let mut ids = HashSet::new();
    let mut dependencies: HashMap<&str, Vec<&str>> = HashMap::new();
    for task in tasks {
        let id = text(task, "id");
        if id.is_empty() || !ids.insert(id) { issue("task.id_invalid", false, "Task IDs must be present and unique.", Some(id)); }
        let agent = text(task, "agentId");
        if !active.contains_key(agent) { issue("assignment.missing", false, "Task references an inactive or missing agent.", Some(id)); }
        if hierarchical && !manager.is_empty() && agent == manager {
            issue("assignment.manager", false, "Assign tasks to workers, not the orchestration manager.", Some(id));
        }
        dependencies.insert(id, task.get("dependencies").and_then(Value::as_array).unwrap_or(&empty)
            .iter().filter_map(Value::as_str).collect());
    }
    for (id, refs) in &dependencies {
        if refs.iter().any(|reference| !ids.contains(reference)) {
            issue("dependency.missing", false, "Task references an unknown dependency.", Some(id));
        }
    }
    fn cycle<'a>(id: &'a str, graph: &HashMap<&'a str, Vec<&'a str>>, visiting: &mut HashSet<&'a str>, visited: &mut HashSet<&'a str>) -> bool {
        if visited.contains(id) { return false; }
        if !visiting.insert(id) { return true; }
        if graph.get(id).is_some_and(|deps| deps.iter().any(|next| cycle(next, graph, visiting, visited))) { return true; }
        visiting.remove(id); visited.insert(id); false
    }
    let mut visited = HashSet::new();
    if ids.iter().any(|id| cycle(id, &dependencies, &mut HashSet::new(), &mut visited)) {
        issue("dependency.cycle", false, "Cyclic task dependencies are not allowed.", None);
    }
    let native_mixed = !requires_python_runtime(payload);
    if !native_mixed && payload.get("selectedProfileMissing").and_then(Value::as_bool) == Some(true) {
        issue("provider.profile_missing", false, "The selected crew API profile no longer exists. Choose an available API profile.", None);
    }
    for (id, agent) in &active {
        let provider = if text(agent, "providerKind").is_empty() { text(payload, "defaultProvider") } else { text(agent, "providerKind") };
        if (provider == "codex" && !native_mixed) || agent.get("backendSelection").is_some_and(|selection| text(selection, "backend") == "codex") { continue; }
        let key = match provider { "openrouter" => Some("openRouter"), "openai-compatible" => Some("openAICompatible"), _ => None };
        let pinned_api = native_mixed && agent.get("backendSelection").is_some_and(|selection| text(selection, "backend") == "openai-compatible");
        let config = if pinned_api { agent.get("resolvedBackend") }
            else { key.and_then(|key| payload.get("providerConfigs").and_then(|configs| configs.get(key))) };
        let override_model = text(agent, "modelOverride");
        let default_model = text(payload, "defaultModel");
        let model = if pinned_api { config.map(|config| text(config, "model")).unwrap_or("") }
            else if !override_model.is_empty() { override_model } else if !default_model.is_empty() { default_model }
            else { config.map(|config| text(config, "model")).unwrap_or_else(|| payload.get("config").map(|config| text(config, "model")).unwrap_or("")) };
        if model.is_empty() { issue("provider.model_missing", false, "The selected provider has no model.", Some(id)); }
        if let Some(config) = config {
            if config.get("profileMissing").and_then(Value::as_bool) == Some(true) {
                issue("provider.profile_missing", false, "The selected provider profile is not configured.", Some(id));
            }
            if text(config, "baseUrl").is_empty() { issue("provider.endpoint_missing", false, "The selected provider has no endpoint.", Some(id)); }
            if text(config, "authMode") != "none" && text(config, "apiKey").is_empty() && config.get("hasApiKey").and_then(Value::as_bool) != Some(true) {
                issue("provider.credential_missing", false, "The selected provider has no API key.", Some(id));
            }
            if !model.is_empty() && config.get("modelCatalogCheckedAt").and_then(Value::as_i64).is_some_and(|checked_at| provider_model_catalog::is_fresh(checked_at, chrono::Utc::now().timestamp_millis()))
                && config.get("models").and_then(Value::as_array).is_some_and(|models| !models.iter().any(|entry| entry.as_str() == Some(model))) {
                issue("provider.model_unavailable", false, "The saved model is unavailable. Select a model from the current catalog.", Some(id));
            }
        } else if key.is_some() || pinned_api { issue("provider.profile_missing", false, "The selected provider profile is not configured.", Some(id)); }
        if native_mixed && !pinned_api && provider != "ollama" {
            issue("provider.profile_missing", false, "Native mixed execution requires an API profile for this agent.", Some(id));
        }
    }
    if !runtime_ready { issue("runtime.not_ready", false, "Crew runtime is not ready. Initialize or verify it before running.", None); }
    let can_run = !issues.iter().any(|issue| issue.severity == "error");
    CrewReadinessReport {
        crew_id: text(payload, "id").to_string(),
        definition_revision: Sha256::digest(serde_json::to_vec(payload).unwrap_or_default()).iter().map(|byte| format!("{byte:02x}")).collect(),
        checked_at: chrono::Utc::now().to_rfc3339(), state: if can_run { "ready" } else { "blocked" }.to_string(),
        can_run, runtime_ready, issues,
    }
}

/// Normalize only the execution copy; stored definitions and historical snapshots remain intact.
pub fn normalize_manager_access(payload: &mut Value) {
    if !text(payload, "process").eq_ignore_ascii_case("hierarchical") { return; }
    let manager = text(payload, "managerAgentId").to_string();
    if let Some(agents) = payload.get_mut("agents").and_then(Value::as_array_mut) {
        for agent in agents {
            if text(agent, "id") == manager {
                agent["tools"] = serde_json::json!([]);
                agent["mcpServerNames"] = serde_json::json!([]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn fixture() -> Value { json!({ "id":"crew", "process":"hierarchical", "managerAgentId":"manager", "defaultProvider":"codex",
        "agents":[{"id":"manager", "tools":["web_search"]},{"id":"worker"}], "tasks":[{"id":"task", "agentId":"worker"}] }) }
    #[test] fn manager_tools_are_warning_and_effective_access_is_empty() {
        let source = fixture(); let report = inspect(&source, true);
        assert!(report.can_run); assert!(report.issues.iter().any(|issue| issue.code == "manager.tools_ignored"));
        let mut copy = source.clone(); normalize_manager_access(&mut copy);
        assert_eq!(copy["agents"][0]["tools"], json!([])); assert_eq!(source["agents"][0]["tools"], json!(["web_search"]));
    }
    #[test] fn empty_and_runtime_not_ready_cannot_report_green() {
        let report = inspect(&json!({"id":"empty"}), false);
        for code in ["tasks.empty", "agents.none_active", "runtime.not_ready"] { assert!(report.issues.iter().any(|issue| issue.code == code)); }
        assert!(!report.can_run);
    }
    #[test] fn manager_assignment_and_cycles_are_blocked() {
        let mut payload = fixture(); payload["tasks"][0]["agentId"] = json!("manager"); payload["tasks"][0]["dependencies"] = json!(["task"]);
        let report = inspect(&payload, true);
        for code in ["assignment.manager", "dependency.cycle"] { assert!(report.issues.iter().any(|issue| issue.code == code)); }
    }
    #[test] fn definition_hash_changes_on_edit() {
        let first = fixture(); let mut second = first.clone(); second["name"] = json!("edited");
        assert_ne!(inspect(&first, true).definition_revision, inspect(&second, true).definition_revision);
    }
    #[test] fn unauthenticated_local_api_does_not_require_a_credential() {
        let payload = json!({ "id":"local", "agents":[{"id":"worker", "providerKind":"openai-compatible"}],
            "tasks":[{"id":"task", "agentId":"worker"}], "providerConfigs":{"openAICompatible":{
                "baseUrl":"http://localhost:1234/v1", "model":"local-model", "authMode":"none" }} });
        assert!(inspect(&payload, true).can_run);
    }
    #[test] fn disabled_codex_agent_cannot_skip_python_readiness() {
        let payload = json!({ "agents":[{"id":"off", "enabled":false, "backendSelection":{"backend":"codex"}}] });
        assert!(requires_python_runtime(&payload));
    }
    fn mixed_fixture(profile_id: &str) -> Value {
        json!({"id":"mixed", "defaultProvider":"openrouter", "defaultModel":"unrelated-crew-model",
            "selectedProfileMissing":true,
            "providerConfigs":{"openRouter":{"profileId":"missing-unused-default","baseUrl":"", "model":""}},
            "agents":[{"id":"codex","backendSelection":{"backend":"codex"}},
                {"id":"api","providerKind":"openrouter","backendSelection":{"backend":"openai-compatible","profileId":profile_id,"model":"selection-model"}}],
            "tasks":[{"id":"task","agentId":"api"}]})
    }
    #[test] fn mixed_pinned_api_uses_its_actual_saved_profile_not_the_crew_default() {
        let db = Database::open_in_memory().unwrap();
        let mut payload = mixed_fixture("default-ollama");
        hydrate_provider_context(&mut payload, &db, |_| Ok(false)).unwrap();
        assert!(inspect(&payload, true).can_run);
        assert_eq!(payload["agents"][1]["resolvedBackend"]["model"], json!("selection-model"));
        assert_eq!(payload["agents"][1]["resolvedBackend"]["baseUrl"], json!("http://localhost:11434/v1"));
        assert_eq!(payload["agents"][1]["resolvedBackend"]["authMode"], json!("none"));
    }
    #[test] fn mixed_api_model_precedence_matches_the_shared_executor_resolver() {
        let db = Database::open_in_memory().unwrap();
        for override_model in [None, Some(" agent-model "), Some("  ")] {
            let mut payload = mixed_fixture("default-ollama");
            if let Some(model) = override_model { payload["agents"][1]["modelOverride"] = json!(model); }
            hydrate_provider_context(&mut payload, &db, |_| Ok(false)).unwrap();
            let actual = resolve_api_execution_profile(&db, "default-ollama", override_model.or(Some("selection-model"))).unwrap();
            assert_eq!(payload["agents"][1]["resolvedBackend"]["model"], json!(actual.model));
        }
    }
    #[test] fn mixed_pinned_profile_and_its_credential_are_checked_independently() {
        let db = Database::open_in_memory().unwrap();
        for (id, expected) in [("missing-pin","provider.profile_missing"), ("default-openrouter","provider.credential_missing")] {
            let mut payload = mixed_fixture(id);
            hydrate_provider_context(&mut payload, &db, |_| Ok(false)).unwrap();
            let report = inspect(&payload, true);
            assert!(!report.can_run);
            assert!(report.issues.iter().any(|issue| issue.code == expected && issue.subject_id.as_deref() == Some("api")));
        }
    }
    #[test] fn only_successfully_fetched_current_backend_catalog_is_unavailable_evidence() {
        let db = Database::open_in_memory().unwrap();
        let mut profile = db.get_api_profile("default-ollama").unwrap().unwrap();
        profile.id = "catalog-fixture".to_string();
        db.upsert_api_profile(&profile).unwrap();
        let mut payload = mixed_fixture(&profile.id);
        hydrate_provider_context(&mut payload, &db, |_| Ok(false)).unwrap();
        assert!(inspect(&payload, true).can_run);
        assert!(provider_model_catalog::record(&profile, vec!["current-model".to_string()],
            provider_model_catalog::capture_generation(&profile.id)));
        hydrate_provider_context(&mut payload, &db, |_| Ok(false)).unwrap();
        assert!(inspect(&payload, true).issues.iter().any(|issue| issue.code == "provider.model_unavailable"));
        profile.base_url = "http://localhost:9999/v1".to_string();
        db.upsert_api_profile(&profile).unwrap();
        hydrate_provider_context(&mut payload, &db, |_| Ok(false)).unwrap();
        assert!(inspect(&payload, true).can_run);
        assert_eq!(payload["agents"][1]["resolvedBackend"]["model"], json!("selection-model"));
        provider_model_catalog::invalidate(&profile.id);
    }
    #[test] fn unknown_and_expired_frontend_catalogs_do_not_block() {
        let payload = json!({"agents":[{"id":"api","providerKind":"openrouter"}],
            "tasks":[{"id":"task","agentId":"api"}], "providerConfigs":{"openRouter":{
                "baseUrl":"https://example.test/v1","authMode":"none","model":"saved-model","models":["other-model"]}}});
        assert!(inspect(&payload, true).can_run);
        let mut old = payload.clone();
        old["providerConfigs"]["openRouter"]["modelCatalogCheckedAt"] = json!(chrono::Utc::now().timestamp_millis() - provider_model_catalog::MAX_AGE_MS);
        assert!(inspect(&old, true).can_run);
    }
}
