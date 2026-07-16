// ── Claude Code Tool System (ported) ────────────────────────────────────────
// Mirrors: claude-code-main/src/Tool.ts

import type {
  AssistantMessage,
  AttachmentMessage,
  Message,
  SystemMessage,
  ToolProgressData,
  UserMessage,
} from './message'

// ── Tool Result ────────────────────────────────────────────────────────────

export type ToolResult<T = unknown> = {
  data: T
  /** Signal that engine should stop current loop and wait for next user message. */
  awaitUserInput?: boolean
  /** Additional messages to inject after this tool result */
  newMessages?: (UserMessage | AssistantMessage | AttachmentMessage | SystemMessage)[]
  /** Modify context for subsequent tools (only for non-concurrent tools) */
  contextModifier?: (context: ToolUseContext) => ToolUseContext
}

// ── Tool Progress ──────────────────────────────────────────────────────────

export type ToolProgress<P extends ToolProgressData = ToolProgressData> = {
  toolUseID: string
  data: P
}

export type ToolCallProgress<P extends ToolProgressData = ToolProgressData> = (
  progress: ToolProgress<P>,
) => void

// ── Tool Interface (Core) ──────────────────────────────────────────────────

export type ToolInputSchema = {
  type: 'object'
  properties: Record<string, {
    type: string
    description: string
    enum?: string[]
    default?: unknown
    items?: {
      type: string
      description?: string
      properties?: Record<string, unknown>
      required?: string[]
    }
  }>
  required?: string[]
}

export type Tool<Input = Record<string, unknown>, Output = unknown> = {
  name: string
  description: string
  /** Alternative names for this tool */
  aliases?: string[]
  /** Input schema for Anthropic API tool definition */
  inputSchema: ToolInputSchema
  /** Execute the tool */
  call(
    input: Input,
    context: ToolUseContext,
    onProgress?: ToolCallProgress,
  ): Promise<ToolResult<Output>>
  /** Whether this tool is safe to run concurrently */
  isConcurrencySafe?(input: Input): boolean
  /** Whether this tool only reads (doesn't modify) data */
  isReadOnly?(input: Input): boolean
  /** Whether this tool is destructive (deletes data etc.) */
  isDestructive?(input: Input): boolean
  /** Whether the tool is currently enabled/available */
  isEnabled?(): boolean
  /** Category for grouping */
  category: ToolCategory
  /** Risk level for permission system */
  riskLevel?: 'low' | 'medium' | 'high'
}

export type ToolCategory =
  | 'filesystem'
  | 'shell'
  | 'search'
  | 'web'
  | 'desktop'
  | 'agent'
  | 'mcp'
  | 'memory'
  | 'task'
  | 'skill'
  | 'config'
  | 'planning'
  | 'user_interaction'
  | 'notebook'

// ── Tool Collection ────────────────────────────────────────────────────────

export type Tools = readonly Tool[]

export function findToolByName(tools: Tools, name: string): Tool | undefined {
  return tools.find(t =>
    t.name === name || t.aliases?.includes(name),
  )
}

export function toolMatchesName(tool: Tool, name: string): boolean {
  return tool.name === name || (tool.aliases?.includes(name) ?? false)
}

// ── Tool Use Context (runtime environment for tools) ───────────────────────

export type ToolUseContext = {
  /** Working directory for file/shell operations */
  cwd: string
  /** Abort signal for cancellation */
  abortController: AbortController
  /** Debug mode flag */
  debug: boolean
  /** Current model name */
  model: string
  /** Available tools */
  tools: Tools
  /** Available commands */
  commands: Command[]
  /** Application state accessor */
  getAppState(): AppState
  /** Application state mutator */
  setAppState(f: (prev: AppState) => AppState): void
  /** Show UI element in the chat (approval dialog, progress etc.) */
  setToolUI?: (ui: ToolUIRequest | null) => void
  /** Request user input/approval */
  requestApproval?: (request: ApprovalRequest) => Promise<ApprovalResult>
  /** Append system message to conversation */
  appendSystemMessage?: (content: string) => void
  /** Permission context */
  permissionContext: ToolPermissionContext
  /** Can-use-tool check with approval */
  canUseTool: CanUseToolFn
  /** MCP server connections */
  mcpConnections: MCPConnection[]
  /** Agent definitions for sub-agent spawning */
  agentDefinitions: AgentDefinition[]
  /** Loaded memory content */
  memoryContent?: string
  /** Current engine run ID */
  runId?: string
  /** Current session ID */
  sessionId?: string
  /** Agent ID (for sub-agents) */
  agentId?: string
  /** Worker sandbox ID for isolated child runs */
  sandboxId?: string
  /** Maximum budget in USD */
  maxBudgetUsd?: number
  /** Custom system prompt override */
  customSystemPrompt?: string
  /** Additional system prompt to append */
  appendSystemPrompt?: string
  /** Message history reference */
  messages: Message[]
}

// ── Tool UI ────────────────────────────────────────────────────────────────

export type AskQuestionOption = {
  label: string
  value?: string
  description?: string
}

export type ToolUIRequest = {
  type: 'progress' | 'approval' | 'input' | 'result' | 'error'
  toolName: string
  content: string
  details?: Record<string, unknown>
} & ({
  type: 'input'
  options?: AskQuestionOption[]
  allowMultiple?: boolean
  allowFreeformInput?: boolean
  freeTextLabel?: string
  freeTextPlaceholder?: string
} | {
  type: 'progress' | 'approval' | 'result' | 'error'
})

// ── Approval System ────────────────────────────────────────────────────────

export type ApprovalRequest = {
  toolName: string
  input: Record<string, unknown>
  description: string
  riskLevel: 'low' | 'medium' | 'high'
  suggestedAction: 'allow' | 'deny' | 'ask'
}

export type ApprovalResult =
  | { allowed: true; reason?: string }
  | { allowed: false; reason: string }

export type CanUseToolFn = (
  toolName: string,
  input: Record<string, unknown>,
  context: ToolUseContext,
) => Promise<ApprovalResult>

// ── Permission Context ─────────────────────────────────────────────────────

export type PermissionMode = 'default' | 'plan' | 'bypass' | 'strict'

export type ToolPermissionRule = {
  pattern: string
  decision: 'allow' | 'deny' | 'ask'
  source: string
}

export type ToolPermissionContext = {
  mode: PermissionMode
  allowRules: ToolPermissionRule[]
  denyRules: ToolPermissionRule[]
  askRules: ToolPermissionRule[]
  allowedDirectories: string[]
}

export function getEmptyToolPermissionContext(): ToolPermissionContext {
  return {
    mode: 'default',
    allowRules: [],
    denyRules: [],
    askRules: [],
    allowedDirectories: [],
  }
}

// ── Application State ──────────────────────────────────────────────────────

export type AppState = {
  cwd: string
  sessionId: string
  turnCount: number
  totalTokens: { input: number; output: number }
  totalCostUsd: number
  planMode: boolean
  activeTasks: string[]
  /** Custom state that tools/commands can hang data off */
  custom: Record<string, unknown>
}

export function createInitialAppState(cwd: string): AppState {
  return {
    cwd,
    sessionId: crypto.randomUUID(),
    turnCount: 0,
    totalTokens: { input: 0, output: 0 },
    totalCostUsd: 0,
    planMode: false,
    activeTasks: [],
    custom: {},
  }
}

// ── Command Interface ──────────────────────────────────────────────────────

export type Command = {
  name: string
  description: string
  shortDescription?: string
  category: CommandCategory
  examples?: string[]
  /** Execute the command, returns optional response text */
  call(args: string, context: ToolUseContext): Promise<string | void>
  /** Whether the command is available in current context */
  isAvailable?(): boolean
}

export type CommandCategory =
  | 'session'
  | 'config'
  | 'code'
  | 'git'
  | 'planning'
  | 'agents'
  | 'tools'
  | 'memory'
  | 'mcp'
  | 'debug'
  | 'security'
  | 'display'
  | 'navigation'
  | 'workspace'
  | 'export'
  | 'plugins'
  | 'crew'
  | 'advanced'

// ── Agent Definitions ──────────────────────────────────────────────────────

export type AgentDefinition = {
  id: string
  name: string
  description: string
  type: 'coding' | 'research' | 'review' | 'planning' | 'custom'
  systemPrompt?: string
  tools?: string[]
  maxTurns?: number
  budget?: { totalUsd?: number }
}

// ── MCP Connection ─────────────────────────────────────────────────────────

export type MCPConnection = {
  name: string
  serverType: 'stdio' | 'sse' | 'streamable-http'
  connected: boolean
  tools: MCPToolInfo[]
  resources: MCPResourceInfo[]
}

export type MCPToolInfo = {
  name: string
  description: string
  inputSchema: ToolInputSchema
}

export type MCPResourceInfo = {
  uri: string
  name: string
  description?: string
  mimeType?: string
}

// ── Thinking Config ────────────────────────────────────────────────────────

export type ThinkingConfig =
  | { type: 'enabled'; budgetTokens: number }
  | { type: 'disabled' }
