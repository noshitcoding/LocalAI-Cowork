# Claude-Code Feature Adoption (Open_Cowork)

Stand: 2026-04-16
Quelle: `andere_ai_agenten_beispiele/claude-code-main/claude-code-main/README.md` + Einstieg in `src/main.tsx`, `src/tools.ts`.

## 1) Extrahierte Claude-Code Feature-Familien

1. Core-Agent Runtime
- Main loop / Query engine
- Tool execution loop
- Session and history lifecycle

2. Tooling Layer
- Bash/PowerShell, read/edit/write, glob/grep
- Web fetch/search
- Notebook edit
- Todo/task tools
- Ask-user question
- Agent/sub-agent tools
- MCP tool/resource adapters

3. Permission + Safety Harness
- Permission modes (`default`, `acceptEdits`, `bypassPermissions`, `dontAsk`, `plan`)
- Tool allow/deny filtering
- Shell safety checks

4. Plan + Task UX
- Plan mode
- Todo/task orchestration
- Explicit approval boundaries

5. Memory + Context Handling
- Session memory
- Compaction/summarization tiers
- Re-read stubs / context optimizations

6. Integration
- MCP registry + tools
- Plugins/skills
- Feature flags/gates

7. UI/Interaction
- Command-like interactions
- Modal workflows and operational status feedback

## 2) Jetzt in Open_Cowork integriert (dieser Slice)

1. Claude-Kompatibilitaetsprofil im Produkt-Store
- Plan mode toggle
- Permission mode selection
- Tool preset selection (`default`, `safe`, `extended`)
- Tool family enable/disable matrix

2. Slash-Command Layer im Work-Chat
- `/help`
- `/tools`
- `/mode plan|execute`
- `/permissions <mode>`
- `/plan <prompt>`
- `/fetch <url>`

3. Ollama-kompatible Prompt-Einbettung
- Runtime-instruction addendum (permission mode, plan mode, active tools, global instruction)
- Plan-mode prompt wrapping for plan-only outputs

4. Backend Web Fetch Tool
- Tauri command: `web_fetch_url`
- URL fetch, text extraction, truncation, title extraction
- Audit event: `web/fetch_url`

5. Claude-Kompatibilitaets-UI
- Neue Einstellungen in Cowork Features Panel
- Persistente Konfiguration fuer mode/preset/tools

6. Policy-/Flag-Infrastruktur (weiterer Slice)
- Zentrale Policy-Flags im Store (`strictPolicyEnforcement`, `allowToolDispatcher`, `allowMcpToolCalls`, `allowWebFetch`, `allowFileReadExtraction`, `autoCompactLongContext`)
- Persistente Deny-Rules mit Wildcard-Matching (z. B. `mcp:*`, `web_fetch:*example.com*`)
- UI-Verwaltung fuer Flags und Rules in Cowork Features

7. Tool Dispatcher mit Policy-Enforcement
- Neue Chat-Commands: `/tool <name> ...` und `/todo add|list`
- Dispatcher-Tools: `read_file` (via `fs_extract_text`), `web_fetch` (via `web_fetch_url`), `mcp_call` (via `mcp_call_tool`)
- Vor jedem Dispatcher-Call: Flag-Check + deny-rule-Check + Tool-Profil-Check

8. Kontext-Kompaktierung fuer lange Sessions
- Optionales Auto-Compaction von Verlaufskontext fuer LLM-Aufrufe
- Synthetischer System-Block mit komprimiertem Verlauf statt ungebremstem Nachrichtenwachstum
- Aktivierbar/deaktivierbar ueber Policy-Flag `autoCompactLongContext`

9. Backend-seitige Policy-Enforcement-Basis
- Persistente Policy-Flags und Deny-Rules in SQLite (`policy_flags`, `policy_deny_rules`)
- Neue Backend-Commands `policy_get` und `policy_set` fuer zentrale Policy-Verwaltung
- Harte Backend-Checks in `mcp_call_tool`, `web_fetch_url` und `fs_extract_text`
- Frontend-Sync der Policy (Store <-> Backend), damit die Runtime-Regeln serverseitig gelten

## 3) Bereits vorher vorhanden und Claude-nah

- Ollama chat/plan health integration
- MCP probe + call
- Approval-oriented task flow
- File-safety guardrails (allowlist, backup/restore, watcher, audit)
- Sub-agent batch execution (parallel)

## 4) Noch offen fuer "vollstaendige" Claude-Paritaet

- Deep query-engine level context compaction tiers (mehrstufig, inkl. serverseitiger cache-edits)
- Dedicated plugin runtime + signatures
- Rich coordinator/worker orchestration protocol
- Full command surface and all specialized tools
- Advanced permission policy engine mit mehrdimensionalen Regelquellen (workspace/user/policy + Prioritaeten)
- Full feature-flag runtime und Telemetry-Gates auf Backend- und Tool-Execution-Ebene

Diese offenen Punkte sind grossere Architektur-Slices und nicht in einem einzelnen kurzfristigen Patch sinnvoll oder risikoarm integrierbar.
