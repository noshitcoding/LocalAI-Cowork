# Feature Implementierungsvergleich

Stand: 2026-04-24

Verglichene Codebasen:
- Dieses Projekt: `app/` (Tauri + React + Rust)
- Referenz 1: `andere_ai_agenten_beispiele/open-cowork` (OpenCoworkAI, Commit b8dea83)
- Referenz 2: `andere_ai_agenten_beispiele/open-claude-cowork` (ComposioHQ, Commit ec758c0)

## Bewertungsbasis

Eigene Feature-/Gap-Quellen:
- `features.md`
- `FEHLENDE_FEATURES_IM_CODE.md`
- `TRACEABILITY_MATRIX.md`

Codebelege (eigene Implementierung):
- File Safety inkl. Allowlist, Backup, Diff: `app/src-tauri/src/file_safety.rs`
- Artefakt-Parsing fuer viele Formate: `app/src-tauri/src/artifact_pipeline.rs`
- Scheduler-Expression Parsing: `app/src-tauri/src/scheduler.rs`
- Audit-Events (append-only JSONL): `app/src-tauri/src/audit.rs`
- Connector-UI mit Backend-Test: `app/src/components/ConnectorPanel.tsx`
- Skill-CRUD + Verbesserung: `app/src/components/SkillPanel.tsx`
- MCP Probe/Tool-Call: `app/src-tauri/src/mcp.rs`

Codebelege (Open Cowork):
- Sandbox Modul (WSL/Lima Adapter Export): `andere_ai_agenten_beispiele/open-cowork/src/main/sandbox/index.ts`
- MCP Lifecycle + stdio/SSE/Streamable HTTP: `andere_ai_agenten_beispiele/open-cowork/src/main/mcp/mcp-manager.ts`
- Skills Discovery + Hot-Reload: `andere_ai_agenten_beispiele/open-cowork/src/main/skills/skills-manager.ts`
- Plugin Runtime inkl. Install/Toggle/Uninstall: `andere_ai_agenten_beispiele/open-cowork/src/main/skills/plugin-runtime-service.ts`
- Remote Channels (Feishu/Slack): `andere_ai_agenten_beispiele/open-cowork/src/main/remote/remote-manager.ts`
- Produkt-Roadmap und Feature-Status: `andere_ai_agenten_beispiele/open-cowork/ROADMAP.md`

Codebelege (Open Claude Cowork):
- Provider-Basis mit Session Map: `andere_ai_agenten_beispiele/open-claude-cowork/server/providers/base-provider.js`
- Claude Provider mit Resume, Abort, Tool Streaming: `andere_ai_agenten_beispiele/open-claude-cowork/server/providers/claude-provider.js`
- SSE Chat API mit Provider-Abstraktion: `andere_ai_agenten_beispiele/open-claude-cowork/server/server.js`
- Renderer Multi-Chat, localStorage Persistenz: `andere_ai_agenten_beispiele/open-claude-cowork/renderer/renderer.js`

## Kurzfazit

- Deine Umsetzung ist aktuell staerker bei lokaler Sicherheit, Dateipipelines und Desktop-naher Persistenz im Rust-Backend.
- Open-Cowork ist staerker bei produktionsnaher Plattformbreite (Remote Control, Plugin Runtime, MCP Lifecycle mit mehreren Transporten, VM-Sandbox Integration).
- Open-Claude-Cowork ist staerker bei klarer Provider-Architektur und schneller End-to-End-Streaming-Integration, aber insgesamt deutlich schlanker.

## Vergleich nach Feature-Bloecken

| Feature-Block | Deine Umsetzung | Open-Cowork | Open-Claude-Cowork | Bewertung |
|---|---|---|---|---|
| Desktop Runtime | Tauri + Rust Backend, React UI | Electron + TS/Node | Electron + JS | Vorteil bei dir fuer native/sichere Rust-Backends; Vorteil Open-Cowork fuer breites Oekosystem |
| File Safety | Allowlist, Diff, Backup, Restore, harte Pfadpruefung | Sandbox + Path Guard, Fokus auf VM Isolation | Nur Basis-Workspace-Flow | Vorteil bei dir (feiner File-Safety Layer) |
| MCP | Probe + Tool-Call via Spawn pro Anfrage | Voller Manager mit Server-Lifecycle, stdio/SSE/Streamable HTTP | MCP via Composio Session + Provider Query | Vorteil Open-Cowork (MCP-Operations-Reife) |
| Skills | Skill CRUD + Improve + Learnings im Produkt | Skills Discovery, Frontmatter, Hot-Reload | SKILL.md Support im SDK-Flow | Open-Cowork leicht vorne bei Skill-Ops; du vorne bei in-App Lernverlauf |
| Plugins | Basiskonzept vorhanden, aber kein Marketplace-Lifecycle wie in Ref | Plugin Runtime inkl. Install/Toggle/Uninstall | Nicht im Fokus | Vorteil Open-Cowork |
| Remote Control | Connector-Panel und lokale Konfig/Test-Basis | Feishu/Slack Channel Gateway + Pairing | Kein volles Remote Gateway | Vorteil Open-Cowork |
| Scheduler | Scheduler Panel + parser fuer daily/weekday/every-X | In Roadmap als Ausbau, teils vorhanden | Nicht Kernfeature | Vorteil bei dir |
| Artefakte | Tiefe Parsing- und Export-Basis im Rust Layer | Starke Skill-Story fuer Office Outputs | Nicht Kernfeature | Vorteil bei dir |
| Session/Chat Persistenz | Lokal in Stores/DB + Audit orientiert | Session + Memory als Produkttrack | Sehr klar im Renderer + Provider Sessions | Open-Claude-Cowork gut in Einfachheit, du gut in Integrations-Tiefe |
| Compliance/Audit | Append-only Audit JSONL + DB Strukturen | Security stark, aber produktzentriert gemischt | Minimal | Vorteil bei dir |

## Was bei den anderen besser ist

1. Open-Cowork MCP-Betriebsschicht ist reifer.
   - Server-Lifecycle (start/stop/restart), Health, mehrere Transporte in einem Manager.
2. Open-Cowork Plugin/Remote Plattform ist deutlich weiter.
   - Installierbare Plugins mit Runtime-Materialisierung.
   - Feishu/Slack Remote Session Routing mit Pairing.
3. Open-Claude-Cowork hat sehr saubere Provider-Schnittstellen.
   - Klar getrennte Provider, Session Map, Abort-Controller, Streaming-Chunk-Normalisierung.

## Was bei deiner Umsetzung besser ist

1. File-Safety-Implementierung ist genauer und sicherheitsorientierter.
   - Erzwungene Allowlist, kanonische Pfadpruefung, Backup+Diff, Delete-Token.
2. Artefakt-Pipeline ist im Kern bereits breit implementiert.
   - Text/CSV/JSON/XML/HTML/ipynb/Bilder/SVG/PDF/DOCX/XLSX/PPTX.
3. Scheduler ist produktiv nutzbar mit klaren Regeln.
   - daily/weekday/every interval + UI + Run-Verlauf.
4. Audit- und Persistenz-Basis ist gut fuer Compliance-Ausbau.

## Konkrete Uebernahme-Kandidaten (Priorisiert)

### P0 (hoher Nutzen, direkt adaptierbar)

1. MCP-Lifecycle erweitern
- Ziel: Von einmaligem Spawn (`app/src-tauri/src/mcp.rs`) auf persistenten Server-Manager mit reconnect/health.
- Vorlage: `andere_ai_agenten_beispiele/open-cowork/src/main/mcp/mcp-manager.ts`.

2. Provider-Abstraktion fuer LLM-Router schaerfen
- Ziel: Einheitliches Interface wie BaseProvider + konkrete Provider-Module (Ollama, Anthropic, OpenAI-kompatibel).
- Vorlage: `andere_ai_agenten_beispiele/open-claude-cowork/server/providers/base-provider.js`.

3. Connector zu echten Channel-Runtimes ausbauen
- Ziel: Vom lokalen Connector-Panel zu Gateway-basierten Live-Channels inkl. Session-Mapping.
- Vorlage: `andere_ai_agenten_beispiele/open-cowork/src/main/remote/remote-manager.ts`.

### P1 (mittlerer Aufwand, hoher Produktwert)

1. Skill-Hot-Reload + Frontmatter-Discovery
- Ziel: Skill-Datei Aenderung ohne Neustart und robustes Skill-Indexing.
- Vorlage: `andere_ai_agenten_beispiele/open-cowork/src/main/skills/skills-manager.ts`.

2. Plugin-Lifecycle (install/toggle/uninstall) integrieren
- Ziel: kontrollierte Erweiterbarkeit fuer Teams.
- Vorlage: `andere_ai_agenten_beispiele/open-cowork/src/main/skills/plugin-runtime-service.ts`.

3. Session-Abort/Resume auf allen Backends vereinheitlichen
- Ziel: reproduzierbares Verhalten bei langen Runs, Abbruch und Wiederaufnahme.
- Vorlage: `andere_ai_agenten_beispiele/open-claude-cowork/server/providers/claude-provider.js`.

### P2 (strategischer Ausbau)

1. VM-Isolation tiefer integrieren (WSL2/Lima analog)
2. Remote Pairing/Approval Flow fuer mobile oder Chat-Channel
3. Marketplace-gestuetzte Skill/Plugin Distribution

## Deep Dive: CMD, Screenshot, Word/PPT

### 1) CMD/Bash Ausfuehrung

**Open-Cowork**
- Nutzt einen expliziten Command-Guard vor Ausfuehrung (Path-Traversal-Block, Mount-Grenzen, Pattern-Block fuer gefaehrliche Kommandos) in `src/main/tools/tool-executor.ts`.
- Koppelt Tool-Ausfuehrung an `SandboxAdapter` (`src/main/tools/sandbox-tool-executor.ts`), der je nach Plattform WSL/Lima/Native waehlt (`src/main/sandbox/sandbox-adapter.ts`).
- Ergebnis: klare Security-Schicht fuer Shell-Aufrufe.

**Open-Claude-Cowork**
- Reicht `allowedTools` inkl. `Bash` in den Provider-Call durch (`server/server.js`, `server/providers/claude-provider.js`).
- Kein vergleichbarer lokaler Command-Sanitizer/Path-Guard in der Server-Schicht gefunden.
- Ergebnis: schlankes Routing, aber weniger lokale Guardrails.

**Dieses Projekt**
- Bietet MCP-Runtime-Lifecycle (`app/src-tauri/src/mcp.rs` + Tauri Commands in `app/src-tauri/src/lib.rs`) und policy-gated Tool-Dispatch.
- Fuer harte Shell-Guardrails ist Open-Cowork aktuell tiefer.

**Bewertung**
- Security/Reife bei lokaler CMD-Ausfuehrung: **Open-Cowork vorne**.
- Architektur-Simplicity: **Open-Claude-Cowork vorne**.
- Lokale/no-cloud Integrationsfaehigkeit mit Rust-Steuerung: **dein Projekt + Open-Cowork-Muster**.

### 2) Screenshots / Computer-Use

**Open-Cowork**
- Eigener GUI-MCP-Server `src/main/mcp/gui-operate-server.ts` mit Tools wie `screenshot` und `screenshot_for_display`.
- `screenshot_for_display` liefert Bilddaten als base64 zur direkten Anzeige, inklusive kurzzeitiger Reuse-Logik und optionalen Annotationen.
- Windows-Pfad mit DPI-aware Screenshot-Capture.

**Open-Claude-Cowork**
- Im Chat-Renderer wird eine Live-Browser-URL (`live.anchorbrowser.io`) erkannt und als Embed angezeigt (`renderer/renderer.js`).
- Im Code keine lokale Screenshot-Capture-Implementierung gefunden; Screenshot-Claim steht in README.

**Dieses Projekt**
- Lokales Screenshot-MCP vorhanden (`LOCAL_SCREENSHOT_MCP_COMMAND`), mit Tools `list_screens` und `capture_screenshot` in `app/src-tauri/src/lib.rs`.
- Vorteil: lokal und ohne Cloud moeglich.
- Gap gegen Open-Cowork: kein gleichwertiges `screenshot_for_display` mit Image-Reply/Re-use/Annotate.

**Bewertung**
- Computer-Use + visuelle Rueckgabe im Agent-Flow: **Open-Cowork klar vorne**.
- Lokale no-cloud Basis unter Windows: **dein Projekt solide**.

### 3) Word/PowerPoint Integration

**Open-Cowork**
- Vollwertige Skill-Pakete fuer `docx` und `pptx` unter `.claude/skills/`.
- `docx/SKILL.md` beschreibt Workflows inkl. XML-basiertem Editieren, `docx-js`, Toolscripts.
- `pptx/SKILL.md` + `pptx/html2pptx.md` liefert reproduzierbaren HTML->PPTX-Workflow (PptxGenJS + html2pptx).
- `pptx/ooxml/scripts/pack.py` validiert/packt Office-Dateien robust.

**Open-Claude-Cowork**
- Kein dediziertes DOCX/PPTX-Skillpaket wie in Open-Cowork.
- Einziger direkter PPTX-Hinweis: `python-pptx`-Nutzung in `.claude/skills/brand-guidelines/SKILL.md`.

**Dieses Projekt**
- Native Artefakt-Ausgabe erzeugt bereits `.xlsx/.docx/.pptx/.pdf` (`app/src-tauri/src/cowork_features.rs`).
- Implementierung ist aktuell bewusst einfach (Report-Output), nicht so workflow-tief wie Open-Cowork Skills.

**Bewertung**
- Tiefe Office-Workflows (Agent-Authoring/Editing): **Open-Cowork vorne**.
- Integrierte lokale Generierung ohne Cloud-Abhaengigkeit: **dein Projekt vorne**.

## Empfohlene naechste Entscheidung

Waehle maximal 2 P0-Pakete fuer die erste Umsetzungswelle.
Empfehlung fuer schnellen sichtbaren Fortschritt:
1. MCP-Lifecycle erweitern
2. Provider-Abstraktion vereinheitlichen
