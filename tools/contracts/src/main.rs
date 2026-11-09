use schemars::schema_for;
use serde_json::{Value, json};
mod android_administration;
mod android_auth_security;
mod android_auth_transport;
mod android_project_versions;
mod android_run_recovery;
mod android_schedules;
mod android_terminal;
mod android_workflows;
mod automation;
mod codex_accounts;
mod connectors;
mod container_recovery;
mod desktop_ops;
mod historical_import;
mod library_config;
mod mcp_oauth_tasks;
mod offline_mcp;
mod owner_managed_processes;
mod owner_managed_results;
mod owner_mcp_knowledge;
mod team_lost_host_recovery;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut schema = serde_json::to_value(schema_for!(cowork_workflow_contracts::ApiContractV2))?;
    schema["$id"] = json!("https://localai-cowork.invalid/contracts/v2");
    let directory = root.join("contracts");
    std::fs::create_dir_all(&directory)?;
    let defaults = cowork_contracts::Budgets::default();
    std::fs::write(
        directory.join("defaults.json"),
        format!(
            "{}\n",
            serde_json::to_string_pretty(&json!({"run_budgets":{
                "max_steps":defaults.max_steps,"max_seconds":defaults.max_seconds,"max_output_tokens":defaults.max_output_tokens,"max_total_tokens":defaults.max_total_tokens
            }}))?
        ),
    )?;
    std::fs::write(
        directory.join("api.schema.json"),
        format!("{}\n", serde_json::to_string_pretty(&schema)?),
    )?;
    let defs = schema["$defs"].clone();
    let mut openapi = json!({"openapi":"3.1.0","info":{"title":"LocalAI Cowork V2","version":env!("CARGO_PKG_VERSION")},
        "servers":[{"url":"/api/v2"}],"security":[{"daemonBearer":[]}],
        "components":{"schemas":defs,"securitySchemes":{"daemonBearer":{"type":"http","scheme":"bearer"}}},"paths":{}});
    let routes = [
        (
            "/runs/{run}/workflow/input-snapshot",
            "get",
            "",
            "WorkflowInputSnapshot",
            false,
        ),
        (
            "/orders/{order}/runs/{run}/working-copy/files",
            "get",
            "",
            "DirectoryListing",
            false,
        ),
        (
            "/orders/{order}/runs/{run}/working-copy/text",
            "get",
            "",
            "TextFile",
            false,
        ),
        (
            "/execution-plans/v2",
            "post",
            "PrepareOwnerExecutionPlanV2",
            "PreparedExecutionPlanV2",
            false,
        ),
        (
            "/execution-plans/v2/file-imports",
            "post",
            "PrepareImportExecutionPlanV2",
            "PreparedExecutionPlanV2",
            false,
        ),
        (
            "/execution-plans/v2/file-imports/retire-request",
            "post",
            "PrepareImportExecutionPlanV2",
            "RetiredExecutionRequest",
            false,
        ),
        (
            "/execution-plans/v2/file-imports/retire-intent",
            "post",
            "CreateImportRun",
            "RetiredExecutionRequest",
            false,
        ),
        (
            "/execution-plans/v2/file-restorations",
            "post",
            "PrepareRestoreExecutionPlanV2",
            "PreparedExecutionPlanV2",
            false,
        ),
        (
            "/execution-plans/v2/file-restorations/retire-request",
            "post",
            "PrepareRestoreExecutionPlanV2",
            "RetiredExecutionRequest",
            false,
        ),
        (
            "/execution-plans/v2/file-restorations/retire-intent",
            "post",
            "CreateRestoreRun",
            "RetiredExecutionRequest",
            false,
        ),
        (
            "/execution-plans/v2/retire-request",
            "post",
            "PrepareOwnerExecutionPlanV2",
            "RetiredExecutionRequest",
            false,
        ),
        (
            "/orders/{order}/execution-plans/v2/{plan}",
            "get",
            "",
            "PreparedExecutionPlanV2",
            false,
        ),
        (
            "/orders/{order}/execution-plans/v2/{plan}/commit",
            "post",
            "CommitExecutionPlan",
            "RunRecord",
            false,
        ),
        (
            "/orders/{order}/execution-plans/v2/{plan}/retire",
            "post",
            "CommitExecutionPlan",
            "PreparedExecutionPlanV2",
            false,
        ),
        (
            "/orders/{order}/execution-inputs",
            "post",
            "PrepareExecutionInput",
            "PreparedExecutionInput",
            false,
        ),
        (
            "/orders/{order}/execution-inputs/{snapshot}",
            "get",
            "",
            "PreparedExecutionInput",
            false,
        ),
        (
            "/orders/{order}/execution-inputs/{snapshot}/retire",
            "post",
            "RetireExecutionInput",
            "PreparedExecutionInput",
            false,
        ),
        (
            "/execution-plans",
            "post",
            "RunRequest",
            "PreparedExecutionPlan",
            false,
        ),
        (
            "/execution-plans/retire-request",
            "post",
            "RunRequest",
            "RetiredExecutionRequest",
            false,
        ),
        (
            "/orders/{order}/execution-plans/{plan}",
            "get",
            "",
            "PreparedExecutionPlan",
            false,
        ),
        (
            "/orders/{order}/execution-plans/{plan}/commit",
            "post",
            "CommitExecutionPlan",
            "RunRecord",
            false,
        ),
        (
            "/orders/{order}/execution-plans/{plan}/retire",
            "post",
            "CommitExecutionPlan",
            "PreparedExecutionPlan",
            false,
        ),
        (
            "/orders/{order}/document-actions/prepare",
            "post",
            "DocumentActionRequest",
            "DocumentPreparation",
            false,
        ),
        (
            "/orders/{order}/document-drafts",
            "put",
            "SaveDocumentDraft",
            "DocumentDraftSave",
            false,
        ),
        (
            "/orders/{order}/document-drafts/{draft}",
            "get",
            "",
            "DocumentDraft",
            false,
        ),
        (
            "/orders/{order}/document-drafts/{draft}/snapshots",
            "get",
            "",
            "DocumentDraftSnapshot",
            true,
        ),
        (
            "/orders/{order}/document-actions/by-key/{key}",
            "get",
            "",
            "DocumentPreparation",
            false,
        ),
        (
            "/orders/{order}/document-actions/{preparation}",
            "get",
            "",
            "DocumentPreparation",
            false,
        ),
        (
            "/orders/{order}/document-actions/{preparation}/commit",
            "post",
            "",
            "RunRecord",
            false,
        ),
        (
            "/toolbox/browser-connections",
            "get",
            "",
            "BrowserExtensionConnection",
            true,
        ),
        (
            "/orders/{order}/toolbox/browser-v2/sessions",
            "get",
            "",
            "BrowserSessionView",
            true,
        ),
        (
            "/orders/{order}/toolbox/browser-v2/sessions",
            "post",
            "CreateBrowserSession",
            "BrowserSessionView",
            false,
        ),
        (
            "/orders/{order}/toolbox/browser-v2/sessions/{session}/takeover",
            "post",
            "BrowserTakeover",
            "BrowserSessionView",
            false,
        ),
        (
            "/orders/{order}/toolbox/browser-v2/sessions/{session}/input",
            "post",
            "BrowserInput",
            "",
            false,
        ),
        (
            "/orders/{order}/toolbox/browser-v2/sessions/{session}/frame",
            "post",
            "",
            "",
            false,
        ),
        (
            "/orders/{order}/toolbox/browser-v2/sessions/{session}/tabs",
            "get",
            "",
            "",
            false,
        ),
        (
            "/orders/{order}/toolbox/browser-v2/profile/delete",
            "post",
            "DeleteBrowserProfile",
            "",
            false,
        ),
        (
            "/projects/{project}/orders/{order}/sandbox-session",
            "post",
            "ManageSandboxSession",
            "SandboxSession",
            false,
        ),
        (
            "/projects/{project}/orders/{order}/sandbox-operations",
            "get",
            "",
            "SandboxSessionOperation",
            true,
        ),
        (
            "/projects/{project}/orders/{order}/sandbox-preview",
            "post",
            "CreateSandboxPreview",
            "SandboxPreview",
            false,
        ),
        (
            "/providers/{provider}/settings",
            "get",
            "",
            "ProviderSettings",
            false,
        ),
        (
            "/providers/{provider}",
            "put",
            "UpdateProviderSettings",
            "ProviderSettings",
            false,
        ),
        (
            "/providers/{provider}",
            "delete",
            "DeleteProviderSettings",
            "",
            false,
        ),
        ("/providers/{provider}/models", "get", "", "", false),
        (
            "/providers/discover-models",
            "post",
            "DiscoverProviderModels",
            "",
            false,
        ),
        ("/providers/{provider}/test", "post", "", "", false),
        (
            "/projects/{project}/settings",
            "get",
            "",
            "ProjectSettings",
            false,
        ),
        (
            "/projects/{project}/settings",
            "put",
            "WriteProjectSettings",
            "ProjectSettings",
            false,
        ),
        (
            "/orders/{order}/management",
            "get",
            "",
            "ChatManagement",
            false,
        ),
        (
            "/orders/{order}/management",
            "put",
            "ManageChat",
            "ChatManagement",
            false,
        ),
        (
            "/orders/{order}/archive",
            "post",
            "ArchiveChat",
            "ChatManagement",
            false,
        ),
        (
            "/orders/{order}/delete",
            "post",
            "DeleteLibraryEntry",
            "LibraryDeleted",
            false,
        ),
        (
            "/projects/{project}/tasks",
            "get",
            "",
            "WorkspaceTask",
            true,
        ),
        (
            "/projects/{project}/tasks",
            "post",
            "WriteWorkspaceTask",
            "WorkspaceTask",
            false,
        ),
        ("/workspace-tasks/{task}", "get", "", "WorkspaceTask", false),
        (
            "/workspace-tasks/{task}",
            "put",
            "WriteWorkspaceTask",
            "WorkspaceTask",
            false,
        ),
        (
            "/workspace-tasks/{task}",
            "delete",
            "ArchiveWorkspaceTask",
            "WorkspaceTask",
            false,
        ),
        (
            "/workspace-tasks/{task}/run",
            "post",
            "StartWorkspaceTask",
            "RunRecord",
            false,
        ),
        (
            "/audit/checkpoints/transitions",
            "post",
            "PrepareAuditTrustTransition",
            "AuditTrustTransitionProposal",
            false,
        ),
        (
            "/audit/checkpoints/transitions/import",
            "post",
            "ImportAuditTrustTransition",
            "SignedAuditTrustTransition",
            false,
        ),
        (
            "/audit/checkpoints/transitions/{request}",
            "get",
            "",
            "AuditTrustTransitionProposal",
            false,
        ),
        (
            "/audit/checkpoints/transitions/{request}/original",
            "get",
            "",
            "",
            false,
        ),
        (
            "/native/audit/v1/verify",
            "post",
            "VerifyAuditV1",
            "AuditV1Verification",
            false,
        ),
        ("/audit/recovery", "get", "", "AuditRecoveryPlan", false),
        (
            "/audit/recovery",
            "post",
            "ConfirmAuditRecovery",
            "AuditRecoveryReceipt",
            false,
        ),
        (
            "/audit/retirements",
            "get",
            "",
            "AuditRecoveryReceipt",
            true,
        ),
        (
            "/audit/retirements/{request}/original",
            "get",
            "",
            "",
            false,
        ),
        (
            "/audit/checkpoints",
            "get",
            "",
            "AuditCheckpointStatus",
            false,
        ),
        (
            "/audit/checkpoints",
            "post",
            "CaptureAuditCheckpoint",
            "AuditCheckpointRequest",
            false,
        ),
        (
            "/audit/checkpoints/witness",
            "post",
            "ConfigureAuditWitness",
            "AuditWitnessBinding",
            false,
        ),
        (
            "/audit/checkpoints/import",
            "post",
            "ImportAuditCheckpoint",
            "SignedAuditCheckpoint",
            false,
        ),
        (
            "/audit/checkpoints/history",
            "get",
            "",
            "SignedAuditCheckpoint",
            true,
        ),
        (
            "/audit/checkpoints/requests/{request}",
            "get",
            "",
            "AuditCheckpointRequest",
            false,
        ),
        (
            "/audit/checkpoints/originals/{request}",
            "get",
            "",
            "",
            false,
        ),
        ("/model-registry", "get", "", "ModelRegistryView", true),
        (
            "/native-model-registry/import",
            "post",
            "ImportModelQualification",
            "ModelRegistryView",
            false,
        ),
        (
            "/native-model-registry/revoke",
            "post",
            "ModelRegistryRef",
            "",
            false,
        ),
        ("/actor-model-registry/projects", "get", "", "", true),
        ("/actor-model-registry/profiles", "get", "", "", true),
        (
            "/actor-model-registry/context",
            "get",
            "",
            "ActorModelContext",
            false,
        ),
        (
            "/actor-model-registry/entries",
            "get",
            "",
            "ActorModelRegistryView",
            true,
        ),
        ("/native-actor-model-registry/projects", "get", "", "", true),
        ("/native-actor-model-registry/profiles", "get", "", "", true),
        (
            "/native-actor-model-registry/context",
            "get",
            "",
            "ActorModelContext",
            false,
        ),
        (
            "/native-actor-model-registry/authority",
            "get",
            "",
            "",
            false,
        ),
        (
            "/native-actor-model-registry/entries",
            "get",
            "",
            "ActorModelRegistryView",
            true,
        ),
        (
            "/native-actor-model-registry/import",
            "post",
            "ImportActorModelQualification",
            "ActorModelRegistryView",
            false,
        ),
        (
            "/native-actor-model-registry/revoke",
            "post",
            "RevokeActorModelQualification",
            "",
            false,
        ),
        (
            "/model-context/preview",
            "post",
            "PreviewModelContext",
            "ModelContextView",
            false,
        ),
        (
            "/model-context/compact",
            "post",
            "CompactModelContext",
            "ContextCompactionView",
            false,
        ),
        (
            "/orders/{order}/context-archives",
            "get",
            "",
            "ContextCompactionView",
            true,
        ),
        (
            "/orders/{order}/context-archives/{archive}",
            "get",
            "",
            "ContextArchiveView",
            false,
        ),
        (
            "/providers/with-credential",
            "post",
            "ConfigurePersonalProvider",
            "ProviderProfile",
            false,
        ),
        (
            "/native-providers/with-credential",
            "post",
            "ConfigurePersonalProvider",
            "ProviderProfile",
            false,
        ),
        (
            "/offline/grants/{grant}",
            "get",
            "",
            "SignedOfflineGrant",
            false,
        ),
        ("/offline/runs/{run}", "get", "", "OfflineRun", false),
        (
            "/offline/tool-config",
            "post",
            "ConfigureOfflineTools",
            "OfflineToolConfigurationView",
            false,
        ),
        (
            "/offline/grants/{grant}/tool-config",
            "get",
            "",
            "OfflineToolConfigurationView",
            false,
        ),
        (
            "/offline/runs/{run}/cancel",
            "post",
            "",
            "OfflineRun",
            false,
        ),
        (
            "/offline/runs/{run}/workspace-file",
            "post",
            "OfflineWorkspaceUpload",
            "OfflineRun",
            false,
        ),
        (
            "/team-offline/{team}/{project}/grants",
            "get",
            "",
            "SignedOfflineGrant",
            true,
        ),
        (
            "/team-offline/{team}/{project}/grants",
            "post",
            "IssueOfflineGrant",
            "SignedOfflineGrant",
            false,
        ),
        (
            "/team-offline/{team}/{project}/grants/{grant}/revoke",
            "post",
            "",
            "",
            false,
        ),
        (
            "/offline-authority/status",
            "post",
            "OfflineGrantStatusRequest",
            "SignedOfflineGrantStatus",
            false,
        ),
        (
            "/team-offline/{team}/{project}/history",
            "get",
            "",
            "OfflineReconciliationReceipt",
            true,
        ),
        (
            "/team-offline/{team}/{project}/reconcile",
            "post",
            "OfflineRunExport",
            "OfflineReconciliationReceipt",
            false,
        ),
        ("/offline/device", "post", "", "OfflineDevice", false),
        (
            "/offline/device/proof",
            "post",
            "OfflineDeviceProofRequest",
            "OfflineDeviceProof",
            false,
        ),
        ("/offline/grants", "get", "", "OfflineLocalGrantView", true),
        (
            "/offline/grants/install",
            "post",
            "InstallOfflineGrant",
            "SignedOfflineGrant",
            false,
        ),
        (
            "/offline/model-key",
            "post",
            "ConfigureOfflineCredential",
            "",
            false,
        ),
        (
            "/offline/grants/{grant}/status-request",
            "post",
            "",
            "OfflineGrantStatusRequest",
            false,
        ),
        (
            "/offline/grants/status",
            "post",
            "SignedOfflineGrantStatus",
            "OfflineGrantStatus",
            false,
        ),
        ("/offline/runs", "get", "", "OfflineRunPage", false),
        (
            "/offline/runs",
            "post",
            "StartOfflineRun",
            "OfflineRun",
            false,
        ),
        (
            "/offline/runs/{run}/approve",
            "post",
            "ApproveOfflineTools",
            "OfflineRun",
            false,
        ),
        (
            "/offline/runs/{run}/export",
            "get",
            "",
            "OfflineRunExport",
            false,
        ),
        ("/teams", "get", "", "Team", true),
        ("/teams", "post", "CreateTeam", "Team", false),
        ("/teams/{team}", "get", "", "Team", false),
        ("/teams/{team}", "put", "UpdateTeam", "Team", false),
        ("/teams/{team}/members", "get", "", "TeamMember", true),
        (
            "/teams/{team}/members",
            "post",
            "AddTeamMember",
            "TeamMember",
            false,
        ),
        (
            "/teams/{team}/members/{member}",
            "put",
            "UpdateTeamMember",
            "TeamMember",
            false,
        ),
        (
            "/teams/{team}/provider",
            "put",
            "ConfigureTeamProvider",
            "TeamProvider",
            false,
        ),
        ("/teams/{team}/projects", "get", "", "TeamProject", true),
        (
            "/teams/{team}/projects",
            "post",
            "CreateTeamProject",
            "TeamProject",
            false,
        ),
        (
            "/teams/{team}/projects/{project}",
            "get",
            "",
            "TeamProject",
            false,
        ),
        (
            "/teams/{team}/projects/{project}",
            "put",
            "UpdateTeamProject",
            "TeamProject",
            false,
        ),
        (
            "/teams/{team}/projects/{project}/grants",
            "put",
            "TeamProjectGrant",
            "TeamProjectGrant",
            false,
        ),
        (
            "/teams/{team}/projects/{project}/grants",
            "get",
            "",
            "TeamProjectGrantView",
            true,
        ),
        (
            "/teams/{team}/projects/{project}/orders",
            "get",
            "",
            "TeamOrder",
            true,
        ),
        (
            "/teams/{team}/projects/{project}/orders",
            "post",
            "CreateTeamOrder",
            "TeamOrder",
            false,
        ),
        (
            "/teams/{team}/projects/{project}/documents",
            "get",
            "",
            "TeamDocument",
            true,
        ),
        (
            "/teams/{team}/projects/{project}/documents",
            "post",
            "SaveTeamDocument",
            "TeamDocument",
            false,
        ),
        (
            "/teams/{team}/projects/{project}/runs",
            "get",
            "",
            "TeamRun",
            true,
        ),
        (
            "/teams/{team}/runs",
            "post",
            "CreateTeamRun",
            "TeamRun",
            false,
        ),
        ("/teams/{team}/runs/{run}", "get", "", "TeamRun", false),
        (
            "/teams/{team}/runs/{run}/cancel",
            "post",
            "",
            "TeamRun",
            false,
        ),
        (
            "/teams/{team}/runs/{run}/steps",
            "get",
            "",
            "TeamRunStep",
            true,
        ),
        (
            "/teams/{team}/runs/{run}/tool-approvals",
            "get",
            "",
            "TeamToolApproval",
            true,
        ),
        (
            "/teams/{team}/runs/{run}/tool-approvals/{approval}/approve",
            "post",
            "ApproveTeamTool",
            "TeamToolApproval",
            false,
        ),
        ("/personal-money", "get", "", "PersonalMoneyStatus", false),
        (
            "/runs/{run}/workflow/inputs/{request}/draft",
            "get",
            "",
            "WorkflowInputDraftLookup",
            false,
        ),
        (
            "/runs/{run}/workflow/inputs/{request}/draft",
            "put",
            "SaveWorkflowInputDraft",
            "WorkflowInputDraftSave",
            false,
        ),
        (
            "/runs/{run}/workflow/inputs/{request}/draft/snapshots/{snapshot}",
            "get",
            "",
            "WorkflowInputDraftSnapshot",
            false,
        ),
        (
            "/runs/{run}/workflow/inputs/{request}/answer-intents",
            "post",
            "PrepareWorkflowInputAnswer",
            "WorkflowInputAnswerIntent",
            false,
        ),
        (
            "/runs/{run}/workflow/input-answer-intents/{intent}",
            "get",
            "",
            "WorkflowInputAnswerIntent",
            false,
        ),
        (
            "/runs/{run}/workflow/input-answer-intents/{intent}/commit",
            "post",
            "",
            "WorkflowInputAnswerIntent",
            false,
        ),
        (
            "/projects/{project}/workflow-drafts",
            "get",
            "",
            "WorkflowDraftPage",
            false,
        ),
        (
            "/projects/{project}/workflow-drafts",
            "put",
            "SaveWorkflowDraft",
            "WorkflowDraftSave",
            false,
        ),
        (
            "/projects/{project}/workflow-drafts/{draft}",
            "get",
            "",
            "WorkflowDraft",
            false,
        ),
        (
            "/projects/{project}/workflow-drafts/{draft}/snapshots",
            "get",
            "",
            "WorkflowDraftSnapshotPage",
            false,
        ),
        (
            "/projects/{project}/workflow-drafts/{draft}/snapshots/{snapshot}",
            "get",
            "",
            "WorkflowDraftSnapshot",
            false,
        ),
        (
            "/projects/{project}/workflow-operations",
            "get",
            "",
            "WorkflowOperationPage",
            false,
        ),
        (
            "/projects/{project}/workflow-operations",
            "post",
            "PrepareWorkflowOperation",
            "WorkflowOperation",
            false,
        ),
        (
            "/projects/{project}/workflow-operations/{operation}",
            "get",
            "",
            "WorkflowOperation",
            false,
        ),
        (
            "/projects/{project}/workflow-operations/{operation}/commit",
            "post",
            "",
            "WorkflowOperation",
            false,
        ),
        (
            "/projects/{project}/workflow-operations/{operation}/discard",
            "post",
            "",
            "WorkflowOperation",
            false,
        ),
        (
            "/projects/{project}/workflow-operations/{operation}/acknowledge",
            "post",
            "",
            "WorkflowOperation",
            false,
        ),
        (
            "/projects/{project}/workflows",
            "get",
            "",
            "WorkflowPage",
            false,
        ),
        (
            "/projects/{project}/workflows",
            "post",
            "CreateWorkflow",
            "WorkflowLibraryEntry",
            false,
        ),
        (
            "/projects/{project}/workflows/{workflow}",
            "get",
            "",
            "WorkflowLibraryEntry",
            false,
        ),
        (
            "/projects/{project}/workflows/{workflow}/versions/{version}",
            "get",
            "",
            "WorkflowVersion",
            false,
        ),
        (
            "/projects/{project}/workflows/{workflow}/versions",
            "post",
            "PublishWorkflow",
            "WorkflowPublication",
            false,
        ),
        (
            "/projects/{project}/workflows/{workflow}/revoke",
            "post",
            "RevokeWorkflow",
            "WorkflowPublication",
            false,
        ),
        ("/runs/{run}/workflow", "get", "", "WorkflowRunView", false),
        (
            "/projects/{project}/mcp-servers",
            "get",
            "",
            "McpServer",
            true,
        ),
        (
            "/projects/{project}/mcp-servers",
            "post",
            "ConfigureMcpServer",
            "McpServer",
            false,
        ),
        ("/runs/{run}/mcp-attempts", "get", "", "McpAttempt", true),
        ("/native/secrets", "get", "", "NativeSecretsStatus", false),
        (
            "/native/secrets",
            "post",
            "NativeSecretsSetup",
            "NativeSecretsReceipt",
            false,
        ),
        (
            "/native/secrets/requests",
            "get",
            "",
            "NativeSecretsRequestLookup",
            false,
        ),
        (
            "/native/secrets/reconcile",
            "post",
            "NativeSecretsResolution",
            "NativeSecretsReceipt",
            false,
        ),
        (
            "/native/secrets/abandon",
            "post",
            "NativeSecretsResolution",
            "NativeSecretsReceipt",
            false,
        ),
        ("/health", "get", "", "Health", false),
        ("/folders", "get", "", "FolderGrant", true),
        ("/native/folders", "get", "", "RegisteredFolder", true),
        (
            "/native/folders",
            "post",
            "NativeFolderSelection",
            "RegisteredFolder",
            false,
        ),
        (
            "/native/folder-requests",
            "get",
            "",
            "FolderRequestLookup",
            false,
        ),
        (
            "/native/folder-revocations",
            "post",
            "RevokeFolder",
            "FolderGrant",
            false,
        ),
        (
            "/runs/{run}/file-changes/{call}/written",
            "get",
            "",
            "TextFile",
            false,
        ),
        (
            "/runs/{run}/files/reconcile",
            "post",
            "ReconcileFile",
            "RunRecord",
            false,
        ),
        (
            "/runs/{run}/file-changes/{call}/previous",
            "get",
            "",
            "TextFile",
            false,
        ),
        ("/providers", "get", "", "ProviderProfile", true),
        (
            "/providers",
            "post",
            "CreateProvider",
            "ProviderProfile",
            false,
        ),
        ("/projects", "get", "", "Project", true),
        ("/projects", "post", "CreateProject", "Project", false),
        ("/orders", "get", "", "Order", true),
        ("/orders", "post", "CreateOrder", "Order", false),
        ("/orders/{order}", "get", "", "Order", false),
        ("/orders/{order}/draft", "put", "SaveDraft", "Order", false),
        ("/orders/{order}/messages", "get", "", "Message", true),
        (
            "/orders/{order}/workspace/files",
            "get",
            "",
            "DirectoryListing",
            false,
        ),
        (
            "/orders/{order}/workspace/text",
            "get",
            "",
            "TextFile",
            false,
        ),
        ("/orders/{order}/workspace", "get", "", "Workspace", false),
        (
            "/orders/{order}/workspace/stat",
            "get",
            "",
            "FileStat",
            false,
        ),
        ("/orders/{order}/uploads", "get", "", "UploadPage", false),
        (
            "/orders/{order}/uploads",
            "post",
            "ReserveUpload",
            "FileUpload",
            false,
        ),
        (
            "/orders/{order}/uploads/{upload}/content",
            "put",
            "",
            "FileUpload",
            false,
        ),
        (
            "/file-imports",
            "post",
            "CreateImportRun",
            "RunRecord",
            false,
        ),
        (
            "/orders/{order}/workspace/download",
            "get",
            "",
            "DownloadMetadata",
            false,
        ),
        (
            "/orders/{order}/runs/{run}/workspace/download",
            "get",
            "",
            "DownloadMetadata",
            false,
        ),
        (
            "/orders/{order}/runs/{run}/workspace/transfer",
            "get",
            "",
            "WorkspaceTransferManifest",
            false,
        ),
        (
            "/orders/{order}/runs/{run}/workspace/transfer/chunk",
            "get",
            "",
            "DownloadMetadata",
            false,
        ),
        ("/orders/{order}/history", "get", "", "OrderHistory", false),
        ("/orders/{order}/export", "get", "", "OrderExport", false),
        (
            "/orders/{order}/branches",
            "post",
            "BranchOrder",
            "Order",
            false,
        ),
        ("/activity", "get", "", "ActivityPage", false),
        ("/search", "get", "", "SearchPage", false),
        ("/orders/{order}/runs", "get", "", "RunRecord", true),
        ("/orders/{order}/artifacts", "get", "", "Artifact", true),
        ("/preflight", "post", "RunRequest", "RunPreflight", false),
        ("/runs", "post", "RunRequest", "RunRecord", false),
        (
            "/file-restorations",
            "post",
            "CreateRestoreRun",
            "RunRecord",
            false,
        ),
        ("/runs/{run}", "get", "", "RunDetail", false),
        ("/runs/{run}/events", "get", "", "RunEvent", true),
        ("/runs/{run}/cancel", "post", "", "RunRecord", false),
        ("/runs/{run}/resume", "post", "", "RunRecord", false),
        (
            "/runs/{run}/approvals/{approval}",
            "post",
            "ResolveApproval",
            "RunRecord",
            false,
        ),
        (
            "/runs/{run}/input",
            "post",
            "SubmitInput",
            "RunRecord",
            false,
        ),
    ];
    for (path, method, request, response, array) in routes {
        let reference = if response.is_empty() {
            json!({"type":"object","additionalProperties":true})
        } else {
            json!({"$ref":format!("#/components/schemas/{response}")})
        };
        let response_schema = if array {
            json!({"type":"array","items":reference})
        } else {
            reference
        };
        let mut operation = json!({"responses":{"200":{"description":"Confirmed durable state","content":{"application/json":{"schema":response_schema}}},
            "default":{"description":"Structured failure","content":{"application/json":{"schema":{"$ref":"#/components/schemas/ApiError"}}}}}});
        if !request.is_empty() {
            operation["requestBody"] = json!({"required":true,"content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{request}")}}}});
        }
        let mut params = Vec::new();
        if path.starts_with("/actor-model-registry/")
            || path.starts_with("/native-actor-model-registry/")
        {
            operation["description"] = json!(
                "Actual global identity session and actor-owned private project/profile only. Reads expose metadata; only the native operator boundary accepts independently reviewed artifact imports. Original receipts remain immutable after profile edits or revocation."
            );
            operation["security"] = if path.starts_with("/native-actor-model-registry/") {
                json!([{"daemonBearer":[],"nativeAuthority":[],"actorNativeSession":[]}])
            } else {
                json!([{"actorSession":[]}])
            };
            if method == "get"
                && (path.ends_with("/context")
                    || path.ends_with("/entries")
                    || path.ends_with("/authority"))
            {
                params.push(json!({"name":"project_id","in":"query","required":true,"schema":{"type":"string","format":"uuid"}}));
            }
            if path.ends_with("/context") {
                params.push(json!({"name":"profile_id","in":"query","required":true,"schema":{"type":"string","format":"uuid"}}));
            }
        }
        if path == "/offline-authority/status" {
            operation["security"] = json!([]);
            operation["description"] = json!(
                "Requires the signed device proof bound to the requested grant; returns a signed current status without a reusable identity bearer."
            );
        }
        if path == "/teams" || path.starts_with("/teams/") || path.starts_with("/team-offline/") {
            operation["security"] = json!([{"teamSession":[]}]);
            operation["description"] = json!(
                "Requires a current identity session, active team membership and the project's current ACL. Owner daemon credentials do not grant team access. LiteLLM manages team token and monetary limits."
            );
        }
        if path.starts_with("/native/")
            || path.starts_with("/native-providers/")
            || path.starts_with("/offline/")
        {
            operation["description"] = json!(
                "Native authority only: requires both credentials and rejects all browser Origin headers. The native key must never enter browser or agent state."
            );
            operation["security"] = json!([{"daemonBearer":[],"nativeAuthority":[]}]);
            if method == "get"
                && !path.starts_with("/native/secrets")
                && !path.starts_with("/offline")
            {
                params.push(json!({"name":"project_id","in":"query","required":true,"schema":{"type":"string"}}));
            }
            if path.ends_with("/folder-requests") || path == "/native/secrets/requests" {
                params.push(json!({"name":"key","in":"query","required":true,"schema":{"type":"string","maxLength":128}}));
            }
        }
        if path == "/offline/runs" && method == "get" {
            params.push(
                json!({"name":"cursor","in":"query","required":false,"schema":{"type":"string"}}),
            );
        }
        for segment in path.split('/') {
            if segment.starts_with('{') {
                params.push(json!({"name":segment.trim_matches(['{','}']),"in":"path","required":true,"schema":{"type":"string"}}));
            }
        }
        if (path == "/orders" || path == "/folders") && method == "get" {
            params.push(json!({"name":"project_id","in":"query","required":true,"schema":{"type":"string"}}));
        }
        if path.ends_with("/events") {
            params
                .push(json!({"name":"after","in":"query","schema":{"type":"integer","default":0}}));
        }
        if path.ends_with("/workspace/files")
            || path.ends_with("/workspace/text")
            || path.ends_with("/workspace/stat")
        {
            params.push(json!({"name":"path","in":"query","required":true,"schema":{"type":"string","minLength":1,"maxLength":4096}}));
        }
        if path.ends_with("/working-copy/files") || path.ends_with("/working-copy/text") {
            params.push(json!({"name":"folder_id","in":"query","required":true,"schema":{"type":"string","minLength":1,"maxLength":256}}));
            params.push(json!({"name":"path","in":"query","required":true,"schema":{"type":"string","minLength":1,"maxLength":4096}}));
            operation["description"] = json!(
                "Read the selected run's confirmed private working copy through its stored folder capabilities. This route does not publish results, grant execution, or address original host directories."
            );
        }
        if path.ends_with("/uploads") && method == "get" {
            params.push(json!({"name":"offset","in":"query","schema":{"type":"integer","minimum":0,"maximum":1000000,"default":0}}));
        }
        if path.ends_with("/uploads/{upload}/content") {
            operation["description"] = json!(
                "Retryable full-file upload to an existing reservation; exact size and SHA-256 must match. At most 64 MiB and 60 seconds; two active body transfers per daemon process. Confirmation means a flushed, verified blob and durable metadata, not a workspace mutation."
            );
            operation["requestBody"] = json!({"required":true,"content":{"application/octet-stream":{"schema":{"type":"string","format":"binary"}}}});
        }
        if path.ends_with("/workspace/download") || path.ends_with("/workspace/transfer/chunk") {
            let transfer_chunk = path.ends_with("/workspace/transfer/chunk");
            let input_type = if transfer_chunk {
                "ReadWorkspaceTransferChunk"
            } else {
                "DownloadFile"
            };
            for (name, required) in if transfer_chunk {
                vec![("path", true), ("offset", true), ("expected_sha256", true)]
            } else {
                vec![("path", true), ("expected_sha256", false)]
            } {
                params.push(json!({"name":name,"in":"query","required":required,"schema":{"$ref":format!("#/components/schemas/{input_type}/properties/{name}")}}));
            }
            operation["responses"]["200"] = json!({
                "description":"Verified regular file (at most 64 MiB). Snapshot paths resolve only in the stored manifest. No token in download URLs.",
                "headers":{
                    "Content-Disposition":{"description":"attachment, with a UTF-8 percent-encoded filename*","schema":{"type":"string"}},
                    "Content-Length":{"schema":{"$ref":"#/components/schemas/DownloadMetadata/properties/size"}},
                    "X-Cowork-File-Name":{"description":"Percent-encoded DownloadMetadata.file_name","schema":{"type":"string"}},
                    "X-Cowork-Sha256":{"schema":{"$ref":"#/components/schemas/DownloadMetadata/properties/sha256"}}
                },
                "content":{"application/octet-stream":{"schema":{"type":"string","format":"binary"}}}
            });
            if transfer_chunk {
                operation["responses"]["200"]["description"] = json!(
                    "Verified retained-source chunk (at most 4 MiB), with aligned offset and exact SHA-256. Repeat the same read to resume. Content hashes do not grant access; order/run owner authentication is required."
                );
            }
        }
        if path == "/activity"
            || path == "/search"
            || (matches!(
                response,
                "WorkflowPage"
                    | "WorkflowDraftPage"
                    | "WorkflowDraftSnapshotPage"
                    | "WorkflowOperationPage"
            ) && method == "get")
        {
            params.push(json!({"name":"offset","in":"query","schema":{"type":"integer","minimum":0,"maximum":1000000,"default":0}}));
            params.push(json!({"name":"limit","in":"query","schema":{"type":"integer","minimum":1,"maximum":100,"default":30}}));
        }
        if response == "WorkflowOperationPage" {
            params.push(json!({"name":"include_acknowledged","in":"query","schema":{"type":"boolean","default":false}}));
        }
        if matches!(response, "WorkflowDraftSave" | "WorkflowInputDraftSave") {
            operation["responses"]["409"] = json!({"description":"Incoming draft preserved as an immutable conflict snapshot; current head unchanged. Other conflicts use ApiError.","content":{"application/json":{"schema":{"oneOf":[{"$ref":format!("#/$defs/{response}")},{"$ref":"#/$defs/ApiError"}]}}}});
        }
        if path == "/activity" {
            params.push(json!({"name":"attention_only","in":"query","schema":{"type":"boolean","default":false}}));
        }
        if path == "/search" {
            params.push(json!({"name":"q","in":"query","required":true,"schema":{"type":"string","minLength":1,"maxLength":200}}));
            params.push(json!({"name":"project_id","in":"query","schema":{"type":"string"}}));
        }
        if !params.is_empty() {
            operation["parameters"] = json!(params);
        }
        openapi["paths"][path][method] = operation;
    }
    openapi["paths"]["/runs/{run}/events/stream"]["get"] = json!({"parameters":[
        {"name":"run","in":"path","required":true,"schema":{"type":"string"}},
        {"name":"after","in":"query","schema":{"type":"integer","default":0}}],
        "responses":{"200":{"description":"Committed run events, event id is sequence","content":{"text/event-stream":{"schema":{"type":"string"}}}}}});
    openapi["components"]["securitySchemes"]["nativeAuthority"] =
        json!({"type":"apiKey","in":"header","name":"x-cowork-native"});
    openapi["components"]["securitySchemes"]["teamSession"] =
        json!({"type":"http","scheme":"bearer"});
    openapi["components"]["securitySchemes"]["actorSession"] =
        json!({"type":"http","scheme":"bearer"});
    openapi["components"]["securitySchemes"]["actorNativeSession"] =
        json!({"type":"apiKey","in":"header","name":"x-cowork-actor-session"});
    openapi["paths"]["/teams/{team}/provider"]["get"] = json!({"security":[{"teamSession":[]}],"parameters":[{"name":"team","in":"path","required":true,"schema":{"type":"string"}}],"responses":{"200":{"description":"Configured non-secret provider metadata or null; never returns an API key.","content":{"application/json":{"schema":{"anyOf":[{"$ref":"#/components/schemas/TeamProvider"},{"type":"null"}]}}}}}});
    owner_managed_results::extend(&mut openapi);
    owner_managed_processes::extend(&mut openapi);
    android_auth_security::extend(&mut openapi);
    android_administration::extend(&mut openapi);
    android_auth_transport::extend(&mut openapi);
    historical_import::extend(&mut openapi);
    library_config::extend(&mut openapi);
    codex_accounts::extend(&mut openapi);
    android_terminal::extend(&mut openapi);
    android_schedules::extend(&mut openapi);
    android_run_recovery::extend(&mut openapi);
    android_project_versions::extend(&mut openapi);
    android_workflows::extend(&mut openapi);
    mcp_oauth_tasks::extend(&mut openapi);
    owner_mcp_knowledge::extend(&mut openapi);
    desktop_ops::extend(&mut openapi);
    connectors::extend(&mut openapi);
    automation::extend(&mut openapi);
    offline_mcp::extend(&mut openapi);
    container_recovery::extend(&mut openapi);
    team_lost_host_recovery::extend(&mut openapi);
    // OpenAPI's schema location differs from standalone JSON Schema.
    fn refs(value: &mut Value) {
        match value {
            Value::Object(map) => {
                for (k, v) in map {
                    if k == "$ref" {
                        if let Some(s) = v.as_str() {
                            *v = Value::String(s.replace("#/$defs/", "#/components/schemas/"));
                        }
                    } else {
                        refs(v);
                    }
                }
            }
            Value::Array(items) => {
                for v in items {
                    refs(v);
                }
            }
            _ => {}
        }
    }
    refs(&mut openapi);
    std::fs::write(
        directory.join("openapi.json"),
        format!("{}\n", serde_json::to_string_pretty(&openapi)?),
    )?;
    Ok(())
}
