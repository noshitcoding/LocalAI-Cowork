//! Shared, frozen actor-run overrides. These can narrow capabilities, never grant tools.
use cowork_contracts::*;
use cowork_domain::{Error, Result, validate_budgets};
use serde::Deserialize;
use serde_json::Value;

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Overrides {
    agent_config: Option<EffectiveAgentConfig>,
    access_mode: Option<AccessMode>,
    budgets: Option<Budgets>,
}
pub fn apply_agent_run_overrides(
    spec: &mut RunSpec,
    input: &Value,
    provider_defaults: &Value,
) -> Result<()> {
    let raw = input.get("run_configuration").or_else(|| {
        input
            .get("task_config")
            .and_then(|v| v.get("run_configuration"))
    });
    let overrides: Overrides = raw
        .map(|v| {
            serde_json::from_value(v.clone())
                .map_err(|_| Error::Validation("Invalid frozen agent settings".into()))
        })
        .transpose()?
        .unwrap_or_default();
    if let Some(budgets) = overrides.budgets {
        validate_budgets(&budgets)?;
        spec.budgets = budgets;
    }
    if let Some(access) = overrides.access_mode {
        spec.access_mode = access;
    }
    if let Some(config) = overrides.agent_config {
        spec.agent_config = Some(config);
    }
    let generation = GenerationSettings {
        temperature: provider_defaults.get("temperature").and_then(Value::as_f64),
        timeout_ms: provider_defaults.get("timeout_ms").and_then(Value::as_u64),
    };
    cowork_storage::library_config::validate_generation_settings(&generation)?;
    if generation.temperature.is_some() || generation.timeout_ms.is_some() {
        let config = spec.agent_config.get_or_insert_with(Default::default);
        if config.generation.temperature.is_none() {
            config.generation.temperature = generation.temperature;
        }
        if config.generation.timeout_ms.is_none() {
            config.generation.timeout_ms = generation.timeout_ms;
        }
    }
    if let Some(config) = &spec.agent_config {
        cowork_storage::library_config::validate_effective_agent_config(config)?;
    }
    if let Some(behavior) = spec.agent_config.as_ref().and_then(|c| c.behavior.as_ref()) {
        spec.budgets.max_steps = spec.budgets.max_steps.min(behavior.max_tool_calls_per_loop);
    }
    let provider = spec.provider.as_ref().ok_or(Error::Forbidden)?;
    if spec.budgets.max_output_tokens >= provider.context_tokens
        || spec.budgets.max_total_tokens < u64::from(provider.context_tokens)
    {
        return Err(Error::BudgetConfiguration);
    }
    if let Some(filter) = spec
        .agent_config
        .as_ref()
        .and_then(|c| c.tool_filter.as_ref())
    {
        spec.allowed_tools.retain(|tool| filter.contains(tool));
        spec.mcp_tools
            .retain(|binding| filter.contains(&binding.alias));
    }
    if matches!(spec.access_mode, AccessMode::ReadOnly) {
        spec.allowed_tools
            .retain(|tool| cowork_domain::read_only_tool(tool));
        for folder in &mut spec.folders {
            folder.writable = false;
        }
        spec.mcp_tools.clear();
    }
    Ok(())
}
