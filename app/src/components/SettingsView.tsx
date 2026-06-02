import { useState } from 'react'
import { useConfigStore } from '../stores/configStore'
import type { AppPreferences, StartView } from '../stores/configStore'
import { useEngineStore } from '../stores/engineStore'
import { useCoworkStore } from '../stores/coworkStore'
import { DEFAULT_SYSTEM_PROMPT } from '../engine/config/engineConfig'
import MemoryPanel from './MemoryPanel'
import SkillPanel from './SkillPanel'
import InsightsPanel from './InsightsPanel'
import ProcessPanel from './ProcessPanel'
import TerminalPanel from './TerminalPanel'
import PersonalitySelector from './PersonalitySelector'
import SessionSearchPanel from './SessionSearchPanel'
import PipelinePanel from './PipelinePanel'
import CrewPanel from './CrewPanel'
import ConnectorPanel from './ConnectorPanel'
import McpView from './McpView'
import RunPanel from './RunPanel'
import RuntimeInstructionsPanel from './RuntimeInstructionsPanel'
import LlmProfilesPanel from './LlmProfilesPanel'

/* ── Tiny reusable primitives (App.css based) ── */

function Toggle({ checked, onChange, label, hint }: { checked: boolean; onChange: (v: boolean) => void; label: string; hint?: string }) {
  return (
    <label className="toggle-row">
      <div className="toggle-label">
        <span>{label}</span>
        {hint && <small className="hint-text">{hint}</small>}
      </div>
      <button
        type="button"
        role="switch"
        aria-checked={checked}
        onClick={() => onChange(!checked)}
        className={`toggle-switch${checked ? ' on' : ''}`}
      >
        <span className="toggle-knob" />
      </button>
    </label>
  )
}

function Section({ title, icon, children }: { title: string; icon: string; children: React.ReactNode }) {
  return (
    <div className="panel">
      <h2>{icon} {title}</h2>
      {children}
    </div>
  )
}

/* ── Category definitions ─────────────────────── */

const CATEGORIES = [
  { key: 'ai', label: 'KI & Modell', icon: '🤖' },
  { key: 'agent', label: 'Agent & Skills', icon: '⚡' },
  { key: 'memory', label: 'Gedaechtnis', icon: '🧠' },
  { key: 'sessions', label: 'Sessions & Insights', icon: '📂' },
  { key: 'terminal', label: 'Terminal & Prozesse', icon: '💻' },
  { key: 'mcp', label: 'MCP Server', icon: '🔌' },
  { key: 'ui', label: 'Oberflaeche', icon: '🎨' },
  { key: 'security', label: 'Sicherheit & Daten', icon: '🔒' },
  { key: 'system', label: 'System & Info', icon: '📁' },
] as const

type CategoryKey = (typeof CATEGORIES)[number]['key']

/* ── Main Component ─────────────────────────── */

export default function SettingsView() {
  const {
    ollama,
    preferences,
    setPreference,
  } = useConfigStore()
  const engineConfig = useEngineStore((s) => s.config)
  const setEngineConfig = useEngineStore((s) => s.setConfig)
  const globalInstruction = useCoworkStore((s) => s.globalInstruction)
  const setGlobalInstruction = useCoworkStore((s) => s.setGlobalInstruction)
  const [activeCategory, setActiveCategory] = useState<CategoryKey>('ai')

  const pref = <K extends keyof AppPreferences>(key: K) => ({
    checked: preferences[key] as boolean,
    onChange: (v: boolean) => setPreference(key, v as AppPreferences[K]),
  })

  return (
    <div className="settings-layout">
      {/* Sidebar navigation */}
      <nav className="settings-sidebar" role="navigation" aria-label="Einstellungs-Kategorien">
        {CATEGORIES.map((cat) => (
          <button
            key={cat.key}
            type="button"
            className={`settings-nav-item${activeCategory === cat.key ? ' active' : ''}`}
            onClick={() => setActiveCategory(cat.key)}
          >
            <span className="settings-nav-icon">{cat.icon}</span>
            <span className="settings-nav-label">{cat.label}</span>
          </button>
        ))}
      </nav>

      {/* Content area */}
      <div className="settings-content">
        {/* ── KI & Modell ───────────── */}
        {activeCategory === 'ai' && (
          <div className="settings-view">
            <h1>KI & Modell</h1>
            <p className="hint-text">Mehrere LLM-Profile, globale Provider-Defaults und Persoenlichkeiten konfigurieren</p>

            <LlmProfilesPanel />

            <Section title="Streaming" icon="💾">
              <Toggle label="Stream-Antworten automatisch speichern" hint="Ollama-Antworten werden waehrend des Streamings gesichert" {...pref('ollamaStreamAutosave')} />
            </Section>
            <PersonalitySelector />
          </div>
        )}

        {/* ── Agent & Skills ────────── */}
        {activeCategory === 'agent' && (
          <div className="settings-view settings-view-wide">
            <h1>Agent & Skills</h1>
            <p className="hint-text">Agent-Verhalten steuern, Skills verwalten und Pipelines konfigurieren</p>

            <Section title="Agent-Verhalten" icon="⚡">
              <Toggle label="Sichere Tools automatisch genehmigen" hint="Leseoperationen ohne Bestätigung ausfuehren" {...pref('autoApproveSafeTools')} />
              <Toggle label="Autopilot fuer alle Tools" hint="Alle Tool-Aufrufe automatisch genehmigen (Vorsicht!)" {...pref('autoPilotAllTools')} />
              <Toggle label="Bei wiederholtem Fehler zum Menschen wechseln" hint="Agent stoppt nach mehreren Fehlversuchen" {...pref('fallbackToHumanOnRepeatedFailure')} />
              <Toggle label="Batch Multi-Select fuer Tasks" hint="Mehrere Tasks gleichzeitig auswaehlen und bearbeiten" {...pref('taskBatchMultiSelectEnabled')} />
              <div className="grid" style={{ marginTop: 12 }}>
                <label>
                  Max Tool-Aufrufe pro Schleife
                  <input type="number" min={1} max={50} value={preferences.maxToolCallsPerLoop} onChange={(e) => setPreference('maxToolCallsPerLoop', Number(e.target.value))} />
                </label>
              </div>
            </Section>

            <Section title="Engine-Konfiguration" icon="🔧">
              <div className="grid">
                <label>
                  Max Turns pro Anfrage
                  <input type="number" min={1} max={100} value={engineConfig.maxTurns} onChange={(e) => setEngineConfig({ maxTurns: Number(e.target.value) })} />
                </label>
                <label>
                  Session Persistence
                  <select value={engineConfig.sessionPersistence ? 'enabled' : 'disabled'} onChange={(e) => setEngineConfig({ sessionPersistence: e.target.value === 'enabled' })}>
                    <option value="enabled">Aktiviert</option>
                    <option value="disabled">Deaktiviert</option>
                  </select>
                </label>
                <label>
                  Berechtigungs-Modus
                  <select value={engineConfig.permissionMode} onChange={(e) => setEngineConfig({ permissionMode: e.target.value as 'default' | 'plan' | 'bypass' | 'strict' })}>
                    <option value="default">Standard</option>
                    <option value="plan">Plan-Modus</option>
                    <option value="bypass">Bypass (alles erlauben)</option>
                    <option value="strict">Strikt (alles fragen)</option>
                  </select>
                </label>
              </div>
            </Section>

            <Section title="Systemprompts" icon="SP">
              <label style={{ marginTop: 12, display: 'block' }}>
                Basis-Systemprompt
                <textarea
                  rows={10}
                  value={engineConfig.systemPrompt}
                  onChange={(e) => setEngineConfig({ systemPrompt: e.target.value })}
                  placeholder="Basisverhalten fuer die agentische Engine..."
                  style={{ width: '100%', resize: 'vertical', fontFamily: 'monospace' }}
                />
              </label>
              <div style={{ display: 'flex', gap: 8, marginTop: 8, marginBottom: 12 }}>
                <button type="button" className="btn-sm" onClick={() => setEngineConfig({ systemPrompt: DEFAULT_SYSTEM_PROMPT })}>
                  Auf Standard zuruecksetzen
                </button>
              </div>
              <label style={{ display: 'block', marginBottom: 12 }}>
                System-Prompt Erweiterung
                <textarea
                  rows={3}
                  value={engineConfig.appendSystemPrompt}
                  onChange={(e) => setEngineConfig({ appendSystemPrompt: e.target.value })}
                  placeholder="Zusaetzliche Anweisungen fuer den Agenten..."
                  style={{ width: '100%', resize: 'vertical' }}
                />
              </label>
              <label style={{ display: 'block' }}>
                Globale Cowork-Instruktion
                <textarea
                  rows={4}
                  value={globalInstruction}
                  onChange={(e) => setGlobalInstruction(e.target.value)}
                  placeholder="Projektweite Instruktionen fuer Chat und Cowork..."
                  style={{ width: '100%', resize: 'vertical' }}
                />
              </label>
            </Section>

            <SkillPanel />
            <PipelinePanel />
            <CrewPanel />
          </div>
        )}

        {/* ── Gedaechtnis ───────────── */}
        {activeCategory === 'memory' && (
          <div className="settings-view">
            <h1>Gedaechtnis</h1>
            <p className="hint-text">Agent-Memory, Profil, Provider und Hinweise verwalten</p>
            <MemoryPanel />
          </div>
        )}

        {/* ── Sessions & Insights ──── */}
        {activeCategory === 'sessions' && (
          <div className="settings-view">
            <h1>Sessions & Insights</h1>
            <p className="hint-text">Vergangene Sessions durchsuchen und Nutzungsstatistiken einsehen</p>
            <SessionSearchPanel />
            <InsightsPanel />
            <RunPanel />
          </div>
        )}

        {/* ── Terminal & Prozesse ──── */}
        {activeCategory === 'terminal' && (
          <div className="settings-view">
            <h1>Terminal & Prozesse</h1>
            <p className="hint-text">Terminal-Backends und verwaltete Prozesse konfigurieren</p>
            <TerminalPanel />
            <ProcessPanel />
          </div>
        )}

        {/* ── MCP Server ────────────── */}
        {activeCategory === 'mcp' && (
          <div className="settings-view">
            <h1>MCP Server</h1>
            <p className="hint-text">Model Context Protocol Server verwalten und testen</p>

            <Section title="MCP Einstellungen" icon="🔌">
              <Toggle label="Auto-Reconnect" hint="MCP-Server bei Verbindungsverlust automatisch neu verbinden" {...pref('mcpAutoReconnect')} />
              <Toggle label="Verbose Logging" hint="Detailliertes MCP-Protokoll-Logging" {...pref('mcpVerboseLogging')} />
              <Toggle label="Env-Editor aktivieren" hint="Umgebungsvariablen manuell bearbeiten" {...pref('mcpEnvEditorEnabled')} />
              <Toggle label="Manueller JSON-Import" hint="MCP-Server per JSON-Import hinzufuegen" {...pref('mcpAllowManualImport')} />
            </Section>

            <McpView />
          </div>
        )}

        {/* ── Oberflaeche ────────────── */}
        {activeCategory === 'ui' && (
          <div className="settings-view">
            <h1>Oberflaeche</h1>
            <p className="hint-text">Darstellung, Benachrichtigungen und akustisches Feedback anpassen</p>

            <Section title="Darstellung" icon="🎨">
              <Toggle label="Fokusmodus" hint="Sidebar und Ablenkungen ausblenden" {...pref('focusMode')} />
              <Toggle label="Kompaktmodus" hint="Weniger Abstaende fuer mehr Inhalt" {...pref('compactMode')} />
              <Toggle label="Verbose-Modus" hint="Interne Prompts, Dateikontext und Tool-/MCP-Diagnose im Chat anzeigen" {...pref('verboseMode')} />
              <Toggle label="Thinking-Fenster begrenzen" hint="Im Verbose-Modus nur die letzten 50 Thinking-Zeilen live anzeigen" {...pref('limitThinkingWindow')} />
              <Toggle label="Super-Verbose Audit" hint="Speichert User-Prompts, Antworten, Tool-Calls und Tool-Outputs vollstaendig in audit/events.jsonl" {...pref('superVerboseAuditLogging')} />
              <Toggle label="Zeitstempel anzeigen" hint="Uhrzeiten bei Chat-Nachrichten" {...pref('showTimestamps')} />
              <Toggle label="Shortcut-Overlay aktivieren" hint="Tastenkuerzel-Hilfe ueber Ctrl+Shift+?" {...pref('shortcutOverlayEnabled')} />
              <Toggle label="Theme mit System synchronisieren" hint="Light/Dark automatisch nach OS-Einstellung" {...pref('syncThemeWithSystem')} />
              <div className="grid" style={{ marginTop: 12 }}>
                <label>
                  Schriftgroesse (%)
                  <input type="number" min={85} max={120} step={5} value={preferences.fontScale} onChange={(e) => setPreference('fontScale', Number(e.target.value))} />
                </label>
                <label>
                  Startansicht
                  <select value={preferences.defaultStartView} onChange={(e) => setPreference('defaultStartView', e.target.value as StartView)}>
                    <option value="last">Letzte Ansicht</option>
                    <option value="work">Arbeitsbereich</option>
                    <option value="settings">Einstellungen</option>
                  </select>
                </label>
              </div>
            </Section>

            <Section title="Benachrichtigungen & Sound" icon="🔔">
              <Toggle label="Desktop-Benachrichtigungen" hint="Windows-Notifications bei wichtigen Ereignissen" {...pref('notificationsEnabled')} />
              <Toggle label="Sounds aktivieren" hint="Akustisches Feedback bei Aktionen" {...pref('soundsEnabled')} />
              <Toggle label="Bestaetigung beim Schliessen" hint="Fragt vor dem Beenden der Desktop-App nach" {...pref('confirmOnCloseWithRunningTasks')} />
            </Section>
          </div>
        )}

        {/* ── Sicherheit & Daten ──── */}
        {activeCategory === 'security' && (
          <div className="settings-view">
            <h1>Sicherheit & Daten</h1>
            <p className="hint-text">Dateizugriff, Befehlsfilter und Datenhaltung konfigurieren</p>

            <Section title="Dateisicherheit" icon="🔒">
              <Toggle label="Nur-Lesen-Modus" hint="Keine Dateien schreiben oder loeschen" {...pref('readOnlyFsMode')} />
              <div className="grid" style={{ marginTop: 12 }}>
                <label>
                  Erlaubte Befehle (Whitelist)
                  <textarea rows={3} value={preferences.commandWhitelist} onChange={(e) => setPreference('commandWhitelist', e.target.value)} placeholder="Ein Befehl pro Zeile" />
                </label>
                <label>
                  Gesperrte Befehle (Blacklist)
                  <textarea rows={3} value={preferences.commandBlacklist} onChange={(e) => setPreference('commandBlacklist', e.target.value)} placeholder="Ein Befehl pro Zeile" />
                </label>
              </div>
            </Section>

            <Section title="Daten & Speicherung" icon="💾">
              <Toggle label="Telemetrie aktivieren" hint="Anonyme Nutzungsstatistiken senden" {...pref('telemetryEnabled')} />
              <Toggle label="Automatisches DB-Backup" hint="SQLite-Datenbank regelmaessig sichern" {...pref('autoBackupDb')} />
              <Toggle label="DB-Cleanup beim Start" hint="Verwaiste Eintraege beim App-Start bereinigen" {...pref('dbCleanupOnStart')} />
              <div className="grid" style={{ marginTop: 12 }}>
                <label>
                  Chat-Aufbewahrung (Tage)
                  <input type="number" min={1} max={365} value={preferences.chatRetentionDays} onChange={(e) => setPreference('chatRetentionDays', Number(e.target.value))} />
                </label>
                <label>
                  Backup-Intervall (Stunden)
                  <input type="number" min={1} max={168} value={preferences.dbBackupIntervalHours} onChange={(e) => setPreference('dbBackupIntervalHours', Number(e.target.value))} />
                </label>
              </div>
            </Section>

            <RuntimeInstructionsPanel />
          </div>
        )}

        {/* ── System & Info ────────── */}
        {activeCategory === 'system' && (
          <div className="settings-view">
            <h1>System & Info</h1>
            <p className="hint-text">Workspace-Pfade, Autostart und App-Informationen</p>

            <Section title="Workspace & System" icon="📁">
              <Toggle label="Beim Systemstart starten" hint="App automatisch mit Windows starten" {...pref('launchAtStartup')} />
              <div className="grid" style={{ marginTop: 12 }}>
                <label>
                  Standard-Workspace-Pfad
                  <input value={preferences.workspaceDefaultPath} onChange={(e) => setPreference('workspaceDefaultPath', e.target.value)} placeholder="C:\Projekte\mein-workspace" style={{ fontFamily: 'monospace' }} />
                </label>
              </div>
            </Section>

            <ConnectorPanel />

            <Section title="Ueber Open_Cowork" icon="✦">
              <div className="card">
                <p><strong>Open_Cowork</strong> v0.2.0</p>
                <p>Tauri + React + Rust Desktop-App fuer agentisches Arbeiten</p>
                <p><strong>Endpoint:</strong> {ollama.baseUrl}</p>
                <p><strong>Modell:</strong> {ollama.model}</p>
              </div>
            </Section>
          </div>
        )}
      </div>
    </div>
  )
}
