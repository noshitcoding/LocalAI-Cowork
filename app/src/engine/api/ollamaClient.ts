// ── Ollama API Client (browser-compatible) ──────────────────────────────────
// Talks to Ollama's /api/chat endpoint with tool support.
// Returns the same StreamEvent + SampleResult types as anthropicClient.ts
// so QueryEngine can use either backend interchangeably.
//
// Enhanced with:
// - Retry logic for connection failures
// - Thinking/reasoning support (reflexion models)
// - Better tool call edge case handling
// - Model capability detection

import type {
  ContentBlock,
  ContentBlockToolUse,
  StreamEvent,
  TokenUsage,
  ToolInputSchema,
} from '../types'
import { EMPTY_USAGE } from '../types'
import { invoke } from '@tauri-apps/api/core'

// ── Configuration ──────────────────────────────────────────────────────────

export type OllamaEngineConfig = {
  baseUrl: string
  model: string
  temperature?: number
  contextWindow?: number
  timeoutMs?: number
  /** Enable thinking/reasoning mode if model supports it */
  thinkingEnabled?: boolean
  /** Keep-alive duration for model in memory */
  keepAlive?: string
}

// ── Model Capabilities ─────────────────────────────────────────────────────

export type OllamaModelCapabilities = {
  supportsTools: boolean
  supportsThinking: boolean
  contextLength: number
  family: string
}

/** Known model families and their capabilities */
const MODEL_CAPABILITIES: Record<string, Partial<OllamaModelCapabilities>> = {
  'gpt-oss': { supportsTools: true, supportsThinking: true },
  'qwen': { supportsTools: true, supportsThinking: true },
  'llama': { supportsTools: true, supportsThinking: false },
  'mistral': { supportsTools: true, supportsThinking: false },
  'deepseek': { supportsTools: true, supportsThinking: true },
  'command-r': { supportsTools: true, supportsThinking: false },
  'gemma': { supportsTools: true, supportsThinking: false },
  'phi': { supportsTools: true, supportsThinking: false },
  'codellama': { supportsTools: false, supportsThinking: false },
}

/**
 * Detect model capabilities from model name.
 */
export function detectModelCapabilities(model: string): OllamaModelCapabilities {
  const lower = model.toLowerCase()

  for (const [family, caps] of Object.entries(MODEL_CAPABILITIES)) {
    if (lower.includes(family)) {
      return {
        supportsTools: caps.supportsTools ?? true,
        supportsThinking: caps.supportsThinking ?? false,
        contextLength: 0,
        family,
      }
    }
  }

  // Default: assume tools supported
  return {
    supportsTools: true,
    supportsThinking: false,
    contextLength: 0,
    family: 'unknown',
  }
}

// ── API Types (Ollama /api/chat format) ────────────────────────────────────

type OllamaMessage = {
  role: 'system' | 'user' | 'assistant' | 'tool'
  content: string
  images?: string[]
  tool_calls?: OllamaToolCall[]
}

type OllamaToolCall = {
  function: {
    name: string
    arguments: Record<string, unknown>
  }
}

type OllamaToolDef = {
  type: 'function'
  function: {
    name: string
    description: string
    parameters: ToolInputSchema
  }
}

type OllamaStreamChunk = {
  model?: string
  message?: {
    role: string
    content?: string
    thinking?: string
    reasoning_content?: string
    reasoning?: string
    tool_calls?: OllamaToolCall[]
  }
  response?: string
  thinking?: string
  reasoning_content?: string
  reasoning?: string
  done?: boolean
  done_reason?: string
  total_duration?: number
  eval_count?: number
  prompt_eval_count?: number
}

function convertContentBlocksToOllamaPayload(
  blocks: ContentBlock[],
): {
  text: string
  images: string[]
  toolResults: Array<{ content: string }>
} {
  const textParts: string[] = []
  const images: string[] = []
  const toolResults: Array<{ content: string }> = []

  for (const block of blocks) {
    if (block.type === 'text') {
      textParts.push(block.text)
      continue
    }
    if (block.type === 'image' && block.source.type === 'base64') {
      images.push(block.source.data)
      continue
    }
    if (block.type === 'tool_result') {
      toolResults.push({ content: block.content })
      continue
    }
  }

  return {
    text: textParts.join('\n').trim(),
    images,
    toolResults,
  }
}

// ── Re-export SampleResult for compatibility ───────────────────────────────

export type SampleResult = {
  content: ContentBlock[]
  model: string
  stopReason: string | null
  usage: TokenUsage
  costUsd: number
}

type TauriChatTurnResponse = {
  endpoint: string
  model: string
  assistantMessage: string
  requiresApproval: boolean
  proposedPlan: string[]
  toolCalls?: OllamaToolCall[]
}

type TauriOllamaHealthResponse = {
  ok: boolean
  endpoint: string
  model: string
  latencyMs: number
  version?: string | null
  models: string[]
  error?: string | null
}

type TauriChatHistoryItem = {
  role: string

  content: string
}

export type EngineToolDef = {
  name: string
  description: string
  input_schema: ToolInputSchema
  aliases?: string[]
}

type OllamaChatRequestBuild = {
  body: Record<string, unknown>
  debugPreview: string
}

function clipOllamaDebugText(value: string, maxChars: number): string {
  if (value.length <= maxChars) return value
  const hiddenChars = value.length - maxChars
  return `${value.slice(0, maxChars)}\n...[truncated ${hiddenChars} chars]`
}

function summarizeOllamaMessagesForDebug(messages: OllamaMessage[]): Array<Record<string, unknown>> {
  return messages.map((message) => ({
    role: message.role,
    content: clipOllamaDebugText(message.content, message.role === 'system' ? 2500 : 1200),
    contentLength: message.content.length,
    images: Array.isArray(message.images)
      ? message.images.map((image, index) => `[base64 image ${index + 1}, ${image.length} chars]`)
      : undefined,
    tool_calls: message.tool_calls,
  }))
}

export function buildOllamaChatRequest(
  config: OllamaEngineConfig,
  messages: Array<{ role: 'user' | 'assistant'; content: string | ContentBlock[] }>,
  systemPrompt: string,
  tools?: EngineToolDef[],
): OllamaChatRequestBuild {
  const capabilities = detectModelCapabilities(config.model)

  const ollamaMessages: OllamaMessage[] = [
    { role: 'system', content: systemPrompt },
  ]

  for (const msg of messages) {
    if (typeof msg.content === 'string') {
      ollamaMessages.push({ role: msg.role, content: msg.content })
      continue
    }

    const payload = convertContentBlocksToOllamaPayload(msg.content)
    if (payload.toolResults.length > 0) {
      for (const tr of payload.toolResults) {
        ollamaMessages.push({
          role: 'tool',
          content: tr.content,
        })
      }
    }

    if (payload.text || payload.images.length > 0) {
      ollamaMessages.push({
        role: msg.role,
        content: payload.text || '[image attachment]',
        images: payload.images.length > 0 ? payload.images : undefined,
      })
    }
  }

  const ollamaTools: OllamaToolDef[] | undefined = (tools && capabilities.supportsTools)
    ? tools.map((t) => ({
        type: 'function' as const,
        function: {
          name: t.name,
          description: t.description,
          parameters: t.input_schema,
        },
      }))
    : undefined

  const body: Record<string, unknown> = {
    model: config.model,
    messages: ollamaMessages,
    stream: true,
  }

  if (capabilities.supportsThinking) {
    const requestedThinking = config.thinkingEnabled ?? true
    body.think = requestedThinking
      ? capabilities.family === 'gpt-oss' ? 'medium' : true
      : false
  }

  if (ollamaTools && ollamaTools.length > 0) {
    body.tools = ollamaTools
  }

  const options: Record<string, unknown> = {}
  if (config.temperature !== undefined) {
    options.temperature = config.temperature
  }
  if (config.contextWindow) {
    options.num_ctx = config.contextWindow
  }
  if (Object.keys(options).length > 0) {
    body.options = options
  }

  if (config.keepAlive) {
    body.keep_alive = config.keepAlive
  }

  const debugPreview = JSON.stringify({
    model: config.model,
    stream: true,
    think: body.think,
    keep_alive: body.keep_alive,
    options: body.options,
    messageCount: ollamaMessages.length,
    messages: summarizeOllamaMessagesForDebug(ollamaMessages),
    toolCount: ollamaTools?.length ?? 0,
    tools: ollamaTools?.map((tool) => tool.function.name),
  }, null, 2)

  return {
    body,
    debugPreview,
  }
}

type TauriWindow = Window & {
  __TAURI_INTERNALS__?: {
    invoke?: unknown
  }
  __TAURI_IPC__?: unknown
}

function canUseTauriInvoke(): boolean {
  if (typeof window === 'undefined') {
    return true
  }

  const tauriWindow = window as TauriWindow
  return typeof tauriWindow.__TAURI_INTERNALS__?.invoke === 'function'
    || typeof tauriWindow.__TAURI_IPC__ === 'function'
}

function normalizeTauriInvokeError(error: unknown): Error {
  const message = error instanceof Error ? error.message : String(error)
  const lower = message.toLowerCase()
  const isBridgeMissing = lower.includes('__tauri_internals__')
    || lower.includes('is undefined')
    || lower.includes('tauri')

  if (isBridgeMissing) {
    return new Error(
      'Tauri bridge is not available. The app is probably not running as a Tauri desktop app. Start LocalAI Cowork with "npm run tauri dev" (or as a built desktop app).',
    )
  }

  return error instanceof Error ? error : new Error(message)
}

const TEXTUAL_TOOL_NAME_ALIASES: Record<string, string> = {
  // Web
  webfetch: 'WebFetch',
  web_fetch: 'WebFetch',
  websearch: 'WebSearch',
  web_search: 'WebSearch',
  // File read/write/edit
  read_file: 'Read',
  read: 'Read',
  write_file: 'Write',
  write: 'Write',
  edit_file: 'Edit',
  edit: 'Edit',
  // File append
  append: 'Append',
  append_file: 'Append',
  file_append: 'Append',
  // Multi edit
  multiedit: 'MultiEdit',
  multi_edit: 'MultiEdit',
  batch_edit: 'MultiEdit',
  // ListDir
  listdir: 'ListDir',
  list_dir: 'ListDir',
  list_directory: 'ListDir',
  ls: 'ListDir',
  dir: 'ListDir',
  // Move
  movepath: 'MovePath',
  move_path: 'MovePath',
  move_file: 'MovePath',
  move_directory: 'MovePath',
  rename_path: 'MovePath',
  // Copy
  copypath: 'CopyPath',
  copy_path: 'CopyPath',
  copy_file: 'CopyPath',
  copy_directory: 'CopyPath',
  // CreateDirectory
  createdirectory: 'CreateDirectory',
  create_directory: 'CreateDirectory',
  mkdir: 'CreateDirectory',
  make_dir: 'CreateDirectory',
  // Delete
  deletefile: 'DeleteFile',
  delete_file: 'DeleteFile',
  remove_file: 'DeleteFile',
  rm: 'DeleteFile',
  // FileInfo
  fileinfo: 'FileInfo',
  file_info: 'FileInfo',
  stat: 'FileInfo',
  file_metadata: 'FileInfo',
  // Rename
  renamefile: 'RenameFile',
  rename_file: 'RenameFile',
  rename: 'RenameFile',
  // Search
  glob: 'Glob',
  grep: 'Grep',
  search: 'Grep',
  // Shell
  bash: 'Bash',
  shell: 'Bash',
  execute: 'Bash',
  // User interaction
  askuser: 'AskUser',
  ask_user: 'AskUser',
  // Plan mode
  enterplanmode: 'EnterPlanMode',
  enter_plan_mode: 'EnterPlanMode',
  plan: 'EnterPlanMode',
  exitplanmode: 'ExitPlanMode',
  exit_plan_mode: 'ExitPlanMode',
  // MCP
  mcptool: 'MCPTool',
  mcp_call: 'MCPTool',
  mcp: 'MCPTool',
  // Tasks
  taskcreate: 'TaskCreate',
  task_create: 'TaskCreate',
  todo_add: 'TaskCreate',
  tasklist: 'TaskList',
  task_list: 'TaskList',
  todo_list: 'TaskList',
  taskupdate: 'TaskUpdate',
  task_update: 'TaskUpdate',
  todo_update: 'TaskUpdate',
  // Memory
  memoryread: 'MemoryRead',
  memory_read: 'MemoryRead',
  recall: 'MemoryRead',
  memorywrite: 'MemoryWrite',
  memory_write: 'MemoryWrite',
  memory: 'MemoryWrite',
  remember: 'MemoryWrite',
  sessionsearch: 'SessionSearch',
  session_search: 'SessionSearch',
  search_sessions: 'SessionSearch',
  // Thinking
  think: 'Think',
  reasoning: 'Think',
  // Skill
  skill: 'Skill',
  run_skill: 'Skill',
  // Agent
  agent: 'Agent',
  subagent: 'Agent',
}

function getPrimaryToolInputKey(tool: EngineToolDef): string | null {
  const required = Array.isArray(tool.input_schema.required)
    ? tool.input_schema.required.find((key) => typeof key === 'string' && key.trim().length > 0)
    : null

  if (required) return required

  const properties = tool.input_schema.properties ?? {}
  const [firstKey] = Object.keys(properties)
  return firstKey ?? null
}

function stripWrappingQuotes(value: string): string | null {
  const trimmed = value.trim()
  if (trimmed.length < 2) return null

  const first = trimmed[0]
  const last = trimmed[trimmed.length - 1]
  if (!((first === '"' && last === '"') || (first === '\'' && last === '\''))) {
    return null
  }

  const inner = trimmed.slice(1, -1)
  if (first === '"') {
    try {
      return JSON.parse(trimmed) as string
    } catch {
      return inner
    }
  }

  return inner.replace(/\\'/g, '\'').replace(/\\"/g, '"')
}

function buildToolLookup(tools?: EngineToolDef[]): Map<string, EngineToolDef> {
  const lookup = new Map<string, EngineToolDef>()

  for (const tool of tools ?? []) {
    lookup.set(tool.name.toLowerCase(), tool)
    lookup.set(tool.name.replace(/[^a-z0-9]/gi, '').toLowerCase(), tool)
    for (const alias of tool.aliases ?? []) {
      lookup.set(alias.toLowerCase(), tool)
      lookup.set(alias.replace(/[^a-z0-9]/gi, '').toLowerCase(), tool)
    }
  }

  for (const [alias, canonical] of Object.entries(TEXTUAL_TOOL_NAME_ALIASES)) {
    const tool = lookup.get(canonical.toLowerCase())
      ?? lookup.get(canonical.replace(/[^a-z0-9]/gi, '').toLowerCase())
    if (tool) {
      lookup.set(alias.toLowerCase(), tool)
    }
  }

  return lookup
}

const TOOL_ARGUMENT_NAME_ALIASES: Record<string, string[]> = {
  file_path: ['filepath', 'filePath', 'filename', 'fileName', 'file', 'path', 'targetFile'],
  source_path: ['source', 'sourcePath', 'from', 'fromPath', 'src', 'srcPath'],
  destination_path: ['destination', 'destinationPath', 'target', 'targetPath', 'to', 'toPath', 'dest', 'destPath'],
  new_name: ['newName', 'name', 'filename', 'fileName'],
  content: ['text', 'contents', 'body', 'value'],
  old_string: ['old', 'oldText', 'find', 'search', 'match'],
  new_string: ['new', 'newText', 'replace', 'replacement'],
  path: ['dir', 'directory', 'folder', 'filename', 'fileName', 'file'],
}

function canonicalizeArgumentKey(value: string): string {
  return value.replace(/[^a-z0-9]/gi, '').toLowerCase()
}

function normalizeToolArguments(
  args: Record<string, unknown>,
  tool: EngineToolDef,
): Record<string, unknown> {
  const properties = tool.input_schema.properties ?? {}
  const propertyKeys = Object.keys(properties)
  if (propertyKeys.length === 0) return args

  const normalized: Record<string, unknown> = { ...args }
  const consumed = new Set<string>()

  for (const key of propertyKeys) {
    const existingKey = Object.keys(args).find((candidate) => canonicalizeArgumentKey(candidate) === canonicalizeArgumentKey(key))
    if (existingKey) {
      normalized[key] = args[existingKey]
      consumed.add(existingKey)
    }
  }

  for (const key of propertyKeys) {
    if (normalized[key] !== undefined) continue

    const aliases = TOOL_ARGUMENT_NAME_ALIASES[key] ?? []
    const aliasMatch = Object.keys(args).find((candidate) => {
      if (consumed.has(candidate)) return false
      const normalizedCandidate = canonicalizeArgumentKey(candidate)
      return aliases.some((alias) => canonicalizeArgumentKey(alias) === normalizedCandidate)
    })

    if (aliasMatch) {
      normalized[key] = args[aliasMatch]
      consumed.add(aliasMatch)
    }
  }

  const primaryKey = getPrimaryToolInputKey(tool)
  const remainingKeys = Object.keys(args).filter((key) => !consumed.has(key))
  if (primaryKey && normalized[primaryKey] === undefined && remainingKeys.length === 1) {
    normalized[primaryKey] = args[remainingKeys[0]]
  }

  return normalized
}

function normalizeToolCall(
  toolCall: OllamaToolCall,
  toolLookup: Map<string, EngineToolDef>,
): OllamaToolCall {
  const rawName = toolCall.function.name
  const tool = toolLookup.get(rawName.toLowerCase())
    ?? toolLookup.get(rawName.replace(/[^a-z0-9]/gi, '').toLowerCase())

  if (!tool) return toolCall

  return {
    function: {
      name: tool.name,
      arguments: normalizeToolArguments(toolCall.function.arguments ?? {}, tool),
    },
  }
}

function normalizeToolCalls(
  toolCalls: OllamaToolCall[],
  tools?: EngineToolDef[],
): OllamaToolCall[] {
  if (!tools || tools.length === 0 || toolCalls.length === 0) return toolCalls
  const toolLookup = buildToolLookup(tools)
  return toolCalls.map((toolCall) => normalizeToolCall(toolCall, toolLookup))
}

function parseTextualToolCallLine(
  line: string,
  toolLookup: Map<string, EngineToolDef>,
): OllamaToolCall | null {
  const normalized = line
    .trim()
    .replace(/^[-*]\s+/, '')
    .replace(/^\d+\.\s+/, '')
    .replace(/^`(.+)`$/u, '$1')

  const match = normalized.match(/^([A-Za-z][A-Za-z0-9_]*)\s*\(([\s\S]*)\)$/)
  if (!match) return null

  const rawToolName = match[1]
  const rawArgs = match[2].trim()
  const tool = toolLookup.get(rawToolName.toLowerCase())
    ?? toolLookup.get(rawToolName.replace(/[^a-z0-9]/gi, '').toLowerCase())
  if (!tool) return null

  if (rawArgs.length === 0) {
    return {
      function: {
        name: tool.name,
        arguments: {},
      },
    }
  }

  if (rawArgs.startsWith('{') && rawArgs.endsWith('}')) {
    try {
      const parsed = JSON.parse(rawArgs) as Record<string, unknown>
      if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
        return normalizeToolCall({
          function: {
            name: tool.name,
            arguments: parsed,
          },
        }, toolLookup)
      }
    } catch {
      // Fall through to simpler parsers.
    }
  }

  const primaryKey = getPrimaryToolInputKey(tool)
  if (!primaryKey) return null

  const quotedValue = stripWrappingQuotes(rawArgs)
  if (quotedValue !== null) {
    return normalizeToolCall({
      function: {
        name: tool.name,
        arguments: { [primaryKey]: quotedValue },
      },
    }, toolLookup)
  }

  const namedMatch = rawArgs.match(/^([A-Za-z][A-Za-z0-9_]*)\s*[:=]\s*(.+)$/)
  if (namedMatch) {
    return normalizeToolCall({
      function: {
        name: tool.name,
        arguments: {
          [namedMatch[1]]: stripWrappingQuotes(namedMatch[2]) ?? namedMatch[2].trim(),
        },
      },
    }, toolLookup)
  }

  return normalizeToolCall({
    function: {
      name: tool.name,
      arguments: { [primaryKey]: rawArgs },
    },
  }, toolLookup)
}

function extractTextualToolCalls(
  fullText: string,
  tools?: EngineToolDef[],
): { text: string; toolCalls: OllamaToolCall[] } {
  if (!fullText.trim() || !tools || tools.length === 0) {
    return { text: fullText, toolCalls: [] }
  }

  const toolLookup = buildToolLookup(tools)
  const lines = fullText.split(/\r?\n/)
  const toolCalls: OllamaToolCall[] = []
  const keptLines: string[] = []

  for (const line of lines) {
    const parsed = parseTextualToolCallLine(line, toolLookup)
    if (parsed) {
      toolCalls.push(parsed)
      continue
    }
    keptLines.push(line)
  }

  if (toolCalls.length === 0) {
    const singleLineCall = parseTextualToolCallLine(fullText.trim(), toolLookup)
    if (!singleLineCall) {
      return { text: fullText, toolCalls: [] }
    }
    return { text: '', toolCalls: normalizeToolCalls([singleLineCall], tools) }
  }

  return {
    text: keptLines.join('\n').trim(),
    toolCalls: normalizeToolCalls(toolCalls, tools),
  }
}

const OPEN_WEBUI_LARGE_DELTA_THRESHOLD = 5
const OPEN_WEBUI_CHUNK_DELAY_MS = 5

function canDelayVisibleStream(): boolean {
  return typeof document === 'undefined' || document.visibilityState !== 'hidden'
}

function nextOpenWebUIChunkSize(remaining: number): number {
  return Math.min(Math.floor(Math.random() * 3) + 1, remaining)
}

async function sleep(ms: number): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, ms))
}

async function* streamSmoothedDelta(
  text: string,
  deltaType: 'text_delta' | 'thinking_delta',
): AsyncGenerator<StreamEvent> {
  if (text.length <= OPEN_WEBUI_LARGE_DELTA_THRESHOLD) {
    yield {
      type: 'content_block_delta',
      index: 0,
      delta: deltaType === 'text_delta'
        ? { type: 'text_delta', text }
        : { type: 'thinking_delta', thinking: text },
    }
    return
  }

  let remaining = text
  while (remaining.length > 0) {
    const chunkSize = nextOpenWebUIChunkSize(remaining.length)
    const chunk = remaining.slice(0, chunkSize)
    remaining = remaining.slice(chunkSize)
    yield {
      type: 'content_block_delta',
      index: 0,
      delta: deltaType === 'text_delta'
        ? { type: 'text_delta', text: chunk }
        : { type: 'thinking_delta', thinking: chunk },
    }
    if (remaining.length > 0 && canDelayVisibleStream()) {
      await sleep(OPEN_WEBUI_CHUNK_DELAY_MS)
    }
  }
}

function toolCallSignature(toolCall: OllamaToolCall): string {
  return JSON.stringify({
    name: toolCall.function.name,
    arguments: toolCall.function.arguments ?? {},
  })
}

type ReasoningTagPair = {
  start: string
  end: string
}

type ParsedReasoningChunk =
  | { type: 'text'; text: string }
  | { type: 'thinking'; text: string }

const DEFAULT_REASONING_TAGS: ReasoningTagPair[] = [
  { start: '<think>', end: '</think>' },
  { start: '<thinking>', end: '</thinking>' },
  { start: '<reason>', end: '</reason>' },
  { start: '<reasoning>', end: '</reasoning>' },
  { start: '<thought>', end: '</thought>' },
  { start: '<|begin_of_thought|>', end: '<|end_of_thought|>' },
  { start: '\u25c1think\u25b7', end: '\u25c1/think\u25b7' },
]

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

function getXmlLikeTagName(tag: string): string | null {
  const match = tag.match(/^<([^>\s]+)>$/)
  return match?.[1] ?? null
}

function buildStartTagRegex(tag: string): RegExp | null {
  const tagName = getXmlLikeTagName(tag)
  if (!tagName) return null
  return new RegExp(`<${escapeRegExp(tagName)}(?:\\s[^>]*)?>`, 'i')
}

function findReasoningStartTag(text: string): { index: number; length: number; tag: ReasoningTagPair } | null {
  let best: { index: number; length: number; tag: ReasoningTagPair } | null = null

  for (const tag of DEFAULT_REASONING_TAGS) {
    const regex = buildStartTagRegex(tag.start)
    const match = regex?.exec(text)
    const index = match
      ? match.index
      : text.toLowerCase().indexOf(tag.start.toLowerCase())
    const length = match ? match[0].length : tag.start.length

    if (index < 0) continue
    if (!best || index < best.index) {
      best = { index, length, tag }
    }
  }

  return best
}

function findReasoningEndTag(text: string, tag: ReasoningTagPair): { index: number; length: number } | null {
  const index = text.toLowerCase().indexOf(tag.end.toLowerCase())
  if (index < 0) return null
  return { index, length: tag.end.length }
}

function getStartTagPartialLength(text: string): number {
  let keep = 0

  for (const tag of DEFAULT_REASONING_TAGS) {
    const lowerStart = tag.start.toLowerCase()
    const maxLength = Math.min(text.length, tag.start.length - 1)
    for (let length = 1; length <= maxLength; length++) {
      const suffix = text.slice(-length).toLowerCase()
      if (lowerStart.startsWith(suffix)) {
        keep = Math.max(keep, length)
      }
    }
  }

  const lastLt = text.lastIndexOf('<')
  if (lastLt >= 0) {
    const suffix = text.slice(lastLt)
    if (!suffix.includes('>')) {
      const lowerSuffix = suffix.toLowerCase()
      for (const tag of DEFAULT_REASONING_TAGS) {
        const tagName = getXmlLikeTagName(tag.start)
        if (!tagName) continue

        const lowerPrefix = `<${tagName.toLowerCase()}`
        if (lowerPrefix.startsWith(lowerSuffix) || lowerSuffix.startsWith(`${lowerPrefix} `)) {
          keep = Math.max(keep, suffix.length)
        }
      }
    }
  }

  return keep
}

function getEndTagPartialLength(text: string, tag: ReasoningTagPair): number {
  let keep = 0
  const lowerEnd = tag.end.toLowerCase()
  const maxLength = Math.min(text.length, tag.end.length - 1)

  for (let length = 1; length <= maxLength; length++) {
    const suffix = text.slice(-length).toLowerCase()
    if (lowerEnd.startsWith(suffix)) {
      keep = Math.max(keep, length)
    }
  }

  return keep
}

class ReasoningTagStreamParser {
  private buffer = ''
  private activeTag: ReasoningTagPair | null = null

  push(text: string): ParsedReasoningChunk[] {
    this.buffer += text
    return this.drain(false)
  }

  flush(): ParsedReasoningChunk[] {
    return this.drain(true)
  }

  private drain(flush: boolean): ParsedReasoningChunk[] {
    const chunks: ParsedReasoningChunk[] = []

    while (this.buffer.length > 0) {
      if (this.activeTag) {
        const end = findReasoningEndTag(this.buffer, this.activeTag)
        if (end) {
          const thinking = this.buffer.slice(0, end.index)
          if (thinking) {
            chunks.push({ type: 'thinking', text: thinking })
          }
          this.buffer = this.buffer.slice(end.index + end.length)
          this.activeTag = null
          continue
        }

        const keep = flush ? 0 : getEndTagPartialLength(this.buffer, this.activeTag)
        const emitLength = this.buffer.length - keep
        if (emitLength > 0) {
          chunks.push({ type: 'thinking', text: this.buffer.slice(0, emitLength) })
          this.buffer = this.buffer.slice(emitLength)
        }
        break
      }

      const start = findReasoningStartTag(this.buffer)
      if (start) {
        const text = this.buffer.slice(0, start.index)
        if (text) {
          chunks.push({ type: 'text', text })
        }
        this.buffer = this.buffer.slice(start.index + start.length)
        this.activeTag = start.tag
        continue
      }

      const keep = flush ? 0 : getStartTagPartialLength(this.buffer)
      const emitLength = this.buffer.length - keep
      if (emitLength > 0) {
        chunks.push({ type: 'text', text: this.buffer.slice(0, emitLength) })
        this.buffer = this.buffer.slice(emitLength)
      }
      break
    }

    return chunks
  }
}

function firstNonEmptyString(...values: Array<string | undefined>): string {
  return values.find((value): value is string => typeof value === 'string' && value.length > 0) ?? ''
}

function getNativeThinkingChunk(chunk: OllamaStreamChunk): string {
  return firstNonEmptyString(
    chunk.message?.thinking,
    chunk.message?.reasoning_content,
    chunk.message?.reasoning,
    chunk.thinking,
    chunk.reasoning_content,
    chunk.reasoning,
  )
}

// ── Streaming API ──────────────────────────────────────────────────────────

/**
 * Stream a message from Ollama's /api/chat endpoint.
 * Returns the same async generator interface as anthropicClient.streamMessages()
 *
 * Enhanced with:
 * - Connection retry logic
 * - Better tool call parsing (handles edge cases)
 * - Thinking/reasoning content support
 */
export async function* streamOllamaMessages(
  config: OllamaEngineConfig,
  messages: Array<{ role: 'user' | 'assistant'; content: string | ContentBlock[] }>,
  systemPrompt: string,
  tools?: EngineToolDef[],
  signal?: AbortSignal,
): AsyncGenerator<StreamEvent, SampleResult> {
  const baseUrl = config.baseUrl.replace(/\/$/, '')
  const { body } = buildOllamaChatRequest(config, messages, systemPrompt, tools)
  const requestedThinking = body.think !== undefined && body.think !== false

  const controller = new AbortController()
  if (signal) {
    signal.addEventListener('abort', () => controller.abort())
  }

  const timeoutMs = Math.max(1000, config.timeoutMs ?? 200_000)
  const timeoutHandle = setTimeout(() => controller.abort(), timeoutMs)

  // ── Retry Logic ─────────────────────────────────────────────────────────
  let response: Response
  const maxRetries = 2
  let lastError: Error | null = null

  for (let attempt = 0; attempt <= maxRetries; attempt++) {
    try {
      response = await fetch(`${baseUrl}/api/chat`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
        signal: controller.signal,
      })

      if (!response.ok) {
        const errorBody = await response.text()
        throw new Error(`Ollama API Error ${response.status}: ${errorBody}`)
      }

      lastError = null
      break
    } catch (err) {
      lastError = err instanceof Error ? err : new Error(String(err))

      // Don't retry on abort or non-retriable errors
      if (controller.signal.aborted) throw lastError
      if (lastError.message.includes('API Error 4')) throw lastError // 4xx errors

      if (attempt < maxRetries) {
        // Wait before retry (exponential backoff)
        await new Promise(r => setTimeout(r, 1000 * (attempt + 1)))
      }
    }
  }

  if (lastError) {
    clearTimeout(timeoutHandle)
    const fallbackResult = await invokeTauriChatFallback(config, messages, systemPrompt, tools)
    return fallbackResult
  }
  response = response!

  const reader = response.body?.getReader()
  if (!reader) {
    clearTimeout(timeoutHandle)
    const fallbackResult = await invokeTauriChatFallback(config, messages, systemPrompt, tools)
    return fallbackResult
  }

  const decoder = new TextDecoder()
  let buffer = ''
  let fullText = ''
  let thinkingText = ''
  const reasoningTagParser = requestedThinking ? new ReasoningTagStreamParser() : null
  const toolCalls: OllamaToolCall[] = []
  const streamedToolBlocks: ContentBlockToolUse[] = []
  const streamedToolSignatures = new Set<string>()
  let modelId = config.model
  let promptTokens = 0
  let completionTokens = 0
  let stopReason: string | null = null

  // Emit message_start
  yield {
    type: 'message_start',
    message: { id: `ollama-${Date.now()}`, model: modelId, usage: { ...EMPTY_USAGE } },
  }

  // Start a text content block
  let blockIndex = 0
  yield {
    type: 'content_block_start',
    index: 0,
    content_block: { type: 'text', text: '' },
  }

  const emitParsedContent = async function* (contentChunk: string): AsyncGenerator<StreamEvent> {
    const parsedChunks = reasoningTagParser
      ? reasoningTagParser.push(contentChunk)
      : [{ type: 'text' as const, text: contentChunk }]

    for (const parsedChunk of parsedChunks) {
      if (!parsedChunk.text) continue

      if (parsedChunk.type === 'thinking') {
        thinkingText += parsedChunk.text
        yield* streamSmoothedDelta(parsedChunk.text, 'thinking_delta')
        continue
      }

      fullText += parsedChunk.text
      yield* streamSmoothedDelta(parsedChunk.text, 'text_delta')
    }
  }

  const processStreamChunk = async function* (chunk: OllamaStreamChunk): AsyncGenerator<StreamEvent> {
    modelId = chunk.model || modelId

    const thinkingChunk = getNativeThinkingChunk(chunk)
    if (thinkingChunk) {
      thinkingText += thinkingChunk
      yield* streamSmoothedDelta(thinkingChunk, 'thinking_delta')
    }

    const contentChunk = chunk.message?.content ?? chunk.response ?? ''
    if (contentChunk) {
      yield* emitParsedContent(contentChunk)
    }

    if (chunk.message?.tool_calls) {
      const normalizedChunkToolCalls = normalizeToolCalls(chunk.message.tool_calls, tools)
      for (const toolCall of normalizedChunkToolCalls) {
        const signature = toolCallSignature(toolCall)
        if (streamedToolSignatures.has(signature)) {
          continue
        }
        streamedToolSignatures.add(signature)
        toolCalls.push(toolCall)
        blockIndex++
        const toolBlock: ContentBlockToolUse = {
          type: 'tool_use',
          id: `ollama-tool-${Date.now()}-${blockIndex}`,
          name: toolCall.function.name,
          input: toolCall.function.arguments,
        }
        streamedToolBlocks.push(toolBlock)
        yield {
          type: 'content_block_start',
          index: blockIndex,
          content_block: toolBlock,
        }
        yield { type: 'content_block_stop', index: blockIndex }
      }
    }

    if (chunk.done) {
      promptTokens = chunk.prompt_eval_count ?? 0
      completionTokens = chunk.eval_count ?? 0
      stopReason = chunk.done_reason ?? 'stop'
    }
  }

  try {
    while (true) {
      const { done, value } = await reader.read()
      if (done) break

      buffer += decoder.decode(value, { stream: true })
      const lines = buffer.split('\n')
      buffer = lines.pop() ?? ''

      for (const line of lines) {
        if (!line.trim()) continue

        let chunk: OllamaStreamChunk
        try {
          chunk = JSON.parse(line)
        } catch {
          continue
        }

        yield* processStreamChunk(chunk)

      }
    }

    if (buffer.trim()) {
      try {
        const chunk = JSON.parse(buffer) as OllamaStreamChunk
        yield* processStreamChunk(chunk)
      } catch {
        // Ignore incomplete trailing JSON.
      }
      buffer = ''
    }

    if (reasoningTagParser) {
      for (const parsedChunk of reasoningTagParser.flush()) {
        if (!parsedChunk.text) continue
        if (parsedChunk.type === 'thinking') {
          thinkingText += parsedChunk.text
          yield* streamSmoothedDelta(parsedChunk.text, 'thinking_delta')
        } else {
          fullText += parsedChunk.text
          yield* streamSmoothedDelta(parsedChunk.text, 'text_delta')
        }
      }
    }
  } finally {
    clearTimeout(timeoutHandle)
    reader.releaseLock()
  }

  // Close text content block
  yield { type: 'content_block_stop', index: 0 }

  if (!fullText.trim() && !thinkingText.trim() && toolCalls.length === 0) {
    return await invokeTauriChatFallback(config, messages, systemPrompt, tools)
  }

  // Build result content blocks
  const resultContent: ContentBlock[] = []

  // Add thinking block if present
  if (thinkingText) {
    resultContent.push({ type: 'thinking', thinking: thinkingText })
  }

  if (toolCalls.length === 0 && fullText.trim()) {
    const extracted = extractTextualToolCalls(fullText, tools)
    if (extracted.toolCalls.length > 0) {
      fullText = extracted.text
      toolCalls.push(...extracted.toolCalls)
    }
  }

  const normalizedToolCalls = normalizeToolCalls(toolCalls, tools)

  if (fullText) {
    resultContent.push({ type: 'text', text: fullText })
  }

  if (streamedToolBlocks.length > 0) {
    stopReason = 'tool_use'
    resultContent.push(...streamedToolBlocks)
  }

  const streamedSignatures = new Set(streamedToolSignatures)
  const unstreamedToolCalls = normalizedToolCalls.filter(
    (toolCall) => !streamedSignatures.has(toolCallSignature(toolCall)),
  )

  // Convert tool calls to ContentBlock tool_use format
  if (unstreamedToolCalls.length > 0) {
    stopReason = 'tool_use'
    for (const tc of unstreamedToolCalls) {
      blockIndex++
      const toolUseId = `ollama-tool-${Date.now()}-${blockIndex}`
      const toolBlock: ContentBlockToolUse = {
        type: 'tool_use',
        id: toolUseId,
        name: tc.function.name,
        input: tc.function.arguments,
      }
      resultContent.push(toolBlock)

      // Emit tool_use as a content block
      yield {
        type: 'content_block_start',
        index: blockIndex,
        content_block: toolBlock,
      }
      yield { type: 'content_block_stop', index: blockIndex }
    }
  }

  // Final usage
  const usage: TokenUsage = {
    input_tokens: promptTokens,
    output_tokens: completionTokens,
  }

  yield {
    type: 'message_delta',
    delta: { stop_reason: stopReason ?? 'stop' },
    usage,
  }
  yield { type: 'message_stop' }

  return {
    content: resultContent,
    model: modelId,
    stopReason,
    usage,
    costUsd: 0, // Local models have no cost
  }
}

function blockContentToText(content: string | ContentBlock[]): string {
  if (typeof content === 'string') return content
  return content
    .map((block) => {
      if (block.type === 'text') return block.text
      if (block.type === 'tool_result') return `[Tool Result ${block.tool_use_id}]: ${block.content}`
      if (block.type === 'thinking') return block.thinking
      return ''
    })
    .filter(Boolean)
    .join('\n')
}

async function invokeTauriChatFallback(
  config: OllamaEngineConfig,
  messages: Array<{ role: 'user' | 'assistant'; content: string | ContentBlock[] }>,
  systemPrompt: string,
  tools?: EngineToolDef[],
): Promise<SampleResult> {
  if (!canUseTauriInvoke()) {
    throw normalizeTauriInvokeError('window.__TAURI_INTERNALS__ is undefined')
  }

  const history: TauriChatHistoryItem[] = messages.map((msg) => ({
    role: msg.role,
    content: blockContentToText(msg.content),
  }))

  const lastUserIndex = [...history].map((m, i) => ({ ...m, i })).reverse().find((m) => m.role === 'user')?.i
  const prompt = lastUserIndex !== undefined ? history[lastUserIndex].content : (history.at(-1)?.content ?? '')
  const promptWithSystem = systemPrompt.trim().length > 0
    ? `System:\n${systemPrompt}\n\nUser:\n${prompt}`
    : prompt
  const priorHistory = lastUserIndex !== undefined
    ? history.slice(0, lastUserIndex)
    : history.slice(0, Math.max(0, history.length - 1))

  let fallback: TauriChatTurnResponse
  try {
    fallback = await invoke<TauriChatTurnResponse>('chat_turn', {
      request: {
        prompt: promptWithSystem,
        history: priorHistory,
        tools: tools ? toOllamaToolDefs(tools) : undefined,
        config: {
          baseUrl: config.baseUrl,
          model: config.model,
          timeoutMs: Math.max(1000, config.timeoutMs ?? 200_000),
        },
      },
    })
  } catch (error) {
    throw normalizeTauriInvokeError(error)
  }

  const extracted = extractTextualToolCalls(fallback.assistantMessage, tools)
  const content: ContentBlock[] = []
  const structuredToolCalls = fallback.toolCalls ?? []

  if (extracted.text) {
    content.push({ type: 'text', text: extracted.text })
  }

  const allToolCalls = normalizeToolCalls(structuredToolCalls.length > 0
    ? structuredToolCalls
    : extracted.toolCalls, tools)

  for (const [index, toolCall] of allToolCalls.entries()) {
    content.push({
      type: 'tool_use',
      id: `ollama-fallback-tool-${Date.now()}-${index + 1}`,
      name: toolCall.function.name,
      input: toolCall.function.arguments,
    })
  }

  return {
    content: content.length > 0 ? content : [{ type: 'text', text: fallback.assistantMessage }],
    model: fallback.model || config.model,
    stopReason: allToolCalls.length > 0 ? 'tool_use' : 'stop',
    usage: { ...EMPTY_USAGE },
    costUsd: 0,
  }
}

/**
 * Non-streaming single message call.
 */
export async function sampleOllamaMessage(
  config: OllamaEngineConfig,
  messages: Array<{ role: 'user' | 'assistant'; content: string | ContentBlock[] }>,
  systemPrompt: string,
  tools?: EngineToolDef[],
  signal?: AbortSignal,
): Promise<SampleResult> {
  const gen = streamOllamaMessages(config, messages, systemPrompt, tools, signal)
  let result: SampleResult | undefined
  while (true) {
    const { done, value } = await gen.next()
    if (done) {
      result = value as SampleResult
      break
    }
  }
  return result!
}

/**
 * Convert engine tool definitions to Ollama format.
 */
export function toOllamaToolDefs(
  tools: Array<{ name: string; description: string; input_schema: ToolInputSchema }>,
): OllamaToolDef[] {
  return tools.map((t) => ({
    type: 'function' as const,
    function: {
      name: t.name,
      description: t.description,
      parameters: t.input_schema,
    },
  }))
}

/**
 * List available models on the Ollama server.
 */
export async function listOllamaModels(
  baseUrl: string,
): Promise<Array<{ name: string; size: number; modifiedAt: string }>> {
  try {
    const response = await fetch(`${baseUrl.replace(/\/$/, '')}/api/tags`)
    if (!response.ok) return []
    const data = await response.json() as { models: Array<{ name: string; size: number; modified_at: string }> }
    return data.models.map(m => ({
      name: m.name,
      size: m.size,
      modifiedAt: m.modified_at,
    }))
  } catch {
    try {
      const health = await invoke<TauriOllamaHealthResponse>('ollama_health_check', {
        config: {
          baseUrl,
          model: '',
          timeoutMs: 15000,
        },
      })
      return (health.models ?? []).map((name) => ({
        name,
        size: 0,
        modifiedAt: '',
      }))
    } catch {
      return []
    }
  }
}

/**
 * Check if Ollama server is reachable.
 */
export async function checkOllamaConnection(baseUrl: string): Promise<boolean> {
  try {
    const response = await fetch(`${baseUrl.replace(/\/$/, '')}/api/tags`, {
      signal: AbortSignal.timeout(10000),
    })
    return response.ok
  } catch {
    try {
      const health = await invoke<TauriOllamaHealthResponse>('ollama_health_check', {
        config: {
          baseUrl,
          model: '',
          timeoutMs: 15000,
        },
      })
      return health.ok
    } catch {
      return false
    }
  }
}

/**
 * Get model info from Ollama.
 */
export async function getOllamaModelInfo(
  baseUrl: string,
  model: string,
): Promise<{ contextLength: number; parameterSize: string } | null> {
  try {
    const response = await fetch(`${baseUrl.replace(/\/$/, '')}/api/show`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ name: model }),
    })
    if (!response.ok) return null
    const data = await response.json() as Record<string, unknown>
    const params = data.model_info as Record<string, unknown> ?? {}
    return {
      contextLength: (params['context_length'] as number) ?? 0,
      parameterSize: String(data.parameters ?? ''),
    }
  } catch {
    return null
  }
}
