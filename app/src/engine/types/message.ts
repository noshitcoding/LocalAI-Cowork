// ── Claude Code Message Types (ported) ──────────────────────────────────────
// Mirrors: claude-code-main/src/types/message.ts

export type UUID = string

export type ContentBlockText = { type: 'text'; text: string }
export type ContentBlockImage = { type: 'image'; source: { type: 'base64'; media_type: string; data: string } }
export type ContentBlockToolUse = { type: 'tool_use'; id: string; name: string; input: Record<string, unknown> }
export type ContentBlockToolResult = { type: 'tool_result'; tool_use_id: string; content: string; is_error?: boolean }
export type ContentBlockThinking = { type: 'thinking'; thinking: string }
export type ContentBlock = ContentBlockText | ContentBlockImage | ContentBlockToolUse | ContentBlockToolResult | ContentBlockThinking

export type UserMessage = {
  type: 'user'
  uuid: UUID
  content: ContentBlock[]
  toolUseResult?: string
  sourceToolAssistantUUID?: UUID
  timestamp: number
}

export type AssistantMessage = {
  type: 'assistant'
  uuid: UUID
  content: ContentBlock[]
  model: string
  stopReason: 'end_turn' | 'tool_use' | 'max_tokens' | 'stop_sequence' | null
  usage: TokenUsage
  apiError?: string
  timestamp: number
}

export type SystemMessage = {
  type: 'system'
  uuid: UUID
  content: string
  injectionPoint?: 'initial' | 'pre_tool' | 'compact_boundary'
  timestamp: number
}

export type AttachmentMessage = {
  type: 'attachment'
  uuid: UUID
  title: string
  content: ContentBlock[]
  attachmentType: 'memory' | 'context' | 'tool_result'
  timestamp: number
}

export type ToolUseSummaryMessage = {
  type: 'tool_use_summary'
  uuid: UUID
  summary: { toolName: string; inputSnippet: string; outputSnippet: string }
  timestamp: number
}

export type TombstoneMessage = {
  type: 'tombstone'
  uuid: UUID
  reason: 'compacted' | 'snipped' | 'filtered'
  timestamp: number
}

export type ProgressMessage = {
  type: 'progress'
  uuid: UUID
  toolUseId: string
  data: ToolProgressData
  timestamp: number
}

export type Message =
  | UserMessage
  | AssistantMessage
  | SystemMessage
  | AttachmentMessage
  | ToolUseSummaryMessage
  | TombstoneMessage
  | ProgressMessage

// ── Stream Events (Anthropic API) ──────────────────────────────────────────

export type StreamEvent =
  | { type: 'message_start'; message: { id: string; model: string; usage: TokenUsage } }
  | { type: 'content_block_start'; index: number; content_block: ContentBlock }
  | { type: 'content_block_delta'; index: number; delta: ContentBlockDelta }
  | { type: 'content_block_stop'; index: number }
  | { type: 'message_delta'; delta: { stop_reason: string }; usage: Partial<TokenUsage> }
  | { type: 'message_stop' }

export type ContentBlockDelta =
  | { type: 'text_delta'; text: string }
  | { type: 'input_json_delta'; partial_json: string }
  | { type: 'thinking_delta'; thinking: string }

// ── Token Usage ────────────────────────────────────────────────────────────

export type TokenUsage = {
  input_tokens: number
  output_tokens: number
  cache_creation_input_tokens?: number
  cache_read_input_tokens?: number
}

export const EMPTY_USAGE: TokenUsage = { input_tokens: 0, output_tokens: 0 }

export function accumulateUsage(prev: TokenUsage, delta: Partial<TokenUsage>): TokenUsage {
  return {
    input_tokens: prev.input_tokens + (delta.input_tokens ?? 0),
    output_tokens: prev.output_tokens + (delta.output_tokens ?? 0),
    cache_creation_input_tokens: (prev.cache_creation_input_tokens ?? 0) + (delta.cache_creation_input_tokens ?? 0),
    cache_read_input_tokens: (prev.cache_read_input_tokens ?? 0) + (delta.cache_read_input_tokens ?? 0),
  }
}

// ── Tool Progress Types ────────────────────────────────────────────────────

export type BashProgress = { type: 'bash_progress'; output: string; exitCode?: number }
export type AgentToolProgress = { type: 'agent_progress'; agentName: string; content: string }
export type MCPProgress = { type: 'mcp_progress'; serverName: string; progress: number }
export type WebSearchProgress = { type: 'web_search_progress'; query: string; results: number }
export type SkillToolProgress = { type: 'skill_progress'; skillName: string; output: string }
export type TaskOutputProgress = { type: 'task_output_progress'; taskId: string; output: string }
export type FileProgress = {
  type: 'file_progress'
  path: string
  operation: 'read' | 'write' | 'edit' | 'create_dir' | 'move' | 'copy' | 'delete' | 'rename'
}

export type ToolProgressData =
  | BashProgress
  | AgentToolProgress
  | MCPProgress
  | WebSearchProgress
  | SkillToolProgress
  | TaskOutputProgress
  | FileProgress

// ── Request Start Event ────────────────────────────────────────────────────

export type RequestStartEvent = {
  type: 'request_start'
  model: string
  inputTokens: number
  timestamp: number
}

// ── Terminal States ────────────────────────────────────────────────────────

export type Terminal =
  | { type: 'end_turn'; messages: Message[] }
  | { type: 'max_turns'; messages: Message[] }
  | { type: 'error'; error: string; messages: Message[] }
  | { type: 'interrupted'; messages: Message[] }

// ── Helpers ────────────────────────────────────────────────────────────────

export function generateUUID(): UUID {
  return crypto.randomUUID()
}

export function createUserMessage(content: string | ContentBlock[]): UserMessage {
  const blocks: ContentBlock[] = typeof content === 'string'
    ? [{ type: 'text', text: content }]
    : content
  return {
    type: 'user',
    uuid: generateUUID(),
    content: blocks,
    timestamp: Date.now(),
  }
}

export function createSystemMessage(content: string, injectionPoint?: SystemMessage['injectionPoint']): SystemMessage {
  return {
    type: 'system',
    uuid: generateUUID(),
    content,
    injectionPoint,
    timestamp: Date.now(),
  }
}

export function createAssistantMessage(content: ContentBlock[], model: string, usage: TokenUsage, stopReason: AssistantMessage['stopReason'] = 'end_turn'): AssistantMessage {
  return {
    type: 'assistant',
    uuid: generateUUID(),
    content,
    model,
    usage,
    stopReason,
    timestamp: Date.now(),
  }
}

export function extractTextContent(message: Message): string {
  if (message.type === 'system') return message.content
  if (message.type === 'tombstone') return ''
  if (message.type === 'tool_use_summary') return `[${message.summary.toolName}] ${message.summary.outputSnippet}`
  if ('content' in message) {
    if (!Array.isArray(message.content)) {
      return typeof message.content === 'string' ? message.content : ''
    }
    return message.content
      .filter((b): b is ContentBlockText => b.type === 'text')
      .map(b => b.text)
      .join('')
  }
  return ''
}

export function getToolUseBlocks(message: AssistantMessage): ContentBlockToolUse[] {
  return message.content.filter((b): b is ContentBlockToolUse => b.type === 'tool_use')
}

export function getThinkingContent(message: AssistantMessage): string {
  return message.content
    .filter((b): b is ContentBlockThinking => b.type === 'thinking')
    .map(b => b.thinking)
    .join('\n')
}
