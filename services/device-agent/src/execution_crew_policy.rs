//! Original V1 restrictions are intersected with this run, never used as grants.
use super::*;
use cowork_contracts::DeviceCrewDefinition;
fn names(value: &Value) -> Result<Vec<String>> {
    if value.is_null() {
        return Ok(vec![]);
    }
    let list = value
        .as_array()
        .context("device_crew_original_policy_array_invalid")?;
    ensure!(list.len() <= 256, "device_crew_policy_bound");
    list.iter()
        .map(|v| {
            let s = v.as_str().context("device_crew_policy_string_invalid")?;
            ensure!(
                s.len() <= 1000 && !s.contains(['\0', '\r', '\n']),
                "device_crew_policy_string_invalid"
            );
            Ok(s.to_owned())
        })
        .collect()
}
pub(super) fn canonical(name: &str) -> String {
    let normalized = name.trim().to_ascii_lowercase().replace('-', "_");
    match normalized.as_str() {
        "read" | "filereadtool" => "read_file",
        "edit" | "write" | "fileedittool" => "edit_file",
        "shell" | "bashtool" => "bash",
        "webfetch" => "web_fetch",
        "websearch" => "web_search",
        "mcp" => "mcp_call",
        "generate_office_workflow" | "pptx_template_workflow" | "docx_template_workflow" => {
            "office_workflow"
        }
        "microsoftoffice" | "office_microsoft" => "microsoft_office",
        n => n,
    }
    .into()
}
pub(super) fn intersect(original: &Value, definition: &mut DeviceCrewDefinition) -> Result<()> {
    // Credential-bearing original executor bindings cannot cross device ownership.
    ensure!(
        original
            .get("executorMcpBindings")
            .is_none_or(|v| v.is_null() || v.as_array().is_some_and(Vec::is_empty)),
        "device_crew_foreign_executor_bindings_not_adopted"
    );
    if let Some(access) = original["governance"]["agentAccess"].as_array() {
        ensure!(
            access.len() <= 16,
            "device_crew_original_agent_access_bound"
        );
        let mut seen = BTreeSet::new();
        for entry in access {
            let id = entry["agentId"]
                .as_str()
                .context("device_crew_original_agent_access_missing")?;
            ensure!(
                seen.insert(id),
                "device_crew_duplicate_original_agent_access"
            );
            let a = definition
                .agents
                .iter_mut()
                .find(|a| a.id == id)
                .context("device_crew_foreign_original_agent_access")?;
            let allowed = names(&entry["allowedTools"])?;
            let blocked = names(&entry["blockedTools"])?;
            a.tools.retain(|n| {
                allowed.iter().any(|v| canonical(v) == n.as_str())
                    && !blocked.iter().any(|v| canonical(v) == n.as_str())
            });
            let allowed = names(&entry["allowedMcpServerNames"])?;
            let blocked = names(&entry["blockedMcpServerNames"])?;
            a.mcp_server_names
                .retain(|n| allowed.contains(n) && !blocked.contains(n));
            a.allow_delegation &= entry["delegationAllowed"] == true;
        }
        ensure!(
            definition
                .agents
                .iter()
                .all(|a| seen.contains(a.id.as_str())),
            "device_crew_original_agent_access_incomplete"
        );
    }
    roots(original)?;
    names(&original["governance"]["denyRules"])?;
    Ok(())
}
fn roots(original: &Value) -> Result<Option<Vec<(String, String)>>> {
    let Some(entries) = original.get("authorizedPaths") else {
        ensure!(
            original
                .get("cwd")
                .is_none_or(|v| v.is_null() || v == "" || v == "."),
            "device_crew_foreign_cwd_not_adopted"
        );
        return Ok(None);
    };
    let entries = entries
        .as_array()
        .context("device_crew_original_roots_invalid")?;
    ensure!(entries.len() <= 64, "device_crew_original_roots_bound");
    let mut roots = vec![];
    for entry in entries {
        let path = entry["path"]
            .as_str()
            .context("device_crew_original_root_missing")?;
        let kind = entry["kind"]
            .as_str()
            .context("device_crew_original_root_kind")?;
        ensure!(
            matches!(kind, "file" | "folder") && entry["access"] == "read_write",
            "device_crew_original_root_not_current_read_write"
        );
        if path != "." {
            workspace::relative(path)?;
        }
        roots.push((path.into(), kind.into()));
    }
    Ok(Some(roots))
}
fn matches(pattern: &str, text: &str) -> bool {
    let p = pattern.to_ascii_lowercase().into_bytes();
    let t = text.to_ascii_lowercase().into_bytes();
    let mut previous = vec![false; t.len() + 1];
    previous[0] = true;
    for c in p {
        let mut next = vec![false; t.len() + 1];
        next[0] = c == b'*' && previous[0];
        for i in 1..=t.len() {
            next[i] = if c == b'*' {
                previous[i] || next[i - 1]
            } else {
                previous[i - 1] && (c == b'?' || c == t[i - 1])
            };
        }
        previous = next;
    }
    previous[t.len()]
}
pub(super) fn tool(original: &Value, name: &str, args: &mut Value) -> Result<()> {
    let roots = roots(original)?;
    let rules = names(&original["governance"]["denyRules"])?;
    let bounded = roots
        .as_ref()
        .is_none_or(|r| r.iter().any(|(p, k)| p == "." && k == "folder"));
    // Shell, Git and external adapters cannot prove per-path confinement with
    // native OS rights. Never discard a restrictive original path policy.
    if !v1_tools::NAMES.contains(&name)
        && !matches!(name, "office" | "office_workflow" | "microsoft_office")
        || matches!(name, "bash" | "git")
    {
        ensure!(
            bounded && rules.is_empty(),
            "device_crew_native_tool_cannot_preserve_original_path_restrictions"
        );
    }
    for field in [
        "path",
        "source",
        "destination",
        "cwd",
        "input",
        "output",
        "output_path",
        "template_path",
        "preview_output",
    ] {
        if let Some(path) = args[field].as_str().filter(|v| !v.is_empty()) {
            workspace::relative(path)?;
            ensure!(
                roots.as_ref().is_none_or(|r| r.iter().any(|(p, k)| p == "."
                    || path == p
                    || k == "folder" && path.starts_with(&format!("{p}/")))),
                "device_crew_original_path_grant_missing"
            );
            for rule in &rules {
                let (tool, target) = rule.split_once(':').unwrap_or((rule, "*"));
                ensure!(
                    !(matches(tool, &canonical(name)) && matches(target, path)),
                    "device_crew_original_path_denied"
                );
            }
        }
    }
    if v1_tools::NAMES.contains(&name) {
        args.as_object_mut()
            .context("device_crew_tool_arguments_invalid")?
            .insert(
                "_crew_policy".into(),
                json!({"roots":roots,"deny_rules":rules}),
            );
    }
    Ok(())
}
