import { useEffect, useState } from 'react'
import { useConfigStore } from '../stores/configStore'
import type { AppPreferences, StartView } from '../stores/configStore'
import { useEngineStore } from '../stores/engineStore'
import MemoryPanel from './MemoryPanel'
import SkillPanel from './SkillPanel'
import InsightsPanel from './InsightsPanel'
import ProcessPanel from './ProcessPanel'
import TerminalPanel from './TerminalPanel'
import PersonalitySelector from './PersonalitySelector'
import SessionSearchPanel from './SessionSearchPanel'
import PipelinePanel from './PipelinePanel'
import SchedulerPanel from './SchedulerPanel'
import CrewPanel from './CrewPanel'
import ConnectorPanel from './ConnectorPanel'
import McpView from './McpView'
import RunPanel from './RunPanel'
import RuntimeInstructionsPanel from './RuntimeInstructionsPanel'

type OllamaHealth = {
  ok: boolean
  models: string[]
  error: string | null
}

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
    setOllama,
    openAIComputerUse,
    setOpenAIComputerUse,
    preferences,
    setPreference,
    availableModels,
    setAvailableModels,
  } = useConfigStore()
  const engineConfig = useEngineStore((s) => s.config)
  const setEngineConfig = useEngineStore((s) => s.setConfig)
  const checkOllamaStatus = useEngineStore((s) => s.checkOllamaStatus)
  const fetchOllamaModels = useEngineStore((s) => s.fetchOllamaModels)
  const contextWarning = useEngineStore((s) => s.contextWarning)
  const compactionCount = useEngineStore((s) => s.compactionCount)
  const currentSessionId = useEngineStore((s) => s.currentSessionId)
  const [health, setHealth] = useState<OllamaHealth | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [activeCategory, setActiveCategory] = useState<CategoryKey>('ai')

  const parseNumberInput = (raw: string, fallback: number): number => {
    const normalized = raw.replace(',', '.').trim()
    const parsed = Number(normalized)
    return Number.isFinite(parsed) ? parsed : fallback
  }

  const pref = <K extends keyof AppPreferences>(key: K) => ({
    checked: preferences[key] as boolean,
    onChange: (v: boolean) => setPreference(key, v as AppPreferences[K]),
  })

  useEffect(() => {
    if (availableModels.length > 0) return
    void refreshModelsOnly()
  }, [])

  const refreshModelsOnly = async () => {
    try {
      const models = await fetchOllamaModels()
      setAvailableModels(models.map((model) => model.id))
    } catch {
      // optional convenience refresh
    }
  }

  const runHealthCheck = async () => {
    setBusy(true)
    setError(null)
    try {
      const [ok, models] = await Promise.all([
        checkOllamaStatus(),
        fetchOllamaModels().catch(() => []),
      ])
      const names = models.map((model) => model.id)
      setAvailableModels(names)
      setHealth({ ok, models: names, error: ok ? null : 'Ollama ist nicht erreichbar.' })
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
      setHealth(null)
    } finally {
      setBusy(false)
    }
  }

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
            <p className="hint-text">Ollama-Endpunkt, Modellwahl und Persoenlichkeiten konfigurieren</p>

            <Section title="Ollama Konfiguration" icon="🤖">
              <div className="grid">
                <label>
                  Endpoint
                  <input value={ollama.baseUrl} onChange={(e) => setOllama({ baseUrl: e.target.value })} placeholder="http://192.168.178.82:11434" style={{ fontFamily: 'monospace' }} />
                </label>
                <label>
                  Modell
                  {availableModels.length > 0 ? (
                    <select value={ollama.model} onChange={(e) => setOllama({ model: e.target.value })}>
                      {availableModels.map((m) => <option key={m} value={m}>{m}</option>)}
                      {!availableModels.includes(ollama.model) && <option value={ollama.model}>{ollama.model}</option>}
                    </select>
                  ) : (
                    <input value={ollama.model} onChange={(e) => setOllama({ model: e.target.value })} placeholder="gpt-oss:20b" />
                  )}
                </label>
                <label>
                  Timeout (ms)
                  <input
                    type="number"
                    min={1000}
                    max={86400000}
                    step={1000}
                    value={ollama.timeoutMs}
                    onChange={(e) => setOllama({ timeoutMs: parseNumberInput(e.target.value, ollama.timeoutMs) })}
                  />
                </label>
                <label>
                  Context Window
                  <input
                    type="number"
                    min={512}
                    max={262144}
                    step={512}
                    value={ollama.contextWindow}
                    onChange={(e) => setOllama({ contextWindow: parseNumberInput(e.target.value, ollama.contextWindow) })}
                  />
                </label>
                <label>
                  Temperature
                  <input
                    type="number"
                    min={0}
                    max={2}
                    step={0.05}
                    value={ollama.temperature}
                    onChange={(e) => setOllama({ temperature: parseNumberInput(e.target.value, ollama.temperature) })}
                  />
                </label>
              </div>
              <div className="actions">
                <button disabled={busy} onClick={runHealthCheck}>
                  {busy ? '⏳ Pruefung laeuft...' : '🔍 Health Check'}
                </button>
                <button disabled={busy} onClick={() => void refreshModelsOnly()}>
                  Modelle laden
                </button>
                {health && <span className={health.ok ? 'success' : 'error'}>{health.ok ? '✓ Verbunden' : '✗ Fehler'}</span>}
              </div>
              {error && <p className="error" style={{ marginTop: 12 }}>{error}</p>}
              {health && (
                <div className="card" style={{ marginTop: 12 }}>
                  <p><strong>Verfuegbare Modelle:</strong> {health.models.join(', ') || 'keine'}</p>
                  {health.error && <p><strong>Fehler:</strong> {health.error}</p>}
                </div>
              )}
              <div className="card" style={{ marginTop: 12 }}>
                <p><strong>Aktive Session:</strong> {currentSessionId ?? 'Keine geladen'}</p>
                <p><strong>Compactions:</strong> {compactionCount}</p>
                <p><strong>Kontextwarnung:</strong> {contextWarning.level === 'none' ? 'Keine' : `${contextWarning.level} (${contextWarning.estimatedTokens} Tokens)`}</p>
              </div>
              <Toggle label="Stream-Antworten automatisch speichern" hint="Ollama-Antworten werden waehrend des Streamings gesichert" {...pref('ollamaStreamAutosave')} />
            </Section>

            <Section title="OpenAI Computer Use" icon="🖱️">
              <div className="grid">
                <label>
                  API Key
                  <input
                    type="password"
                    value={openAIComputerUse.apiKey}
                    onChange={(e) => setOpenAIComputerUse({ apiKey: e.target.value })}
                    placeholder="sk-..."
                    style={{ fontFamily: 'monospace' }}
                  />
                </label>
                <label>
                  Base URL
                  <input
                    value={openAIComputerUse.baseUrl}
                    onChange={(e) => setOpenAIComputerUse({ baseUrl: e.target.value })}
                    placeholder="https://api.openai.com/v1"
                    style={{ fontFamily: 'monospace' }}
                  />
                </label>
                <label>
                  Modell
                  <input
                    value={openAIComputerUse.model}
                    onChange={(e) => setOpenAIComputerUse({ model: e.target.value })}
                    placeholder="computer-use-preview"
                    style={{ fontFamily: 'monospace' }}
                  />
                </label>
                <label>
                  Max Steps
                  <input
                    type="number"
                    min={1}
                    max={200}
                    value={openAIComputerUse.maxSteps}
                    onChange={(e) => setOpenAIComputerUse({ maxSteps: parseNumberInput(e.target.value, openAIComputerUse.maxSteps) })}
                  />
                </label>
                <label>
                  Action Delay (ms)
                  <input
                    type="number"
                    min={0}
                    max={10000}
                    step={50}
                    value={openAIComputerUse.actionDelayMs}
                    onChange={(e) => setOpenAIComputerUse({ actionDelayMs: parseNumberInput(e.target.value, openAIComputerUse.actionDelayMs) })}
                  />
                </label>
                <label>
                  Launch Delay (ms)
                  <input
                    type="number"
                    min={0}
                    max={30000}
                    step={100}
                    value={openAIComputerUse.launchDelayMs}
                    onChange={(e) => setOpenAIComputerUse({ launchDelayMs: parseNumberInput(e.target.value, openAIComputerUse.launchDelayMs) })}
                  />
                </label>
              </div>
              <Toggle
                label="Safety Checks automatisch bestaetigen"
                hint="Nur fuer kontrollierte lokale Testumgebungen. Sonst Human-in-the-loop beibehalten."
                checked={openAIComputerUse.autoAcknowledgeSafetyChecks}
                onChange={(value) => setOpenAIComputerUse({ autoAcknowledgeSafetyChecks: value })}
              />
            </Section>
            <PersonalitySelector />
          </div>
        )}

        {/* ── Agent & Skills ────────── */}
        {activeCategory === 'agent' && (
          <div className="settings-view">
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
              <label style={{ marginTop: 12, display: 'block' }}>
                System-Prompt Erweiterung
                <textarea
                  rows={3}
                  value={engineConfig.appendSystemPrompt}
                  onChange={(e) => setEngineConfig({ appendSystemPrompt: e.target.value })}
                  placeholder="Zusaetzliche Anweisungen fuer den Agenten..."
                  style={{ width: '100%', resize: 'vertical' }}
                />
              </label>
            </Section>

            <SkillPanel />
            <PipelinePanel />
            <SchedulerPanel />
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
              <Toggle label="Bestaetigung beim Schliessen mit laufenden Tasks" hint="Warnung vor versehentlichem Beenden" {...pref('confirmOnCloseWithRunningTasks')} />
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
