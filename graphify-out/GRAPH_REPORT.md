# Graph Report - Open_Cowork  (2026-07-18)

## Corpus Check
- 264 files · ~435,416 words
- Verdict: corpus is large enough that graph structure adds value.

## Summary
- 4288 nodes · 11743 edges · 162 communities (140 shown, 22 thin omitted)
- Extraction: 99% EXTRACTED · 1% INFERRED · 0% AMBIGUOUS · INFERRED: 81 edges (avg confidence: 0.74)
- Token cost: 0 input · 0 output

## Graph Freshness
- Built from commit: `3df8df7a`
- Run `git rev-parse HEAD` and compare to check if the graph is stale.
- Run `graphify update .` after code changes (no API cost).

## Community Hubs (Navigation)
- Result
- audit_service.rs
- crewStore.ts
- Database
- terminal_backends.rs
- lib.rs
- String
- registry.ts
- CoworkView.tsx
- main.py
- engineStore.ts
- workTaskCrewRuntime.ts
- safeInvoke
- tr
- crew_python_bridge.rs
- index.ts
- ollamaClient.ts
- configStore.ts
- index.ts
- ollama.rs
- ServiceError
- String
- QueryEngine
- db.rs
- artifact_pipeline.rs
- Option
- supply-chain.mjs
- RunPanel.tsx
- coworkStore.ts
- Capability
- mcp.rs
- context.rs
- cowork_features.rs
- collect_crew_governance_payload
- App.tsx
- credential_store.rs
- devDependencies
- openaiCompatibleClient.ts
- file_safety.rs
- AuditEvent
- AppHandle
- terminal_sessions.rs
- dependencies
- i18n.ts
- CrewRuntimeTaskTests
- load_policy_state
- network_safety.rs
- chatAttachments.ts
- memorySystem.ts
- EventEnvelope
- memory_engine.rs
- office_integration.rs
- CrewLiveMonitor.tsx
- memoryStore.ts
- resources
- SettingsView.tsx
- sensitive_data.rs
- personalityStore.ts
- chatProvider.ts
- claude_code_bridge.rs
- logStore.ts
- scripts
- projectStore.ts
- insightsStore.ts
- mcp_call_tool
- compilerOptions
- index.ts
- terminalStore.ts
- crew_provider_health_check
- registry.ts
- _safe_result
- i18n-audit.mjs
- messageDisplay.ts
- pipelineStore.ts
- lifecycle.ts
- globalSearch.ts
- RequestContext
- chatStore.ts
- workTasksStore.ts
- crew_tools.py
- ollamaStreaming.ts
- compilerOptions
- check-budgets.mjs
- useTerminalStore
- SkillPanel.tsx
- _BingParser
- worker_sandbox.rs
- next_run_from_expression
- agentCoordinator.ts
- engineStore.test.ts
- insights.rs
- duckduckgo-websearch-server.mjs
- ProjectView.tsx
- documentWorkspaceStore.ts
- skill_engine.rs
- doctor.mjs
- .insert_engine_run_event_with_details
- webSearchSources.ts
- McpView.tsx
- build.rs
- permissions
- .get_engine_run
- LocalAI Cowork
- scan-secrets.mjs
- PageParser
- queryEngine.approval.test.ts
- .__init__
- package.json
- sign-windows-installer.ps1
- Ollama Konfiguration
- ui-quality.spec.ts
- LocalAI Cowork App
- WorkTaskRow
- followUpPrompt.ts
- ChatTurnRequest
- generate-brand-assets.mjs
- createDesktopscreenshotAttachment
- startup_recovery_reconciles_active_state_after_reopen_and_is_idempotent
- Contributing
- Desktop-Steuerung und Computer Use
- Security Policy
- .list_artifact_exports
- README.md
- Support
- desktop-smoke.mjs
- verify.mjs
- pull_request_template.md
- Q: warum wird mein Opencowork nicht bei einer Google suche angezigt?
- Q: Where are credentials and private data handled, and what repository surfaces control SEO, releases, and community reporting?
- Q: Überlege dir ein neues Logo für Opencowork
- openaiCompatibleClient.test.ts
- CreateDirectoryTool
- _fetch_public_text
- Quick Start
- capture-marketing-preview.mjs
- ollamaClient.test.ts
- resolveShellNavigationTarget
- ollamaStreaming.test.ts
- tsconfig.json
- refactor-settings.js
- refactor-ui.js
- eslint-plugin-react-refresh
- globals
- tailwindcss
- @tauri-apps/cli
- memorySystem.ipc.test.ts
- registry.filesystem.test.ts
- registry.memory.test.ts
- documentWorkspaceStore.test.ts

## God Nodes (most connected - your core abstractions)
1. `Database` - 369 edges
2. `tr()` - 97 edges
3. `CoworkView()` - 76 edges
4. `safeInvoke()` - 60 edges
5. `useConfigStore` - 47 edges
6. `CredentialStore` - 37 edges
7. `QueryEngine` - 36 edges
8. `hasTauriRuntime()` - 36 edges
9. `ServiceError` - 33 edges
10. `execute_definition()` - 32 edges

## Surprising Connections (you probably didn't know these)
- `CrewPanel()` --indirect_call--> `providerKind`  [INFERRED]
  app/src/components/CrewPanel.tsx → app/src/engine/types/lifecycle.ts
- `FeaturesView()` --indirect_call--> `command()`  [INFERRED]
  app/src/components/FeaturesView.tsx → app/scripts/supply-chain.mjs
- `TerminalDock()` --indirect_call--> `command()`  [INFERRED]
  app/src/components/TerminalDock.tsx → app/scripts/supply-chain.mjs
- `deterministic_office_fallback()` --calls--> `OfficeWorkflowTool`  [INFERRED]
  app/src-tauri/python/crew_runtime/main.py → app/src-tauri/python/crew_runtime/crew_tools.py
- `LiveCapture` --uses--> `OfficeWorkflowTool`  [INFERRED]
  app/src-tauri/python/crew_runtime/main.py → app/src-tauri/python/crew_runtime/crew_tools.py

## Import Cycles
- None detected.

## Communities (162 total, 22 thin omitted)

### Community 0 - "Result"
Cohesion: 0.04
Nodes (155): CredentialStore, Default, ArtifactExportRow, backend_delete(), backend_ensure_local(), backend_exec(), backend_list(), backend_upsert() (+147 more)

### Community 1 - "audit_service.rs"
Cohesion: 0.05
Nodes (104): append_audit_event(), integrity_report(), Option, Path, PathBuf, Result, String, Value (+96 more)

### Community 2 - "crewStore.ts"
Cohesion: 0.03
Nodes (75): CrewControlPlanePanel(), formatTimestamp(), Props, CrewGovernancePanel(), formatApprovalStatus(), formatApprovalType(), formatTimestamp(), GOVERNANCE_MODES (+67 more)

### Community 3 - "Database"
Cohesion: 0.05
Nodes (17): CrewApprovalRow, CrewDefinitionRow, CrewRoleBindingRow, CrewRunEventRow, Database, map_worker_sandbox_row(), MemoryEntryRow, Option (+9 more)

### Community 4 - "terminal_backends.rs"
Cohesion: 0.06
Nodes (82): attach_process_tree(), configure_process_tree(), ProcessTreeGuard, Child, Option, Result, Send, terminate_platform_tree() (+74 more)

### Community 5 - "lib.rs"
Cohesion: 0.04
Nodes (77): authorize_worker_sandbox_source(), claude_code_list_commands(), claude_code_list_tools(), configure_pdfium_search_paths(), db_delete_messages(), decode_html_entities(), DeletedMessagesResponse, execute_pipeline_llm_step() (+69 more)

### Community 6 - "String"
Cohesion: 0.04
Nodes (80): crew_stop(), CrewGovernanceAgentAccessPayload, CrewGovernancePayload, CrewMemoryEntryPayload, CrewMemoryPayload, CrewStopRequest, CrewUserProfilePayload, db_list_messages() (+72 more)

### Community 7 - "registry.ts"
Cohesion: 0.03
Nodes (71): agentTool, askUserTool, AskUserToolInput, bashTool, copyPathTool, createDirectoryTool, deleteFileTool, DesktopActionResponse (+63 more)

### Community 8 - "CoworkView.tsx"
Cohesion: 0.05
Nodes (67): appendStoppedAssistantContent(), AskUserOption, AskUserPromptModel, buildChatExportPayload(), buildEngineUserInput(), buildProjectInstructionsPromptContext(), buildProjectLinkPromptContext(), ChatExportFormat (+59 more)

### Community 9 - "main.py"
Cohesion: 0.07
Nodes (67): agent_display_name(), bridge_textual_tool_call(), build_agent(), build_artifact_repair_description(), build_governance_note(), build_llm(), build_memory_note(), build_task_description() (+59 more)

### Community 10 - "engineStore.ts"
Cohesion: 0.05
Nodes (54): invokeMock, normalizeSessions(), normalizeSessionSummary(), SessionLike, SessionSearchPanel(), sessionRecord, sessionSummary, toNumber() (+46 more)

### Community 11 - "workTaskCrewRuntime.ts"
Cohesion: 0.07
Nodes (60): crew, scheduledTask, TasksView(), buildCrewRunOutput(), createCrewStreamId(), CrewTaskMessageParams, handleCrewTaskMessage(), appendCrewLiveEntry() (+52 more)

### Community 12 - "safeInvoke"
Cohesion: 0.06
Nodes (51): CrewRuntimePanel(), formatTimestamp(), hasTauriRuntimeMock, randomId(), RuntimeInstructionRow, RuntimeInstructionsPanel(), hydrateMcpServer(), initializeCredentialVault() (+43 more)

### Community 13 - "tr"
Cohesion: 0.07
Nodes (51): CATEGORY_LABELS, CATEGORY_ORDER, CommandPalette(), FeaturesView(), isWorkbenchTab(), TABS, WorkbenchTab, Layout() (+43 more)

### Community 14 - "crew_python_bridge.rs"
Cohesion: 0.13
Nodes (59): build_status_from_json(), command_available(), configured_crew_python(), crew_runtime_bootstrap(), crew_runtime_execute_request(), crew_runtime_status(), crew_runtime_status_internal(), crew_runtime_validate_definition() (+51 more)

### Community 15 - "index.ts"
Cohesion: 0.07
Nodes (46): ANTHROPIC_MODELS, AnthropicAPIError, AnthropicConfig, APIContentBlock, APIMessage, APIToolDef, calculateCost(), COST_PER_MILLION (+38 more)

### Community 16 - "ollamaClient.ts"
Cohesion: 0.07
Nodes (52): blockContentToText(), buildOllamaChatRequest(), buildStartTagRegex(), buildToolLookup(), canDelayVisibleStream(), canonicalizeArgumentKey(), canUseTauriInvoke(), clipOllamaDebugText() (+44 more)

### Community 17 - "configStore.ts"
Cohesion: 0.05
Nodes (46): ExternalProviderHealthCheckResult, ExternalProviderModelsResult, isLlmProviderKind(), LlmProfilesPanel(), modelSuffix(), parseNumericInput(), ProfileHealthState, ProfileModelsState (+38 more)

### Community 18 - "index.ts"
Cohesion: 0.09
Nodes (49): PermissionDecision, getAllTools(), accumulateUsage(), AgentToolProgress, AssistantMessage, AttachmentMessage, BashProgress, ContentBlockDelta (+41 more)

### Community 19 - "ollama.rs"
Cohesion: 0.13
Nodes (51): build_chat_messages(), build_chat_prompt(), build_chat_turn_response(), build_chat_turn_response_preserves_tool_calls(), build_http_client(), chat_turn(), chat_turn_stream(), chat_turn_with_tools() (+43 more)

### Community 20 - "ServiceError"
Cohesion: 0.08
Nodes (25): api_error_response_matches_contract_shape(), ApiError, ApiErrorResponse, conflict_error_maps_to_http_conflict(), current_tauri_result_conversion_keeps_existing_string_boundary(), forbidden_error_maps_to_permission_denied(), internal_error_does_not_leak_source_to_safe_message_or_tauri_conversion(), into_tauri_result() (+17 more)

### Community 21 - "String"
Cohesion: 0.06
Nodes (17): CrewDefinitionVersionRow, EngineRunCheckpointRow, LearningOutcomeRow, ManagedProcessRow, MemoryProviderRow, PersonalityRow, project_thread_assignment_is_exclusive_and_delete_can_remove_threads(), ProjectResourceRow (+9 more)

### Community 22 - "QueryEngine"
Cohesion: 0.09
Nodes (20): streamOllamaMessagesMock, QueryEngine, chunk(), DEFAULT_ORCHESTRATOR_CONFIG, ToolExecutionEvent, ToolExecutionResult, ToolOrchestrator, ToolOrchestratorConfig (+12 more)

### Community 23 - "db.rs"
Cohesion: 0.09
Nodes (34): add_column_if_missing(), configure_connection(), connection_pragmas_enforce_integrity_and_contention_policy(), corrupt_database_files_and_invalid_data_directories_are_rejected(), create_pre_migration_backup(), creating_an_existing_session_does_not_replace_its_frozen_snapshot(), current_schema_version(), database_error() (+26 more)

### Community 24 - "artifact_pipeline.rs"
Cohesion: 0.16
Nodes (45): ArtifactParseResponse, bind_pdfium(), extract_pdf_text_with_pdfium(), extract_text_for_llm(), extract_text_for_llm_limited(), parse_artifact(), parse_binary(), parse_csv() (+37 more)

### Community 25 - "Option"
Cohesion: 0.06
Nodes (46): ArtifactVersionRow, build_exec_command_text(), crew_approval_create(), crew_approval_list(), crew_approval_resolve(), crew_role_binding_list(), crew_role_binding_upsert(), crew_runs_list() (+38 more)

### Community 26 - "supply-chain.mjs"
Cohesion: 0.11
Nodes (43): argumentValue(), assertRegularAsset(), assertTauriVersionCompatibility(), assertVersionConsistency(), cargoPurl(), cargoRustVersion(), cargoVersion(), collectInventory() (+35 more)

### Community 27 - "RunPanel.tsx"
Cohesion: 0.09
Nodes (40): canOpenArtifact(), CoworkContextRail(), CoworkContextRailProps, STATUS_LABELS, baseProps, safeInvokeMock, task, toolStatusIcon() (+32 more)

### Community 28 - "coworkStore.ts"
Cohesion: 0.06
Nodes (41): BackendConnectorTestResponse, BackendScheduledRunRow, BackendScheduledTaskRow, buildPolicySyncRequest(), CLAUDE_TOOL_CAPABILITIES, ClaudeToolCapability, ClaudeToolPreset, ConnectorConfig (+33 more)

### Community 29 - "Capability"
Cohesion: 0.11
Nodes (24): Capability, capability_deserializes_from_policy_name(), capability_response_serializes_policy_names(), CapabilityCategory, CapabilityDescriptor, CapabilityResponse, CapabilityStatus, dangerous_capabilities_are_disabled_by_default_when_supported() (+16 more)

### Community 30 - "mcp.rs"
Cohesion: 0.16
Nodes (42): call_tool(), format_call_result(), McpCallRequest, McpCallResponse, McpError, McpProbeResponse, McpRuntimeServerStatus, McpServerRequest (+34 more)

### Community 31 - "context.rs"
Cohesion: 0.09
Nodes (19): Actor, ActorId, ActorKind, ActorRole, anonymous_fixture_has_no_actor_id(), ClientPlatform, context_serializes_with_camel_case_fields(), local_default_context_needs_no_network_config() (+11 more)

### Community 32 - "cowork_features.rs"
Cohesion: 0.17
Nodes (42): analyze_single_path(), apply_office_template_transform(), ArtifactVersionExportInput, build_artifact_field_rows(), build_workflow_headers(), build_workflow_rows(), build_workflow_totals(), compute_numeric_totals() (+34 more)

### Community 33 - "collect_crew_governance_payload"
Cohesion: 0.08
Nodes (43): build_crew_memory_query(), build_effective_crew_tool_ids(), collect_crew_governance_payload(), collect_crew_memory_payload(), crew_agent_can_delegate(), crew_execute(), crew_role_allows_execution(), crew_role_allows_tool_operations() (+35 more)

### Community 34 - "App.tsx"
Cohesion: 0.07
Nodes (35): App(), AppRoutes(), BackendPolicyState, confirmAppClose(), CoworkView, CrewView, FeaturesView, hasRunningWork() (+27 more)

### Community 35 - "credential_store.rs"
Cohesion: 0.12
Nodes (26): account_id(), account_ids_are_stable_and_do_not_expose_locator_values(), CredentialBackend, CredentialLocator, CredentialReadResponse, CredentialSetRequest, CredentialStoreError, empty_values_delete_existing_credentials() (+18 more)

### Community 36 - "devDependencies"
Cohesion: 0.05
Nodes (41): devDependencies, autoprefixer, @axe-core/playwright, eslint, @eslint/js, eslint-plugin-react-hooks, jsdom, @playwright/test (+33 more)

### Community 37 - "openaiCompatibleClient.ts"
Cohesion: 0.08
Nodes (40): APIMessage, APIToolDef, blocksToUserContent(), buildEndpoint(), buildModelsEndpoint(), createAbortSignal(), extractReasoningContent(), extractReasoningDetailText() (+32 more)

### Community 38 - "file_safety.rs"
Cohesion: 0.16
Nodes (38): allowed(), backup_root(), BackupEntry, canonicalize_for_policy(), copy_directory_recursive(), copy_path(), create_backup_path(), create_directory() (+30 more)

### Community 39 - "AuditEvent"
Cohesion: 0.14
Nodes (22): AuditService<S>, Into, ServiceResult, audit_event_serializes_with_snake_case_contract_fields(), AuditEvent, AuditId, AuditOutcome, AuditRiskClass (+14 more)

### Community 40 - "AppHandle"
Cohesion: 0.10
Nodes (38): aggregate_gateway_status(), build_local_gateway_subsystems(), check_audit_writable(), claude_code_send_stream(), document_render_preview(), enforce_file_tool_policy(), ensure_run_file_access(), execute_task() (+30 more)

### Community 41 - "terminal_sessions.rs"
Cohesion: 0.13
Nodes (37): close_terminal_session(), configure_shell_command(), create_terminal_session(), default_shell(), interrupt_terminal_session(), kill_terminal_session(), ManagedTerminalSession, pty_size() (+29 more)

### Community 42 - "dependencies"
Cohesion: 0.05
Nodes (37): dependencies, class-variance-authority, clsx, i18next, lucide-react, react, react-dom, react-i18next (+29 more)

### Community 43 - "i18n.ts"
Cohesion: 0.07
Nodes (23): CoworkQuickPrompts(), CoworkQuickPromptsProps, CrewExecutionLogRow, CrewExecutionResponse, CrewHistoryPanel(), CrewRunEventRow, CrewRunHistoryRow, formatTimestamp() (+15 more)

### Community 44 - "CrewRuntimeTaskTests"
Cohesion: 0.06
Nodes (6): CrewRuntimeComplexIntegrationTests, CrewRuntimeIntegrationTests, CrewRuntimeParallelIntegrationTests, CrewRuntimeStatusTests, CrewRuntimeTaskTests, CrewRuntimeToolTests

### Community 45 - "load_policy_state"
Cohesion: 0.09
Nodes (36): active_toolset_inference_detects_custom_edits(), backend_file_mutations_respect_the_active_toolset_policy(), build_policy_tool_states(), build_toolset_policy(), canonical_policy_tool_id(), crew_tool_allowed_by_flags(), default_policy_enabled_tool_ids_vec(), default_policy_flags() (+28 more)

### Community 46 - "network_safety.rs"
Cohesion: 0.13
Nodes (31): append_with_limit(), fetch_public_text(), is_allowed_text_content_type(), is_followable_redirect(), is_public_ipv4(), is_public_ipv6(), normalize_content_type(), origin_for_audit() (+23 more)

### Community 47 - "chatAttachments.ts"
Cohesion: 0.11
Nodes (33): allowFolderAttachments(), AttachmentPromptBuildResult, buildAttachmentPromptContext(), collectSnippets(), extractQueryTerms(), ExtractTextLimitedResponse, FsAttachmentMetadataEntry, FsAttachmentMetadataResponse (+25 more)

### Community 48 - "memorySystem.ts"
Cohesion: 0.09
Nodes (32): AutomaticMemoryCandidate, buildSystemPromptWithMemory(), captureAutomaticMemoryDraft(), compactConversation(), countCharacters(), COWORK_MEMORY_FILES, estimateConversationTokens(), estimateTokens() (+24 more)

### Community 49 - "EventEnvelope"
Cohesion: 0.14
Nodes (17): event_envelope_serializes_with_contract_fields(), EventEnvelope, EventId, EventReplayMetadata, EventSequence, legacy_tauri_event_keeps_current_event_name(), noop_event_sink_accepts_envelopes(), DateTime (+9 more)

### Community 50 - "memory_engine.rs"
Cohesion: 0.16
Nodes (33): compact_low_confidence(), contains_invisible_control(), create_memory_snapshot(), curated_memory_requires_a_unique_substring_and_enforces_capacity(), curated_memory_supports_add_replace_remove_and_exact_deduplication(), database(), duplicate_check_only_rejects_the_same_content(), find_unique_match() (+25 more)

### Community 51 - "office_integration.rs"
Cohesion: 0.18
Nodes (32): detect_app(), detect_app_for_kind(), detect_office_apps(), document_format(), DocumentPreviewRequest, DocumentPreviewResponse, export_office_to_pdf(), find_in_path() (+24 more)

### Community 52 - "CrewLiveMonitor.tsx"
Cohesion: 0.11
Nodes (30): AgentStream, buildAgentStreams(), buildEntryMeta(), buildRollingWindowLines(), CATEGORY_LABELS, createEmptyCounts(), CrewLiveDisplayCategory, CrewLiveFilter (+22 more)

### Community 53 - "memoryStore.ts"
Cohesion: 0.09
Nodes (23): formatDateTime(), getTabLabel(), MemoryPanel(), MemoryTab, randomId(), safeInvokeMock, truncateText(), buildKnowledgeImportEntries() (+15 more)

### Community 54 - "resources"
Cohesion: 0.06
Nodes (33): app, security, trayIcon, windows, build, beforeBuildCommand, beforeDevCommand, devUrl (+25 more)

### Community 55 - "SettingsView.tsx"
Cohesion: 0.09
Nodes (27): ConnectorPanel(), ProcessPanel(), CATEGORIES, CategoryKey, EMPTY_GATEWAY_HEALTH, EMPTY_STARTUP_RECOVERY, GatewayHealth, GatewaySubsystem (+19 more)

### Community 56 - "sensitive_data.rs"
Cohesion: 0.13
Nodes (23): bounded_json_stays_valid_and_redacted(), bounds_utf8_text_without_splitting_characters(), diagnostic_label(), is_sensitive_key(), normalized_key(), recursively_redacts_sensitive_keys_and_nested_environment_maps(), redact_and_bound_json_text(), redact_and_bound_optional_json() (+15 more)

### Community 57 - "personalityStore.ts"
Cohesion: 0.10
Nodes (25): DEFAULT_PERSONALITY_ICONS, EMPTY_FORM, formatRoleLabel(), PersonalityEditor(), PersonalityForm, PersonalitySelector(), randomId(), ROLE_OPTIONS (+17 more)

### Community 58 - "chatProvider.ts"
Cohesion: 0.11
Nodes (26): detectModelCapabilities(), DefaultLlmProfileIds, CHAT_PROVIDER_LABELS, CHAT_PROVIDER_OPTIONS, ChatProviderContext, ChatProviderKind, ChatProviderState, collectProviderModels() (+18 more)

### Community 59 - "claude_code_bridge.rs"
Cohesion: 0.17
Nodes (25): ClaudeCodeBridge, ClaudeCodeCommandInfo, ClaudeCodeConfig, ClaudeCodeProcess, ClaudeCodeResponse, ClaudeCodeStatus, ClaudeCodeStreamChunk, ClaudeCodeToolInfo (+17 more)

### Community 60 - "logStore.ts"
Cohesion: 0.12
Nodes (22): queryClient, isSensitiveKey(), normalizeKey(), redactAtDepth(), redactRecord(), redactSensitiveData(), redactText(), redactCrewExecutionLog() (+14 more)

### Community 61 - "scripts"
Cohesion: 0.07
Nodes (29): scripts, brand:assets, budgets:build, build, dev, dev:tauri, doctor, doctor:ci (+21 more)

### Community 62 - "projectStore.ts"
Cohesion: 0.11
Nodes (27): AddProjectResourceInput, addUniqueResources(), DbProject, DbProjectResource, DeleteProjectOptions, generateId(), getEnabledProjectAttachments(), getEnabledProjectLinks() (+19 more)

### Community 63 - "insightsStore.ts"
Cohesion: 0.13
Nodes (24): formatDateTime(), getLocale(), InsightsPanel(), MetricTone, invokeMock, addLocalEvent(), asArray(), asNullableString() (+16 more)

### Community 64 - "mcp_call_tool"
Cohesion: 0.11
Nodes (28): audit_event(), capture_screenshot_for_display_payload(), fs_watch_start(), fs_watch_stop(), local_docs_mcp_call(), local_screenshot_mcp_call(), mcp_call_tool(), parse_bool_tool_arg() (+20 more)

### Community 65 - "compilerOptions"
Cohesion: 0.07
Nodes (27): compilerOptions, allowImportingTsExtensions, baseUrl, erasableSyntaxOnly, ignoreDeprecations, jsx, lib, module (+19 more)

### Community 66 - "index.ts"
Cohesion: 0.21
Nodes (16): OllamaEngineConfig, applyToolResultBudget(), autoCompact(), createTokenBudget(), estimateConversationTokens(), estimateTokens(), fallbackCompact(), generateToolUseSummary() (+8 more)

### Community 67 - "terminalStore.ts"
Cohesion: 0.11
Nodes (22): AiTerminalCommandResult, appendOutput(), BackendExecResponse, completePendingAiCommand(), CreateSessionInput, findSession(), getAllSessions(), handleTerminalExit() (+14 more)

### Community 68 - "crew_provider_health_check"
Cohesion: 0.13
Nodes (26): apply_provider_headers(), build_provider_chat_urls(), build_provider_model_urls(), connector_test_reachability(), ConnectorReachabilityRequest, ConnectorReachabilityResponse, crew_provider_health_check(), crew_provider_models_list() (+18 more)

### Community 69 - "registry.ts"
Cohesion: 0.09
Nodes (24): agentsCommand, clearCommand, commandRegistry, compactCommand, costCommand, cwdCommand, debugCommand, executeCommand() (+16 more)

### Community 70 - "_safe_result"
Cohesion: 0.11
Nodes (14): BashTool, CopyPathTool, EditFileTool, GlobTool, GrepTool, MovePathTool, OfficeWorkflowTool, ReadFileTool (+6 more)

### Community 71 - "i18n-audit.mjs"
Cohesion: 0.09
Nodes (18): collectTsFindings(), deKeys, enKeys, files, germanInEnglishResources, ignoredDirs, isTestFile(), missingInDe (+10 more)

### Community 72 - "messageDisplay.ts"
Cohesion: 0.16
Nodes (20): MessageStreamPanel(), MessageStreamPanelProps, MessageThinking(), MessageThinkingProps, MessageVerbose(), AssistantPresentationOptions, buildModelDebugContent(), escapeRegExp() (+12 more)

### Community 73 - "pipelineStore.ts"
Cohesion: 0.13
Nodes (18): ActiveTab, PipelinePanel(), OllamaConfig, BackendPipelineExecutionResult, getLocalPipelines(), hasString(), isRecord(), normalizeRpcPipeline() (+10 more)

### Community 74 - "lifecycle.ts"
Cohesion: 0.09
Nodes (22): ApprovalResolution, McpServerInstatce, McpServerStatus, McpTratsportKind, PluginMatifest, providerAdapter, providerCapability, providerHealth (+14 more)

### Community 75 - "globalSearch.ts"
Cohesion: 0.12
Nodes (18): ChatThread, Project, buildSearchIndex(), BuildSearchIndexInput, compact(), filterSearchIndex(), getTaskSearchTitle(), normalize() (+10 more)

### Community 76 - "RequestContext"
Cohesion: 0.16
Nodes (22): NoopAuditSink, RequestContext, DateTime, Utc, EventSink, NoopEventSink, Send, Sync (+14 more)

### Community 77 - "chatStore.ts"
Cohesion: 0.11
Nodes (15): AskQuestionOption, ChatMessage, ChatState, CrewLiveSeverity, DbMessage, isTauriRuntime(), LiveToolCallStatus, loadedThreadMessages (+7 more)

### Community 78 - "workTasksStore.ts"
Cohesion: 0.15
Nodes (20): BackendWorkTask, mapBackendWorkTask(), mergeTaskPatch(), migrateLegacyStorageToSqlite(), normalizeRunner(), normalizeStatus(), normalizeTask(), optionalString() (+12 more)

### Community 79 - "crew_tools.py"
Cohesion: 0.16
Nodes (19): _agent_access(), BashInput, build_runtime_tools(), _canonical_tool_id(), EditFileInput, GlobInput, GrepInput, OfficeWorkflowInput (+11 more)

### Community 80 - "ollamaStreaming.ts"
Cohesion: 0.16
Nodes (21): buildChatPrompt(), buildResponse(), callOllamaGenerate(), canUseTauriInvoke(), ChatTurnRequest, ChatTurnResponse, createStreamId(), detectRiskyAction() (+13 more)

### Community 81 - "compilerOptions"
Cohesion: 0.10
Nodes (20): compilerOptions, allowImportingTsExtensions, erasableSyntaxOnly, lib, module, moduleDetection, moduleResolution, noEmit (+12 more)

### Community 82 - "check-budgets.mjs"
Cohesion: 0.10
Nodes (16): assets, assetsDir, budgets, cssAssets, cssGzipBytes, distDir, indexHtml, indexHtmlPath (+8 more)

### Community 83 - "useTerminalStore"
Cohesion: 0.14
Nodes (14): getSessionLabel(), getStatusLabel(), TerminalDock(), TerminalDockProps, xtermInstances, randomId(), TerminalPanel(), invokeMock (+6 more)

### Community 84 - "SkillPanel.tsx"
Cohesion: 0.15
Nodes (13): emptyForm, formatDateTime(), getRunModeLabel(), getTabLabel(), randomId(), runModeLabels, SkillPanel(), SkillTab (+5 more)

### Community 85 - "_BingParser"
Cohesion: 0.12
Nodes (4): _BingParser, _DuckDuckGoParser, HTMLParser, _TextExtractor

### Community 86 - "worker_sandbox.rs"
Cohesion: 0.26
Nodes (17): copy_dir_recursive(), CopyStats, destroy_workspace_snapshot(), prepare_workspace_snapshot(), HashSet, Path, PathBuf, Result (+9 more)

### Community 87 - "next_run_from_expression"
Cohesion: 0.24
Nodes (17): next_daily(), next_run_from_expression(), next_weekday(), normalize(), parse_interval_duration(), parse_time(), parse_weekday(), parses_daily_expression() (+9 more)

### Community 88 - "agentCoordinator.ts"
Cohesion: 0.16
Nodes (7): AgentCoordinator, AgentInstance, appendAgentRunEvent(), DEFAULT_AGENTS, stringifyRunPayload(), EngineConfig, EngineEvent

### Community 89 - "engineStore.test.ts"
Cohesion: 0.13
Nodes (9): autoSaveSessionMock, buildSystemPromptWithMemoryMock, captureAutomaticMemoryDraftMock, FakeQueryEngine, invokeMock, loadFrozenMemorySnapshotMock, loadSessionMock, queryBarriers (+1 more)

### Community 90 - "insights.rs"
Cohesion: 0.25
Nodes (14): build_summary(), CategoryCount, EventSummary, InsightsEventRequest, InsightsQueryRequest, InsightsSummary, record_event(), Arc (+6 more)

### Community 91 - "duckduckgo-websearch-server.mjs"
Cohesion: 0.22
Nodes (11): decodeHtmlEntities(), DEFAULTS, extractTargetUrl(), formatTextResult(), normalizeSafeSearch(), parseResultsFromHtml(), rl, safeSearchToKp() (+3 more)

### Community 92 - "ProjectView.tsx"
Cohesion: 0.23
Nodes (11): formatDate(), getProjectTitleForThread(), ProjectView(), readDraggedThreadId(), navigateMock, getProjectForThread(), extractFileAttachmentsFromFileList(), extractFileAttachmentsFromUriList() (+3 more)

### Community 93 - "documentWorkspaceStore.ts"
Cohesion: 0.20
Nodes (13): ArtifactVersionResponse, createDocument(), DocumentPreviewPage, DocumentPreviewResponse, DocumentWorkspaceItem, DocumentWorkspaceState, inferFormat(), isDocumentWorkspacePath() (+5 more)

### Community 94 - "skill_engine.rs"
Cohesion: 0.32
Nodes (13): analyze_for_improvement(), analyze_for_skill_generation(), derive_skill_name(), match_skill_for_input(), Arc, Option, String, simple_pattern_match() (+5 more)

### Community 95 - "doctor.mjs"
Cohesion: 0.21
Nodes (10): addCheck(), checks, ciMode, commandVersion(), missingOptional, missingRequired, npmInvocation, root (+2 more)

### Community 96 - ".insert_engine_run_event_with_details"
Cohesion: 0.23
Nodes (6): audit_event_insert(), diagnostic_database_sinks_redact_before_persistence(), engine_event_retention_keeps_the_latest_bounded_window(), engine_run_artifacts_round_trip_and_cascade(), engine_run_events_are_ordered_and_summarized(), insert_test_engine_run()

### Community 97 - "webSearchSources.ts"
Cohesion: 0.35
Nodes (9): HighlightedChatText(), HighlightedChatTextProps, appendWebSearchSources(), extractWebSearchSources(), formatWebSearchSourcesBlock(), mergeWebSearchSources(), normalizeUrl(), parseWebSearchSourcesFromToolResult() (+1 more)

### Community 98 - "McpView.tsx"
Cohesion: 0.23
Nodes (11): ClaudeMcpServer, exampleJson(), McpCallResponse, McpProbeResponse, McpRuntimeServerStatus, McpTool, McpView(), parseEnv() (+3 more)

### Community 99 - "build.rs"
Cohesion: 0.36
Nodes (11): cargo_home(), is_dir(), main(), prepare_webview2_loader(), push_build_candidates(), push_registry_candidates(), Option, Path (+3 more)

### Community 100 - "permissions"
Cohesion: 0.17
Nodes (11): description, identifier, permissions, $schema, windows, core:default, core:window:allow-destroy, dialog:default (+3 more)

### Community 101 - ".get_engine_run"
Cohesion: 0.18
Nodes (9): EngineRunArtifactRow, EngineRunEventRow, EngineRunRow, InsightsEventRow, map_engine_run_artifact_row(), map_engine_run_event_row(), map_engine_run_row(), map_insights_row() (+1 more)

### Community 102 - "LocalAI Cowork"
Cohesion: 0.17
Nodes (12): Contributing, Current Scope, Documentation, Highlights, License And Disclaimer, LocalAI Cowork, MCP Example, Ollama Setup (+4 more)

### Community 103 - "scan-secrets.mjs"
Cohesion: 0.29
Nodes (11): allowedPrivacyMatch(), allowedSecretMatch(), entropy(), HISTORY, isText(), lineNumber(), privacyRules, runGit() (+3 more)

### Community 104 - "PageParser"
Cohesion: 0.27
Nodes (7): local_target(), main(), PageParser, png_dimensions(), HTMLParser, Path, validate_html()

### Community 107 - "package.json"
Cohesion: 0.20
Nodes (9): description, license, name, private, repository, type, url, type (+1 more)

### Community 108 - "sign-windows-installer.ps1"
Cohesion: 0.31
Nodes (6): Assert-CodeSigningCertificate(), Assert-InstallerSignature(), ConvertTo-ProcessArgument(), Invoke-BoundedProcess(), Normalize-Thumbprint(), Test-CodeSigningEku()

### Community 109 - "Ollama Konfiguration"
Cohesion: 0.20
Nodes (10): 1. Endpoint nicht erreichbar, 2. Modell nicht vorhanden, 3. Timeouts, API-Endpunkte, Beispiel: lokaler Start von Ollama, Fehlerbilder und Diagnose, Konfigurationsquellen, Ollama Konfiguration (+2 more)

### Community 110 - "ui-quality.spec.ts"
Cohesion: 0.22
Nodes (5): PRODUCT_SURFACES, ProductSurface, ProductTheme, THEMES, VIEWPORTS

### Community 111 - "LocalAI Cowork App"
Cohesion: 0.22
Nodes (8): Checks, Important Paths, Installer, License, Local Development, LocalAI Cowork App, Notes, Stack

### Community 112 - "WorkTaskRow"
Cohesion: 0.28
Nodes (3): delete_work_task_removes_matching_schedule(), work_task_lifecycle_round_trip(), WorkTaskRow

### Community 113 - "followUpPrompt.ts"
Cohesion: 0.42
Nodes (7): buildClarificationContinuationPrompt(), ClarificationContext, CLARIFYING_QUESTION_PATTERNS, FollowUpPromptMessage, inferClarificationContext(), isLikelyClarifyingQuestion(), isLikelyShortFollowUpAnswer()

### Community 114 - "ChatTurnRequest"
Cohesion: 0.29
Nodes (8): chat_turn(), chat_turn_stream(), chat_turn_stream_cancel(), ChatStreamRegistry, ChatTurnRequest, HashSet, ChatMessage, ChatTurnResponse

### Community 115 - "generate-brand-assets.mjs"
Cohesion: 0.29
Nodes (6): appRoot, faviconSvg, iconDir, masterPath, repoRoot, webOnly

### Community 116 - "createDesktopscreenshotAttachment"
Cohesion: 0.33
Nodes (7): captureDesktopVerificationAttachment(), createDesktopscreenshotAttachment(), createMcpscreenshotAttachment(), createToolStreamId(), formatDesktopscreenshotSummary(), parseBase64DataUrl(), parseMcpscreenshotPayload()

### Community 118 - "Contributing"
Cohesion: 0.29
Nodes (7): Before You Start, Contributing, Contribution License, Development Setup, Pull Request Checklist, Security And Privacy, Validation

### Community 119 - "Desktop-Steuerung und Computer Use"
Cohesion: 0.29
Nodes (7): 1. Lokale Desktop-Tools fuer Gemma/Ollama, 2. Screenshot-MCP, 3. `ComputerUseAppTest`, Bekannte Namensfalle, Desktop-Steuerung und Computer Use, Uebersicht, Wann welchen Pfad nutzen?

### Community 120 - "Security Policy"
Cohesion: 0.29
Nodes (6): Public Security Improvements, Report A Vulnerability, Security Design Notes, Security Policy, Supported Versions, Vulnerability Exceptions

### Community 123 - "Support"
Cohesion: 0.33
Nodes (5): Before Opening An Issue, Safe Diagnostic Sharing, Scope, Support, Where To Ask

### Community 126 - "pull_request_template.md"
Cohesion: 0.40
Nodes (4): Screenshots, Security and privacy, Validation, What changed

### Community 127 - "Q: warum wird mein Opencowork nicht bei einer Google suche angezigt?"
Cohesion: 0.40
Nodes (4): Answer, Outcome, Q: warum wird mein Opencowork nicht bei einer Google suche angezigt?, Source Nodes

### Community 128 - "Q: Where are credentials and private data handled, and what repository surfaces control SEO, releases, and community reporting?"
Cohesion: 0.40
Nodes (4): Answer, Outcome, Q: Where are credentials and private data handled, and what repository surfaces control SEO, releases, and community reporting?, Source Nodes

### Community 129 - "Q: Überlege dir ein neues Logo für Opencowork"
Cohesion: 0.40
Nodes (4): Answer, Outcome, Q: Überlege dir ein neues Logo für Opencowork, Source Nodes

### Community 130 - "openaiCompatibleClient.test.ts"
Cohesion: 0.50
Nodes (3): hasTauriRuntimeMock, readToolDef, safeInvokeMock

### Community 132 - "_fetch_public_text"
Cohesion: 0.67
Nodes (3): _fetch_public_text(), _SafeRedirectHandler, _validate_public_url()

### Community 133 - "Quick Start"
Cohesion: 0.50
Nodes (4): Build A Windows Installer, Prerequisites, Quick Start, Run The App In Development

### Community 136 - "resolveShellNavigationTarget"
Cohesion: 0.67
Nodes (3): inferShellCwdFromCommand(), normalizeShellPath(), resolveShellNavigationTarget()

## Knowledge Gaps
- **767 isolated node(s):** `ProductSurface`, `ProductTheme`, `PRODUCT_SURFACES`, `VIEWPORTS`, `THEMES` (+762 more)
  These have ≤1 connection - possible missing edges or undocumented components.
- **22 thin communities (<3 nodes) omitted from report** — run `graphify query` to explore isolated nodes.

## Suggested Questions
_Questions this graph is uniquely positioned to answer:_

- **Why does `Database` connect `Database` to `Result`, `terminal_backends.rs`, `lib.rs`, `String`, `String`, `db.rs`, `Option`, `collect_crew_governance_payload`, `AppHandle`, `load_policy_state`, `memory_engine.rs`, `sensitive_data.rs`, `mcp_call_tool`, `RequestContext`, `insights.rs`, `skill_engine.rs`, `.insert_engine_run_event_with_details`, `.get_engine_run`, `WorkTaskRow`, `startup_recovery_reconciles_active_state_after_reopen_and_is_idempotent`, `.list_artifact_exports`?**
  _High betweenness centrality (0.107) - this node is a cross-community bridge._
- **Why does `TerminalSessionRegistry` connect `terminal_sessions.rs` to `Result`, `AppHandle`, `lib.rs`?**
  _High betweenness centrality (0.017) - this node is a cross-community bridge._
- **What connects `ProductSurface`, `ProductTheme`, `PRODUCT_SURFACES` to the rest of the system?**
  _767 weakly-connected nodes found - possible documentation gaps or missing edges._
- **Should `Result` be split into smaller, more focused modules?**
  _Cohesion score 0.0375366568914956 - nodes in this community are weakly interconnected._
- **Should `audit_service.rs` be split into smaller, more focused modules?**
  _Cohesion score 0.05309734513274336 - nodes in this community are weakly interconnected._
- **Should `crewStore.ts` be split into smaller, more focused modules?**
  _Cohesion score 0.03371993127147766 - nodes in this community are weakly interconnected._
- **Should `Database` be split into smaller, more focused modules?**
  _Cohesion score 0.053992221459620224 - nodes in this community are weakly interconnected._