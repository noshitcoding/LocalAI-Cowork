//! Resolution of frozen agent values. A filter only narrows existing rights.
use crate::{Error, Result};
use cowork_contracts::{
    AccessMode, AgentBehaviorSettings, AgentExecutionMode, ApplicationPreferences,
    EffectiveAgentConfig, GenerationSettings,
};
use std::collections::BTreeSet;

pub fn narrow_access_mode(base: &AccessMode, role: &AccessMode) -> AccessMode {
    fn rank(value: &AccessMode) -> u8 {
        match value {
            AccessMode::ReadOnly => 0,
            AccessMode::ConfirmAll => 1,
            AccessMode::ConfirmMutations => 2,
            AccessMode::Autonomous => 3,
        }
    }
    if rank(base) <= rank(role) {
        base.clone()
    } else {
        role.clone()
    }
}

pub fn validate_agent_values(config: &EffectiveAgentConfig) -> Result<()> {
    if let Some(behavior) = &config.behavior
        && (!(1..=4096).contains(&behavior.max_tool_calls_per_loop)
            || behavior.command_allow_groups.len() > 16
            || behavior.command_blacklist.len() > 256
            || behavior
                .command_allow_groups
                .iter()
                .any(|group| group.is_empty() || group.len() > 256)
            || behavior
                .command_allow_groups
                .iter()
                .flatten()
                .chain(&behavior.command_blacklist)
                .any(|pattern| {
                    pattern.is_empty() || pattern.len() > 4096 || pattern.contains('\0')
                }))
    {
        return Err(Error::Validation("Invalid agent behavior".into()));
    }
    let GenerationSettings {
        temperature,
        timeout_ms,
    } = config.generation.clone();
    if temperature.is_some_and(|v| !v.is_finite() || !(0.0..=2.0).contains(&v))
        || timeout_ms.is_some_and(|v| !(1000..=1_200_000).contains(&v))
        || config.instructions.len() > 256_000
        || config.instructions.contains('\0')
    {
        return Err(Error::Validation(
            "Invalid agent generation or instructions".into(),
        ));
    }
    for filter in [&config.tool_filter, &config.skill_ids, &config.memory_ids]
        .into_iter()
        .flatten()
    {
        let unique: BTreeSet<_> = filter.iter().collect();
        if filter.len() > 128
            || unique.len() != filter.len()
            || filter
                .iter()
                .any(|v| v.is_empty() || v.len() > 128 || v.chars().any(char::is_control))
        {
            return Err(Error::Validation("Invalid agent tool selection".into()));
        }
    }
    Ok(())
}

/// A workflow role inherits the run context and can narrow its tool selection.
pub fn merge_agent_values(
    base: Option<&EffectiveAgentConfig>,
    role: Option<&EffectiveAgentConfig>,
) -> Option<EffectiveAgentConfig> {
    match (base, role) {
        (None, None) => None,
        (Some(value), None) | (None, Some(value)) => Some(value.clone()),
        (Some(base), Some(role)) => {
            let mut resolved = role.clone();
            resolved.execution_mode = Some(narrow_execution_mode(
                base.execution_mode.unwrap_or_default(),
                role.execution_mode.unwrap_or_default(),
            ));
            resolved.behavior = match (&base.behavior, &role.behavior) {
                (Some(base), Some(role)) => {
                    let mut value = role.clone();
                    value
                        .command_allow_groups
                        .extend(base.command_allow_groups.clone());
                    value
                        .command_blacklist
                        .extend(base.command_blacklist.clone());
                    value.max_tool_calls_per_loop = value
                        .max_tool_calls_per_loop
                        .min(base.max_tool_calls_per_loop);
                    value.fallback_to_human_on_repeated_failure |=
                        base.fallback_to_human_on_repeated_failure;
                    Some(value)
                }
                (Some(value), None) | (None, Some(value)) => Some(value.clone()),
                (None, None) => None,
            };
            resolved.agent_source = role
                .agent_source
                .clone()
                .or_else(|| base.agent_source.clone());
            resolved.personality_source = role
                .personality_source
                .clone()
                .or_else(|| base.personality_source.clone());
            resolved.instructions = [base.instructions.as_str(), role.instructions.as_str()]
                .into_iter()
                .filter(|s| !s.trim().is_empty())
                .collect::<Vec<_>>()
                .join("\n\n");
            if base.instructions == role.instructions {
                resolved.instructions = role.instructions.clone();
            }
            resolved.generation.temperature =
                role.generation.temperature.or(base.generation.temperature);
            resolved.generation.timeout_ms =
                role.generation.timeout_ms.or(base.generation.timeout_ms);
            resolved.access_mode = match (&base.access_mode, &role.access_mode) {
                (Some(base), Some(role)) => Some(narrow_access_mode(base, role)),
                (Some(value), None) | (None, Some(value)) => Some(value.clone()),
                (None, None) => None,
            };
            fn narrowed(
                base: &Option<Vec<String>>,
                role: &Option<Vec<String>>,
            ) -> Option<Vec<String>> {
                match (base, role) {
                    (Some(base), Some(role)) => Some(
                        role.iter()
                            .filter(|id| base.contains(id))
                            .cloned()
                            .collect(),
                    ),
                    (Some(value), None) | (None, Some(value)) => Some(value.clone()),
                    (None, None) => None,
                }
            }
            resolved.skill_ids = narrowed(&base.skill_ids, &role.skill_ids);
            resolved.memory_ids = narrowed(&base.memory_ids, &role.memory_ids);
            resolved.tool_filter = match (&base.tool_filter, &role.tool_filter) {
                (Some(base), Some(role)) => {
                    Some(role.iter().filter(|v| base.contains(v)).cloned().collect())
                }
                (Some(filter), None) | (None, Some(filter)) => Some(filter.clone()),
                (None, None) => None,
            };
            Some(resolved)
        }
    }
}

pub fn narrow_execution_mode(a: AgentExecutionMode, b: AgentExecutionMode) -> AgentExecutionMode {
    use AgentExecutionMode::*;
    if a == Plan || b == Plan {
        Plan
    } else if a == Hybrid || b == Hybrid {
        Hybrid
    } else {
        Execute
    }
}
pub fn freeze_behavior_defaults(config: &mut EffectiveAgentConfig, prefs: &ApplicationPreferences) {
    let lines = |text: &str| {
        text.lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let whitelist = lines(&prefs.command_whitelist);
    let defaults = AgentBehaviorSettings {
        command_allow_groups: if whitelist.is_empty() {
            vec![]
        } else {
            vec![whitelist]
        },
        command_blacklist: lines(&prefs.command_blacklist),
        max_tool_calls_per_loop: prefs.max_tool_calls_per_loop,
        fallback_to_human_on_repeated_failure: prefs.fallback_to_human_on_repeated_failure,
    };
    let base = EffectiveAgentConfig {
        behavior: Some(defaults),
        ..Default::default()
    };
    config.behavior = merge_agent_values(Some(&base), Some(config)).and_then(|c| c.behavior);
    if prefs.read_only_fs_mode {
        config.access_mode = Some(AccessMode::ReadOnly);
    }
}

pub const HYBRID_EXECUTION_MARKER: &str = "V2_HYBRID_EXECUTION_PHASE";
pub fn consecutive_tool_failures(messages: &[serde_json::Value]) -> usize {
    let mut count = 0;
    for message in messages.iter().rev().filter(|m| m["role"] == "tool") {
        let Some(content) = message["content"]
            .as_str()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        else {
            break;
        };
        if content
            .get("error")
            .is_some_and(|v| !v.is_null() && v != false && v != "")
            || content["succeeded"] == false
            || content["success"] == false
            || content["_workflow_success"] == false
            || content
                .pointer("/outcome/exit_code")
                .and_then(|v| v.as_i64())
                .is_some_and(|code| code != 0)
            || content
                .get("exit_code")
                .and_then(|v| v.as_i64())
                .is_some_and(|code| code != 0)
        {
            count += 1;
        } else {
            break;
        }
    }
    count
}
pub fn hybrid_planning(spec: &cowork_contracts::RunSpec, messages: &[serde_json::Value]) -> bool {
    spec.agent_config.as_ref().and_then(|c| c.execution_mode) == Some(AgentExecutionMode::Hybrid)
        && !messages
            .iter()
            .any(|m| m["role"] == "system" && m["cowork_phase"] == HYBRID_EXECUTION_MARKER)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn behavior_merge_keeps_both_allow_boundaries_and_plan_wins() {
        let prefs = ApplicationPreferences {
            command_whitelist: "python *\ngit status*".into(),
            command_blacklist: "* --force*".into(),
            max_tool_calls_per_loop: 5,
            ..Default::default()
        };
        let mut base = EffectiveAgentConfig {
            execution_mode: Some(AgentExecutionMode::Hybrid),
            ..Default::default()
        };
        freeze_behavior_defaults(&mut base, &prefs);
        let mut role = EffectiveAgentConfig {
            execution_mode: Some(AgentExecutionMode::Plan),
            ..Default::default()
        };
        freeze_behavior_defaults(
            &mut role,
            &ApplicationPreferences {
                command_whitelist: "git *".into(),
                max_tool_calls_per_loop: 3,
                ..Default::default()
            },
        );
        let merged = merge_agent_values(Some(&base), Some(&role)).unwrap();
        assert_eq!(merged.execution_mode, Some(AgentExecutionMode::Plan));
        assert_eq!(
            merged.behavior.as_ref().unwrap().command_allow_groups.len(),
            2
        );
        assert_eq!(merged.behavior.as_ref().unwrap().max_tool_calls_per_loop, 3);
        validate_agent_values(&merged).unwrap();
        let messages =
            vec![serde_json::json!({"role":"tool","content":"{\"error\":\"denied\"}"}); 3];
        assert_eq!(consecutive_tool_failures(&messages), 3);
    }
    #[test]
    fn role_overrides_values_without_widening_tools_or_dropping_context() {
        let base = EffectiveAgentConfig {
            instructions: "global and project".into(),
            generation: GenerationSettings {
                temperature: Some(0.7),
                timeout_ms: Some(5000),
            },
            tool_filter: Some(vec!["read_artifact".into()]),
            ..Default::default()
        };
        let role = EffectiveAgentConfig {
            instructions: "role".into(),
            generation: GenerationSettings {
                temperature: Some(0.0),
                timeout_ms: None,
            },
            tool_filter: Some(vec!["read_artifact".into(), "execute_program".into()]),
            ..Default::default()
        };
        let merged = merge_agent_values(Some(&base), Some(&role)).unwrap();
        assert_eq!(merged.instructions, "global and project\n\nrole");
        assert_eq!(merged.generation.temperature, Some(0.0));
        assert_eq!(merged.generation.timeout_ms, Some(5000));
        assert_eq!(merged.tool_filter, Some(vec!["read_artifact".into()]));
        validate_agent_values(&merged).unwrap();
    }
}
