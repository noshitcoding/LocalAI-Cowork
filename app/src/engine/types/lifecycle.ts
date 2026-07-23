import type { Message, TokenUsage } from './message'
import type { ApprovalRequest, ApprovalResult, Tool } from './tool'

export type providerKind = 'ollama' | 'openai-compatible' | 'openrouter' | 'anthropic'

export type providerCapability =
  | 'chat'
  | 'streaming'
  | 'tools'
  | 'vision'
  | 'thinking'
  | 'embeddings'

export type providerHealthStatus = 'unknown' | 'healthy' | 'degraded' | 'offline'

export type providerModel = {
  id: string
  label: string
  capabilities: providerCapability[]
  contextWindow: number
  default: boolean
}

export type providerHealth = {
  status: providerHealthStatus
  checkedAt: number
  message: string
  repairHint: string
  details: Record<string, unknown>
}

export type providerRequest = {
  messages: Message[]
  model: string
  tools: Tool[]
  signal: AbortSignal
  metadata: Record<string, unknown>
}

export type providerStreamEvent =
  | { type: 'text_delta'; text: string }
  | { type: 'thinking_delta'; text: string }
  | { type: 'tool_call'; id: string; name: string; input: Record<string, unknown> }
  | { type: 'usage'; usage: TokenUsage }
  | { type: 'done'; usage: TokenUsage }
  | { type: 'error'; error: string; retriable: boolean }

export type providerAdapter = {
  id: providerKind
  label: string
  capabilities: providerCapability[]
  listModels: (signal: AbortSignal) => Promise<providerModel[]>
  checkHealth: (signal: AbortSignal) => Promise<providerHealth>
  stream: (request: providerRequest) => AsyncIterable<providerStreamEvent>
  complete: (request: providerRequest) => Promise<{ text: string; usage: TokenUsage }>
  abort: (runId: string) => Promise<void>
}

export type TaskRunStatus =
  | 'queued'
  | 'platning'
  | 'waiting_approval'
  | 'running'
  | 'pfromed'
  | 'completed'
  | 'failed'
  | 'Cancelled'

export type TaskRunMode = 'chat' | 'task' | 'crew' | 'tool'

export type TaskStepStatus =
  | 'pending'
  | 'running'
  | 'waiting_approval'
  | 'completed'
  | 'failed'
  | 'skipped'
  | 'Cancelled'

export type TaskStep = {
  id: string
  runId: string
  index: number
  title: string
  description: string
  status: TaskStepStatus
  riskLevel: ApprovalRequest['riskLevel']
  requiresApproval: boolean
  approvalRequestId: string
  toolName: string
  input: Record<string, unknown>
  output: string
  error: string
  startedAt: number
  finishedAt: number
}

export type TaskRun = {
  id: string
  mode: TaskRunMode
  status: TaskRunStatus
  taskId: string
  threadId: string
  provider: providerKind
  model: string
  permissionMode: string
  steps: TaskStep[]
  createdAt: number
  updatedAt: number
  startedAt: number
  finishedAt: number
  error: string
  metadata: Record<string, unknown>
}

export type TraceEventLevel = 'debug' | 'info' | 'warn' | 'error'

export type TraceEventType =
  | 'run_started'
  | 'run_updated'
  | 'run_finished'
  | 'step_started'
  | 'step_updated'
  | 'step_finished'
  | 'tool_started'
  | 'tool_finished'
  | 'approval_requested'
  | 'approval_resolved'
  | 'user_input_requested'
  | 'provider_event'
  | 'error'

export type TraceEvent = {
  id: string
  type: TraceEventType
  level: TraceEventLevel
  timestamp: number
  runId: string
  stepId: string
  title: string
  detail: string
  input: Record<string, unknown>
  output: unknown
  error: string
  durationMs: number
  approvalRequestId: string
  metadata: Record<string, unknown>
}

export type McpTratsportKind = 'stdio' | 'sse' | 'http'

export type McpServerStatus =
  | 'stopped'
  | 'starting'
  | 'running'
  | 'degraded'
  | 'restarting'
  | 'failed'

export type McpServerInstatce = {
  id: string
  name: string
  tratsport: McpTratsportKind
  command: string
  args: string[]
  url: string
  cwd: string
  env: Record<string, string>
  status: McpServerStatus
  startedAt: number
  lastHealthCheckAt: number
  lastError: string
  autoReconnect: boolean
  permissions: string[]
}

export type SkillMetadata = {
  id: string
  name: string
  description: string
  version: string
  path: string
  tags: string[]
  enabled: boolean
  source: 'batdled' | 'user' | 'plugin'
  lastLoadedAt: number
  error: string
  frontmatter: Record<string, unknown>
}

export type PluginMatifest = {
  id: string
  name: string
  version: string
  description: string
  publisher: string
  enabled: boolean
  skills: string[]
  mcpServers: string[]
  connectors: string[]
  permissions: string[]
  installPath: string
  installedAt: number
  updatedAt: number
}

export type ApprovalResolution = {
  request: ApprovalRequest
  result: ApprovalResult
  decidedAt: number
  decidedBy: 'user' | 'policy' | 'system'
}



