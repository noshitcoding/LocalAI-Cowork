# Development und Betrieb

## Lokale Entwicklung

```powershell
cd app
npm install
npm run tauri build
```

## Build

```powershell
cd app
npm run tauri build
```

## Tests

Vollstaendige lokale Qualitaetspipeline:

```powershell
cd app
npm run verify
```

`verify` ist das kanonische lokale und CI-Gate. Es prueft Toolchain, Release-Scripts,
TypeScript, ESLint, i18n, Frontend-Tests, Produktionsbuild, Bundle-Budgets sowie
`cargo check`, Rust-Tests und Clippy.

Frontend:

```powershell
cd app
npm run verify
```

`verify` ist das kanonische lokale und CI-Gate. Es prueft Toolchain, Release-Scripts,
TypeScript, ESLint, i18n, Frontend-Tests, Produktionsbuild, Bundle-Budgets sowie
`cargo check`, Rust-Tests und Clippy.

Frontend:

```powershell
cd app
npm run test:ci
```

Rust:

```powershell
cd app/src-tauri
cargo test
```

Desktop Smoke:

- [DESKTOP_SMOKE_TEST.md](DESKTOP_SMOKE_TEST.md)

Desktop-Steuerung und Computer Use:

- [DESKTOP_CONTROL_AND_COMPUTER_USE.md](DESKTOP_CONTROL_AND_COMPUTER_USE.md)

## CI-Gates (aktueller Stand)

- Toolchain Doctor und Release-Script-Tests muessen erfolgreich sein
- TypeScript und ESLint muessen ohne Fehler durchlaufen
- Das DE/EN-i18n-Audit muss vollstaendig sein
- Frontend Build muss erfolgreich sein
- Frontend Unit-Tests muessen erfolgreich sein
- Frontend Build-Budgets muessen eingehalten werden
- Rust Unit-Tests muessen erfolgreich sein
- Rust Compile-Check muss erfolgreich sein
- Rust Clippy darf keine Warnungen enthalten
- Produkt-, Cargo-, Tauri- und Tag-Version muessen konsistent sein
- Rust `1.89.0` und alle GitHub Actions sind unveraenderlich gepinnt
- Alle npm- und Windows-Cargo-Lizenzen muessen die explizite SPDX-Policy bestehen
- `npm audit --audit-level=high` und `cargo-audit 0.22.2` sind blockierend
- Semgrep Security Scan muss erfolgreich sein
- Trivy Filesystem Scan muss erfolgreich sein
- Tag-Releases muessen CycloneDX-SBOM, Drittanbieterhinweise, Provenienz, SHA256SUMS und GitHub-Attestierungen erzeugen

Lokale Supply-Chain-Pruefung:

```powershell
cd app
npm run supply-chain:check
npm run security:npm
cd src-tauri
cargo audit
```

Lokale Supply-Chain-Pruefung:

```powershell
cd app
npm run supply-chain:check
npm run security:npm
cd src-tauri
cargo audit
```

## Funktionstest: Cowork Chat

1. EXE bauen (`npm run tauri build`)
2. Die gebaute Desktop-App aus `app/src-tauri/target/release/bundle/` starten
3. Health-Check gegen Ollama ausfuehren
4. Im Bereich "Cowork Chat" eine Aufgabe senden
5. Antwort und ggf. Freigabehinweis pruefen
6. Bei Freigabehinweis "Plan freigeben" ausloesen

Vollstaendige Desktop-Checkliste:
- [DESKTOP_SMOKE_TEST.md](DESKTOP_SMOKE_TEST.md)

Technische Einordnung der Desktop-Steuerungswege:
- [DESKTOP_CONTROL_AND_COMPUTER_USE.md](DESKTOP_CONTROL_AND_COMPUTER_USE.md)

## Funktionstest: MCP Probe

Beispielkonfiguration:
- Name: `filesystem`
- Command: `npx`
- Args: `-y @modelcontextprotocol/server-filesystem .`

Schritte:
1. In den Einstellungen zur Kategorie "🔌 MCP Server" navigieren
2. Konfiguration im MCP-Bereich eintragen
3. "MCP Server pruefen" ausfuehren
4. Rueckgabe von Protocol-Version und Toolliste pruefen

## Funktionstest: Einstellungen (Settings)

Die Einstellungen sind in 9 Kategorien organisiert, erreichbar ueber die linke Sidebar:

| Kategorie | Inhalt |
|---|---|
| KI & Modell | Ollama-Endpunkt, Modellwahl, Health-Check, Persoenlichkeiten |
| Agent & Skills | Agent-Verhalten, Skills, Pipelines |
| Gedaechtnis | Memory-Eintraege, Profil, Provider, Hints |
| Sessions & Insights | Session-Suche, Nutzungsstatistiken |
| Terminal & Prozesse | Terminal-Backends, verwaltete Prozesse |
| MCP Server | MCP-Einstellungen, Server-Import, Probe, Tool-Ausfuehrung |
| Oberflaeche | UI-Darstellung, Benachrichtigungen, Sound |
| Sicherheit & Daten | Dateisicherheit, Datenhaltung |
| System & Info | Workspace, Autostart, App-Version |

Testschritte:
1. App starten, auf den "Settings"-Tab klicken
2. Alle 9 Kategorien in der Sidebar durchklicken
3. Pruefen dass jede Kategorie den korrekten Inhalt zeigt
4. In "KI & Modell" den Health-Check ausfuehren
5. In "Oberflaeche" den Kompaktmodus aktivieren/deaktivieren

## Runbook: Startprobleme

## Fall A: `npm run tauri build` erzeugt keine EXE

- Node/NPM Versionen pruefen
- Rust Toolchain pruefen (`rustc -V`, `cargo -V`)
- WebView2 Runtime vorhanden?

## Fall B: Ollama nicht erreichbar

- Dienststatus pruefen
- Netzwerkpfad/FW pruefen
- Endpoint in der UI korrigieren

## Fall C: Build in CI rot

- `npm run tauri build` lokal reproduzieren
- `npm run test:ci` lokal reproduzieren
- `cargo test` in `app/src-tauri` reproduzieren

## Backup und Recovery (Initialfassung)

- Git ist Quelle der Wahrheit fuer Quellcode
- Jede relevante Aenderung wird als eigener Commit erfasst
- Recovery erfolgt ueber Checkout eines stabilen Commits/Tags
- Die lokale Laufzeitdatenbank liegt als `open_cowork.db` im Tauri-App-Data-Verzeichnis.
- Vor jedem Schema-Upgrade einer bestehenden Datenbank wird mit der SQLite-Online-Backup-API eine verifizierte Kopie unter `database-backups/pre-migration-*.db` angelegt; maximal drei Kopien bleiben erhalten.
- Open Cowork muss vor einer manuellen Wiederherstellung vollstaendig beendet sein. Die aktuelle Datenbank sowie zugehoerige `-wal`- und `-shm`-Dateien werden zuerst separat gesichert und aus dem aktiven Verzeichnis entfernt; erst danach wird eine gepruefte Backup-Datei als `open_cowork.db` eingesetzt.
- Ein Integritaets-, Foreign-Key- oder Versionsfehler ist ein Stop-Zustand. Die App darf auf dieser Datenbank weder weiter migrieren noch neue Daten schreiben.

## Empfohlene Betriebsgrenzen (vorlaeufig)

- Timeout fuer Ollama >= 20s bei groesseren Modellen
- GPU/CPU-Auslastung waehrend Requests beobachten
- Produktivmodelle festlegen und dokumentieren
