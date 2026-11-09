//! Local adapter admission. Legacy names never authorize an unavailable runtime.
use crate::execution::ExecutionConfig;
use anyhow::{Result, ensure};
use serde_json::Value;
pub const EXTRA_TOOLS: &[&str] = &[
    "read_file",
    "edit_file",
    "create_directory",
    "copy_path",
    "move_path",
    "delete_file",
    "glob",
    "grep",
    "bash",
    "git",
    "web_fetch",
    "web_search",
    "office_workflow",
    "microsoft_office",
    "browser_visible",
];
pub fn tools(
    programs: &std::collections::BTreeMap<String, std::path::PathBuf>,
    native: bool,
) -> Vec<String> {
    if !native {
        return vec![];
    }
    let mut names = vec![];
    if programs.contains_key("python") {
        names.extend(
            [
                "read_file",
                "edit_file",
                "create_directory",
                "copy_path",
                "move_path",
                "delete_file",
                "glob",
                "grep",
                "web_fetch",
                "web_search",
                "office_workflow",
            ]
            .into_iter()
            .map(str::to_owned),
        );
    }
    if programs.contains_key("python") && programs.contains_key("shell") {
        names.push("bash".into());
    }
    if programs.contains_key("python") && programs.contains_key("git") {
        names.push("git".into());
    }
    if programs.contains_key("python") && programs.contains_key("chromium") {
        names.push("browser_visible".into());
    }
    if cfg!(windows)
        && programs.contains_key("powershell")
        && programs.contains_key("python")
        && ["word", "excel", "powerpoint"]
            .iter()
            .all(|n| programs.contains_key(*n))
    {
        names.push("microsoft_office".into());
    }
    names
}
pub fn available(name: &str, config: &ExecutionConfig) -> bool {
    let has = |n: &str| config.allowed_tools.iter().any(|v| v == n);
    match name {
        "files" => [
            "read_file",
            "edit_file",
            "create_directory",
            "copy_path",
            "move_path",
            "glob",
            "grep",
        ]
        .iter()
        .all(|n| has(n)),
        "shell" => has("bash"),
        "git" => has("git"),
        "web.fetch" => has("web_fetch") && has("web_search"),
        "office.ooxml" => has("office_workflow"),
        "office.libreoffice" => has("office") && config.programs.contains_key("libreoffice"),
        "office.microsoft" => has("microsoft_office"),
        "browser.headless" => has("browser"),
        "browser.visible" => has("browser_visible"),
        "model.ollama" => config.model.as_ref().is_some_and(|m| m.api == "ollama"),
        "model.vllm" => config
            .model
            .as_ref()
            .is_some_and(|m| m.api == "openai" && m.implementation == "vllm"),
        "crew.python" => {
            config.model.is_some()
                && config.native_owner_rights
                && config.programs.contains_key("python")
                && config.interop.crewai_version.is_some()
                && config
                    .interop
                    .crewai_source_sha256
                    .as_ref()
                    .is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        }
        _ => false,
    }
}
pub fn validate(config: &ExecutionConfig) -> Result<()> {
    if let Some(model) = &config.model {
        ensure!(
            matches!(model.api.as_str(), "openai" | "ollama")
                && matches!(
                    model.implementation.as_str(),
                    "openai_compatible" | "ollama" | "vllm"
                ),
            "device_model_adapter_invalid"
        );
        ensure!(
            (model.api == "ollama") == (model.implementation == "ollama"),
            "device_model_descriptor_differs"
        );
    }
    ensure!(
        config.interop.allowed_hosts.len() <= 64
            && config.interop.allowed_hosts.iter().all(|h| !h.is_empty()
                && h.len() <= 253
                && h.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')),
        "device_interop_host_policy_invalid"
    );
    ensure!(
        config.interop.crew_profile_bindings.len() <= 64
            && config
                .interop
                .crew_profile_bindings
                .iter()
                .all(|(id, model)| !id.is_empty()
                    && !model.is_empty()
                    && id.len() <= 200
                    && model.len() <= 200
                    && !id.chars().any(char::is_control)
                    && !model.chars().any(char::is_control)),
        "device_crew_local_profile_map_invalid"
    );
    if let Some(search) = &config.interop.web_search_url {
        let url = reqwest::Url::parse(search)?;
        ensure!(
            search.len() <= 4096
                && search.matches("{query}").count() == 1
                && url.scheme() == "https"
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.fragment().is_none(),
            "device_search_endpoint_invalid"
        );
    }
    Ok(())
}
pub fn descriptors(config: &ExecutionConfig) -> Value {
    let adapters = [
        "files",
        "shell",
        "git",
        "web.fetch",
        "office.ooxml",
        "office.libreoffice",
        "office.microsoft",
        "browser.headless",
        "browser.visible",
        "model.ollama",
        "model.vllm",
        "crew.python",
    ]
    .into_iter()
    .map(|name| cowork_contracts::DeviceInteropAdapter {
        name: name.into(),
        version: if name == "crew.python" {
            config
                .interop
                .crewai_version
                .clone()
                .unwrap_or_else(|| "unconfigured".into())
        } else {
            "device-adapter/1".into()
        },
        operations: match name {
            "office.microsoft" => vec![
                "word:export_pdf",
                "word:replace_text",
                "word:word_append_paragraph",
                "word:word_format_text",
                "excel:export_pdf",
                "excel:replace_text",
                "excel:excel_set_cell",
                "excel:excel_format_range",
                "excel:excel_add_chart",
                "powerpoint:export_pdf",
                "powerpoint:replace_text",
                "powerpoint:powerpoint_add_slide",
                "powerpoint:powerpoint_format_text",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            "office.ooxml" => vec!["native".into(), "template".into(), "parallel".into()],
            "browser.visible" => vec![
                "navigate".into(),
                "click".into(),
                "fill".into(),
                "dom".into(),
                "screenshot".into(),
            ],
            "crew.python" => vec!["sequential".into(), "hierarchical".into()],
            _ => vec![name.into()],
        },
        configured: available(name, config),
    })
    .collect();
    serde_json::to_value(cowork_contracts::DeviceInteropInventory {
        format: "device-local-adapter-inventory/1".into(),
        adapters,
        automatic_replay: false,
        adopted_processes: false,
    })
    .expect("plain inventory serializes")
}
