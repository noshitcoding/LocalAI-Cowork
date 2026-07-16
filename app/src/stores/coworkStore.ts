import { create } from 'zustand'
import { persist } from 'zustand/middleware'
import { connectorLocator, setCredential } from '../security/credentialVault'
import { sanitizeConnectorsForPersistence } from '../security/credentialPersistence'
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
  taskKind: 'prompt' | 'crew'
  crewId: string | null
  crewSnapshotJson: string | null
  modelConfigJson: string | null
  priority: number
  dependsOnTaskIds: string[]
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

export type ToolsetPolicy = {
  id: string
  label: string
  description: string
  riskLevel: 'low' | 'medium' | 'high' | string
  toolIds: string[]
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
  enabledToolIds: string[]
  activeToolsetPolicyId: string
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
  activeToolsetPolicyId: string
  toolsetPolicies: ToolsetPolicy[]
  setGlobalInstruction: (instruction: string) => void
  upsertFolderInstruction: (item: FolderInstruction) => void
  removeFolderInstruction: (id: string) => void
  toggleConnector: (key: ConnectorKey, enabled: boolean) => void
  setConnectorNote: (key: ConnectorKey, note: string) => void
  updateConnectorConfig: (key: ConnectorKey, patch: Partial<Omit<ConnectorConfig, 'apiKey' | 'webhookUrl'>>) => void
  setConnectorApiKey: (key: ConnectorKey, apiKey: string) => Promise<void>
  setConnectorWebhookUrl: (key: ConnectorKey, webhookUrl: string) => Promise<void>
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
  setActiveToolsetPolicy: (policyId: string) => void
  setPolicySnapshot: (
    flags: Partial<CoworkPolicyFlags>,
    denyRules: string[],
    enabledToolIds: string[],
    activeToolsetPolicyId?: string,
    toolsetPolicies?: ToolsetPolicy[]
  ) => void
}

const DEFAULT_CONNECTORS: ConnectorConfig[] = [
  { key: 'chrome', label: 'Claude in Chrome', enabled: false, note: '', lastTestStatus: 'idle', lastTestAt: null },
  { key: 'slack', label: 'Slack', enabled: false, note: '', webhookUrl: '', apiKey: '', lastTestStatus: 'idle', lastTestAt: null },
  { key: 'drive', label: 'Google Drive', enabled: false, note: '', webhookUrl: '', apiKey: '', lastTestStatus: 'idle', lastTestAt: null },
  { key: 'webhook', label: 'Custom Webhook', enabled: false, note: '', webhookUrl: '', apiKey: '', lastTestStatus: 'idle', lastTestAt: null },
]

const CLAUDE_TOOL_CAPABILITIES: ClaudeToolCapability[] = [
  { id: 'bash', label: 'Bash / PowerShell', description: 'Execute shell commands' },
  { id: 'read_file', label: 'Read files', description: 'Read workspace files' },
  { id: 'edit_file', label: 'Edit files', description: 'Modify files precisely' },
  { id: 'create_directory', label: 'Create folder', description: 'create directories safely' },
  { id: 'move_path', label: 'Move files', description: 'move/rename files and folders' },
  { id: 'copy_path', label: 'Copy files', description: 'copy files and folders' },
  { id: 'glob', label: 'File search', description: 'Find files by pattern' },
  { id: 'grep', label: 'Text search', description: 'Regex/string search across files' },
  { id: 'web_fetch', label: 'Web Fetch', description: 'load the contents of a URL' },
  { id: 'web_search', label: 'Web Search', description: 'Web search over search queries' },
  { id: 'office_workflow', label: 'Office / PowerPoint', description: 'Create PPTX and DOCX artifacts' },
  { id: 'todo', label: 'Task/Todo', description: 'Maintain todo list and work plan' },
  { id: 'delegate_task', label: 'Delegate task', description: 'Delegate tasks to other crew members' },
  { id: 'ask_user', label: 'Ask questions', description: 'Gezielte Ask questions for Klarheit' },
  { id: 'mcp', label: 'MCP Tools', description: 'Use MCP servers and tools' },
]

const DEFAULT_ENABLED_CLAUDE_TOOLS = CLAUDE_TOOL_CAPABILITIES.map((tool) => tool.id)

const TOOL_PRESET_MAP: Record<ClaudeToolPreset, string[]> = {
  default: DEFAULT_ENABLED_CLAUDE_TOOLS,
  safe: ['read_file', 'glob', 'grep', 'todo', 'ask_user', 'web_fetch', 'web_search'],
  extended: DEFAULT_ENABLED_CLAUDE_TOOLS,
}

const CUSTOM_TOOLSET_POLICY_ID = 'custom'

const DEFAULT_TOOLSET_POLICIES: ToolsetPolicy[] = [
  {
    id: 'host_full',
    label: 'Host full',
    description: 'Full local agent profile for trusted workspace automation.',
    riskLevel: 'high',
    toolIds: DEFAULT_ENABLED_CLAUDE_TOOLS,
  },
  {
    id: 'safe_research',
    label: 'Safe research',
    description: 'Read-only workspace and web research without shell, file edits, MCP, or delegation.',
    riskLevel: 'low',
    toolIds: ['read_file', 'glob', 'grep', 'web_fetch', 'web_search', 'todo', 'ask_user'],
  },
  {
    id: 'code_edit',
    label: 'Code edit',
    description: 'Local development profile with filesystem edits and shell, without web, MCP, or delegation.',
    riskLevel: 'medium',
    toolIds: ['bash', 'read_file', 'edit_file', 'create_directory', 'move_path', 'copy_path', 'glob', 'grep', 'office_workflow', 'todo', 'ask_user'],
  },
  {
    id: 'remote_mcp',
    label: 'Remote MCP',
    description: 'Connector-oriented profile for remote tools and web research, without local shell or file edits.',
    riskLevel: 'medium',
    toolIds: ['web_fetch', 'web_search', 'todo', 'ask_user', 'mcp'],
  },
  {
    id: 'supervisor',
    label: 'Supervisor',
    description: 'Coordination profile for planning, asking the user, delegation, and read-only context gathering.',
    riskLevel: 'medium',
    toolIds: ['read_file', 'glob', 'grep', 'todo', 'delegate_task', 'ask_user', 'mcp'],
  },
]

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
  taskKind?: 'prompt' | 'crew'
  crewId?: string | null
  crewSnapshotJson?: string | null
  modelConfigJson?: string | null
  priority?: number
  dependsOnTaskIds?: string[]
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
    taskKind: row.taskKind ?? 'prompt',
    crewId: row.crewId ?? null,
    crewSnapshotJson: row.crewSnapshotJson ?? null,
    modelConfigJson: row.modelConfigJson ?? null,
    priority: row.priority ?? 100,
    dependsOnTaskIds: row.dependsOnTaskIds ?? [],
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

function normalizeEnabledClaudeToolIds(enabledToolIds: string[]): string[] {
  const knownTools = new Set(CLAUDE_TOOL_CAPABILITIES.map((tool) => tool.id))
  const seen = new Set<string>()

  return enabledToolIds
    .map((toolId) => toolId.trim())
    .filter((toolId) => {
      if (!toolId || !knownTools.has(toolId) || seen.has(toolId)) {
        return false
      }

      seen.add(toolId)
      return true
    })
}

function toolIdsEqual(left: string[], right: string[]): boolean {
  if (left.length !== right.length) return false
  return left.every((toolId, index) => toolId === right[index])
}

function normalizeToolsetPolicies(toolsetPolicies?: ToolsetPolicy[]): ToolsetPolicy[] {
  const source = toolsetPolicies && toolsetPolicies.length > 0
    ? toolsetPolicies
    : DEFAULT_TOOLSET_POLICIES
  const seen = new Set<string>()

  return source
    .map((policy) => ({
      ...policy,
      id: policy.id.trim(),
      label: policy.label.trim() || policy.id.trim(),
      description: policy.description.trim(),
      riskLevel: policy.riskLevel || 'medium',
      toolIds: normalizeEnabledClaudeToolIds(policy.toolIds),
    }))
    .filter((policy) => {
      if (!policy.id || seen.has(policy.id)) return false
      seen.add(policy.id)
      return true
    })
}

function inferToolsetPolicyId(
  enabledToolIds: string[],
  activeToolsetPolicyId: string | undefined,
  toolsetPolicies: ToolsetPolicy[]
): string {
  const normalizedEnabledToolIds = normalizeEnabledClaudeToolIds(enabledToolIds)
  const requestedPolicyId = activeToolsetPolicyId?.trim()

  if (requestedPolicyId === CUSTOM_TOOLSET_POLICY_ID) {
    return CUSTOM_TOOLSET_POLICY_ID
  }

  if (requestedPolicyId) {
    const requestedPolicy = toolsetPolicies.find((policy) => policy.id === requestedPolicyId)
    if (requestedPolicy && toolIdsEqual(requestedPolicy.toolIds, normalizedEnabledToolIds)) {
      return requestedPolicy.id
    }
  }

  return toolsetPolicies.find((policy) => toolIdsEqual(policy.toolIds, normalizedEnabledToolIds))?.id
    ?? CUSTOM_TOOLSET_POLICY_ID
}

function buildPolicySyncRequest(
  policyFlags: CoworkPolicyFlags,
  toolDenyRules: string[],
  enabledToolIds: string[],
  activeToolsetPolicyId: string
): PolicySyncRequest {
  return {
    flags: policyFlags,
    denyRules: normalizePolicyDenyRules(toolDenyRules),
    enabledToolIds: normalizeEnabledClaudeToolIds(enabledToolIds),
    activeToolsetPolicyId: activeToolsetPolicyId.trim() || CUSTOM_TOOLSET_POLICY_ID,
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
        description: 'Creates a compact campaign briefing based on {{input}}.',
        promptTemplate: 'Create ein Kampagnen-Briefing for: {{input}}. Use Targetgruppe, Usenversprechen, Channels und KPI.',
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
        description: 'Structures discovery questions and next steps.',
        promptTemplate: 'Write a discovery plan for {{input}} with questions, risks, and next steps.',
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
        description: 'Summarizes KPI developments from the input.',
        promptTemplate: 'Analyze {{input}} and provide a KPI summary with trends, outliers, and recommendations.',
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
      activeToolsetPolicyId: 'host_full',
      toolsetPolicies: DEFAULT_TOOLSET_POLICIES,
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
      setConnectorApiKey: async (key, apiKey) => {
        await setCredential(connectorLocator(key, 'api_key'), apiKey)
        set((state) => ({
          connectors: state.connectors.map((connector) => (
            connector.key === key ? { ...connector, apiKey } : connector
          )),
        }))
      },
      setConnectorWebhookUrl: async (key, webhookUrl) => {
        await setCredential(connectorLocator(key, 'webhook_url'), webhookUrl)
        set((state) => ({
          connectors: state.connectors.map((connector) => (
            connector.key === key ? { ...connector, webhookUrl } : connector
          )),
        }))
      },
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
          taskKind: task.taskKind,
          crewId: task.crewId,
          crewSnapshotJson: task.crewSnapshotJson,
          modelConfigJson: task.modelConfigJson,
          priority: task.priority,
          dependsOnTaskIds: task.dependsOnTaskIds,
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
            taskKind: task.taskKind,
            crewId: task.crewId,
            crewSnapshotJson: task.crewSnapshotJson,
            modelConfigJson: task.modelConfigJson,
            priority: task.priority,
            dependsOnTaskIds: task.dependsOnTaskIds,
            active: task.active,
          },
        }, {
          id: task.id,
          name: task.name,
          prompt: task.prompt,
          scheduleExpr: task.cronLike,
          taskKind: task.taskKind,
          crewId: task.crewId,
          crewSnapshotJson: task.crewSnapshotJson,
          modelConfigJson: task.modelConfigJson,
          priority: task.priority,
          dependsOnTaskIds: task.dependsOnTaskIds,
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
          taskKind: get().scheduledTasks.find((task) => task.id === id)?.taskKind ?? 'prompt',
          crewId: get().scheduledTasks.find((task) => task.id === id)?.crewId ?? null,
          crewSnapshotJson: get().scheduledTasks.find((task) => task.id === id)?.crewSnapshotJson ?? null,
          modelConfigJson: get().scheduledTasks.find((task) => task.id === id)?.modelConfigJson ?? null,
          priority: get().scheduledTasks.find((task) => task.id === id)?.priority ?? 100,
          dependsOnTaskIds: get().scheduledTasks.find((task) => task.id === id)?.dependsOnTaskIds ?? [],
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
        await safeInvoke<null>('scheduler_delete_task', { id }, null)
        set((state) => ({
          scheduledTasks: state.scheduledTasks.filter((task) => task.id !== id),
          scheduledRuns: state.scheduledRuns.filter((run) => run.taskId !== id),
        }))
      },
      setClaudePlanMode: (enabled) => set({ claudePlanMode: enabled }),
      setClaudePermissionMode: (mode) => set({ claudePermissionMode: mode }),
      setClaudeToolPreset: (preset) =>
        set((state) => {
          const enabledClaudeToolIds = [...TOOL_PRESET_MAP[preset]]
          return {
            claudeToolPreset: preset,
            enabledClaudeToolIds,
            activeToolsetPolicyId: inferToolsetPolicyId(
              enabledClaudeToolIds,
              preset === 'safe' ? 'safe_research' : undefined,
              state.toolsetPolicies
            ),
          }
        }),
      toggleClaudeTool: (toolId, enabled) =>
        set((state) => {
          const isKnownTool = state.claudeTools.some((tool) => tool.id === toolId)
          if (!isKnownTool) return {}

          if (enabled) {
            if (state.enabledClaudeToolIds.includes(toolId)) return {}
            return {
              enabledClaudeToolIds: [...state.enabledClaudeToolIds, toolId],
              claudeToolPreset: 'default' as ClaudeToolPreset,
              activeToolsetPolicyId: CUSTOM_TOOLSET_POLICY_ID,
            }
          }

          return {
            enabledClaudeToolIds: state.enabledClaudeToolIds.filter((id) => id !== toolId),
            claudeToolPreset: 'default' as ClaudeToolPreset,
            activeToolsetPolicyId: CUSTOM_TOOLSET_POLICY_ID,
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
      setActiveToolsetPolicy: (policyId) =>
        set((state) => {
          const normalizedPolicyId = policyId.trim() || CUSTOM_TOOLSET_POLICY_ID
          if (normalizedPolicyId === CUSTOM_TOOLSET_POLICY_ID) {
            return {
              activeToolsetPolicyId: CUSTOM_TOOLSET_POLICY_ID,
              claudeToolPreset: 'default' as ClaudeToolPreset,
            }
          }

          const policy = state.toolsetPolicies.find((entry) => entry.id === normalizedPolicyId)
          if (!policy) return {}

          return {
            activeToolsetPolicyId: policy.id,
            enabledClaudeToolIds: [...policy.toolIds],
            claudeToolPreset: policy.id === 'safe_research' ? 'safe' as ClaudeToolPreset : 'default' as ClaudeToolPreset,
          }
        }),
      setPolicySnapshot: (flags, denyRules, enabledToolIds, activeToolsetPolicyId, toolsetPolicies) => {
        suppressPolicySync = true
        set((state) => {
          const normalizedToolsetPolicies = normalizeToolsetPolicies(toolsetPolicies)
          const policyFlags = {
            ...state.policyFlags,
            ...flags,
          }
          const toolDenyRules = normalizePolicyDenyRules(denyRules)
          const normalizedEnabledToolIds = normalizeEnabledClaudeToolIds(enabledToolIds)
          const nextActiveToolsetPolicyId = inferToolsetPolicyId(
            normalizedEnabledToolIds,
            activeToolsetPolicyId,
            normalizedToolsetPolicies
          )
          markPolicySyncAsCurrent(buildPolicySyncRequest(
            policyFlags,
            toolDenyRules,
            normalizedEnabledToolIds,
            nextActiveToolsetPolicyId
          ))
          return {
            policyFlags,
            toolDenyRules,
            enabledClaudeToolIds: normalizedEnabledToolIds,
            activeToolsetPolicyId: nextActiveToolsetPolicyId,
            toolsetPolicies: normalizedToolsetPolicies,
            claudeToolPreset: nextActiveToolsetPolicyId === 'safe_research' ? 'safe' : 'default',
          }
        })
        suppressPolicySync = false
      },
    }),
    {
      name: 'open-cowork-features',
      partialize: (state) => ({
        ...state,
        connectors: sanitizeConnectorsForPersistence(state.connectors),
      }),
      onRehydrateStorage: () => () => {
        policySyncReady = true
        queuePolicySync(buildPolicySyncRequest(
          useCoworkStore.getState().policyFlags,
          useCoworkStore.getState().toolDenyRules,
          useCoworkStore.getState().enabledClaudeToolIds,
          useCoworkStore.getState().activeToolsetPolicyId,
        ))
      },
      merge: (persisted, current) => {
        const state = persisted as Partial<CoworkState>
        const hasPersistedTools = Array.isArray(state.enabledClaudeToolIds)
        const persistedTools = hasPersistedTools ? state.enabledClaudeToolIds ?? [] : DEFAULT_ENABLED_CLAUDE_TOOLS
        const normalizedTools = normalizeEnabledClaudeToolIds(persistedTools)
        const normalizedToolsetPolicies = normalizeToolsetPolicies(state.toolsetPolicies)
        const enabledClaudeToolIds = hasPersistedTools ? normalizedTools : DEFAULT_ENABLED_CLAUDE_TOOLS
        const activeToolsetPolicyId = inferToolsetPolicyId(
          enabledClaudeToolIds,
          state.activeToolsetPolicyId,
          normalizedToolsetPolicies
        )
        return {
          ...current,
          ...state,
          folderInstructions: Array.isArray(state.folderInstructions) ? state.folderInstructions : [],
          connectors: mergeConnectors(state.connectors),
          plugins: normalizePlugins(state.plugins),
          scheduledTasks: Array.isArray(state.scheduledTasks) ? state.scheduledTasks : [],
          scheduledRuns: Array.isArray(state.scheduledRuns) ? state.scheduledRuns : [],
          claudeTools: CLAUDE_TOOL_CAPABILITIES,
          enabledClaudeToolIds,
          activeToolsetPolicyId,
          toolsetPolicies: normalizedToolsetPolicies,
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
    state.toolDenyRules === previousState.toolDenyRules &&
    state.enabledClaudeToolIds === previousState.enabledClaudeToolIds &&
    state.activeToolsetPolicyId === previousState.activeToolsetPolicyId
  ) {
    return
  }

  queuePolicySync(buildPolicySyncRequest(
    state.policyFlags,
    state.toolDenyRules,
    state.enabledClaudeToolIds,
    state.activeToolsetPolicyId
  ))
})
