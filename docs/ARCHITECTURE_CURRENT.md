---
title: Current Architecture
type: overview
doc_type: current-state
status: seed-current
owner: product-docs
last_updated: 2026-07-02
last_verified: 2026-07-02
canonical_for:
  - current implementation architecture
  - app boundary overview
source_files:
  - app/src/App.tsx
  - app/src/components/Layout.tsx
  - app/src/engine/core/queryEngine.ts
  - app/src/engine/tools/registry.ts
  - app/src/stores/configStore.ts
  - app/src/stores/coworkStore.ts
  - app/src/stores/projectStore.ts
  - app/src/stores/workTasksStore.ts
  - app/src-tauri/src/lib.rs
  - app/src-tauri/src/db.rs
  - app/src-tauri/tauri.conf.json
supersedes:
  - docs/ARCHITECTURE.md
related_docs:
  - docs/product/current-routes.md
  - docs/product/core-domain-decisions.md
  - docs/product/runtime-compatibility.md
---

# Current Architecture

Open Cowork is a local-first Tauri desktop app. The implemented product is a React workspace shell backed by Zustand state, Tauri IPC commands, Rust services, SQLite, bundled desktop resources, and optional local or remote model/tool providers.

## System Shape

```text
React 19 UI and route shell
  -> Zustand stores and agentic QueryEngine
  -> Tauri IPC commands
  -> Rust backend services
  -> SQLite, app data, filesystem, desktop APIs, MCP processes, Python crew runtime
  -> Ollama, OpenAI-compatible endpoints, OpenRouter, web/connector endpoints
```

## Frontend Boundary

`app/src/App.tsx` owns the route tree and startup orchestration. `Layout` owns the persistent desktop shell: top navigation, left sidebar, command palette, theme toggle, shortcut overlay, and lazy route rendering.

The current route set is `/`, `/tasks`, `/crew`, `/projects`, `/features`, and `/settings`; unknown paths redirect to `/`.

## Agent Runtime Boundary

`QueryEngine` owns the agentic loop: build prompt, stream from the selected provider, request tool execution, enforce permissions, emit progress/approval events, compact context, and stop on completion, errors, or user input.

Built-in tools are registered in `app/src/engine/tools/registry.ts`. File, shell, desktop, web, MCP, office, memory, task, skill, and user-interaction tools delegate privileged operations through Tauri commands.

## Backend Boundary

`app/src-tauri/src/lib.rs` registers the Tauri command surface and wires shared state:

- SQLite database opened from the Tauri app data directory.
- Scheduler worker, terminal session registry, file watchers, crew execution registry, chat stream registry.
- Rust services for file safety, artifacts, PDF/Office preview, MCP runtime, desktop control, web fetch/search, process management, memory, skills, sessions, policies, and gateway diagnostics.
- Python crew runtime and PDFium resources are bundled with the desktop app.

## Persistence Boundary

| Area | Current Store |
| --- | --- |
| Chat threads/messages, projects, schedules, memory, skills, sessions, policy, engine runs, artifacts, personalities, insights | SQLite via Tauri commands |
| Config, provider profiles, preferences, MCP server list | Zustand persist / localStorage |
| User-facing work tasks in `/tasks` | Zustand persist / localStorage |
| Crew editor state and default agents | Zustand persist / localStorage |
| Scheduled task definitions and run history | SQLite |

The split is intentional current state, not a guarantee that all domain data is already centralized in SQLite.

## Transitional Notes

- `taskStore` still exists for legacy task/progress panels and backend DB task commands; `/tasks` uses `workTasksStore`.
- Crew editor state is local-store based, while crew definitions, approvals, run events, and scheduled crew snapshots exist in backend tables.
- `safeInvoke` allows selected UI behavior to degrade outside Tauri, but full product functionality requires the Tauri runtime.
