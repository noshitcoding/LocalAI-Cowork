# Open_Cowork

Open_Cowork ist eine Windows-Desktop-Anwendung (Tauri + React + Rust) fuer agentisches Arbeiten mit lokaler Modellanbindung.

## Features

- **Chat-Interface** mit Thread-Management und persistenten Verlaeufen (SQLite)
- **Ollama-Integration** fuer lokale LLM-Anbindung (Chat, Planung, Health-Check)
- **Plan/Freigabe-Flow** fuer risikobehaftete Prompts mit Approval-UI
- **Task-Management** mit Status-Lifecycle (created → planned → waiting_approval → running → completed/failed/cancelled)
- **MCP-Server-Integration** mit Probing (tools/list) und Tool-Ausfuehrung (tools/call) via stdio JSON-RPC
- **Plugin-/Skill-System** mit anpassbaren Slash-Commands und Prompt-Templates
- **Persistente Datenhaltung** in SQLite (Threads, Messages, Tasks, Steps, Audit-Events)
- **4-View-Layout** mit Sidebar-Navigation (Chat, Tasks, MCP, Einstellungen)
- **CI-Pipeline** mit TypeScript-Check, Vitest, Cargo-Tests, Clippy, Security-Scans

## Projektstruktur

- `app`: Tauri-Anwendung (Frontend + Rust Backend)
- `docs`: Architektur-, Betriebs- und Konfigurationsdokumente
- `WINDOWS_DESKTOP_APP_ANFORDERUNGEN.md`: Anforderungsdokument
- `TRACEABILITY_MATRIX.md`: Traceability-Matrix

## Voraussetzungen

- Windows 11 oder Windows 10
- Node.js 22+
- npm 10+
- Rust (via rustup)
- WebView2 Runtime
- Ollama erreichbar, Standard-Endpunkt: `http://192.168.178.82:11434`

## Schnellstart

1. Desktop-EXE bauen:

```powershell
cd app
npm install
npm run tauri build
```

2. Build-Artefakte finden:

`app/src-tauri/target/release/bundle/`

## Tests

```powershell
# Frontend (14 Tests: App, chatStore, taskStore)
cd app
npx vitest run

# Backend (8 Tests: db, ollama)
cd app/src-tauri
cargo test
```

## Standard MCP Server (DuckDuckGo Web Search)

Open_Cowork ist standardmaessig mit einem lokalen MCP-Server fuer DuckDuckGo-Websuche vorkonfiguriert.

- Name: `duckduckgo-websearch`
- Command: `node`
- Args: `scripts/mcp/duckduckgo-websearch-server.mjs`

Anpassbare Parameter (Env im MCP-Dialog):

- `DDG_MAX_RESULTS` (Standard `5`)
- `DDG_REGION` (Standard `wt-wt`, z. B. `de-de`, `us-en`)
- `DDG_SAFESEARCH` (`off`, `moderate`, `strict`; Standard `moderate`)
- `DDG_TIMEOUT_MS` (Standard `10000`)
- `DDG_HTML_ENDPOINT` (optional, Standard `https://html.duckduckgo.com/html/`)

Toolname im MCP-Server: `search_web`

## Dokumentation

- `docs/ARCHITECTURE.md`
- `docs/OLLAMA_CONFIGURATION.md`
- `docs/DEVELOPMENT_AND_OPERATIONS.md`
- `docs/DESKTOP_SMOKE_TEST.md`
- `docs/DESKTOP_CONTROL_AND_COMPUTER_USE.md`

## Plugin-Skill Beispiele (in der App enthalten)

In `Einstellungen -> Cowork Features -> Plugins & Skills` koennen mit einem Klick drei Beispiele installiert werden:

- `Marketing Briefing Toolkit` mit `/briefing`
- `Sales Follow-up Assistant` mit `/discovery-plan`
- `Finance KPI Analyzer` mit `/kpi-summary`

Alle Skills sind anpassbar (Beschreibung, Slash-Befehl, Prompt-Template, Run-Mode `plan|execute`) und im Chat direkt nutzbar.

## Hinweis zum Scope

Dieses Repository ist auf iterative Umsetzung ausgelegt. Der aktuelle Stand deckt ab:
- Desktop-Host nur als Tauri-EXE ohne separaten Web-Port
- Chat + Planung mit Ollama-Integration
- SQLite-Persistenz fuer Threads, Tasks, Steps
- MCP-Server Probing und Tool-Ausfuehrung
- Zustand-basiertes State-Management mit DB-Sync
- 22 automatisierte Tests (14 Frontend + 8 Rust)
- CI/CD mit Lint, Test, Security-Gates
