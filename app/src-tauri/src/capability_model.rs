#![allow(dead_code)]

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    ServerHealthRead,
    AuthLogin,
    TenantRead,
    TenantAdmin,
    WorkspaceRead,
    WorkspaceWrite,
    ProjectRead,
    ProjectWrite,
    TaskRead,
    TaskWrite,
    ChatRead,
    ChatWrite,
    RunStart,
    RunCancel,
    SchedulerRead,
    SchedulerWrite,
    FileServerRead,
    FileServerWrite,
    FileClientPick,
    FileClientReadSelected,
    ToolMcpRead,
    ToolMcpInvoke,
    ToolShellExecute,
    TerminalServerOpen,
    TerminalAgentOpen,
    DesktopScreenView,
    DesktopInputControl,
    DesktopWindowFocus,
    DesktopClipboardRead,
    DesktopClipboardWrite,
    SyncOfflineRead,
    SyncOfflinePush,
    SyncCrossServer,
    AdminAuthManage,
    AdminScimManage,
    AdminAuditView,
}

impl Capability {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ServerHealthRead => "server.health.read",
            Self::AuthLogin => "auth.login",
            Self::TenantRead => "tenant.read",
            Self::TenantAdmin => "tenant.admin",
            Self::WorkspaceRead => "workspace.read",
            Self::WorkspaceWrite => "workspace.write",
            Self::ProjectRead => "project.read",
            Self::ProjectWrite => "project.write",
            Self::TaskRead => "task.read",
            Self::TaskWrite => "task.write",
            Self::ChatRead => "chat.read",
            Self::ChatWrite => "chat.write",
            Self::RunStart => "run.start",
            Self::RunCancel => "run.cancel",
            Self::SchedulerRead => "scheduler.read",
            Self::SchedulerWrite => "scheduler.write",
            Self::FileServerRead => "file.server.read",
            Self::FileServerWrite => "file.server.write",
            Self::FileClientPick => "file.client.pick",
            Self::FileClientReadSelected => "file.client.read_selected",
            Self::ToolMcpRead => "tool.mcp.read",
            Self::ToolMcpInvoke => "tool.mcp.invoke",
            Self::ToolShellExecute => "tool.shell.execute",
            Self::TerminalServerOpen => "terminal.server.open",
            Self::TerminalAgentOpen => "terminal.agent.open",
            Self::DesktopScreenView => "desktop.screen.view",
            Self::DesktopInputControl => "desktop.input.control",
            Self::DesktopWindowFocus => "desktop.window.focus",
            Self::DesktopClipboardRead => "desktop.clipboard.read",
            Self::DesktopClipboardWrite => "desktop.clipboard.write",
            Self::SyncOfflineRead => "sync.offline.read",
            Self::SyncOfflinePush => "sync.offline.push",
            Self::SyncCrossServer => "sync.cross_server",
            Self::AdminAuthManage => "admin.auth.manage",
            Self::AdminScimManage => "admin.scim.manage",
            Self::AdminAuditView => "admin.audit.view",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        Some(match value {
            "server.health.read" => Self::ServerHealthRead,
            "auth.login" => Self::AuthLogin,
            "tenant.read" => Self::TenantRead,
            "tenant.admin" => Self::TenantAdmin,
            "workspace.read" => Self::WorkspaceRead,
            "workspace.write" => Self::WorkspaceWrite,
            "project.read" => Self::ProjectRead,
            "project.write" => Self::ProjectWrite,
            "task.read" => Self::TaskRead,
            "task.write" => Self::TaskWrite,
            "chat.read" => Self::ChatRead,
            "chat.write" => Self::ChatWrite,
            "run.start" => Self::RunStart,
            "run.cancel" => Self::RunCancel,
            "scheduler.read" => Self::SchedulerRead,
            "scheduler.write" => Self::SchedulerWrite,
            "file.server.read" => Self::FileServerRead,
            "file.server.write" => Self::FileServerWrite,
            "file.client.pick" => Self::FileClientPick,
            "file.client.read_selected" => Self::FileClientReadSelected,
            "tool.mcp.read" => Self::ToolMcpRead,
            "tool.mcp.invoke" => Self::ToolMcpInvoke,
            "tool.shell.execute" => Self::ToolShellExecute,
            "terminal.server.open" => Self::TerminalServerOpen,
            "terminal.agent.open" => Self::TerminalAgentOpen,
            "desktop.screen.view" => Self::DesktopScreenView,
            "desktop.input.control" => Self::DesktopInputControl,
            "desktop.window.focus" => Self::DesktopWindowFocus,
            "desktop.clipboard.read" => Self::DesktopClipboardRead,
            "desktop.clipboard.write" => Self::DesktopClipboardWrite,
            "sync.offline.read" => Self::SyncOfflineRead,
            "sync.offline.push" => Self::SyncOfflinePush,
            "sync.cross_server" => Self::SyncCrossServer,
            "admin.auth.manage" => Self::AdminAuthManage,
            "admin.scim.manage" => Self::AdminScimManage,
            "admin.audit.view" => Self::AdminAuditView,
            _ => return None,
        })
    }

    pub fn category(&self) -> CapabilityCategory {
        match self {
            Self::ServerHealthRead => CapabilityCategory::Server,
            Self::AuthLogin => CapabilityCategory::Auth,
            Self::TenantRead | Self::TenantAdmin => CapabilityCategory::Tenant,
            Self::WorkspaceRead | Self::WorkspaceWrite => CapabilityCategory::Workspace,
            Self::ProjectRead | Self::ProjectWrite => CapabilityCategory::Project,
            Self::TaskRead | Self::TaskWrite => CapabilityCategory::Task,
            Self::ChatRead | Self::ChatWrite => CapabilityCategory::Chat,
            Self::RunStart | Self::RunCancel => CapabilityCategory::Run,
            Self::SchedulerRead | Self::SchedulerWrite => CapabilityCategory::Scheduler,
            Self::FileServerRead
            | Self::FileServerWrite
            | Self::FileClientPick
            | Self::FileClientReadSelected => CapabilityCategory::File,
            Self::ToolMcpRead | Self::ToolMcpInvoke | Self::ToolShellExecute => {
                CapabilityCategory::Tool
            }
            Self::TerminalServerOpen | Self::TerminalAgentOpen => CapabilityCategory::Terminal,
            Self::DesktopScreenView
            | Self::DesktopInputControl
            | Self::DesktopWindowFocus
            | Self::DesktopClipboardRead
            | Self::DesktopClipboardWrite => CapabilityCategory::Desktop,
            Self::SyncOfflineRead | Self::SyncOfflinePush | Self::SyncCrossServer => {
                CapabilityCategory::Sync
            }
            Self::AdminAuthManage | Self::AdminScimManage | Self::AdminAuditView => {
                CapabilityCategory::Admin
            }
        }
    }

    pub fn is_dangerous(&self) -> bool {
        matches!(
            self,
            Self::TenantAdmin
                | Self::ToolMcpInvoke
                | Self::ToolShellExecute
                | Self::TerminalServerOpen
                | Self::TerminalAgentOpen
                | Self::DesktopScreenView
                | Self::DesktopInputControl
                | Self::DesktopWindowFocus
                | Self::DesktopClipboardRead
                | Self::DesktopClipboardWrite
                | Self::SyncOfflinePush
                | Self::SyncCrossServer
                | Self::AdminAuthManage
                | Self::AdminScimManage
                | Self::AdminAuditView
        )
    }
}

impl Serialize for Capability {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Capability {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_str(&value)
            .ok_or_else(|| de::Error::custom(format!("unknown capability: {value}")))
    }
}

pub const ALL_CAPABILITIES: &[Capability] = &[
    Capability::ServerHealthRead,
    Capability::AuthLogin,
    Capability::TenantRead,
    Capability::TenantAdmin,
    Capability::WorkspaceRead,
    Capability::WorkspaceWrite,
    Capability::ProjectRead,
    Capability::ProjectWrite,
    Capability::TaskRead,
    Capability::TaskWrite,
    Capability::ChatRead,
    Capability::ChatWrite,
    Capability::RunStart,
    Capability::RunCancel,
    Capability::SchedulerRead,
    Capability::SchedulerWrite,
    Capability::FileServerRead,
    Capability::FileServerWrite,
    Capability::FileClientPick,
    Capability::FileClientReadSelected,
    Capability::ToolMcpRead,
    Capability::ToolMcpInvoke,
    Capability::ToolShellExecute,
    Capability::TerminalServerOpen,
    Capability::TerminalAgentOpen,
    Capability::DesktopScreenView,
    Capability::DesktopInputControl,
    Capability::DesktopWindowFocus,
    Capability::DesktopClipboardRead,
    Capability::DesktopClipboardWrite,
    Capability::SyncOfflineRead,
    Capability::SyncOfflinePush,
    Capability::SyncCrossServer,
    Capability::AdminAuthManage,
    Capability::AdminScimManage,
    Capability::AdminAuditView,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeMode {
    LocalSidecar,
    RemoteDocker,
    DesktopRemoteClient,
    Browser,
    Mobile,
    PairedDesktopAgent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityCategory {
    Server,
    Auth,
    Tenant,
    Workspace,
    Project,
    Task,
    Chat,
    Run,
    Scheduler,
    File,
    Tool,
    Terminal,
    Desktop,
    Sync,
    Admin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    Enabled,
    Disabled,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityDescriptor {
    pub capability: Capability,
    pub category: CapabilityCategory,
    pub status: CapabilityStatus,
    pub dangerous: bool,
    pub reason: Option<String>,
}

impl CapabilityDescriptor {
    pub fn enabled(capability: Capability) -> Self {
        Self {
            capability,
            category: capability.category(),
            status: CapabilityStatus::Enabled,
            dangerous: capability.is_dangerous(),
            reason: None,
        }
    }

    pub fn disabled(capability: Capability, reason: impl Into<String>) -> Self {
        Self {
            capability,
            category: capability.category(),
            status: CapabilityStatus::Disabled,
            dangerous: capability.is_dangerous(),
            reason: Some(reason.into()),
        }
    }

    pub fn unsupported(capability: Capability, reason: impl Into<String>) -> Self {
        Self {
            capability,
            category: capability.category(),
            status: CapabilityStatus::Unsupported,
            dangerous: capability.is_dangerous(),
            reason: Some(reason.into()),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.status == CapabilityStatus::Enabled
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityResponse {
    pub runtime_mode: RuntimeMode,
    pub capabilities: Vec<CapabilityDescriptor>,
}

impl CapabilityResponse {
    pub fn local_sidecar_default() -> Self {
        Self::from_supported(RuntimeMode::LocalSidecar, LOCAL_SIDECAR_SUPPORTED)
    }

    pub fn remote_docker_default() -> Self {
        Self::from_supported(RuntimeMode::RemoteDocker, REMOTE_DOCKER_SUPPORTED)
    }

    pub fn desktop_remote_client_default() -> Self {
        Self::from_supported(
            RuntimeMode::DesktopRemoteClient,
            DESKTOP_REMOTE_CLIENT_SUPPORTED,
        )
    }

    pub fn browser_default() -> Self {
        Self::from_supported(RuntimeMode::Browser, BROWSER_SUPPORTED)
    }

    pub fn mobile_default() -> Self {
        Self::from_supported(RuntimeMode::Mobile, MOBILE_SUPPORTED)
    }

    pub fn paired_desktop_agent_default() -> Self {
        Self::from_supported(RuntimeMode::PairedDesktopAgent, PAIRED_AGENT_SUPPORTED)
    }

    pub fn from_supported(runtime_mode: RuntimeMode, supported: &[Capability]) -> Self {
        let capabilities = ALL_CAPABILITIES
            .iter()
            .copied()
            .map(|capability| {
                if !supported.contains(&capability) {
                    CapabilityDescriptor::unsupported(capability, "unsupported_in_runtime")
                } else if capability.is_dangerous() {
                    CapabilityDescriptor::disabled(capability, "dangerous_disabled_by_default")
                } else {
                    CapabilityDescriptor::enabled(capability)
                }
            })
            .collect();

        Self {
            runtime_mode,
            capabilities,
        }
    }

    pub fn with_explicit_enabled(mut self, capability: Capability) -> Self {
        if let Some(descriptor) = self
            .capabilities
            .iter_mut()
            .find(|candidate| candidate.capability == capability)
        {
            if descriptor.status != CapabilityStatus::Unsupported {
                descriptor.status = CapabilityStatus::Enabled;
                descriptor.reason = None;
            }
        }

        self
    }

    pub fn descriptor(&self, capability: Capability) -> Option<&CapabilityDescriptor> {
        self.capabilities
            .iter()
            .find(|candidate| candidate.capability == capability)
    }

    pub fn is_enabled(&self, capability: Capability) -> bool {
        self.descriptor(capability)
            .map(CapabilityDescriptor::is_enabled)
            .unwrap_or(false)
    }
}

const BASE_SERVER_SUPPORTED: &[Capability] = &[
    Capability::ServerHealthRead,
    Capability::AuthLogin,
    Capability::TenantRead,
    Capability::WorkspaceRead,
    Capability::WorkspaceWrite,
    Capability::ProjectRead,
    Capability::ProjectWrite,
    Capability::TaskRead,
    Capability::TaskWrite,
    Capability::ChatRead,
    Capability::ChatWrite,
    Capability::RunStart,
    Capability::RunCancel,
    Capability::SchedulerRead,
    Capability::SchedulerWrite,
    Capability::FileServerRead,
    Capability::FileServerWrite,
    Capability::ToolMcpRead,
    Capability::ToolMcpInvoke,
    Capability::ToolShellExecute,
    Capability::TerminalServerOpen,
    Capability::SyncCrossServer,
    Capability::AdminAuthManage,
    Capability::AdminScimManage,
    Capability::AdminAuditView,
];

const LOCAL_SIDECAR_SUPPORTED: &[Capability] = &[
    Capability::ServerHealthRead,
    Capability::AuthLogin,
    Capability::TenantRead,
    Capability::TenantAdmin,
    Capability::WorkspaceRead,
    Capability::WorkspaceWrite,
    Capability::ProjectRead,
    Capability::ProjectWrite,
    Capability::TaskRead,
    Capability::TaskWrite,
    Capability::ChatRead,
    Capability::ChatWrite,
    Capability::RunStart,
    Capability::RunCancel,
    Capability::SchedulerRead,
    Capability::SchedulerWrite,
    Capability::FileServerRead,
    Capability::FileServerWrite,
    Capability::FileClientPick,
    Capability::FileClientReadSelected,
    Capability::ToolMcpRead,
    Capability::ToolMcpInvoke,
    Capability::ToolShellExecute,
    Capability::TerminalServerOpen,
    Capability::DesktopScreenView,
    Capability::DesktopInputControl,
    Capability::DesktopWindowFocus,
    Capability::DesktopClipboardRead,
    Capability::DesktopClipboardWrite,
    Capability::SyncOfflineRead,
    Capability::SyncOfflinePush,
    Capability::SyncCrossServer,
    Capability::AdminAuthManage,
    Capability::AdminScimManage,
    Capability::AdminAuditView,
];

const REMOTE_DOCKER_SUPPORTED: &[Capability] = BASE_SERVER_SUPPORTED;

const DESKTOP_REMOTE_CLIENT_SUPPORTED: &[Capability] = &[
    Capability::ServerHealthRead,
    Capability::AuthLogin,
    Capability::TenantRead,
    Capability::WorkspaceRead,
    Capability::WorkspaceWrite,
    Capability::ProjectRead,
    Capability::ProjectWrite,
    Capability::TaskRead,
    Capability::TaskWrite,
    Capability::ChatRead,
    Capability::ChatWrite,
    Capability::RunStart,
    Capability::RunCancel,
    Capability::SchedulerRead,
    Capability::SchedulerWrite,
    Capability::FileServerRead,
    Capability::FileServerWrite,
    Capability::FileClientPick,
    Capability::FileClientReadSelected,
    Capability::SyncOfflineRead,
    Capability::SyncOfflinePush,
    Capability::SyncCrossServer,
    Capability::DesktopScreenView,
    Capability::DesktopInputControl,
    Capability::DesktopWindowFocus,
    Capability::DesktopClipboardRead,
    Capability::DesktopClipboardWrite,
];

const BROWSER_SUPPORTED: &[Capability] = &[
    Capability::ServerHealthRead,
    Capability::AuthLogin,
    Capability::TenantRead,
    Capability::WorkspaceRead,
    Capability::WorkspaceWrite,
    Capability::ProjectRead,
    Capability::ProjectWrite,
    Capability::TaskRead,
    Capability::TaskWrite,
    Capability::ChatRead,
    Capability::ChatWrite,
    Capability::RunStart,
    Capability::RunCancel,
    Capability::SchedulerRead,
    Capability::FileServerRead,
    Capability::FileServerWrite,
    Capability::SyncOfflineRead,
    Capability::SyncOfflinePush,
    Capability::SyncCrossServer,
];

const MOBILE_SUPPORTED: &[Capability] = &[
    Capability::ServerHealthRead,
    Capability::AuthLogin,
    Capability::TenantRead,
    Capability::WorkspaceRead,
    Capability::ProjectRead,
    Capability::TaskRead,
    Capability::ChatRead,
    Capability::ChatWrite,
    Capability::RunCancel,
    Capability::SchedulerRead,
    Capability::FileServerRead,
    Capability::SyncOfflineRead,
];

const PAIRED_AGENT_SUPPORTED: &[Capability] = &[
    Capability::FileClientPick,
    Capability::FileClientReadSelected,
    Capability::TerminalAgentOpen,
    Capability::DesktopScreenView,
    Capability::DesktopInputControl,
    Capability::DesktopWindowFocus,
    Capability::DesktopClipboardRead,
    Capability::DesktopClipboardWrite,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_sidecar_default_represents_exe_mode() {
        let response = CapabilityResponse::local_sidecar_default();

        assert_eq!(response.runtime_mode, RuntimeMode::LocalSidecar);
        assert!(response.is_enabled(Capability::ServerHealthRead));
        assert!(response.is_enabled(Capability::FileClientPick));
        assert_eq!(
            response
                .descriptor(Capability::DesktopInputControl)
                .map(|descriptor| descriptor.status),
            Some(CapabilityStatus::Disabled)
        );
        assert!(
            response
                .descriptor(Capability::DesktopInputControl)
                .expect("desktop input descriptor exists")
                .dangerous
        );
    }

    #[test]
    fn remote_docker_default_has_server_capabilities_without_notebook_control() {
        let response = CapabilityResponse::remote_docker_default();

        assert_eq!(response.runtime_mode, RuntimeMode::RemoteDocker);
        assert!(response.is_enabled(Capability::FileServerRead));
        assert_eq!(
            response
                .descriptor(Capability::FileClientPick)
                .map(|descriptor| descriptor.status),
            Some(CapabilityStatus::Unsupported)
        );
        assert_eq!(
            response
                .descriptor(Capability::DesktopInputControl)
                .map(|descriptor| descriptor.status),
            Some(CapabilityStatus::Unsupported)
        );
    }

    #[test]
    fn dangerous_capabilities_are_disabled_by_default_when_supported() {
        for response in [
            CapabilityResponse::local_sidecar_default(),
            CapabilityResponse::remote_docker_default(),
            CapabilityResponse::desktop_remote_client_default(),
            CapabilityResponse::browser_default(),
            CapabilityResponse::mobile_default(),
            CapabilityResponse::paired_desktop_agent_default(),
        ] {
            for descriptor in response.capabilities {
                if descriptor.dangerous {
                    assert_ne!(descriptor.status, CapabilityStatus::Enabled);
                }
            }
        }
    }

    #[test]
    fn dangerous_capability_requires_explicit_enable() {
        let response = CapabilityResponse::remote_docker_default();
        assert!(!response.is_enabled(Capability::ToolShellExecute));

        let response = response.with_explicit_enabled(Capability::ToolShellExecute);
        assert!(response.is_enabled(Capability::ToolShellExecute));
    }

    #[test]
    fn mobile_default_disables_terminal_and_desktop_control() {
        let response = CapabilityResponse::mobile_default();

        assert_eq!(response.runtime_mode, RuntimeMode::Mobile);
        assert!(response.is_enabled(Capability::TaskRead));
        assert_eq!(
            response
                .descriptor(Capability::TerminalServerOpen)
                .map(|descriptor| descriptor.status),
            Some(CapabilityStatus::Unsupported)
        );
        assert_eq!(
            response
                .descriptor(Capability::DesktopInputControl)
                .map(|descriptor| descriptor.status),
            Some(CapabilityStatus::Unsupported)
        );
    }

    #[test]
    fn capability_response_serializes_policy_names() {
        let response = CapabilityResponse::remote_docker_default();
        let value = serde_json::to_value(response).expect("capability response serializes");
        let capabilities = value["capabilities"]
            .as_array()
            .expect("capabilities are an array");

        assert_eq!(value["runtimeMode"], "remote_docker");
        assert!(capabilities
            .iter()
            .any(
                |capability| capability["capability"] == "tool.shell.execute"
                    && capability["status"] == "disabled"
                    && capability["dangerous"] == true
            ));
    }

    #[test]
    fn capability_deserializes_from_policy_name() {
        let capability: Capability =
            serde_json::from_str("\"desktop.input.control\"").expect("capability deserializes");

        assert_eq!(capability, Capability::DesktopInputControl);
        assert_eq!(capability.as_str(), "desktop.input.control");
    }
}
