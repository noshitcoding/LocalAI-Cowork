import { open } from '@tauri-apps/plugin-dialog'
import { listen } from '@tauri-apps/api/event'
import { useEffect, useMemo, useRef, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useChatStore, type CrewLiveEntry, type CrewLiveEntryCategory, type CrewLiveState, type CrewLiveStatus } from '../stores/chatStore'
import { useConfigStore } from '../stores/configStore'
import { useCoworkStore, type ScheduledTask } from '../stores/coworkStore'
import { useCrewStore, type Crew, type CrewProviderKind } from '../stores/crewStore'
import { useTaskTemplatesStore } from '../stores/taskTemplatesStore'
import { useUiStore } from '../stores/uiStore'
import { useWorkTasksStore, type WorkTask, type WorkTaskRunner } from '../stores/workTasksStore'
import { safeInvoke, safeInvokeVoid } from '../utils/safeInvoke'
import { streamChatTurn } from '../utils/ollamaStreaming'

type CrewDefinitionVersionRow = {
  id: string
  crewId: string
  versionNumber: number
  changeSummary: string | null
  definitionJson: string
  createdAt: string
}

type CrewScheduleSnapshotMetadata = {
  snapshotSource: 'live' | 'saved-version'
  definitionVersionId?: string
  definitionVersionNumber?: number
  definitionChangeSummary?: string | null
  definitionSavedAt?: string | null
}

type CrewExecutionLog = {
  id: string
  crewId: string
  agentId: string
  taskId: string
  action: string
  result: string
  timestamp: number
}

type CrewExecutionLogEvent = {
  streamId?: string | null
  runId?: string | null
  log: CrewExecutionLog
}

type CrewTaskExecutionResponse = {
  taskId: string
  agentId: string
  status: 'pending' | 'running' | 'completed' | 'failed' | 'canceled'
  output: string | null
}

type CrewExecutionResponse = {
  crewId: string
  status: 'idle' | 'running' | 'completed' | 'failed' | 'canceled'
  taskResults: CrewTaskExecutionResponse[]
  logs: CrewExecutionLog[]
  error: string | null
}

type CrewResolvedProviderConfigs = {
  openAICompatible: { baseUrl: string; model: string; apiKey: string; timeoutMs: number } | undefined
  openRouter: { baseUrl: string; model: string; apiKey: string; timeoutMs: number } | undefined
}

const CREW_LIVE_MAX_ENTRIES = 150
const CREW_AGENT_COLORS = [
  '#2563eb',
  '#059669',
  '#d97706',
  '#7c3aed',
  '#dc2626',
  '#0891b2',
  '#be123c',
  '#4f46e5',
]
const CREW_BOX_CHARS = /[\s│┃║╭╮╰╯┌┐└┘├┤┬┴┼─━═╔╗╚╝╠╣╦╩╬]+/u
const CREW_BOX_EDGE_START = /^[\s│┃║╭╮╰╯┌┐└┘├┤┬┴┼─━═╔╗╚╝╠╣╦╩╬]+/u
const CREW_BOX_EDGE_END = /[\s│┃║╭╮╰╯┌┐└┘├┤┬┴┼─━═╔╗╚╝╠╣╦╩╬]+$/u

function resolveDefaultAgentId(crew: Crew): string | null {
  if (crew.process === 'hierarchical' && crew.managerAgentId) {
    const worker = crew.agents.find((agent) => agent.enabled && agent.id !== crew.managerAgentId)
    if (worker) return worker.id

    const manager = crew.agents.find((agent) => agent.enabled && agent.id === crew.managerAgentId)
    if (manager) return manager.id
  }

  const enabled = crew.agents.find((agent) => agent.enabled)
  if (enabled) return enabled.id
  return crew.agents[0]?.id ?? null
}

function resolveCrewRuntimeConfig(crew: Crew, fallbackConfig: { baseUrl: string; model: string; timeoutMs: number }) {
  if (!crew.runtimeConfig.enabled) {
    return fallbackConfig
  }

  return {
    ...fallbackConfig,
    baseUrl: crew.runtimeConfig.baseUrl.trim() || fallbackConfig.baseUrl,
    model: crew.runtimeConfig.model.trim() || fallbackConfig.model,
    timeoutMs: Math.max(1000, crew.runtimeConfig.timeoutMs || fallbackConfig.timeoutMs),
  }
}

function resolveExternalProviderConfig(
  config: { enabled: boolean; baseUrl: string; model: string; apiKey: string; timeoutMs: number },
  fallbackConfig: { baseUrl?: string; model?: string; apiKey?: string } | undefined,
  fallbackBaseUrl: string,
) {
  if (!config.enabled) {
    return undefined
  }

  return {
    baseUrl: config.baseUrl.trim() || fallbackConfig?.baseUrl?.trim() || fallbackBaseUrl,
    model: config.model.trim() || fallbackConfig?.model?.trim() || '',
    apiKey: config.apiKey.trim() || fallbackConfig?.apiKey?.trim() || '',
    timeoutMs: Math.max(1000, config.timeoutMs || 600000),
  }
}

function applyCrewDefaultModel(
  crew: Crew,
  config: { baseUrl: string; model: string; timeoutMs: number },
  providerConfigs: CrewResolvedProviderConfigs,
) {
  const defaultProvider: CrewProviderKind = crew.defaultProvider ?? 'ollama'
  const defaultModel = crew.defaultModel?.trim()
  if (!defaultModel) {
    return { config, providerConfigs }
  }

  if (defaultProvider === 'ollama') {
    return {
      config: { ...config, model: defaultModel },
      providerConfigs,
    }
  }

  if (defaultProvider === 'openai-compatible' && providerConfigs.openAICompatible) {
    return {
      config,
      providerConfigs: {
        ...providerConfigs,
        openAICompatible: { ...providerConfigs.openAICompatible, model: defaultModel },
      },
    }
  }

  if (defaultProvider === 'openrouter' && providerConfigs.openRouter) {
    return {
      config,
      providerConfigs: {
        ...providerConfigs,
        openRouter: { ...providerConfigs.openRouter, model: defaultModel },
      },
    }
  }

  return { config, providerConfigs }
}

function buildWorkTaskCrewGuidelines(crew: Crew, task: WorkTask): string {
  const workTaskContext = [
    `Work-Task-Auftrag: ${deriveTaskName(task)}`,
    task.prompt.trim(),
    task.expectedOutput.trim() ? `Erwartetes Gesamtergebnis:\n${task.expectedOutput.trim()}` : '',
  ].filter(Boolean).join('\n\n')

  return [
    crew.executionGuidelines.trim(),
    workTaskContext,
  ].filter(Boolean).join('\n\n')
}

function buildCrewRuntimeTasks(crew: Crew, task: WorkTask, enabledAgentIds: Set<string>) {
  const crewTasks = crew.tasks ?? []
  const runnableCrewTasks = crewTasks.filter((crewTask) => enabledAgentIds.has(crewTask.agentId))

  if (crewTasks.length > 0 && runnableCrewTasks.length === 0) {
    throw new Error('Keine ausfuehrbaren Crew-Tasks vorhanden: alle zugewiesenen Crew-Mitglieder sind deaktiviert oder fehlen.')
  }

  if (runnableCrewTasks.length > 0) {
    const runnableTaskIds = new Set(runnableCrewTasks.map((crewTask) => crewTask.id))
    return runnableCrewTasks.map((crewTask) => ({
      id: crewTask.id,
      description: crewTask.description,
      expectedOutput: crewTask.expectedOutput || task.expectedOutput || 'Erstelle ein vollstaendiges Ergebnis.',
      agentId: crewTask.agentId,
      context: crewTask.context.filter((contextId) => runnableTaskIds.has(contextId)),
      dependencies: crewTask.dependencies.filter((dependencyId) => runnableTaskIds.has(dependencyId)),
      asyncExecution: crew.process === 'parallel' ? true : crewTask.asyncExecution,
    }))
  }

  const agentId = resolveDefaultAgentId(crew)
  if (!agentId) {
    throw new Error('Crew hat keinen Agenten.')
  }

  return [
    {
      id: task.id,
      description: task.prompt,
      expectedOutput: task.expectedOutput || 'Erstelle ein vollstaendiges Ergebnis.',
      agentId,
      context: [],
      dependencies: [],
      asyncExecution: crew.process === 'parallel',
    },
  ]
}

function buildCrewRunOutput(response: CrewExecutionResponse, fallbackTaskId: string): string {
  const directResult = response.taskResults.find((result) => result.taskId === fallbackTaskId)
  if (directResult?.output?.trim()) {
    return directResult.output
  }

  const renderedResults = response.taskResults
    .filter((result) => result.output?.trim())
    .map((result) => [
      `Task: ${result.taskId}`,
      `Agent: ${result.agentId}`,
      result.output?.trim() ?? '',
    ].filter(Boolean).join('\n'))

  if (renderedResults.length > 0) {
    return renderedResults.join('\n\n---\n\n')
  }

  return response.error ?? 'Crew-Lauf abgeschlossen ohne Textausgabe.'
}

function formatTimestamp(ts: number | null | undefined): string {
  if (!ts) return '—'
  try {
    return new Date(ts).toLocaleString('de-DE')
  } catch {
    return String(ts)
  }
}

function deriveTaskName(task: WorkTask): string {
  const title = task.title.trim()
  if (title) return title
  const prompt = task.prompt.trim()
  if (!prompt) return task.id
  const singleLine = prompt.replace(/\s+/g, ' ').trim()
  return singleLine.length > 48 ? `${singleLine.slice(0, 48)}…` : singleLine
}

function findScheduledTask(scheduledTasks: ScheduledTask[], taskId: string): ScheduledTask | null {
  return scheduledTasks.find((entry) => entry.id === taskId) ?? null
}

function isAbsolutePath(path: string): boolean {
  const trimmed = path.trim()
  return /^(?:[a-zA-Z]:[\\/]|\\\\|\/)/.test(trimmed)
}

function createCrewStreamId(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return `crew-${crypto.randomUUID()}`
  }

  return `crew-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`
}

function buildTaskThreadSummary(task: WorkTask): string {
  const lines = [
    `Task angelegt: ${deriveTaskName(task)}`,
    `Runner: ${task.runner === 'crew' ? 'Crew' : 'Modell'}`,
    task.expectedOutput.trim() ? `Expected Output: ${task.expectedOutput.trim()}` : '',
    task.workDir.trim() ? `Arbeitsordner: ${task.workDir.trim()}` : '',
  ].filter(Boolean)

  return lines.join('\n')
}

function buildTaskPromptMessage(task: WorkTask): string {
  const parts = [task.prompt.trim()]

  if (task.expectedOutput.trim()) {
    parts.push(`Erwartetes Ergebnis:\n${task.expectedOutput.trim()}`)
  }

  if (task.workDir.trim()) {
    parts.push(`Arbeitsordner:\n${task.workDir.trim()}`)
  }

  return parts.filter(Boolean).join('\n\n')
}

function getCrewLogActionLabel(action: string): string {
  switch (action) {
    case 'run_started': return 'Lauf gestartet'
    case 'runtime_context': return 'Runtime-Kontext'
    case 'agent_ready': return 'Agent bereit'
    case 'task_handoff': return 'Uebergabe'
    case 'crew_kickoff': return 'CrewAI gestartet'
    case 'runtime_stdout': return 'Runtime-Ausgabe'
    case 'runtime_stderr': return 'Runtime-Fehlerausgabe'
    case 'crew_finished': return 'CrewAI abgeschlossen'
    case 'task_completed': return 'Task abgeschlossen'
    case 'runtime_failed': return 'Runtime fehlgeschlagen'
    default: return action
  }
}

function stripCrewRuntimeChrome(line: string): string {
  const trimmed = line.trimEnd()
  if (!trimmed || trimmed.replace(CREW_BOX_CHARS, '').trim().length === 0) {
    return ''
  }

  return trimmed
    .replace(CREW_BOX_EDGE_START, '')
    .replace(CREW_BOX_EDGE_END, '')
    .trim()
}

function cleanCrewLogDetail(log: CrewExecutionLog): string {
  const lines = log.result
    .replace(/\r/g, '\n')
    .split('\n')
    .map(stripCrewRuntimeChrome)
    .filter((line) => line.trim().length > 0)

  return lines.join('\n').trim()
}

function classifyCrewLog(log: CrewExecutionLog, detail: string): CrewLiveEntryCategory {
  const combined = `${log.action}\n${detail}`.toLowerCase()

  if (combined.includes('traceback') || combined.includes('error') || combined.includes('failed') || log.action.includes('stderr') || log.action.includes('failed')) {
    return 'error'
  }
  if (combined.includes('mcp')) {
    return 'mcp'
  }
  if (combined.includes('delegate_work') || combined.includes('delegation') || combined.includes('coworker')) {
    return 'delegation'
  }
  if (combined.includes('tool execution') || combined.includes('tool:') || combined.includes('args:')) {
    return 'tool'
  }
  if (log.action === 'task_handoff') {
    return 'handoff'
  }
  if (log.action === 'agent_ready' || combined.includes('agent started')) {
    return 'agent'
  }
  if (log.action === 'runtime_context') {
    return 'context'
  }
  if (log.action === 'task_completed') {
    return 'task'
  }
  if (log.action === 'run_started' || log.action === 'crew_kickoff' || log.action === 'crew_finished') {
    return 'status'
  }

  return 'output'
}

function firstMatchingLine(detail: string, pattern: RegExp): string | null {
  return detail.split('\n').map((line) => line.trim()).find((line) => pattern.test(line)) ?? null
}

function buildCrewLiveTitle(log: CrewExecutionLog, category: CrewLiveEntryCategory, detail: string): string {
  if (category === 'tool') {
    return firstMatchingLine(detail, /^Tool:/i) ?? 'Tool-Ausfuehrung'
  }
  if (category === 'delegation') {
    return 'Delegation an Crew-Mitglied'
  }
  if (category === 'mcp') {
    return 'MCP-Kontext oder MCP-Zugriff'
  }
  if (category === 'handoff') {
    return `Task-Uebergabe an ${log.agentId}`
  }
  if (category === 'agent') {
    return firstMatchingLine(detail, /^Agent:/i) ?? `Agent ${log.agentId} bereit`
  }
  if (category === 'task') {
    return 'Task-Ergebnis erhalten'
  }
  if (category === 'error') {
    return 'Crew-Fehler'
  }
  if (category === 'context') {
    return 'Runtime-Kontext geladen'
  }
  if (category === 'status') {
    return getCrewLogActionLabel(log.action)
  }

  return getCrewLogActionLabel(log.action)
}

function normalizeCrewAgentLabel(value: string): string {
  const normalized = value.trim().replace(/^['"]|['"]$/g, '')
  if (!normalized) return ''
  return normalized.replace(/\s+/g, '-').toLowerCase()
}

function deriveCrewLiveAgentId(log: CrewExecutionLog, detail: string): string {
  const agentLine = firstMatchingLine(detail, /^Agent:\s*(.+)$/i)
  if (agentLine) {
    const agent = normalizeCrewAgentLabel(agentLine.replace(/^Agent:\s*/i, ''))
    if (agent) return agent
  }

  const handoffMatch = detail.match(/Task an Agent uebergeben:\s*([^\n]+)/i)
  if (handoffMatch?.[1]) {
    const agent = normalizeCrewAgentLabel(handoffMatch[1])
    if (agent) return agent
  }

  const coworkerMatch = detail.match(/coworker['"]?\s*:\s*['"]([^'"]+)/i)
  if (coworkerMatch?.[1]) {
    const agent = normalizeCrewAgentLabel(coworkerMatch[1])
    if (agent) return agent
  }

  return log.agentId || 'runtime'
}

function createCrewLiveEntry(log: CrewExecutionLog): CrewLiveEntry | null {
  const detail = cleanCrewLogDetail(log)
  if (!detail && (log.action === 'runtime_stdout' || log.action === 'runtime_stderr')) {
    return null
  }

  const category = classifyCrewLog(log, detail)
  return {
    id: log.id,
    timestamp: log.timestamp || Date.now(),
    agentId: deriveCrewLiveAgentId(log, detail),
    taskId: log.taskId || 'runtime',
    action: log.action,
    category,
    title: buildCrewLiveTitle(log, category, detail),
    detail,
  }
}

function assignCrewAgentColor(agentId: string, colors: Record<string, string>): Record<string, string> {
  if (!agentId.trim() || colors[agentId]) {
    return colors
  }

  const next = { ...colors }
  next[agentId] = CREW_AGENT_COLORS[Object.keys(next).length % CREW_AGENT_COLORS.length]
  return next
}

function shouldMergeCrewLiveEntries(previous: CrewLiveEntry | undefined, next: CrewLiveEntry): boolean {
  if (!previous) return false
  if (previous.agentId !== next.agentId || previous.taskId !== next.taskId) return false
  if (previous.action !== 'runtime_stdout' || next.action !== 'runtime_stdout') return false
  if (previous.category !== next.category) return false
  return previous.detail.length < 12000
}

function appendCrewLiveEntry(state: CrewLiveState, entry: CrewLiveEntry): CrewLiveState {
  const agentColors = assignCrewAgentColor(entry.agentId, state.agentColors)
  const entries = [...state.entries]
  const previous = entries[entries.length - 1]

  if (shouldMergeCrewLiveEntries(previous, entry)) {
    entries[entries.length - 1] = {
      ...previous!,
      detail: [previous!.detail, entry.detail].filter(Boolean).join('\n'),
      timestamp: entry.timestamp,
    }
  } else {
    entries.push(entry)
  }

  return {
    ...state,
    agentColors,
    entries: entries.slice(-CREW_LIVE_MAX_ENTRIES),
    updatedAt: Date.now(),
  }
}

function buildCrewLiveMessageContent(state: CrewLiveState): string {
  const latest = state.entries[state.entries.length - 1]
  return [
    'Crew Live Monitor',
    `Status: ${state.status}`,
    `Angezeigte Ereignisse: ${state.entries.length}`,
    latest ? `Letztes Ereignis: ${latest.title}` : '',
  ].filter(Boolean).join('\n')
}

function hydrateCrewFromDefinition(baseCrew: Crew, rawDefinition: string): Crew | null {
  try {
    const parsed = JSON.parse(rawDefinition) as Partial<Crew>
    return {
      ...baseCrew,
      ...parsed,
      providerProfiles: parsed.providerProfiles ?? baseCrew.providerProfiles,
      agents: Array.isArray(parsed.agents) ? parsed.agents : baseCrew.agents,
      tasks: Array.isArray(parsed.tasks) ? parsed.tasks : baseCrew.tasks,
      runtimeConfig: parsed.runtimeConfig ?? baseCrew.runtimeConfig,
      status: baseCrew.status,
      createdAt: baseCrew.createdAt,
      updatedAt: baseCrew.updatedAt,
    }
  } catch {
    return null
  }
}

async function resolveCrewScheduleSource(crew: Crew): Promise<{ crew: Crew; metadata: CrewScheduleSnapshotMetadata }> {
  try {
    const versions = await safeInvoke<CrewDefinitionVersionRow[]>('crew_definition_versions_list', { crewId: crew.id }, [])
    const latestVersion = Array.isArray(versions) ? versions[0] : undefined
    if (!latestVersion?.definitionJson?.trim()) {
      return {
        crew,
        metadata: { snapshotSource: 'live' },
      }
    }

    const hydrated = hydrateCrewFromDefinition(crew, latestVersion.definitionJson)
    if (!hydrated) {
      return {
        crew,
        metadata: { snapshotSource: 'live' },
      }
    }

    return {
      crew: hydrated,
      metadata: {
        snapshotSource: 'saved-version',
        definitionVersionId: latestVersion.id,
        definitionVersionNumber: latestVersion.versionNumber,
        definitionChangeSummary: latestVersion.changeSummary,
        definitionSavedAt: latestVersion.createdAt,
      },
    }
  } catch {
    return {
      crew,
      metadata: { snapshotSource: 'live' },
    }
  }
}

function readCrewScheduleSnapshotMetadata(snapshotJson: string | null | undefined): CrewScheduleSnapshotMetadata | null {
  if (!snapshotJson?.trim()) {
    return null
  }

  try {
    const parsed = JSON.parse(snapshotJson) as Partial<CrewScheduleSnapshotMetadata>
    if (parsed.snapshotSource !== 'live' && parsed.snapshotSource !== 'saved-version') {
      return null
    }

    return {
      snapshotSource: parsed.snapshotSource,
      definitionVersionId: typeof parsed.definitionVersionId === 'string' ? parsed.definitionVersionId : undefined,
      definitionVersionNumber: typeof parsed.definitionVersionNumber === 'number' ? parsed.definitionVersionNumber : undefined,
      definitionChangeSummary: typeof parsed.definitionChangeSummary === 'string' || parsed.definitionChangeSummary === null ? parsed.definitionChangeSummary : undefined,
      definitionSavedAt: typeof parsed.definitionSavedAt === 'string' || parsed.definitionSavedAt === null ? parsed.definitionSavedAt : undefined,
    }
  } catch {
    return null
  }
}

export default function TasksView() {
  const navigate = useNavigate()
  const crews = useCrewStore((s) => s.crews)
  const { tasks, addTask, updateTask, removeTask, upsertMany } = useWorkTasksStore()
  const addThread = useChatStore((s) => s.addThread)
  const activeThreadId = useChatStore((s) => s.activeThreadId)
  const threads = useChatStore((s) => s.threads)
  const setActiveThread = useChatStore((s) => s.setActiveThread)
  const addChatMessage = useChatStore((s) => s.addMessage)
  const updateChatMessage = useChatStore((s) => s.updateMessage)
  const setActiveMode = useUiStore((s) => s.setActiveMode)
  const setWorkingFolder = useUiStore((s) => s.setWorkingFolder)

  const templates = useTaskTemplatesStore((s) => s.templates)
  const removeTemplate = useTaskTemplatesStore((s) => s.removeTemplate)

  const {
    scheduledTasks,
    loadScheduledTasks,
    upsertScheduledTask,
    toggleScheduledTask,
    removeScheduledTask,
  } = useCoworkStore()

  const ollamaConfig = useConfigStore((s) => s.ollama)
  const defaultLlmProfileIds = useConfigStore((s) => s.defaultLlmProfileIds)
  const llmProfiles = useConfigStore((s) => s.llmProfiles)

  const [newTitle, setNewTitle] = useState('')
  const [newPrompt, setNewPrompt] = useState('')
  const [newExpectedOutput, setNewExpectedOutput] = useState('')
  const [newWorkDir, setNewWorkDir] = useState('')
  const [newRunner, setNewRunner] = useState<WorkTaskRunner>('crew')
  const [newCrewId, setNewCrewId] = useState<string>('')
  const [newModel, setNewModel] = useState<string>('')
  const runningTaskControllersRef = useRef(new Map<string, AbortController>())
  const runningCrewTaskIdsRef = useRef(new Map<string, string>())
  const canceledTaskIdsRef = useRef(new Set<string>())

  const normalizedNewWorkDir = newWorkDir.trim()
  const canCreateTask = newPrompt.trim().length > 0
    && (newRunner !== 'crew' || Boolean(newCrewId))
    && (!normalizedNewWorkDir || isAbsolutePath(normalizedNewWorkDir))

  useEffect(() => {
    void loadScheduledTasks()
  }, [loadScheduledTasks])

  useEffect(() => {
    if (newRunner !== 'crew') return
    if (newCrewId && crews.some((crew) => crew.id === newCrewId)) return
    setNewCrewId(crews[0]?.id ?? '')
  }, [crews, newCrewId, newRunner])

  useEffect(() => {
    // One-way migration helper: import legacy templates as runnable tasks.
    if (tasks.length > 0) return
    if (templates.length === 0) return

    const migrated: WorkTask[] = templates.map((template) => ({
      id: template.id,
      title: template.title ?? '',
      prompt: template.description ?? '',
      expectedOutput: template.expectedOutput ?? '',
      workDir: '',
      threadId: null,
      runner: 'model',
      crewId: null,
      model: '',
      scheduleExpr: '',
      scheduleEnabled: false,
      status: 'idle',
      output: null,
      error: null,
      lastRunAt: null,
      createdAt: Date.now(),
      updatedAt: Date.now(),
    }))

    upsertMany(migrated)
  }, [tasks.length, templates, upsertMany])

  useEffect(() => {
    // One-way migration helper: import Crew-attached tasks into Work Tasks.
    const existingIds = new Set(tasks.map((task) => task.id))
    const legacyCrewTasks: WorkTask[] = []
    for (const crew of crews) {
      for (const crewTask of crew.tasks ?? []) {
        if (existingIds.has(crewTask.id)) continue
        legacyCrewTasks.push({
          id: crewTask.id,
          title: '',
          prompt: crewTask.description,
          expectedOutput: crewTask.expectedOutput,
          workDir: '',
          threadId: null,
          runner: 'crew',
          crewId: crew.id,
          model: '',
          scheduleExpr: '',
          scheduleEnabled: false,
          status: 'idle',
          output: crewTask.output ?? null,
          error: null,
          lastRunAt: null,
          createdAt: crew.updatedAt || Date.now(),
          updatedAt: crew.updatedAt || Date.now(),
        })
      }
    }

    if (legacyCrewTasks.length === 0) return
    upsertMany(legacyCrewTasks)
  }, [crews, tasks, upsertMany])

  const crewsById = useMemo(() => new Map(crews.map((crew) => [crew.id, crew])), [crews])

  const ensureAllowedTaskFolder = async (workDir: string) => {
    const normalized = workDir.trim()
    if (!normalized || !isAbsolutePath(normalized)) return
    await safeInvokeVoid('fs_add_allowed_folder', { path: normalized })
  }

  const createTaskThread = (task: WorkTask, preserveCurrentThread = true): string => {
    const existingThreadId = task.threadId && threads.some((thread) => thread.id === task.threadId)
      ? task.threadId
      : null

    if (existingThreadId) {
      return existingThreadId
    }

    const previousActiveThreadId = activeThreadId
    const threadId = addThread(deriveTaskName(task))
    addChatMessage(threadId, {
      role: 'system',
      content: buildTaskThreadSummary(task),
      visibleInChat: true,
      timestamp: Date.now(),
    })
    updateTask(task.id, { threadId })

    if (preserveCurrentThread) {
      setActiveThread(previousActiveThreadId)
    }

    return threadId
  }

  const applyTaskWorkingFolder = async (task: WorkTask) => {
    const normalizedWorkDir = task.workDir.trim()
    if (normalizedWorkDir && isAbsolutePath(normalizedWorkDir)) {
      await ensureAllowedTaskFolder(normalizedWorkDir)
      setWorkingFolder(normalizedWorkDir)
      return
    }

    setWorkingFolder(null)
  }

  const pickWorkDir = async (): Promise<string | null> => {
    try {
      const selected = await open({
        directory: true,
        multiple: false,
      })
      return typeof selected === 'string' ? selected.trim() : null
    } catch {
      const path = window.prompt('Absoluten Ordnerpfad eingeben:')
      return path ? path.trim() : null
    }
  }

  const handlePickNewWorkDir = async () => {
    const selected = await pickWorkDir()
    if (selected) {
      setNewWorkDir(selected)
      await ensureAllowedTaskFolder(selected)
    }
  }

  const handlePickTaskWorkDir = async (task: WorkTask) => {
    const selected = await pickWorkDir()
    if (selected === null) return

    updateTask(task.id, { workDir: selected })
    if (isAbsolutePath(selected)) {
      await ensureAllowedTaskFolder(selected)
    }
  }

  const handleOpenTaskChat = async (task: WorkTask) => {
    const threadId = createTaskThread(task, false)
    await applyTaskWorkingFolder(task)
    setActiveMode('work')
    setActiveThread(threadId)
    navigate('/')
  }

  const handleCreateTask = () => {
    if (!canCreateTask) return

    const id = addTask({
      title: newTitle,
      prompt: newPrompt,
      expectedOutput: newExpectedOutput,
      workDir: normalizedNewWorkDir,
      runner: newRunner,
      crewId: newRunner === 'crew' ? newCrewId : null,
      model: newRunner === 'model' ? newModel : '',
    })

    const createdTask = useWorkTasksStore.getState().tasks.find((task) => task.id === id)
    if (createdTask) {
      void ensureAllowedTaskFolder(createdTask.workDir)
      createTaskThread(createdTask, true)
    }

    setNewTitle('')
    setNewPrompt('')
    setNewExpectedOutput('')
    setNewWorkDir('')
  }

  const handleRunTask = async (task: WorkTask) => {
    const normalizedWorkDir = task.workDir.trim()
    if (normalizedWorkDir && !isAbsolutePath(normalizedWorkDir)) {
      const message = 'Arbeitsordner muss absolut sein.'
      updateTask(task.id, {
        status: 'failed',
        error: message,
        output: message,
        lastRunAt: Date.now(),
      })
      return
    }

    const taskForRun = normalizedWorkDir ? { ...task, workDir: normalizedWorkDir } : task
    const threadId = createTaskThread(taskForRun, true)
    const startedAt = Date.now()

    updateTask(task.id, {
      status: 'running',
      output: '',
      error: null,
    })
    canceledTaskIdsRef.current.delete(task.id)
    const abortController = new AbortController()
    runningTaskControllersRef.current.set(task.id, abortController)

    await ensureAllowedTaskFolder(normalizedWorkDir)
    addChatMessage(threadId, {
      role: 'system',
      content: [
        'Task-Lauf gestartet',
        `Runner: ${task.runner === 'crew' ? 'Crew' : 'Modell'}`,
        normalizedWorkDir ? `Arbeitsordner: ${normalizedWorkDir}` : '',
      ].filter(Boolean).join('\n'),
      visibleInChat: true,
      timestamp: startedAt,
    })
    addChatMessage(threadId, {
      role: 'user',
      content: buildTaskPromptMessage(taskForRun),
      timestamp: startedAt,
    })

    if (task.runner === 'model') {
      const model = task.model.trim() || ollamaConfig.model
      const config = {
        ...ollamaConfig,
        model,
      }
      const assistantMessageId = addChatMessage(threadId, {
        role: 'assistant',
        content: '',
        timestamp: Date.now(),
        streaming: true,
      })

      try {
        let buffered = ''
        const response = await streamChatTurn(
          {
            prompt: task.prompt,
            history: normalizedWorkDir ? [{ role: 'system', content: `Arbeitsverzeichnis: ${normalizedWorkDir}` }] : [],
            config,
          },
          (chunk) => {
            if (abortController.signal.aborted) return
            buffered += chunk
            updateTask(task.id, { output: buffered })
            updateChatMessage(threadId, assistantMessageId, { content: buffered })
          },
          { signal: abortController.signal },
        )

        if (abortController.signal.aborted || canceledTaskIdsRef.current.has(task.id)) {
          const message = 'Task abgebrochen.'
          updateTask(task.id, {
            status: 'canceled',
            error: null,
            output: buffered || message,
            lastRunAt: Date.now(),
          })
          updateChatMessage(threadId, assistantMessageId, {
            content: buffered ? `${buffered}\n\n${message}` : message,
            streaming: false,
          }, {
            persist: true,
          })
          return
        }

        updateTask(task.id, {
          status: 'completed',
          output: response.assistantMessage,
          lastRunAt: Date.now(),
        })
        updateChatMessage(threadId, assistantMessageId, {
          content: response.assistantMessage,
          streaming: false,
        }, {
          persist: true,
        })
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error)
        const aborted = abortController.signal.aborted || canceledTaskIdsRef.current.has(task.id)
        updateTask(task.id, {
          status: aborted ? 'canceled' : 'failed',
          error: aborted ? null : message,
          output: aborted ? 'Task abgebrochen.' : message,
          lastRunAt: Date.now(),
        })
        updateChatMessage(threadId, assistantMessageId, {
          content: aborted ? 'Task abgebrochen.' : message,
          streaming: false,
        }, {
          persist: true,
        })
      } finally {
        runningTaskControllersRef.current.delete(task.id)
        canceledTaskIdsRef.current.delete(task.id)
      }

      return
    }

    const crewStreamId = createCrewStreamId()
    const streamedCrewLogIds = new Set<string>()
    let unlistenCrewLogs: (() => void) | null = null
    let crewLiveState: CrewLiveState = {
      streamId: crewStreamId,
      title: `${deriveTaskName(taskForRun)} - Crew-Ausfuehrung`,
      status: 'running',
      entries: [],
      agentColors: {},
      updatedAt: Date.now(),
    }
    const crewLiveMessageId = addChatMessage(threadId, {
      role: 'assistant',
      content: buildCrewLiveMessageContent(crewLiveState),
      timestamp: Date.now(),
      streaming: true,
      crewLive: crewLiveState,
    })
    const publishCrewLive = (persist = false) => {
      updateChatMessage(threadId, crewLiveMessageId, {
        content: buildCrewLiveMessageContent(crewLiveState),
        streaming: crewLiveState.status === 'running',
        crewLive: crewLiveState,
      }, {
        persist,
      })
    }
    const appendCrewLogToMonitor = (log: CrewExecutionLog) => {
      if (!log.id || streamedCrewLogIds.has(log.id)) return
      const entry = createCrewLiveEntry(log)
      if (!entry) return
      streamedCrewLogIds.add(log.id)
      crewLiveState = appendCrewLiveEntry(crewLiveState, entry)
      publishCrewLive()
    }
    const finishCrewLive = (status: CrewLiveStatus, persist = true) => {
      crewLiveState = {
        ...crewLiveState,
        status,
        updatedAt: Date.now(),
      }
      publishCrewLive(persist)
    }

    try {
      if (!task.crewId) {
        throw new Error('Bitte eine Crew auswaehlen.')
      }

      const crew = crewsById.get(task.crewId)
      if (!crew) {
        throw new Error('Crew nicht gefunden (evtl. geloescht).')
      }

      const enabledAgents = crew.agents.filter((agent) => agent.enabled)
      if (enabledAgents.length === 0) {
        throw new Error('Keine aktiven Crew-Mitglieder vorhanden.')
      }

      const enabledAgentIds = new Set(enabledAgents.map((agent) => agent.id))
      const runtimeTasks = buildCrewRuntimeTasks(crew, task, enabledAgentIds)

      const defaultOpenAICompatibleProfile = llmProfiles.find((profile) => profile.id === defaultLlmProfileIds['openai-compatible'] && profile.provider === 'openai-compatible')
        ?? llmProfiles.find((profile) => profile.provider === 'openai-compatible')
      const defaultOpenRouterProfile = llmProfiles.find((profile) => profile.id === defaultLlmProfileIds.openrouter && profile.provider === 'openrouter')
        ?? llmProfiles.find((profile) => profile.provider === 'openrouter')

      let providerConfigs = {
        openAICompatible: resolveExternalProviderConfig(
          crew.providerProfiles.openAICompatible,
          defaultOpenAICompatibleProfile,
          defaultOpenAICompatibleProfile?.baseUrl || crew.providerProfiles.openAICompatible.baseUrl || 'https://api.openai.com/v1',
        ),
        openRouter: resolveExternalProviderConfig(
          crew.providerProfiles.openRouter,
          defaultOpenRouterProfile,
          defaultOpenRouterProfile?.baseUrl || crew.providerProfiles.openRouter.baseUrl || 'https://openrouter.ai/api/v1',
        ),
      }

      let config = resolveCrewRuntimeConfig(crew, {
        baseUrl: ollamaConfig.baseUrl,
        model: ollamaConfig.model,
        timeoutMs: ollamaConfig.timeoutMs,
      })
      const appliedCrewDefault = applyCrewDefaultModel(crew, config, providerConfigs)
      config = appliedCrewDefault.config
      providerConfigs = appliedCrewDefault.providerConfigs
      const crewDefaultProvider = crew.defaultProvider ?? 'ollama'
      runningCrewTaskIdsRef.current.set(task.id, crew.id)

      try {
        unlistenCrewLogs = await listen<CrewExecutionLogEvent>('crew-execution-log', (event) => {
          const payload = event.payload
          if (!payload || payload.streamId !== crewStreamId) return
          appendCrewLogToMonitor(payload.log)
        })
      } catch {
        // In browser-only tests or fallback environments the Tauri event bus is unavailable.
      }

      const response = await safeInvoke<CrewExecutionResponse>('crew_execute', {
        request: {
          id: crew.id,
          streamId: crewStreamId,
          name: crew.name,
          description: crew.description,
          executionSubject: crew.executionSubject,
          executionGuidelines: buildWorkTaskCrewGuidelines(crew, taskForRun),
          knowledgeFocus: crew.knowledgeFocus,
          governanceMode: crew.governanceMode,
          outputMode: crew.outputMode,
          stopOnFailure: crew.stopOnFailure,
          retryCount: crew.retryCount,
          managerReviewEnabled: crew.managerReviewEnabled,
          managerReviewGuidelines: crew.managerReviewGuidelines,
          shareAllTaskOutputs: crew.shareAllTaskOutputs,
          sharedOutputCharLimit: crew.sharedOutputCharLimit,
          providerConfigs,
          process: crew.process,
          managerAgentId: crew.managerAgentId,
          verbose: crew.verbose,
          maxRpm: crew.maxRpm,
          maxParallelTasks: crew.maxParallelTasks,
          agents: enabledAgents.map((agent) => ({
            id: agent.id,
            name: agent.name,
            role: agent.role,
            goal: agent.goal,
            backstory: agent.backstory,
            skillsMarkdown: agent.skillsMarkdown,
            personalityId: agent.personalityId,
            modelOverride: agent.modelOverride?.trim() ? agent.modelOverride : null,
            providerKind: crewDefaultProvider,
            tools: agent.tools,
            mcpServerNames: agent.mcpServerNames,
            enabled: agent.enabled,
            allowDelegation: agent.allowDelegation,
            verbose: agent.verbose,
            maxIterations: agent.maxIterations,
          })),
          tasks: runtimeTasks,
          cwd: normalizedWorkDir || null,
          config,
        },
      })

      const mappedStatus = response.status === 'completed' ? 'completed' : 'failed'
      if (canceledTaskIdsRef.current.has(task.id) || response.status === 'canceled') {
        finishCrewLive('canceled')
        updateTask(task.id, {
          status: 'canceled',
          output: 'Task abgebrochen.',
          error: null,
          lastRunAt: Date.now(),
        })
        addChatMessage(threadId, {
          role: 'assistant',
          content: 'Task abgebrochen.',
          timestamp: Date.now(),
        })
        return
      }
      const output = buildCrewRunOutput(response, task.id)

      for (const log of response.logs) {
        appendCrewLogToMonitor(log)
      }
      finishCrewLive(mappedStatus === 'completed' ? 'completed' : 'failed')

      addChatMessage(threadId, {
        role: 'assistant',
        content: output,
        timestamp: Date.now(),
      })

      updateTask(task.id, {
        status: mappedStatus,
        output,
        error: response.error ?? null,
        lastRunAt: Date.now(),
      })
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      const aborted = canceledTaskIdsRef.current.has(task.id) || abortController.signal.aborted
      const waitingForApproval = message.trim().toLowerCase().startsWith('crew wartet auf freigabe:')
      finishCrewLive(aborted ? 'canceled' : 'failed')
      addChatMessage(threadId, {
        role: 'assistant',
        content: aborted ? 'Task abgebrochen.' : message,
        timestamp: Date.now(),
      })
      updateTask(task.id, {
        status: aborted ? 'canceled' : waitingForApproval ? 'waiting_approval' : 'failed',
        error: aborted ? null : message,
        output: aborted ? 'Task abgebrochen.' : message,
        lastRunAt: Date.now(),
      })
    } finally {
      unlistenCrewLogs?.()
      runningTaskControllersRef.current.delete(task.id)
      runningCrewTaskIdsRef.current.delete(task.id)
      canceledTaskIdsRef.current.delete(task.id)
    }
  }

  const handleCancelTask = async (task: WorkTask) => {
    canceledTaskIdsRef.current.add(task.id)
    runningTaskControllersRef.current.get(task.id)?.abort()
    const crewId = runningCrewTaskIdsRef.current.get(task.id)
    if (crewId) {
      await safeInvoke('crew_stop', { request: { crewId } }, null)
    }
    updateTask(task.id, {
      status: 'canceled',
      error: null,
      output: task.output?.trim() ? `${task.output}\n\nTask abgebrochen.` : 'Task abgebrochen.',
      lastRunAt: Date.now(),
    })
  }

  const handleUpsertSchedule = async (task: WorkTask) => {
    const scheduleExpr = task.scheduleExpr.trim()
    if (!scheduleExpr) {
      updateTask(task.id, { scheduleEnabled: false })
      return
    }

    const normalizedWorkDir = task.workDir.trim()
    if (normalizedWorkDir && !isAbsolutePath(normalizedWorkDir)) {
      updateTask(task.id, { scheduleEnabled: false })
      return
    }
    if (normalizedWorkDir) {
      await ensureAllowedTaskFolder(normalizedWorkDir)
    }

    let scheduled: ScheduledTask | null = findScheduledTask(scheduledTasks, task.id)

    if (task.runner === 'crew') {
      if (!task.crewId) {
        updateTask(task.id, { scheduleEnabled: false })
        return
      }

      const currentCrew = crewsById.get(task.crewId)
      if (!currentCrew) {
        updateTask(task.id, { scheduleEnabled: false })
        return
      }

      const { crew, metadata } = await resolveCrewScheduleSource(currentCrew)

      const enabledAgents = crew.agents.filter((agent) => agent.enabled)
      if (enabledAgents.length === 0) {
        updateTask(task.id, { scheduleEnabled: false })
        return
      }
      const enabledAgentIds = new Set(enabledAgents.map((agent) => agent.id))
      let runtimeTasks
      try {
        runtimeTasks = buildCrewRuntimeTasks(crew, task, enabledAgentIds)
      } catch {
        updateTask(task.id, { scheduleEnabled: false })
        return
      }

      const defaultOpenAICompatibleProfile = llmProfiles.find((profile) => profile.id === defaultLlmProfileIds['openai-compatible'] && profile.provider === 'openai-compatible')
        ?? llmProfiles.find((profile) => profile.provider === 'openai-compatible')
      const defaultOpenRouterProfile = llmProfiles.find((profile) => profile.id === defaultLlmProfileIds.openrouter && profile.provider === 'openrouter')
        ?? llmProfiles.find((profile) => profile.provider === 'openrouter')

      let providerConfigs = {
        openAICompatible: resolveExternalProviderConfig(
          crew.providerProfiles.openAICompatible,
          defaultOpenAICompatibleProfile,
          defaultOpenAICompatibleProfile?.baseUrl || crew.providerProfiles.openAICompatible.baseUrl || 'https://api.openai.com/v1',
        ),
        openRouter: resolveExternalProviderConfig(
          crew.providerProfiles.openRouter,
          defaultOpenRouterProfile,
          defaultOpenRouterProfile?.baseUrl || crew.providerProfiles.openRouter.baseUrl || 'https://openrouter.ai/api/v1',
        ),
      }

      let config = resolveCrewRuntimeConfig(crew, {
        baseUrl: ollamaConfig.baseUrl,
        model: ollamaConfig.model,
        timeoutMs: ollamaConfig.timeoutMs,
      })
      const appliedCrewDefault = applyCrewDefaultModel(crew, config, providerConfigs)
      config = appliedCrewDefault.config
      providerConfigs = appliedCrewDefault.providerConfigs
      const crewDefaultProvider = crew.defaultProvider ?? 'ollama'

      const crewSnapshotJson = JSON.stringify({
        id: crew.id,
        name: crew.name,
        description: crew.description,
        executionSubject: crew.executionSubject,
        executionGuidelines: buildWorkTaskCrewGuidelines(crew, task),
        knowledgeFocus: crew.knowledgeFocus,
        governanceMode: crew.governanceMode,
        outputMode: crew.outputMode,
        stopOnFailure: crew.stopOnFailure,
        retryCount: crew.retryCount,
        managerReviewEnabled: crew.managerReviewEnabled,
        managerReviewGuidelines: crew.managerReviewGuidelines,
        shareAllTaskOutputs: crew.shareAllTaskOutputs,
        sharedOutputCharLimit: crew.sharedOutputCharLimit,
        providerConfigs,
        process: crew.process,
        managerAgentId: crew.managerAgentId,
        verbose: crew.verbose,
        maxRpm: crew.maxRpm,
        maxParallelTasks: crew.maxParallelTasks,
        agents: enabledAgents.map((agent) => ({
          ...agent,
          modelOverride: agent.modelOverride?.trim() ? agent.modelOverride : null,
          providerKind: crewDefaultProvider,
        })),
        tasks: runtimeTasks,
        config,
        cwd: normalizedWorkDir || null,
        snapshotSource: metadata.snapshotSource,
        definitionVersionId: metadata.definitionVersionId,
        definitionVersionNumber: metadata.definitionVersionNumber,
        definitionChangeSummary: metadata.definitionChangeSummary,
        definitionSavedAt: metadata.definitionSavedAt,
      })

      scheduled = {
        id: task.id,
        name: deriveTaskName(task),
        prompt: task.prompt,
        cronLike: scheduleExpr,
        taskKind: 'crew',
        crewId: crew.id,
        crewSnapshotJson,
        modelConfigJson: null,
        priority: scheduled?.priority ?? 100,
        dependsOnTaskIds: scheduled?.dependsOnTaskIds ?? [],
        active: Boolean(task.scheduleEnabled),
        lastRunAt: scheduled?.lastRunAt ?? null,
        nextRunAt: scheduled?.nextRunAt ?? null,
      }
    } else {
      scheduled = {
        id: task.id,
        name: deriveTaskName(task),
        prompt: task.prompt,
        cronLike: scheduleExpr,
        taskKind: 'prompt',
        crewId: null,
        crewSnapshotJson: null,
        modelConfigJson: JSON.stringify({
          ...ollamaConfig,
          model: task.model.trim() || ollamaConfig.model,
          cwd: normalizedWorkDir || null,
        }),
        priority: scheduled?.priority ?? 100,
        dependsOnTaskIds: scheduled?.dependsOnTaskIds ?? [],
        active: Boolean(task.scheduleEnabled),
        lastRunAt: scheduled?.lastRunAt ?? null,
        nextRunAt: scheduled?.nextRunAt ?? null,
      }
    }

    await upsertScheduledTask(scheduled)
  }

  const handleToggleSchedule = async (task: WorkTask, enabled: boolean) => {
    updateTask(task.id, { scheduleEnabled: enabled })
    const scheduled = findScheduledTask(scheduledTasks, task.id)
    if (scheduled) {
      await toggleScheduledTask(task.id, enabled)
    }
  }

  const handleRemoveSchedule = async (task: WorkTask) => {
    updateTask(task.id, { scheduleEnabled: false, scheduleExpr: '' })
    const scheduled = findScheduledTask(scheduledTasks, task.id)
    if (scheduled) {
      await removeScheduledTask(task.id)
    }
  }

  const handleRemoveLegacyTemplate = (templateId: string) => {
    // Templates are legacy; keep deletion available so users can clean up old storage.
    removeTemplate(templateId)
  }

  return (
    <div className="settings-view">
      <h1>Tasks</h1>
      <p className="hint-text">Tasks erstellen, Crew oder Modell zuordnen, starten und pro Task schedulen.</p>

      <div className="panel">
        <h2>➕ Neuer Task</h2>
        <div className="grid">
          <label>
            Titel (optional)
            <input value={newTitle} onChange={(e) => setNewTitle(e.target.value)} placeholder="z.B. Weekly Report" />
          </label>
          <label>
            Ausfuehrung
            <select value={newRunner} onChange={(e) => setNewRunner(e.target.value as WorkTaskRunner)}>
              <option value="crew">Crew</option>
              <option value="model">Modell</option>
            </select>
          </label>
          {newRunner === 'crew' ? (
            <label>
              Crew
              <select value={newCrewId} onChange={(e) => setNewCrewId(e.target.value)}>
                {crews.length === 0 && (
                  <option value="">Keine Crews vorhanden</option>
                )}
                {crews.map((crew) => (
                  <option key={crew.id} value={crew.id}>{crew.name}</option>
                ))}
              </select>
            </label>
          ) : (
            <label>
              Modell (optional)
              <input value={newModel} onChange={(e) => setNewModel(e.target.value)} placeholder={`Default: ${ollamaConfig.model || '—'}`} />
            </label>
          )}
          <label>
            Expected Output (optional)
            <input value={newExpectedOutput} onChange={(e) => setNewExpectedOutput(e.target.value)} placeholder="z.B. Bullet-Report" />
          </label>
          <label style={{ gridColumn: '1 / -1' }}>
            Arbeitsordner (optional, absolut)
            <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
              <input value={newWorkDir} onChange={(e) => setNewWorkDir(e.target.value)} placeholder="C:\\Projekte\\mein-task" />
              <button type="button" className="btn-secondary" onClick={() => void handlePickNewWorkDir()}>
                Ordner waehlen
              </button>
            </div>
            {normalizedNewWorkDir && !isAbsolutePath(normalizedNewWorkDir) ? (
              <div className="hint-text">Der Arbeitsordner muss absolut sein.</div>
            ) : null}
          </label>
          <label style={{ gridColumn: '1 / -1' }}>
            Aufgabe
            <textarea value={newPrompt} onChange={(e) => setNewPrompt(e.target.value)} rows={4} placeholder="Was soll der Task tun?" />
          </label>
        </div>
        <div className="actions">
          <button type="button" onClick={handleCreateTask} disabled={!canCreateTask}>
            Task erstellen
          </button>
        </div>
        {newRunner === 'crew' && crews.length === 0 && (
          <p className="hint-text">Erstelle zuerst eine Crew in den Einstellungen, um Crew-Tasks auszufuehren.</p>
        )}
      </div>

      <div className="panel">
        <div className="panel-heading-row">
          <h2>🧩 Deine Tasks</h2>
          <span className="hint-text">{tasks.length} Task(s)</span>
        </div>

        {tasks.length === 0 ? (
          <p className="hint-text">Noch keine Tasks. Erstelle oben deinen ersten Task.</p>
        ) : (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
            {tasks.map((task) => {
              const scheduled = findScheduledTask(scheduledTasks, task.id)
              const crewName = task.crewId ? crewsById.get(task.crewId)?.name : null
              const crewScheduleMetadata = task.runner === 'crew'
                ? readCrewScheduleSnapshotMetadata(scheduled?.crewSnapshotJson)
                : null

              return (
                <div key={task.id} className="card">
                  <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 10 }}>
                    <div style={{ display: 'flex', alignItems: 'center', gap: 8, flexWrap: 'wrap' }}>
                      <strong>{deriveTaskName(task)}</strong>
                      <span style={{ fontSize: 11, padding: '1px 6px', borderRadius: 8, background: 'var(--accent)', color: '#fff' }}>
                        {task.runner === 'crew' ? 'Crew' : 'Modell'}
                      </span>
                      <span style={{ fontSize: 11, padding: '1px 6px', borderRadius: 8, background: task.status === 'completed' ? 'var(--success)' : task.status === 'failed' ? 'var(--danger)' : task.status === 'running' ? 'var(--accent)' : task.status === 'waiting_approval' ? 'var(--warning)' : task.status === 'canceled' ? 'var(--text-muted)' : 'var(--border-color)', color: task.status === 'idle' ? 'var(--text-secondary)' : '#fff' }}>
                        {task.status}
                      </span>
                    </div>
                    <div className="actions" style={{ marginTop: 0 }}>
                      <button type="button" onClick={() => void handleOpenTaskChat(task)}>
                        Chat
                      </button>
                      <button type="button" onClick={() => void handleRunTask(task)} disabled={(task.status === 'running' || task.status === 'waiting_approval') || !task.prompt.trim() || (task.runner === 'crew' && !task.crewId) || Boolean(task.workDir.trim() && !isAbsolutePath(task.workDir))}>
                        Start
                      </button>
                      {task.status === 'running' && (
                        <button type="button" className="btn-stop" onClick={() => void handleCancelTask(task)}>
                          Stopp
                        </button>
                      )}
                      <button type="button" className="btn-secondary" onClick={() => removeTask(task.id)} disabled={task.status === 'running'}>
                        Loeschen
                      </button>
                    </div>
                  </div>

                  <div className="grid" style={{ marginTop: 10 }}>
                    <label>
                      Titel
                      <input value={task.title} onChange={(e) => updateTask(task.id, { title: e.target.value })} />
                    </label>
                    <label>
                      Ausfuehrung
                      <select value={task.runner} onChange={(e) => updateTask(task.id, { runner: e.target.value as WorkTaskRunner })}>
                        <option value="crew">Crew</option>
                        <option value="model">Modell</option>
                      </select>
                    </label>
                    {task.runner === 'crew' ? (
                      <label>
                        Crew
                        <select value={task.crewId ?? ''} onChange={(e) => updateTask(task.id, { crewId: e.target.value || null })}>
                          <option value="">Crew waehlen</option>
                          {crews.map((crew) => (
                            <option key={crew.id} value={crew.id}>{crew.name}</option>
                          ))}
                        </select>
                        {task.crewId && !crewName ? (
                          <div className="hint-text">Zugeordnete Crew existiert nicht mehr.</div>
                        ) : null}
                      </label>
                    ) : (
                      <label>
                        Modell (optional)
                        <input
                          value={task.model}
                          onChange={(e) => updateTask(task.id, { model: e.target.value })}
                          placeholder={`Default: ${ollamaConfig.model || '—'}`}
                        />
                      </label>
                    )}
                    <label>
                      Expected Output
                      <input value={task.expectedOutput} onChange={(e) => updateTask(task.id, { expectedOutput: e.target.value })} />
                    </label>
                    <label style={{ gridColumn: '1 / -1' }}>
                      Arbeitsordner (absolut)
                      <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
                        <input value={task.workDir} onChange={(e) => updateTask(task.id, { workDir: e.target.value })} placeholder="C:\\Projekte\\mein-task" />
                        <button type="button" className="btn-secondary" onClick={() => void handlePickTaskWorkDir(task)}>
                          Ordner waehlen
                        </button>
                      </div>
                      {task.workDir.trim() && !isAbsolutePath(task.workDir) ? (
                        <div className="hint-text">Der Arbeitsordner muss absolut sein.</div>
                      ) : null}
                    </label>
                    <label style={{ gridColumn: '1 / -1' }}>
                      Aufgabe
                      <textarea value={task.prompt} onChange={(e) => updateTask(task.id, { prompt: e.target.value })} rows={3} />
                    </label>
                  </div>

                  <div className="card" style={{ marginTop: 10 }}>
                    <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 10 }}>
                      <strong>⏰ Scheduler</strong>
                      <div style={{ fontSize: 11, color: 'var(--text-muted)' }}>
                        Letzter Lauf: {formatTimestamp(scheduled?.lastRunAt ?? null)} · Naechster Lauf: {formatTimestamp(scheduled?.nextRunAt ?? null)}
                      </div>
                    </div>

                    <div className="grid" style={{ marginTop: 8 }}>
                      <label>
                        Ausdruck
                        <input
                          value={task.scheduleExpr}
                          onChange={(e) => updateTask(task.id, { scheduleExpr: e.target.value })}
                          placeholder="z.B. daily 09:00"
                        />
                      </label>
                      <label style={{ display: 'flex', flexDirection: 'column' }}>
                        Aktiv
                        <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginTop: 6 }}>
                          <input
                            type="checkbox"
                            checked={task.scheduleEnabled}
                            onChange={(e) => void handleToggleSchedule(task, e.target.checked)}
                          />
                          <span className="hint-text">Job {task.scheduleEnabled ? 'aktiv' : 'pausiert'}</span>
                        </div>
                      </label>
                    </div>
                    <div className="actions" style={{ marginTop: 10 }}>
                      <button type="button" className="btn-sm" onClick={() => void handleUpsertSchedule(task)} disabled={!task.scheduleExpr.trim()}>
                        Speichern
                      </button>
                      <button type="button" className="btn-sm" onClick={() => void handleRemoveSchedule(task)} disabled={!scheduled && !task.scheduleExpr.trim()}>
                        Entfernen
                      </button>
                      {task.runner === 'crew' && !task.crewId ? (
                        <span className="hint-text">(Crew erforderlich fuer Crew-Schedule)</span>
                      ) : null}
                    </div>
                    {task.runner === 'crew' && crewScheduleMetadata ? (
                      <div className="hint-text" style={{ marginTop: 8 }}>
                        {crewScheduleMetadata.snapshotSource === 'saved-version'
                          ? `Quelle: gespeicherte Crew-Version v${crewScheduleMetadata.definitionVersionNumber ?? '—'}${crewScheduleMetadata.definitionSavedAt ? ` vom ${new Date(crewScheduleMetadata.definitionSavedAt).toLocaleString('de-DE')}` : ''}${crewScheduleMetadata.definitionChangeSummary ? ` · ${crewScheduleMetadata.definitionChangeSummary}` : ''}`
                          : 'Quelle: aktueller Crew-Editor-Stand'}
                      </div>
                    ) : null}
                  </div>

                  {(task.output || task.error) && (
                    <pre style={{ whiteSpace: 'pre-wrap', marginTop: 10, fontSize: 12 }}>
                      {(task.error ?? task.output ?? '').slice(0, 6000)}
                    </pre>
                  )}
                </div>
              )
            })}
          </div>
        )}
      </div>

      {templates.length > 0 && (
        <div className="panel">
          <div className="panel-heading-row">
            <h2>📦 Legacy: Templates</h2>
            <span className="hint-text">{templates.length} Template(s) im alten Speicher</span>
          </div>
          <p className="hint-text">Diese Templates werden nicht mehr aktiv genutzt. Du kannst sie hier bei Bedarf aufraeumen.</p>

          <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
            {templates.map((template) => (
              <div key={template.id} className="card">
                <strong>{template.title?.trim() ? template.title : template.id}</strong>
                <div style={{ fontSize: 12, color: 'var(--text-secondary)', marginTop: 4 }}>{template.description}</div>
                <div className="actions" style={{ marginTop: 10 }}>
                  <button type="button" className="btn-secondary" onClick={() => handleRemoveLegacyTemplate(template.id)}>
                    Loeschen
                  </button>
                </div>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  )
}
