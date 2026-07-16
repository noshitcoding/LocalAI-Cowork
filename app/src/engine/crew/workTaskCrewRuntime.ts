import type { CrewLiveEntry, CrewLiveEntryCategory, CrewLiveState } from '../../stores/chatStore'
import type { Crew, CrewProviderKind } from '../../stores/crewStore'
import type { WorkTask } from '../../stores/workTasksStore'

export type CrewExecutionLog = {
  id: string
  crewId: string
  agentId: string
  taskId: string
  action: string
  result: string
  timestamp: number
  agentName?: string | null
  sourceAgent?: string | null
  targetAgent?: string | null
  provider?: string | null
  model?: string | null
  taskTitle?: string | null
  phase?: string | null
  summary?: string | null
  detail?: string | null
  severity?: 'info' | 'warning' | 'error' | null
  providerReasoning?: string | null
}

export type CrewExecutionLogEvent = {
  streamId?: string | null
  runId?: string | null
  log: CrewExecutionLog
}

export type CrewExecutionResponse = {
  crewId: string
  status: 'idle' | 'running' | 'completed' | 'failed' | 'canceled'
  taskResults: Array<{ taskId: string; agentId: string; status: string; output: string | null }>
  logs: CrewExecutionLog[]
  error: string | null
}

export type CrewResolvedProviderConfigs = {
  openAICompatible: { baseUrl: string; model: string; apiKey: string; timeoutMs: number } | undefined
  openRouter: { baseUrl: string; model: string; apiKey: string; timeoutMs: number } | undefined
}

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
const CREW_BOX_CHARS = /[\s\u2500-\u257F]+/u
const CREW_BOX_EDGE_START = /^[\s\u2500-\u257F]+/u
const CREW_BOX_EDGE_END = /[\s\u2500-\u257F]+$/u
const FRESH_NEWS_TASK_PATTERN = /\b(news|nachrichten|latest|last\s+\d+\s+hours?|last\s+24\s+hours|today|heute|aktuell|tagesbericht|daily\s+news|breaking)\b/i
const RESEARCH_TASK_PATTERN = /\b(research|recherche|recherchier\w*|sources?|quellen?|literature\s+review|literatur(?:recherche|uebersicht)|market\s+research|marktanalyse|fact\s*check|faktencheck)\b/i
const CODING_TASK_PATTERN = /\b(code|coding|program\w*|programmier\w*|implement\w*|refactor\w*|debug\w*|bug(?:fix)?|fix\w*|tests?|typescript|javascript|python|rust|repository|repo|codebase|source\s*code|quellcode)\b/i
const PRESENTATION_TASK_PATTERN = /\b(power\s*point|presentation|praesentation|präsentation|pptx?|slides?|folien?|slide\s+deck|pitch\s+deck|ppp)\b/i

function getTaskSearchText(task: Pick<WorkTask, 'title' | 'prompt' | 'expectedOutput'>): string {
  return [task.title, task.prompt, task.expectedOutput].join('\n')
}

function getLocalTimezone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || 'local'
  } catch {
    return 'local'
  }
}

function buildCurrentRunContext(now = new Date()): string {
  return [
    `Current run date: ${now.toLocaleDateString('en-CA')}`,
    `Current run time: ${now.toISOString()}`,
    `Local timezone: ${getLocalTimezone()}`,
  ].join('\n')
}

export function isFreshNewsTask(task: Pick<WorkTask, 'title' | 'prompt' | 'expectedOutput'>): boolean {
  return FRESH_NEWS_TASK_PATTERN.test(getTaskSearchText(task))
}

export function isResearchTask(task: Pick<WorkTask, 'title' | 'prompt' | 'expectedOutput'>): boolean {
  return isFreshNewsTask(task) || RESEARCH_TASK_PATTERN.test(getTaskSearchText(task))
}

export function isCodingTask(task: Pick<WorkTask, 'title' | 'prompt' | 'expectedOutput'>): boolean {
  return CODING_TASK_PATTERN.test(getTaskSearchText(task))
}

export function isPresentationTask(task: Pick<WorkTask, 'title' | 'prompt' | 'expectedOutput'>): boolean {
  return PRESENTATION_TASK_PATTERN.test(getTaskSearchText(task))
}

function buildFreshNewsGuidelines(task: WorkTask): string {
  if (!isFreshNewsTask(task)) return ''

  return [
    'Fresh-news requirements:',
    '- Treat "today", "latest", and "last 24 hours" relative to the current run date/time above.',
    '- Use web_search to discover current sources before web_fetch; do not rely on model memory for news.',
    '- Include source URLs and publication dates for every factual news item.',
    '- If web_search or web_fetch is unavailable, say the report cannot be verified instead of inventing news.',
    '- Exclude items older than the requested freshness window unless explicitly labeled as background.',
  ].join('\n')
}

export function augmentCrewToolsForTask(tools: string[], task: WorkTask): string[] {
  const augmented = [...tools]
  if (isResearchTask(task)) {
    augmented.push('web_search', 'web_fetch')
  }
  if (isCodingTask(task)) {
    augmented.push('read_file', 'glob', 'grep', 'edit_file', 'create_directory', 'bash')
  }
  if (isPresentationTask(task)) {
    augmented.push('read_file', 'create_directory', 'office_workflow')
  }
  return Array.from(new Set(augmented))
}

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

export function resolveCrewRuntimeConfig(crew: Crew, fallbackConfig: { baseUrl: string; model: string; timeoutMs: number }) {
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

export function resolveExternalProviderConfig(
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

export function applyCrewDefaultModel(
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

function deriveTaskName(task: WorkTask): string {
  const title = task.title.trim()
  if (title) return title
  const prompt = task.prompt.trim()
  if (!prompt) return task.id
  const singleLine = prompt.replace(/\s+/g, ' ').trim()
  return singleLine.length > 48 ? `${singleLine.slice(0, 48)}...` : singleLine
}

export function buildWorkTaskCrewGuidelines(crew: Crew, task: WorkTask): string {
  const workTaskContext = [
    buildCurrentRunContext(),
    `Work task request: ${deriveTaskName(task)}`,
    task.prompt.trim(),
    task.expectedOutput.trim() ? `Expected overall result:\n${task.expectedOutput.trim()}` : '',
    buildFreshNewsGuidelines(task),
  ].filter(Boolean).join('\n\n')

  return [
    crew.executionGuidelines.trim(),
    workTaskContext,
  ].filter(Boolean).join('\n\n')
}

export function buildCrewRuntimeTasks(crew: Crew, task: WorkTask, enabledAgentIds: Set<string>) {
  const crewTasks = crew.tasks ?? []
  const runnableCrewTasks = crewTasks.filter((crewTask) => enabledAgentIds.has(crewTask.agentId))

  if (crewTasks.length > 0 && runnableCrewTasks.length === 0) {
    throw new Error('No executable crew tasks are available: all assigned crew members are disabled or missing.')
  }

  if (runnableCrewTasks.length > 0) {
    const runnableTaskIds = new Set(runnableCrewTasks.map((crewTask) => crewTask.id))
    return runnableCrewTasks.map((crewTask, index) => ({
      id: crewTask.id,
      description: crewTask.description,
      expectedOutput: crewTask.expectedOutput || task.expectedOutput || 'Create a complete result.',
      agentId: crewTask.agentId,
      context: crewTask.context.filter((contextId) => runnableTaskIds.has(contextId)),
      dependencies: crewTask.dependencies.filter((dependencyId) => runnableTaskIds.has(dependencyId)),
      asyncExecution: crew.process === 'parallel'
        ? index < runnableCrewTasks.length - 1
        : crewTask.asyncExecution,
    }))
  }

  const agentId = resolveDefaultAgentId(crew)
  if (!agentId) {
    throw new Error('Crew has no agent.')
  }

  return [
    {
      id: task.id,
      description: task.prompt,
      expectedOutput: task.expectedOutput || 'Create a complete result.',
      agentId,
      context: [],
      dependencies: [],
      asyncExecution: false,
    },
  ]
}

function getCrewLogActionLabel(action: string): string {
  switch (action) {
    case 'run_started': return 'Run started'
    case 'runtime_context': return 'Runtime context'
    case 'agent_ready': return 'Agent ready'
    case 'task_handoff': return 'Handoff'
    case 'crew_kickoff': return 'CrewAI started'
    case 'runtime_stdout': return 'Runtime output'
    case 'runtime_stderr': return 'Runtime error output'
    case 'crew_finished': return 'CrewAI completed'
    case 'task_completed': return 'Task completed'
    case 'runtime_failed': return 'Runtime failed'
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
  const rawDetail = log.detail?.trim() ? log.detail : log.result
  const lines = rawDetail
    .replace(/\r/g, '\n')
    .split('\n')
    .map(stripCrewRuntimeChrome)
    .filter((line) => line.trim().length > 0)

  return lines.join('\n').trim()
}

function normalizeCrewLivePhase(value: string | null | undefined): CrewLiveEntryCategory | null {
  const normalized = value?.trim().toLowerCase()
  if (!normalized) return null
  if (normalized === 'reasoning') return 'thinking'
  if (normalized === 'handoff') return 'handoff'
  if (normalized === 'delegation') return 'delegation'
  if (normalized === 'tool') return 'tool'
  if (normalized === 'mcp') return 'mcp'
  if (normalized === 'result') return 'result'
  if (normalized === 'task') return 'task'
  if (normalized === 'error') return 'error'
  if (normalized === 'agent') return 'agent'
  if (normalized === 'context') return 'context'
  if (normalized === 'status') return 'status'
  if (normalized === 'output') return 'output'
  return null
}

function classifyCrewLog(log: CrewExecutionLog, detail: string): CrewLiveEntryCategory {
  const structuredPhase = normalizeCrewLivePhase(log.phase)
  if (structuredPhase) {
    return structuredPhase
  }

  const combined = `${log.action}\n${detail}`.toLowerCase()

  if (combined.includes('traceback') || combined.includes('error') || combined.includes('failed') || log.action.includes('stderr') || log.action.includes('failed')) {
    return 'error'
  }
  if (combined.includes('thinking') || combined.includes('reasoning') || combined.includes('work process')) {
    return 'thinking'
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
    return 'result'
  }
  if (log.action === 'run_started' || log.action === 'crew_kickoff' || log.action === 'crew_finished') {
    return 'status'
  }

  return 'output'
}

function firstMatchingLine(detail: string, pattern: RegExp): string | null {
  return detail.split('\n').map((line) => line.trim()).find((line) => pattern.test(line)) ?? null
}

function parseAgentNameFromDetail(detail: string): string | null {
  const agentLine = firstMatchingLine(detail, /^Agent:\s*(.+)$/i)
  if (agentLine) return agentLine.replace(/^Agent:\s*/i, '').trim()

  const nameMatch = detail.match(/(?:^|\n)Name:\s*([^|\n]+)/i)
  if (nameMatch?.[1]) return nameMatch[1].trim()

  const handoffMatch = detail.match(/Task an Agent uebergeben:\s*([^\n]+)/i)
  if (handoffMatch?.[1]) return handoffMatch[1].trim()

  return null
}

function getDisplayAgentName(log: CrewExecutionLog, detail: string): string {
  const structuredName = log.agentName?.trim()
  if (structuredName) return structuredName
  const parsedName = parseAgentNameFromDetail(detail)
  if (parsedName) return parsedName
  return log.agentId || 'Runtime'
}

function buildCrewLiveTitle(log: CrewExecutionLog, category: CrewLiveEntryCategory, detail: string): string {
  const summary = log.summary?.trim()
  if (summary) return summary

  if (category === 'tool') {
    return firstMatchingLine(detail, /^Tool:/i) ?? 'Tool-Execution'
  }
  if (category === 'delegation') {
    if (log.sourceAgent?.trim() && log.targetAgent?.trim()) {
      return `${log.sourceAgent.trim()} -> ${log.targetAgent.trim()}`
    }
    return log.targetAgent?.trim() ? `Delegation an ${log.targetAgent.trim()}` : 'Delegation an Crew-Mitglied'
  }
  if (category === 'mcp') {
    return 'MCP context or MCP access'
  }
  if (category === 'thinking') {
    return `Work process: ${getDisplayAgentName(log, detail)}`
  }
  if (category === 'handoff') {
    return `Task-Handoff an ${log.targetAgent?.trim() || getDisplayAgentName(log, detail)}`
  }
  if (category === 'agent') {
    return firstMatchingLine(detail, /^Agent:/i) ?? `${getDisplayAgentName(log, detail)} ready`
  }
  if (category === 'task') {
    return 'Task result received'
  }
  if (category === 'result') {
    return log.taskTitle?.trim() ? `Result: ${log.taskTitle.trim()}` : 'Task result received'
  }
  if (category === 'error') {
    return 'Crew error'
  }
  if (category === 'context') {
    return 'Runtime context loaded'
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

function looksLikeTechnicalAgentId(value: string): boolean {
  const normalized = value.trim().toLowerCase()
  return /(?:^|-)pers-\d{8,}/.test(normalized)
    || normalized.startsWith('personality-pers-')
    || normalized.startsWith('agent-personality-pers-')
    || normalized.startsWith('python-runtime')
    || normalized.startsWith('crew-runtime')
}

function deriveCrewLiveAgentId(log: CrewExecutionLog, detail: string): string {
  const displayAgent = log.targetAgent?.trim() || log.agentName?.trim() || parseAgentNameFromDetail(detail)
  if (displayAgent && !looksLikeTechnicalAgentId(displayAgent)) {
    const agent = normalizeCrewAgentLabel(displayAgent)
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

  if (log.agentId && !looksLikeTechnicalAgentId(log.agentId)) {
    return log.agentId
  }

  return displayAgent ? normalizeCrewAgentLabel(displayAgent) || log.agentId || 'runtime' : log.agentId || 'runtime'
}

function getLogDetailWithMetadata(log: CrewExecutionLog, baseDetail: string): string {
  const metadataLines = [
    log.provider?.trim() ? `Provider: ${log.provider.trim()}` : '',
    log.model?.trim() ? `Model: ${log.model.trim()}` : '',
    log.taskTitle?.trim() ? `Task: ${log.taskTitle.trim()}` : '',
    log.sourceAgent?.trim() ? `Source: ${log.sourceAgent.trim()}` : '',
    log.targetAgent?.trim() ? `Target: ${log.targetAgent.trim()}` : '',
    log.agentId?.trim() ? `Technical ID: ${log.agentId.trim()}` : '',
    log.providerReasoning?.trim() ? `Provider-Reasoning: ${log.providerReasoning.trim()}` : '',
  ].filter(Boolean)

  const merged = [...metadataLines, baseDetail].filter(Boolean).join('\n')
  return merged.trim()
}

export function createCrewLiveEntry(log: CrewExecutionLog): CrewLiveEntry | null {
  const baseDetail = cleanCrewLogDetail(log)
  const detail = getLogDetailWithMetadata(log, baseDetail)
  if (!detail && (log.action === 'runtime_stdout' || log.action === 'runtime_stderr')) {
    return null
  }

  const category = classifyCrewLog(log, detail)
  return {
    id: log.id,
    timestamp: log.timestamp || Date.now(),
    agentId: deriveCrewLiveAgentId(log, detail),
    rawAgentId: log.agentId || null,
    taskId: log.taskId || 'runtime',
    action: log.action,
    category,
    title: buildCrewLiveTitle(log, category, detail),
    detail,
    agentName: log.agentName?.trim() || parseAgentNameFromDetail(detail),
    sourceAgent: log.sourceAgent ?? null,
    targetAgent: log.targetAgent ?? null,
    rawTargetAgentId: null,
    provider: log.provider ?? null,
    model: log.model ?? null,
    taskTitle: log.taskTitle ?? null,
    phase: log.phase ?? null,
    summary: log.summary ?? null,
    severity: log.severity ?? (category === 'error' ? 'error' : 'info'),
    providerReasoning: log.providerReasoning ?? null,
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

export function appendCrewLiveEntry(state: CrewLiveState, entry: CrewLiveEntry): CrewLiveState {
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
    entries,
    updatedAt: Date.now(),
  }
}

export function buildCrewLiveMessageContent(state: CrewLiveState): string {
  const latest = state.entries[state.entries.length - 1]
  return [
    'Crew Live Monitor',
    `Status: ${state.status}`,
    `Displayed events: ${state.entries.length}`,
    latest ? `Letztes Ereignis: ${latest.title}` : '',
  ].filter(Boolean).join('\n')
}
