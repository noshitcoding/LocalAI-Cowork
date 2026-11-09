use crate::execution::{ExecutionConfig, McpBinding, ModelConfiguration};
use anyhow::{Context, Result, ensure};
use cowork_contracts::DeviceExecutorRegistration;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    pub format: u16,
    pub server_url: String,
    pub registration: DeviceExecutorRegistration,
    pub agent_token_file: PathBuf,
    pub model: Option<Model>,
    pub workspace_root: PathBuf,
    pub toolbox_data_dir: PathBuf,
    pub mcp_bindings_file: Option<PathBuf>,
    pub github_token_file: Option<PathBuf>,
    pub rfb_endpoint: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub base_url: String,
    pub model_name: String,
    pub api_key_file: Option<PathBuf>,
    #[serde(default = "default_api")]
    pub api: String,
    #[serde(default = "default_implementation")]
    pub implementation: String,
}
fn default_api() -> String {
    "openai".into()
}
fn default_implementation() -> String {
    "openai_compatible".into()
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolboxSettings {
    #[serde(default)]
    programs: BTreeMap<String, PathBuf>,
    github_token_file: Option<PathBuf>,
    #[serde(default)]
    native_owner_rights: bool,
    #[serde(default)]
    interop: crate::execution::InteropConfiguration,
}
pub fn ordinary(path: &Path, directory: bool) -> Result<()> {
    ensure!(path.is_absolute(), "Select an absolute local path");
    for parent in path.ancestors() {
        let metadata =
            std::fs::symlink_metadata(parent).context("Selected local path is unavailable")?;
        ensure!(
            !metadata.is_symlink(),
            "Selected paths must not contain symbolic links"
        );
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            ensure!(
                metadata.file_attributes() & 0x400 == 0,
                "Selected paths must not contain reparse points"
            );
        }
    }
    let metadata = std::fs::symlink_metadata(path)?;
    ensure!(
        if directory {
            metadata.is_dir()
        } else {
            metadata.is_file()
        },
        "Expected an ordinary local file or directory"
    );
    Ok(())
}
pub fn private_file(path: &Path, limit: u64) -> Result<File> {
    ordinary(path, false)?;
    let file = File::open(path)?;
    ensure!(
        file.metadata()?.len() <= limit,
        "Private configuration file exceeds limit"
    );
    #[cfg(windows)]
    crate::private_secret_file::validate(&file)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = file.metadata()?;
        ensure!(
            m.mode() & 0o077 == 0 && m.uid() == unsafe { libc::geteuid() } && m.nlink() == 1,
            "Configuration and credentials must be owned and private (0600)"
        );
    }
    Ok(file)
}
pub fn private_directory(path: &Path) -> Result<()> {
    ordinary(path, true)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(0x0200_0000)
            .open(path)?;
        crate::private_secret_file::validate_directory(&file)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = path.metadata()?;
        ensure!(
            m.mode() & 0o077 == 0 && m.uid() == unsafe { libc::geteuid() },
            "Agent state directory must be owned and private (0700)"
        );
    }
    Ok(())
}
pub fn secret(path: &Path) -> Result<Zeroizing<String>> {
    let mut raw = Zeroizing::new(String::new());
    private_file(path, 8192)?
        .take(8193)
        .read_to_string(&mut raw)?;
    let value = Zeroizing::new(raw.trim().to_owned());
    ensure!(
        !value.is_empty() && value.bytes().all(|b| (33..=126).contains(&b)),
        "Invalid private credential"
    );
    Ok(value)
}
fn json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    serde_json::from_reader(private_file(path, 256 * 1024)?)
        .context("Invalid private agent configuration")
}
pub fn origin(value: &str, path_allowed: bool) -> Result<()> {
    let url = reqwest::Url::parse(value).context("Invalid service URL")?;
    ensure!(
        url.scheme() == "https"
            || url.scheme() == "http"
                && matches!(
                    url.host_str(),
                    Some("127.0.0.1" | "localhost" | "[::1]" | "::1")
                ),
        "Service connections require HTTPS or loopback HTTP"
    );
    ensure!(
        url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "Use a service URL without credentials, query or fragment"
    );
    ensure!(
        path_allowed || matches!(url.path(), "" | "/"),
        "Server URL must be a canonical origin"
    );
    Ok(())
}
impl Configuration {
    pub fn load(path: &Path) -> Result<Self> {
        private_directory(
            path.parent()
                .context("Configuration needs an operator directory")?,
        )?;
        let c: Self = json(path)?;
        ensure!(
            c.format == 1
                && c.registration.schema_version == 2
                && c.registration.protocol_version == 2,
            "Unsupported V2 agent protocol"
        );
        ensure!(
            c.registration.max_concurrent_runs == 1,
            "Pair this V2 agent for one concurrent run"
        );
        ensure!(
            matches!(
                c.registration.kind.as_str(),
                "personal_device" | "managed_windows"
            ),
            "Unsupported device kind"
        );
        ensure!(
            c.registration.kind != "managed_windows" || cfg!(windows),
            "Managed Windows requires Windows"
        );
        origin(&c.server_url, false)?;
        private_directory(&c.workspace_root)?;
        private_directory(&c.toolbox_data_dir)?;
        let _ = secret(&c.agent_token_file)?;
        ensure!(
            c.rfb_endpoint.is_none(),
            "External RFB forwarding is unsupported; use the native desktop capability"
        );
        Ok(c)
    }
    pub fn execution(&self) -> Result<ExecutionConfig> {
        let path = self.toolbox_data_dir.join("toolbox.json");
        let toolbox: ToolboxSettings = if path.exists() {
            json(&path)?
        } else {
            ToolboxSettings::default()
        };
        ensure!(
            toolbox.programs.len() <= 32,
            "Program allowlist exceeds limit"
        );
        for (alias, path) in &toolbox.programs {
            ensure!(
                !alias.is_empty()
                    && alias.len() <= 64
                    && alias
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)),
                "Invalid program alias"
            );
            ordinary(path, false)?;
        }
        let model = self
            .model
            .as_ref()
            .map(|m| -> Result<_> {
                origin(&m.base_url, true)?;
                ensure!(
                    !m.model_name.is_empty()
                        && m.model_name.len() <= 200
                        && !m.model_name.chars().any(char::is_control),
                    "Invalid device model name"
                );
                Ok(ModelConfiguration {
                    base_url: m.base_url.clone(),
                    model: m.model_name.clone(),
                    api_key: m
                        .api_key_file
                        .as_ref()
                        .map(|p| secret(p).map(|s| s.to_string()))
                        .transpose()?,
                    timeout_seconds: 120,
                    api: m.api.clone(),
                    implementation: m.implementation.clone(),
                })
            })
            .transpose()?;
        let github_path = self
            .github_token_file
            .as_ref()
            .or(toolbox.github_token_file.as_ref());
        let github_token = github_path
            .map(|p| secret(p).map(|s| s.to_string()))
            .transpose()?;
        let mcp_bindings: Vec<McpBinding> = self
            .mcp_bindings_file
            .as_ref()
            .map(|p| json(p))
            .transpose()?
            .unwrap_or_default();
        ensure!(mcp_bindings.len() <= 8, "MCP binding limit exceeded");
        let mut tools = [
            "list_sources",
            "read_source",
            "create_artifact",
            "list_workspace_files",
            "read_workspace_file",
            "write_workspace_file",
            "publish_artifact",
            "request_input",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
        if toolbox.native_owner_rights {
            if !toolbox.programs.is_empty() {
                tools.push("execute_program".into());
            }
            if toolbox.programs.contains_key("chromium") {
                tools.push("browser".into());
            }
            if toolbox.programs.contains_key("python") {
                tools.push("office".into());
            }
            if toolbox.programs.contains_key("gh") && github_token.is_some() {
                tools.push("github".into());
            }
            if !mcp_bindings.is_empty() {
                tools.push("mcp_call".into());
            }
        }
        tools.extend(crate::capability_profiles::tools(
            &toolbox.programs,
            toolbox.native_owner_rights,
        ));
        if toolbox.interop.web_search_url.is_none() {
            tools.retain(|n| n != "web_search");
        }
        let execution = ExecutionConfig {
            data_directory: self.workspace_root.clone(),
            model,
            programs: toolbox.programs.clone(),
            environment: std::collections::BTreeMap::new(),
            github_token,
            mcp_bindings,
            allowed_tools: tools,
            native_owner_rights: toolbox.native_owner_rights,
            require_local_consent: self.registration.kind == "personal_device",
            max_steps: 32,
            interop: toolbox.interop,
            capability_names: self
                .registration
                .capabilities
                .iter()
                .map(|c| c.name.clone())
                .collect(),
        };
        for cap in &self.registration.capabilities {
            let available = match cap.name.as_str() {
                "model.external" | "model.chat" => execution.model.is_some(),
                "terminal" | "tool.terminal" => execution
                    .allowed_tools
                    .iter()
                    .any(|n| n == "execute_program"),
                "browser" | "tool.browser" => {
                    execution.allowed_tools.iter().any(|n| n == "browser")
                }
                "office" | "windows.office" => {
                    execution.allowed_tools.iter().any(|n| n == "office")
                }
                "github" | "tool.github" => execution.allowed_tools.iter().any(|n| n == "github"),
                "mcp" | "tool.mcp.invoke" => {
                    execution.allowed_tools.iter().any(|n| n == "mcp_call")
                }
                "desktop.windows" => {
                    cfg!(windows) && crate::desktop::available() && toolbox.native_owner_rights
                }
                "desktop.linux" => {
                    cfg!(target_os = "linux")
                        && crate::desktop::available()
                        && toolbox.native_owner_rights
                }
                other => crate::capability_profiles::available(other, &execution),
            };
            ensure!(
                available,
                "Paired capability is unsupported or lacks explicit local configuration: {}",
                cap.name
            );
        }
        let environment = [
            "PATH",
            "SystemRoot",
            "WINDIR",
            "LANG",
            "LC_ALL",
            "TEMP",
            "TMP",
        ]
        .into_iter()
        .filter_map(|n| std::env::var(n).ok().map(|v| (n.into(), v)))
        .collect();
        let mut execution = execution;
        execution.environment = environment;
        Ok(execution)
    }
}
