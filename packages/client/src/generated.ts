// Generated from Rust contract crates. Do not edit.

export type AccessMode = "read_only" | "confirm_mutations" | "confirm_all" | "autonomous";

export type ActivateHistoricalImport = {
  "expected_limit_revision": number;
  "explicit_activation": boolean;
  "plan_sha256": string;
};

export type ActivityItem = {
  "order_title": string;
  "project_name": string;
  "run": RunRecord;
};

export type ActivityPage = {
  "attention": number;
  "items": Array<ActivityItem>;
  "next_offset"?: number | null;
  "total": number;
};

export type ActorChatConfiguration = {
  "project_id": string;
  "revision": number;
  "thread_id": string;
  "updated_at": number;
  "value"?: ActorChatRunConfiguration | null;
};

export type ActorChatRunConfiguration = {
  "expected_mcp_revisions"?: {
  [key: string]: number;
};
  "mcp_metadata_ids"?: Array<string>;
  "memory_ids"?: Array<string>;
  "model_binding"?: ModelRegistryRef | null;
  "model_profile_id": string;
  "output"?: StructuredOutput | null;
  "provider_revision": number;
  "run_configuration": ActorRunConfiguration;
  "skill_ids"?: Array<string>;
};

export type ActorContextRecord = {
  "deleted": boolean;
  "etag": string;
  "id": string;
  "project_id"?: string | null;
  "revision": number;
  "value": unknown;
};

export type ActorMemoryDefinition = {
  "category": string;
  "confidence"?: number | null;
  "content": string;
  "enabled"?: boolean;
  "key": string;
  "project_id"?: string | null;
};

export type ActorModelContext = {
  "actor_id": string;
  "project_id": string;
  "project_revision": number;
  "provider": ProviderProfile;
  "provider_binding": unknown;
};

export type ActorModelRegistryView = {
  "context": ActorModelContext;
  "qualification": ModelRegistryView;
  "request_sha256": string;
  "source_sha256": string;
};

export type ActorRunConfiguration = {
  "access_mode": AccessMode;
  "agent_config": EffectiveAgentConfig;
  "budgets": Budgets;
};

export type ActorSkillDefinition = {
  "auto_learn"?: boolean;
  "description": string;
  "enabled"?: boolean;
  "name": string;
  "project_id"?: string | null;
  "prompt_template": string;
  "run_mode"?: AgentExecutionMode;
  "trigger_pattern"?: string | null;
};

export type ActorSkillLearningHistory = {
  "items": Array<ActorSkillLearningOutcome>;
};

export type ActorSkillLearningOutcome = {
  "node_ids": Array<string>;
  "observed_tools": Array<string>;
  "project_id": string;
  "proposed_prompt_template"?: string | null;
  "reason": string;
  "recorded_at": number;
  "run_id": string;
  "skill_id": string;
  "skill_revision": number;
  "source_definition": unknown;
  "state": string;
};

export type AddTeamMember = {
  "identity_id": string;
  "role": TeamRole;
};

export type AgentBehaviorSettings = {
  "command_allow_groups"?: Array<Array<string>>;
  "command_blacklist"?: Array<string>;
  "fallback_to_human_on_repeated_failure"?: boolean;
  "max_tool_calls_per_loop"?: number;
};

export type AgentConfiguration = {
  "access_mode"?: AccessMode;
  "agent_source"?: LibrarySource | null;
  "allow_delegation"?: boolean;
  "backstory"?: string;
  "budgets"?: Budgets;
  "generation"?: GenerationSettings;
  "goal"?: string;
  "instructions"?: string;
  "mcp_tools"?: Array<McpToolSelection>;
  "memory_ids"?: Array<string>;
  "model"?: AgentModelSelection;
  "native_tools"?: Array<string>;
  "personality_source"?: LibrarySource | null;
  "role"?: string;
  "skill_ids"?: Array<string>;
  "skills_markdown"?: string;
  "tool_filter"?: Array<string> | null;
};

export type AgentExecutionMode = "execute" | "plan" | "hybrid";

export type AgentModelSelection = {
  "kind": "inherit";
} | {
  "kind": "provider";
  "provider_id": string;
  "provider_revision": number;
} | {
  "account_id": string;
  "kind": "codex";
  "model"?: string | null;
  "reasoning_effort"?: string | null;
};

export type AgentTemplate = {
  "created_at": number;
  "id": string;
  "revision": number;
  "updated_at": number;
  "value": AgentTemplateDefinition;
};

export type AgentTemplateDefinition = {
  "configuration"?: AgentConfiguration;
  "description"?: string;
  "icon"?: string | null;
  "name"?: string;
  "personality"?: Personality | null;
};

export type AndroidAcceptInvitation = {
  "device_id": string;
  "display_name": string;
  "password": string;
  "token": string;
};

export type AndroidAdministrationRevision = {
  "expected_revision": number;
};

export type AndroidApplyProjectMerge = {
  "base_version_id": string;
  "current_version_id": string;
  "expected_project_revision": number;
  "resolutions"?: Array<AndroidMergeFileResolution>;
  "result_version_id": string;
};

export type AndroidApplyProjectVersion = {
  "expected_current_version_id"?: string | null;
  "expected_project_revision": number;
};

export type AndroidAuthManagementPolicy = {
  "admin_required": boolean;
  "revision": number;
  "schema_version": number;
};

export type AndroidAuthSecurityContext = {
  "schema_version": number;
  "session_id": string;
  "user_id": string;
};

export type AndroidAuthSession = {
  "active": boolean;
  "created_at": string;
  "current": boolean;
  "device_id": string;
  "expires_at": string;
  "id": string;
  "last_used_at": string;
  "revoke_reason"?: string | null;
  "revoked_at"?: string | null;
  "schema_version": number;
};

export type AndroidAuthTokens = {
  "access_expires_at": string;
  "access_token": string;
  "refresh_expires_at": string;
  "refresh_token": string;
  "schema_version": number;
  "session_id": string;
  "user_id": string;
};

export type AndroidAuthenticationAssurance = {
  "enrollment_allowed": boolean;
  "oidc_organisation_id"?: string | null;
  "passkey_registered": boolean;
  "provider_verification_expires_at"?: string | null;
  "provider_verified"?: boolean | null;
  "reauthentication_required": boolean;
  "required": boolean;
  "session_verified": boolean;
  "totp_enabled": boolean;
};

export type AndroidBrowserAuthTokens = {
  "access_expires_at": string;
  "access_token": string;
  "refresh_expires_at": string;
  "schema_version": number;
  "session_id": string;
  "user_id": string;
};

export type AndroidContentAccess = {
  "actor_user_id": string;
  "automatic_content_access": boolean;
  "instance_viewer": boolean;
  "local_membership_allowed": boolean;
  "managed": boolean;
  "organisation_id"?: string | null;
  "permissions": Array<string>;
  "project_id"?: string | null;
  "schema_version": number;
  "support_access": boolean;
};

export type AndroidCreateInvitation = {
  "email": string;
  "expires_at"?: string | null;
};

export type AndroidCreateProjectVersion = {
  "created_by_run_id"?: string | null;
  "diff_summary"?: unknown;
  "merge_base_version_id"?: string | null;
  "parent_version_id"?: string | null;
  "snapshot_manifest_id": string;
};

export type AndroidCreateSchedule = {
  "cron": string;
  "enabled"?: boolean;
  "executor_target": unknown;
  "input"?: unknown;
  "model_profile_id"?: string | null;
  "project_id": string;
  "task_id": string;
  "thread_id": string;
  "timezone": string;
};

export type AndroidCreateScheduleTask = {
  "config"?: unknown;
  "default_target"?: unknown;
  "instructions": string;
  "name": string;
  "project_id": string;
  "release"?: boolean;
  "required_capabilities"?: Array<string>;
};

export type AndroidCreateTeam = {
  "name": string;
};

export type AndroidCreateTerminalSession = {
  "columns": number;
  "rows": number;
};

export type AndroidCreateThreadSchedule = {
  "cron": string;
  "enabled"?: boolean;
  "executor_target": unknown;
  "idempotency_key": string;
  "input"?: unknown;
  "model_profile_id"?: string | null;
  "project_id": string;
  "task_id": string;
  "timezone": string;
  "title": string;
};

export type AndroidDisableTotp = {
  "password": string;
  "second_factor": string;
};

export type AndroidExecutorPoolProjectWrite = {
  "active": boolean;
  "expected_revision": number;
  "organization_id": string;
  "project_id": string;
  "request_id": string;
};

export type AndroidExecutorPoolWrite = {
  "active": boolean;
  "expected_revision": number;
  "id": string;
  "name": string;
  "organization_id": string;
  "policy": unknown;
  "request_id": string;
};

export type AndroidExecutorSyncBatch = {
  "changes": Array<AndroidWireSyncChange>;
  "project_id": string;
};

export type AndroidExecutorSyncGrantWrite = {
  "active": boolean;
  "expected_revision": number;
  "expires_at": number;
  "project_id": string;
  "request_id": string;
  "write": boolean;
};

export type AndroidForkDestination = {
  "kind": "private_local";
  "name": string;
} | {
  "kind": "team_managed";
  "name": string;
  "team_id": string;
};

export type AndroidForkObjectMapping = {
  "destination_id": string;
  "kind": string;
  "original_sha256": string;
  "source_id": string;
};

export type AndroidForkProject = {
  "destination": AndroidForkDestination;
  "expected_source_project_revision": number;
  "idempotency_key": string;
  "source_version_id": string;
};

export type AndroidForkThread = {
  "forked_from_message_id"?: string | null;
  "forked_from_thread_id"?: string | null;
  "idempotency_key"?: string | null;
  "project_id": string;
  "title": string;
};

export type AndroidInvitationSecret = {
  "email": string;
  "expires_at": string;
  "invitation_id": string;
  "schema_version": number;
  "token": string;
};

export type AndroidMergeFileResolution = {
  "choice": AndroidMergeResolutionChoice;
  "path": string;
};

export type AndroidMergeFileReview = {
  "auto_mergeable": boolean;
  "base_digest"?: string | null;
  "conflict_preview"?: string | null;
  "current_digest"?: string | null;
  "path": string;
  "renamed_from"?: string | null;
  "result_digest"?: string | null;
  "status": AndroidMergeFileStatus;
};

export type AndroidMergeFileStatus = "unchanged" | "added" | "deleted" | "current_only" | "result_only" | "identical_change" | "auto_merged" | "text_conflict" | "binary_conflict" | "renamed";

export type AndroidMergeResolutionChoice = "current" | "result" | "delete" | "auto_merged";

export type AndroidMobileCacheActivate = {
  "expected_revision": number;
  "ordinals": Array<number>;
  "remainder_history_only_reason": string;
  "request_id": string;
  "thread_id": string;
};

export type AndroidMobileCacheBegin = {
  "archive_sha256": string;
  "archive_size": number;
  "request_id": string;
};

export type AndroidNativeAuthorization = {
  "code_challenge": string;
  "code_challenge_method": string;
  "device_id": string;
  "email": string;
  "password": string;
  "second_factor"?: string | null;
};

export type AndroidNativeAuthorizationCode = {
  "code": string;
  "expires_at": string;
  "schema_version": number;
};

export type AndroidNativeTokenExchange = {
  "code": string;
  "code_verifier": string;
  "device_id": string;
};

export type AndroidOidcBegin = {
  "device_id": string;
  "native_code_challenge"?: string | null;
  "profile_id": string;
  "purpose": string;
  "redirect_uri": string;
  "request_id": string;
  "resume_secret": string;
};

export type AndroidOidcCompatStart = {
  "client_state": string;
  "code_challenge": string;
  "code_challenge_method": string;
  "device_id": string;
  "redirect_uri": string;
};

export type AndroidOidcComplete = {
  "code": string;
  "flow_id": string;
  "resume_secret": string;
  "state": string;
};

export type AndroidOidcConfig = {
  "allow_signup": boolean;
  "client_id": string;
  "endpoint_origins"?: Array<string>;
  "groups_claim"?: string;
  "issuer": string;
  "redirect_uris": Array<string>;
};

export type AndroidOidcFinish = {
  "device_id": string;
  "flow": AndroidOidcComplete;
  "native_code_challenge"?: string | null;
  "request_id": string;
};

export type AndroidOidcFlow = {
  "authorization_url": string;
  "expires_at": number;
  "id": string;
  "phase": string;
  "profile_id": string;
  "profile_revision": number;
  "purpose": string;
};

export type AndroidOidcGroupMapping = {
  "group": string;
  "role": string;
};

export type AndroidOidcLink = {
  "active": boolean;
  "id": string;
  "issuer": string;
  "profile_id": string;
  "revision": number;
  "subject": string;
};

export type AndroidOidcMembershipOrigin = {
  "active": boolean;
  "claims_sha256": string;
  "expires_at": number;
  "external_identity_id": string;
  "flow_id": string;
  "groups_sha256": string;
  "identity_id": string;
  "membership_id": string;
  "membership_revision": number;
  "organisation_id": string;
  "profile_id": string;
  "role": string;
  "settings_revision": number;
};

export type AndroidOidcOrigin = {
  "claims_sha256": string;
  "expires_at": number;
  "external_identity_id": string;
  "flow_id": string;
  "issuer": string;
  "method": string;
  "organization_id"?: string | null;
  "profile_id": string;
  "profile_revision": number;
  "verified_at": number;
};

export type AndroidOidcProfile = {
  "active": boolean;
  "config": AndroidOidcConfig;
  "config_sha256": string;
  "epoch": string;
  "has_secret": boolean;
  "id": string;
  "name": string;
  "organization_id"?: string | null;
  "revision": number;
};

export type AndroidOidcProfileWrite = {
  "active": boolean;
  "client_secret"?: string | null;
  "config": AndroidOidcConfig;
  "expected_revision": number;
  "name": string;
  "organization_id"?: string | null;
  "request_id": string;
};

export type AndroidOidcRecover = {
  "flow_id": string;
  "resume_secret": string;
};

export type AndroidOidcRetire = {
  "flow_id": string;
  "request_id": string;
  "resume_secret": string;
};

export type AndroidOidcSsoProvider = {
  "groups_claim": string;
  "id": string;
  "issuer": string;
  "label": string;
};

export type AndroidOidcSsoSettings = {
  "auto_provision": boolean;
  "available": boolean;
  "enabled": boolean;
  "group_mappings": Array<AndroidOidcGroupMapping>;
  "organisation_id": string;
  "provider_id"?: string | null;
  "providers": Array<AndroidOidcSsoProvider>;
  "revision": number;
};

export type AndroidOidcSsoWrite = {
  "auto_provision": boolean;
  "enabled": boolean;
  "expected_revision": number;
  "group_mappings": Array<AndroidOidcGroupMapping>;
  "provider_id"?: string | null;
};

export type AndroidOidcUnlink = {
  "expected_revision": number;
  "request_id": string;
};

export type AndroidPasswordCredentials = {
  "device_id": string;
  "email": string;
  "password": string;
  "second_factor"?: string | null;
};

export type AndroidPersonalStorageQuotaWrite = {
  "expected_revision": number;
  "limit_bytes": number;
  "request_id": string;
};

export type AndroidPrivateProjectMemberPage = {
  "items": Array<AndroidPrivateProjectMemberView>;
  "next_cursor"?: string | null;
  "schema_version": number;
};

export type AndroidPrivateProjectMemberReceipt = {
  "active": boolean;
  "permissions": Array<string>;
  "project_id": string;
  "request_sha256": string;
  "revision": number;
  "schema_version": number;
  "source_owner_id": string;
  "user_id": string;
};

export type AndroidPrivateProjectMemberView = {
  "active": boolean;
  "display_name": string;
  "permissions": Array<string>;
  "project_id": string;
  "revision": number;
  "role": string;
  "source_owner_id": string;
  "user_id": string;
};

export type AndroidProjectForkReceipt = {
  "committed_at": string;
  "destination_project_id": string;
  "destination_team_id"?: string | null;
  "destination_version_id"?: string | null;
  "execution_authority_copied": boolean;
  "format": string;
  "idempotency_key": string;
  "mappings": Array<AndroidForkObjectMapping>;
  "original_content_sha256": string;
  "source_project_id": string;
  "source_version_id": string;
};

export type AndroidProjectMemberPage = {
  "items": Array<AndroidProjectMemberView>;
  "next_cursor"?: string | null;
};

export type AndroidProjectMemberView = {
  "active": boolean;
  "display_name": string;
  "membership_id": string;
  "membership_revision": number;
  "permissions": Array<string>;
  "project_id": string;
  "revision": number;
  "role": string;
  "schema_version": number;
  "user_id": string;
};

export type AndroidProjectMemberWrite = {
  "expected_revision"?: number | null;
  "idempotency_key"?: string | null;
  "role": string;
  "user_id": string;
};

export type AndroidProjectMergeReview = {
  "base_version_id": string;
  "current_version_id": string;
  "files": Array<AndroidMergeFileReview>;
  "project_id": string;
  "result_version_id": string;
  "schema_version": number;
};

export type AndroidProjectVersion = {
  "created_at": string;
  "created_by_run_id"?: string | null;
  "id": string;
  "merge_base_version_id"?: string | null;
  "parent_version_id"?: string | null;
  "project_id": string;
  "revision": number;
  "schema_version": number;
  "snapshot_manifest_id": string;
};

export type AndroidProjectVersionContent = {
  "kind": string;
  "original_sha256": string;
  "payload": unknown;
  "source_id": string;
};

export type AndroidRecoveryCodes = {
  "recovery_codes": Array<string>;
  "schema_version": number;
};

export type AndroidRefreshSession = {
  "refresh_token": string;
};

export type AndroidResumeRun = {
  "expected_revision": number;
  "request_id": string;
};

export type AndroidRunCheckpoint = {
  "created_at": string;
  "executor_state": unknown;
  "id": string;
  "run_id": string;
  "safe_to_resume": boolean;
  "schema_version": number;
  "sequence": number;
};

export type AndroidRunCheckpointDescriptor = {
  "created_at": string;
  "id": string;
  "origin": string;
  "run_id": string;
  "safe_to_resume": boolean;
  "sequence": number;
  "state_page_sha256": Array<string>;
  "state_pages": number;
  "state_sha256": string;
  "state_size": number;
};

export type AndroidRunCheckpointPage = {
  "entries": Array<AndroidRunCheckpointDescriptor>;
  "next_sequence"?: number | null;
  "run_id": string;
};

export type AndroidRunResumeReceipt = {
  "checkpoint_id": string;
  "queued_revision": number;
  "request_id": string;
  "run_id": string;
};

export type AndroidRunWaiting = {
  "checked_at"?: string | null;
  "reason"?: AndroidRunWaitingReason | null;
  "run_id": string;
};

export type AndroidRunWaitingReason = "platform_capacity" | "organisation_capacity" | "user_capacity" | "instance_capacity" | "chat_workspace_busy" | "admission_unavailable" | "runner_capacity";

export type AndroidScheduleHistory = {
  "next_cursor"?: number | null;
  "occurrences": Array<AndroidScheduleOccurrence>;
};

export type AndroidScheduleOccurrence = {
  "catch_up_truncated": boolean;
  "completed_at"?: string | null;
  "due_at": string;
  "error"?: string | null;
  "executor_target": unknown;
  "id": string;
  "missed_occurrences": number;
  "run_id": string;
  "schedule_id": string;
  "sequence": number;
  "status": string;
  "triggered_at": string;
};

export type AndroidScheduleOriginal = {
  "purpose": string;
  "receipt": unknown;
  "request_sha256": string;
};

export type AndroidSchedulePreview = {
  "ambiguous_time_policy": string;
  "cron": string;
  "next_occurrences": Array<string>;
  "nonexistent_time_policy": string;
  "timezone": string;
};

export type AndroidScheduleRecord = {
  "blocked_reason"?: string | null;
  "created_at": string;
  "cron": string;
  "deleted_at"?: string | null;
  "enabled": boolean;
  "etag": string;
  "executor_target": unknown;
  "id": string;
  "input": unknown;
  "last_triggered_at"?: string | null;
  "model_profile_id"?: string | null;
  "next_run_at"?: string | null;
  "project_id": string;
  "revision": number;
  "schema_version": number;
  "task_id": string;
  "thread_id": string;
  "timezone": string;
  "updated_at": string;
};

export type AndroidScheduleResolutionReceipt = {
  "action": string;
  "dispatch_sha256": string;
  "evidence_sha256": string;
  "finalized_at": string;
  "grant_id": string;
  "id": string;
  "occurrence_id": string;
  "original_creation_key": string;
  "original_request_sha256": string;
  "original_spec_sha256": string;
  "outcome": string;
  "run_id": string;
  "schedule": AndroidScheduleRecord;
  "schedule_id": string;
  "schedule_retired": boolean;
  "schema_version": number;
  "terminal_receipt_sha256"?: string | null;
  "unresolved_effects": boolean;
};

export type AndroidScheduleResolutionRequest = {
  "action": string;
  "evidence_sha256": string;
  "expected_revision": number;
};

export type AndroidScheduleResolutionView = {
  "block_reason"?: string | null;
  "can_resolve": boolean;
  "can_retire": boolean;
  "dispatch_sha256": string;
  "evidence_sha256": string;
  "family": string;
  "grant_id": string;
  "never_created_guest": boolean;
  "never_dispatched": boolean;
  "occurrence_id": string;
  "occurrence_status": string;
  "original_creation_key": string;
  "original_request_sha256": string;
  "original_resolution"?: AndroidScheduleResolutionReceipt | null;
  "original_spec_sha256": string;
  "physical_quiescence_verified": boolean;
  "physical_stop_verified": boolean;
  "run_id": string;
  "run_state": string;
  "schedule_id": string;
  "schedule_retired": boolean;
  "schedule_revision": number;
  "schema_version": number;
  "terminal_receipt_sha256"?: string | null;
  "unresolved_effects": boolean;
};

export type AndroidSetTeamMember = {
  "role": string;
  "user_id": string;
};

export type AndroidSupportGrantRevoke = {
  "expected_revision": number;
  "request_id": string;
};

export type AndroidSupportGrantWrite = {
  "expected_revision": number;
  "expires_at": number;
  "project_id": string;
  "reason": string;
  "request_id": string;
  "user_id": string;
};

export type AndroidTeam = {
  "created_at": string;
  "deleted_at"?: string | null;
  "etag": string;
  "id": string;
  "name": string;
  "owner_user_id": string;
  "revision": number;
  "schema_version": number;
  "updated_at": string;
};

export type AndroidTeamMember = {
  "created_at": string;
  "display_name": string;
  "email": string;
  "role": string;
  "schema_version": number;
  "team_id": string;
  "user_id": string;
};

export type AndroidTerminalResize = {
  "columns": number;
  "operation": string;
  "rows": number;
};

export type AndroidTerminalSession = {
  "columns": number;
  "created_at": string;
  "frames": Array<unknown>;
  "input_bytes": number;
  "observations"?: Array<unknown>;
  "original_receipt"?: unknown;
  "output_bytes": number;
  "publication"?: unknown;
  "rows": number;
  "run_id": string;
  "schema_version": number;
  "session_id": string;
  "state": string;
};

export type AndroidTerminalSessionPage = {
  "after": number;
  "items": Array<AndroidTerminalSession>;
  "observed_at": string;
};

export type AndroidTerminalSessionTicket = {
  "expires_at": string;
  "protocol": string;
  "schema_version": number;
  "session_id": string;
  "token": string;
};

export type AndroidThreadSchedule = {
  "schedule": AndroidScheduleRecord;
  "schema_version": number;
  "thread": unknown;
};

export type AndroidTotpCode = {
  "code": string;
};

export type AndroidTotpSetup = {
  "expires_at": string;
  "otpauth_uri": string;
  "schema_version": number;
  "secret": string;
};

export type AndroidTotpStatus = {
  "enabled": boolean;
  "schema_version": number;
  "unused_recovery_codes": number;
};

export type AndroidUpdateSchedule = {
  "cron": string;
  "enabled": boolean;
  "executor_target": unknown;
  "expected_revision": number;
  "input"?: unknown;
  "model_profile_id"?: string | null;
  "timezone": string;
};

export type AndroidWireSyncBatch = {
  "changes": Array<AndroidWireSyncChange>;
};

export type AndroidWireSyncChange = {
  "base_revision": number;
  "client_timestamp": string;
  "device_id": string;
  "entity_id": string;
  "entity_type": string;
  "operation": string;
  "operation_id": string;
  "payload"?: unknown;
  "schema_version": number;
};

export type AndroidWorkflowChange = {
  "expected_revision": number;
  "request_id": string;
  "revoke": boolean;
  "version_id"?: string | null;
};

export type AndroidWorkflowEngine = "workflow" | "crew";

export type AndroidWorkflowEntry = {
  "engine": AndroidWorkflowEngine;
  "id": string;
  "name": string;
  "owner_id": string;
  "project_id": string;
  "published_version"?: AndroidWorkflowVersionRef | null;
  "revision": number;
  "revoked": boolean;
  "source_entity_id"?: string | null;
  "source_revision"?: number | null;
};

export type AndroidWorkflowNodeView = {
  "dependencies": Array<string>;
  "model_steps": number;
  "node_id": string;
  "original_receipt_sha256"?: string | null;
  "result_artifact_id"?: string | null;
  "result_sha256"?: string | null;
  "result_size"?: number | null;
  "role_id": string;
  "state": string;
  "tool_steps": number;
};

export type AndroidWorkflowOperationReceipt = {
  "committed_at": string;
  "owner_id": string;
  "project_id": string;
  "request_id": string;
  "request_sha256": string;
  "version"?: AndroidWorkflowVersionRef | null;
  "workflow": AndroidWorkflowEntry;
};

export type AndroidWorkflowPage = {
  "items": Array<AndroidWorkflowEntry>;
  "next_cursor"?: string | null;
};

export type AndroidWorkflowPreflight = {
  "engine": AndroidWorkflowEngine;
  "max_parallel": number;
  "plan_sha256": string;
  "ready": boolean;
  "required_capabilities": Array<string>;
  "roles": Array<unknown>;
  "selection": AndroidWorkflowSelection;
};

export type AndroidWorkflowRunView = {
  "checkpoint_sha256": string;
  "engine": AndroidWorkflowEngine;
  "next_cursor"?: string | null;
  "nodes": Array<AndroidWorkflowNodeView>;
  "owner_id": string;
  "plan_sha256": string;
  "project_id": string;
  "revision": number;
  "run_id": string;
  "selection": AndroidWorkflowSelection;
};

export type AndroidWorkflowSelection = {
  "definition_sha256": string;
  "registry_revision": number;
  "version_id": string;
  "workflow_id": string;
};

export type AndroidWorkflowVersion = {
  "definition": unknown;
  "version": AndroidWorkflowVersionRef;
  "workflow": AndroidWorkflowEntry;
};

export type AndroidWorkflowVersionPage = {
  "next_revision"?: number | null;
  "versions": Array<AndroidWorkflowVersionRef>;
};

export type AndroidWorkflowVersionRef = {
  "created_at": string;
  "definition_sha256": string;
  "id": string;
  "revision": number;
};

export type AndroidWorkflowWrite = {
  "definition": unknown;
  "engine": AndroidWorkflowEngine;
  "expected_revision": number;
  "name": string;
  "project_id": string;
  "publish"?: boolean;
  "request_id": string;
  "source_entity_id"?: string | null;
  "source_revision"?: number | null;
};

export type AnswerMcpTaskInteraction = {
  "action": string;
  "content"?: unknown;
  "edited_messages"?: Array<unknown> | null;
  "expected_revision": number;
  "idempotency_key": string;
};

export type ApiError = {
  "code": string;
  "correlation_id": string;
  "message": string;
  "retryable": boolean;
};

export type ApplicationPreferences = {
  "auto_approve_safe_tools"?: boolean;
  "auto_backup_db"?: boolean;
  "auto_pilot_all_tools"?: boolean;
  "chat_retention_days"?: number;
  "command_blacklist"?: string;
  "command_whitelist"?: string;
  "compact_mode"?: boolean;
  "confirm_on_close_with_running_tasks"?: boolean;
  "db_backup_interval_hours"?: number;
  "db_cleanup_on_start"?: boolean;
  "default_access_mode"?: AccessMode;
  "default_agent_id"?: string | null;
  "default_budgets"?: Budgets;
  "default_personality_id"?: string | null;
  "default_provider_id"?: string | null;
  "default_start_view"?: string;
  "fallback_to_human_on_repeated_failure"?: boolean;
  "focus_mode"?: boolean;
  "font_scale"?: number;
  "language"?: string;
  "launch_at_startup"?: boolean;
  "limit_thinking_window"?: boolean;
  "max_tool_calls_per_loop"?: number;
  "mcp_allow_manual_import"?: boolean;
  "mcp_auto_reconnect"?: boolean;
  "mcp_env_editor_enabled"?: boolean;
  "mcp_verbose_logging"?: boolean;
  "notifications_enabled"?: boolean;
  "ollama_stream_autosave"?: boolean;
  "provider_defaults"?: {
  [key: string]: string;
};
  "read_only_fs_mode"?: boolean;
  "shortcut_overlay_enabled"?: boolean;
  "show_timestamps"?: boolean;
  "sounds_enabled"?: boolean;
  "super_verbose_audit_logging"?: boolean;
  "sync_theme_with_system"?: boolean;
  "system_prompt"?: string;
  "task_batch_multi_select_enabled"?: boolean;
  "telemetry_enabled"?: boolean;
  "terminal_persistence_mode"?: string;
  "theme"?: string;
  "verbose_mode"?: boolean;
  "workspace_default_path"?: string;
};

export type Approval = {
  "decision"?: boolean | null;
  "fingerprint": string;
  "id": string;
  "policy_revision": number;
  "run_id": string;
  "target": string;
  "tool": ToolCall;
};

export type ApproveOfflineTools = {
  "approve": boolean;
  "expected_revision": number;
  "request_sha256": Array<string>;
};

export type ApproveTeamTool = {
  "approved": boolean;
  "request_sha256": string;
};

export type ArchiveChat = {
  "archived": boolean;
  "expected_revision": number;
  "idempotency_key": string;
};

export type ArchiveWorkspaceTask = {
  "expected_revision": number;
  "idempotency_key": string;
};

export type Artifact = {
  "content": string;
  "created_at": number;
  "id": string;
  "media_type": string;
  "name": string;
  "order_id": string;
  "run_id": string;
  "sha256": string;
  "size": number;
};

export type AuditArchiveBundle = {
  "format": string;
  "pages": Array<SignedAuditArchive>;
};

export type AuditCheckpointRequest = {
  "archive": AuditArchiveBundle;
  "binding": AuditWitnessBinding;
  "captured_at": number;
  "format": string;
  "previous_checkpoint_sha256"?: string | null;
  "public_key": string;
  "public_key_sha256": string;
  "request_id": string;
  "signature": string;
};

export type AuditCheckpointStatus = {
  "binding"?: AuditWitnessBinding | null;
  "current_chain_matches": boolean;
  "current_prefix_matches": boolean;
  "latest"?: SignedAuditCheckpoint | null;
  "latest_sha256"?: string | null;
  "pending_request_id"?: string | null;
  "pending_transition_request_id"?: string | null;
  "trust_transitions"?: Array<SignedAuditTrustTransition>;
};

export type AuditCost = {
  "basis": string;
  "currency"?: string | null;
  "price_source_sha256"?: string | null;
  "scale": number;
  "units": string;
};

export type AuditCount = {
  "category": string;
  "count": number;
};

export type AuditDiagnostics = {
  "captured_at": number;
  "format": string;
  "insights": AuditInsights;
  "redaction": string;
};

export type AuditExecutionInsights = {
  "comparison_from_ms": number;
  "current": AuditExecutionTotals;
  "facts": Array<AuditMetricFact>;
  "from_ms": number;
  "has_more": boolean;
  "next_sequence": number;
  "previous": AuditExecutionTotals;
  "snapshot_sequence": number;
  "until_ms": number;
};

export type AuditExecutionTotals = {
  "complete": boolean;
  "costs": Array<AuditCost>;
  "duration_ms": string;
  "facts": string;
  "incomplete_usage_receipts": string;
  "input_tokens": string;
  "missing_cost_model_receipts": string;
  "model_duration_ms": string;
  "model_receipts": string;
  "non_execution_timestamp_receipts": string;
  "output_tokens": string;
  "run_receipts": string;
  "skill_context_uses": string;
  "unknown_duration_runs": string;
  "unknown_model_duration_receipts": string;
  "unknown_skill_model_receipts": string;
};

export type AuditInsights = {
  "categories": Array<AuditCount>;
  "execution"?: AuditExecutionInsights | null;
  "knowledge_documents"?: number | null;
  "messages"?: number | null;
  "orders"?: number | null;
  "projects"?: number | null;
  "public_key_sha256"?: string | null;
  "recent_events": Array<AuditRecord>;
  "retired_chains"?: Array<AuditRecoveryReceipt>;
  "run_states": Array<AuditCount>;
  "runs"?: number | null;
  "scope": string;
  "skills"?: number | null;
  "total_events": number;
  "verification": AuditVerification;
};

export type AuditInsightsQuery = {
  "after_sequence"?: number;
  "from_ms"?: number | null;
  "limit"?: number;
  "snapshot_sequence"?: number | null;
  "until_ms"?: number | null;
};

export type AuditMetricFact = {
  "costs": Array<AuditCost>;
  "duration_ms"?: string | null;
  "id": string;
  "input_tokens"?: string | null;
  "kind": string;
  "observed_at": number;
  "output_tokens"?: string | null;
  "request_sha256"?: string | null;
  "run_id": string;
  "scope": string;
  "sequence": number;
  "skill_provenance_complete": boolean;
  "skills": Array<AuditSkillUse>;
  "source_kind": string;
  "source_sha256": string;
  "time_basis": string;
  "usage_complete": boolean;
};

export type AuditOrigin = "native_v2" | "legacy_unsigned" | "preserved_archive";

export type AuditOriginRotationPlan = {
  "format": string;
  "instance_id": string;
  "next_audit_key": string;
  "next_epoch": number;
  "next_origin_signature": string;
  "next_public_key": string;
  "next_public_key_sha256": string;
  "original_audit_key": string;
  "original_epoch": number;
  "original_registry_sha256": string;
  "prepared_at": number;
  "previous_origin_signature": string;
  "previous_public_key": string;
  "previous_public_key_sha256": string;
  "proposals": Array<AuditTrustTransitionProposal>;
  "request_id": string;
};

export type AuditOriginalReference = {
  "content_sha256": string;
  "request_id": string;
};

export type AuditPage = {
  "after_sequence": number;
  "chain_id": string;
  "complete": boolean;
  "events": Array<AuditRecord>;
  "format": string;
  "next_sequence": number;
  "previous_sha256": string;
  "scope": string;
  "through_sequence": number;
  "through_sha256": string;
};

export type AuditRecord = {
  "action": string;
  "actor_id"?: string | null;
  "event_id": string;
  "observed_at": number;
  "origin": AuditOrigin;
  "previous_sha256": string;
  "sequence": number;
  "sha256": string;
  "source_sha256"?: string | null;
  "target_id"?: string | null;
};

export type AuditRecoveryPlan = {
  "bytes": number;
  "corrupt": boolean;
  "prior_chain_id"?: string | null;
  "prior_sequence": string;
  "public_key_sha256"?: string | null;
  "records": number;
  "scope": string;
  "snapshot_sha256": string;
  "verification_complete": boolean;
};

export type AuditRecoveryReceipt = {
  "actor_id"?: string | null;
  "captured_at": number;
  "format": string;
  "new_chain_id": string;
  "original": AuditRecoveryPlan;
  "public_key": string;
  "public_key_sha256": string;
  "request_id": string;
  "scope": string;
  "signature": string;
};

export type AuditSkillUse = {
  "id": string;
  "instructions_sha256": string;
  "revision": number;
};

export type AuditTrustTransitionProposal = {
  "format": string;
  "next_binding": AuditWitnessBinding;
  "next_origin_signature": string;
  "prepared_at": number;
  "previous_binding": AuditWitnessBinding;
  "previous_checkpoint": SignedAuditCheckpoint;
  "previous_origin_signature": string;
  "request_id": string;
};

export type AuditV1Assessment = {
  "chain_complete": boolean;
  "error_code"?: string | null;
  "first_problem_line"?: number | null;
  "first_sequence"?: string | null;
  "last_sequence"?: string | null;
  "legacy_events": number;
  "signed_events": number;
  "valid": boolean;
};

export type AuditV1Verification = {
  "archives": Array<AuditOriginalReference>;
  "assessment": AuditV1Assessment;
  "captured_at": number;
  "format": string;
  "key_source": string;
  "public_key": string;
  "public_key_sha256": string;
  "request_id": string;
  "scope": string;
  "signature": string;
};

export type AuditVerification = {
  "chain_id": string;
  "checked_events": number;
  "complete": boolean;
  "first_problem_sequence"?: number | null;
  "head_sequence": number;
  "head_sha256": string;
  "legacy_unsigned_events": number;
  "preserved_archives": number;
  "scope": string;
  "valid": boolean;
};

export type AuditWitnessBinding = {
  "actor_id"?: string | null;
  "binding_id": string;
  "configured_at": number;
  "format": string;
  "origin_id": string;
  "origin_public_key_sha256": string;
  "public_key": string;
  "scope": string;
  "signature": string;
  "witness_public_key_sha256": string;
};

export type AutomationContract = {
  "configure_webhook": ConfigureAutomationWebhook;
  "event": AutomationEvent;
  "events": AutomationEventPage;
  "file_watch_status": AutomationFileWatchStatus;
  "preview": AutomationEventPreview;
  "retire_event": RetireAutomationEvent;
  "submit_event": SubmitAutomationEvent;
  "trigger": AutomationTrigger;
  "webhook_grant": AutomationWebhookGrant;
  "write_trigger": WriteAutomationTrigger;
};

export type AutomationEvent = {
  "data": unknown;
  "event_id": string;
  "id": string;
  "project_id": string;
  "reason"?: string | null;
  "received_at": number;
  "run_id"?: string | null;
  "source": AutomationEventSource;
  "state": AutomationEventState;
  "task_id": string;
  "task_revision": number;
  "trigger_id": string;
  "trigger_revision": number;
  "updated_at": number;
  "workflow_input"?: AutomationWorkflowInputSnapshot | null;
};

export type AutomationEventPage = {
  "items": Array<AutomationEvent>;
  "next_cursor"?: number | null;
};

export type AutomationEventPreview = {
  "input_error"?: string | null;
  "matches": boolean;
  "reason"?: string | null;
  "task_revision"?: number | null;
  "trigger_revision"?: number | null;
  "workflow_input"?: AutomationWorkflowInputSnapshot | null;
  "would_queue": boolean;
};

export type AutomationEventSource = "manual" | "webhook" | "file";

export type AutomationEventState = "queued" | "filtered" | "paused" | "dispatching" | "started" | "blocked" | "discarded";

export type AutomationFileWatch = {
  "changes": Array<FileWatchChange>;
  "debounce_ms": number;
  "exclude_paths": Array<string>;
  "extensions": Array<string>;
  "folder": FolderGrant;
  "include_existing": boolean;
  "max_file_bytes": number;
  "max_files": number;
  "max_scan_bytes": number;
  "path": string;
  "poll_interval_ms": number;
  "recursive": boolean;
};

export type AutomationFileWatchStatus = {
  "ignored_entries": number;
  "initialized": boolean;
  "last_scan_at"?: number | null;
  "next_scan_at": number;
  "pending_changes": number;
  "reason"?: string | null;
  "scanning": boolean;
  "tracked_files": number;
  "trigger_id": string;
  "trigger_revision": number;
};

export type AutomationTrigger = {
  "created_at": number;
  "enabled": boolean;
  "file_watch"?: AutomationFileWatch | null;
  "id": string;
  "name": string;
  "predicates": Array<EventPredicate>;
  "project_id": string;
  "revision": number;
  "source": AutomationEventSource;
  "task_id": string;
  "task_revision": number;
  "updated_at": number;
  "workflow_inputs"?: AutomationWorkflowInputs | null;
};

export type AutomationWebhookGrant = {
  "enabled": boolean;
  "expires_at": number;
  "revision": number;
  "trigger_id": string;
};

export type AutomationWorkflowInputSnapshot = {
  "definition_sha256": string;
  "mapping_sha256": string;
  "values": unknown;
  "version_id": string;
  "workflow_id": string;
};

export type AutomationWorkflowInputs = {
  "bindings": {
  [key: string]: string;
};
  "format_version": number;
};

export type BeginHistoricalImport = {
  "expected_limit_revision": number;
  "files": Array<HistoricalFileDeclaration>;
  "pins": HistoricalPinSet;
  "preview_format"?: string | null;
  "request_id": string;
};

export type BranchOrder = {
  "idempotency_key": string;
  "through_message_id": string;
  "title": string;
};

export type BrokerControllerRef = {
  "generation": number;
  "id": string;
};

export type BrowserCapabilities = {
  "developer_tools": boolean;
  "keyboard": boolean;
  "live_view": boolean;
  "pointer": boolean;
  "selected_tabs_only": boolean;
};

export type BrowserCommand = {
  "document_id"?: string | null;
  "height"?: number | null;
  "operation": string;
  "selector"?: string | null;
  "tab_id"?: string | null;
  "text"?: string | null;
  "url"?: string | null;
  "width"?: number | null;
  "x"?: number | null;
  "y"?: number | null;
};

export type BrowserEngine = "chrome" | "firefox";

export type BrowserExtensionConnection = {
  "connected": boolean;
  "engine": BrowserEngine;
  "id": string;
  "label": string;
  "tabs": Array<BrowserTab>;
};

export type BrowserInput = {
  "command": BrowserCommand;
  "epoch": string;
  "idempotency_key": string;
};

export type BrowserMode = "managed" | "extension";

export type BrowserSessionView = {
  "capabilities": BrowserCapabilities;
  "connected": boolean;
  "connection_id"?: string | null;
  "device_id": string;
  "engine": BrowserEngine;
  "epoch": string;
  "id": string;
  "manual": boolean;
  "mode": BrowserMode;
  "order_id": string;
  "project_id": string;
};

export type BrowserTab = {
  "active": boolean;
  "document_id": string;
  "id": string;
  "title": string;
  "url": string;
};

export type BrowserTakeover = {
  "manual": boolean;
};

export type Budgets = {
  "max_output_tokens": number;
  "max_seconds": number;
  "max_steps": number;
  "max_total_tokens"?: number;
};

export type BuildHistoricalHistoryIndex = {
  "expected_import_revision": number;
  "expected_limit_revision": number;
  "request_id": string;
};

export type BuildKnowledgeGraph = {
  "folder_id": string;
  "include_descendants": boolean;
};

export type CaptureAuditArchive = {
  "after_sequence"?: number;
  "limit": number;
  "request_id": string;
  "through_sequence"?: number | null;
};

export type CaptureAuditCheckpoint = {
  "binding_id": string;
  "previous_checkpoint_sha256"?: string | null;
  "request_id": string;
};

export type ChatConfiguration = {
  "order_id": string;
  "revision": number;
  "updated_at": number;
  "value"?: ChatRunConfiguration | null;
};

export type ChatManagement = {
  "archived": boolean;
  "order": Order;
};

export type ChatRunConfiguration = {
  "access_mode": AccessMode;
  "agent_config": EffectiveAgentConfig;
  "budgets": Budgets;
  "environment"?: ExecutionEnvironmentRef | null;
  "folders"?: Array<FolderGrant>;
  "mcp_tools"?: Array<McpToolSelection>;
  "model_binding"?: ModelRegistryRef | null;
  "native_tools"?: Array<string>;
  "output"?: StructuredOutput | null;
  "provider_id"?: string | null;
  "provider_revision"?: number | null;
  "workflow"?: ChatWorkflowSelection | null;
};

export type ChatWorkflowSelection = {
  "inputs"?: unknown;
  "version_id": string;
  "workflow_id": string;
};

export type CloseHistoricalUnsentFile = {
  "expected_request_sha256": string;
};

export type CodexAccount = {
  "created_at": number;
  "id": string;
  "revision": number;
  "updated_at": number;
  "value": CodexAccountDefinition;
};

export type CodexAccountAction = {
  "expected_revision": number;
  "idempotency_key": string;
};

export type CodexAccountDefinition = {
  "name"?: string;
  "priority"?: number;
};

export type CodexAccountStatus = {
  "account": CodexAccount;
  "email"?: string | null;
  "plan_type"?: string | null;
  "rate_limits": unknown;
  "status": string;
};

export type CodexAccountsContract = {
  "account": CodexAccount;
  "action": CodexAccountAction;
  "login_request": CodexLoginRequest;
  "login_view": CodexLoginView;
  "materialize_model_profile": MaterializeCodexModelProfile;
  "model": CodexModel;
  "runtime": CodexRuntimeStatus;
  "status": CodexAccountStatus;
  "write": WriteCodexAccount;
};

export type CodexLoginFlow = "browser" | "device";

export type CodexLoginRequest = {
  "expected_revision": number;
  "flow": CodexLoginFlow;
  "idempotency_key": string;
};

export type CodexLoginView = {
  "authorization_url"?: string | null;
  "flow": CodexLoginFlow;
  "login_id": string;
  "user_code"?: string | null;
  "verification_url"?: string | null;
};

export type CodexModel = {
  "default_reasoning_effort"?: string | null;
  "id": string;
  "name": string;
  "reasoning_efforts": Array<string>;
};

export type CodexRuntimeStatus = {
  "available": boolean;
  "configured": boolean;
  "error_code"?: string | null;
  "protocol_schema": string;
  "version": string;
};

export type CommitExecutionPlan = {
  "sha256": string;
};

export type CompactModelContext = {
  "idempotency_key": string;
  "remove_groups": Array<string>;
  "request": CreateRun;
  "snapshot_sha256": string;
  "summary"?: string | null;
};

export type ConfigureAuditWitness = {
  "independently_confirmed": boolean;
  "origin_id": string;
  "origin_public_key_sha256": string;
  "request_id": string;
  "witness_public_key_sha256": string;
};

export type ConfigureAutomationWebhook = {
  "enabled": boolean;
  "expected_revision": number;
  "expires_at": number;
  "idempotency_key": string;
  "token"?: string | null;
};

export type ConfigureHistoricalImportLimit = {
  "expected_revision": number;
  "limit_bytes": number;
};

export type ConfigureMcpOAuth = {
  "acknowledge_unknown"?: boolean;
  "client_secret"?: string | null;
  "configuration": McpOAuthConfiguration;
  "expected_revision": number;
  "idempotency_key": string;
  "metadata_sha256": string;
  "profile_id": string;
  "server_id": string;
  "server_revision": number;
};

export type ConfigureMcpServer = {
  "base_revision": number;
  "configuration": McpServerConfiguration;
  "idempotency_key": string;
  "server_id": string;
};

export type ConfigureOfflineCredential = {
  "api_key"?: string | null;
  "explicitly_allow_no_key": boolean;
  "grant_id": string;
};

export type ConfigureOfflineTools = {
  "allowed_tools": Array<string>;
  "explicitly_allow_native_owner_rights": boolean;
  "github_token"?: string | null;
  "grant_id": string;
  "mcp_connections": Array<OfflineMcpConnectionInput>;
  "programs": Array<string>;
};

export type ConfigureOwnerMcpCredential = {
  "bearer": string;
  "endpoint": string;
  "expected_revision": number;
  "project_id": string;
  "server_id": string;
};

export type ConfigureOwnerMcpStdio = {
  "arguments": Array<string>;
  "cwd"?: string | null;
  "environment"?: {
  [key: string]: string;
} | null;
  "expected_revision"?: number;
  "program": string;
  "project_id": string;
  "server_id": string;
};

export type ConfigurePersonalProvider = {
  "api_key"?: string | null;
  "explicitly_allow_no_key": boolean;
  "idempotency_key": string;
  "profile": CreateProvider;
};

export type ConfigureTeamGithub = {
  "enabled": boolean;
  "expected_revision": number;
  "hostname": string;
  "token": string;
};

export type ConfigureTeamMcpServer = {
  "bearer"?: string | null;
  "enabled": boolean;
  "endpoint": string;
  "expected_revision": number;
  "id"?: string | null;
  "name": string;
};

export type ConfigureTeamProvider = {
  "api_key": string;
  "base_url": string;
  "expected_revision": number;
  "model": string;
};

export type ConfirmAuditRecovery = {
  "preserve_original_and_start_new_chain": boolean;
  "request_id": string;
  "snapshot_sha256": string;
};

export type ConfirmOwnerManagedResultDelivery = {
  "id": string;
  "request_sha256": string;
};

export type ContextArchiveView = {
  "original": unknown;
  "receipt": ContextCompactionView;
  "retained": unknown;
};

export type ContextCompactionView = {
  "count_mode": string;
  "created_at": number;
  "id": string;
  "input_tokens"?: number | null;
  "model_binding"?: ModelRegistryRef | null;
  "order_id": string;
  "original_sha256": string;
  "removed_groups": Array<string>;
  "retained_sha256": string;
  "revision": number;
  "summary"?: string | null;
};

export type ContextGroupView = {
  "characters": number;
  "id": string;
  "message_count": number;
  "preview": string;
  "required": boolean;
  "sha256": string;
};

export type CreateBrowserSession = {
  "connection_id"?: string | null;
  "engine"?: BrowserEngine;
  "mode"?: BrowserMode;
};

export type CreateImportRun = {
  "access_mode"?: AccessMode;
  "base_revision": number;
  "idempotency_key": string;
  "import": ImportFile;
  "order_id": string;
  "prompt": string;
};

export type CreateOrder = {
  "project_id": string;
  "title": string;
};

export type CreateProject = {
  "name": string;
};

export type CreateProvider = {
  "capabilities"?: ModelCapabilities;
  "context_tokens": number;
  "endpoint": string;
  "kind": ProviderKind;
  "model": string;
  "name": string;
};

export type CreateRestoreRun = {
  "access_mode"?: AccessMode;
  "base_revision": number;
  "idempotency_key": string;
  "order_id": string;
  "prompt": string;
  "restore": RestoreFile;
};

export type CreateRun = {
  "access_mode"?: AccessMode;
  "agent_config"?: EffectiveAgentConfig | null;
  "base_revision": number;
  "budgets"?: Budgets;
  "environment"?: ExecutionEnvironmentRef | null;
  "folders"?: Array<FolderGrant>;
  "idempotency_key": string;
  "mcp_tools"?: Array<McpToolSelection>;
  "model_binding"?: ModelRegistryRef | null;
  "native_tools"?: Array<string>;
  "order_id": string;
  "output"?: StructuredOutput | null;
  "prompt": string;
  "provider_id": string;
  "provider_revision": number;
};

export type CreateSandboxPreview = {
  "environment": ExecutionEnvironmentRef;
  "expected_sandbox_uid": string;
  "port": number;
};

export type CreateTeam = {
  "idempotency_key": string;
  "name": string;
};

export type CreateTeamOrder = {
  "idempotency_key": string;
  "prompt": string;
  "title": string;
};

export type CreateTeamProject = {
  "idempotency_key": string;
  "name": string;
};

export type CreateTeamRun = {
  "idempotency_key": string;
  "order_id": string;
  "plan"?: TeamRunPlan;
  "prompt": string;
};

export type CreateWorkflow = {
  "definition": WorkflowDefinition;
  "idempotency_key": string;
};

export type DatabaseBackup = {
  "bytes": number;
  "created_at": number;
  "format": string;
  "id": string;
  "sha256": string;
};

export type DatabaseBackupSaveOutcome = {
  "state": "canceled";
} | {
  "bytes": number;
  "sha256": string;
  "state": "saved";
};

export type DecimalU64 = string;

export type DeleteActorContext = {
  "expected_revision": number;
  "request_id": string;
};

export type DeleteBrowserProfile = {
  "engine": BrowserEngine;
};

export type DeleteExternalMemoryProvider = {
  "expected_revision": number;
  "request_id": string;
};

export type DeleteLibraryEntry = {
  "expected_revision": number;
  "idempotency_key": string;
};

export type DeleteOwnerConnector = {
  "expected_revision": number;
  "idempotency_key": string;
};

export type DeleteProviderSettings = {
  "expected_revision": number;
  "idempotency_key": string;
};

export type DeviceCapability = {
  "attributes"?: {
  [key: string]: unknown;
};
  "name": string;
  "schema_version": number;
  "version": string;
};

export type DeviceCredentialMetadata = {
  "executor_id": string;
  "expires_at": string;
  "id": string;
  "label": string;
  "revision": number;
  "revoked": boolean;
};

export type DeviceCrewAgent = {
  "agent_config"?: EffectiveAgentConfig | null;
  "allow_delegation"?: boolean;
  "backstory"?: string;
  "goal": string;
  "id": string;
  "mcp_server_names"?: Array<string>;
  "model"?: string | null;
  "role": string;
  "tools"?: Array<string>;
};

export type DeviceCrewDefinition = {
  "agents": Array<DeviceCrewAgent>;
  "format_version": number;
  "manager_agent_id"?: string | null;
  "name": string;
  "process": string;
  "tasks": Array<DeviceCrewTask>;
};

export type DeviceCrewTask = {
  "agent_id": string;
  "async_execution"?: boolean;
  "context"?: Array<string>;
  "description": string;
  "expected_output": string;
  "id": string;
};

export type DeviceExecutorRegistration = {
  "capabilities": Array<DeviceCapability>;
  "display_name": string;
  "executor_id": string;
  "kind": string;
  "labels"?: {
  [key: string]: string;
};
  "max_concurrent_runs": number;
  "owner_user_id"?: string | null;
  "personal_device_remote_control"?: string | null;
  "pool_id"?: string | null;
  "protocol_version": number;
  "schema_version": number;
};

export type DeviceFeatureConfiguration = {
  "agent_protocol_version": number;
  "fcm_enabled": boolean;
  "fleet_enabled": boolean;
  "minimum_compatible_schema_version": number;
  "origin"?: string | null;
  "passkeys_enabled": boolean;
  "push_delivery": string;
  "rp_id"?: string | null;
  "schema_version": number;
};

export type DeviceInteropAdapter = {
  "configured": boolean;
  "name": string;
  "operations": Array<string>;
  "version": string;
};

export type DeviceInteropInventory = {
  "adapters": Array<DeviceInteropAdapter>;
  "adopted_processes": boolean;
  "automatic_replay": boolean;
  "format": string;
};

export type DeviceOfficeRequest = {
  "action"?: string;
  "application": string;
  "output": string;
  "parameters"?: unknown;
  "preview_output"?: string | null;
  "source": string;
};

export type DevicePairingRecord = {
  "active_runs": number;
  "draining": boolean;
  "last_seen_at": string;
  "online": boolean;
  "registration": unknown;
  "revision": number;
  "revoked": boolean;
  "schema_version": number;
  "team_id"?: string | null;
};

export type DevicePasskeyChallenge = {
  "challenge_id": string;
  "expires_at": string;
  "public_key": unknown;
  "schema_version": number;
};

export type DevicePasskeyRecord = {
  "created_at": string;
  "id": string;
  "label": string;
  "last_used_at"?: string | null;
  "schema_version": number;
};

export type DevicePushSubscription = {
  "created_at": string;
  "device_id": string;
  "id": string;
  "last_success_at"?: string | null;
  "provider": string;
  "schema_version": number;
};

export type DirectoryListing = {
  "entries": Array<FileEntry>;
  "folder_id": string;
  "path": string;
  "truncated": boolean;
};

export type DiscoverProviderModels = {
  "api_key"?: string | null;
  "endpoint": string;
  "explicitly_allow_no_key"?: boolean;
  "kind": ProviderKind;
};

export type DocumentActionRequest = {
  "access_mode": AccessMode;
  "base_revision": number;
  "idempotency_key": string;
  "tool": ToolCall;
  "version": number;
};

export type DocumentDraft = {
  "content": unknown;
  "id": string;
  "order_id": string;
  "revision": number;
  "sha256": string;
  "updated_at": number;
};

export type DocumentDraftSave = {
  "draft": DocumentDraft;
  "saved": boolean;
  "snapshot": DocumentDraftSnapshot;
};

export type DocumentDraftSnapshot = {
  "base_revision": number;
  "conflict": boolean;
  "content": unknown;
  "created_at": number;
  "draft_id": string;
  "id": string;
  "order_id": string;
  "sha256": string;
};

export type DocumentPreparation = {
  "access_mode": AccessMode;
  "base_revision": number;
  "created_at": number;
  "engine_revision": string;
  "fingerprint": string;
  "id": string;
  "idempotency_key": string;
  "order_id": string;
  "requires_approval": boolean;
  "run_id"?: string | null;
  "tool": ToolCall;
};

export type DocumentProvenance = {
  "engine_version": string;
  "inputs": Array<DocumentProvenanceInput>;
  "operation": string;
  "receipt_sha256": string;
  "tool": string;
};

export type DocumentProvenanceInput = {
  "folder_id": string;
  "path": string;
  "sha256": string;
};

export type DownloadFile = {
  "expected_sha256"?: string | null;
  "path": string;
};

export type DownloadMetadata = {
  "file_name": string;
  "sha256": string;
  "size": number;
};

export type EffectiveAgentConfig = {
  "access_mode"?: AccessMode | null;
  "agent_source"?: LibrarySource | null;
  "behavior"?: AgentBehaviorSettings | null;
  "execution_mode"?: AgentExecutionMode | null;
  "generation"?: GenerationSettings;
  "instructions"?: string;
  "memory_ids"?: Array<string> | null;
  "personality_source"?: LibrarySource | null;
  "skill_ids"?: Array<string> | null;
  "tool_filter"?: Array<string> | null;
};

export type EventPredicate = {
  "operator": "exists";
  "pointer": string;
} | {
  "operator": "equals";
  "pointer": string;
  "value": unknown;
} | {
  "operator": "contains";
  "pointer": string;
  "text": string;
} | {
  "operator": "greater_than";
  "pointer": string;
  "value": number;
} | {
  "operator": "less_than";
  "pointer": string;
  "value": number;
};

export type ExecutionActor = {
  "kind": "installation_owner";
} | {
  "binding": TeamExecutionActor;
  "kind": "team_session";
};

export type ExecutionBoundary = {
  "connector_aliases": Array<string>;
  "environment"?: ExecutionEnvironmentRef | null;
  "executor": string;
  "folders": Array<FolderGrant>;
  "input_snapshot_bound": boolean;
  "process_access": ExecutionProcessAccess;
  "process_network": ExecutionNetworkAccess;
};

export type ExecutionDestinationV2 = {
  "controller": BrokerControllerRef;
  "kind": "broker_controller";
} | {
  "controller": BrokerControllerRef;
  "engine_revision": string;
  "kind": "document_engine";
} | {
  "controller": BrokerControllerRef;
  "kind": "managed_runner";
  "programs": {
  [key: string]: string;
};
  "runner": ExecutionRunnerIdentity;
  "target": ExecutionTargetRef;
};

export type ExecutionEnvironmentRef = {
  "definition_sha256": string;
  "environment_id": string;
  "qualification_sha256": string;
  "version_id": string;
};

export type ExecutionInputRef = {
  "manifest_sha256": string;
  "sha256": string;
  "snapshot_id": string;
};

export type ExecutionInputSnapshot = {
  "created_at": number;
  "format_version": number;
  "id": string;
  "manifest": WorkspaceTransferManifest;
  "order_id": string;
  "order_revision": number;
  "project_id": string;
  "request_sha256": string;
  "sources": Array<ExecutionInputSource>;
};

export type ExecutionInputSource = {
  "alias": string;
  "name": string;
};

export type ExecutionNetworkAccess = {
  "kind": "no_process";
} | {
  "kind": "user_rights";
} | {
  "kind": "denied";
} | {
  "kind": "web_proxy";
  "policy_id": string;
};

export type ExecutionPlan = {
  "actor": string;
  "boundary": ExecutionBoundary;
  "created_at": number;
  "expires_at": number;
  "format_version": number;
  "id": string;
  "idempotency_key": string;
  "order_id": string;
  "preflight": RunPreflight;
  "project_id": string;
  "request_sha256": string;
  "spec_sha256": string;
};

export type ExecutionPlanV2 = {
  "actor": ExecutionActor;
  "boundary": ExecutionBoundary;
  "created_at": number;
  "expires_at": number;
  "format_version": number;
  "id": string;
  "idempotency_key": string;
  "input": ExecutionInputSnapshot;
  "order_id": string;
  "preflight": ExecutionPreflightV2;
  "project_id": string;
  "request_sha256": string;
  "spec_sha256": string;
  "target": ExecutionDestinationV2;
  "workspace": RunWorkspaceBinding;
};

export type ExecutionPreflightV2 = {
  "kind": "import";
  "review": ImportExecutionPreflightV2;
} | {
  "kind": "restore";
  "review": RestoreExecutionPreflightV2;
} | {
  "kind": "owner";
  "review": RunPreflight;
} | {
  "kind": "team";
  "review": TeamExecutionPreflight;
};

export type ExecutionProcessAccess = "broker_only" | "user_rights" | "qualified_environment";

export type ExecutionRunnerIdentity = {
  "adapter_build_sha256": string;
  "control_public_key": Array<number>;
  "generation": number;
  "runner_id": string;
  "runner_public_key": Array<number>;
};

export type ExecutionTargetRef = {
  "identity_sha256": string;
  "revision": number;
  "target_id": string;
};

export type ExternalMemoryDefinition = {
  "auth_scheme": string;
  "content_field": string;
  "endpoint": string;
  "id_field"?: string | null;
  "items_pointer": string;
  "method": string;
  "name": string;
  "request_body"?: unknown;
};

export type ExternalMemoryItem = {
  "content": string;
  "id": string;
};

export type ExternalMemoryProvider = {
  "has_secret": boolean;
  "id": string;
  "revision": number;
  "value": ExternalMemoryDefinition;
};

export type ExternalMemoryRetrieval = {
  "endpoint": string;
  "items": Array<ExternalMemoryItem>;
  "provider_id": string;
  "provider_revision": number;
  "request_id": string;
  "response_sha256": string;
  "retrieved_at": number;
};

export type FileChange = {
  "after_sha256": string;
  "before_sha256"?: string | null;
  "folder_id": string;
  "path": string;
  "run_id": string;
  "size": number;
  "tool_call_id": string;
};

export type FileEntry = {
  "kind": string;
  "name": string;
  "path": string;
  "size": number;
};

export type FileLocation = {
  "folder_id": string;
  "path": string;
};

export type FileNode = {
  "kind": FileNodeKind;
  "resource_id": string;
  "sha256": string;
  "size": number;
};

export type FileNodeKind = "file" | "directory";

export type FileOperation = {
  "after"?: FileNode | null;
  "before"?: FileNode | null;
  "destination"?: string | null;
  "entries": Array<WorkspaceEntry>;
  "folder_id": string;
  "generated_from"?: DocumentProvenance | null;
  "imported_from"?: FileUpload | null;
  "kind": FileOperationKind;
  "path": string;
  "restored_from"?: FileRestoreSource | null;
  "run_id": string;
  "tool_call_id": string;
};

export type FileOperationKind = "create_directory" | "rename" | "remove" | "restore" | "import" | "generate";

export type FileRestoreSource = {
  "run_id": string;
  "tool_call_id": string;
  "version": RetainedVersion;
};

export type FileStat = {
  "folder_id": string;
  "node"?: FileNode | null;
  "path": string;
};

export type FileUpload = {
  "created_at": number;
  "file": DownloadMetadata;
  "id": string;
  "order_id": string;
  "project_id": string;
  "revision": number;
  "state": UploadState;
};

export type FileWatchChange = "created" | "modified" | "removed";

export type FinishDevicePasskeyAuthentication = {
  "challenge_id": string;
  "credential": unknown;
};

export type FinishDevicePasskeyRegistration = {
  "challenge_id": string;
  "credential": unknown;
  "label": string;
};

export type FolderGrant = {
  "id": string;
  "name": string;
  "order_id"?: string | null;
  "project_id": string;
  "resource_id": string;
  "revision": string;
  "writable": boolean;
};

export type FolderRequestLookup = {
  "folder"?: RegisteredFolder | null;
};

export type GenerationSettings = {
  "temperature"?: number | null;
  "timeout_ms"?: number | null;
};

export type Health = {
  "capabilities": Array<string>;
  "deployment": string;
  "product": string;
  "schema_version": number;
  "version": string;
};

export type HistoricalActivatedObject = {
  "id": string;
  "kind": string;
  "project_id": string;
  "source_id": string;
};

export type HistoricalActivationReceipt = {
  "activated_at": number;
  "catalog_sha256"?: string | null;
  "charged_bytes": number;
  "import_id": string;
  "objects": Array<HistoricalActivatedObject>;
  "owner": HistoricalOwner;
  "plan_sha256": string;
  "preview_id": string;
  "remainder_history_only_reason"?: string | null;
  "remainder_parts"?: number | null;
  "source_sha256": string;
};

export type HistoricalChunkReceipt = {
  "chunk_index": number;
  "import_id": string;
  "sha256": string;
  "size": number;
  "stream": HistoricalStream;
};

export type HistoricalContentArtifact = {
  "content": string;
  "created_at": number;
  "id": string;
  "media_type": string;
  "name": string;
  "sha256": string;
  "size": number;
  "source_id"?: string | null;
};

export type HistoricalContentCatalogEntry = {
  "content_bytes": number;
  "pages": Array<HistoricalContentPage>;
  "source": HistoricalContentProject;
  "unsupported_reason"?: string | null;
};

export type HistoricalContentCatalogItem = {
  "content_bytes": number;
  "documents": number;
  "files": number;
  "metadata_pages": Array<HistoricalContentPage>;
  "name": string;
  "orders": number;
  "ordinal": number;
  "source_id": string;
  "unsupported_reason"?: string | null;
};

export type HistoricalContentCatalogManifest = {
  "count": number;
  "format": string;
  "sha256": string;
  "source_sha256": string;
};

export type HistoricalContentCatalogPage = {
  "activated_parts"?: {
  [key: string]: string;
} | null;
  "editable_selection_closed"?: boolean | null;
  "entries": Array<HistoricalContentCatalogItem>;
  "manifest": HistoricalContentCatalogManifest;
  "next_offset"?: number | null;
  "offset": number;
};

export type HistoricalContentDocument = {
  "content": string;
  "source_id": string;
  "title": string;
};

export type HistoricalContentFile = {
  "media_type": string;
  "name": string;
  "reason": string;
  "sha256"?: string | null;
  "size"?: number | null;
  "source_id": string;
};

export type HistoricalContentMessage = {
  "content": string;
  "created_at": number;
  "id": string;
  "role": string;
  "source_id"?: string | null;
};

export type HistoricalContentOrder = {
  "artifacts": Array<HistoricalContentArtifact>;
  "draft": string;
  "messages": Array<HistoricalContentMessage>;
  "source_id": string;
  "source_kind": string;
  "title": string;
};

export type HistoricalContentPage = {
  "chunk_index": number;
  "sha256": string;
  "size": number;
  "stream": string;
};

export type HistoricalContentProject = {
  "documents": Array<HistoricalContentDocument>;
  "files": Array<HistoricalContentFile>;
  "name": string;
  "orders": Array<HistoricalContentOrder>;
  "source_id": string;
};

export type HistoricalFileDeclaration = {
  "sha256": string;
  "size": number;
  "stream": HistoricalStream;
};

export type HistoricalFileOperation = {
  "backend_id": string;
  "created_at": number;
  "import_id": string;
  "media_type": string;
  "name": string;
  "order_id": string;
  "owner": HistoricalOwner;
  "plan_sha256": string;
  "preview_id": string;
  "project_id": string;
  "request_sha256": string;
  "sha256": string;
  "size": number;
  "source_id": string;
  "state": string;
  "target_id": string;
};

export type HistoricalFileStageRequest = {
  "backend_id": string;
  "sha256": string;
  "size": number;
  "source_id": string;
};

export type HistoricalHistoryCell = {
  "kind": "null";
} | {
  "decimal": string;
  "kind": "integer";
} | {
  "ieee754_binary64_be": string;
  "kind": "real_bits";
} | {
  "kind": "text_bytes";
  "pages": Array<HistoricalHistoryPagePin>;
  "preview_base64": string;
  "sha256": string;
  "size": number;
} | {
  "kind": "blob";
  "pages": Array<HistoricalHistoryPagePin>;
  "preview_base64": string;
  "sha256": string;
  "size": number;
};

export type HistoricalHistoryCellsPage = {
  "cells": Array<HistoricalHistoryColumn>;
  "page_index": number;
  "position": number;
  "rowid": string;
  "table_ordinal": number;
};

export type HistoricalHistoryColumn = {
  "column_index": number;
  "value": HistoricalHistoryCell;
};

export type HistoricalHistoryIndex = {
  "built_at": number;
  "cells": number;
  "format": string;
  "import_id": string;
  "index_sha256": string;
  "original_intake_sha256": string;
  "original_record_sha256": string;
  "original_value_bytes": number;
  "owner": HistoricalOwner;
  "pins": HistoricalPinSet;
  "receipt_sha256": string;
  "request_id": string;
  "request_sha256": string;
  "reserved_bytes": number;
  "rows": number;
  "source_family": string;
  "source_profile": string;
  "tables": Array<HistoricalHistoryPagePin>;
  "verifier_contract": string;
};

export type HistoricalHistoryPagePin = {
  "sha256": string;
  "size": number;
};

export type HistoricalHistoryRow = {
  "cell_pages": Array<HistoricalHistoryPagePin>;
  "columns": number;
  "original_sha256": string;
  "position": number;
  "rowid": string;
};

export type HistoricalHistoryRowsPage = {
  "index_sha256": string;
  "next_offset"?: number | null;
  "offset": number;
  "rows": Array<HistoricalHistoryRow>;
  "table_ordinal": number;
};

export type HistoricalHistoryTable = {
  "canonical_bytes": number;
  "canonical_sha256": string;
  "columns": Array<string>;
  "name": string;
  "ordinal": number;
  "rows": number;
};

export type HistoricalHistoryTablesPage = {
  "index_sha256": string;
  "next_offset"?: number | null;
  "offset": number;
  "tables": Array<HistoricalHistoryTable>;
};

export type HistoricalImportLimit = {
  "installation_id": string;
  "limit_bytes": number;
  "reserved_bytes": number;
  "revision": number;
};

export type HistoricalImportPreview = {
  "catalog_count"?: number | null;
  "catalog_sha256"?: string | null;
  "history_only_file_ids": Array<string>;
  "id": string;
  "import_id": string;
  "limit": HistoricalImportLimit;
  "original_rows": number;
  "owner": HistoricalOwner;
  "plan_sha256": string;
  "projected_bytes": number;
  "projects": Array<HistoricalPreparedProject>;
  "remainder_history_only_reason"?: string | null;
  "request_id": string;
  "request_sha256": string;
  "source_profile": string;
  "source_sha256": string;
  "state": string;
};

export type HistoricalImportRecord = {
  "declared_bytes": number;
  "files": Array<HistoricalFileDeclaration>;
  "id": string;
  "original_rows"?: number | null;
  "owner": HistoricalOwner;
  "pins": HistoricalPinSet;
  "preview_format"?: string | null;
  "received_bytes": number;
  "request_id": string;
  "request_sha256": string;
  "reserved_bytes": number;
  "revision": number;
  "source_family"?: string | null;
  "source_profile"?: string | null;
  "state": string;
  "verifier_contract"?: string | null;
};

export type HistoricalImportTarget = {
  "id": string;
  "name": string;
  "profile_revision": number;
  "revision": number;
};

export type HistoricalOwner = {
  "installation_id": string;
  "kind": "personal_native";
} | {
  "admission_project_id": string;
  "creator_id": string;
  "kind": "organization";
  "organization_id": string;
};

export type HistoricalPinSet = {
  "cold_target_snapshot": string;
  "export_manifest": string;
  "mapping": string;
  "preview": string;
  "quarantine_database": string;
  "quarantine_manifest": string;
};

export type HistoricalPreparedProject = {
  "content_bytes": number;
  "content_pages": Array<HistoricalContentPage>;
  "document_ids": Array<string>;
  "file_ids": Array<string>;
  "order_ids": Array<string>;
  "project_id": string;
  "source": HistoricalContentProject;
  "target": HistoricalProjectTarget;
  "unsupported_reason"?: string | null;
};

export type HistoricalPreviewProjectSummary = {
  "name": string;
  "ordinal": number;
  "pages": Array<HistoricalHistoryPagePin>;
  "project_id": string;
  "source_id": string;
  "target": HistoricalProjectTarget;
};

export type HistoricalPreviewSummary = {
  "catalog_count"?: number | null;
  "catalog_sha256"?: string | null;
  "format": string;
  "history_only_file_ids": Array<string>;
  "id": string;
  "import_id": string;
  "limit": HistoricalImportLimit;
  "original_rows": number;
  "plan_sha256": string;
  "projected_bytes": number;
  "projects": Array<HistoricalPreviewProjectSummary>;
  "remainder_history_only_reason"?: string | null;
  "source_profile": string;
  "state": string;
};

export type HistoricalProjectMapping = {
  "source_project_id": string;
  "target": HistoricalProjectTarget;
};

export type HistoricalProjectTarget = {
  "kind": "history_only";
  "reason": string;
} | {
  "kind": "new_personal";
} | {
  "expected_revision": number;
  "kind": "existing_personal";
  "project_id": string;
} | {
  "expected_profile_revision": number;
  "expected_revision": number;
  "kind": "existing_organization";
  "project_id": string;
};

export type HistoricalStream = "quarantine_manifest" | "quarantine_database" | "mapping" | "preview" | "cold_target_snapshot";

export type HistoricalUnsentFileReceipt = {
  "closed_at": number;
  "closure_sha256": string;
  "format": string;
  "original_charge_id"?: string | null;
  "original_operation": HistoricalFileOperation;
  "released_bytes": number;
  "retained_reserved_bytes": number;
};

export type HistorySnapshot = {
  "artifacts": Array<Artifact>;
  "created_at": number;
  "messages": Array<Message>;
  "sha256": string;
  "source_order_id": string;
  "through_message_id": string;
  "workspace_snapshots"?: Array<WorkspaceSnapshot>;
};

export type ImportActorModelQualification = {
  "context": ActorModelContext;
  "idempotency_key": string;
  "qualification_bundle": unknown;
  "source_sha256": string;
};

export type ImportAuditCheckpoint = {
  "candidate_request_id": string;
  "checkpoint_bytes_base64": string;
  "request_id": string;
};

export type ImportAuditTrustTransition = {
  "independently_confirmed": boolean;
  "original_bytes_base64": string;
  "proposal_request_id": string;
  "request_id": string;
};

export type ImportExecutionPreflightV2 = {
  "access_mode": AccessMode;
  "allowed_tools": Array<string>;
  "budgets": Budgets;
  "executor": string;
  "folders": Array<FolderGrant>;
  "import": ImportFile;
  "ready": boolean;
  "upload": FileUpload;
};

export type ImportFile = {
  "expected_sha256"?: string | null;
  "file": DownloadMetadata;
  "folder_id": string;
  "path": string;
  "upload_id": string;
};

export type ImportModelQualification = {
  "idempotency_key": string;
  "qualification_bundle": unknown;
};

export type InstallOfflineGrant = {
  "confirmed_issuer_origin": string;
  "confirmed_issuer_public_key": string;
  "ticket": SignedOfflineGrant;
};

export type IssueOfflineGrant = {
  "allowed_tools": Array<string>;
  "expires_in_seconds": number;
  "maximum_duration_seconds": number;
  "maximum_runs": number;
  "offline_base_url": string;
  "offline_model": string;
  "output"?: StructuredOutput | null;
  "prompt": string;
  "proof": OfflineDeviceProof;
};

export type KnowledgeDocument = {
  "content": string;
  "created_at": number;
  "folder_id"?: string | null;
  "id": string;
  "kind": KnowledgeKind;
  "memory"?: MemoryMetadata | null;
  "project_id": string;
  "provenance"?: KnowledgeProvenance;
  "revision": number;
  "tags": Array<string>;
  "title": string;
  "updated_at": number;
};

export type KnowledgeFolder = {
  "id": string;
  "name": string;
  "parent_id"?: string | null;
};

export type KnowledgeFolders = {
  "folders": Array<KnowledgeFolder>;
  "revision": number;
};

export type KnowledgeGraphEdge = {
  "from": string;
  "relation": string;
  "to": string;
};

export type KnowledgeGraphHit = {
  "document": KnowledgeDocument;
  "score": number;
  "via": string;
};

export type KnowledgeGraphNode = {
  "id": string;
  "kind": KnowledgeNodeKind;
  "label": string;
};

export type KnowledgeGraphResults = {
  "fingerprint": string;
  "hits": Array<KnowledgeGraphHit>;
};

export type KnowledgeImportRequest = {
  "documents": Array<WriteKnowledge>;
  "idempotency_key": string;
};

export type KnowledgeImportResult = {
  "documents": Array<KnowledgeDocument>;
  "project_id": string;
};

export type KnowledgeKind = "knowledge" | "memory";

export type KnowledgeLocation = {
  "folder_id"?: string | null;
};

export type KnowledgeNodeKind = "topic" | "document" | "tag" | "source";

export type KnowledgeOrigin = "user_authored" | "imported" | "retrieved" | "legacy_unknown";

export type KnowledgeProvenance = {
  "format": number;
  "origin": KnowledgeOrigin;
  "sources": Array<KnowledgeSource>;
};

export type KnowledgeRevision = {
  "content_sha256": string;
  "document": unknown;
  "document_id": string;
  "provenance": KnowledgeProvenance;
  "provenance_sha256": string;
  "recorded_at": number;
  "revision": number;
};

export type KnowledgeSource = {
  "artifact_id"?: string | null;
  "file_id"?: string | null;
  "format": number;
  "locator": string;
  "revision": string;
  "sha256": string;
  "source_uri"?: string | null;
  "verification"?: KnowledgeSourceVerification;
};

export type KnowledgeSourceVerification = "declared" | "snapshot_verified";

export type KnowledgeTopicGraph = {
  "built_at": number;
  "documents": number;
  "edges": Array<KnowledgeGraphEdge>;
  "fingerprint": string;
  "folder_id": string;
  "include_descendants": boolean;
  "nodes": Array<KnowledgeGraphNode>;
  "stale": boolean;
};

export type LibraryConfigContract = {
  "actor_chat_configuration": ActorChatConfiguration;
  "actor_context_record": ActorContextRecord;
  "actor_skill_learning_history": ActorSkillLearningHistory;
  "agent_configuration": AgentConfiguration;
  "agent_template": AgentTemplate;
  "delete_actor_context": DeleteActorContext;
  "delete_external_memory_provider": DeleteExternalMemoryProvider;
  "delete_library_entry": DeleteLibraryEntry;
  "effective_agent_config": EffectiveAgentConfig;
  "external_memory_provider": ExternalMemoryProvider;
  "external_memory_retrieval": ExternalMemoryRetrieval;
  "library_deleted": LibraryDeleted;
  "personality": Personality;
  "preferences_record": PreferencesRecord;
  "retrieve_external_memory": RetrieveExternalMemory;
  "write_actor_chat_configuration": WriteActorChatConfiguration;
  "write_actor_memory": WriteActorMemory;
  "write_actor_skill": WriteActorSkill;
  "write_agent_template": WriteAgentTemplate;
  "write_external_memory_provider": WriteExternalMemoryProvider;
  "write_personality": WritePersonality;
  "write_preferences": WritePreferences;
};

export type LibraryDeleted = {
  "deleted": boolean;
  "id": string;
  "revision": number;
};

export type LibrarySource = {
  "id": string;
  "revision": number;
};

export type MaintenanceStatus = {
  "backend": string;
  "backup_available": boolean;
  "backups": Array<DatabaseBackup>;
  "cleaned_chats": number;
  "last_cleanup_at"?: number | null;
  "last_error"?: string | null;
  "preferences": PreferencesRecord;
};

export type ManageChat = {
  "expected_revision": number;
  "idempotency_key": string;
  "project_id": string;
  "title": string;
};

export type ManageSandboxSession = {
  "environment": ExecutionEnvironmentRef;
  "expected_sandbox_uid"?: string | null;
  "idempotency_key": string;
  "operation": SandboxLifecycleOperation;
};

export type MaterializeCodexModelProfile = {
  "context_tokens": number;
  "expected_account_revision": number;
  "idempotency_key": string;
  "model": string;
  "reasoning_effort"?: string | null;
};

export type McpAsyncTask = {
  "effects_unknown": boolean;
  "expires_at"?: number | null;
  "id": string;
  "operation_pending": boolean;
  "poll_interval_ms": number;
  "protocol": string;
  "result_sha256"?: string | null;
  "revision": number;
  "run_id": string;
  "server_id": string;
  "state": string;
  "tool_call_id": string;
};

export type McpAsyncTaskRequest = {
  "expected_revision": number;
  "idempotency_key": string;
};

export type McpAttempt = {
  "binding": McpToolBinding;
  "connection_id": string;
  "created_at": number;
  "id": string;
  "project_id": string;
  "request_sha256": string;
  "revision": number;
  "rpc_request_id": string;
  "run_id": string;
  "state": McpAttemptState;
  "tool": ToolCall;
  "transport_protocol"?: string | null;
  "transport_request_sha256": string;
  "transport_task_ttl_ms"?: number | null;
  "updated_at": number;
};

export type McpAttemptState = {
  "state": "dispatch_committed";
} | {
  "code": string;
  "response"?: unknown;
  "response_sha256"?: string | null;
  "state": "outcome_uncertain";
} | {
  "result": McpResult;
  "state": "completed";
} | {
  "evidence_sha256": string;
  "result": McpResult;
  "state": "reconciled_completed";
} | {
  "evidence_sha256": string;
  "state": "reconciled_not_executed";
};

export type McpOAuthConfiguration = {
  "authorization_server": string;
  "client_id": string;
  "enabled": boolean;
  "redirect_uri": string;
  "resource": string;
  "scopes": Array<string>;
  "shared_team_consent": boolean;
};

export type McpOAuthDiscovery = {
  "resource": string;
  "server_id": string;
  "server_revision": number;
};

export type McpOAuthFlow = {
  "authorization_url"?: string | null;
  "callback_received": boolean;
  "expires_at": number;
  "id": string;
  "profile_id": string;
  "profile_revision": number;
  "state": string;
};

export type McpOAuthMetadata = {
  "authorization_servers": Array<unknown>;
  "resource": string;
  "resource_metadata_uri": string;
  "scopes_supported": Array<string>;
  "sha256": string;
  "suggested_scopes": Array<string>;
};

export type McpOAuthProfile = {
  "binding_sha256": string;
  "configuration": McpOAuthConfiguration;
  "expires_at"?: number | null;
  "id": string;
  "metadata_sha256": string;
  "owner_id": string;
  "owner_kind": string;
  "project_id": string;
  "reauthorization_required": boolean;
  "revision": number;
  "server_id": string;
  "state": string;
};

export type McpOAuthRequest = {
  "expected_revision": number;
  "idempotency_key": string;
};

export type McpResult = {
  "response": unknown;
  "sha256": string;
};

export type McpServer = {
  "catalog_sha256": string;
  "configuration": McpServerConfiguration;
  "configuration_sha256": string;
  "id": string;
  "project_id": string;
  "revision": number;
};

export type McpServerConfiguration = {
  "catalog": Array<unknown>;
  "credential_reference"?: string | null;
  "credential_revision": number;
  "enabled": boolean;
  "endpoint": string;
  "name": string;
  "selected_tools": Array<string>;
};

export type McpTaskDetail = {
  "original_result"?: unknown;
  "task": McpAsyncTask;
};

export type McpTaskInteraction = {
  "effects_unknown": boolean;
  "expires_at": number;
  "generated_response"?: unknown;
  "id": string;
  "method": string;
  "original_request": unknown;
  "request_sha256": string;
  "response_sha256"?: string | null;
  "revision": number;
  "run_id": string;
  "server_id": string;
  "state": string;
  "task_record_id": string;
};

export type McpToolBinding = {
  "alias": string;
  "catalog_sha256": string;
  "configuration_sha256": string;
  "definition": unknown;
  "endpoint": string;
  "server_id": string;
  "server_revision": number;
};

export type McpToolSelection = {
  "server_id": string;
  "server_revision": number;
  "tool_name": string;
};

export type MemoryMetadata = {
  "category"?: string;
  "confidence"?: number | null;
  "enabled"?: boolean;
  "profile_key"?: string | null;
  "scope"?: MemoryScope;
};

export type MemoryScope = {
  "kind": "user";
} | {
  "kind": "project";
} | {
  "kind": "chat";
  "order_id": string;
} | {
  "agent_id": string;
  "kind": "agent";
};

export type Message = {
  "content": string;
  "created_at": number;
  "id": string;
  "order_id": string;
  "role": string;
  "run_id": string;
};

export type ModelAttempt = {
  "finished_at"?: number | null;
  "id": string;
  "limit_exceeded": boolean;
  "request_sha256"?: string | null;
  "reserved_tokens": number;
  "started_at": number;
  "state": ModelAttemptState;
  "step": number;
  "usage": Usage;
};

export type ModelAttemptState = "in_flight" | "measured" | "unmeasured" | "interrupted";

export type ModelCapabilities = {
  "image_input"?: boolean;
};

export type ModelContextView = {
  "archive_id"?: string | null;
  "conservative_bytes"?: number | null;
  "context_limit": number;
  "count_mode": string;
  "fits": boolean;
  "groups": Array<ContextGroupView>;
  "input_tokens"?: number | null;
  "model_binding"?: ModelRegistryRef | null;
  "order_id": string;
  "order_revision": number;
  "output_allowance": number;
  "selection_revision": number;
  "snapshot_sha256": string;
};

export type ModelRegistryRef = {
  "id": string;
  "sha256": string;
  "version": number;
};

export type ModelRegistryView = {
  "binding": ModelRegistryRef;
  "chat_template_sha256": string;
  "context_tokens": number;
  "deployment_sha256": string;
  "evidence_sha256": string;
  "model_sha256": string;
  "provider_id": string;
  "provider_revision": number;
  "provider_sha256": string;
  "reference_implementation": string;
  "registered_at": number;
  "revoked": boolean;
  "tokenizer_sha256": string;
  "wire_mode": string;
};

export type NativeAuthTransportCapture = {
  "operation_id": string;
  "response_json": string;
};

export type NativeAuthTransportReplay = {
  "request": NativeAuthTransportRequest;
  "response_json"?: string | null;
};

export type NativeAuthTransportRequest = {
  "actor_id"?: string | null;
  "body_json": string;
  "endpoint": string;
  "operation_id": string;
  "purpose": string;
  "session_id"?: string | null;
};

export type NativeAuthTransportResolution = {
  "acknowledged": boolean;
  "operation_id": string;
};

export type NativeAuthTransportView = {
  "actor_id"?: string | null;
  "endpoint": string;
  "operation_id": string;
  "purpose": string;
  "session_id"?: string | null;
  "state": string;
};

export type NativeChooseFolderRequest = {
  "idempotency_key": string;
  "language": UiLanguage;
  "project_id": string;
  "writable": boolean;
};

export type NativeDeletedSchedule = {
  "id": string;
  "name": string;
};

export type NativeFolderOutcome = {
  "status": "canceled";
} | {
  "folder": RegisteredFolder;
  "status": "registered";
};

export type NativeFolderSelection = {
  "expected_resource_id": string;
  "idempotency_key": string;
  "name": string;
  "path": string;
  "project_id": string;
  "writable"?: boolean;
};

export type NativeMcpFactory = {
  "arguments": Array<string>;
  "credential_reference": string;
  "credential_revision": number;
  "cwd"?: string | null;
  "enabled": boolean;
  "endpoint": string;
  "environment"?: {
  [key: string]: number;
};
  "native_only"?: boolean;
  "program": string;
  "project_id": string;
  "revision": number;
  "server_id": string;
};

export type NativeSaveDatabaseBackup = {
  "expected_bytes": number;
  "expected_sha256": string;
  "id": string;
  "language": UiLanguage;
};

export type NativeSaveOutcome = {
  "status": "canceled";
} | {
  "file": DownloadMetadata;
  "status": "saved";
};

export type NativeSaveRequest = {
  "file": DownloadMetadata;
  "language": UiLanguage;
};

export type NativeScheduleHistory = {
  "items": Array<NativeScheduleHistoryEntry>;
  "next_cursor"?: number | null;
};

export type NativeScheduleHistoryEntry = {
  "deleted": boolean;
  "next_run"?: number | null;
  "occurrence"?: ScheduleOccurrence | null;
  "paused"?: boolean | null;
  "recorded_at": number;
  "run"?: RunRecord | null;
  "schedule_revision"?: number | null;
  "sequence": number;
};

export type NativeSecretsReceipt = {
  "operation_id": string;
  "operation_state": NativeSecretsSetupState;
  "store": NativeSecretsStatus;
};

export type NativeSecretsRequestLookup = {
  "receipt"?: NativeSecretsReceipt | null;
};

export type NativeSecretsResolution = {
  "base_revision": number;
  "operation_id": string;
};

export type NativeSecretsSetup = {
  "base_revision": number;
  "idempotency_key": string;
};

export type NativeSecretsSetupState = "pending" | "activated" | "abandoned";

export type NativeSecretsStatus = {
  "configured": boolean;
  "key_available": boolean;
  "pending_operation_id"?: string | null;
  "revision": number;
};

export type NativeSkillHistory = {
  "items": Array<NativeSkillRevision>;
  "next_cursor"?: number | null;
};

export type NativeSkillRevision = {
  "deleted": boolean;
  "recorded_at": number;
  "sequence": number;
  "skill"?: Skill | null;
};

export type OfflineArtifact = {
  "content": string;
  "id": string;
  "media_type": string;
  "name": string;
  "sha256": string;
};

export type OfflineDevice = {
  "id": string;
  "name": string;
  "public_key": string;
};

export type OfflineDeviceProof = {
  "device": OfflineDevice;
  "request": OfflineDeviceProofRequest;
  "signature": string;
};

export type OfflineDeviceProofRequest = {
  "idempotency_key": string;
  "issuer_origin": string;
  "order_id": string;
  "project_id": string;
  "team_id": string;
};

export type OfflineGrant = {
  "allowed_tools": Array<string>;
  "device": OfflineDevice;
  "expires_at": number;
  "id": string;
  "issued_at": number;
  "issuer_instance": string;
  "issuer_origin": string;
  "issuer_public_key": string;
  "maximum_artifact_bytes": number;
  "maximum_duration_seconds": number;
  "maximum_model_requests": number;
  "maximum_runs": number;
  "model": string;
  "offline_base_url": string;
  "order_id": string;
  "output"?: StructuredOutput | null;
  "project_id": string;
  "prompt": string;
  "provider_base_url": string;
  "provider_model": string;
  "scope": OfflineScopeRevision;
  "source_sha256": string;
  "sources": Array<OfflineSource>;
  "team_id": string;
  "version": number;
};

export type OfflineGrantStatus = {
  "allowed": boolean;
  "checked_at": number;
  "grant_id": string;
  "issuer_origin": string;
  "nonce": string;
  "reason": string;
};

export type OfflineGrantStatusRequest = {
  "device_signature": string;
  "grant_id": string;
  "nonce": string;
};

export type OfflineGrantView = {
  "allowed_tools": Array<string>;
  "device": OfflineDevice;
  "expires_at": number;
  "id": string;
  "issuer_origin": string;
  "issuer_public_key": string;
  "maximum_duration_seconds": number;
  "maximum_runs": number;
  "model": string;
  "offline_base_url": string;
  "order_id": string;
  "output"?: StructuredOutput | null;
  "project_id": string;
  "provider_base_url": string;
  "provider_model": string;
  "source_count": number;
  "team_id": string;
};

export type OfflineJournalEntry = {
  "at": number;
  "kind": string;
  "payload": unknown;
  "previous_sha256": string;
  "sequence": number;
  "sha256": string;
};

export type OfflineLocalGrantView = {
  "grant": OfflineGrantView;
  "key_configured": boolean;
  "revoked": boolean;
  "used_runs": number;
};

export type OfflineMcpConnectionInput = {
  "allowed_tools": Array<string>;
  "bearer"?: string | null;
  "endpoint": string;
  "explicitly_allow_no_key": boolean;
  "id": string;
};

export type OfflineMcpConnectionView = {
  "allowed_tools": Array<string>;
  "catalog_sha256": string;
  "credential_configured": boolean;
  "definitions": Array<unknown>;
  "endpoint": string;
  "id": string;
};

export type OfflineMcpInteractionPage = {
  "items": Array<McpTaskInteraction>;
  "next_cursor"?: string | null;
};

export type OfflineMcpResultBytes = {
  "bytes_base64": string;
  "sha256": string;
  "size": number;
};

export type OfflinePendingTool = {
  "arguments": unknown;
  "id": string;
  "name": string;
  "request_sha256": string;
};

export type OfflineReconciliationReceipt = {
  "artifacts": Array<OfflineArtifact>;
  "grant_id": string;
  "id": string;
  "local_run_id": string;
  "order_id": string;
  "project_id": string;
  "recorded_at": number;
  "report_sha256": string;
  "response"?: string | null;
  "state": string;
  "team_id": string;
};

export type OfflineRun = {
  "artifacts": Array<OfflineArtifact>;
  "deadline_at": number;
  "device_id": string;
  "error"?: string | null;
  "finished_at"?: number | null;
  "grant_id": string;
  "id": string;
  "journal": Array<OfflineJournalEntry>;
  "model_requests": number;
  "ordinal": number;
  "output_contract_sha256"?: string | null;
  "pending_tools": Array<OfflinePendingTool>;
  "reconciled": boolean;
  "response"?: string | null;
  "revision": number;
  "started_at": number;
  "state": string;
  "version": number;
};

export type OfflineRunExport = {
  "device_signature": string;
  "run": OfflineRun;
  "ticket": SignedOfflineGrant;
};

export type OfflineRunPage = {
  "next_cursor"?: string | null;
  "runs": Array<OfflineRunSummary>;
};

export type OfflineRunSummary = {
  "artifact_count": number;
  "error"?: string | null;
  "grant_id": string;
  "id": string;
  "journal_count": number;
  "ordinal": number;
  "revision": number;
  "started_at": number;
  "state": string;
};

export type OfflineScopeRevision = {
  "authorization_epoch": number;
  "content_grant_revision": number;
  "identity_id": string;
  "identity_revision": number;
  "membership_revision": number;
  "order_revision": number;
  "project_revision": number;
  "provider_revision": number;
  "resource_revision": number;
};

export type OfflineSource = {
  "content": string;
  "id": string;
  "name": string;
  "revision": number;
  "sha256": string;
};

export type OfflineToolConfigurationView = {
  "allowed_tools": Array<string>;
  "available_programs": Array<string>;
  "browser_available": boolean;
  "configuration_sha256": string;
  "configured": boolean;
  "filesystem_isolation": boolean;
  "github_credential_configured": boolean;
  "grant_id": string;
  "mcp_connections": Array<OfflineMcpConnectionView>;
  "office_available": boolean;
  "programs": Array<string>;
};

export type OfflineWorkspaceUpload = {
  "bytes": Array<number>;
  "path": string;
};

export type Order = {
  "created_at": number;
  "draft": string;
  "id": string;
  "project_id": string;
  "revision": number;
  "title": string;
};

export type OrderArchive = {
  "artifacts": Array<Artifact>;
  "events": Array<RunEvent>;
  "history": OrderHistory;
  "messages": Array<Message>;
  "order": Order;
  "project": Project;
  "runs": Array<RunDetail>;
  "uploads"?: Array<FileUpload>;
  "workflow_input_answer_records"?: unknown;
  "workflow_input_resume_operations"?: unknown;
  "workflows"?: Array<WorkflowSnapshot>;
};

export type OrderExport = {
  "data": OrderArchive;
  "exported_at": number;
  "format_version": number;
  "sha256": string;
};

export type OrderHistory = {
  "inherited"?: HistorySnapshot | null;
};

export type OwnerConnector = {
  "deleted": boolean;
  "destination": string;
  "enabled": boolean;
  "has_bearer": boolean;
  "id": string;
  "name": string;
  "native_only": boolean;
  "note": string;
  "project_id": string;
  "revision": number;
  "server_id": string;
};

export type OwnerConnectorProbe = {
  "destination": string;
  "reachable": boolean;
  "status": number;
};

export type OwnerDatabaseBackupRequest = {
  "idempotency_key": string;
};

export type OwnerExecutionTargetV2 = {
  "kind": "broker_controller";
} | {
  "kind": "registered_runner";
} | {
  "kind": "document_engine";
};

export type OwnerManagedProcessReconciliation = {
  "process_attempt": ProcessAttempt;
  "run": RunRecord;
};

export type OwnerManagedResultDelivery = {
  "accept_before": number;
  "accepted_at"?: number | null;
  "created_at": number;
  "delivery_receipt_sha256"?: string | null;
  "id": string;
  "manifest_sha256": string;
  "request_sha256": string;
  "result_version_id": string;
  "retirement_receipt_sha256"?: string | null;
  "retirement_recorded_at"?: number | null;
  "retirement_revision"?: number | null;
  "revision": number;
  "state": OwnerManagedResultDeliveryState;
};

export type OwnerManagedResultDeliveryState = "prepared" | "committed" | "expired" | "accepted" | "retired" | "unconfirmed";

export type OwnerManagedResultDeliveryView = {
  "delivery"?: OwnerManagedResultDelivery | null;
  "order_id": string;
  "process_attempt_id": string;
  "run_id": string;
};

export type OwnerManagedResultEntry = {
  "order_id": string;
  "process_attempt_id": string;
  "run_id": string;
  "version"?: OwnerManagedResultVersion | null;
};

export type OwnerManagedResultList = {
  "entries": Array<OwnerManagedResultEntry>;
};

export type OwnerManagedResultManifest = {
  "entry": OwnerManagedResultEntry;
  "manifest": WorkspaceTransferManifest;
};

export type OwnerManagedResultVersion = {
  "created_at": number;
  "file_count": number;
  "id": string;
  "manifest_sha256": string;
  "revision": number;
  "state": OwnerManagedResultVersionState;
  "total_bytes": number;
};

export type OwnerManagedResultVersionState = "receiving" | "ready";

export type OwnerMcpCapabilities = {
  "environment": boolean;
  "http": boolean;
  "programs": Array<string>;
  "stdio": boolean;
  "stdio_cwd_policy": OwnerMcpCwdPolicy;
};

export type OwnerMcpCatalog = {
  "catalog": Array<unknown>;
  "catalog_sha256": string;
  "credential_reference"?: string | null;
  "credential_revision"?: number | null;
  "endpoint"?: string | null;
  "factory_revision"?: number | null;
};

export type OwnerMcpCredential = {
  "credential_reference": string;
  "credential_revision": number;
  "endpoint"?: string | null;
};

export type OwnerMcpCwdPolicy = "managed_relative";

export type OwnerMcpDiscovery = {
  "credential_reference"?: string | null;
  "credential_revision"?: number;
  "endpoint": string;
  "project_id": string;
  "server_id": string;
};

export type OwnerMcpExpectedRevision = {
  "expected_revision": number;
};

export type OwnerMcpRevision = {
  "revision": number;
};

export type OwnerTerminalInput = {
  "bytes_base64": string;
  "sequence": number;
};

export type OwnerTerminalLaunch = {
  "approval_sha256"?: string | null;
  "cols": number;
  "idempotency_key": string;
  "program": string;
  "rows": number;
};

export type OwnerTerminalOutput = {
  "bytes_base64": string;
  "first_sequence": number;
  "next_sequence": number;
  "session": OwnerTerminalSession;
  "truncated": boolean;
};

export type OwnerTerminalPreview = {
  "approval_sha256": string;
  "boundary": string;
  "cwd": string;
  "executable": string;
};

export type OwnerTerminalResize = {
  "cols": number;
  "rows": number;
};

export type OwnerTerminalSession = {
  "cols": number;
  "created_at": number;
  "exit_code"?: number | null;
  "id": string;
  "input_sequence": number;
  "order_id": string;
  "pid"?: number | null;
  "program": string;
  "rows": number;
  "state": string;
};

export type OwnerTerminalStatus = {
  "active_sessions": number;
};

export type PairDeviceExecutor = {
  "registration": DeviceExecutorRegistration;
  "team_id"?: string | null;
};

export type PersonalMoneyCurrency = {
  "code": string;
  "scale": number;
};

export type PersonalMoneyDefaults = {
  "pool_policy_version": DecimalU64;
  "project_limit_units": DecimalU64;
  "run_limit_units": DecimalU64;
};

export type PersonalMoneyPoolStatus = {
  "charged_units": DecimalU64;
  "confirmed_attempts": DecimalU64;
  "confirmed_units": DecimalU64;
  "currency": PersonalMoneyCurrency;
  "limit_exceeded": boolean;
  "limit_units": DecimalU64;
  "no_headroom": boolean;
  "overrun": boolean;
  "pending_attempts": DecimalU64;
  "pending_units": DecimalU64;
  "policy_version": DecimalU64;
  "remaining_units": DecimalU64;
  "unknown_attempts": DecimalU64;
  "unknown_units": DecimalU64;
};

export type PersonalMoneyStatus = {
  "origin": "not_configured";
} | {
  "installation": PersonalMoneyPoolStatus;
  "origin": "legacy_manual";
} | {
  "defaults": PersonalMoneyDefaults;
  "installation": PersonalMoneyPoolStatus;
  "origin": "operator_configuration";
};

export type Personality = {
  "created_at": number;
  "id": string;
  "revision": number;
  "updated_at": number;
  "value": PersonalityDefinition;
};

export type PersonalityDefinition = {
  "description"?: string;
  "generation"?: GenerationSettings;
  "goal"?: string;
  "icon"?: string | null;
  "is_default"?: boolean;
  "model"?: AgentModelSelection;
  "name"?: string;
  "role"?: string;
  "skills_markdown"?: string;
  "system_prompt"?: string;
};

export type PreferencesRecord = {
  "revision": number;
  "updated_at": number;
  "value": ApplicationPreferences;
};

export type Preflight = {
  "access_mode": AccessMode;
  "allowed_tools": Array<string>;
  "budgets": Budgets;
  "environment"?: ExecutionEnvironmentRef | null;
  "executor": string;
  "folders"?: Array<FolderGrant>;
  "mcp_tools"?: Array<McpToolBinding>;
  "model_binding"?: ModelRegistryRef | null;
  "policy_revision": number;
  "provider": ProviderProfile;
  "ready": boolean;
};

export type PrepareAuditTrustTransition = {
  "binding_id": string;
  "independently_confirmed": boolean;
  "new_witness_public_key_sha256": string;
  "previous_checkpoint_sha256": string;
  "request_id": string;
};

export type PrepareExecutionInput = {
  "base_revision": number;
  "folder_ids": Array<string>;
  "idempotency_key": string;
};

export type PrepareImportExecutionPlanV2 = {
  "input": ExecutionInputRef;
  "request": CreateImportRun;
  "target": OwnerExecutionTargetV2;
};

export type PrepareOwnerExecutionPlanV2 = {
  "input": ExecutionInputRef;
  "request": RunRequest;
  "target": OwnerExecutionTargetV2;
};

export type PrepareRestoreExecutionPlanV2 = {
  "input": ExecutionInputRef;
  "request": CreateRestoreRun;
  "target": OwnerExecutionTargetV2;
};

export type PrepareTeamLostHostRetirement = {
  "expected_revision": number;
  "nonce": string;
  "operation_id": string;
  "original_plan_sha256": string;
};

export type PrepareWorkflowInputAnswer = {
  "base_draft_revision": number;
  "fingerprint": string;
  "idempotency_key": string;
};

export type PrepareWorkflowOperation = {
  "draft_snapshot"?: WorkflowDraftSnapshotRef | null;
  "intent_id": string;
  "request": WorkflowOperationRequest;
};

export type PreparedExecutionInput = {
  "retired": boolean;
  "sha256": string;
  "snapshot": ExecutionInputSnapshot;
};

export type PreparedExecutionPlan = {
  "committed_run_id"?: string | null;
  "plan": ExecutionPlan;
  "retired": boolean;
  "sha256": string;
};

export type PreparedExecutionPlanV2 = {
  "canonical_json": string;
  "committed_run_id"?: string | null;
  "plan": ExecutionPlanV2;
  "retired": boolean;
  "sha256": string;
};

export type PreserveAuditHistory = {
  "bytes_base64": string;
  "expected_public_key_sha256"?: string | null;
  "request_id": string;
};

export type PreservedAuditHistory = {
  "anchor_chain_id"?: string | null;
  "anchor_sequence": number;
  "bytes": number;
  "content_sha256": string;
  "original_chain_id"?: string | null;
  "public_key_sha256"?: string | null;
  "request_id": string;
  "trust": string;
};

export type PreviewHistoricalImport = {
  "catalog_count"?: number | null;
  "catalog_sha256"?: string | null;
  "expected_import_revision": number;
  "expected_limit_revision": number;
  "history_only_file_ids": Array<string>;
  "projects": Array<HistoricalProjectMapping>;
  "remainder_history_only_reason"?: string | null;
  "request_id": string;
};

export type PreviewModelContext = {
  "request": CreateRun;
};

export type ProcessAttempt = {
  "created_at": number;
  "environment": ExecutionEnvironmentRef;
  "id": string;
  "project_id": string;
  "request_sha256": string;
  "revision": number;
  "run_id": string;
  "state": ProcessAttemptState;
  "tool": ToolCall;
  "updated_at": number;
  "workspace_resource_id": string;
};

export type ProcessAttemptState = {
  "state": "launch_committed";
} | {
  "result": ProcessResult;
  "state": "completed";
} | {
  "evidence_sha256": string;
  "result": ProcessResult;
  "state": "reconciled_completed";
} | {
  "evidence_sha256": string;
  "state": "reconciled_not_started";
};

export type ProcessCapture = {
  "bytes": Array<number>;
  "sha256": string;
  "total_bytes": number;
  "truncated": boolean;
};

export type ProcessInvocation = {
  "arguments": Array<string>;
  "program": string;
  "timeout_seconds": number;
};

export type ProcessResult = {
  "duration_ms": number;
  "exit_code"?: number | null;
  "output_complete": boolean;
  "process_id"?: number | null;
  "signal"?: number | null;
  "stderr": ProcessCapture;
  "stdout": ProcessCapture;
  "termination": ProcessTermination;
};

export type ProcessTermination = "exited" | "canceled" | "timed_out";

export type ProductSchedule = {
  "active_occurrence"?: ScheduleOccurrence | null;
  "created_at": number;
  "id": string;
  "last_error"?: string | null;
  "last_run_id"?: string | null;
  "name": string;
  "next_run"?: number | null;
  "paused": boolean;
  "project_id": string;
  "recurrence": ScheduleRecurrence;
  "revision": number;
  "template": ScheduledRunTemplate;
  "timezone": string;
  "updated_at": number;
};

export type Project = {
  "created_at": number;
  "id": string;
  "name": string;
  "revision": number;
};

export type ProjectSettings = {
  "instructions": string;
  "name": string;
  "project_id": string;
  "revision": number;
  "sources": Array<ProjectSource>;
  "working_folder"?: string | null;
};

export type ProjectSource = {
  "access": string;
  "enabled": boolean;
  "id": string;
  "kind": string;
  "label": string;
  "reference": string;
};

export type ProviderKind = "ollama" | "openai_compatible" | "codex" | "test";

export type ProviderProfile = {
  "capabilities"?: ModelCapabilities;
  "context_tokens": number;
  "credential_ready": boolean;
  "credential_required": boolean;
  "endpoint": string;
  "generation"?: GenerationSettings;
  "id": string;
  "kind": ProviderKind;
  "model": string;
  "name": string;
  "revision": number;
};

export type ProviderSettings = {
  "generation"?: GenerationSettings;
  "profile": ProviderProfile;
};

export type ProviderSettingsContract = {
  "delete": DeleteProviderSettings;
  "discover": DiscoverProviderModels;
  "settings": ProviderSettings;
  "update": UpdateProviderSettings;
};

export type PublishWorkflow = {
  "base_revision": number;
  "definition": WorkflowDefinition;
  "idempotency_key": string;
};

export type ReadWorkspaceTransferChunk = {
  "expected_sha256": string;
  "offset": number;
  "path": string;
};

export type ReceiveTeamLostHostRetirement = {
  "operation_id": string;
  "original_intent_sha256": string;
  "witness": unknown;
};

export type ReconcileFile = {
  "decision": ReconcileFileDecision;
  "fingerprint": string;
};

export type ReconcileFileDecision = "accept_completed" | "confirm_not_applied";

export type ReconcileOwnerManagedProcess = {
  "expected_request_sha256": string;
  "expected_run_revision": number;
  "finish_cancellation": boolean;
};

export type RegisterDevicePush = {
  "device_id": string;
  "provider": string;
  "token": string;
};

export type RegisteredFolder = {
  "canonical_path": string;
  "grant": FolderGrant;
  "revoked": boolean;
};

export type RemoveFile = {
  "expected_sha256": string;
  "folder_id": string;
  "path": string;
};

export type RenameFile = {
  "destination": string;
  "expected_sha256": string;
  "folder_id": string;
  "path": string;
};

export type ReserveUpload = {
  "file": DownloadMetadata;
  "idempotency_key": string;
};

export type ResolveApproval = {
  "allow": boolean;
  "fingerprint": string;
};

export type ResolveTeamEffect = {
  "effect_id": string;
  "expected_revision": number;
  "idempotency_key": string;
  "outcome": string;
  "owner_verified": boolean;
  "receipt": unknown;
  "request_sha256": string;
  "source": string;
};

export type RestoreContentBindingV2 = {
  "kind": FileNodeKind;
  "sha256": string;
};

export type RestoreExecutionPreflightV2 = {
  "access_mode": AccessMode;
  "allowed_tools": Array<string>;
  "budgets": Budgets;
  "content": RestoreContentBindingV2;
  "executor": string;
  "folders": Array<FolderGrant>;
  "ready": boolean;
  "restore": RestoreFile;
};

export type RestoreFile = {
  "expected_source_sha256"?: string | null;
  "folder_id": string;
  "path": string;
  "source": FileRestoreSource;
};

export type ResumeTeamRun = {
  "acknowledge_cancelled": boolean;
  "expected_revision": number;
  "idempotency_key": string;
};

export type ResumeWorkflowInput = {
  "base_run_revision": number;
  "base_workflow_revision": number;
  "expected_checkpoint_sha256": string;
  "expected_cursor_sha256": string;
  "idempotency_key": string;
  "run_id": string;
};

export type RetainedVersion = "text_before" | "text_after" | "operation_before" | "operation_after";

export type RetireAutomationEvent = {
  "expected_updated_at": number;
  "idempotency_key": string;
};

export type RetireExecutionInput = {
  "sha256": string;
};

export type RetireTeamContainer = {
  "allow_legacy_location_adoption": boolean;
  "expected_revision": number;
  "idempotency_key": string;
  "nonce": string;
  "plan_sha256": string;
};

export type RetiredExecutionRequest = {
  "idempotency_key": string;
  "order_id": string;
  "request_sha256": string;
  "retired": boolean;
};

export type RetrieveExternalMemory = {
  "expected_revision": number;
  "request_id": string;
};

export type RevokeActorModelQualification = {
  "actor_id": string;
  "binding": ModelRegistryRef;
  "project_id": string;
};

export type RevokeFolder = {
  "expected_revision": string;
  "folder_id": string;
  "project_id": string;
};

export type RevokeWorkflow = {
  "base_revision": number;
  "idempotency_key": string;
};

export type RunDetail = {
  "approvals": Array<Approval>;
  "artifacts": Array<Artifact>;
  "file_changes"?: Array<FileChange>;
  "file_operations"?: Array<FileOperation>;
  "input_request"?: ToolCall | null;
  "mcp_attempts"?: Array<McpAttempt>;
  "process_attempts"?: Array<ProcessAttempt>;
  "run": RunRecord;
  "token_budget": TokenBudgetStatus;
  "uncertain_file_change"?: FileChange | null;
  "uncertain_file_fingerprint"?: string | null;
  "uncertain_file_operation"?: FileOperation | null;
  "workspace_snapshot"?: WorkspaceSnapshot | null;
};

export type RunEvent = {
  "created_at": number;
  "id": string;
  "kind": string;
  "payload": unknown;
  "run_id": string;
  "schema_version": number;
  "sequence": number;
};

export type RunPreflight = Preflight | WorkflowPreflight;

export type RunRecord = {
  "created_at": number;
  "error_code"?: string | null;
  "id": string;
  "last_sequence": number;
  "model_attempts"?: Array<ModelAttempt>;
  "revision": number;
  "spec": RunSpec;
  "started_at"?: number | null;
  "state": RunState;
  "steps": number;
  "updated_at": number;
  "usage": Usage;
};

export type RunRequest = CreateRun | WorkflowRunRequest;

export type RunSpec = {
  "access_mode": AccessMode;
  "agent_config"?: EffectiveAgentConfig | null;
  "allowed_tools": Array<string>;
  "budgets": Budgets;
  "environment"?: ExecutionEnvironmentRef | null;
  "executor": string;
  "file_action"?: ToolCall | null;
  "folders"?: Array<FolderGrant>;
  "input_workspace"?: RunWorkspaceBinding | null;
  "mcp_tools"?: Array<McpToolBinding>;
  "model_binding"?: ModelRegistryRef | null;
  "order_id": string;
  "output"?: StructuredOutput | null;
  "policy_revision": number;
  "project_id": string;
  "prompt": string;
  "provider"?: ProviderProfile | null;
  "workflow"?: WorkflowBinding | null;
};

export type RunState = "waiting_for_timer" | "queued" | "waiting_for_executor" | "waiting_for_snapshot" | "running" | "waiting_approval" | "waiting_input" | "cancel_requested" | "interrupted" | "completed" | "failed" | "canceled" | "expired";

export type RunWorkspaceBinding = {
  "id": string;
  "input": ExecutionInputRef;
};

export type SandboxLifecycleOperation = "ensure" | "inspect" | "suspend" | "resume" | "stop";

export type SandboxLifecycleState = "provisioning" | "ready" | "suspended" | "stopping" | "stopped" | "unknown";

export type SandboxPreview = {
  "expires_at": number;
  "url": string;
};

export type SandboxSession = {
  "environment": ExecutionEnvironmentRef;
  "observed_at": number;
  "operation_id": string;
  "order_id": string;
  "pod_uid"?: string | null;
  "project_id": string;
  "sandbox_name": string;
  "sandbox_uid"?: string | null;
  "state": SandboxLifecycleState;
  "workspace_preserved": boolean;
};

export type SandboxSessionOperation = {
  "id": string;
  "request": ManageSandboxSession;
  "state": string;
};

export type SaveDocumentDraft = {
  "base_revision": number;
  "content": unknown;
  "id": string;
  "idempotency_key": string;
};

export type SaveDraft = {
  "base_revision": number;
  "draft": string;
};

export type SaveOwnerConnector = {
  "bearer"?: string | null;
  "enabled": boolean;
  "expected_revision": number;
  "id": string;
  "idempotency_key": string;
  "name": string;
  "note": string;
  "recover_only"?: boolean;
  "remove_bearer"?: boolean;
  "url"?: string | null;
};

export type SaveTeamDocument = {
  "archived"?: boolean;
  "content": string;
  "expected_revision": number;
  "id"?: string | null;
  "kind": TeamDocumentKind;
  "name": string;
  "provenance"?: KnowledgeProvenance | null;
};

export type SaveWorkflowDraft = {
  "base_revision": number;
  "content": WorkflowDraftContent;
  "id": string;
  "idempotency_key": string;
};

export type SaveWorkflowInputDraft = {
  "answer": string;
  "base_revision": number;
  "fingerprint": string;
  "idempotency_key": string;
};

export type ScheduleOccurrence = {
  "claimed_at": number;
  "due_at": number;
  "id": string;
  "request": CreateRun;
  "run_id"?: string | null;
  "uncertain": boolean;
  "workflow_request"?: WorkflowRunRequest | null;
};

export type ScheduleRecurrence = {
  "expression": string;
  "kind": "cron";
} | {
  "at_ms": number;
  "kind": "once";
} | {
  "hour": number;
  "kind": "daily";
  "minute": number;
} | {
  "hour": number;
  "kind": "weekly";
  "minute": number;
  "weekday": number;
} | {
  "kind": "interval";
  "seconds": number;
};

export type ScheduledRunTemplate = {
  "access_mode"?: AccessMode;
  "agent_config"?: EffectiveAgentConfig | null;
  "budgets"?: Budgets;
  "environment"?: ExecutionEnvironmentRef | null;
  "folders"?: Array<FolderGrant>;
  "mcp_tools"?: Array<McpToolSelection>;
  "native_tools"?: Array<string>;
  "order_id": string;
  "prompt": string;
  "provider_id": string;
  "provider_revision": number;
  "workflow"?: WorkflowRunRequest | null;
};

export type SearchHit = {
  "excerpt": string;
  "id": string;
  "kind": SearchKind;
  "order_id": string;
  "order_title": string;
  "project_id": string;
  "project_name": string;
};

export type SearchKind = "order" | "message" | "artifact";

export type SearchKnowledgeGraph = {
  "folder_id": string;
  "include_descendants": boolean;
  "query": string;
};

export type SearchPage = {
  "items": Array<SearchHit>;
  "next_offset"?: number | null;
};

export type SetAndroidAuthManagementPolicy = {
  "admin_required": boolean;
  "expected_revision": number;
  "operation_id": string;
};

export type SignedAuditArchive = {
  "captured_at": number;
  "format": string;
  "page": AuditPage;
  "public_key": string;
  "public_key_sha256": string;
  "request_id": string;
  "signature": string;
};

export type SignedAuditCheckpoint = {
  "binding_id": string;
  "candidate_sha256": string;
  "chain_id": string;
  "format": string;
  "head_sequence": number;
  "head_sha256": string;
  "operator_chain_transition": boolean;
  "origin_id": string;
  "origin_public_key_sha256": string;
  "previous_checkpoint_sha256"?: string | null;
  "public_key": string;
  "public_key_sha256": string;
  "request_id": string;
  "scope": string;
  "signature": string;
  "witness_sequence": number;
  "witnessed_at": number;
};

export type SignedAuditTrustTransition = {
  "accepted_at": number;
  "format": string;
  "next_witness_public_key": string;
  "next_witness_signature": string;
  "previous_witness_public_key": string;
  "previous_witness_signature": string;
  "proposal": AuditTrustTransitionProposal;
};

export type SignedOfflineGrant = {
  "grant": OfflineGrant;
  "signature": string;
};

export type SignedOfflineGrantStatus = {
  "server_signature": string;
  "status": OfflineGrantStatus;
};

export type Skill = {
  "auto_learn"?: boolean;
  "created_at": number;
  "description": string;
  "enabled": boolean;
  "id": string;
  "instructions": string;
  "name": string;
  "project_id": string;
  "prompt_template"?: string | null;
  "revision": number;
  "run_mode"?: AgentExecutionMode;
  "trigger_pattern"?: string | null;
  "updated_at": number;
};

export type SkillLearningHistory = {
  "items": Array<SkillLearningOutcome>;
};

export type SkillLearningOutcome = {
  "observed_tools": Array<string>;
  "proposed_instructions"?: string | null;
  "reason": string;
  "recorded_at": number;
  "run_id": string;
  "skill_id": string;
  "skill_revision": number;
  "state": RunState;
};

export type StartDevicePasskeyAuthentication = {
  "device_id": string;
  "email": string;
};

export type StartOfflineRun = {
  "grant_id": string;
  "idempotency_key": string;
};

export type StartWorkspaceTask = {
  "expected_revision": number;
  "idempotency_key": string;
  "order_revision": number;
};

export type StructuredOutput = {
  "name": string;
  "schema": unknown;
};

export type SubmitAutomationEvent = {
  "data": unknown;
  "event_id": string;
};

export type SubmitInput = {
  "answer": string;
  "request_id": string;
};

export type SubmitWorkflowInput = {
  "fingerprint": string;
  "idempotency_key": string;
  "input": SubmitInput;
};

export type Team = {
  "id": string;
  "name": string;
  "revision": number;
  "state": string;
};

export type TeamAgent = {
  "id": string;
  "instructions": string;
  "knowledge"?: Array<string>;
  "output"?: StructuredOutput | null;
  "skills"?: Array<string>;
};

export type TeamContainerRecoveryPlan = {
  "container_id": string;
  "container_present": boolean;
  "engine_id": string;
  "host_id": string;
  "legacy_location_adoption": boolean;
  "node_name"?: string | null;
  "node_uid"?: string | null;
  "nonce": string;
  "order_id": string;
  "paused": boolean;
  "plan_sha256": string;
  "record_format": number;
  "record_sha256": string;
  "revision": number;
  "run_id": string;
  "running": boolean;
  "team_id": string;
};

export type TeamContainerRecoveryReceipt = {
  "container_id": string;
  "container_removed": boolean;
  "effects_unknown": boolean;
  "engine_id": string;
  "host_id": string;
  "legacy_location_adoption": boolean;
  "nonce": string;
  "operation_id": string;
  "plan_sha256": string;
  "record_sha256": string;
  "request_sha256": string;
  "revision": number;
  "run_id": string;
};

export type TeamDocument = {
  "archived": boolean;
  "content": string;
  "id": string;
  "kind": TeamDocumentKind;
  "name": string;
  "project_id": string;
  "provenance"?: KnowledgeProvenance;
  "revision": number;
  "team_id": string;
};

export type TeamDocumentKind = "skill" | "knowledge";

export type TeamEffectDetail = {
  "effect": TeamEffectView;
  "outcome"?: string | null;
  "receipt"?: unknown;
  "request": unknown;
  "result"?: unknown;
  "source"?: string | null;
};

export type TeamEffectView = {
  "attempt": number;
  "created_at": number;
  "id": string;
  "kind": string;
  "provider_revision": number;
  "request_id": string;
  "request_sha256": string;
  "run_id": string;
  "scope_sha256": string;
  "sequence": number;
  "slot": string;
  "state": string;
};

export type TeamExecutionActor = {
  "grant_revision": number;
  "identity_id": string;
  "membership_id": string;
  "membership_revision": number;
  "organization_epoch": number;
  "organization_id": string;
  "project_id": string;
  "resource_revision": number;
  "session_id": string;
  "session_revision": number;
};

export type TeamExecutionBlocker = "unbound_execution_target";

export type TeamExecutionDocumentBinding = {
  "content_sha256": string;
  "id": string;
  "kind": TeamDocumentKind;
  "name": string;
  "revision": number;
};

export type TeamExecutionMcpBinding = {
  "catalog_sha256": string;
  "revision": number;
  "server_id": string;
  "tools": Array<string>;
};

export type TeamExecutionMemoryBinding = {
  "id": string;
  "revision": number;
};

export type TeamExecutionModelBinding = {
  "base_url": string;
  "model": string;
  "provider_revision": number;
};

export type TeamExecutionPreflight = {
  "actor": TeamExecutionActor;
  "blockers": Array<TeamExecutionBlocker>;
  "documents": Array<TeamExecutionDocumentBinding>;
  "github_revision"?: number | null;
  "mcp_servers": Array<TeamExecutionMcpBinding>;
  "model": TeamExecutionModelBinding;
  "order_prompt": string;
  "order_revision": number;
  "personal_knowledge": Array<TeamExecutionMemoryBinding>;
  "project_revision": number;
  "ready": boolean;
  "request": CreateTeamRun;
};

export type TeamGithubConfiguration = {
  "enabled": boolean;
  "hostname": string;
  "key_configured": boolean;
  "revision": number;
  "team_id": string;
};

export type TeamHostTrustProfileView = {
  "activated_at"?: number | null;
  "active": boolean;
  "binding": TeamOriginalHostBinding;
  "id": string;
  "inspected_at": number;
  "profile_sha256": string;
  "public_key_sha256": string;
  "revision": number;
  "team_id": string;
};

export type TeamLostHostPlan = {
  "affected_runs": Array<string>;
  "container_id"?: string | null;
  "container_name": string;
  "current_revision": number;
  "intent": unknown;
  "intent_sha256": string;
  "nonce": string;
  "order_id": string;
  "original_profile_sha256": string;
  "original_record_sha256": string;
  "original_record_source": string;
  "original_runtime_count": number;
  "plan_sha256": string;
  "profile_id": string;
  "run_id": string;
  "team_id": string;
};

export type TeamLostHostRetirementReceipt = {
  "container_removed": boolean;
  "effects_unknown": boolean;
  "host_retired": boolean;
  "nonce": string;
  "operation_id": string;
  "original_intent_sha256": string;
  "original_profile_sha256": string;
  "original_record_sha256": string;
  "retirement_id": string;
  "run_id": string;
  "witness_sha256": string;
  "workspace_promotion_allowed": boolean;
};

export type TeamMcpServer = {
  "catalog_sha256"?: string | null;
  "credential_configured": boolean;
  "enabled": boolean;
  "endpoint": string;
  "id": string;
  "name": string;
  "revision": number;
  "team_id": string;
  "tool_count": number;
};

export type TeamMcpTool = {
  "description"?: string | null;
  "input_schema": unknown;
  "name": string;
  "remote_name": string;
};

export type TeamMember = {
  "active": boolean;
  "id": string;
  "identity_id": string;
  "revision": number;
  "role": TeamRole;
  "team_id": string;
};

export type TeamOrder = {
  "archived": boolean;
  "created_at": number;
  "id": string;
  "project_id": string;
  "prompt": string;
  "revision": number;
  "team_id": string;
  "title": string;
};

export type TeamOriginalHostBinding = {
  "engine_id": string;
  "format": number;
  "host_id": string;
  "node_name"?: string | null;
  "node_uid"?: string | null;
};

export type TeamProject = {
  "archived": boolean;
  "created_at": number;
  "id": string;
  "name": string;
  "revision": number;
  "team_id": string;
};

export type TeamProjectGrant = {
  "active": boolean;
  "execute": boolean;
  "expected_revision": number;
  "membership_id": string;
  "read": boolean;
  "share": boolean;
  "write": boolean;
};

export type TeamProjectGrantView = {
  "active": boolean;
  "execute": boolean;
  "membership_id": string;
  "read": boolean;
  "revision": number;
  "share": boolean;
  "write": boolean;
};

export type TeamProvider = {
  "base_url": string;
  "key_configured": boolean;
  "model": string;
  "revision": number;
  "team_id": string;
};

export type TeamRecoveryView = {
  "blocked_reason"?: string | null;
  "effects": Array<TeamEffectView>;
  "has_more": boolean;
  "next_cursor": number;
  "resume_allowed": boolean;
  "revision": number;
  "run_id": string;
  "scope_sha256": string;
  "status": TeamRunStatus;
};

export type TeamRole = "owner" | "admin" | "member" | "viewer";

export type TeamRun = {
  "created_at": number;
  "error"?: string | null;
  "id": string;
  "model": string;
  "order_id": string;
  "output"?: StructuredOutput | null;
  "project_id": string;
  "provider_revision": number;
  "response"?: string | null;
  "status": TeamRunStatus;
  "team_id": string;
  "updated_at": number;
};

export type TeamRunPlan = {
  "knowledge"?: Array<string>;
  "output"?: StructuredOutput | null;
  "personal_knowledge"?: boolean;
  "skills"?: Array<string>;
  "tools"?: Array<string>;
  "workflow"?: TeamWorkflowPlan | null;
};

export type TeamRunStatus = "queued" | "running" | "completed" | "failed" | "unknown" | "cancelled";

export type TeamRunStep = {
  "error"?: string | null;
  "id": string;
  "response"?: string | null;
  "state": string;
  "updated_at": number;
};

export type TeamToolApproval = {
  "action": unknown;
  "created_at": number;
  "error"?: string | null;
  "expires_at": number;
  "id": string;
  "job_id"?: string | null;
  "order_id": string;
  "project_id": string;
  "request_sha256": string;
  "run_id": string;
  "state": string;
  "team_id": string;
  "tool": string;
  "updated_at": number;
};

export type TeamWorkflowMode = "sequential" | "parallel" | "hierarchical";

export type TeamWorkflowPlan = {
  "agents": Array<TeamAgent>;
  "manager_agent_id"?: string | null;
  "mode": TeamWorkflowMode;
  "steps": Array<TeamWorkflowStep>;
};

export type TeamWorkflowStep = {
  "agent_id": string;
  "depends_on"?: Array<string>;
  "id": string;
  "prompt": string;
};

export type TextFile = {
  "content": string;
  "folder_id": string;
  "path": string;
  "sha256": string;
  "size": number;
};

export type TokenBudgetStatus = {
  "charged_tokens": number;
  "limit": number;
  "limit_exceeded": boolean;
  "remaining_tokens": number;
  "unconfirmed_tokens": number;
};

export type ToolCall = {
  "arguments": unknown;
  "id": string;
  "name": string;
};

export type UiLanguage = "de" | "en";

export type UpdateDeviceExecutor = {
  "draining": boolean;
  "expected_revision": number;
  "revoked": boolean;
};

export type UpdateProviderSettings = {
  "api_key"?: string | null;
  "expected_revision": number;
  "generation"?: GenerationSettings;
  "idempotency_key": string;
  "profile": CreateProvider;
  "remove_key"?: boolean;
};

export type UpdateTeam = {
  "closed"?: boolean;
  "expected_revision": number;
  "name": string;
};

export type UpdateTeamMember = {
  "active": boolean;
  "expected_revision": number;
  "role": TeamRole;
};

export type UpdateTeamOrder = {
  "archived": boolean;
  "expected_revision": number;
  "prompt": string;
  "title": string;
};

export type UpdateTeamProject = {
  "archived": boolean;
  "expected_revision": number;
  "name": string;
};

export type UploadPage = {
  "items": Array<FileUpload>;
  "next_offset"?: number | null;
};

export type UploadState = "reserved" | "ready";

export type Usage = {
  "cost"?: number | null;
  "incomplete"?: boolean;
  "input_tokens"?: number | null;
  "output_tokens"?: number | null;
  "price_source_version"?: string | null;
};

export type VerifyAuditV1 = {
  "archives": Array<AuditOriginalReference>;
  "request_id": string;
};

export type WorkflowBinding = {
  "definition_sha256": string;
  "inputs"?: unknown;
  "plan_sha256": string;
  "version_id": string;
  "workflow_id": string;
};

export type WorkflowControlOperation = {
  "form": WorkflowReviewForm;
  "kind": "review";
  "rights": WorkflowRights;
} | {
  "kind": "wait";
  "seconds_input": string;
} | {
  "kind": "condition";
  "predicate": WorkflowPredicate;
} | {
  "kind": "compose";
} | {
  "candidates": Array<WorkflowOutputRef>;
  "kind": "select";
} | {
  "candidates": Array<WorkflowOutputRef>;
  "kind": "collect";
};

export type WorkflowControlStep = {
  "dependencies": Array<string>;
  "name": string;
  "operation": WorkflowControlOperation;
};

export type WorkflowDataContext = "inherit" | "bound_inputs_only";

export type WorkflowDataSource = {
  "source": "literal";
  "value": unknown;
} | {
  "pointer": string;
  "source": "input";
} | {
  "pointer": string;
  "source": "step";
  "step_id": string;
};

export type WorkflowDataType = {
  "type": "text";
} | {
  "type": "number";
} | {
  "type": "integer";
} | {
  "type": "boolean";
} | {
  "type": "null";
} | {
  "item": WorkflowDataType;
  "max_items": number;
  "type": "list";
} | {
  "fields": {
  [key: string]: WorkflowDataType;
};
  "type": "object";
} | {
  "type": "nullable";
  "value": WorkflowDataType;
};

export type WorkflowDataflow = {
  "input": WorkflowDataType;
  "steps": {
  [key: string]: WorkflowStepData;
};
};

export type WorkflowDefinition = {
  "budgets": Budgets;
  "controls"?: {
  [key: string]: WorkflowControlStep;
};
  "dataflow"?: WorkflowDataflow | null;
  "documents"?: {
  [key: string]: WorkflowDocumentStep;
};
  "failure": WorkflowFailure;
  "format_version": number;
  "gates"?: {
  [key: string]: WorkflowGate;
};
  "iterations"?: {
  [key: string]: WorkflowEmbeddedIteration;
};
  "mode": WorkflowMode;
  "name": string;
  "rights": WorkflowRights;
  "roles": Array<WorkflowRole>;
  "steps": Array<WorkflowStep>;
  "subflows"?: {
  [key: string]: WorkflowEmbeddedSubflow;
};
};

export type WorkflowDocumentFolder = {
  "kind": "current_workspace";
} | {
  "folder_id": string;
  "kind": "selected_folder";
};

export type WorkflowDocumentFormat = "docx" | "pdf";

export type WorkflowDocumentOperation = {
  "kind": "read_text";
  "offset": number;
  "path_input": string;
  "sha256_input": string;
} | {
  "kind": "preview";
  "pages": Array<number>;
  "path_input": string;
  "sha256_input": string;
} | {
  "dpi": number;
  "kind": "ocr";
  "languages": Array<string>;
  "max_words": number;
  "offset": number;
  "pages": Array<number>;
  "path_input": string;
  "rotation": number;
  "sha256_input": string;
} | {
  "content_input": string;
  "format": WorkflowDocumentFormat;
  "kind": "create";
  "path_input": string;
} | {
  "columns": Array<WorkflowTableColumn>;
  "key_fields": Array<string>;
  "kind": "append_table_rows";
  "output_path_input": string;
  "path_input": string;
  "rows_input": string;
  "sha256_input": string;
  "sheet": string;
  "table": string;
};

export type WorkflowDocumentStep = {
  "budgets": Budgets;
  "dependencies": Array<string>;
  "folder": WorkflowDocumentFolder;
  "name": string;
  "operation": WorkflowDocumentOperation;
  "rights": WorkflowRights;
};

export type WorkflowDraft = {
  "content": WorkflowDraftContent;
  "id": string;
  "project_id": string;
  "revision": number;
  "sha256": string;
  "snapshot_id": string;
  "updated_at": number;
};

export type WorkflowDraftContent = {
  "archived": boolean;
  "data": unknown;
  "editor_format": string;
  "origin"?: WorkflowDraftOrigin | null;
};

export type WorkflowDraftOrigin = {
  "base_revision": number;
  "version_id": string;
  "workflow_id": string;
};

export type WorkflowDraftPage = {
  "items": Array<WorkflowDraft>;
  "next_offset"?: number | null;
  "total": number;
};

export type WorkflowDraftSave = {
  "draft": WorkflowDraft;
  "outcome": "saved";
  "snapshot": WorkflowDraftSnapshot;
} | {
  "draft": WorkflowDraft;
  "outcome": "conflict";
  "snapshot": WorkflowDraftSnapshot;
};

export type WorkflowDraftSnapshot = {
  "base_revision": number;
  "conflict": boolean;
  "content": WorkflowDraftContent;
  "created_at": number;
  "draft_id": string;
  "id": string;
  "project_id": string;
  "revision": number;
  "sha256": string;
};

export type WorkflowDraftSnapshotPage = {
  "items": Array<WorkflowDraftSnapshotSummary>;
  "next_offset"?: number | null;
  "total": number;
};

export type WorkflowDraftSnapshotRef = {
  "draft_id": string;
  "sha256": string;
  "snapshot_id": string;
};

export type WorkflowDraftSnapshotSummary = {
  "base_revision": number;
  "conflict": boolean;
  "created_at": number;
  "draft_id": string;
  "id": string;
  "project_id": string;
  "revision": number;
  "sha256": string;
};

export type WorkflowEffectResolutionView = {
  "automatic_retry": boolean;
  "run": RunRecord;
  "settled_calls": Array<string>;
  "unresolved_calls": Array<WorkflowUnresolvedEffect>;
};

export type WorkflowEmbeddedIteration = {
  "arguments": {
  [key: string]: WorkflowIterationArgument;
};
  "budgets": Budgets;
  "collection_input": string;
  "definition": WorkflowDefinition;
  "dependencies": Array<string>;
  "max_parallel": number;
  "name": string;
  "result": WorkflowOutputRef;
  "rights": WorkflowRights;
};

export type WorkflowEmbeddedSubflow = {
  "budgets": Budgets;
  "definition": WorkflowDefinition;
  "dependencies": Array<string>;
  "name": string;
  "result": WorkflowOutputRef;
  "rights": WorkflowRights;
};

export type WorkflowFailure = "stop" | "continue_independent";

export type WorkflowGate = {
  "step_id": string;
  "when": boolean;
};

export type WorkflowInputAnswerExport = {
  "drafts": Array<WorkflowInputDraft>;
  "intents": Array<WorkflowInputAnswerIntent>;
  "owner": string;
  "snapshots": Array<WorkflowInputDraftSnapshot>;
};

export type WorkflowInputAnswerIntent = {
  "created_at": number;
  "draft_snapshot": WorkflowInputDraftSnapshotRef;
  "fingerprint": string;
  "id": string;
  "owner": string;
  "project_id": string;
  "receipt"?: WorkflowInputReceipt | null;
  "rejection"?: WorkflowInputAnswerRejection | null;
  "request_id": string;
  "request_sha256": string;
  "run_id": string;
  "state": WorkflowInputAnswerIntentState;
  "submission": SubmitWorkflowInput;
  "updated_at": number;
};

export type WorkflowInputAnswerIntentState = "pending" | "accepted" | "rejected";

export type WorkflowInputAnswerIntentSummary = {
  "answer_sha256": string;
  "created_at": number;
  "draft_revision": number;
  "id": string;
  "state": WorkflowInputAnswerIntentState;
};

export type WorkflowInputAnswerRejection = {
  "code": string;
  "message": string;
};

export type WorkflowInputDraft = {
  "answer": string;
  "answer_sha256": string;
  "fingerprint": string;
  "owner": string;
  "project_id": string;
  "request_id": string;
  "revision": number;
  "run_id": string;
  "snapshot_id"?: string | null;
  "updated_at": number;
};

export type WorkflowInputDraftLookup = {
  "draft": WorkflowInputDraft;
  "intents": Array<WorkflowInputAnswerIntentSummary>;
  "snapshots": Array<WorkflowInputDraftSnapshotSummary>;
};

export type WorkflowInputDraftSave = {
  "draft": WorkflowInputDraft;
  "outcome": "saved";
  "snapshot": WorkflowInputDraftSnapshot;
} | {
  "draft": WorkflowInputDraft;
  "outcome": "conflict";
  "snapshot": WorkflowInputDraftSnapshot;
};

export type WorkflowInputDraftSnapshot = {
  "answer": string;
  "answer_sha256": string;
  "base_revision": number;
  "conflict": boolean;
  "created_at": number;
  "fingerprint": string;
  "id": string;
  "owner": string;
  "project_id": string;
  "request_id": string;
  "revision": number;
  "run_id": string;
};

export type WorkflowInputDraftSnapshotRef = {
  "answer_sha256": string;
  "revision": number;
  "snapshot_id": string;
};

export type WorkflowInputDraftSnapshotSummary = {
  "answer_sha256": string;
  "base_revision": number;
  "conflict": boolean;
  "created_at": number;
  "id": string;
  "revision": number;
};

export type WorkflowInputReceipt = {
  "answer_sha256": string;
  "confirmed_sequence": number;
  "fingerprint": string;
  "receipt_id": string;
  "request_id": string;
};

export type WorkflowInputResumePreview = {
  "available": boolean;
  "base_run_revision": number;
  "base_workflow_revision": number;
  "blocked_reason"?: string | null;
  "expected_checkpoint_sha256": string;
  "expected_cursor_sha256"?: string | null;
};

export type WorkflowInputSnapshot = {
  "canonical_json": string;
  "definition_sha256": string;
  "project_id": string;
  "source": WorkflowInputSource;
  "version_id": string;
  "workflow_id": string;
};

export type WorkflowInputSource = {
  "run_id": string;
  "sha256": string;
};

export type WorkflowInputState = "waiting_answer" | "answered" | "closed";

export type WorkflowInputView = {
  "actionable": boolean;
  "closed_reason"?: string | null;
  "expires_at": number;
  "fingerprint": string;
  "node_id": string;
  "question": string;
  "receipt"?: WorkflowInputReceipt | null;
  "request_id": string;
  "review"?: WorkflowReviewRequest | null;
  "role_id"?: string | null;
  "state": WorkflowInputState;
};

export type WorkflowIterationArgument = {
  "pointer"?: string;
  "source": "item";
} | {
  "source": "index";
} | {
  "pointer": string;
  "source": "input";
} | {
  "source": "literal";
  "value": unknown;
};

export type WorkflowLibraryEntry = {
  "id": string;
  "name": string;
  "project_id": string;
  "revision": number;
  "revoked": boolean;
  "versions": Array<WorkflowVersionRef>;
};

export type WorkflowMode = {
  "mode": "sequential";
} | {
  "max_parallel": number;
  "mode": "parallel";
} | {
  "manager_step_id"?: string | null;
  "max_delegations": number;
  "max_depth": number;
  "max_parallel": number;
  "mode": "hierarchical";
};

export type WorkflowModelView = {
  "provider": ProviderProfile;
  "role_id": string;
};

export type WorkflowNodeKind = "subflow" | "arguments";

export type WorkflowNodeStatus = "waiting_timer" | "skipped" | "pending" | "ready" | "running" | "waiting_approval" | "waiting_input" | "completed" | "failed" | "blocked" | "interrupted";

export type WorkflowNodeView = {
  "charged_tokens": number;
  "dependencies": Array<string>;
  "error_code"?: string | null;
  "goal": string;
  "id": string;
  "kind"?: WorkflowNodeKind | null;
  "role_id"?: string | null;
  "scope_id"?: string | null;
  "skip_reason"?: string | null;
  "state": WorkflowNodeStatus;
  "steps": number;
  "wake_at"?: number | null;
};

export type WorkflowOperation = {
  "acknowledged_at"?: number | null;
  "created_at": number;
  "draft_snapshot"?: WorkflowDraftSnapshotRef | null;
  "id": string;
  "intent_id": string;
  "project_id": string;
  "receipt"?: WorkflowOperationReceipt | null;
  "rejection"?: WorkflowOperationRejection | null;
  "request": WorkflowOperationRequest;
  "request_sha256": string;
  "state": WorkflowOperationState;
  "updated_at": number;
};

export type WorkflowOperationPage = {
  "items": Array<WorkflowOperation>;
  "next_offset"?: number | null;
  "total": number;
};

export type WorkflowOperationReceipt = {
  "kind": "created";
  "publication": WorkflowPublication;
} | {
  "kind": "published";
  "publication": WorkflowPublication;
} | {
  "kind": "revoked";
  "publication": WorkflowPublication;
} | {
  "created_at": number;
  "kind": "started";
  "run_id": string;
  "spec": RunSpec;
} | {
  "accepted_at": number;
  "accepted_run_revision": number;
  "accepted_workflow_revision": number;
  "confirmed_sequence": number;
  "cursor_sha256": string;
  "generation": number;
  "kind": "input_resume_queued";
  "resume_id": string;
  "run_id": string;
};

export type WorkflowOperationRejection = {
  "code": string;
  "message": string;
};

export type WorkflowOperationRequest = {
  "kind": "create";
  "request": CreateWorkflow;
} | {
  "kind": "publish";
  "request": PublishWorkflow;
  "workflow_id": string;
} | {
  "kind": "revoke";
  "request": RevokeWorkflow;
  "workflow_id": string;
} | {
  "kind": "start";
  "request": WorkflowRunRequest;
} | {
  "kind": "resume_input";
  "request": ResumeWorkflowInput;
};

export type WorkflowOperationState = "pending" | "accepted" | "rejected" | "discarded";

export type WorkflowOutputRef = {
  "pointer": string;
  "step_id": string;
};

export type WorkflowPage = {
  "items": Array<WorkflowLibraryEntry>;
  "next_offset"?: number | null;
  "total": number;
};

export type WorkflowPredicate = {
  "input": string;
  "operator": "equal";
  "value": unknown;
} | {
  "input": string;
  "operator": "not_equal";
  "value": unknown;
} | {
  "input": string;
  "operator": "greater";
  "value": unknown;
} | {
  "input": string;
  "operator": "less";
  "value": unknown;
} | {
  "input": string;
  "operator": "contains";
  "value": string;
} | {
  "input": string;
  "operator": "is_true";
} | {
  "input": string;
  "operator": "is_null";
} | {
  "operator": "all";
  "predicates": Array<WorkflowPredicate>;
} | {
  "operator": "any";
  "predicates": Array<WorkflowPredicate>;
} | {
  "operator": "not";
  "predicate": WorkflowPredicate;
};

export type WorkflowPreflight = {
  "access_mode": AccessMode;
  "allowed_tools": Array<string>;
  "budgets": Budgets;
  "executor": string;
  "input_source"?: WorkflowInputSource | null;
  "max_parallel": number;
  "name": string;
  "policy_revision": number;
  "ready": boolean;
  "roles": Array<WorkflowModelView>;
  "version_revision": number;
  "workflow": WorkflowBinding;
};

export type WorkflowPublicState = "waiting_timer" | "queued" | "running" | "interrupted" | "completed" | "failed" | "canceled";

export type WorkflowPublication = {
  "created_at": number;
  "project_id": string;
  "revision": number;
  "revoked": boolean;
  "version_id"?: string | null;
  "workflow_id": string;
};

export type WorkflowReviewCheck = {
  "kind": "sum";
  "terms": Array<string>;
  "total": string;
};

export type WorkflowReviewEvidence = {
  "kind": WorkflowReviewEvidenceKind;
  "name": string;
  "output": unknown;
  "step_id": string;
};

export type WorkflowReviewEvidenceKind = "text" | "ocr";

export type WorkflowReviewField = {
  "format": WorkflowReviewFormat;
  "help": string;
  "label": string;
  "maximum"?: number | null;
  "minimum"?: number | null;
  "required": boolean;
  "value_type": WorkflowDataType;
};

export type WorkflowReviewForm = {
  "checks": Array<WorkflowReviewCheck>;
  "fields": {
  [key: string]: WorkflowReviewField;
};
  "question": string;
  "source_steps"?: Array<string>;
};

export type WorkflowReviewFormat = "value" | "date" | "money_minor";

export type WorkflowReviewRequest = {
  "evidence"?: Array<WorkflowReviewEvidence>;
  "form": WorkflowReviewForm;
  "values": unknown;
};

export type WorkflowRights = {
  "current_workspace"?: boolean;
  "folders": Array<string>;
  "tools": Array<string>;
};

export type WorkflowRole = {
  "agent_config"?: EffectiveAgentConfig | null;
  "budgets": Budgets;
  "delegate_to": Array<string>;
  "id": string;
  "instructions": string;
  "model_binding"?: ModelRegistryRef | null;
  "output"?: StructuredOutput | null;
  "provider_id": string;
  "provider_revision": number;
  "rights": WorkflowRights;
};

export type WorkflowRunApproval = {
  "approval": Approval;
  "node_id": string;
  "role_id"?: string | null;
};

export type WorkflowRunKind = "workflow";

export type WorkflowRunRequest = {
  "access_mode"?: AccessMode;
  "agent_config"?: EffectiveAgentConfig | null;
  "base_revision": number;
  "budgets"?: Budgets;
  "environment"?: ExecutionEnvironmentRef | null;
  "expected_binding"?: WorkflowBinding | null;
  "folders"?: Array<FolderGrant>;
  "idempotency_key": string;
  "input_source"?: WorkflowInputSource | null;
  "inputs"?: unknown;
  "kind": WorkflowRunKind;
  "mcp_tools"?: Array<McpToolSelection>;
  "native_tools"?: Array<string>;
  "order_id": string;
  "prompt": string;
  "version_id": string;
  "workflow_id": string;
};

export type WorkflowRunView = {
  "approvals": Array<WorkflowRunApproval>;
  "binding": WorkflowBinding;
  "charged_tokens": number;
  "completion_resume_available": boolean;
  "input_resume_available"?: boolean;
  "input_resume_blocked_reason"?: string | null;
  "input_resume_preview"?: WorkflowInputResumePreview;
  "inputs"?: Array<WorkflowInputView>;
  "max_parallel": number;
  "mode": WorkflowMode;
  "name": string;
  "nodes": Array<WorkflowNodeView>;
  "roles": Array<WorkflowModelView>;
  "run": RunRecord;
  "settled_resume_available"?: boolean;
  "settled_resume_blocked_reason"?: string | null;
  "state": WorkflowPublicState;
  "steps_used": number;
  "version_revision": number;
};

export type WorkflowSnapshot = {
  "agent_contexts"?: unknown;
  "binding": WorkflowBinding;
  "human_inputs"?: unknown;
  "plan": unknown;
  "progress": unknown;
  "progress_sha256": string;
  "project_id": string;
  "run_id": string;
};

export type WorkflowStep = {
  "budgets": Budgets;
  "dependencies": Array<string>;
  "execution"?: WorkflowStepExecution | null;
  "goal": string;
  "id": string;
  "rights": WorkflowRights;
  "role_id": string;
  "zero_context"?: boolean;
};

export type WorkflowStepData = {
  "context_policy"?: WorkflowDataContext;
  "inputs": {
  [key: string]: WorkflowValueBinding;
};
  "output": WorkflowDataType;
};

export type WorkflowStepExecution = {
  "kind": "web_fetch";
  "url": string;
} | {
  "kind": "web_search";
  "query": string;
};

export type WorkflowTableColumn = {
  "field": string;
  "header": string;
  "kind": WorkflowTableColumnKind;
  "nullable": boolean;
};

export type WorkflowTableColumnKind = "text" | "integer" | "number" | "boolean" | "date" | "money_minor";

export type WorkflowUnresolvedEffect = {
  "id": string;
  "reason": string;
};

export type WorkflowValueBinding = {
  "value": WorkflowDataSource;
  "value_type": WorkflowDataType;
};

export type WorkflowVersion = {
  "definition": WorkflowDefinition;
  "project_id": string;
  "reference": WorkflowVersionRef;
  "workflow_id": string;
};

export type WorkflowVersionRef = {
  "created_at": number;
  "id": string;
  "name": string;
  "revision": number;
  "sha256": string;
};

export type Workspace = {
  "base_snapshot_sha256"?: string | null;
  "folder": FolderGrant;
  "order_id": string;
};

export type WorkspaceEntry = {
  "path": string;
  "sha256"?: string | null;
  "size": number;
};

export type WorkspaceManagementContract = {
  "archive_chat": ArchiveChat;
  "archive_workspace_task": ArchiveWorkspaceTask;
  "chat_configuration": ChatConfiguration;
  "chat_management": ChatManagement;
  "manage_chat": ManageChat;
  "project_settings": ProjectSettings;
  "start_workspace_task": StartWorkspaceTask;
  "workspace_task": WorkspaceTask;
  "write_chat_configuration": WriteChatConfiguration;
  "write_project_settings": WriteProjectSettings;
  "write_workspace_task": WriteWorkspaceTask;
};

export type WorkspaceSnapshot = {
  "entries": Array<WorkspaceEntry>;
  "order_id": string;
  "run_id": string;
  "sha256": string;
};

export type WorkspaceTask = {
  "archived": boolean;
  "created_at": number;
  "id": string;
  "last_run_id"?: string | null;
  "project_id": string;
  "revision": number;
  "updated_at": number;
  "value": WorkspaceTaskDefinition;
};

export type WorkspaceTaskDefinition = {
  "access_mode": AccessMode;
  "agent_config"?: EffectiveAgentConfig | null;
  "budgets": Budgets;
  "description": string;
  "environment"?: ExecutionEnvironmentRef | null;
  "execution": WorkspaceTaskExecution;
  "expected_output": string;
  "folders"?: Array<FolderGrant>;
  "mcp_tools"?: Array<McpToolSelection>;
  "name": string;
  "native_tools"?: Array<string>;
  "order_id": string;
  "prompt": string;
};

export type WorkspaceTaskExecution = {
  "kind": "model";
  "provider_id": string;
  "provider_revision": number;
} | {
  "inputs"?: unknown;
  "kind": "workflow";
  "version_id": string;
  "workflow_id": string;
};

export type WorkspaceTransferChunk = {
  "offset": number;
  "sha256": string;
  "size": number;
};

export type WorkspaceTransferFile = {
  "chunks": Array<WorkspaceTransferChunk>;
  "path": string;
  "sha256": string;
  "size": number;
};

export type WorkspaceTransferManifest = {
  "directories": Array<string>;
  "files": Array<WorkspaceTransferFile>;
  "format_version": number;
  "sha256": string;
  "total_bytes": number;
};

export type WriteActorChatConfiguration = {
  "expected_revision": number;
  "idempotency_key": string;
  "value": ActorChatRunConfiguration;
};

export type WriteActorMemory = {
  "expected_revision": number;
  "request_id": string;
  "value": ActorMemoryDefinition;
};

export type WriteActorSkill = {
  "expected_revision": number;
  "request_id": string;
  "value": ActorSkillDefinition;
};

export type WriteAgentTemplate = {
  "expected_revision": number;
  "idempotency_key": string;
  "value": AgentTemplateDefinition;
};

export type WriteAutomationTrigger = {
  "enabled": boolean;
  "expected_revision": number;
  "file_watch"?: AutomationFileWatch | null;
  "id"?: string | null;
  "idempotency_key": string;
  "name": string;
  "predicates": Array<EventPredicate>;
  "source": AutomationEventSource;
  "task_id": string;
  "task_revision": number;
  "workflow_inputs"?: AutomationWorkflowInputs | null;
};

export type WriteChatConfiguration = {
  "expected_revision": number;
  "idempotency_key": string;
  "value": ChatRunConfiguration;
};

export type WriteCodexAccount = {
  "expected_revision": number;
  "idempotency_key": string;
  "value": CodexAccountDefinition;
};

export type WriteExternalMemoryProvider = {
  "api_key"?: string | null;
  "expected_revision": number;
  "remove_key"?: boolean;
  "request_id": string;
  "value": ExternalMemoryDefinition;
};

export type WriteKnowledge = {
  "content": string;
  "expected_revision": number;
  "id"?: string | null;
  "kind": KnowledgeKind;
  "location"?: KnowledgeLocation | null;
  "memory"?: MemoryMetadata | null;
  "provenance"?: KnowledgeProvenance | null;
  "tags"?: Array<string>;
  "title": string;
};

export type WriteKnowledgeFolders = {
  "expected_revision": number;
  "folders": Array<KnowledgeFolder>;
};

export type WritePersonality = {
  "expected_revision": number;
  "idempotency_key": string;
  "value": PersonalityDefinition;
};

export type WritePreferences = {
  "expected_revision": number;
  "idempotency_key": string;
  "value": ApplicationPreferences;
};

export type WriteProjectSettings = {
  "expected_revision": number;
  "idempotency_key": string;
  "instructions": string;
  "name": string;
  "sources": Array<ProjectSource>;
  "working_folder"?: string | null;
};

export type WriteSchedule = {
  "expected_revision": number;
  "id"?: string | null;
  "name": string;
  "paused"?: boolean;
  "recurrence": ScheduleRecurrence;
  "template": ScheduledRunTemplate;
  "timezone": string;
};

export type WriteSkill = {
  "auto_learn"?: boolean;
  "description"?: string;
  "enabled"?: boolean;
  "expected_revision": number;
  "id"?: string | null;
  "instructions": string;
  "name": string;
  "prompt_template"?: string | null;
  "run_mode"?: AgentExecutionMode;
  "trigger_pattern"?: string | null;
};

export type WriteTextFile = {
  "content": string;
  "expected_sha256"?: string | null;
  "folder_id": string;
  "path": string;
};

export type WriteWorkspaceTask = {
  "expected_revision": number;
  "idempotency_key": string;
  "value": WorkspaceTaskDefinition;
};

export interface ContractTypes {
  AccessMode: AccessMode;
  ActivateHistoricalImport: ActivateHistoricalImport;
  ActivityItem: ActivityItem;
  ActivityPage: ActivityPage;
  ActorChatConfiguration: ActorChatConfiguration;
  ActorChatRunConfiguration: ActorChatRunConfiguration;
  ActorContextRecord: ActorContextRecord;
  ActorMemoryDefinition: ActorMemoryDefinition;
  ActorModelContext: ActorModelContext;
  ActorModelRegistryView: ActorModelRegistryView;
  ActorRunConfiguration: ActorRunConfiguration;
  ActorSkillDefinition: ActorSkillDefinition;
  ActorSkillLearningHistory: ActorSkillLearningHistory;
  ActorSkillLearningOutcome: ActorSkillLearningOutcome;
  AddTeamMember: AddTeamMember;
  AgentBehaviorSettings: AgentBehaviorSettings;
  AgentConfiguration: AgentConfiguration;
  AgentExecutionMode: AgentExecutionMode;
  AgentModelSelection: AgentModelSelection;
  AgentTemplate: AgentTemplate;
  AgentTemplateDefinition: AgentTemplateDefinition;
  AndroidAcceptInvitation: AndroidAcceptInvitation;
  AndroidAdministrationRevision: AndroidAdministrationRevision;
  AndroidApplyProjectMerge: AndroidApplyProjectMerge;
  AndroidApplyProjectVersion: AndroidApplyProjectVersion;
  AndroidAuthManagementPolicy: AndroidAuthManagementPolicy;
  AndroidAuthSecurityContext: AndroidAuthSecurityContext;
  AndroidAuthSession: AndroidAuthSession;
  AndroidAuthTokens: AndroidAuthTokens;
  AndroidAuthenticationAssurance: AndroidAuthenticationAssurance;
  AndroidBrowserAuthTokens: AndroidBrowserAuthTokens;
  AndroidContentAccess: AndroidContentAccess;
  AndroidCreateInvitation: AndroidCreateInvitation;
  AndroidCreateProjectVersion: AndroidCreateProjectVersion;
  AndroidCreateSchedule: AndroidCreateSchedule;
  AndroidCreateScheduleTask: AndroidCreateScheduleTask;
  AndroidCreateTeam: AndroidCreateTeam;
  AndroidCreateTerminalSession: AndroidCreateTerminalSession;
  AndroidCreateThreadSchedule: AndroidCreateThreadSchedule;
  AndroidDisableTotp: AndroidDisableTotp;
  AndroidExecutorPoolProjectWrite: AndroidExecutorPoolProjectWrite;
  AndroidExecutorPoolWrite: AndroidExecutorPoolWrite;
  AndroidExecutorSyncBatch: AndroidExecutorSyncBatch;
  AndroidExecutorSyncGrantWrite: AndroidExecutorSyncGrantWrite;
  AndroidForkDestination: AndroidForkDestination;
  AndroidForkObjectMapping: AndroidForkObjectMapping;
  AndroidForkProject: AndroidForkProject;
  AndroidForkThread: AndroidForkThread;
  AndroidInvitationSecret: AndroidInvitationSecret;
  AndroidMergeFileResolution: AndroidMergeFileResolution;
  AndroidMergeFileReview: AndroidMergeFileReview;
  AndroidMergeFileStatus: AndroidMergeFileStatus;
  AndroidMergeResolutionChoice: AndroidMergeResolutionChoice;
  AndroidMobileCacheActivate: AndroidMobileCacheActivate;
  AndroidMobileCacheBegin: AndroidMobileCacheBegin;
  AndroidNativeAuthorization: AndroidNativeAuthorization;
  AndroidNativeAuthorizationCode: AndroidNativeAuthorizationCode;
  AndroidNativeTokenExchange: AndroidNativeTokenExchange;
  AndroidOidcBegin: AndroidOidcBegin;
  AndroidOidcCompatStart: AndroidOidcCompatStart;
  AndroidOidcComplete: AndroidOidcComplete;
  AndroidOidcConfig: AndroidOidcConfig;
  AndroidOidcFinish: AndroidOidcFinish;
  AndroidOidcFlow: AndroidOidcFlow;
  AndroidOidcGroupMapping: AndroidOidcGroupMapping;
  AndroidOidcLink: AndroidOidcLink;
  AndroidOidcMembershipOrigin: AndroidOidcMembershipOrigin;
  AndroidOidcOrigin: AndroidOidcOrigin;
  AndroidOidcProfile: AndroidOidcProfile;
  AndroidOidcProfileWrite: AndroidOidcProfileWrite;
  AndroidOidcRecover: AndroidOidcRecover;
  AndroidOidcRetire: AndroidOidcRetire;
  AndroidOidcSsoProvider: AndroidOidcSsoProvider;
  AndroidOidcSsoSettings: AndroidOidcSsoSettings;
  AndroidOidcSsoWrite: AndroidOidcSsoWrite;
  AndroidOidcUnlink: AndroidOidcUnlink;
  AndroidPasswordCredentials: AndroidPasswordCredentials;
  AndroidPersonalStorageQuotaWrite: AndroidPersonalStorageQuotaWrite;
  AndroidPrivateProjectMemberPage: AndroidPrivateProjectMemberPage;
  AndroidPrivateProjectMemberReceipt: AndroidPrivateProjectMemberReceipt;
  AndroidPrivateProjectMemberView: AndroidPrivateProjectMemberView;
  AndroidProjectForkReceipt: AndroidProjectForkReceipt;
  AndroidProjectMemberPage: AndroidProjectMemberPage;
  AndroidProjectMemberView: AndroidProjectMemberView;
  AndroidProjectMemberWrite: AndroidProjectMemberWrite;
  AndroidProjectMergeReview: AndroidProjectMergeReview;
  AndroidProjectVersion: AndroidProjectVersion;
  AndroidProjectVersionContent: AndroidProjectVersionContent;
  AndroidRecoveryCodes: AndroidRecoveryCodes;
  AndroidRefreshSession: AndroidRefreshSession;
  AndroidResumeRun: AndroidResumeRun;
  AndroidRunCheckpoint: AndroidRunCheckpoint;
  AndroidRunCheckpointDescriptor: AndroidRunCheckpointDescriptor;
  AndroidRunCheckpointPage: AndroidRunCheckpointPage;
  AndroidRunResumeReceipt: AndroidRunResumeReceipt;
  AndroidRunWaiting: AndroidRunWaiting;
  AndroidRunWaitingReason: AndroidRunWaitingReason;
  AndroidScheduleHistory: AndroidScheduleHistory;
  AndroidScheduleOccurrence: AndroidScheduleOccurrence;
  AndroidScheduleOriginal: AndroidScheduleOriginal;
  AndroidSchedulePreview: AndroidSchedulePreview;
  AndroidScheduleRecord: AndroidScheduleRecord;
  AndroidScheduleResolutionReceipt: AndroidScheduleResolutionReceipt;
  AndroidScheduleResolutionRequest: AndroidScheduleResolutionRequest;
  AndroidScheduleResolutionView: AndroidScheduleResolutionView;
  AndroidSetTeamMember: AndroidSetTeamMember;
  AndroidSupportGrantRevoke: AndroidSupportGrantRevoke;
  AndroidSupportGrantWrite: AndroidSupportGrantWrite;
  AndroidTeam: AndroidTeam;
  AndroidTeamMember: AndroidTeamMember;
  AndroidTerminalResize: AndroidTerminalResize;
  AndroidTerminalSession: AndroidTerminalSession;
  AndroidTerminalSessionPage: AndroidTerminalSessionPage;
  AndroidTerminalSessionTicket: AndroidTerminalSessionTicket;
  AndroidThreadSchedule: AndroidThreadSchedule;
  AndroidTotpCode: AndroidTotpCode;
  AndroidTotpSetup: AndroidTotpSetup;
  AndroidTotpStatus: AndroidTotpStatus;
  AndroidUpdateSchedule: AndroidUpdateSchedule;
  AndroidWireSyncBatch: AndroidWireSyncBatch;
  AndroidWireSyncChange: AndroidWireSyncChange;
  AndroidWorkflowChange: AndroidWorkflowChange;
  AndroidWorkflowEngine: AndroidWorkflowEngine;
  AndroidWorkflowEntry: AndroidWorkflowEntry;
  AndroidWorkflowNodeView: AndroidWorkflowNodeView;
  AndroidWorkflowOperationReceipt: AndroidWorkflowOperationReceipt;
  AndroidWorkflowPage: AndroidWorkflowPage;
  AndroidWorkflowPreflight: AndroidWorkflowPreflight;
  AndroidWorkflowRunView: AndroidWorkflowRunView;
  AndroidWorkflowSelection: AndroidWorkflowSelection;
  AndroidWorkflowVersion: AndroidWorkflowVersion;
  AndroidWorkflowVersionPage: AndroidWorkflowVersionPage;
  AndroidWorkflowVersionRef: AndroidWorkflowVersionRef;
  AndroidWorkflowWrite: AndroidWorkflowWrite;
  AnswerMcpTaskInteraction: AnswerMcpTaskInteraction;
  ApiError: ApiError;
  ApplicationPreferences: ApplicationPreferences;
  Approval: Approval;
  ApproveOfflineTools: ApproveOfflineTools;
  ApproveTeamTool: ApproveTeamTool;
  ArchiveChat: ArchiveChat;
  ArchiveWorkspaceTask: ArchiveWorkspaceTask;
  Artifact: Artifact;
  AuditArchiveBundle: AuditArchiveBundle;
  AuditCheckpointRequest: AuditCheckpointRequest;
  AuditCheckpointStatus: AuditCheckpointStatus;
  AuditCost: AuditCost;
  AuditCount: AuditCount;
  AuditDiagnostics: AuditDiagnostics;
  AuditExecutionInsights: AuditExecutionInsights;
  AuditExecutionTotals: AuditExecutionTotals;
  AuditInsights: AuditInsights;
  AuditInsightsQuery: AuditInsightsQuery;
  AuditMetricFact: AuditMetricFact;
  AuditOrigin: AuditOrigin;
  AuditOriginRotationPlan: AuditOriginRotationPlan;
  AuditOriginalReference: AuditOriginalReference;
  AuditPage: AuditPage;
  AuditRecord: AuditRecord;
  AuditRecoveryPlan: AuditRecoveryPlan;
  AuditRecoveryReceipt: AuditRecoveryReceipt;
  AuditSkillUse: AuditSkillUse;
  AuditTrustTransitionProposal: AuditTrustTransitionProposal;
  AuditV1Assessment: AuditV1Assessment;
  AuditV1Verification: AuditV1Verification;
  AuditVerification: AuditVerification;
  AuditWitnessBinding: AuditWitnessBinding;
  AutomationContract: AutomationContract;
  AutomationEvent: AutomationEvent;
  AutomationEventPage: AutomationEventPage;
  AutomationEventPreview: AutomationEventPreview;
  AutomationEventSource: AutomationEventSource;
  AutomationEventState: AutomationEventState;
  AutomationFileWatch: AutomationFileWatch;
  AutomationFileWatchStatus: AutomationFileWatchStatus;
  AutomationTrigger: AutomationTrigger;
  AutomationWebhookGrant: AutomationWebhookGrant;
  AutomationWorkflowInputSnapshot: AutomationWorkflowInputSnapshot;
  AutomationWorkflowInputs: AutomationWorkflowInputs;
  BeginHistoricalImport: BeginHistoricalImport;
  BranchOrder: BranchOrder;
  BrokerControllerRef: BrokerControllerRef;
  BrowserCapabilities: BrowserCapabilities;
  BrowserCommand: BrowserCommand;
  BrowserEngine: BrowserEngine;
  BrowserExtensionConnection: BrowserExtensionConnection;
  BrowserInput: BrowserInput;
  BrowserMode: BrowserMode;
  BrowserSessionView: BrowserSessionView;
  BrowserTab: BrowserTab;
  BrowserTakeover: BrowserTakeover;
  Budgets: Budgets;
  BuildHistoricalHistoryIndex: BuildHistoricalHistoryIndex;
  BuildKnowledgeGraph: BuildKnowledgeGraph;
  CaptureAuditArchive: CaptureAuditArchive;
  CaptureAuditCheckpoint: CaptureAuditCheckpoint;
  ChatConfiguration: ChatConfiguration;
  ChatManagement: ChatManagement;
  ChatRunConfiguration: ChatRunConfiguration;
  ChatWorkflowSelection: ChatWorkflowSelection;
  CloseHistoricalUnsentFile: CloseHistoricalUnsentFile;
  CodexAccount: CodexAccount;
  CodexAccountAction: CodexAccountAction;
  CodexAccountDefinition: CodexAccountDefinition;
  CodexAccountStatus: CodexAccountStatus;
  CodexAccountsContract: CodexAccountsContract;
  CodexLoginFlow: CodexLoginFlow;
  CodexLoginRequest: CodexLoginRequest;
  CodexLoginView: CodexLoginView;
  CodexModel: CodexModel;
  CodexRuntimeStatus: CodexRuntimeStatus;
  CommitExecutionPlan: CommitExecutionPlan;
  CompactModelContext: CompactModelContext;
  ConfigureAuditWitness: ConfigureAuditWitness;
  ConfigureAutomationWebhook: ConfigureAutomationWebhook;
  ConfigureHistoricalImportLimit: ConfigureHistoricalImportLimit;
  ConfigureMcpOAuth: ConfigureMcpOAuth;
  ConfigureMcpServer: ConfigureMcpServer;
  ConfigureOfflineCredential: ConfigureOfflineCredential;
  ConfigureOfflineTools: ConfigureOfflineTools;
  ConfigureOwnerMcpCredential: ConfigureOwnerMcpCredential;
  ConfigureOwnerMcpStdio: ConfigureOwnerMcpStdio;
  ConfigurePersonalProvider: ConfigurePersonalProvider;
  ConfigureTeamGithub: ConfigureTeamGithub;
  ConfigureTeamMcpServer: ConfigureTeamMcpServer;
  ConfigureTeamProvider: ConfigureTeamProvider;
  ConfirmAuditRecovery: ConfirmAuditRecovery;
  ConfirmOwnerManagedResultDelivery: ConfirmOwnerManagedResultDelivery;
  ContextArchiveView: ContextArchiveView;
  ContextCompactionView: ContextCompactionView;
  ContextGroupView: ContextGroupView;
  CreateBrowserSession: CreateBrowserSession;
  CreateImportRun: CreateImportRun;
  CreateOrder: CreateOrder;
  CreateProject: CreateProject;
  CreateProvider: CreateProvider;
  CreateRestoreRun: CreateRestoreRun;
  CreateRun: CreateRun;
  CreateSandboxPreview: CreateSandboxPreview;
  CreateTeam: CreateTeam;
  CreateTeamOrder: CreateTeamOrder;
  CreateTeamProject: CreateTeamProject;
  CreateTeamRun: CreateTeamRun;
  CreateWorkflow: CreateWorkflow;
  DatabaseBackup: DatabaseBackup;
  DatabaseBackupSaveOutcome: DatabaseBackupSaveOutcome;
  DecimalU64: DecimalU64;
  DeleteActorContext: DeleteActorContext;
  DeleteBrowserProfile: DeleteBrowserProfile;
  DeleteExternalMemoryProvider: DeleteExternalMemoryProvider;
  DeleteLibraryEntry: DeleteLibraryEntry;
  DeleteOwnerConnector: DeleteOwnerConnector;
  DeleteProviderSettings: DeleteProviderSettings;
  DeviceCapability: DeviceCapability;
  DeviceCredentialMetadata: DeviceCredentialMetadata;
  DeviceCrewAgent: DeviceCrewAgent;
  DeviceCrewDefinition: DeviceCrewDefinition;
  DeviceCrewTask: DeviceCrewTask;
  DeviceExecutorRegistration: DeviceExecutorRegistration;
  DeviceFeatureConfiguration: DeviceFeatureConfiguration;
  DeviceInteropAdapter: DeviceInteropAdapter;
  DeviceInteropInventory: DeviceInteropInventory;
  DeviceOfficeRequest: DeviceOfficeRequest;
  DevicePairingRecord: DevicePairingRecord;
  DevicePasskeyChallenge: DevicePasskeyChallenge;
  DevicePasskeyRecord: DevicePasskeyRecord;
  DevicePushSubscription: DevicePushSubscription;
  DirectoryListing: DirectoryListing;
  DiscoverProviderModels: DiscoverProviderModels;
  DocumentActionRequest: DocumentActionRequest;
  DocumentDraft: DocumentDraft;
  DocumentDraftSave: DocumentDraftSave;
  DocumentDraftSnapshot: DocumentDraftSnapshot;
  DocumentPreparation: DocumentPreparation;
  DocumentProvenance: DocumentProvenance;
  DocumentProvenanceInput: DocumentProvenanceInput;
  DownloadFile: DownloadFile;
  DownloadMetadata: DownloadMetadata;
  EffectiveAgentConfig: EffectiveAgentConfig;
  EventPredicate: EventPredicate;
  ExecutionActor: ExecutionActor;
  ExecutionBoundary: ExecutionBoundary;
  ExecutionDestinationV2: ExecutionDestinationV2;
  ExecutionEnvironmentRef: ExecutionEnvironmentRef;
  ExecutionInputRef: ExecutionInputRef;
  ExecutionInputSnapshot: ExecutionInputSnapshot;
  ExecutionInputSource: ExecutionInputSource;
  ExecutionNetworkAccess: ExecutionNetworkAccess;
  ExecutionPlan: ExecutionPlan;
  ExecutionPlanV2: ExecutionPlanV2;
  ExecutionPreflightV2: ExecutionPreflightV2;
  ExecutionProcessAccess: ExecutionProcessAccess;
  ExecutionRunnerIdentity: ExecutionRunnerIdentity;
  ExecutionTargetRef: ExecutionTargetRef;
  ExternalMemoryDefinition: ExternalMemoryDefinition;
  ExternalMemoryItem: ExternalMemoryItem;
  ExternalMemoryProvider: ExternalMemoryProvider;
  ExternalMemoryRetrieval: ExternalMemoryRetrieval;
  FileChange: FileChange;
  FileEntry: FileEntry;
  FileLocation: FileLocation;
  FileNode: FileNode;
  FileNodeKind: FileNodeKind;
  FileOperation: FileOperation;
  FileOperationKind: FileOperationKind;
  FileRestoreSource: FileRestoreSource;
  FileStat: FileStat;
  FileUpload: FileUpload;
  FileWatchChange: FileWatchChange;
  FinishDevicePasskeyAuthentication: FinishDevicePasskeyAuthentication;
  FinishDevicePasskeyRegistration: FinishDevicePasskeyRegistration;
  FolderGrant: FolderGrant;
  FolderRequestLookup: FolderRequestLookup;
  GenerationSettings: GenerationSettings;
  Health: Health;
  HistoricalActivatedObject: HistoricalActivatedObject;
  HistoricalActivationReceipt: HistoricalActivationReceipt;
  HistoricalChunkReceipt: HistoricalChunkReceipt;
  HistoricalContentArtifact: HistoricalContentArtifact;
  HistoricalContentCatalogEntry: HistoricalContentCatalogEntry;
  HistoricalContentCatalogItem: HistoricalContentCatalogItem;
  HistoricalContentCatalogManifest: HistoricalContentCatalogManifest;
  HistoricalContentCatalogPage: HistoricalContentCatalogPage;
  HistoricalContentDocument: HistoricalContentDocument;
  HistoricalContentFile: HistoricalContentFile;
  HistoricalContentMessage: HistoricalContentMessage;
  HistoricalContentOrder: HistoricalContentOrder;
  HistoricalContentPage: HistoricalContentPage;
  HistoricalContentProject: HistoricalContentProject;
  HistoricalFileDeclaration: HistoricalFileDeclaration;
  HistoricalFileOperation: HistoricalFileOperation;
  HistoricalFileStageRequest: HistoricalFileStageRequest;
  HistoricalHistoryCell: HistoricalHistoryCell;
  HistoricalHistoryCellsPage: HistoricalHistoryCellsPage;
  HistoricalHistoryColumn: HistoricalHistoryColumn;
  HistoricalHistoryIndex: HistoricalHistoryIndex;
  HistoricalHistoryPagePin: HistoricalHistoryPagePin;
  HistoricalHistoryRow: HistoricalHistoryRow;
  HistoricalHistoryRowsPage: HistoricalHistoryRowsPage;
  HistoricalHistoryTable: HistoricalHistoryTable;
  HistoricalHistoryTablesPage: HistoricalHistoryTablesPage;
  HistoricalImportLimit: HistoricalImportLimit;
  HistoricalImportPreview: HistoricalImportPreview;
  HistoricalImportRecord: HistoricalImportRecord;
  HistoricalImportTarget: HistoricalImportTarget;
  HistoricalOwner: HistoricalOwner;
  HistoricalPinSet: HistoricalPinSet;
  HistoricalPreparedProject: HistoricalPreparedProject;
  HistoricalPreviewProjectSummary: HistoricalPreviewProjectSummary;
  HistoricalPreviewSummary: HistoricalPreviewSummary;
  HistoricalProjectMapping: HistoricalProjectMapping;
  HistoricalProjectTarget: HistoricalProjectTarget;
  HistoricalStream: HistoricalStream;
  HistoricalUnsentFileReceipt: HistoricalUnsentFileReceipt;
  HistorySnapshot: HistorySnapshot;
  ImportActorModelQualification: ImportActorModelQualification;
  ImportAuditCheckpoint: ImportAuditCheckpoint;
  ImportAuditTrustTransition: ImportAuditTrustTransition;
  ImportExecutionPreflightV2: ImportExecutionPreflightV2;
  ImportFile: ImportFile;
  ImportModelQualification: ImportModelQualification;
  InstallOfflineGrant: InstallOfflineGrant;
  IssueOfflineGrant: IssueOfflineGrant;
  KnowledgeDocument: KnowledgeDocument;
  KnowledgeFolder: KnowledgeFolder;
  KnowledgeFolders: KnowledgeFolders;
  KnowledgeGraphEdge: KnowledgeGraphEdge;
  KnowledgeGraphHit: KnowledgeGraphHit;
  KnowledgeGraphNode: KnowledgeGraphNode;
  KnowledgeGraphResults: KnowledgeGraphResults;
  KnowledgeImportRequest: KnowledgeImportRequest;
  KnowledgeImportResult: KnowledgeImportResult;
  KnowledgeKind: KnowledgeKind;
  KnowledgeLocation: KnowledgeLocation;
  KnowledgeNodeKind: KnowledgeNodeKind;
  KnowledgeOrigin: KnowledgeOrigin;
  KnowledgeProvenance: KnowledgeProvenance;
  KnowledgeRevision: KnowledgeRevision;
  KnowledgeSource: KnowledgeSource;
  KnowledgeSourceVerification: KnowledgeSourceVerification;
  KnowledgeTopicGraph: KnowledgeTopicGraph;
  LibraryConfigContract: LibraryConfigContract;
  LibraryDeleted: LibraryDeleted;
  LibrarySource: LibrarySource;
  MaintenanceStatus: MaintenanceStatus;
  ManageChat: ManageChat;
  ManageSandboxSession: ManageSandboxSession;
  MaterializeCodexModelProfile: MaterializeCodexModelProfile;
  McpAsyncTask: McpAsyncTask;
  McpAsyncTaskRequest: McpAsyncTaskRequest;
  McpAttempt: McpAttempt;
  McpAttemptState: McpAttemptState;
  McpOAuthConfiguration: McpOAuthConfiguration;
  McpOAuthDiscovery: McpOAuthDiscovery;
  McpOAuthFlow: McpOAuthFlow;
  McpOAuthMetadata: McpOAuthMetadata;
  McpOAuthProfile: McpOAuthProfile;
  McpOAuthRequest: McpOAuthRequest;
  McpResult: McpResult;
  McpServer: McpServer;
  McpServerConfiguration: McpServerConfiguration;
  McpTaskDetail: McpTaskDetail;
  McpTaskInteraction: McpTaskInteraction;
  McpToolBinding: McpToolBinding;
  McpToolSelection: McpToolSelection;
  MemoryMetadata: MemoryMetadata;
  MemoryScope: MemoryScope;
  Message: Message;
  ModelAttempt: ModelAttempt;
  ModelAttemptState: ModelAttemptState;
  ModelCapabilities: ModelCapabilities;
  ModelContextView: ModelContextView;
  ModelRegistryRef: ModelRegistryRef;
  ModelRegistryView: ModelRegistryView;
  NativeAuthTransportCapture: NativeAuthTransportCapture;
  NativeAuthTransportReplay: NativeAuthTransportReplay;
  NativeAuthTransportRequest: NativeAuthTransportRequest;
  NativeAuthTransportResolution: NativeAuthTransportResolution;
  NativeAuthTransportView: NativeAuthTransportView;
  NativeChooseFolderRequest: NativeChooseFolderRequest;
  NativeDeletedSchedule: NativeDeletedSchedule;
  NativeFolderOutcome: NativeFolderOutcome;
  NativeFolderSelection: NativeFolderSelection;
  NativeMcpFactory: NativeMcpFactory;
  NativeSaveDatabaseBackup: NativeSaveDatabaseBackup;
  NativeSaveOutcome: NativeSaveOutcome;
  NativeSaveRequest: NativeSaveRequest;
  NativeScheduleHistory: NativeScheduleHistory;
  NativeScheduleHistoryEntry: NativeScheduleHistoryEntry;
  NativeSecretsReceipt: NativeSecretsReceipt;
  NativeSecretsRequestLookup: NativeSecretsRequestLookup;
  NativeSecretsResolution: NativeSecretsResolution;
  NativeSecretsSetup: NativeSecretsSetup;
  NativeSecretsSetupState: NativeSecretsSetupState;
  NativeSecretsStatus: NativeSecretsStatus;
  NativeSkillHistory: NativeSkillHistory;
  NativeSkillRevision: NativeSkillRevision;
  OfflineArtifact: OfflineArtifact;
  OfflineDevice: OfflineDevice;
  OfflineDeviceProof: OfflineDeviceProof;
  OfflineDeviceProofRequest: OfflineDeviceProofRequest;
  OfflineGrant: OfflineGrant;
  OfflineGrantStatus: OfflineGrantStatus;
  OfflineGrantStatusRequest: OfflineGrantStatusRequest;
  OfflineGrantView: OfflineGrantView;
  OfflineJournalEntry: OfflineJournalEntry;
  OfflineLocalGrantView: OfflineLocalGrantView;
  OfflineMcpConnectionInput: OfflineMcpConnectionInput;
  OfflineMcpConnectionView: OfflineMcpConnectionView;
  OfflineMcpInteractionPage: OfflineMcpInteractionPage;
  OfflineMcpResultBytes: OfflineMcpResultBytes;
  OfflinePendingTool: OfflinePendingTool;
  OfflineReconciliationReceipt: OfflineReconciliationReceipt;
  OfflineRun: OfflineRun;
  OfflineRunExport: OfflineRunExport;
  OfflineRunPage: OfflineRunPage;
  OfflineRunSummary: OfflineRunSummary;
  OfflineScopeRevision: OfflineScopeRevision;
  OfflineSource: OfflineSource;
  OfflineToolConfigurationView: OfflineToolConfigurationView;
  OfflineWorkspaceUpload: OfflineWorkspaceUpload;
  Order: Order;
  OrderArchive: OrderArchive;
  OrderExport: OrderExport;
  OrderHistory: OrderHistory;
  OwnerConnector: OwnerConnector;
  OwnerConnectorProbe: OwnerConnectorProbe;
  OwnerDatabaseBackupRequest: OwnerDatabaseBackupRequest;
  OwnerExecutionTargetV2: OwnerExecutionTargetV2;
  OwnerManagedProcessReconciliation: OwnerManagedProcessReconciliation;
  OwnerManagedResultDelivery: OwnerManagedResultDelivery;
  OwnerManagedResultDeliveryState: OwnerManagedResultDeliveryState;
  OwnerManagedResultDeliveryView: OwnerManagedResultDeliveryView;
  OwnerManagedResultEntry: OwnerManagedResultEntry;
  OwnerManagedResultList: OwnerManagedResultList;
  OwnerManagedResultManifest: OwnerManagedResultManifest;
  OwnerManagedResultVersion: OwnerManagedResultVersion;
  OwnerManagedResultVersionState: OwnerManagedResultVersionState;
  OwnerMcpCapabilities: OwnerMcpCapabilities;
  OwnerMcpCatalog: OwnerMcpCatalog;
  OwnerMcpCredential: OwnerMcpCredential;
  OwnerMcpCwdPolicy: OwnerMcpCwdPolicy;
  OwnerMcpDiscovery: OwnerMcpDiscovery;
  OwnerMcpExpectedRevision: OwnerMcpExpectedRevision;
  OwnerMcpRevision: OwnerMcpRevision;
  OwnerTerminalInput: OwnerTerminalInput;
  OwnerTerminalLaunch: OwnerTerminalLaunch;
  OwnerTerminalOutput: OwnerTerminalOutput;
  OwnerTerminalPreview: OwnerTerminalPreview;
  OwnerTerminalResize: OwnerTerminalResize;
  OwnerTerminalSession: OwnerTerminalSession;
  OwnerTerminalStatus: OwnerTerminalStatus;
  PairDeviceExecutor: PairDeviceExecutor;
  PersonalMoneyCurrency: PersonalMoneyCurrency;
  PersonalMoneyDefaults: PersonalMoneyDefaults;
  PersonalMoneyPoolStatus: PersonalMoneyPoolStatus;
  PersonalMoneyStatus: PersonalMoneyStatus;
  Personality: Personality;
  PersonalityDefinition: PersonalityDefinition;
  PreferencesRecord: PreferencesRecord;
  Preflight: Preflight;
  PrepareAuditTrustTransition: PrepareAuditTrustTransition;
  PrepareExecutionInput: PrepareExecutionInput;
  PrepareImportExecutionPlanV2: PrepareImportExecutionPlanV2;
  PrepareOwnerExecutionPlanV2: PrepareOwnerExecutionPlanV2;
  PrepareRestoreExecutionPlanV2: PrepareRestoreExecutionPlanV2;
  PrepareTeamLostHostRetirement: PrepareTeamLostHostRetirement;
  PrepareWorkflowInputAnswer: PrepareWorkflowInputAnswer;
  PrepareWorkflowOperation: PrepareWorkflowOperation;
  PreparedExecutionInput: PreparedExecutionInput;
  PreparedExecutionPlan: PreparedExecutionPlan;
  PreparedExecutionPlanV2: PreparedExecutionPlanV2;
  PreserveAuditHistory: PreserveAuditHistory;
  PreservedAuditHistory: PreservedAuditHistory;
  PreviewHistoricalImport: PreviewHistoricalImport;
  PreviewModelContext: PreviewModelContext;
  ProcessAttempt: ProcessAttempt;
  ProcessAttemptState: ProcessAttemptState;
  ProcessCapture: ProcessCapture;
  ProcessInvocation: ProcessInvocation;
  ProcessResult: ProcessResult;
  ProcessTermination: ProcessTermination;
  ProductSchedule: ProductSchedule;
  Project: Project;
  ProjectSettings: ProjectSettings;
  ProjectSource: ProjectSource;
  ProviderKind: ProviderKind;
  ProviderProfile: ProviderProfile;
  ProviderSettings: ProviderSettings;
  ProviderSettingsContract: ProviderSettingsContract;
  PublishWorkflow: PublishWorkflow;
  ReadWorkspaceTransferChunk: ReadWorkspaceTransferChunk;
  ReceiveTeamLostHostRetirement: ReceiveTeamLostHostRetirement;
  ReconcileFile: ReconcileFile;
  ReconcileFileDecision: ReconcileFileDecision;
  ReconcileOwnerManagedProcess: ReconcileOwnerManagedProcess;
  RegisterDevicePush: RegisterDevicePush;
  RegisteredFolder: RegisteredFolder;
  RemoveFile: RemoveFile;
  RenameFile: RenameFile;
  ReserveUpload: ReserveUpload;
  ResolveApproval: ResolveApproval;
  ResolveTeamEffect: ResolveTeamEffect;
  RestoreContentBindingV2: RestoreContentBindingV2;
  RestoreExecutionPreflightV2: RestoreExecutionPreflightV2;
  RestoreFile: RestoreFile;
  ResumeTeamRun: ResumeTeamRun;
  ResumeWorkflowInput: ResumeWorkflowInput;
  RetainedVersion: RetainedVersion;
  RetireAutomationEvent: RetireAutomationEvent;
  RetireExecutionInput: RetireExecutionInput;
  RetireTeamContainer: RetireTeamContainer;
  RetiredExecutionRequest: RetiredExecutionRequest;
  RetrieveExternalMemory: RetrieveExternalMemory;
  RevokeActorModelQualification: RevokeActorModelQualification;
  RevokeFolder: RevokeFolder;
  RevokeWorkflow: RevokeWorkflow;
  RunDetail: RunDetail;
  RunEvent: RunEvent;
  RunPreflight: RunPreflight;
  RunRecord: RunRecord;
  RunRequest: RunRequest;
  RunSpec: RunSpec;
  RunState: RunState;
  RunWorkspaceBinding: RunWorkspaceBinding;
  SandboxLifecycleOperation: SandboxLifecycleOperation;
  SandboxLifecycleState: SandboxLifecycleState;
  SandboxPreview: SandboxPreview;
  SandboxSession: SandboxSession;
  SandboxSessionOperation: SandboxSessionOperation;
  SaveDocumentDraft: SaveDocumentDraft;
  SaveDraft: SaveDraft;
  SaveOwnerConnector: SaveOwnerConnector;
  SaveTeamDocument: SaveTeamDocument;
  SaveWorkflowDraft: SaveWorkflowDraft;
  SaveWorkflowInputDraft: SaveWorkflowInputDraft;
  ScheduleOccurrence: ScheduleOccurrence;
  ScheduleRecurrence: ScheduleRecurrence;
  ScheduledRunTemplate: ScheduledRunTemplate;
  SearchHit: SearchHit;
  SearchKind: SearchKind;
  SearchKnowledgeGraph: SearchKnowledgeGraph;
  SearchPage: SearchPage;
  SetAndroidAuthManagementPolicy: SetAndroidAuthManagementPolicy;
  SignedAuditArchive: SignedAuditArchive;
  SignedAuditCheckpoint: SignedAuditCheckpoint;
  SignedAuditTrustTransition: SignedAuditTrustTransition;
  SignedOfflineGrant: SignedOfflineGrant;
  SignedOfflineGrantStatus: SignedOfflineGrantStatus;
  Skill: Skill;
  SkillLearningHistory: SkillLearningHistory;
  SkillLearningOutcome: SkillLearningOutcome;
  StartDevicePasskeyAuthentication: StartDevicePasskeyAuthentication;
  StartOfflineRun: StartOfflineRun;
  StartWorkspaceTask: StartWorkspaceTask;
  StructuredOutput: StructuredOutput;
  SubmitAutomationEvent: SubmitAutomationEvent;
  SubmitInput: SubmitInput;
  SubmitWorkflowInput: SubmitWorkflowInput;
  Team: Team;
  TeamAgent: TeamAgent;
  TeamContainerRecoveryPlan: TeamContainerRecoveryPlan;
  TeamContainerRecoveryReceipt: TeamContainerRecoveryReceipt;
  TeamDocument: TeamDocument;
  TeamDocumentKind: TeamDocumentKind;
  TeamEffectDetail: TeamEffectDetail;
  TeamEffectView: TeamEffectView;
  TeamExecutionActor: TeamExecutionActor;
  TeamExecutionBlocker: TeamExecutionBlocker;
  TeamExecutionDocumentBinding: TeamExecutionDocumentBinding;
  TeamExecutionMcpBinding: TeamExecutionMcpBinding;
  TeamExecutionMemoryBinding: TeamExecutionMemoryBinding;
  TeamExecutionModelBinding: TeamExecutionModelBinding;
  TeamExecutionPreflight: TeamExecutionPreflight;
  TeamGithubConfiguration: TeamGithubConfiguration;
  TeamHostTrustProfileView: TeamHostTrustProfileView;
  TeamLostHostPlan: TeamLostHostPlan;
  TeamLostHostRetirementReceipt: TeamLostHostRetirementReceipt;
  TeamMcpServer: TeamMcpServer;
  TeamMcpTool: TeamMcpTool;
  TeamMember: TeamMember;
  TeamOrder: TeamOrder;
  TeamOriginalHostBinding: TeamOriginalHostBinding;
  TeamProject: TeamProject;
  TeamProjectGrant: TeamProjectGrant;
  TeamProjectGrantView: TeamProjectGrantView;
  TeamProvider: TeamProvider;
  TeamRecoveryView: TeamRecoveryView;
  TeamRole: TeamRole;
  TeamRun: TeamRun;
  TeamRunPlan: TeamRunPlan;
  TeamRunStatus: TeamRunStatus;
  TeamRunStep: TeamRunStep;
  TeamToolApproval: TeamToolApproval;
  TeamWorkflowMode: TeamWorkflowMode;
  TeamWorkflowPlan: TeamWorkflowPlan;
  TeamWorkflowStep: TeamWorkflowStep;
  TextFile: TextFile;
  TokenBudgetStatus: TokenBudgetStatus;
  ToolCall: ToolCall;
  UiLanguage: UiLanguage;
  UpdateDeviceExecutor: UpdateDeviceExecutor;
  UpdateProviderSettings: UpdateProviderSettings;
  UpdateTeam: UpdateTeam;
  UpdateTeamMember: UpdateTeamMember;
  UpdateTeamOrder: UpdateTeamOrder;
  UpdateTeamProject: UpdateTeamProject;
  UploadPage: UploadPage;
  UploadState: UploadState;
  Usage: Usage;
  VerifyAuditV1: VerifyAuditV1;
  WorkflowBinding: WorkflowBinding;
  WorkflowControlOperation: WorkflowControlOperation;
  WorkflowControlStep: WorkflowControlStep;
  WorkflowDataContext: WorkflowDataContext;
  WorkflowDataSource: WorkflowDataSource;
  WorkflowDataType: WorkflowDataType;
  WorkflowDataflow: WorkflowDataflow;
  WorkflowDefinition: WorkflowDefinition;
  WorkflowDocumentFolder: WorkflowDocumentFolder;
  WorkflowDocumentFormat: WorkflowDocumentFormat;
  WorkflowDocumentOperation: WorkflowDocumentOperation;
  WorkflowDocumentStep: WorkflowDocumentStep;
  WorkflowDraft: WorkflowDraft;
  WorkflowDraftContent: WorkflowDraftContent;
  WorkflowDraftOrigin: WorkflowDraftOrigin;
  WorkflowDraftPage: WorkflowDraftPage;
  WorkflowDraftSave: WorkflowDraftSave;
  WorkflowDraftSnapshot: WorkflowDraftSnapshot;
  WorkflowDraftSnapshotPage: WorkflowDraftSnapshotPage;
  WorkflowDraftSnapshotRef: WorkflowDraftSnapshotRef;
  WorkflowDraftSnapshotSummary: WorkflowDraftSnapshotSummary;
  WorkflowEffectResolutionView: WorkflowEffectResolutionView;
  WorkflowEmbeddedIteration: WorkflowEmbeddedIteration;
  WorkflowEmbeddedSubflow: WorkflowEmbeddedSubflow;
  WorkflowFailure: WorkflowFailure;
  WorkflowGate: WorkflowGate;
  WorkflowInputAnswerExport: WorkflowInputAnswerExport;
  WorkflowInputAnswerIntent: WorkflowInputAnswerIntent;
  WorkflowInputAnswerIntentState: WorkflowInputAnswerIntentState;
  WorkflowInputAnswerIntentSummary: WorkflowInputAnswerIntentSummary;
  WorkflowInputAnswerRejection: WorkflowInputAnswerRejection;
  WorkflowInputDraft: WorkflowInputDraft;
  WorkflowInputDraftLookup: WorkflowInputDraftLookup;
  WorkflowInputDraftSave: WorkflowInputDraftSave;
  WorkflowInputDraftSnapshot: WorkflowInputDraftSnapshot;
  WorkflowInputDraftSnapshotRef: WorkflowInputDraftSnapshotRef;
  WorkflowInputDraftSnapshotSummary: WorkflowInputDraftSnapshotSummary;
  WorkflowInputReceipt: WorkflowInputReceipt;
  WorkflowInputResumePreview: WorkflowInputResumePreview;
  WorkflowInputSnapshot: WorkflowInputSnapshot;
  WorkflowInputSource: WorkflowInputSource;
  WorkflowInputState: WorkflowInputState;
  WorkflowInputView: WorkflowInputView;
  WorkflowIterationArgument: WorkflowIterationArgument;
  WorkflowLibraryEntry: WorkflowLibraryEntry;
  WorkflowMode: WorkflowMode;
  WorkflowModelView: WorkflowModelView;
  WorkflowNodeKind: WorkflowNodeKind;
  WorkflowNodeStatus: WorkflowNodeStatus;
  WorkflowNodeView: WorkflowNodeView;
  WorkflowOperation: WorkflowOperation;
  WorkflowOperationPage: WorkflowOperationPage;
  WorkflowOperationReceipt: WorkflowOperationReceipt;
  WorkflowOperationRejection: WorkflowOperationRejection;
  WorkflowOperationRequest: WorkflowOperationRequest;
  WorkflowOperationState: WorkflowOperationState;
  WorkflowOutputRef: WorkflowOutputRef;
  WorkflowPage: WorkflowPage;
  WorkflowPredicate: WorkflowPredicate;
  WorkflowPreflight: WorkflowPreflight;
  WorkflowPublicState: WorkflowPublicState;
  WorkflowPublication: WorkflowPublication;
  WorkflowReviewCheck: WorkflowReviewCheck;
  WorkflowReviewEvidence: WorkflowReviewEvidence;
  WorkflowReviewEvidenceKind: WorkflowReviewEvidenceKind;
  WorkflowReviewField: WorkflowReviewField;
  WorkflowReviewForm: WorkflowReviewForm;
  WorkflowReviewFormat: WorkflowReviewFormat;
  WorkflowReviewRequest: WorkflowReviewRequest;
  WorkflowRights: WorkflowRights;
  WorkflowRole: WorkflowRole;
  WorkflowRunApproval: WorkflowRunApproval;
  WorkflowRunKind: WorkflowRunKind;
  WorkflowRunRequest: WorkflowRunRequest;
  WorkflowRunView: WorkflowRunView;
  WorkflowSnapshot: WorkflowSnapshot;
  WorkflowStep: WorkflowStep;
  WorkflowStepData: WorkflowStepData;
  WorkflowStepExecution: WorkflowStepExecution;
  WorkflowTableColumn: WorkflowTableColumn;
  WorkflowTableColumnKind: WorkflowTableColumnKind;
  WorkflowUnresolvedEffect: WorkflowUnresolvedEffect;
  WorkflowValueBinding: WorkflowValueBinding;
  WorkflowVersion: WorkflowVersion;
  WorkflowVersionRef: WorkflowVersionRef;
  Workspace: Workspace;
  WorkspaceEntry: WorkspaceEntry;
  WorkspaceManagementContract: WorkspaceManagementContract;
  WorkspaceSnapshot: WorkspaceSnapshot;
  WorkspaceTask: WorkspaceTask;
  WorkspaceTaskDefinition: WorkspaceTaskDefinition;
  WorkspaceTaskExecution: WorkspaceTaskExecution;
  WorkspaceTransferChunk: WorkspaceTransferChunk;
  WorkspaceTransferFile: WorkspaceTransferFile;
  WorkspaceTransferManifest: WorkspaceTransferManifest;
  WriteActorChatConfiguration: WriteActorChatConfiguration;
  WriteActorMemory: WriteActorMemory;
  WriteActorSkill: WriteActorSkill;
  WriteAgentTemplate: WriteAgentTemplate;
  WriteAutomationTrigger: WriteAutomationTrigger;
  WriteChatConfiguration: WriteChatConfiguration;
  WriteCodexAccount: WriteCodexAccount;
  WriteExternalMemoryProvider: WriteExternalMemoryProvider;
  WriteKnowledge: WriteKnowledge;
  WriteKnowledgeFolders: WriteKnowledgeFolders;
  WritePersonality: WritePersonality;
  WritePreferences: WritePreferences;
  WriteProjectSettings: WriteProjectSettings;
  WriteSchedule: WriteSchedule;
  WriteSkill: WriteSkill;
  WriteTextFile: WriteTextFile;
  WriteWorkspaceTask: WriteWorkspaceTask;
}

export const defaultRunBudgets: Required<Budgets> = {"max_output_tokens":4096,"max_seconds":1800,"max_steps":64,"max_total_tokens":262144};
