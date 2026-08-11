use std::{collections::HashSet, time::Duration};

use anyhow::{bail, Context, Result};
use cowork_contracts::{Capability, RunSpec};
use serde_json::{json, Map, Value};

#[derive(Debug, Clone)]
pub struct CrewModelConfig {
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
    pub timeout: Duration,
    pub verify_tls_certificates: bool,
}

pub fn apply_crew_agent_tool_policy<F>(
    agent: &mut Map<String, Value>,
    has_capability: F,
) -> Result<Vec<String>>
where
    F: Fn(&str) -> bool,
{
    agent.insert("allowDelegation".to_owned(), Value::Bool(false));
    let mut allowed = Vec::new();
    for tool in crew_agent_tool_ids(agent)? {
        if matches!(tool.as_str(), "delegate_task" | "mcp") {
            continue;
        }
        if let Some(required) = crew_tool_capability(&tool)? {
            if !has_capability(required) {
                bail!("Crew tool {tool:?} requires executor capability {required:?}");
            }
        }
        allowed.push(tool);
    }
    Ok(allowed)
}

pub fn required_crew_tool_capabilities(definition: &Value) -> Result<Vec<Capability>> {
    let agents = definition
        .get("agents")
        .and_then(Value::as_array)
        .context("the frozen Crew definition must contain agents")?;
    let mut required = Vec::new();
    let mut seen = HashSet::new();
    for agent in agents {
        if agent.get("enabled").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        let agent = agent.as_object().context("Crew agents must be objects")?;
        for tool in crew_agent_tool_ids(agent)? {
            if let Some(capability) = crew_tool_capability(&tool)? {
                if seen.insert(capability) {
                    required.push(Capability::from(capability));
                }
            }
        }
    }
    Ok(required)
}

fn crew_agent_tool_ids(agent: &Map<String, Value>) -> Result<Vec<String>> {
    let Some(tools) = agent.get("tools") else {
        return Ok(Vec::new());
    };
    let tools = tools
        .as_array()
        .context("Crew agent tools must be an array")?;
    if tools.len() > 64 {
        bail!("Crew agents may select at most 64 tools");
    }
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    for value in tools {
        let tool = value
            .as_str()
            .map(canonical_crew_tool_id)
            .filter(|value| !value.is_empty() && value.len() <= 100)
            .context("Crew agent tool names must be strings of at most 100 characters")?;
        if seen.insert(tool.clone()) {
            result.push(tool);
        }
    }
    Ok(result)
}

fn crew_tool_capability(tool: &str) -> Result<Option<&'static str>> {
    match tool {
        "todo" | "delegate_task" | "mcp" => Ok(None),
        "read_file" | "edit_file" | "create_directory" | "move_path" | "copy_path" | "glob"
        | "grep" => Ok(Some("files")),
        "bash" => Ok(Some("shell")),
        "web_fetch" | "web_search" => Ok(Some("web.fetch")),
        "office_workflow" => Ok(Some("office.ooxml")),
        _ => bail!("Crew tool {tool:?} is not supported by the pinned runtime"),
    }
}

fn canonical_crew_tool_id(value: &str) -> String {
    let normalized = value.trim().to_ascii_lowercase().replace('-', "_");
    match normalized.as_str() {
        "shell" | "bashtool" => "bash".to_owned(),
        "read" | "filereadtool" => "read_file".to_owned(),
        "edit" | "write" | "fileedittool" => "edit_file".to_owned(),
        "webfetch" => "web_fetch".to_owned(),
        "websearch" => "web_search".to_owned(),
        "mcp_call" => "mcp".to_owned(),
        "generate_office_workflow" | "pptx_template_workflow" | "docx_template_workflow" => {
            "office_workflow".to_owned()
        }
        _ => normalized,
    }
}

pub fn prepare_crew_request(
    mut definition: Value,
    run: &RunSpec,
    model: &CrewModelConfig,
) -> Result<Value> {
    let request = definition
        .as_object_mut()
        .context("the frozen Crew definition must be an object")?;
    let crew_id = required_string(request, "id")?;
    required_string(request, "name")?;

    let agents = request
        .get_mut("agents")
        .and_then(Value::as_array_mut)
        .context("the frozen Crew definition must contain agents")?;
    agents.retain(|agent| agent.get("enabled").and_then(Value::as_bool) != Some(false));
    let mut active_agent_ids = HashSet::with_capacity(agents.len());
    for agent in agents.iter_mut() {
        let agent = agent
            .as_object_mut()
            .context("Crew agents must be objects")?;
        let id = required_string(agent, "id")?;
        active_agent_ids.insert(id);
        agent.insert(
            "providerKind".to_owned(),
            Value::String("openai-compatible".to_owned()),
        );
        agent.insert(
            "modelOverride".to_owned(),
            Value::String(model.model.clone()),
        );
    }
    if active_agent_ids.is_empty() {
        bail!("the frozen Crew definition has no enabled agents");
    }

    let tasks = request
        .get_mut("tasks")
        .and_then(Value::as_array_mut)
        .context("the frozen Crew definition must contain tasks")?;
    tasks.retain(|task| {
        task.get("agentId")
            .and_then(Value::as_str)
            .is_some_and(|id| active_agent_ids.contains(id))
    });
    let active_task_ids = tasks
        .iter()
        .filter_map(|task| task.get("id").and_then(Value::as_str))
        .map(ToOwned::to_owned)
        .collect::<HashSet<_>>();
    for task in tasks.iter_mut() {
        let task = task.as_object_mut().context("Crew tasks must be objects")?;
        required_string(task, "id")?;
        if let Some(dependencies) = task.get_mut("dependencies").and_then(Value::as_array_mut) {
            dependencies.retain(|dependency| {
                dependency
                    .as_str()
                    .is_some_and(|id| active_task_ids.contains(id))
            });
        }
    }
    if tasks.is_empty() {
        bail!("the frozen Crew definition has no tasks assigned to enabled agents");
    }

    let timeout_ms = u64::try_from(model.timeout.as_millis()).unwrap_or(u64::MAX);
    let provider = json!({
        "baseUrl": model.base_url,
        "apiKey": model.api_key.as_deref().unwrap_or("localai-cowork"),
        "model": model.model,
        "models": [model.model],
        "timeoutMs": timeout_ms,
        "verifyTlsCertificates": model.verify_tls_certificates,
    });
    request.insert(
        "providerConfigs".to_owned(),
        json!({"openAICompatible": provider}),
    );
    request.insert(
        "config".to_owned(),
        json!({
            "baseUrl": model.base_url,
            "model": model.model,
            "timeoutMs": timeout_ms,
            "verifyTlsCertificates": model.verify_tls_certificates,
        }),
    );
    request.insert("runId".to_owned(), Value::String(run.id.to_string()));
    request.insert(
        "streamId".to_owned(),
        Value::String(format!("server-crew-{}", run.id)),
    );
    request.insert("cwd".to_owned(), Value::String("/workspace".to_owned()));

    if let Some(prompt) = run
        .input
        .get("prompt")
        .or_else(|| run.input.get("task_instructions"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let existing = request
            .get("executionGuidelines")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let expected = task_config_value(&run.input, "expected_output")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let mut guidelines = existing.map(ToOwned::to_owned).unwrap_or_default();
        if !guidelines.is_empty() {
            guidelines.push_str("\n\n");
        }
        guidelines.push_str("Work task request:\n");
        guidelines.push_str(prompt);
        if let Some(expected) = expected {
            guidelines.push_str("\n\nExpected overall result:\n");
            guidelines.push_str(expected);
        }
        request.insert("executionGuidelines".to_owned(), Value::String(guidelines));
    }
    if let Some(context) = crate::frozen_runtime_context_text(&run.input) {
        let existing = request
            .get("executionGuidelines")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let guidelines = existing
            .map(|existing| format!("{existing}\n\n{context}"))
            .unwrap_or(context);
        request.insert("executionGuidelines".to_owned(), Value::String(guidelines));
    }

    request.insert("id".to_owned(), Value::String(crew_id));
    Ok(definition)
}

fn required_string(object: &Map<String, Value>, key: &str) -> Result<String> {
    let value = object
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .with_context(|| format!("the frozen Crew definition requires {key}"))?;
    if value.len() > 4_096 {
        bail!("the frozen Crew definition {key} exceeds 4096 characters");
    }
    Ok(value.to_owned())
}

fn task_config_value<'a>(input: &'a Value, key: &str) -> Option<&'a Value> {
    let config = input.get("task_config")?;
    config
        .get(key)
        .filter(|value| !value.is_null())
        .or_else(|| {
            config
                .get("sync_metadata")
                .and_then(Value::as_object)
                .and_then(|metadata| metadata.get(key))
        })
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use cowork_contracts::{ExecutorTarget, FrozenReference, ProjectPrivacy, SCHEMA_VERSION};
    use uuid::Uuid;

    use super::*;

    fn run(input: Value) -> RunSpec {
        let project_id = Uuid::new_v4();
        RunSpec {
            schema_version: SCHEMA_VERSION,
            id: Uuid::new_v4(),
            thread_id: Uuid::new_v4(),
            project_id,
            project: FrozenReference {
                id: project_id,
                revision: 1,
            },
            project_privacy: ProjectPrivacy::TeamManaged,
            task: None,
            creator_user_id: Uuid::new_v4(),
            executor_target: ExecutorTarget::ServerLinux { pool_id: None },
            required_capabilities: vec![],
            input,
            model_profile_id: None,
            snapshot_id: None,
            idempotency_key: Uuid::new_v4().to_string(),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn prepares_a_secret_injected_request_and_filters_disabled_assignments() {
        let spec = run(json!({
            "prompt":"Review the current report",
            "task_config":{"sync_metadata":{"expected_output":"A concise verdict"}},
            "frozen_runtime_context":{"memory":[{"definition":{
                "category":"preference","key":"tone","content":"Be concise."
            }}]}
        }));
        let request = prepare_crew_request(
            json!({
                "id":"reviewers",
                "name":"Review crew",
                "executionGuidelines":"Use evidence.",
                "agents":[
                    {"id":"active","name":"Active","enabled":true,"providerKind":"ollama"},
                    {"id":"disabled","name":"Disabled","enabled":false}
                ],
                "tasks":[
                    {"id":"review","agentId":"active","dependencies":["disabled-task"]},
                    {"id":"disabled-task","agentId":"disabled","dependencies":[]}
                ]
            }),
            &spec,
            &CrewModelConfig {
                base_url: "https://models.example.test/v1".to_owned(),
                api_key: Some("server-secret".to_owned()),
                model: "review-model".to_owned(),
                timeout: Duration::from_secs(30),
                verify_tls_certificates: true,
            },
        )
        .unwrap();
        assert_eq!(request["agents"].as_array().unwrap().len(), 1);
        assert_eq!(request["tasks"].as_array().unwrap().len(), 1);
        assert_eq!(request["agents"][0]["providerKind"], "openai-compatible");
        assert_eq!(request["agents"][0]["modelOverride"], "review-model");
        assert_eq!(request["tasks"][0]["dependencies"], json!([]));
        assert_eq!(
            request["providerConfigs"]["openAICompatible"]["apiKey"],
            "server-secret"
        );
        assert!(request["executionGuidelines"]
            .as_str()
            .unwrap()
            .contains("Expected overall result:\nA concise verdict"));
        assert!(request["executionGuidelines"]
            .as_str()
            .unwrap()
            .contains("[preference/tone] Be concise."));
    }

    #[test]
    fn crew_tool_policy_is_capability_bound_and_disables_delegation() {
        let mut agent = json!({
            "allowDelegation":true,
            "tools":["read", "websearch", "office_workflow", "todo", "read", "mcp"]
        })
        .as_object()
        .unwrap()
        .clone();
        let capabilities = ["files", "web.fetch", "office.ooxml"];
        let allowed =
            apply_crew_agent_tool_policy(&mut agent, |required| capabilities.contains(&required))
                .unwrap();
        assert_eq!(
            allowed,
            vec!["read_file", "web_search", "office_workflow", "todo"]
        );
        assert_eq!(agent["allowDelegation"], false);

        let mut shell_agent = json!({"tools":["bash"]}).as_object().unwrap().clone();
        assert!(apply_crew_agent_tool_policy(&mut shell_agent, |_| false).is_err());
    }

    #[test]
    fn crew_tool_capabilities_are_derived_from_enabled_agents() {
        let capabilities = required_crew_tool_capabilities(&json!({
            "agents":[
                {"id":"writer","tools":["read_file","office_workflow","todo"]},
                {"id":"researcher","tools":["web_search","bash","read"]},
                {"id":"disabled","enabled":false,"tools":["unsupported_tool"]}
            ]
        }))
        .unwrap();
        assert_eq!(
            capabilities,
            vec![
                Capability::from("files"),
                Capability::from("office.ooxml"),
                Capability::from("web.fetch"),
                Capability::from("shell"),
            ]
        );
    }
}
