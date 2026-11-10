use cowork_contracts::*;
use cowork_domain::approval_fingerprint;

#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    Allow,
    ApprovalRequired,
    Deny,
}

/// Every layer must grant the exact tool. No agent or external metadata can enlarge a layer.
pub fn evaluate(
    run: &RunRecord,
    tool: &ToolCall,
    layers: &[&[String]],
    explicit_denies: &[String],
    approvals: &[Approval],
) -> Decision {
    if run
        .spec
        .agent_config
        .as_ref()
        .and_then(|c| c.execution_mode)
        == Some(AgentExecutionMode::Plan)
    {
        return Decision::Deny;
    }
    if let Some(behavior) = run
        .spec
        .agent_config
        .as_ref()
        .and_then(|c| c.behavior.as_ref())
    {
        if run.steps >= behavior.max_tool_calls_per_loop {
            return Decision::Deny;
        }
        if tool.name == "execute_program" {
            let command = std::iter::once(tool.arguments["program"].as_str().unwrap_or(""))
                .chain(
                    tool.arguments["arguments"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|v| v.as_str()),
                )
                .collect::<Vec<_>>()
                .join(" ");
            if behavior
                .command_blacklist
                .iter()
                .any(|pattern| command_matches(pattern, &command))
                || behavior.command_allow_groups.iter().any(|group| {
                    !group
                        .iter()
                        .any(|pattern| command_matches(pattern, &command))
                })
            {
                return Decision::Deny;
            }
        }
    }
    // The built-in catalog, never an MCP-provided "read only" annotation, owns effects.
    let mutating = match tool.name.as_str() {
        "create_artifact" | "write_text_file" | "create_directory" | "rename_file"
        | "remove_file" | "restore_file" | "import_file" | "execute_program" | "browser"
        | "office" | "github" | "document_write" | "image_edit" => true,
        "delegate_agents" | "list_artifacts" | "read_artifact" | "request_input" | "list_files"
        | "read_text_file" | "stat_file" | "document_read" => false,
        // Every explicitly frozen MCP tool is potentially mutating. Remote hints
        // cannot lower this classification, including a readOnlyHint=true claim.
        _ if cowork_domain::mcp::mcp_binding(run, tool).is_ok() => true,
        _ => return Decision::Deny,
    };
    if explicit_denies.contains(&tool.name)
        || !run.spec.allowed_tools.contains(&tool.name)
        || layers.iter().any(|layer| !layer.contains(&tool.name))
    {
        return Decision::Deny;
    }
    if run.spec.access_mode == AccessMode::ReadOnly && mutating {
        return Decision::Deny;
    }
    if let Some(approval) = approvals.iter().find(|a| {
        a.run_id == run.id
            && a.fingerprint == approval_fingerprint(run, tool)
            && a.policy_revision == run.spec.policy_revision
            && a.target == run.spec.executor
            && a.tool == *tool
            && a.decision.is_some()
    }) {
        return if approval.decision == Some(true) {
            Decision::Allow
        } else {
            Decision::Deny
        };
    }
    // Owner-native tools always need an exact human approval. Autonomous mode
    // cannot reuse a manual preview SHA or invent native model permissions.
    if matches!(tool.name.as_str(), "browser" | "office" | "github")
        || (tool.name == "execute_program" && run.spec.environment.is_none())
    {
        return Decision::ApprovalRequired;
    }
    match run.spec.access_mode {
        AccessMode::ConfirmAll => Decision::ApprovalRequired,
        AccessMode::ConfirmMutations if mutating => Decision::ApprovalRequired,
        _ => Decision::Allow,
    }
}

fn command_matches(pattern: &str, command: &str) -> bool {
    // Literal, case-sensitive command/argument matching. '*' spans characters;
    // no regex or shell expansion is evaluated by the policy.
    let parts: Vec<_> = pattern.split('*').collect();
    let mut cursor = 0;
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        let remaining = &command[cursor..];
        if index == 0 && !pattern.starts_with('*') {
            if !remaining.starts_with(part) {
                return false;
            }
            cursor += part.len();
        } else if index == parts.len() - 1 && !pattern.ends_with('*') {
            return remaining.ends_with(part);
        } else if let Some(offset) = remaining.find(part) {
            cursor += offset + part.len();
        } else {
            return false;
        }
    }
    pattern.ends_with('*') || cursor == command.len()
}

pub fn migrate_access_mode(old: &str) -> AccessMode {
    match old {
        "default" | "confirm_mutations" => AccessMode::ConfirmMutations,
        "bypass" | "autonomous" => AccessMode::Autonomous,
        "confirm_all" => AccessMode::ConfirmAll,
        _ => AccessMode::ReadOnly,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn run() -> RunRecord {
        serde_json::from_value(json!({
          "id":"r","spec":{"order_id":"o","project_id":"p","prompt":"x",
          "provider":{"id":"p","name":"p","kind":"test","endpoint":"","model":"test","revision":1,
          "context_tokens":8192,"credential_required":false,"credential_ready":true},
          "access_mode":"confirm_mutations","executor":"local","policy_revision":1,
          "allowed_tools":["create_artifact"],"budgets":{"max_steps":64,"max_seconds":1800,"max_output_tokens":4096}},
          "state":"running","revision":1,"last_sequence":1,"steps":0,"created_at":0,"updated_at":0,"started_at":null,"error_code":null,
          "usage":{"input_tokens":null,"output_tokens":null,"cost":null,"price_source_version":null}
        })).unwrap()
    }
    #[test]
    fn standard_never_silently_becomes_autonomous() {
        assert_eq!(migrate_access_mode("default"), AccessMode::ConfirmMutations);
        assert_eq!(migrate_access_mode("unknown"), AccessMode::ReadOnly);
        let r = run();
        let t = ToolCall {
            id: "t".into(),
            name: "create_artifact".into(),
            arguments: json!({"content":"x"}),
        };
        assert_eq!(evaluate(&r, &t, &[], &[], &[]), Decision::ApprovalRequired);
    }
    #[test]
    fn command_rules_intersect_and_remain_denied_after_approval() {
        let mut r = run();
        r.spec.allowed_tools.push("execute_program".into());
        r.spec.agent_config = Some(EffectiveAgentConfig {
            behavior: Some(AgentBehaviorSettings {
                command_allow_groups: vec![vec!["git *".into()], vec!["git status*".into()]],
                command_blacklist: vec!["*--force*".into()],
                max_tool_calls_per_loop: 3,
                fallback_to_human_on_repeated_failure: true,
            }),
            ..Default::default()
        });
        let mut call = ToolCall {
            id: "command".into(),
            name: "execute_program".into(),
            arguments: json!({"program":"git","arguments":["status"]}),
        };
        assert_eq!(
            evaluate(&r, &call, &[], &[], &[]),
            Decision::ApprovalRequired
        );
        call.arguments["arguments"] = json!(["push"]);
        assert_eq!(evaluate(&r, &call, &[], &[], &[]), Decision::Deny);
        call.arguments["arguments"] = json!(["status", "--force"]);
        assert_eq!(evaluate(&r, &call, &[], &[], &[]), Decision::Deny);
        call.arguments["arguments"] = json!(["status"]);
        r.steps = 3;
        assert_eq!(evaluate(&r, &call, &[], &[], &[]), Decision::Deny);
    }
    #[test]
    fn approval_binds_arguments_and_cannot_override_denies_or_read_only() {
        let mut r = run();
        let mut t = ToolCall {
            id: "t".into(),
            name: "create_artifact".into(),
            arguments: json!({"content":"x"}),
        };
        let a = Approval {
            id: "a".into(),
            run_id: r.id.clone(),
            tool: t.clone(),
            target: r.spec.executor.clone(),
            policy_revision: 1,
            fingerprint: approval_fingerprint(&r, &t),
            decision: Some(true),
        };
        assert_eq!(
            evaluate(&r, &t, &[], &[], std::slice::from_ref(&a)),
            Decision::Allow
        );
        assert_eq!(
            evaluate(&r, &t, &[&[]], &[], std::slice::from_ref(&a)),
            Decision::Deny
        );
        assert_eq!(
            evaluate(&r, &t, &[], &[t.name.clone()], std::slice::from_ref(&a)),
            Decision::Deny
        );
        t.arguments = json!({"content":"changed"});
        assert_eq!(
            evaluate(&r, &t, &[], &[], std::slice::from_ref(&a)),
            Decision::ApprovalRequired
        );
        r.spec.access_mode = AccessMode::ReadOnly;
        assert_eq!(evaluate(&r, &t, &[], &[], &[a]), Decision::Deny);
    }
    #[test]
    fn document_tools_follow_access_mode_without_forced_native_approval() {
        let mut r = run();
        r.spec
            .allowed_tools
            .extend(["document_read", "document_write", "image_edit", "office"].map(String::from));
        let call = |name: &str| ToolCall {
            id: name.into(),
            name: name.into(),
            arguments: json!({}),
        };
        let expected = [
            (
                AccessMode::ReadOnly,
                [Decision::Allow, Decision::Deny, Decision::Deny],
            ),
            (
                AccessMode::ConfirmAll,
                [
                    Decision::ApprovalRequired,
                    Decision::ApprovalRequired,
                    Decision::ApprovalRequired,
                ],
            ),
            (
                AccessMode::ConfirmMutations,
                [
                    Decision::Allow,
                    Decision::ApprovalRequired,
                    Decision::ApprovalRequired,
                ],
            ),
            (
                AccessMode::Autonomous,
                [Decision::Allow, Decision::Allow, Decision::Allow],
            ),
        ];
        for (mode, decisions) in expected {
            r.spec.access_mode = mode.clone();
            for (name, decision) in ["document_read", "document_write", "image_edit"]
                .into_iter()
                .zip(decisions)
            {
                assert_eq!(
                    evaluate(&r, &call(name), &[], &[], &[]),
                    decision,
                    "{name} in {mode:?}"
                );
            }
        }
        // The owner-native office tool keeps its forced approval even in autonomous mode.
        assert_eq!(
            evaluate(&r, &call("office"), &[], &[], &[]),
            Decision::ApprovalRequired
        );
        r.spec.allowed_tools.retain(|t| t != "document_write");
        assert_eq!(
            evaluate(&r, &call("document_write"), &[], &[], &[]),
            Decision::Deny
        );
    }
}
