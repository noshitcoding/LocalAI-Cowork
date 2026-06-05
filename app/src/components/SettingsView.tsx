import { useState } from 'react'
import { useTranslation } from 'react-i18next'
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
import { tr } from '../i18n'

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
  { key: 'ai', label: 'AI & model', icon: '🤖' },
  { key: 'agent', label: 'Agent & Skills', icon: '⚡' },
  { key: 'memory', label: 'Memory', icon: '🧠' },
  { key: 'sessions', label: 'Sessions & Insights', icon: '📂' },
  { key: 'terminal', label: 'Terminal & Processes', icon: '💻' },
  { key: 'mcp', label: 'MCP Server', icon: '🔌' },
  { key: 'ui', label: 'Interface', icon: '🎨' },
  { key: 'security', label: 'Security & data', icon: '🔒' },
  { key: 'system', label: 'System & Info', icon: '📁' },
] as const

type CategoryKey = (typeof CATEGORIES)[number]['key']

/* ── Main Component ─────────────────────────── */

export default function SettingsView() {
  useTranslation()

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
      <nav className="settings-sidebar" role="navigation" aria-label={tr("Settings categories")}>
        {CATEGORIES.map((cat) => (
          <button
            key={cat.key}
            type="button"
            className={`settings-nav-item${activeCategory === cat.key ? ' active' : ''}`}
            onClick={() => setActiveCategory(cat.key)}
          >
            <span className="settings-nav-icon">{cat.icon}</span>
            <span className="settings-nav-label">{tr(cat.label)}</span>
          </button>
        ))}
      </nav>

      {/* Content area */}
      <div className="settings-content">
        {/* ── KI & Model ───────────── */}
        {activeCategory === 'ai' && (
          <div className="settings-view">
            <h1>{tr("AI & model")}</h1>
            <p className="hint-text">{tr("Configure multiple LLM profiles, global provider defaults, and personalities")}</p>

            <LlmProfilesPanel />

            <Section title={tr("Streaming")} icon="💾">
              <Toggle label={tr("Automatically save stream answers")} hint={tr("Ollama answers are saved during streaming")} {...pref('ollamaStreamAutosave')} />
            </Section>
            <PersonalitySelector />
          </div>
        )}

        {/* ── Agent & Skills ────────── */}
        {activeCategory === 'agent' && (
          <div className="settings-view settings-view-wide">
            <h1>{tr("Agent & Skills")}</h1>
            <p className="hint-text">{tr("Control agent behavior, manage skills, and configure pipelines")}</p>

            <Section title={tr("Agent behavior")} icon="⚡">
              <Toggle label={tr("Automatically approve safe tools")} hint={tr("Execute read operations without confirmation")} {...pref('autoApproveSafeTools')} />
              <Toggle label={tr("Autopilot for all tools")} hint={tr("Approve all tool calls automatically (caution!)")} {...pref('autoPilotAllTools')} />
              <Toggle label={tr("Fallback to a human after repeated errors")} hint={tr("Agent stops after repeated failed attempts")} {...pref('fallbackToHumanOnRepeatedFailure')} />
              <Toggle label={tr("Batch multi-select for tasks")} hint={tr("Select and edit multiple tasks at once")} {...pref('taskBatchMultiSelectEnabled')} />
              <div className="grid" style={{ marginTop: 12 }}>
                <label>{tr("Max tool calls per loop")}<input type="number" min={1} max={50} value={preferences.maxToolCallsPerLoop} onChange={(e) => setPreference('maxToolCallsPerLoop', Number(e.target.value))} />
                </label>
              </div>
            </Section>

            <Section title={tr("Engine configuration")} icon="🔧">
              <div className="grid">
                <label>{tr("Max turns per request")}<input type="number" min={1} max={100} value={engineConfig.maxTurns} onChange={(e) => setEngineConfig({ maxTurns: Number(e.target.value) })} />
                </label>
                <label>{tr("Session Persistence")}<select value={engineConfig.sessionPersistence ? 'enabled' : 'disabled'} onChange={(e) => setEngineConfig({ sessionPersistence: e.target.value === 'enabled' })}>
                    <option value="enabled">{tr("Enabled")}</option>
                    <option value="disabled">{tr("Disabled")}</option>
                  </select>
                </label>
                <label>{tr("Permission mode")}<select value={engineConfig.permissionMode} onChange={(e) => setEngineConfig({ permissionMode: e.target.value as 'default' | 'plan' | 'bypass' | 'strict' })}>
                    <option value="default">{tr("Standard")}</option>
                    <option value="plan">{tr("Plan-Mode")}</option>
                    <option value="bypass">{tr("Bypass (allow everything)")}</option>
                    <option value="strict">{tr("Strict (ask everything)")}</option>
                  </select>
                </label>
              </div>
            </Section>

            <Section title={tr("System prompts")} icon="SP">
              <label style={{ marginTop: 12, display: 'block' }}>{tr("Base system prompt")}<textarea
                  rows={10}
                  value={engineConfig.systemPrompt}
                  onChange={(e) => setEngineConfig({ systemPrompt: e.target.value })}
                  placeholder={tr("Base behavior for the agentic engine...")}
                  style={{ width: '100%', resize: 'vertical', fontFamily: 'monospace' }}
                />
              </label>
              <div style={{ display: 'flex', gap: 8, marginTop: 8, marginBottom: 12 }}>
                <button type="button" className="btn-sm" onClick={() => setEngineConfig({ systemPrompt: DEFAULT_SYSTEM_PROMPT })}>{tr("Reset to default")}</button>
              </div>
              <label style={{ display: 'block', marginBottom: 12 }}>{tr("System prompt extension")}<textarea
                  rows={3}
                  value={engineConfig.appendSystemPrompt}
                  onChange={(e) => setEngineConfig({ appendSystemPrompt: e.target.value })}
                  placeholder={tr("Additional instructions for the agent...")}
                  style={{ width: '100%', resize: 'vertical' }}
                />
              </label>
              <label style={{ display: 'block' }}>{tr("Global cowork instruction")}<textarea
                  rows={4}
                  value={globalInstruction}
                  onChange={(e) => setGlobalInstruction(e.target.value)}
                  placeholder={tr("Project-wide instructions for chat and cowork...")}
                  style={{ width: '100%', resize: 'vertical' }}
                />
              </label>
            </Section>

            <SkillPanel />
            <PipelinePanel />
            <CrewPanel />
          </div>
        )}

        {/* ── Memory ───────────── */}
        {activeCategory === 'memory' && (
          <div className="settings-view">
            <h1>{tr("Memory")}</h1>
            <p className="hint-text">{tr("Manage agent memory, profile, provider, and notes")}</p>
            <MemoryPanel />
          </div>
        )}

        {/* ── Sessions & Insights ──── */}
        {activeCategory === 'sessions' && (
          <div className="settings-view">
            <h1>{tr("Sessions & Insights")}</h1>
            <p className="hint-text">{tr("Search past sessions and review usage statistics")}</p>
            <SessionSearchPanel />
            <InsightsPanel />
            <RunPanel />
          </div>
        )}

        {/* ── Terminal & Processes ──── */}
        {activeCategory === 'terminal' && (
          <div className="settings-view">
            <h1>{tr("Terminal & Processes")}</h1>
            <p className="hint-text">{tr("Configure terminal backends and managed processes")}</p>
            <TerminalPanel />
            <ProcessPanel />
          </div>
        )}

        {/* ── MCP Server ────────────── */}
        {activeCategory === 'mcp' && (
          <div className="settings-view">
            <h1>{tr("MCP Server")}</h1>
            <p className="hint-text">{tr("Manage and test Model Context Protocol servers")}</p>

            <Section title={tr("MCP Settings")} icon="🔌">
              <Toggle label={tr("Auto-reconnect")} hint={tr("Reconnect MCP servers automatically after connection loss")} {...pref('mcpAutoReconnect')} />
              <Toggle label={tr("Verbose logging")} hint={tr("Detailed MCP protocol logging")} {...pref('mcpVerboseLogging')} />
              <Toggle label={tr("Enable environment editor")} hint={tr("Edit environment variables manually")} {...pref('mcpEnvEditorEnabled')} />
              <Toggle label={tr("Manual JSON import")} hint={tr("Add MCP servers through JSON import")} {...pref('mcpAllowManualImport')} />
            </Section>

            <McpView />
          </div>
        )}

        {/* ── Interface ────────────── */}
        {activeCategory === 'ui' && (
          <div className="settings-view">
            <h1>{tr("Interface")}</h1>
            <p className="hint-text">{tr("Customize display, notifications, and audio feedback")}</p>

            <Section title={tr("Appearance")} icon="🎨">
              <Toggle label={tr("Focus mode")} hint={tr("Hide sidebars and distractions")} {...pref('focusMode')} />
              <Toggle label={tr("Compact mode")} hint={tr("Less spacing for more content")} {...pref('compactMode')} />
              <Toggle label={tr("Verbose mode")} hint={tr("Show internal prompts, file context, and tool/MCP diagnostics in chat")} {...pref('verboseMode')} />
              <Toggle label={tr("Limit thinking window")} hint={tr("In verbose mode, show only the latest 50 thinking lines live")} {...pref('limitThinkingWindow')} />
              <Toggle label={tr("Super-verbose audit")} hint={tr("Store user prompts, answers, tool calls, and tool outputs fully in audit/events.jsonl")} {...pref('superVerboseAuditLogging')} />
              <Toggle label={tr("Show timestamps")} hint={tr("Times on chat messages")} {...pref('showTimestamps')} />
              <Toggle label={tr("Enable shortcut overlay")} hint={tr("Keyboard shortcut help via Ctrl+Shift+?")} {...pref('shortcutOverlayEnabled')} />
              <Toggle label={tr("Sync theme with system")} hint={tr("Switch light/dark mode automatically from the OS setting")} {...pref('syncThemeWithSystem')} />
              <div className="grid" style={{ marginTop: 12 }}>
                <label>{tr("Font size (%)")}<input type="number" min={85} max={120} step={5} value={preferences.fontScale} onChange={(e) => setPreference('fontScale', Number(e.target.value))} />
                </label>
                <label>{tr("Start view")}<select value={preferences.defaultStartView} onChange={(e) => setPreference('defaultStartView', e.target.value as StartView)}>
                    <option value="last">{tr("Last view")}</option>
                    <option value="work">{tr("Workspace")}</option>
                    <option value="settings">{tr("Settings")}</option>
                  </select>
                </label>
              </div>
            </Section>

            <Section title={tr("Notifications & sound")} icon="🔔">
              <Toggle label={tr("Desktop notifications")} hint={tr("Windows notifications for important events")} {...pref('notificationsEnabled')} />
              <Toggle label={tr("Enable sounds")} hint={tr("Audio feedback for actions")} {...pref('soundsEnabled')} />
              <Toggle label={tr("Confirm on close")} hint={tr("Ask before exiting the desktop app")} {...pref('confirmOnCloseWithRunningTasks')} />
            </Section>
          </div>
        )}

        {/* ── Security & data ──── */}
        {activeCategory === 'security' && (
          <div className="settings-view">
            <h1>{tr("Security & data")}</h1>
            <p className="hint-text">{tr("Configure file access, command filters, and data retention")}</p>

            <Section title={tr("File security")} icon="🔒">
              <Toggle label={tr("Read-only mode")} hint={tr("No file writes or deletes")} {...pref('readOnlyFsMode')} />
              <div className="grid" style={{ marginTop: 12 }}>
                <label>{tr("Allowed commands (allowlist)")}<textarea rows={3} value={preferences.commandWhitelist} onChange={(e) => setPreference('commandWhitelist', e.target.value)} placeholder={tr("One command per line")} />
                </label>
                <label>{tr("Blocked commands (blacklist)")}<textarea rows={3} value={preferences.commandBlacklist} onChange={(e) => setPreference('commandBlacklist', e.target.value)} placeholder={tr("One command per line")} />
                </label>
              </div>
            </Section>

            <Section title={tr("Data & storage")} icon="💾">
              <Toggle label={tr("Enable telemetry")} hint={tr("Send anonymous usage statistics")} {...pref('telemetryEnabled')} />
              <Toggle label={tr("Automatic DB backup")} hint={tr("Regularly back up the SQLite database")} {...pref('autoBackupDb')} />
              <Toggle label={tr("DB cleanup on startup")} hint={tr("Clean up orphaned entries when the app starts")} {...pref('dbCleanupOnStart')} />
              <div className="grid" style={{ marginTop: 12 }}>
                <label>{tr("Chat retention (days)")}<input type="number" min={1} max={365} value={preferences.chatRetentionDays} onChange={(e) => setPreference('chatRetentionDays', Number(e.target.value))} />
                </label>
                <label>{tr("Backup interval (hours)")}<input type="number" min={1} max={168} value={preferences.dbBackupIntervalHours} onChange={(e) => setPreference('dbBackupIntervalHours', Number(e.target.value))} />
                </label>
              </div>
            </Section>

            <RuntimeInstructionsPanel />
          </div>
        )}

        {/* ── System & Info ────────── */}
        {activeCategory === 'system' && (
          <div className="settings-view">
            <h1>{tr("System & Info")}</h1>
            <p className="hint-text">{tr("Workspace paths, startup, and app information")}</p>

            <Section title={tr("Workspace & System")} icon="📁">
              <Toggle label={tr("Launch at system startup")} hint={tr("Start the app automatically with Windows")} {...pref('launchAtStartup')} />
              <div className="grid" style={{ marginTop: 12 }}>
                <label>{tr("Default workspace path")}<input value={preferences.workspaceDefaultPath} onChange={(e) => setPreference('workspaceDefaultPath', e.target.value)} placeholder={tr("C:\\Projects\\my-workspace")} style={{ fontFamily: 'monospace' }} />
                </label>
              </div>
            </Section>

            <ConnectorPanel />

            <Section title={tr("About Open_Cowork")} icon="✦">
              <div className="card about-open-cowork-card">
                <div className="about-open-cowork-intro">
                  <strong>{tr("Open_Cowork")}</strong>
                  <span>{tr("Local desktop workspace for chat, tools, tasks, and multi-agent runs.")}</span>
                </div>
                <dl className="about-open-cowork-details">
                  <div>
                    <dt>{tr("Creator")}</dt>
                    <dd>noshitcoding</dd>
                  </div>
                  <div>
                    <dt>{tr("Project page")}</dt>
                    <dd>
                      <a href="https://github.com/noshitcoding/Open_Cowork" target="_blank" rel="noreferrer">
                        github.com/noshitcoding/Open_Cowork
                      </a>
                    </dd>
                  </div>
                  <div>
                    <dt>{tr("Runtime")}</dt>
                    <dd>{tr("Tauri desktop app")}</dd>
                  </div>
                  <div>
                    <dt>{tr("Local LLM endpoint")}</dt>
                    <dd>{ollama.baseUrl || tr("Not configured")}</dd>
                  </div>
                  <div>
                    <dt>{tr("Default model")}</dt>
                    <dd>{ollama.model || tr("Not configured")}</dd>
                  </div>
                  <div>
                    <dt>{tr("Workspace")}</dt>
                    <dd>{preferences.workspaceDefaultPath || tr("Last used")}</dd>
                  </div>
                </dl>
                <div className="about-open-cowork-disclaimer">
                  <strong>{tr("Disclaimer")}</strong>
                  <p>{tr("Open_Cowork can execute commands, use tools, and modify local files. Use it at your own risk, review AI output before relying on it, and do not treat responses as legal, medical, financial, or safety-critical advice.")}</p>
                </div>
              </div>
            </Section>
          </div>
        )}
      </div>
    </div>
  )
}
