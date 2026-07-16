# Architektur

> Superseded: This historical architecture snapshot is no longer the source of truth. Use [ARCHITECTURE_CURRENT.md](ARCHITECTURE_CURRENT.md) and [SOURCE_OF_TRUTH.md](SOURCE_OF_TRUTH.md) for current architecture and documentation precedence.

## Uebersicht

Open_Cowork verwendet eine lokale Desktop-Architektur mit klarer Trennung zwischen UI und privilegierten Operationen.

```text
[React UI (Zustand Stores)]
    |
    v (Tauri invoke)
[Rust Core / Commands]
    |
    +--> [Ollama Client]
    +--> [Cowork Chat Turn Logic]
    +--> [MCP Client (probe + tool call)]
    +--> [SQLite Persistence (rusqlite)]
    +--> [Logging]
```

## Komponenten

## 1. Frontend (`app/src`)

### Routing & Layout
- React Router mit 2 Routen: Cowork (`/`), Einstellungen (`/settings`)
- Top-Bar mit Cowork/Settings Tabs und Shortcuts (Ctrl+1/2)
- Optionaler linker Sidebar fuer Thread-Liste, Command-Palette (Ctrl+K)

### Zustand Stores
- **configStore**: Ollama-Konfiguration (baseUrl, model, timeout), MCP-Server, App-Preferences, localStorage-Persistenz
- **chatStore**: Chat-Threads, Messages, Approval-State, DB-Sync via Tauri-Commands
- **taskStore**: Task-Lifecycle, Steps mit Status, DB-Sync via Tauri-Commands
- **memoryStore**: Agent-Memory-Eintraege, Profil, Provider, Hints
- **skillStore**: Erlernbare Skills, Lernverlauf, Auto-Generierung
- **engineStore**: QueryEngine-Bindung, Ollama-Laufzeit, Session-Laden/Speichern, Kontext- und Approval-State
- **insightsStore**: Nutzungsstatistiken, Events, Summary
- **personalityStore**: Persoenlichkeitsprofile mit Model-Override
- **pipelineStore**: Artifact-Pipelines und Tool-Gateway
- **processStore**: Verwaltete Hintergrundprozesse
- **terminalStore**: Terminal-Backends, Befehlsausfuehrung

### Views
- **CoworkView / WelcomeScreen**: Thread-Management, Nachrichtenverlauf, Approval-Box, Ollama-Integration
- **SettingsView**: Unified-Settings mit 9-Kategorie-Sidebar:
  - 🤖 KI & Modell — Ollama-Konfiguration, Health-Check, ModelSwitcher, PersonalitySelector
  - ⚡ Agent & Skills — Agent-Verhalten-Toggles, SkillPanel, PipelinePanel
  - 🧠 Gedaechtnis — MemoryPanel (Eintraege, Profil, Provider, Hints)
  - 📂 Sessions & Insights — SessionSearchPanel, InsightsPanel
  - 💻 Terminal & Prozesse — TerminalPanel, ProcessPanel
  - 🔌 MCP Server — MCP-Einstellungen + vollstaendige McpView
  - 🎨 Oberflaeche — UI-Toggles, Benachrichtigungen, Sound
  - 🔒 Sicherheit & Daten — Dateisicherheit, Datenhaltung
  - 📁 System & Info — Workspace, Autostart, Ueber

## 2. Tauri Shell (`app/src-tauri/src/lib.rs`)

- Registry fuer Commands:
  - Ollama: `ollama_health_check`, `generate_plan`, `chat_turn`
  - MCP: `mcp_probe`, `mcp_call_tool`
  - Persistenz: `db_save_thread`, `db_list_threads`, `db_delete_thread`,
    `db_save_message`, `db_list_messages`, `db_save_task`, `db_update_task_status`,
    `db_list_tasks`, `db_save_step`, `db_update_step`, `db_list_steps`
- SQLite-Datenbank-Initialisierung im `setup()` Hook
- Fehler-Mapping in nutzerlesbare Strings
- Logging-Plugin in Debug-Builds

## 3. Ollama Client (`app/src-tauri/src/ollama.rs`)

- Konfigurierbarer Endpoint, Modellname, Timeout
- Endpoint-Validierung via URL-Parser
- HTTP-Aufrufe via `reqwest` mit Timeout
- Parse-Logik fuer nummerierte Schritte
- Chat-Turn-Logik mit Verlaufseinbettung
- Risikoheuristik fuer Freigabeanforderungen

## 4. MCP Client (`app/src-tauri/src/mcp.rs`)

- `probe_server`: Startet MCP-Server-Prozesse via stdio, sendet JSON-RPC `initialize` und `tools/list`, gibt Tool-Metadaten zurueck
- `call_tool`: Startet MCP-Server, initialisiert, sendet `tools/call` mit Argumenten, parst Content-Array aus Ergebnis

## 5. SQLite Persistenz (`app/src-tauri/src/db.rs`)

- Schema-Migration mit Versionierung
- Tabellen: `chat_threads`, `chat_messages`, `tasks`, `task_steps`, `audit_events`
- WAL-Modus fuer Performance, Foreign Keys aktiv
- Thread-safe via `Mutex<Connection>`
- In-Memory-Modus fuer Tests
- Datenbank-Pfad: `%APPDATA%/io.noshitcoding.opencowork/open_cowork.db`

## Sicherheits- und Robustheitsprinzipien

- Keine Mock-Integration fuer LLM-Aufrufe
- Explizite Fehlerbehandlung bei Netzwerk- und Parsefehlern
- Defensive Defaults (`http://localhost:11434`, `llama3.1:8b`)
- Input-Validierung (leere Commands, ungueltige URLs)
- Foreign Key Constraints mit CASCADE-Loeschung
- Unit-Tests fuer DB, Parsing, Store-Logik
