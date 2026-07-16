---
title: Core Domain Decisions
type: overview
doc_type: decision-record
status: seed-current
owner: product-docs
last_updated: 2026-07-02
last_verified: 2026-07-02
canonical_for:
  - current domain model boundaries
  - current product object definitions
source_files:
  - app/src/stores/chatStore.ts
  - app/src/stores/projectStore.ts
  - app/src/stores/workTasksStore.ts
  - app/src/stores/crewStore.ts
  - app/src/stores/coworkStore.ts
  - app/src/stores/configStore.ts
  - app/src/engine/core/queryEngine.ts
  - app/src-tauri/src/db.rs
related_docs:
  - docs/ARCHITECTURE_CURRENT.md
---

# Core Domain Decisions

## Product Positioning

Open Cowork is a Windows-first, local-first desktop workspace for AI-assisted work. The product centers on chat, tasks, projects, tools, model/provider configuration, MCP, local file context, scheduled work, and multi-agent crew runs.

## Current Domain Objects

| Object | Current Meaning | Current Source |
| --- | --- | --- |
| Chat thread | Conversation history with provider settings, permission config, attachments, live tool state, and optional crew/model runner metadata | `chatStore`, SQLite |
| Project | Lightweight grouping of chats plus reusable instructions and enabled file/folder/link resources | `projectStore`, SQLite |
| Work task | User-facing task request with runner, prompt, expected output, working folder, linked chat, schedule metadata, output, and status | `workTasksStore`, SQLite `work_tasks`, LocalStorage fallback |
| Scheduled task | Backend schedule entry for prompt or crew execution, including snapshots and run history | `coworkStore`, SQLite |
| Crew | Multi-agent configuration with agents, providers, governance, runtime config, output mode, process type, and access controls | `crewStore`, localStorage plus backend run/definition tables |
| Memory | Persisted entries, snapshots, hints, user profile data, and optional provider metadata | SQLite |
| Toolset policy | Named set of enabled tools plus policy flags and deny rules | `coworkStore`, SQLite policy tables |
| LLM profile | Provider endpoint/model/API-key profile for Ollama, OpenAI-compatible, or OpenRouter | `configStore`, localStorage |

## Decisions

### CD-001: Local-first is the default product model

The desktop app is the primary runtime. Hosted SaaS assumptions are out of scope for current docs unless introduced by future code.

### CD-002: Projects organize context, not isolated workspaces

Projects attach existing chat threads and reusable resources. A chat can be moved between projects, detached, or left unassigned. Project instructions and enabled resources are injected into chat context when that project owns the active thread.

### CD-003: Work tasks are separate from chat threads

Tasks can open or create a related chat thread, but the task is its own object. A task may run through a selected crew or directly through a model. Absolute working folders are required when a task uses filesystem context.

### CD-004: Scheduling persists executable snapshots

Prompt schedules persist model config. Crew schedules persist a crew snapshot, including enabled agents, runtime tasks, provider configs, and snapshot source metadata. Scheduled execution should not depend on later editor-only assumptions unless the schedule is updated.

### CD-005: Crew agents synchronize with personality profiles

Personality profiles are the reusable source for agent identity fields when linked. Crew-local agent access, enabled state, iteration limits, tools, and MCP access remain crew-specific.

### CD-006: Tool execution is policy-governed

The engine evaluates permission mode, tool risk, allow/deny rules, and toolset policy before executing privileged tools. Read-only tools can run automatically in default mode; medium/high risk tools can require approval.

### CD-007: Persistence is intentionally mixed today

Current behavior uses SQLite for backend-owned records, projects, chats, scheduled work, and WorkTasks. LocalStorage remains for UI/config/editor state and as browser fallback or one-time migration backup where documented. Do not document future centralization as implemented until the code changes.

### CD-008: File, desktop, Office, PDF, shell, and MCP capabilities cross the Tauri boundary

Privileged operations should remain behind Tauri commands or explicit provider/tool APIs. UI components should not bypass the backend for local filesystem or desktop actions.
