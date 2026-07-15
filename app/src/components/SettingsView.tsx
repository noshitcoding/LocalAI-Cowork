import { useEffect, useState } from 'react'
import type { KeyboardEvent } from 'react'
import { useTranslation } from 'react-i18next'
import { Link, useSearchParams } from 'react-router-dom'
import { save } from '@tauri-apps/plugin-dialog'
import {
  Bell,
  Bot,
  Brain,
  CheckCircle2,
  Database,
  Download,
  FileText,
  Folder,
  FolderOpen,
  HardDrive,
  Info,
  LockKeyhole,
  Palette,
  PlugZap,
  Save,
  Search,
  Settings2,
  ShieldCheck,
  SquareTerminal,
  Zap,
} from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
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
import ConnectorPanel from './ConnectorPanel'
import McpView from './McpView'
import RunPanel from './RunPanel'
import RuntimeInstructionsPanel from './RuntimeInstructionsPanel'
import LlmProfilesPanel from './LlmProfilesPanel'
import { tr } from '../i18n'
import { safeInvoke } from '../utils/safeInvoke'

/* Tiny reusable primitives (App.css based) */

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

function Section({ title, icon: Icon, children }: { title: string; icon: LucideIcon; children: React.ReactNode }) {
  return (
    <div className="panel">
      <h2 className="settings-section-title">
        <Icon className="settings-section-icon" size={18} strokeWidth={1.8} aria-hidden="true" />
        <span>{title}</span>
      </h2>
      {children}
    </div>
  )
}

type GatewaySubsystem = {
  id: string
  label: string
  category: string
  status: 'ok' | 'degraded' | 'failed' | 'unavailable' | 'unknown' | string
  message: string
  checkedAt: string
  detailJson?: string | null
}

type GatewayHealth = {
  status: string
  checkedAt: string
  subsystems: GatewaySubsystem[]
}

type RuntimeProviderMapping = {
  inputUrl: string
  mappedUrl: string
  runtimeMode: string
  changed: boolean
  reason: string
}

type SupportBundleResponse = {
  path: string
  sizeBytes: number
  createdAt: string
  fileCount: number
}

type StartupRecoveryReport = {
  recoveredAt: string
  engineRuns: number
  legacyTasks: number
  taskSteps: number
  workTasks: number
  scheduledRuns: number
  crewRuns: number
  workerSandboxes: number
  managedProcesses: number
  terminalBackends: number
}

const EMPTY_GATEWAY_HEALTH: GatewayHealth = {
  status: 'unknown',
  checkedAt: '',
  subsystems: [],
}

const EMPTY_STARTUP_RECOVERY: StartupRecoveryReport = {
  recoveredAt: '',
  engineRuns: 0,
  legacyTasks: 0,
  taskSteps: 0,
  workTasks: 0,
  scheduledRuns: 0,
  crewRuns: 0,
  workerSandboxes: 0,
  managedProcesses: 0,
  terminalBackends: 0,
}

function GatewayDiagnosticsPanel() {
  const ollama = useConfigStore((s) => s.ollama)
  const [health, setHealth] = useState<GatewayHealth>(EMPTY_GATEWAY_HEALTH)
  const [recovery, setRecovery] = useState<StartupRecoveryReport>(EMPTY_STARTUP_RECOVERY)
  const [loading, setLoading] = useState(false)
  const [mappingUrl, setMappingUrl] = useState(ollama.baseUrl || 'http://127.0.0.1:11434')
  const [mappingMode, setMappingMode] = useState('isolated')
  const [mapping, setMapping] = useState<RuntimeProviderMapping | null>(null)

  const refreshGateway = async (includeProviderProbe = false) => {
    setLoading(true)
    try {
      const request = includeProviderProbe
        ? {
            includeProviderProbe: true,
            providerKind: 'ollama',
            baseUrl: ollama.baseUrl,
            model: ollama.model,
            verifyTlsCertificates: true,
          }
        : { includeProviderProbe: false }
      const [snapshot, recoverySnapshot] = await Promise.all([
        safeInvoke<GatewayHealth>('gateway_health', { request }, EMPTY_GATEWAY_HEALTH),
        safeInvoke<StartupRecoveryReport | null>('startup_recovery_status', undefined, null),
      ])
      setHealth(snapshot ?? EMPTY_GATEWAY_HEALTH)
      setRecovery(recoverySnapshot ?? EMPTY_STARTUP_RECOVERY)
    } finally {
      setLoading(false)
    }
  }

  const resolveMapping = async () => {
    const result = await safeInvoke<RuntimeProviderMapping | null>('runtime_provider_mapping_resolve', {
      request: {
        baseUrl: mappingUrl,
        runtimeMode: mappingMode,
      },
    }, null)
    setMapping(result)
  }

  useEffect(() => {
    void refreshGateway(false)
    // Initial gateway snapshot only; manual buttons trigger later probes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const recoveredStates = Object.entries(recovery)
    .filter(([key]) => key !== 'recoveredAt')
    .reduce((total, [, value]) => total + Number(value), 0)

  return (
    <Section title={tr("Gateway diagnostics")} icon={Info}>
      <div className="grid settings-grid-bottom-space">
        <label>{tr("Gateway status")}<input readOnly value={health.status} />
        </label>
        <label>{tr("Checked at")}<input readOnly value={health.checkedAt ? new Date(health.checkedAt).toLocaleString() : tr("Not checked")} />
        </label>
        <label>{tr("Recovered startup states")}<input readOnly value={recoveredStates} />
        </label>
      </div>
      <div className="actions">
        <button type="button" className="btn-sm" onClick={() => void refreshGateway(false)} disabled={loading}>
          {loading ? tr("Checking...") : tr("Refresh local status")}
        </button>
        <button type="button" className="btn-sm" onClick={() => void refreshGateway(true)} disabled={loading}>
          {tr("Probe provider")}
        </button>
      </div>

      <div className="tool-list settings-grid-spaced">
        {health.subsystems.length === 0 ? (
          <p className="hint-text">{tr("Gateway status is not available outside the Tauri runtime.")}</p>
        ) : health.subsystems.map((subsystem) => (
          <div key={subsystem.id} className="tool-item">
            <div className="tool-item-action-row">
              <strong>{tr(subsystem.label)}</strong>
              <span className={`status-badge ${subsystem.status === 'ok' ? 'online' : 'offline'}`}>
                {subsystem.status}
              </span>
            </div>
            <small className="hint-text">{tr(subsystem.category)} - {subsystem.message}</small>
          </div>
        ))}
      </div>

      <div className="grid settings-grid-spaced">
        <label>{tr("Provider URL")}<input value={mappingUrl} onChange={(event) => setMappingUrl(event.target.value)} placeholder="http://127.0.0.1:11434" />
        </label>
        <label>{tr("Runtime mode")}<select value={mappingMode} onChange={(event) => setMappingMode(event.target.value)}>
            <option value="host">{tr("Host")}</option>
            <option value="isolated">{tr("Isolated")}</option>
            <option value="docker">{tr("Docker")}</option>
            <option value="workspace_copy">{tr("Workspace copy")}</option>
          </select>
        </label>
      </div>
      <div className="actions">
        <button type="button" className="btn-sm" onClick={() => void resolveMapping()}>{tr("Resolve runtime URL")}</button>
      </div>
      {mapping && (
        <div className="tool-result">
          <strong>{mapping.changed ? tr("Mapped URL") : tr("Unchanged URL")}</strong>
          <pre>{mapping.mappedUrl}</pre>
          <small className="hint-text">{mapping.reason}</small>
        </div>
      )}
    </Section>
  )
}

function SupportBundlePanel() {
  const [exporting, setExporting] = useState(false)
  const [status, setStatus] = useState<'idle' | 'saved' | 'failed'>('idle')

  const createBundle = async () => {
    setStatus('idle')
    setExporting(true)
    try {
      const date = new Date().toISOString().slice(0, 10)
      const path = await save({
        defaultPath: `open-cowork-support-${date}.zip`,
        filters: [{ name: 'ZIP', extensions: ['zip'] }],
      })
      if (!path) return
      const result = await safeInvoke<SupportBundleResponse | null>('support_bundle_create', { path }, null)
      setStatus(result ? 'saved' : 'failed')
    } catch {
      setStatus('failed')
    } finally {
      setExporting(false)
    }
  }

  return (
    <Section title={tr("Support diagnostics")} icon={HardDrive}>
      <div className="actions">
        <button type="button" className="btn-sm btn-secondary support-bundle-button" onClick={() => void createBundle()} disabled={exporting}>
          <Download size={15} aria-hidden="true" />
          {exporting ? tr("Creating support bundle...") : tr("Create support bundle")}
        </button>
      </div>
      {status === 'saved' && <p className="hint-text" role="status">{tr("Support bundle saved.")}</p>}
      {status === 'failed' && <p className="hint-text" role="alert">{tr("Support bundle export failed.")}</p>}
    </Section>
  )
}

/* Category definitions */

const CATEGORIES = [
  { key: 'ai', label: 'AI & model', description: 'Configure multiple LLM profiles, global provider defaults, and personalities', icon: Bot },
  { key: 'agent', label: 'Agent & Skills', description: 'Control agent behavior, manage skills, and configure pipelines', icon: Zap },
  { key: 'memory', label: 'Memory', description: 'Manage agent memory, profile, provider, and notes', icon: Brain },
  { key: 'sessions', label: 'Sessions & Insights', description: 'Search past sessions and review usage statistics', icon: FolderOpen },
  { key: 'terminal', label: 'Terminal & Processes', description: 'Configure terminal backends and managed processes', icon: SquareTerminal },
  { key: 'mcp', label: 'MCP Server', description: 'Manage and test Model Context Protocol servers', icon: PlugZap },
  { key: 'ui', label: 'Interface', description: 'Customize display, notifications, and audio feedback', icon: Palette },
  { key: 'security', label: 'Security & data', description: 'Configure file access, command filters, and data retention', icon: ShieldCheck },
  { key: 'system', label: 'System & Info', description: 'Workspace paths, startup, and app information', icon: Folder },
] as const

type CategoryKey = (typeof CATEGORIES)[number]['key']

const isCategoryKey = (value: string | null): value is CategoryKey =>
  CATEGORIES.some((category) => category.key === value)

const getTabId = (key: CategoryKey) => `settings-tab-${key}`
const getPanelId = (key: CategoryKey) => `settings-panel-${key}`
const getPanelProps = (key: CategoryKey) => ({
  id: getPanelId(key),
  role: 'tabpanel' as const,
  'aria-labelledby': getTabId(key),
})

function SettingsPageHeader({ category }: { category: CategoryKey }) {
  const config = CATEGORIES.find((item) => item.key === category) ?? CATEGORIES[0]
  const Icon = config.icon

  return (
    <header className="settings-page-header">
      <span className="settings-page-icon" aria-hidden="true"><Icon size={20} strokeWidth={1.9} /></span>
      <div className="settings-page-heading">
        <span className="settings-page-kicker">{tr('Workspace preferences')}</span>
        <h1>{tr(config.label)}</h1>
        <p>{tr(config.description)}</p>
      </div>
      <span className="settings-save-state"><CheckCircle2 size={15} aria-hidden="true" />{tr('Saved automatically')}</span>
    </header>
  )
}

/* Main component */

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
  const policyFlags = useCoworkStore((s) => s.policyFlags)
  const setPolicyFlag = useCoworkStore((s) => s.setPolicyFlag)
  const claudeTools = useCoworkStore((s) => s.claudeTools)
  const enabledClaudeToolIds = useCoworkStore((s) => s.enabledClaudeToolIds)
  const toggleClaudeTool = useCoworkStore((s) => s.toggleClaudeTool)
  const activeToolsetPolicyId = useCoworkStore((s) => s.activeToolsetPolicyId)
  const toolsetPolicies = useCoworkStore((s) => s.toolsetPolicies)
  const setActiveToolsetPolicy = useCoworkStore((s) => s.setActiveToolsetPolicy)
  const [searchParams, setSearchParams] = useSearchParams()
  const categoryParam = searchParams.get('section')
  const activeCategory: CategoryKey = isCategoryKey(categoryParam) ? categoryParam : 'ai'
  const activeToolsetPolicy = toolsetPolicies.find((policy) => policy.id === activeToolsetPolicyId)
  const [categorySearch, setCategorySearch] = useState('')
  const normalizedCategorySearch = categorySearch.trim().toLocaleLowerCase()
  const getVisibleCategories = (search: string) => {
    const normalizedSearch = search.trim().toLocaleLowerCase()
    return CATEGORIES.filter((category) => (
      !normalizedSearch
      || tr(category.label).toLocaleLowerCase().includes(normalizedSearch)
      || tr(category.description).toLocaleLowerCase().includes(normalizedSearch)
    ))
  }
  const visibleCategories = getVisibleCategories(categorySearch)

  const setActiveCategory = (category: CategoryKey) => {
    const nextParams = new URLSearchParams(searchParams)
    if (category === 'ai') {
      nextParams.delete('section')
    } else {
      nextParams.set('section', category)
    }
    setSearchParams(nextParams)
  }

  const handleCategorySearchChange = (value: string) => {
    setCategorySearch(value)
    const matches = getVisibleCategories(value)
    if (matches.length > 0 && !matches.some((category) => category.key === activeCategory)) {
      setActiveCategory(matches[0].key)
    }
  }

  const handleCategoryKeyDown = (event: KeyboardEvent<HTMLButtonElement>, category: CategoryKey) => {
    const currentIndex = visibleCategories.findIndex((item) => item.key === category)
    if (currentIndex < 0 || visibleCategories.length === 0) return

    let nextIndex = currentIndex
    if (event.key === 'ArrowDown' || event.key === 'ArrowRight') nextIndex = (currentIndex + 1) % visibleCategories.length
    else if (event.key === 'ArrowUp' || event.key === 'ArrowLeft') nextIndex = (currentIndex - 1 + visibleCategories.length) % visibleCategories.length
    else if (event.key === 'Home') nextIndex = 0
    else if (event.key === 'End') nextIndex = visibleCategories.length - 1
    else return

    event.preventDefault()
    const nextCategory = visibleCategories[nextIndex].key
    setActiveCategory(nextCategory)
    window.requestAnimationFrame(() => document.getElementById(getTabId(nextCategory))?.focus())
  }

  const pref = <K extends keyof AppPreferences>(key: K) => ({
    checked: preferences[key] as boolean,
    onChange: (v: boolean) => setPreference(key, v as AppPreferences[K]),
  })

  return (
    <div className="settings-layout">
      {/* Sidebar navigation */}
      <aside className="settings-sidebar">
        <label className="settings-category-search">
          <Search size={15} aria-hidden="true" />
          <input
            type="search"
            value={categorySearch}
            onChange={(event) => handleCategorySearchChange(event.currentTarget.value)}
            aria-label={tr('Search settings')}
            placeholder={tr('Search settings')}
          />
          {normalizedCategorySearch ? <span>{visibleCategories.length}</span> : null}
        </label>
        <nav className="settings-nav-list" role="tablist" aria-label={tr("Settings categories")}>
          {visibleCategories.map((cat) => {
            const Icon = cat.icon
            return (
              <button
                key={cat.key}
                id={getTabId(cat.key)}
                type="button"
                role="tab"
                tabIndex={activeCategory === cat.key ? 0 : -1}
                className={`settings-nav-item${activeCategory === cat.key ? ' active' : ''}`}
                aria-selected={activeCategory === cat.key}
                aria-controls={getPanelId(cat.key)}
                onClick={() => setActiveCategory(cat.key)}
                onKeyDown={(event) => handleCategoryKeyDown(event, cat.key)}
              >
                <Icon className="settings-nav-icon" size={16} strokeWidth={1.8} aria-hidden="true" />
                <span className="settings-nav-label">{tr(cat.label)}</span>
              </button>
            )
          })}
          {visibleCategories.length === 0 ? (
            <p className="settings-category-empty" role="status">{tr('No settings sections match your search')}</p>
          ) : null}
        </nav>
      </aside>

      {/* Content area */}
      <div className="settings-content">
        {/* AI and model */}
        {activeCategory === 'ai' && (
          <div className="settings-view" {...getPanelProps('ai')}>
            <SettingsPageHeader category="ai" />

            <LlmProfilesPanel />

            <Section title={tr("Streaming")} icon={Save}>
              <Toggle label={tr("Automatically save stream answers")} hint={tr("Ollama answers are saved during streaming")} {...pref('ollamaStreamAutosave')} />
            </Section>
            <PersonalitySelector />
          </div>
        )}

        {/* Agent and skills */}
        {activeCategory === 'agent' && (
          <div className="settings-view settings-view-wide" {...getPanelProps('agent')}>
            <SettingsPageHeader category="agent" />

            <Section title={tr("Agent behavior")} icon={Zap}>
              <Toggle label={tr("Automatically approve safe tools")} hint={tr("Execute read operations without confirmation")} {...pref('autoApproveSafeTools')} />
              <Toggle label={tr("Autopilot for all tools")} hint={tr("Approve all tool calls automatically (caution!)")} {...pref('autoPilotAllTools')} />
              <Toggle label={tr("Fallback to a human after repeated errors")} hint={tr("Agent stops after repeated failed attempts")} {...pref('fallbackToHumanOnRepeatedFailure')} />
              <Toggle label={tr("Batch multi-select for tasks")} hint={tr("Select and edit multiple tasks at once")} {...pref('taskBatchMultiSelectEnabled')} />
              <div className="grid settings-grid-spaced">
                <label>{tr("Max tool calls per loop")}<input type="number" min={1} max={50} value={preferences.maxToolCallsPerLoop} onChange={(e) => setPreference('maxToolCallsPerLoop', Number(e.target.value))} />
                </label>
              </div>
            </Section>

            <Section title={tr("Engine configuration")} icon={Settings2}>
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

            <Section title={tr("System prompts")} icon={FileText}>
              <label className="settings-block-field spaced">{tr("Base system prompt")}<textarea
                  rows={10}
                  value={engineConfig.systemPrompt}
                  onChange={(e) => setEngineConfig({ systemPrompt: e.target.value })}
                  placeholder={tr("Base behavior for the agentic engine...")}
                  className="settings-resize-textarea settings-mono-textarea"
                />
              </label>
              <div className="settings-inline-actions">
                <button type="button" className="btn-sm" onClick={() => setEngineConfig({ systemPrompt: DEFAULT_SYSTEM_PROMPT })}>{tr("Reset to default")}</button>
              </div>
              <label className="settings-block-field with-bottom-space">{tr("System prompt extension")}<textarea
                  rows={3}
                  value={engineConfig.appendSystemPrompt}
                  onChange={(e) => setEngineConfig({ appendSystemPrompt: e.target.value })}
                  placeholder={tr("Additional instructions for the agent...")}
                  className="settings-resize-textarea"
                />
              </label>
              <label className="settings-block-field">{tr("Global cowork instruction")}<textarea
                  rows={4}
                  value={globalInstruction}
                  onChange={(e) => setGlobalInstruction(e.target.value)}
                  placeholder={tr("Project-wide instructions for chat and cowork...")}
                  className="settings-resize-textarea"
                />
              </label>
            </Section>

            <SkillPanel />
            <PipelinePanel />
            <Section title={tr("Crew configuration")} icon={Bot}>
              <p className="hint-text settings-policy-description">
                {tr("Crew setup now lives in the dedicated crew workspace. Use it to manage crew members, tasks, and run settings in one place.")}
              </p>
              <div className="settings-inline-actions">
                <Link className="btn-sm" to="/crew">{tr("Open crew workspace")}</Link>
              </div>
            </Section>
          </div>
        )}

        {/* Memory */}
        {activeCategory === 'memory' && (
          <div className="settings-view" {...getPanelProps('memory')}>
            <SettingsPageHeader category="memory" />
            <MemoryPanel />
          </div>
        )}

        {/* Sessions and insights */}
        {activeCategory === 'sessions' && (
          <div className="settings-view" {...getPanelProps('sessions')}>
            <SettingsPageHeader category="sessions" />
            <SessionSearchPanel />
            <InsightsPanel />
            <RunPanel />
          </div>
        )}

        {/* Terminal and processes */}
        {activeCategory === 'terminal' && (
          <div className="settings-view" {...getPanelProps('terminal')}>
            <SettingsPageHeader category="terminal" />
            <TerminalPanel />
            <ProcessPanel />
          </div>
        )}

        {/* MCP server */}
        {activeCategory === 'mcp' && (
          <div className="settings-view" {...getPanelProps('mcp')}>
            <SettingsPageHeader category="mcp" />

            <Section title={tr("MCP Settings")} icon={PlugZap}>
              <Toggle label={tr("Auto-reconnect")} hint={tr("Reconnect MCP servers automatically after connection loss")} {...pref('mcpAutoReconnect')} />
              <Toggle label={tr("Verbose logging")} hint={tr("Detailed MCP protocol logging")} {...pref('mcpVerboseLogging')} />
              <Toggle label={tr("Enable environment editor")} hint={tr("Edit environment variables manually")} {...pref('mcpEnvEditorEnabled')} />
              <Toggle label={tr("Manual JSON import")} hint={tr("Add MCP servers through JSON import")} {...pref('mcpAllowManualImport')} />
            </Section>

            <McpView />
          </div>
        )}

        {/* Interface */}
        {activeCategory === 'ui' && (
          <div className="settings-view" {...getPanelProps('ui')}>
            <SettingsPageHeader category="ui" />

            <Section title={tr("Appearance")} icon={Palette}>
              <Toggle label={tr("Focus mode")} hint={tr("Hide sidebars and distractions")} {...pref('focusMode')} />
              <Toggle label={tr("Compact mode")} hint={tr("Less spacing for more content")} {...pref('compactMode')} />
              <Toggle label={tr("Verbose mode")} hint={tr("Show internal prompts, file context, and tool/MCP diagnostics in chat")} {...pref('verboseMode')} />
              <Toggle label={tr("Limit thinking window")} hint={tr("In verbose mode, show only the latest 50 thinking lines live")} {...pref('limitThinkingWindow')} />
              <Toggle label={tr("Super-verbose audit")} hint={tr("Store user prompts, answers, tool calls, and tool outputs fully in audit/events.jsonl")} {...pref('superVerboseAuditLogging')} />
              <Toggle label={tr("Show timestamps")} hint={tr("Times on chat messages")} {...pref('showTimestamps')} />
              <Toggle label={tr("Enable shortcut overlay")} hint={tr("Keyboard shortcut help via Ctrl+Shift+?")} {...pref('shortcutOverlayEnabled')} />
              <Toggle label={tr("Sync theme with system")} hint={tr("Switch light/dark mode automatically from the OS setting")} {...pref('syncThemeWithSystem')} />
              <div className="grid settings-grid-spaced">
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

            <Section title={tr("Notifications & sound")} icon={Bell}>
              <Toggle label={tr("Desktop notifications")} hint={tr("Windows notifications for important events")} {...pref('notificationsEnabled')} />
              <Toggle label={tr("Enable sounds")} hint={tr("Audio feedback for actions")} {...pref('soundsEnabled')} />
              <Toggle label={tr("Confirm on close")} hint={tr("Ask before exiting the desktop app")} {...pref('confirmOnCloseWithRunningTasks')} />
            </Section>
          </div>
        )}

        {/* Security and data */}
        {activeCategory === 'security' && (
          <div className="settings-view" {...getPanelProps('security')}>
            <SettingsPageHeader category="security" />

            <Section title={tr("File security")} icon={LockKeyhole}>
              <Toggle label={tr("Read-only mode")} hint={tr("No file writes or deletes")} {...pref('readOnlyFsMode')} />
              <div className="grid settings-command-grid">
                <label>{tr("Allowed commands (allowlist)")}<textarea className="settings-command-textarea" rows={6} value={preferences.commandWhitelist} onChange={(e) => setPreference('commandWhitelist', e.target.value)} placeholder={tr("One command per line")} />
                </label>
                <label>{tr("Blocked commands (blacklist)")}<textarea className="settings-command-textarea" rows={6} value={preferences.commandBlacklist} onChange={(e) => setPreference('commandBlacklist', e.target.value)} placeholder={tr("One command per line")} />
                </label>
              </div>
            </Section>

            <Section title={tr("Toolset policy")} icon={ShieldCheck}>
              <div className="grid settings-grid-bottom-space">
                <label>{tr("Active toolset")}<select value={activeToolsetPolicyId} onChange={(e) => setActiveToolsetPolicy(e.target.value)}>
                    <option value="custom">{tr("Custom")}</option>
                    {toolsetPolicies.map((policy) => (
                      <option key={policy.id} value={policy.id}>
                        {policy.label} ({policy.riskLevel})
                      </option>
                    ))}
                  </select>
                </label>
                <label>{tr("Enabled tools")}<input readOnly value={`${enabledClaudeToolIds.length} / ${claudeTools.length}`} />
                </label>
              </div>
              <p className="hint-text settings-policy-description">
                {activeToolsetPolicy
                  ? tr(activeToolsetPolicy.description)
                  : tr("Manual tool selection. Changes are persisted as a custom toolset policy.")}
              </p>

              <div className="grid settings-grid-spaced">
                <Toggle
                  label={tr("Strict policy enforcement")}
                  hint={tr("Disabled tools and deny rules block execution")}
                  checked={policyFlags.strictPolicyEnforcement}
                  onChange={(value) => setPolicyFlag('strictPolicyEnforcement', value)}
                />
                <Toggle
                  label={tr("Allow shell execution")}
                  hint={tr("Shell tools still pass command guards and workspace limits")}
                  checked={policyFlags.allowShellExecution}
                  onChange={(value) => setPolicyFlag('allowShellExecution', value)}
                />
                <Toggle
                  label={tr("Allow MCP tool calls")}
                  hint={tr("Connector and MCP tools may execute when enabled in the active toolset")}
                  checked={policyFlags.allowMcpToolCalls}
                  onChange={(value) => setPolicyFlag('allowMcpToolCalls', value)}
                />
                <Toggle
                  label={tr("Allow web tools")}
                  hint={tr("Applies to Web Fetch and Web Search")}
                  checked={policyFlags.allowWebFetch && policyFlags.allowWebSearch}
                  onChange={(value) => {
                    setPolicyFlag('allowWebFetch', value)
                    setPolicyFlag('allowWebSearch', value)
                  }}
                />
              </div>

              <div className="settings-toolset-grid">
                {claudeTools.map((tool) => (
                  <label key={tool.id} className="crew-checkbox-label" title={tool.description}>
                    <input
                      type="checkbox"
                      checked={enabledClaudeToolIds.includes(tool.id)}
                      onChange={(event) => toggleClaudeTool(tool.id, event.currentTarget.checked)}
                    />
                    <span>{tr(tool.label)}</span>
                  </label>
                ))}
              </div>
            </Section>

            <Section title={tr("Data & storage")} icon={Database}>
              <Toggle label={tr("Enable telemetry")} hint={tr("Send anonymous usage statistics")} {...pref('telemetryEnabled')} />
              <Toggle label={tr("Automatic DB backup")} hint={tr("Regularly back up the SQLite database")} {...pref('autoBackupDb')} />
              <Toggle label={tr("DB cleanup on startup")} hint={tr("Clean up orphaned entries when the app starts")} {...pref('dbCleanupOnStart')} />
              <div className="grid settings-grid-spaced">
                <label>{tr("Chat retention (days)")}<input type="number" min={1} max={365} value={preferences.chatRetentionDays} onChange={(e) => setPreference('chatRetentionDays', Number(e.target.value))} />
                </label>
                <label>{tr("Backup interval (hours)")}<input type="number" min={1} max={168} value={preferences.dbBackupIntervalHours} onChange={(e) => setPreference('dbBackupIntervalHours', Number(e.target.value))} />
                </label>
              </div>
            </Section>

            <RuntimeInstructionsPanel />
          </div>
        )}

        {/* System and info */}
        {activeCategory === 'system' && (
          <div className="settings-view" {...getPanelProps('system')}>
            <SettingsPageHeader category="system" />

            <Section title={tr("Workspace & System")} icon={HardDrive}>
              <Toggle label={tr("Launch at system startup")} hint={tr("Start the app automatically with Windows")} {...pref('launchAtStartup')} />
              <div className="grid settings-grid-spaced">
                <label>{tr("Default workspace path")}<input className="settings-mono-input" value={preferences.workspaceDefaultPath} onChange={(e) => setPreference('workspaceDefaultPath', e.target.value)} placeholder={tr("C:\\Projects\\my-workspace")} />
                </label>
              </div>
            </Section>

            <GatewayDiagnosticsPanel />

            <SupportBundlePanel />

            <ConnectorPanel />

            <Section title={tr("About Open_Cowork")} icon={Info}>
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
