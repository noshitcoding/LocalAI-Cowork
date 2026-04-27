import { create } from 'zustand'
import { persist } from 'zustand/middleware'
import { safeInvoke } from '../utils/safeInvoke'
import { parseBackendDate } from '../utils/schedulerUtils'

export type FolderInstruction = {
  id: string
  folderPath: string
  instruction: string
}

export type ConnectorKey = 'chrome' | 'slack' | 'drive' | 'webhook'

export type ConnectorTestStatus = 'idle' | 'testing' | 'success' | 'error'

export type ConnectorConfig = {
  key: ConnectorKey
  label: string
  enabled: boolean
  note: string
  apiKey?: string
  webhookUrl?: string
  lastTestStatus?: ConnectorTestStatus
  lastTestMessage?: string
  lastTestAt?: number | null
}

export type PluginSkill = {
  id: string
  name: string
  command: string
  description: string
  promptTemplate: string
  runMode: 'plan' | 'execute'
}

export type Plugin = {
  id: string
  name: string
  domain: 'marketing' | 'sales' | 'finance' | 'custom'
  enabled: boolean
  skills: PluginSkill[]
}

export type ScheduledTask = {
  id: string
  name: string
  prompt: string
  cronLike: string
  active: boolean
  lastRunAt: number | null
  nextRunAt: number | null
}

export type ScheduledTaskRun = {
  id: string
  taskId: string
  status: string
  startedAt: number
  finishedAt: number | null
  result: string | null
  error: string | null
}

export type ClaudePermissionMode =
  | 'default'
  | 'acceptEdits'
  | 'bypassPermissions'
  | 'dontAsk'
  | 'plan'

export type ClaudeToolPreset = 'default' | 'safe' | 'extended'

export type ClaudeToolCapability = {
  id: string
  label: string
  description: string
}

export type CoworkPolicyFlags = {
  strictPolicyEnforcement: boolean
  allowToolDispatcher: boolean
  allowMcpToolCalls: boolean
  allowShellExecution: boolean
  allowWebFetch: boolean
  allowWebSearch: boolean
  allowFileReadExtraction: boolean
  autoCompactLongContext: boolean
}

type PolicySyncRequest = {
  flags: CoworkPolicyFlags
  denyRules: string[]
}

type BackendConnectorTestResponse = {
  reachable: boolean
  status: number | null
  message: string
  checkedAt: string
}

type CoworkState = {
  globalInstruction: string
  folderInstructions: FolderInstruction[]
  connectors: ConnectorConfig[]
  plugins: Plugin[]
  scheduledTasks: ScheduledTask[]
  scheduledRuns: ScheduledTaskRun[]
  claudePlanMode: boolean
  claudePermissionMode: ClaudePermissionMode
  claudeToolPreset: ClaudeToolPreset
  claudeTools: ClaudeToolCapability[]
  enabledClaudeToolIds: string[]
  toolDenyRules: string[]
  policyFlags: CoworkPolicyFlags
  setGlobalInstruction: (instruction: string) => void
  upsertFolderInstruction: (item: FolderInstruction) => void
  removeFolderInstruction: (id: string) => void
  toggleConnector: (key: ConnectorKey, enabled: boolean) => void
  setConnectorNote: (key: ConnectorKey, note: string) => void
  updateConnectorConfig: (key: ConnectorKey, patch: Partial<ConnectorConfig>) => void
  testConnector: (key: ConnectorKey) => Promise<void>
  upsertPlugin: (plugin: Plugin) => void
  togglePlugin: (id: string, enabled: boolean) => void
  removePlugin: (id: string) => void
  installPluginExamples: () => void
  loadScheduledTasks: () => Promise<void>
  loadScheduledRuns: (limit?: number) => Promise<void>
  upsertScheduledTask: (task: ScheduledTask) => Promise<void>
  toggleScheduledTask: (id: string, active: boolean) => Promise<void>
  markScheduledTaskRun: (id: string, at: number) => Promise<void>
  runScheduledTaskNow: (id: string) => Promise<void>
  removeScheduledTask: (id: string) => Promise<void>
  setClaudePlanMode: (enabled: boolean) => void
  setClaudePermissionMode: (mode: ClaudePermissionMode) => void
  setClaudeToolPreset: (preset: ClaudeToolPreset) => void
  toggleClaudeTool: (toolId: string, enabled: boolean) => void
  addToolDenyRule: (rule: string) => void
  removeToolDenyRule: (rule: string) => void
  setPolicyFlag: <K extends keyof CoworkPolicyFlags>(key: K, value: CoworkPolicyFlags[K]) => void
  setPolicySnapshot: (flags: Partial<CoworkPolicyFlags>, denyRules: string[]) => void
}

const DEFAULT_CONNECTORS: ConnectorConfig[] = [
  { key: 'chrome', label: 'Claude in Chrome', enabled: false, note: '', lastTestStatus: 'idle', lastTestAt: null },
  { key: 'slack', label: 'Slack', enabled: false, note: '', webhookUrl: '', apiKey: '', lastTestStatus: 'idle', lastTestAt: null },
  { key: 'drive', label: 'Google Drive', enabled: false, note: '', webhookUrl: '', apiKey: '', lastTestStatus: 'idle', lastTestAt: null },
  { key: 'webhook', label: 'Custom Webhook', enabled: false, note: '', webhookUrl: '', apiKey: '', lastTestStatus: 'idle', lastTestAt: null },
]

const CLAUDE_TOOL_CAPABILITIES: ClaudeToolCapability[] = [
  { id: 'bash', label: 'Bash / PowerShell', description: 'Shell-Befehle ausfuehren' },
  { id: 'read_file', label: 'Dateien lesen', description: 'Workspace-Dateien einlesen' },
  { id: 'edit_file', label: 'Dateien bearbeiten', description: 'Dateien gezielt aendern' },
  { id: 'create_directory', label: 'Ordner erstellen', description: 'Verzeichnisse sicher anlegen' },
  { id: 'move_path', label: 'Dateien verschieben', description: 'Dateien und Ordner verschieben/umbenennen' },
  { id: 'copy_path', label: 'Dateien kopieren', description: 'Dateien und Ordner kopieren' },
  { id: 'glob', label: 'Dateisuche', description: 'Dateien per Pattern finden' },
  { id: 'grep', label: 'Textsuche', description: 'Regex-/String-Suche ueber Dateien' },
  { id: 'web_fetch', label: 'Web Fetch', description: 'Inhalte einer URL laden' },
  { id: 'web_search', label: 'Web Search', description: 'Web-Suche ueber Suchanfragen' },
  { id: 'todo', label: 'Task/Todo', description: 'Todo-Liste und Arbeitsplan pflegen' },
  { id: 'ask_user', label: 'Rueckfragen', description: 'Gezielte Rueckfragen fuer Klarheit' },
  { id: 'mcp', label: 'MCP Tools', description: 'MCP-Server und Tools nutzen' },
]

const DEFAULT_ENABLED_CLAUDE_TOOLS = CLAUDE_TOOL_CAPABILITIES.map((tool) => tool.id)

const TOOL_PRESET_MAP: Record<ClaudeToolPreset, string[]> = {
  default: DEFAULT_ENABLED_CLAUDE_TOOLS,
  safe: ['read_file', 'glob', 'grep', 'todo', 'ask_user', 'web_fetch', 'web_search'],
  extended: DEFAULT_ENABLED_CLAUDE_TOOLS,
}

const DEFAULT_POLICY_FLAGS: CoworkPolicyFlags = {
  strictPolicyEnforcement: true,
  allowToolDispatcher: true,
  allowMcpToolCalls: true,
  allowShellExecution: true,
  allowWebFetch: true,
  allowWebSearch: true,
  allowFileReadExtraction: true,
  autoCompactLongContext: true,
}

let policySyncReady = false
let suppressPolicySync = false
let lastSyncedPolicyKey: string | null = null
let queuedPolicySync: PolicySyncRequest | null = null
let queuedPolicySyncKey: string | null = null
let policySyncInFlight = false

function hasTauriRuntime(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}

type BackendScheduledTaskRow = {
  id: string
  name: string
  prompt: string
  scheduleExpr: string
  active: boolean
  lastRunAt: string | null
  nextRunAt: string | null
}

type BackendScheduledRunRow = {
  id: string
  taskId: string
  status: string
  startedAt: string
  finishedAt: string | null
  result: string | null
  error: string | null
}

function mapScheduledTaskRow(row: BackendScheduledTaskRow): ScheduledTask {
  return {
    id: row.id,
    name: row.name,
    prompt: row.prompt,
    cronLike: row.scheduleExpr,
    active: row.active,
    lastRunAt: parseBackendDate(row.lastRunAt),
    nextRunAt: parseBackendDate(row.nextRunAt),
  }
}

function mapScheduledRunRow(row: BackendScheduledRunRow): ScheduledTaskRun {
  return {
    id: row.id,
    taskId: row.taskId,
    status: row.status,
    startedAt: parseBackendDate(row.startedAt) ?? Date.now(),
    finishedAt: parseBackendDate(row.finishedAt),
    result: row.result,
    error: row.error,
  }
}

function normalizePolicyDenyRules(denyRules: string[]): string[] {
  return denyRules
    .map((rule) => rule.trim())
    .filter((rule, index, arr) => rule.length > 0 && arr.indexOf(rule) === index)
    .slice(0, 80)
}

function buildPolicySyncRequest(
  policyFlags: CoworkPolicyFlags,
  toolDenyRules: string[]
): PolicySyncRequest {
  return {
    flags: policyFlags,
    denyRules: normalizePolicyDenyRules(toolDenyRules),
  }
}

function getPolicySyncKey(request: PolicySyncRequest): string {
  return JSON.stringify(request)
}

async function flushQueuedPolicySync(): Promise<void> {
  if (policySyncInFlight || !policySyncReady || !hasTauriRuntime() || !queuedPolicySync || !queuedPolicySyncKey) {
    return
  }

  const request = queuedPolicySync
  const requestKey = queuedPolicySyncKey
  let queuedDuringFlightKey: string | null = null
  queuedPolicySync = null
  queuedPolicySyncKey = null
  policySyncInFlight = true

  try {
    await safeInvoke('policy_set', { request }, undefined)
    lastSyncedPolicyKey = requestKey
  } catch (error) {
    if (!queuedPolicySyncKey) {
      queuedPolicySync = request
      queuedPolicySyncKey = requestKey
    }
    console.error('Failed to sync cowork policy to backend', error)
  } finally {
    queuedDuringFlightKey = queuedPolicySyncKey
    policySyncInFlight = false
    if (
      queuedDuringFlightKey &&
      queuedDuringFlightKey !== requestKey &&
      queuedDuringFlightKey !== lastSyncedPolicyKey
    ) {
      void flushQueuedPolicySync()
    }
  }
}

function queuePolicySync(request: PolicySyncRequest): void {
  if (!policySyncReady || !hasTauriRuntime()) return

  const requestKey = getPolicySyncKey(request)
  if (requestKey === lastSyncedPolicyKey || requestKey === queuedPolicySyncKey) {
    if (requestKey === queuedPolicySyncKey) {
      void flushQueuedPolicySync()
    }
    return
  }

  queuedPolicySync = request
  queuedPolicySyncKey = requestKey
  void flushQueuedPolicySync()
}

function markPolicySyncAsCurrent(request: PolicySyncRequest): void {
  lastSyncedPolicyKey = getPolicySyncKey(request)
  queuedPolicySync = null
  queuedPolicySyncKey = null
}

export const PLUGIN_EXAMPLES: Plugin[] = [
  {
    id: 'plugin-marketing-toolkit',
    name: 'Marketing Toolkit',
    domain: 'marketing',
    enabled: true,
    skills: [
      {
        id: 'skill-campaign-brief',
        name: 'Campaign Brief',
        command: '/campaign-brief',
        description: 'Erstellt ein kompaktes Kampagnen-Briefing auf Basis von {{input}}.',
        promptTemplate: 'Erstelle ein Kampagnen-Briefing fuer: {{input}}. Nutze Zielgruppe, Nutzenversprechen, Channels und KPI.',
        runMode: 'execute',
      },
    ],
  },
  {
    id: 'plugin-sales-toolkit',
    name: 'Sales Toolkit',
    domain: 'sales',
    enabled: true,
    skills: [
      {
        id: 'skill-discovery-plan',
        name: 'Discovery Plan',
        command: '/discovery-plan',
        description: 'Strukturiert Discovery-Fragen und naechste Schritte.',
        promptTemplate: 'Formuliere einen Discovery-Plan fuer {{input}} mit Fragen, Risiken und naechsten Schritten.',
        runMode: 'plan',
      },
    ],
  },
  {
    id: 'plugin-finance-toolkit',
    name: 'Finance Toolkit',
    domain: 'finance',
    enabled: true,
    skills: [
      {
        id: 'skill-kpi-recap',
        name: 'KPI Recap',
        command: '/kpi-recap',
        description: 'Fasst KPI-Entwicklungen aus dem Input zusammen.',
        promptTemplate: 'Analysiere {{input}} und gib eine KPI-Zusammenfassung mit Trends, Ausreissern und Handlungsempfehlungen.',
        runMode: 'execute',
      },
    ],
  },
]

function normalizePlugins(items: Plugin[] | undefined): Plugin[] {
  if (!items) return []
  return items.map((plugin) => ({
    ...plugin,
    skills: (plugin.skills ?? []).map((skill) => ({
      ...skill,
      promptTemplate: skill.promptTemplate ?? '',
      runMode: skill.runMode ?? 'execute',
    })),
  }))
}

function mergeConnectors(items: ConnectorConfig[] | undefined): ConnectorConfig[] {
  if (!items || items.length === 0) return DEFAULT_CONNECTORS

  const byKey = new Map(items.map((connector) => [connector.key, connector]))
  return DEFAULT_CONNECTORS.map((base) => ({
    ...base,
    ...(byKey.get(base.key) ?? {}),
    lastTestStatus: byKey.get(base.key)?.lastTestStatus ?? base.lastTestStatus ?? 'idle',
    lastTestAt: byKey.get(base.key)?.lastTestAt ?? base.lastTestAt ?? null,
  }))
}

export const useCoworkStore = create<CoworkState>()(
  persist(
    (set, get) => ({
      globalInstruction: '',
      folderInstructions: [],
      connectors: DEFAULT_CONNECTORS,
      plugins: [],
      scheduledTasks: [],
      scheduledRuns: [],
      claudePlanMode: false,
      claudePermissionMode: 'default',
      claudeToolPreset: 'default',
      claudeTools: CLAUDE_TOOL_CAPABILITIES,
      enabledClaudeToolIds: DEFAULT_ENABLED_CLAUDE_TOOLS,
      toolDenyRules: [],
      policyFlags: DEFAULT_POLICY_FLAGS,
      setGlobalInstruction: (instruction) => set({ globalInstruction: instruction }),
      upsertFolderInstruction: (item) =>
        set((state) => {
          const existingIndex = state.folderInstructions.findIndex((entry) => entry.id === item.id)
          if (existingIndex >= 0) {
            const next = [...state.folderInstructions]
            next[existingIndex] = item
            return { folderInstructions: next }
          }
          return { folderInstructions: [item, ...state.folderInstructions] }
        }),
      removeFolderInstruction: (id) =>
        set((state) => ({
          folderInstructions: state.folderInstructions.filter((entry) => entry.id !== id),
        })),
      toggleConnector: (key, enabled) =>
        set((state) => ({
          connectors: state.connectors.map((connector) =>
            connector.key === key ? { ...connector, enabled } : connector
          ),
        })),
      setConnectorNote: (key, note) =>
        set((state) => ({
          connectors: state.connectors.map((connector) =>
            connector.key === key ? { ...connector, note } : connector
          ),
        })),
      updateConnectorConfig: (key, patch) =>
        set((state) => ({
          connectors: state.connectors.map((connector) =>
            connector.key === key ? { ...connector, ...patch } : connector
          ),
        })),
      testConnector: async (key) => {
        const connector = get().connectors.find((entry) => entry.key === key)
        if (!connector) return

        set((state) => ({
          connectors: state.connectors.map((entry) =>
            entry.key === key
              ? {
                  ...entry,
                  lastTestStatus: 'testing',
                  lastTestMessage: 'Verbindung wird geprueft...',
                }
              : entry
          ),
        }))

        try {
          const response = await safeInvoke<BackendConnectorTestResponse>('connector_test_reachability', {
            request: {
              key: connector.key,
              label: connector.label,
              apiKey: connector.apiKey ?? null,
              webhookUrl: connector.webhookUrl ?? null,
            },
          })

          set((state) => ({
            connectors: state.connectors.map((entry) =>
              entry.key === key
                ? {
                    ...entry,
                    lastTestStatus: response.reachable ? 'success' : 'error',
                    lastTestMessage: response.status ? `${response.message} [HTTP ${response.status}]` : response.message,
                    lastTestAt: parseBackendDate(response.checkedAt) ?? Date.now(),
                  }
                : entry
            ),
          }))
        } catch (error) {
          set((state) => ({
            connectors: state.connectors.map((entry) =>
              entry.key === key
                ? {
                    ...entry,
                    lastTestStatus: 'error',
                    lastTestMessage: error instanceof Error ? error.message : String(error),
                    lastTestAt: Date.now(),
                  }
                : entry
            ),
          }))
        }
      },
      upsertPlugin: (plugin) =>
        set((state) => {
          const hasPlugin = state.plugins.some((entry) => entry.id === plugin.id)
          return {
            plugins: hasPlugin
              ? state.plugins.map((entry) => (entry.id === plugin.id ? plugin : entry))
              : [plugin, ...state.plugins],
          }
        }),
      togglePlugin: (id, enabled) =>
        set((state) => ({
          plugins: state.plugins.map((plugin) =>
            plugin.id === id ? { ...plugin, enabled } : plugin
          ),
        })),
      removePlugin: (id) =>
        set((state) => ({
          plugins: state.plugins.filter((plugin) => plugin.id !== id),
        })),
      installPluginExamples: () =>
        set((state) => {
          const existing = new Map(state.plugins.map((plugin) => [plugin.id, plugin]))
          for (const example of PLUGIN_EXAMPLES) {
            if (!existing.has(example.id)) {
              existing.set(example.id, example)
            }
          }
          return {
            plugins: Array.from(existing.values()),
          }
        }),
      loadScheduledTasks: async () => {
        const local = get().scheduledTasks
        const rows = await safeInvoke<BackendScheduledTaskRow[]>('scheduler_list_tasks', undefined, local.map((task) => ({
          id: task.id,
          name: task.name,
          prompt: task.prompt,
          scheduleExpr: task.cronLike,
          active: task.active,
          lastRunAt: task.lastRunAt ? new Date(task.lastRunAt).toISOString() : null,
          nextRunAt: task.nextRunAt ? new Date(task.nextRunAt).toISOString() : null,
        })))
        set({ scheduledTasks: rows.map(mapScheduledTaskRow) })
      },
      loadScheduledRuns: async (limit = 20) => {
        const rows = await safeInvoke<BackendScheduledRunRow[]>('scheduler_list_runs', { limit }, [])
        set({ scheduledRuns: rows.map(mapScheduledRunRow) })
      },
      upsertScheduledTask: async (task) => {
        const row = await safeInvoke<BackendScheduledTaskRow>('scheduler_upsert_task', {
          request: {
            id: task.id,
            name: task.name,
            prompt: task.prompt,
            scheduleExpr: task.cronLike,
            active: task.active,
          },
        }, {
          id: task.id,
          name: task.name,
          prompt: task.prompt,
          scheduleExpr: task.cronLike,
          active: task.active,
          lastRunAt: task.lastRunAt ? new Date(task.lastRunAt).toISOString() : null,
          nextRunAt: task.nextRunAt ? new Date(task.nextRunAt).toISOString() : null,
        })
        const mapped = mapScheduledTaskRow(row)
        set((state) => {
          const hasTask = state.scheduledTasks.some((entry) => entry.id === mapped.id)
          return {
            scheduledTasks: hasTask
              ? state.scheduledTasks.map((entry) => (entry.id === mapped.id ? mapped : entry))
              : [mapped, ...state.scheduledTasks],
          }
        })
        await get().loadScheduledRuns()
      },
      toggleScheduledTask: async (id, active) => {
        const row = await safeInvoke<BackendScheduledTaskRow>('scheduler_set_task_active', {
          request: { id, active },
        }, {
          id,
          name: get().scheduledTasks.find((task) => task.id === id)?.name ?? id,
          prompt: get().scheduledTasks.find((task) => task.id === id)?.prompt ?? '',
          scheduleExpr: get().scheduledTasks.find((task) => task.id === id)?.cronLike ?? '',
          active,
          lastRunAt: get().scheduledTasks.find((task) => task.id === id)?.lastRunAt
            ? new Date(get().scheduledTasks.find((task) => task.id === id)!.lastRunAt!).toISOString()
            : null,
          nextRunAt: get().scheduledTasks.find((task) => task.id === id)?.nextRunAt
            ? new Date(get().scheduledTasks.find((task) => task.id === id)!.nextRunAt!).toISOString()
            : null,
        })
        const mapped = mapScheduledTaskRow(row)
        set((state) => ({
          scheduledTasks: state.scheduledTasks.map((task) => task.id === id ? mapped : task),
        }))
      },
      markScheduledTaskRun: async (id, _at) => {
        await get().runScheduledTaskNow(id)
      },
      runScheduledTaskNow: async (id) => {
        await safeInvoke('scheduler_run_task_now', { id }, undefined)
        await Promise.all([get().loadScheduledTasks(), get().loadScheduledRuns()])
      },
      removeScheduledTask: async (id) => {
        await safeInvoke('scheduler_delete_task', { id }, undefined)
        set((state) => ({
          scheduledTasks: state.scheduledTasks.filter((task) => task.id !== id),
          scheduledRuns: state.scheduledRuns.filter((run) => run.taskId !== id),
        }))
      },
      setClaudePlanMode: (enabled) => set({ claudePlanMode: enabled }),
      setClaudePermissionMode: (mode) => set({ claudePermissionMode: mode }),
      setClaudeToolPreset: (preset) =>
        set(() => ({
          claudeToolPreset: preset,
          enabledClaudeToolIds: TOOL_PRESET_MAP[preset],
        })),
      toggleClaudeTool: (toolId, enabled) =>
        set((state) => {
          const isKnownTool = state.claudeTools.some((tool) => tool.id === toolId)
          if (!isKnownTool) return {}

          if (enabled) {
            if (state.enabledClaudeToolIds.includes(toolId)) return {}
            return {
              enabledClaudeToolIds: [...state.enabledClaudeToolIds, toolId],
              claudeToolPreset: 'default' as ClaudeToolPreset,
            }
          }

          return {
            enabledClaudeToolIds: state.enabledClaudeToolIds.filter((id) => id !== toolId),
            claudeToolPreset: 'default' as ClaudeToolPreset,
          }
        }),
      addToolDenyRule: (rule) =>
        set((state) => {
          const normalized = rule.trim()
          if (!normalized) return {}
          if (state.toolDenyRules.includes(normalized)) return {}
          return {
            toolDenyRules: [normalized, ...state.toolDenyRules].slice(0, 80),
          }
        }),
      removeToolDenyRule: (rule) =>
        set((state) => ({
          toolDenyRules: state.toolDenyRules.filter((entry) => entry !== rule),
        })),
      setPolicyFlag: (key, value) =>
        set((state) => ({
          policyFlags: {
            ...state.policyFlags,
            [key]: value,
          },
        })),
      setPolicySnapshot: (flags, denyRules) => {
        suppressPolicySync = true
        set((state) => {
          const policyFlags = {
            ...state.policyFlags,
            ...flags,
          }
          const toolDenyRules = normalizePolicyDenyRules(denyRules)
          markPolicySyncAsCurrent(buildPolicySyncRequest(policyFlags, toolDenyRules))
          return {
            policyFlags,
            toolDenyRules,
          }
        })
        suppressPolicySync = false
      },
    }),
    {
      name: 'open-cowork-features',
      onRehydrateStorage: () => () => {
        policySyncReady = true
        queuePolicySync(buildPolicySyncRequest(
          useCoworkStore.getState().policyFlags,
          useCoworkStore.getState().toolDenyRules
        ))
      },
      merge: (persisted, current) => {
        const state = persisted as Partial<CoworkState>
        const persistedTools = Array.isArray(state.enabledClaudeToolIds)
          ? state.enabledClaudeToolIds
          : DEFAULT_ENABLED_CLAUDE_TOOLS
        const knownTools = new Set(CLAUDE_TOOL_CAPABILITIES.map((tool) => tool.id))
        const normalizedTools = persistedTools.filter((id) => knownTools.has(id))
        return {
          ...current,
          ...state,
          folderInstructions: Array.isArray(state.folderInstructions) ? state.folderInstructions : [],
          connectors: mergeConnectors(state.connectors),
          plugins: normalizePlugins(state.plugins),
          scheduledTasks: Array.isArray(state.scheduledTasks) ? state.scheduledTasks : [],
          scheduledRuns: Array.isArray(state.scheduledRuns) ? state.scheduledRuns : [],
          claudeTools: CLAUDE_TOOL_CAPABILITIES,
          enabledClaudeToolIds: normalizedTools.length > 0 ? normalizedTools : DEFAULT_ENABLED_CLAUDE_TOOLS,
          toolDenyRules: Array.isArray(state.toolDenyRules) ? state.toolDenyRules : [],
          policyFlags: {
            ...DEFAULT_POLICY_FLAGS,
            ...(state.policyFlags ?? {}),
          },
        }
      },
    }
  )
)

useCoworkStore.subscribe((state, previousState) => {
  if (suppressPolicySync) return

  if (
    state.policyFlags === previousState.policyFlags &&
    state.toolDenyRules === previousState.toolDenyRules
  ) {
    return
  }

  queuePolicySync(buildPolicySyncRequest(state.policyFlags, state.toolDenyRules))
})
