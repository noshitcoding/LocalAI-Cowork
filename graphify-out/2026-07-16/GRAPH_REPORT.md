# Graph Report - Open_Cowork  (2026-07-16)

## Corpus Check
- 322 files · ~333,832 words
- Verdict: corpus is large enough that graph structure adds value.

## Summary
- 5126 nodes · 12805 edges · 246 communities (213 shown, 33 thin omitted)
- Extraction: 99% EXTRACTED · 1% INFERRED · 0% AMBIGUOUS · INFERRED: 94 edges (avg confidence: 0.72)
- Token cost: 0 input · 0 output

## Graph Freshness
- Built from commit: `d7692e30`
- Run `git rev-parse HEAD` and compare to check if the graph is stale.
- Run `graphify update .` after code changes (no API cost).

## Community Hubs (Navigation)
- Result
- lib.rs
- crewStore.ts
- registry.ts
- String
- crew_python_bridge.rs
- CoworkView.tsx
- AppHandle
- App.tsx
- ollamaClient.ts
- Option
- tr
- index.ts
- Database
- index.ts
- ServiceError
- ollama.rs
- workTaskCrewRuntime.ts
- properties
- artifact_pipeline.rs
- useConfigStore
- safeInvoke
- main.py
- Capability
- String
- coworkStore.ts
- configStore.ts
- context.rs
- cowork_features.rs
- properties
- chatStore.ts
- mcp.rs
- properties
- openaiCompatibleClient.ts
- engineStore.ts
- Option
- personalityStore.ts
- terminal_sessions.rs
- chatAttachments.ts
- CrewLiveMonitor.tsx
- EventEnvelope
- office_integration.rs
- LeftSidebar.tsx
- toolOrchestrator.ts
- Vec
- claude_code_bridge.rs
- properties
- properties
- AuditEvent
- file_safety.rs
- properties
- Message
- audit_service.rs
- projectStore.ts
- db.rs
- validate-product-docs.mjs
- insightsStore.ts
- crew_provider_health_check
- tauri.conf.json
- terminalStore.ts
- properties
- registry.ts
- RunPanel.tsx
- QueryEngine
- .open_in_memory
- validate-agent-discipline.mjs
- devDependencies
- i18n-audit.mjs
- messageDisplay.ts
- globalSearch.ts
- RequestContext
- compilerOptions
- Feature 2: Zusammenklapp-Buttons für Task-Chats
- lifecycle.ts
- ollamaStreaming.ts
- UI-Anforderungsliste fuer Open_Cowork
- memoryStore.ts
- workTasksStore.ts
- terminal_backends.rs
- INDEX.md
- 5. Anforderungen pro Ansicht
- check-budgets.mjs
- 5.5 Settings-Ansicht
- memory_engine.rs
- dependencies
- SkillPanel.tsx
- useTerminalStore
- chatProvider.ts
- load_policy_state
- asString
- next_run_from_expression
- Vec
- compilerOptions
- Implementation Status: ✅ COMPLETE
- Changes Made
- scripts
- skill_engine.rs
- process_manager.rs
- Fix for Per-Chat/Per-Task Permission Mode and Allowed Directories
- 5.3 Plan-, Approval- und Task-UI
- Feature Implementierungsvergleich
- Open Cowork
- insights.rs
- Current Architecture
- FEHLENDE_FEATURES_IM_CODE.md
- duckduckgo-websearch-server.mjs
- documentWorkspaceStore.ts
- gateway_health
- worker_sandbox.rs
- modelCapabilities.ts
- WorkTaskRow
- TerminalDock.tsx
- README.md
- Decisions
- workTasksStore.ts
- build.rs
- Architektur
- Decisions
- Ollama Konfiguration
- generateIndex
- Desktop Smoke Test
- Decisions
- main.rs
- parseBlock
- package.json
- Open Cowork App
- doctor.mjs
- startup_recovery_reconciles_active_state_after_reopen_and_is_idempotent
- .list_artifact_exports
- Runtime Compatibility
- .list_steps
- Contributing
- Product Documentation Index
- useTerminalStore
- .search_memory_entries
- Decisions
- 6. Querschnittliche UI-Anforderungen
- desktop-smoke.mjs
- default.json
- queryEngine.approval.test.ts
- Claude-Code Feature Adoption (Open_Cowork)
- 3. Gestaltungsprinzipien
- uniqueStrings
- CreateDirectoryTool
- Current Routes
- 4. Informationsarchitektur
- Copilot Project Instructions
- Memory And Skill Discipline
- Skills Index
- openaiCompatibleClient.test.ts
- skill-and-memory.instructions.md
- Agent Memory Test Tasks
- ollamaClient.test.ts
- _fetch_public_text
- McpView.tsx
- ollamaStreaming.test.ts
- tsconfig.json
- refactor-settings.js
- refactor-ui.js
- .get_engine_run
- AGENTS.md
- registry.filesystem.test.ts
- documentWorkspaceStore.test.ts
- gateway_health
- agent-memory-notes-example.md
- duckduckgo-websearch-server.mjs
- Draft Knowledge Base
- credentialPersistence.ts
- createDesktopscreenshotAttachment
- ui-quality.spec.ts
- verify.mjs
- sensitive_data.rs
- gateway_health
- webSearchSources.ts
- Audit Integrity and Threat Model Contract
- Hermes Memory and Command Adoption
- 6. Querschnittliche UI-Anforderungen
- AnthropicAPIError
- Release Supply Chain Contract
- getToolDefinitions
- .insert_engine_run_event_with_details
- 7. Desktop- und Fensterverhalten
- McpView.tsx
- 8. Accessibility
- 5.2 Cowork-Hauptansicht
- 5.3 Plan-, Approval- und Task-UI
- 7. Desktop- und Fensterverhalten
- 9. Visuelle und textliche Anforderungen
- WorkTaskRow
- resolveShellNavigationTarget
- main.rs
- normalizeDesktopCoordinateSpace
- CrewGovernancePanel.tsx
- App.test.tsx
- Error
- Self
- AppHandle
- globalSearch.ts
- taskStore.ts
- Item
- messageDisplay.ts
- State
- StatusCode
- T
- CrewApprovalRow
- chatStore.ts
- CrewDefinitionRow
- CrewDefinitionVersionRow
- CrewExecutionLogRow
- CrewRoleBindingRow
- EngineRunArtifactRow
- EngineRunCheckpointRow
- modelCapabilities.ts
- OfficeWorkflowResponse
- RuntimeInstructionRow
- WorkerSandboxRow
- CoworkContextRail.tsx
- documentWorkspaceStore.ts
- gateway_health
- engineRunRecords.ts
- webSearchSources.ts
- claudeBridge.ts
- queryEngine.approval.test.ts
- CrewRuntimeToolTests
- scan-secrets.mjs
- SettingsView.test.tsx
- test_crew_runtime.py
- followUpPrompt.ts
- .list_steps
- windowState.ts
- Security Policy
- Support
- 5.5 Settings-Ansicht
- registerAllBuiltinTools

## God Nodes (most connected - your core abstractions)
1. `Database` - 369 edges
2. `tr()` - 111 edges
3. `CoworkView()` - 80 edges
4. `safeInvoke()` - 61 edges
5. `useConfigStore` - 47 edges
6. `CredentialStore` - 37 edges
7. `TasksView()` - 37 edges
8. `QueryEngine` - 36 edges
9. `hasTauriRuntime()` - 36 edges
10. `ServiceError` - 33 edges

## Surprising Connections (you probably didn't know these)
- `FeaturesView()` --indirect_call--> `command()`  [INFERRED]
  app/src/components/FeaturesView.tsx → app/scripts/supply-chain.mjs
- `TerminalDock()` --indirect_call--> `command()`  [INFERRED]
  app/src/components/TerminalDock.tsx → app/scripts/supply-chain.mjs
- `deterministic_office_fallback()` --calls--> `OfficeWorkflowTool`  [INFERRED]
  app/src-tauri/python/crew_runtime/main.py → app/src-tauri/python/crew_runtime/crew_tools.py
- `LiveCapture` --uses--> `OfficeWorkflowTool`  [INFERRED]
  app/src-tauri/python/crew_runtime/main.py → app/src-tauri/python/crew_runtime/crew_tools.py
- `build_agent()` --calls--> `build_runtime_tools()`  [INFERRED]
  app/src-tauri/python/crew_runtime/main.py → app/src-tauri/python/crew_runtime/crew_tools.py

## Import Cycles
- None detected.

## Communities (246 total, 33 thin omitted)

### Community 0 - "Result"
Cohesion: 0.05
Nodes (152): CredentialStore, Default, Database, ArtifactExportRow, ArtifactVersionRow, authorize_worker_sandbox_source(), backend_delete(), backend_ensure_local() (+144 more)

### Community 1 - "lib.rs"
Cohesion: 0.08
Nodes (45): build_crew_memory_query(), build_effective_crew_tool_ids(), collect_crew_governance_payload(), collect_crew_memory_payload(), crew_agent_can_delegate(), crew_execute(), crew_role_allows_execution(), crew_role_allows_tool_operations() (+37 more)

### Community 2 - "crewStore.ts"
Cohesion: 0.11
Nodes (16): AskQuestionOption, ChatState, CrewLiveEntryCategory, CrewLiveSeverity, CrewLiveStatus, DbMessage, getActiveThread(), isTauriRuntime() (+8 more)

### Community 3 - "registry.ts"
Cohesion: 0.03
Nodes (66): agentTool, askUserTool, AskUserToolInput, bashTool, copyPathTool, createDirectoryTool, deleteFileTool, DesktopActionResponse (+58 more)

### Community 4 - "String"
Cohesion: 0.09
Nodes (27): ModelSwitcher(), DEFAULT_PERSONALITY_ICONS, EMPTY_FORM, formatRoleLabel(), PersonalityEditor(), PersonalityForm, PersonalitySelector(), randomId() (+19 more)

### Community 5 - "crew_python_bridge.rs"
Cohesion: 0.13
Nodes (58): build_status_from_json(), command_available(), crew_runtime_bootstrap(), crew_runtime_execute_request(), crew_runtime_status(), crew_runtime_status_internal(), crew_runtime_validate_definition(), CrewPythonBridge (+50 more)

### Community 6 - "CoworkView.tsx"
Cohesion: 0.07
Nodes (54): appendStoppedAssistantContent(), AskUserOption, AskUserPromptModel, buildChatExportPayload(), buildProjectInstructionsPromptContext(), buildProjectLinkPromptContext(), ChatExportFormat, clipVerboseText() (+46 more)

### Community 7 - "AppHandle"
Cohesion: 0.18
Nodes (18): OllamaEngineConfig, sampleOllamaMessage(), applyToolResultBudget(), autoCompact(), createTokenBudget(), estimateConversationTokens(), estimateTokens(), fallbackCompact() (+10 more)

### Community 8 - "App.tsx"
Cohesion: 0.04
Nodes (70): CrewRuntimePanel(), formatTimestamp(), hasTauriRuntimeMock, formatDateTime(), getTabLabel(), MemoryPanel(), MemoryTab, randomId() (+62 more)

### Community 9 - "ollamaClient.ts"
Cohesion: 0.07
Nodes (53): blockContentToText(), buildOllamaChatRequest(), buildStartTagRegex(), buildToolLookup(), canDelayVisibleStream(), canonicalizeArgumentKey(), canUseTauriInvoke(), clipOllamaDebugText() (+45 more)

### Community 10 - "Option"
Cohesion: 0.06
Nodes (81): TaskCreatePanel(), TaskCreatePanelProps, TaskDetailPane(), TaskDetailPaneProps, TaskProjectContext, TaskListPane(), TaskListPaneProps, baseTask (+73 more)

### Community 12 - "index.ts"
Cohesion: 0.07
Nodes (42): ANTHROPIC_MODELS, AnthropicAPIError, AnthropicConfig, APIContentBlock, APIMessage, APIToolDef, calculateCost(), COST_PER_MILLION (+34 more)

### Community 13 - "Database"
Cohesion: 0.12
Nodes (26): account_id(), account_ids_are_stable_and_do_not_expose_locator_values(), CredentialBackend, CredentialLocator, CredentialReadResponse, CredentialSetRequest, CredentialStoreError, empty_values_delete_existing_credentials() (+18 more)

### Community 14 - "index.ts"
Cohesion: 0.08
Nodes (54): PermissionDecision, getAllTools(), accumulateUsage(), AgentToolProgress, AssistantMessage, AttachmentMessage, BashProgress, ContentBlockDelta (+46 more)

### Community 15 - "ServiceError"
Cohesion: 0.08
Nodes (25): api_error_response_matches_contract_shape(), ApiError, ApiErrorResponse, conflict_error_maps_to_http_conflict(), current_tauri_result_conversion_keeps_existing_string_boundary(), forbidden_error_maps_to_permission_denied(), internal_error_does_not_leak_source_to_safe_message_or_tauri_conversion(), into_tauri_result() (+17 more)

### Community 16 - "ollama.rs"
Cohesion: 0.13
Nodes (51): build_chat_messages(), build_chat_prompt(), build_chat_turn_response(), build_chat_turn_response_preserves_tool_calls(), build_http_client(), chat_turn(), chat_turn_stream(), chat_turn_with_tools() (+43 more)

### Community 17 - "workTaskCrewRuntime.ts"
Cohesion: 0.09
Nodes (28): AddProjectResourceInput, addUniqueResources(), DbProject, DbProjectResource, DeleteProjectOptions, generateId(), getEnabledProjectAttachments(), getEnabledProjectLinks() (+20 more)

### Community 18 - "properties"
Cohesion: 0.17
Nodes (12): items, type, properties, permissions, purpose, route, title, user_story (+4 more)

### Community 19 - "artifact_pipeline.rs"
Cohesion: 0.23
Nodes (37): ArtifactParseResponse, bind_pdfium(), extract_pdf_text_with_pdfium(), extract_text_for_llm(), extract_text_for_llm_limited(), parse_artifact(), parse_binary(), parse_csv() (+29 more)

### Community 20 - "useConfigStore"
Cohesion: 0.13
Nodes (21): TaskRunToolbar(), TaskRunToolbarProps, BackendWorkTask, mapBackendWorkTask(), mergeTaskPatch(), migrateLegacyStorageToSqlite(), normalizeRunner(), normalizeStatus() (+13 more)

### Community 21 - "safeInvoke"
Cohesion: 0.13
Nodes (31): append_with_limit(), fetch_public_text(), is_allowed_text_content_type(), is_followable_redirect(), is_public_ipv4(), is_public_ipv6(), normalize_content_type(), origin_for_audit() (+23 more)

### Community 22 - "main.py"
Cohesion: 0.07
Nodes (67): agent_display_name(), bridge_textual_tool_call(), build_agent(), build_artifact_repair_description(), build_governance_note(), build_llm(), build_memory_note(), build_task_description() (+59 more)

### Community 23 - "Capability"
Cohesion: 0.09
Nodes (32): Capability, capability_deserializes_from_policy_name(), capability_response_serializes_policy_names(), CapabilityCategory, CapabilityDescriptor, CapabilityResponse, CapabilityStatus, dangerous_capabilities_are_disabled_by_default_when_supported() (+24 more)

### Community 24 - "String"
Cohesion: 0.06
Nodes (15): CrewDefinitionVersionRow, CrewRunEventRow, EngineRunCheckpointRow, LearningOutcomeRow, ManagedProcessRow, MemoryProviderRow, PersonalityRow, ProjectRow (+7 more)

### Community 25 - "coworkStore.ts"
Cohesion: 0.06
Nodes (40): BackendConnectorTestResponse, BackendScheduledRunRow, BackendScheduledTaskRow, buildPolicySyncRequest(), CLAUDE_TOOL_CAPABILITIES, ClaudeToolCapability, ClaudeToolPreset, ConnectorConfig (+32 more)

### Community 26 - "configStore.ts"
Cohesion: 0.22
Nodes (16): ChatMessage, loadThreadMessagesFromDb(), clipText(), createFallbackUuid(), hydrateStoredMessage(), isChatRole(), parsePersistedSessionMessage(), parseStoredChatMessagePayload() (+8 more)

### Community 27 - "context.rs"
Cohesion: 0.09
Nodes (19): Actor, ActorId, ActorKind, ActorRole, anonymous_fixture_has_no_actor_id(), ClientPlatform, context_serializes_with_camel_case_fields(), local_default_context_needs_no_network_config() (+11 more)

### Community 28 - "cowork_features.rs"
Cohesion: 0.17
Nodes (42): analyze_single_path(), apply_office_template_transform(), ArtifactVersionExportInput, build_artifact_field_rows(), build_workflow_headers(), build_workflow_rows(), build_workflow_totals(), compute_numeric_totals() (+34 more)

### Community 29 - "properties"
Cohesion: 0.12
Nodes (17): type, type, type, type, properties, accessibility, confirmation, handler (+9 more)

### Community 30 - "chatStore.ts"
Cohesion: 0.09
Nodes (36): active_toolset_inference_detects_custom_edits(), backend_file_mutations_respect_the_active_toolset_policy(), build_policy_tool_states(), build_toolset_policy(), canonical_policy_tool_id(), crew_tool_allowed_by_flags(), default_policy_enabled_tool_ids_vec(), default_policy_flags() (+28 more)

### Community 31 - "mcp.rs"
Cohesion: 0.16
Nodes (43): mcp_probe(), call_tool(), format_call_result(), McpCallRequest, McpCallResponse, McpError, McpProbeResponse, McpRuntimeServerStatus (+35 more)

### Community 32 - "properties"
Cohesion: 0.14
Nodes (14): type, type, type, pattern, type, properties, accessibility, component (+6 more)

### Community 33 - "openaiCompatibleClient.ts"
Cohesion: 0.08
Nodes (40): APIMessage, APIToolDef, blocksToUserContent(), buildEndpoint(), buildModelsEndpoint(), createAbortSignal(), extractReasoningContent(), extractReasoningDetailText() (+32 more)

### Community 34 - "engineStore.ts"
Cohesion: 0.07
Nodes (31): sessionRecord, sessionSummary, getAllCommands(), EngineBackend, CrewTaskMessageParams, autoSaveSession(), DbSessionRow, deleteSession() (+23 more)

### Community 35 - "Option"
Cohesion: 0.05
Nodes (42): 10. Release-Gates, 11. Steuerung und Priorisierung, 12. Unmittelbar naechste Umsetzungsschritte, 1. Zielbild, 2. Verifizierter Ausgangspunkt und Fortschritt, 3. Produkt- und Architekturentscheidungen, 4. Ziel-Domaenen und neue Vertraege, 5. Themengebiete und Feature-Plan (+34 more)

### Community 36 - "personalityStore.ts"
Cohesion: 0.10
Nodes (16): audit_event_insert(), diagnostic_database_sinks_redact_before_persistence(), engine_event_retention_keeps_the_latest_bounded_window(), engine_run_artifacts_round_trip_and_cascade(), engine_run_events_are_ordered_and_summarized(), engine_runs_capture_gateway_metadata(), EngineRunArtifactRow, EngineRunEventRow (+8 more)

### Community 37 - "terminal_sessions.rs"
Cohesion: 0.13
Nodes (37): close_terminal_session(), configure_shell_command(), create_terminal_session(), default_shell(), interrupt_terminal_session(), kill_terminal_session(), ManagedTerminalSession, pty_size() (+29 more)

### Community 38 - "chatAttachments.ts"
Cohesion: 0.11
Nodes (14): BashTool, CopyPathTool, EditFileTool, GlobTool, GrepTool, MovePathTool, OfficeWorkflowTool, ReadFileTool (+6 more)

### Community 39 - "CrewLiveMonitor.tsx"
Cohesion: 0.11
Nodes (30): AgentStream, buildAgentStreams(), buildEntryMeta(), buildRollingWindowLines(), CATEGORY_LABELS, createEmptyCounts(), CrewLiveDisplayCategory, CrewLiveFilter (+22 more)

### Community 40 - "EventEnvelope"
Cohesion: 0.14
Nodes (17): event_envelope_serializes_with_contract_fields(), EventEnvelope, EventId, EventReplayMetadata, EventSequence, legacy_tauri_event_keeps_current_event_name(), noop_event_sink_accepts_envelopes(), DateTime (+9 more)

### Community 41 - "office_integration.rs"
Cohesion: 0.18
Nodes (32): detect_app(), detect_app_for_kind(), detect_office_apps(), document_format(), DocumentPreviewRequest, DocumentPreviewResponse, export_office_to_pdf(), find_in_path() (+24 more)

### Community 42 - "LeftSidebar.tsx"
Cohesion: 0.08
Nodes (41): WorkerSandboxRow, build_exec_command_text(), crew_runs_list(), CrewRunHistoryRow, db_save_task(), db_update_step(), db_update_thread_provider_settings(), emit_exec_chunk() (+33 more)

### Community 43 - "toolOrchestrator.ts"
Cohesion: 0.09
Nodes (23): agentsCommand, clearCommand, commandRegistry, compactCommand, costCommand, cwdCommand, debugCommand, executeCommand() (+15 more)

### Community 44 - "Vec"
Cohesion: 0.23
Nodes (4): CreateDirectoryTool, Path, _resolve_workspace_path(), _subprocess_environment()

### Community 45 - "claude_code_bridge.rs"
Cohesion: 0.17
Nodes (26): ClaudeCodeBridge, ClaudeCodeCommandInfo, ClaudeCodeConfig, ClaudeCodeProcess, ClaudeCodeResponse, ClaudeCodeStatus, ClaudeCodeStreamChunk, ClaudeCodeToolInfo (+18 more)

### Community 46 - "properties"
Cohesion: 0.05
Nodes (40): pattern, type, type, id, purpose, source_files, tests, title (+32 more)

### Community 47 - "properties"
Cohesion: 0.05
Nodes (40): items, type, items, type, pattern, type, pattern, type (+32 more)

### Community 48 - "AuditEvent"
Cohesion: 0.14
Nodes (22): AuditService<S>, Into, ServiceResult, audit_event_serializes_with_snake_case_contract_fields(), AuditEvent, AuditId, AuditOutcome, AuditRiskClass (+14 more)

### Community 49 - "file_safety.rs"
Cohesion: 0.16
Nodes (38): allowed(), backup_root(), BackupEntry, canonicalize_for_policy(), copy_directory_recursive(), copy_path(), create_backup_path(), create_directory() (+30 more)

### Community 50 - "properties"
Cohesion: 0.06
Nodes (39): items, type, type, items, type, pattern, type, type (+31 more)

### Community 51 - "Message"
Cohesion: 0.13
Nodes (22): _agent_access(), BashInput, build_runtime_tools(), _canonical_tool_id(), EditFileInput, _fetch_public_text(), GlobInput, GrepInput (+14 more)

### Community 52 - "audit_service.rs"
Cohesion: 0.18
Nodes (3): _BingParser, _DuckDuckGoParser, HTMLParser

### Community 53 - "projectStore.ts"
Cohesion: 0.06
Nodes (44): ExternalProviderHealthCheckResult, ExternalProviderModelsResult, isLlmProviderKind(), LlmProfilesPanel(), modelSuffix(), parseNumericInput(), ProfileHealthState, ProfileModelsState (+36 more)

### Community 54 - "db.rs"
Cohesion: 0.25
Nodes (6): local_target(), main(), PageParser, HTMLParser, Path, validate_html()

### Community 55 - "validate-product-docs.mjs"
Cohesion: 0.08
Nodes (23): appRoutes, appRoutesFile, catalog, CATALOG_KIND_CONFIG, catalogDir, catalogIndexPath, catalogMarkdown, docs (+15 more)

### Community 56 - "insightsStore.ts"
Cohesion: 0.13
Nodes (24): formatDateTime(), getLocale(), InsightsPanel(), MetricTone, invokeMock, addLocalEvent(), asArray(), asNullableString() (+16 more)

### Community 58 - "tauri.conf.json"
Cohesion: 0.06
Nodes (33): app, security, trayIcon, windows, build, beforeBuildCommand, beforeDevCommand, devUrl (+25 more)

### Community 59 - "terminalStore.ts"
Cohesion: 0.07
Nodes (33): getSessionLabel(), getStatusLabel(), TerminalDock(), TerminalDockProps, xtermInstances, createSession(), invokeMock, runAiCommandMock (+25 more)

### Community 60 - "properties"
Cohesion: 0.06
Nodes (34): type, type, type, pattern, type, accessibility, id, owner_element (+26 more)

### Community 61 - "registry.ts"
Cohesion: 0.08
Nodes (23): CoworkQuickPrompts(), CoworkQuickPromptsProps, CrewExecutionLogRow, CrewExecutionResponse, CrewHistoryPanel(), CrewRunEventRow, CrewRunHistoryRow, formatTimestamp() (+15 more)

### Community 62 - "RunPanel.tsx"
Cohesion: 0.09
Nodes (31): audit_event(), capture_screenshot_for_display_payload(), chat_turn_stream_cancel(), ChatStreamRegistry, fs_watch_list(), fs_watch_start(), fs_watch_stop(), local_docs_mcp_call() (+23 more)

### Community 63 - "QueryEngine"
Cohesion: 0.04
Nodes (89): apply_provider_headers(), build_provider_chat_urls(), build_provider_model_urls(), command_contains_path_traversal(), connector_test_reachability(), ConnectorReachabilityRequest, ConnectorReachabilityResponse, crew_provider_health_check() (+81 more)

### Community 65 - "validate-agent-discipline.mjs"
Cohesion: 0.08
Nodes (19): args, checkNotes, descLower, expandedPrompts, frontmatter, memoryPathArgIndex, missing, missingKeywords (+11 more)

### Community 66 - "devDependencies"
Cohesion: 0.05
Nodes (39): devDependencies, @axe-core/playwright, eslint, @eslint/js, eslint-plugin-react-hooks, eslint-plugin-react-refresh, globals, jsdom (+31 more)

### Community 67 - "i18n-audit.mjs"
Cohesion: 0.09
Nodes (18): collectTsFindings(), deKeys, enKeys, files, germanInEnglishResources, ignoredDirs, isTestFile(), missingInDe (+10 more)

### Community 68 - "messageDisplay.ts"
Cohesion: 0.06
Nodes (101): append_audit_event(), integrity_report(), Option, Path, PathBuf, Result, String, Value (+93 more)

### Community 69 - "globalSearch.ts"
Cohesion: 0.04
Nodes (10): CrewApprovalRow, CrewDefinitionRow, CrewRoleBindingRow, map_worker_sandbox_row(), MemoryEntryRow, Option, RuntimeInstructionRow, scheduled_run_claim_prevents_overlap_and_completion_updates_the_same_row() (+2 more)

### Community 70 - "RequestContext"
Cohesion: 0.13
Nodes (25): AuditSink, NoopAuditSink, Send, Sync, RequestContext, DateTime, Utc, EventSink (+17 more)

### Community 71 - "compilerOptions"
Cohesion: 0.07
Nodes (27): compilerOptions, allowImportingTsExtensions, baseUrl, erasableSyntaxOnly, ignoreDeprecations, jsx, lib, module (+19 more)

### Community 72 - "Feature 2: Zusammenklapp-Buttons für Task-Chats"
Cohesion: 0.09
Nodes (22): 1. Anforderungsliste (Requirements List), 2. Einzelne Tasks (Individual Tasks), 3. Qualitätskriterien (Quality Criteria), 4. Technische Hinweise, 5. Umsetzungsreihenfolge (Implementation Order), Anforderungen: Task-Chat Collapse & Projekt-Lösch-Button Entfernung, Betroffene Dateien, Chat-Typ „Task“ identifizieren (+14 more)

### Community 73 - "lifecycle.ts"
Cohesion: 0.10
Nodes (35): buildEngineUserInput(), allowFolderAttachments(), AttachmentPromptBuildResult, buildAttachmentPromptContext(), collectSnippets(), extractQueryTerms(), ExtractTextLimitedResponse, FsAttachmentMetadataEntry (+27 more)

### Community 74 - "ollamaStreaming.ts"
Cohesion: 0.16
Nodes (21): buildChatPrompt(), buildResponse(), callOllamaGenerate(), canUseTauriInvoke(), ChatTurnRequest, ChatTurnResponse, createStreamId(), detectRiskyAction() (+13 more)

### Community 75 - "UI-Anforderungsliste fuer Open_Cowork"
Cohesion: 0.22
Nodes (8): 10. Abnahmekriterien fuer die UI, 11. Priorisierung fuer Open_Cowork, 12. Bezug zu bestehender Dokumentation, 1. Zweck und Scope, 2. UI-Ziele, Muss fuer den naechsten UI-Ausbau, Soll nachziehen, UI-Anforderungsliste fuer Open_Cowork

### Community 76 - "memoryStore.ts"
Cohesion: 0.08
Nodes (36): claude_code_send_stream(), configure_pdfium_search_paths(), crew_approval_resolve(), CrewApprovalResolveRequest, document_render_preview(), enforce_file_tool_policy(), ensure_run_file_access(), execute_task() (+28 more)

### Community 77 - "workTasksStore.ts"
Cohesion: 0.14
Nodes (14): Backup und Recovery (Initialfassung), Build, CI-Gates (aktueller Stand), Development und Betrieb, Empfohlene Betriebsgrenzen (vorlaeufig), Fall A: `npm run tauri build` erzeugt keine EXE, Fall B: Ollama nicht erreichbar, Fall C: Build in CI rot (+6 more)

### Community 78 - "terminal_backends.rs"
Cohesion: 0.08
Nodes (62): attach_process_tree(), configure_process_tree(), ProcessTreeGuard, Child, Option, Result, Send, terminate_platform_tree() (+54 more)

### Community 79 - "INDEX.md"
Cohesion: 0.06
Nodes (16): Crew Task Import Control, Task Create Panel, Task Detail Pane, Task List Pane, Task Run Toolbar, Task Scheduler Panel, Crew Task Import Flow, WorkTask Run And Schedule Flow (+8 more)

### Community 80 - "5. Anforderungen pro Ansicht"
Cohesion: 0.17
Nodes (12): 5.1 Welcome Screen, 5.3 Plan-, Approval- und Task-UI, 5.4 Features-Ansicht, 5. Anforderungen pro Ansicht, UI-020 Einstieg und Orientierung, UI-021 Guided Onboarding, UI-040 Planansicht, UI-041 Approval-Interaktion (+4 more)

### Community 81 - "check-budgets.mjs"
Cohesion: 0.10
Nodes (16): assets, assetsDir, budgets, cssAssets, cssGzipBytes, distDir, indexHtml, indexHtmlPath (+8 more)

### Community 82 - "5.5 Settings-Ansicht"
Cohesion: 0.16
Nodes (17): queryClient, isSensitiveKey(), normalizeKey(), redactAtDepth(), redactRecord(), redactSensitiveData(), redactText(), redactCrewExecutionLog() (+9 more)

### Community 83 - "memory_engine.rs"
Cohesion: 0.17
Nodes (32): compact_low_confidence(), contains_invisible_control(), create_memory_snapshot(), curated_memory_requires_a_unique_substring_and_enforces_capacity(), curated_memory_supports_add_replace_remove_and_exact_deduplication(), database(), duplicate_check_only_rejects_the_same_content(), find_unique_match() (+24 more)

### Community 84 - "dependencies"
Cohesion: 0.05
Nodes (37): dependencies, class-variance-authority, clsx, i18next, lucide-react, react, react-dom, react-i18next (+29 more)

### Community 85 - "SkillPanel.tsx"
Cohesion: 0.23
Nodes (20): delete_reference(), deleting_a_reference_removes_its_resolvable_value(), failed_commit_preserves_previous_revision(), is_reference(), locator(), parse_reference(), prepare(), PreparedSecureConfig (+12 more)

### Community 86 - "useTerminalStore"
Cohesion: 0.40
Nodes (4): Screenshots, Security and privacy, Validation, What changed

### Community 87 - "chatProvider.ts"
Cohesion: 0.11
Nodes (43): argumentValue(), assertRegularAsset(), assertTauriVersionCompatibility(), assertVersionConsistency(), cargoPurl(), cargoRustVersion(), cargoVersion(), collectInventory() (+35 more)

### Community 88 - "load_policy_state"
Cohesion: 0.12
Nodes (16): accessibility, id, owner_element, tests, required, confirmation, destructive, disabled_when (+8 more)

### Community 89 - "asString"
Cohesion: 0.19
Nodes (19): asString(), comparePaths(), createYamlParser(), duplicateRouteRegistryValues(), extractCatalogRecords(), extractStringProperty(), listYamlFiles(), loadCatalogSchemas() (+11 more)

### Community 90 - "next_run_from_expression"
Cohesion: 0.24
Nodes (17): next_daily(), next_run_from_expression(), next_weekday(), normalize(), parse_interval_duration(), parse_time(), parse_weekday(), parses_daily_expression() (+9 more)

### Community 91 - "Vec"
Cohesion: 0.04
Nodes (59): url, buildDefaultCrewName(), CREW_STARTER_PRESETS, CrewPanel(), CrewProviderModelsResult, downloadCrewJson(), getCrewDiagnostics(), getProviderKey() (+51 more)

### Community 92 - "compilerOptions"
Cohesion: 0.10
Nodes (20): compilerOptions, allowImportingTsExtensions, erasableSyntaxOnly, lib, module, moduleDetection, moduleResolution, noEmit (+12 more)

### Community 93 - "Implementation Status: ✅ COMPLETE"
Cohesion: 0.11
Nodes (17): 1. Type System, 2. Store Layer, 3. Engine Layer, 4. Database Layer, 5. UI Layer, 6. Tests, Conclusion, Files Modified (10 files) (+9 more)

### Community 94 - "Changes Made"
Cohesion: 0.11
Nodes (17): Backward Compatibility, Behavior, Changes Made, Core Types, Database, Engine Core, Files Modified, How to Use (+9 more)

### Community 95 - "scripts"
Cohesion: 0.07
Nodes (27): scripts, budgets:build, build, dev, dev:tauri, doctor, doctor:ci, i18n:audit (+19 more)

### Community 96 - "skill_engine.rs"
Cohesion: 0.32
Nodes (13): analyze_for_improvement(), analyze_for_skill_generation(), derive_skill_name(), match_skill_for_input(), Arc, Option, String, simple_pattern_match() (+5 more)

### Community 97 - "process_manager.rs"
Cohesion: 0.12
Nodes (16): id, purpose, route, source_files, tests, title, required, data_dependencies (+8 more)

### Community 98 - "Fix for Per-Chat/Per-Task Permission Mode and Allowed Directories"
Cohesion: 0.12
Nodes (16): 1. Type Definitions, 2. Store Updates, 3. Engine Core Updates, 4. Database Schema, 5. UI Components, 6. CSS, Benefits, Fallback Behavior (+8 more)

### Community 99 - "5.3 Plan-, Approval- und Task-UI"
Cohesion: 0.11
Nodes (29): add_column_if_missing(), configure_connection(), connection_pragmas_enforce_integrity_and_contention_policy(), corrupt_database_files_and_invalid_data_directories_are_rejected(), create_pre_migration_backup(), current_schema_version(), database_error(), database_test_dir() (+21 more)

### Community 100 - "Feature Implementierungsvergleich"
Cohesion: 0.12
Nodes (15): 1) CMD/Bash Ausfuehrung, 2) Screenshots / Computer-Use, 3) Word/PowerPoint Integration, Bewertungsbasis, Deep Dive: CMD, Screenshot, Word/PPT, Empfohlene naechste Entscheidung, Feature Implementierungsvergleich, Konkrete Uebernahme-Kandidaten (Priorisiert) (+7 more)

### Community 101 - "Open Cowork"
Cohesion: 0.12
Nodes (16): Build A Windows Installer, Contributing, Current Scope, Documentation, Highlights, License And Disclaimer, MCP Example, Ollama Setup (+8 more)

### Community 102 - "insights.rs"
Cohesion: 0.25
Nodes (14): build_summary(), CategoryCount, EventSummary, InsightsEventRequest, InsightsQueryRequest, InsightsSummary, record_event(), Arc (+6 more)

### Community 103 - "Current Architecture"
Cohesion: 0.12
Nodes (25): formatDate(), getProjectTitleForThread(), ProjectView(), readDraggedThreadId(), DefaultLlmProfileIds, getResolvedProvider(), getProjectForThread(), extractFileAttachmentsFromFileList() (+17 more)

### Community 104 - "FEHLENDE_FEATURES_IM_CODE.md"
Cohesion: 0.13
Nodes (14): 10) Security und Compliance (Kapitel 7.10, IDs 216-230), 11) Enterprise Controls, Monitoring, Betrieb (Kapitel 7.11, IDs 231-240), 12) UX Foundation und Produktivitaetsfeatures (Kapitel 7.1 UX, IDs 241-250), 13) Performance und Stabilitaet (Kapitel 7.1 Performance, IDs 251-254), 14) QA- und Security-Testtiefe (Anforderungsnah, ueber aktuellen Stand hinaus), 1) Plattform und Windows-Basis (Kapitel 7.1, IDs 1-25), 3) Task-Orchestrierung (fortgeschritten) und Personalisierung (Kapitel 7.3, IDs 41-70, 171-185), 4) Dateisystem und File Safety Layer (Kapitel 7.4, IDs 71-100) (+6 more)

### Community 105 - "duckduckgo-websearch-server.mjs"
Cohesion: 0.19
Nodes (19): asBoolean(), asNullableString(), asNumber(), asRecord(), asString(), asTimestampString(), EngineRunCheckpointRow, EngineRunRow (+11 more)

### Community 106 - "documentWorkspaceStore.ts"
Cohesion: 0.31
Nodes (10): buildProductDoc(), coversRouteInventory(), ELEMENT_COLLECTION_FIELDS, firstPresent(), firstStringFromFields(), normalizeElementList(), normalizeRoutes(), normalizeStringList() (+2 more)

### Community 107 - "gateway_health"
Cohesion: 0.13
Nodes (9): autoSaveSessionMock, buildSystemPromptWithMemoryMock, captureAutomaticMemoryDraftMock, FakeQueryEngine, invokeMock, loadFrozenMemorySnapshotMock, loadSessionMock, queryBarriers (+1 more)

### Community 108 - "worker_sandbox.rs"
Cohesion: 0.26
Nodes (17): copy_dir_recursive(), CopyStats, destroy_workspace_snapshot(), prepare_workspace_snapshot(), HashSet, Path, PathBuf, Result (+9 more)

### Community 109 - "modelCapabilities.ts"
Cohesion: 0.09
Nodes (27): normalizeSessions(), normalizeSessionSummary(), SessionLike, SessionSearchPanel(), toNumber(), CATEGORIES, CategoryKey, EMPTY_GATEWAY_HEALTH (+19 more)

### Community 110 - "WorkTaskRow"
Cohesion: 0.14
Nodes (14): accessibility, id, purpose, route, source_files, tests, required, buttons (+6 more)

### Community 111 - "TerminalDock.tsx"
Cohesion: 0.13
Nodes (12): Agent Runtime Boundary, Backend Boundary, Current Architecture, Frontend Boundary, Persistence Boundary, System Shape, Transitional Notes, Current Canonical Docs (+4 more)

### Community 112 - "README.md"
Cohesion: 0.16
Nodes (8): AgentCoordinator, AgentInstance, appendAgentRunEvent(), DEFAULT_AGENTS, stringifyRunPayload(), EngineConfig, EngineEvent, AgentDefinition

### Community 113 - "Decisions"
Cohesion: 0.15
Nodes (12): CD-001: Local-first is the default product model, CD-002: Projects organize context, not isolated workspaces, CD-003: Work tasks are separate from chat threads, CD-004: Scheduling persists executable snapshots, CD-005: Crew agents synchronize with personality profiles, CD-006: Tool execution is policy-governed, CD-007: Persistence is intentionally mixed today, CD-008: File, desktop, Office, PDF, shell, and MCP capabilities cross the Tauri boundary (+4 more)

### Community 115 - "build.rs"
Cohesion: 0.36
Nodes (11): cargo_home(), is_dir(), main(), prepare_webview2_loader(), push_build_candidates(), push_registry_candidates(), Option, Path (+3 more)

### Community 116 - "Architektur"
Cohesion: 0.13
Nodes (18): ActiveTab, PipelinePanel(), OllamaConfig, BackendPipelineExecutionResult, getLocalPipelines(), hasString(), isRecord(), normalizeRpcPipeline() (+10 more)

### Community 117 - "Decisions"
Cohesion: 0.17
Nodes (11): Current Foundation, Decisions, Design System Decisions, DS-001: Use existing CSS tokens first, DS-002: Desktop productivity shell is the base layout, DS-003: Icons come from `lucide-react`, DS-004: Cards and panels have different roles, DS-005: Theme and density are user preferences (+3 more)

### Community 118 - "Ollama Konfiguration"
Cohesion: 0.18
Nodes (10): 1. Endpoint nicht erreichbar, 2. Modell nicht vorhanden, 3. Timeouts, API-Endpunkte, Beispiel: lokaler Start von Ollama, Fehlerbilder und Diagnose, Konfigurationsquellen, Ollama Konfiguration (+2 more)

### Community 119 - "generateIndex"
Cohesion: 0.18
Nodes (11): addGroup(), buildDuplicateRows(), buildMissingHints(), catalogKindTitle(), countDocs(), formatDocElements(), generateCatalogMarkdown(), generateIndex() (+3 more)

### Community 120 - "Desktop Smoke Test"
Cohesion: 0.11
Nodes (9): creating_an_existing_session_does_not_replace_its_frozen_snapshot(), delete_thread_cascades(), messages_round_trip(), migration_creates_tables(), project_thread_assignment_is_exclusive_and_delete_can_remove_threads(), ProjectResourceRow, projects_resources_and_threads_round_trip(), session_search_finds_linked_persisted_chat_messages() (+1 more)

### Community 121 - "Decisions"
Cohesion: 0.20
Nodes (9): DD-001: Current docs live outside V1 planning artifacts, DD-002: Seed docs describe implemented behavior only, DD-003: Frontmatter is required for current source-of-truth docs, DD-004: The old architecture doc is superseded, not deleted, DD-005: Keep canonical docs concise, DD-006: Code changes should update docs at the ownership boundary, Decisions, Documentation Decisions (+1 more)

### Community 122 - "main.rs"
Cohesion: 0.17
Nodes (12): 1. Frontend (`app/src`), 2. Tauri Shell (`app/src-tauri/src/lib.rs`), 3. Ollama Client (`app/src-tauri/src/ollama.rs`), 4. MCP Client (`app/src-tauri/src/mcp.rs`), 5. SQLite Persistenz (`app/src-tauri/src/db.rs`), Architektur, Komponenten, Routing & Layout (+4 more)

### Community 123 - "parseBlock"
Cohesion: 0.31
Nodes (10): isIgnorableLine(), isKeyValue(), parseBlock(), parseFrontmatter(), parseKeyValue(), parseListBlock(), parseScalar(), parseYamlScalar() (+2 more)

### Community 124 - "package.json"
Cohesion: 0.22
Nodes (8): description, license, name, private, repository, type, type, version

### Community 125 - "Open Cowork App"
Cohesion: 0.22
Nodes (8): Checks, Important Paths, Installer, License, Local Development, Notes, Open Cowork App, Stack

### Community 126 - "doctor.mjs"
Cohesion: 0.21
Nodes (10): addCheck(), checks, ciMode, commandVersion(), missingOptional, missingRequired, npmInvocation, root (+2 more)

### Community 128 - ".list_artifact_exports"
Cohesion: 0.18
Nodes (11): Desktop Smoke Test, Desktop-Steuerung validieren, Durchklickrunde, Fehlerprotokoll, Minimalfreigabe, OpenAI Computer Use Debugging, Smoke-Start, Smoke-Start (+3 more)

### Community 129 - "Runtime Compatibility"
Cohesion: 0.22
Nodes (8): Browser Versus Tauri, Compatibility Update Rule, Current Target, Development Commands, Development Runtime, Packaging Runtime, Provider Runtime Modes, Runtime Compatibility

### Community 131 - "Contributing"
Cohesion: 0.25
Nodes (7): Before You Start, Contributing, Contribution License, Development Setup, Pull Request Checklist, Security And Privacy, Validation

### Community 132 - "Product Documentation Index"
Cohesion: 0.25
Nodes (8): Duplicate Statuses, Element Counts, Missing-Doc Hints, Product Documentation Index, Route Map, Schema Errors, Source Documents, Summary

### Community 133 - "useTerminalStore"
Cohesion: 0.67
Nodes (3): ELEMENT_FIELDS, frontmatterHasList(), validateProductDoc()

### Community 134 - ".search_memory_entries"
Cohesion: 0.33
Nodes (7): captureDesktopVerificationAttachment(), createDesktopscreenshotAttachment(), createMcpscreenshotAttachment(), createToolStreamId(), formatDesktopscreenshotSummary(), parseBase64DataUrl(), parseMcpscreenshotPayload()

### Community 135 - "Decisions"
Cohesion: 0.29
Nodes (6): Decisions, Legacy Surface Decisions, LS-001: Remove unrouted duplicate UI surfaces, LS-002: Keep PlanApproval compatibility data, not the old task screen, LS-003: Keep RightSidebar panels, remove the unused container export, Review Rule

### Community 136 - "6. Querschnittliche UI-Anforderungen"
Cohesion: 0.20
Nodes (11): items, type, items, type, pattern, type, buttons, infos (+3 more)

### Community 138 - "default.json"
Cohesion: 0.17
Nodes (11): description, identifier, permissions, $schema, windows, core:default, core:window:allow-destroy, dialog:default (+3 more)

### Community 139 - "queryEngine.approval.test.ts"
Cohesion: 0.20
Nodes (10): items, type, type, data_dependencies, source_files, tests, items, type (+2 more)

### Community 140 - "Claude-Code Feature Adoption (Open_Cowork)"
Cohesion: 0.33
Nodes (5): 1) Extrahierte Claude-Code Feature-Familien, 2) Jetzt in Open_Cowork integriert (dieser Slice), 3) Bereits vorher vorhanden und Claude-nah, 4) Noch offen fuer "vollstaendige" Claude-Paritaet, Claude-Code Feature Adoption (Open_Cowork)

### Community 141 - "3. Gestaltungsprinzipien"
Cohesion: 0.33
Nodes (6): 3. Gestaltungsprinzipien, UI-001 Klarer Arbeitskontext, UI-002 Progressive Offenlegung, UI-003 Kontrollierbarkeit, UI-004 Desktop-First Bedienung, UI-005 Konsistenz

### Community 142 - "uniqueStrings"
Cohesion: 0.40
Nodes (6): buildRouteRows(), compareRoutes(), formatStatuses(), mergeRoutes(), readAppRoutes(), uniqueStrings()

### Community 144 - "Current Routes"
Cohesion: 0.40
Nodes (4): Current Routes, Route Update Rule, Settings Sections, Shell Decisions

### Community 145 - "4. Informationsarchitektur"
Cohesion: 0.40
Nodes (5): 4. Informationsarchitektur, UI-010 Hauptnavigation, UI-011 Linke Sidebar, UI-012 Hauptarbeitsflaeche, UI-013 Rechte Kontextzone

### Community 146 - "Copilot Project Instructions"
Cohesion: 0.40
Nodes (4): Completion Checklist, Copilot Project Instructions, Goal, Required Behavior

### Community 147 - "Memory And Skill Discipline"
Cohesion: 0.40
Nodes (4): Definition Of Done, Memory And Skill Discipline, Purpose, Steps

### Community 148 - "Skills Index"
Cohesion: 0.40
Nodes (4): Available Skills, Maintenance Rule, memory-and-skill-discipline, Skills Index

### Community 149 - "openaiCompatibleClient.test.ts"
Cohesion: 0.50
Nodes (3): hasTauriRuntimeMock, readToolDef, safeInvokeMock

### Community 151 - "skill-and-memory.instructions.md"
Cohesion: 0.50
Nodes (3): Quality Rules, Trigger Cases, Workflow

### Community 152 - "Agent Memory Test Tasks"
Cohesion: 0.50
Nodes (3): Agent Memory Test Tasks, Prompts, Verification rule

### Community 155 - "McpView.tsx"
Cohesion: 0.36
Nodes (6): Assert-CodeSigningCertificate(), Assert-InstallerSignature(), ConvertTo-ProcessArgument(), Invoke-BoundedProcess(), Normalize-Thumbprint(), Test-CodeSigningEku()

### Community 160 - ".get_engine_run"
Cohesion: 0.09
Nodes (21): ApprovalResolution, McpServerInstatce, McpServerStatus, McpTratsportKind, PluginMatifest, providerAdapter, providerCapability, providerHealth (+13 more)

### Community 174 - "duckduckgo-websearch-server.mjs"
Cohesion: 0.22
Nodes (11): decodeHtmlEntities(), DEFAULTS, extractTargetUrl(), formatTextResult(), normalizeSafeSearch(), parseResultsFromHtml(), rl, safeSearchToKp() (+3 more)

### Community 178 - "ui-quality.spec.ts"
Cohesion: 0.33
Nodes (3): PRODUCT_SURFACES, ProductSurface, VIEWPORTS

### Community 180 - "sensitive_data.rs"
Cohesion: 0.13
Nodes (23): bounded_json_stays_valid_and_redacted(), bounds_utf8_text_without_splitting_characters(), diagnostic_label(), is_sensitive_key(), normalized_key(), recursively_redacts_sensitive_keys_and_nested_environment_maps(), redact_and_bound_json_text(), redact_and_bound_optional_json() (+15 more)

### Community 181 - "gateway_health"
Cohesion: 0.04
Nodes (89): chat_turn(), chat_turn_stream(), ChatTurnRequest, crew_stop(), CrewGovernanceAgentAccessPayload, CrewGovernancePayload, CrewMemoryEntryPayload, CrewMemoryPayload (+81 more)

### Community 182 - "webSearchSources.ts"
Cohesion: 0.11
Nodes (16): streamOllamaMessagesMock, QueryEngine, chunk(), DEFAULT_ORCHESTRATOR_CONFIG, ToolExecutionEvent, ToolExecutionResult, ToolOrchestrator, ToolOrchestratorConfig (+8 more)

### Community 183 - "Audit Integrity and Threat Model Contract"
Cohesion: 0.29
Nodes (6): Audit Integrity and Threat Model Contract, Guarantees and non-goals, Protected assets and trust boundaries, Rotation and legacy compatibility, Signed record format, Status and product behavior

### Community 185 - "6. Querschnittliche UI-Anforderungen"
Cohesion: 0.29
Nodes (7): 6. Querschnittliche UI-Anforderungen, UI-070 Globale Suche, UI-071 Command Palette, UI-072 Shortcuts, UI-073 Benachrichtigungen, UI-074 Lade-, Leer- und Fehlerzustaende, UI-075 Statuskonsistenz

### Community 186 - "AnthropicAPIError"
Cohesion: 0.29
Nodes (7): 1. Lokale Desktop-Tools fuer Gemma/Ollama, 2. Screenshot-MCP, 3. `ComputerUseAppTest`, Bekannte Namensfalle, Desktop-Steuerung und Computer Use, Uebersicht, Wann welchen Pfad nutzen?

### Community 189 - "Release Supply Chain Contract"
Cohesion: 0.29
Nodes (6): Failure and change policy, Locked source and toolchain, Release evidence, Release Supply Chain Contract, Vulnerability and license policy, Windows Authenticode

### Community 191 - ".insert_engine_run_event_with_details"
Cohesion: 0.29
Nodes (6): Compatibility matrix, Hermes Memory and Command Adoption, Intentional differences, Memory lifecycle, Scope, Slash-command contract

### Community 192 - "7. Desktop- und Fensterverhalten"
Cohesion: 0.05
Nodes (76): App(), AppRoutes(), BackendPolicyState, confirmAppClose(), CoworkView, CrewView, FeaturesView, hasRunningWork() (+68 more)

### Community 194 - "8. Accessibility"
Cohesion: 0.40
Nodes (5): 8. Accessibility, UI-090 Tastaturbedienung, UI-091 Screenreader- und Semantik-Anforderungen, UI-092 Kontrast und Lesbarkeit, UI-093 Zoom und Skalierung

### Community 195 - "5.2 Cowork-Hauptansicht"
Cohesion: 0.40
Nodes (5): 5.2 Cowork-Hauptansicht, UI-030 Prompteingabe, UI-031 Nachrichtenverlauf, UI-032 Thinking-, Status- und Streaming-Darstellung, UI-033 Folgeaktionen

### Community 196 - "5.3 Plan-, Approval- und Task-UI"
Cohesion: 0.29
Nodes (6): Automatic draft knowledge, Curated memory, Hermes Memory P0 Smoke Checklist, Release gate, Session recall, Slash commands

### Community 197 - "7. Desktop- und Fensterverhalten"
Cohesion: 0.50
Nodes (4): 7. Desktop- und Fensterverhalten, UI-080 Responsives Desktop-Layout, UI-081 Fensterzustand und Persistenz, UI-082 Fokusmanagement

### Community 198 - "9. Visuelle und textliche Anforderungen"
Cohesion: 0.50
Nodes (4): 9. Visuelle und textliche Anforderungen, UI-100 Design System, UI-101 Dichte und Ruhe, UI-102 Sprache und Mikrotexte

### Community 199 - "WorkTaskRow"
Cohesion: 0.28
Nodes (3): delete_work_task_removes_matching_schedule(), work_task_lifecycle_round_trip(), WorkTaskRow

### Community 200 - "resolveShellNavigationTarget"
Cohesion: 0.67
Nodes (3): inferShellCwdFromCommand(), normalizeShellPath(), resolveShellNavigationTarget()

### Community 201 - "main.rs"
Cohesion: 0.29
Nodes (7): type, source_files, tests, items, type, items, type

### Community 202 - "normalizeDesktopCoordinateSpace"
Cohesion: 0.29
Nodes (7): pattern, items, type, items, type, owned_elements, primary_actions

### Community 203 - "CrewGovernancePanel.tsx"
Cohesion: 0.14
Nodes (17): CrewControlPlanePanel(), formatTimestamp(), Props, CrewGovernancePanel(), formatApprovalStatus(), formatApprovalType(), formatTimestamp(), GOVERNANCE_MODES (+9 more)

### Community 204 - "App.test.tsx"
Cohesion: 0.40
Nodes (4): Answer, Outcome, Q: warum wird mein Opencowork nicht bei einer Google suche angezigt?, Source Nodes

### Community 205 - "Error"
Cohesion: 0.50
Nodes (3): $schema, title, type

### Community 206 - "Self"
Cohesion: 0.50
Nodes (3): $schema, title, type

### Community 207 - "AppHandle"
Cohesion: 0.50
Nodes (3): $schema, title, type

### Community 208 - "globalSearch.ts"
Cohesion: 0.15
Nodes (17): SessionSummary, ChatThread, buildSearchIndex(), BuildSearchIndexInput, compact(), filterSearchIndex(), getTaskSearchTitle(), normalize() (+9 more)

### Community 209 - "taskStore.ts"
Cohesion: 0.13
Nodes (16): isTauriRuntime(), normalizeRiskLevel(), normalizeStepState(), normalizeTaskStatus(), PermissionConfig, persistInvoke(), PlanApproval, PlanApprovalStatus (+8 more)

### Community 210 - "Item"
Cohesion: 0.67
Nodes (3): pattern, type, id

### Community 211 - "messageDisplay.ts"
Cohesion: 0.26
Nodes (14): AssistantPresentationOptions, buildModelDebugContent(), escapeRegExp(), extractThinkingContent(), extractThinkingFallback(), resolveAssistantPresentation(), resolveDisplayedAssistantContent(), resolveDisplayedThinkingContent() (+6 more)

### Community 212 - "State"
Cohesion: 0.67
Nodes (3): pattern, type, owner_element

### Community 213 - "StatusCode"
Cohesion: 0.67
Nodes (3): pattern, type, id

### Community 214 - "T"
Cohesion: 0.67
Nodes (3): source_files, items, type

### Community 215 - "CrewApprovalRow"
Cohesion: 0.67
Nodes (3): tests, items, type

### Community 216 - "chatStore.ts"
Cohesion: 0.05
Nodes (39): FeaturesView(), isWorkbenchTab(), TABS, WorkbenchTab, ClaudeMcpServer, exampleJson(), McpCallResponse, McpProbeResponse (+31 more)

### Community 217 - "CrewDefinitionRow"
Cohesion: 0.67
Nodes (3): visible_text, items, type

### Community 218 - "CrewDefinitionVersionRow"
Cohesion: 0.67
Nodes (3): items, type, empty_states

### Community 219 - "CrewExecutionLogRow"
Cohesion: 0.67
Nodes (3): items, type, error_states

### Community 220 - "CrewRoleBindingRow"
Cohesion: 0.67
Nodes (3): pattern, type, id

### Community 221 - "EngineRunArtifactRow"
Cohesion: 0.67
Nodes (3): items, type, layout_regions

### Community 222 - "EngineRunCheckpointRow"
Cohesion: 0.67
Nodes (3): items, type, loading_states

### Community 224 - "modelCapabilities.ts"
Cohesion: 0.26
Nodes (12): detectModelCapabilities(), ChatProviderState, detectProviderModelCapabilities(), hasPersistedVisionBlock(), inferFamily(), markModelVisionUnsupported(), ModelCapabilities, normalizeModelKey() (+4 more)

### Community 228 - "CoworkContextRail.tsx"
Cohesion: 0.19
Nodes (12): canOpenArtifact(), CoworkContextRail(), CoworkContextRailProps, STATUS_LABELS, baseProps, safeInvokeMock, task, toolStatusIcon() (+4 more)

### Community 229 - "documentWorkspaceStore.ts"
Cohesion: 0.20
Nodes (13): ArtifactVersionResponse, createDocument(), DocumentPreviewPage, DocumentPreviewResponse, DocumentWorkspaceItem, DocumentWorkspaceState, inferFormat(), isDocumentWorkspacePath() (+5 more)

### Community 230 - "gateway_health"
Cohesion: 0.31
Nodes (14): aggregate_gateway_status(), build_local_gateway_subsystems(), check_audit_writable(), gateway_health(), gateway_payload(), gateway_probe(), gateway_provider_probe(), gateway_status() (+6 more)

### Community 231 - "engineRunRecords.ts"
Cohesion: 0.31
Nodes (11): asNullableString(), asNumber(), asRecord(), asString(), asTimestampString(), EngineRunArtifactRow, EngineRunEventRow, ISO_EPOCH (+3 more)

### Community 232 - "webSearchSources.ts"
Cohesion: 0.35
Nodes (9): HighlightedChatText(), HighlightedChatTextProps, appendWebSearchSources(), extractWebSearchSources(), formatWebSearchSourcesBlock(), mergeWebSearchSources(), normalizeUrl(), parseWebSearchSourcesFromToolResult() (+1 more)

### Community 233 - "claudeBridge.ts"
Cohesion: 0.20
Nodes (9): ClaudePermissionMode, buildClaudeSystemAddendum(), compactHistoryForPrompt(), isToolDeniedByRules(), ParsedSlashCommand, parseSlashCommand(), SLASH_COMMAND_DEFINITIONS, SlashCommandDefinition (+1 more)

### Community 236 - "scan-secrets.mjs"
Cohesion: 0.31
Nodes (9): allowedPrivacyMatch(), HISTORY, isText(), lineNumber(), privacyRules, runGit(), scanCurrent(), scanHistory() (+1 more)

### Community 237 - "SettingsView.test.tsx"
Cohesion: 0.22
Nodes (4): checkOllamaStatusMock, fetchOllamaModelsMock, invokeMock, saveDialogMock

### Community 238 - "test_crew_runtime.py"
Cohesion: 0.22
Nodes (4): CrewRuntimeComplexIntegrationTests, CrewRuntimeIntegrationTests, CrewRuntimeParallelIntegrationTests, CrewRuntimeStatusTests

### Community 239 - "followUpPrompt.ts"
Cohesion: 0.42
Nodes (7): buildClarificationContinuationPrompt(), ClarificationContext, CLARIFYING_QUESTION_PATTERNS, FollowUpPromptMessage, inferClarificationContext(), isLikelyClarifyingQuestion(), isLikelyShortFollowUpAnswer()

### Community 241 - "windowState.ts"
Cohesion: 0.48
Nodes (6): canUseDesktopApis(), captureAndStoreWindowState(), loadFromStorage(), saveToStorage(), setupWindowStatePersistence(), StoredWindowState

### Community 242 - "Security Policy"
Cohesion: 0.33
Nodes (5): Public Security Improvements, Report A Vulnerability, Security Design Notes, Security Policy, Supported Versions

### Community 243 - "Support"
Cohesion: 0.33
Nodes (5): Before Opening An Issue, Safe Diagnostic Sharing, Scope, Support, Where To Ask

### Community 244 - "5.5 Settings-Ansicht"
Cohesion: 0.50
Nodes (4): 5.5 Settings-Ansicht, UI-060 Einstellungsarchitektur, UI-061 Konfigurationsklarheit, UI-062 Rueckmeldung und Tests in Einstellungen

## Knowledge Gaps
- **1295 isolated node(s):** `ProductSurface`, `PRODUCT_SURFACES`, `VIEWPORTS`, `name`, `private` (+1290 more)
  These have ≤1 connection - possible missing edges or undocumented components.
- **33 thin communities (<3 nodes) omitted from report** — run `graphify query` to explore isolated nodes.

## Suggested Questions
_Questions this graph is uniquely positioned to answer:_

- **Why does `Database` connect `Result` to `lib.rs`, `String`, `chatStore.ts`, `personalityStore.ts`, `LeftSidebar.tsx`, `sensitive_data.rs`, `gateway_health`, `RunPanel.tsx`, `getToolDefinitions`, `QueryEngine`, `globalSearch.ts`, `RequestContext`, `WorkTaskRow`, `memoryStore.ts`, `terminal_backends.rs`, `memory_engine.rs`, `skill_engine.rs`, `5.3 Plan-, Approval- und Task-UI`, `insights.rs`, `gateway_health`, `.list_steps`, `workTasksStore.ts`, `Desktop Smoke Test`?**
  _High betweenness centrality (0.073) - this node is a cross-community bridge._
- **Why does `CoworkView()` connect `CoworkView.tsx` to `crewStore.ts`, `App.tsx`, `Option`, `workTaskCrewRuntime.ts`, `webSearchSources.ts`, `crew_provider_health_check`, `registry.ts`, `7. Desktop- und Fensterverhalten`, `lifecycle.ts`, `taskStore.ts`, `5.5 Settings-Ansicht`, `messageDisplay.ts`, `chatStore.ts`, `Vec`, `Current Architecture`, `webSearchSources.ts`, `duckduckgo-websearch-server.mjs`, `claudeBridge.ts`, `gateway_health`, `followUpPrompt.ts`?**
  _High betweenness centrality (0.035) - this node is a cross-community bridge._
- **Why does `_TextExtractor` connect `crew_provider_health_check` to `Message`, `audit_service.rs`?**
  _High betweenness centrality (0.023) - this node is a cross-community bridge._
- **What connects `ProductSurface`, `PRODUCT_SURFACES`, `VIEWPORTS` to the rest of the system?**
  _1295 weakly-connected nodes found - possible documentation gaps or missing edges._
- **Should `Result` be split into smaller, more focused modules?**
  _Cohesion score 0.045224817009410945 - nodes in this community are weakly interconnected._
- **Should `lib.rs` be split into smaller, more focused modules?**
  _Cohesion score 0.07878787878787878 - nodes in this community are weakly interconnected._
- **Should `crewStore.ts` be split into smaller, more focused modules?**
  _Cohesion score 0.1067193675889328 - nodes in this community are weakly interconnected._