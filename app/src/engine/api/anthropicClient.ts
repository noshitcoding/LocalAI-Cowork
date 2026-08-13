// ── Anthropic API Client (browser-compatible) ──────────────────────────────
// Mirrors: claude-code-main/src/services/api/claude.ts
// Uses fetch() directly — works in Tauri webview without Node.js

import type {
  ContentBlock,
  StreamEvent,
  TokenUsage,
  ContentBlockDelta,
  ToolInputSchema,
} from '../types'
import { EMPTY_USAGE, accumulateUsage } from '../types'

// ── Configuration ──────────────────────────────────────────────────────────

export type AnthropicConfig = {
  apiKey: string
  model: string
  baseUrl?: string
  maxTokens?: number
  thinking?: { type: 'enabled'; budgetTokens: number } | { type: 'disabled' }
  temperature?: number
}

export const ANTHROPIC_MODELS = [
  { id: 'claude-sonnet-4-20250514', name: 'Claude Sonnet 4', tier: 'standard' },
  { id: 'claude-opus-4-20250514', name: 'Claude Opus 4', tier: 'premium' },
  { id: 'claude-3-5-haiku-20241022', name: 'Claude 3.5 Haiku', tier: 'fast' },
] as const

const DEFAULT_BASE_URL = 'https://api.anthropic.com'
const API_VERSION = '2023-06-01'
const DEFAULT_MAX_TOKENS = 16384

// ── Cost Calculation ───────────────────────────────────────────────────────

const COST_PER_MILLION: Record<string, { input: number; output: number; cacheWrite?: number; cacheRead?: number }> = {
  'claude-sonnet-4-20250514': { input: 3, output: 15, cacheWrite: 3.75, cacheRead: 0.3 },
  'claude-opus-4-20250514': { input: 15, output: 75, cacheWrite: 18.75, cacheRead: 1.5 },
  'claude-3-5-haiku-20241022': { input: 0.8, output: 4, cacheWrite: 1, cacheRead: 0.08 },
}

export function calculateCost(model: string, usage: TokenUsage): number {
  const rates = COST_PER_MILLION[model]
  if (!rates) return 0
  const m = 1_000_000
  return (
    (usage.input_tokens * rates.input) / m +
    (usage.output_tokens * rates.output) / m +
    ((usage.cache_creation_input_tokens ?? 0) * (rates.cacheWrite ?? rates.input)) / m +
    ((usage.cache_read_input_tokens ?? 0) * (rates.cacheRead ?? rates.input)) / m
  )
}

// ── API Message Types (Anthropic format) ───────────────────────────────────

type APIMessage = {
  role: 'user' | 'assistant'
  content: string | APIContentBlock[]
}

type APIContentBlock =
  | { type: 'text'; text: string }
  | { type: 'image'; source: { type: 'base64'; media_type: string; data: string } }
  | { type: 'tool_use'; id: string; name: string; input: Record<string, unknown> }
  | { type: 'tool_result'; tool_use_id: string; content: string; is_error?: boolean }
  | { type: 'thinking'; thinking: string }

type APIToolDef = {
  name: string
  description: string
  input_schema: ToolInputSchema
}

// ── Streaming API ──────────────────────────────────────────────────────────

export type StreamCallbacks = {
  onEvent?: (event: StreamEvent) => void
  onText?: (text: string) => void
  onThinking?: (thinking: string) => void
  onToolUse?: (toolUse: { id: string; name: string; input: Record<string, unknown> }) => void
  onUsage?: (usage: TokenUsage) => void
  onError?: (error: Error) => void
}

export type SampleResult = {
  content: ContentBlock[]
  model: string
  stopReason: string | null
  usage: TokenUsage
  costUsd: number
}

/**
 * Stream a message from the Anthropic API.
 * Returns an async generator of StreamEvents, with final SampleResult.
 */
export async function* streamMessages(
  config: AnthropicConfig,
  messages: APIMessage[],
  systemPrompt: string,
  tools?: APIToolDef[],
  signal?: AbortSignal,
): AsyncGenerator<StreamEvent, SampleResult> {
  const baseUrl = config.baseUrl ?? DEFAULT_BASE_URL
  const maxTokens = config.maxTokens ?? DEFAULT_MAX_TOKENS

  const body: Record<string, unknown> = {
    model: config.model,
    max_tokens: maxTokens,
    system: systemPrompt,
    messages,
    stream: true,
  }

  if (tools && tools.length > 0) {
    body.tools = tools
  }

  if (config.thinking?.type === 'enabled') {
    body.thinking = {
      type: 'enabled',
      budget_tokens: config.thinking.budgetTokens,
    }
  }

  if (config.temperature !== undefined) {
    body.temperature = config.temperature
  }

  const response = await fetch(`${baseUrl}/v1/messages`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'x-api-key': config.apiKey,
      'anthropic-version': API_VERSION,
      'anthropic-dangerous-direct-browser-access': 'true',
    },
    body: JSON.stringify(body),
    signal,
  })

  if (!response.ok) {
    const errorBody = await response.text()
    throw new AnthropicAPIError(response.status, errorBody)
  }

  const reader = response.body?.getReader()
  if (!reader) throw new Error('No response body')

  const decoder = new TextDecoder()
  let buffer = ''
  let totalUsage: TokenUsage = { ...EMPTY_USAGE }
  const resultContent: ContentBlock[] = []
  let stopReason: string | null = null
  let modelId = config.model

  // Parse accumulation state
  const currentBlocks: Map<number, ContentBlock> = new Map()
  const toolInputBuffers: Map<number, string> = new Map()

  try {
    while (true) {
      const { done, value } = await reader.read()
      if (done) break

      buffer += decoder.decode(value, { stream: true })
      const lines = buffer.split('\n')
      buffer = lines.pop() ?? ''

      for (const line of lines) {
        if (!line.startsWith('data: ')) continue
        const data = line.slice(6).trim()
        if (data === '[DONE]') continue

        let event: Record<string, unknown>
        try {
          event = JSON.parse(data)
        } catch {
          continue
        }

        const eventType = event.type as string

        switch (eventType) {
          case 'message_start': {
            const msg = event.message as Record<string, unknown>
            modelId = (msg.model as string) ?? config.model
            const msgUsage = msg.usage as Partial<TokenUsage> | undefined
            if (msgUsage) totalUsage = accumulateUsage(totalUsage, msgUsage)
            yield { type: 'message_start', message: { id: msg.id as string, model: modelId, usage: { ...totalUsage } } }
            break
          }

          case 'content_block_start': {
            const idx = event.index as number
            const block = event.content_block as ContentBlock
            currentBlocks.set(idx, block)
            if (block.type === 'tool_use') {
              toolInputBuffers.set(idx, '')
            }
            yield { type: 'content_block_start', index: idx, content_block: block }
            break
          }

          case 'content_block_delta': {
            const idx = event.index as number
            const delta = event.delta as ContentBlockDelta
            yield { type: 'content_block_delta', index: idx, delta }

            // Accumulate into current block
            const currentBlock = currentBlocks.get(idx)
            if (delta.type === 'text_delta' && currentBlock?.type === 'text') {
              (currentBlock as { text: string }).text += delta.text
            } else if (delta.type === 'thinking_delta' && currentBlock?.type === 'thinking') {
              (currentBlock as { thinking: string }).thinking += delta.thinking
            } else if (delta.type === 'input_json_delta') {
              const existing = toolInputBuffers.get(idx) ?? ''
              toolInputBuffers.set(idx, existing + delta.partial_json)
            }
            break
          }

          case 'content_block_stop': {
            const idx = event.index as number
            const block = currentBlocks.get(idx)
            // Finalize tool_use input
            if (block?.type === 'tool_use') {
              const jsonStr = toolInputBuffers.get(idx) ?? '{}'
              try {
                (block as { input: Record<string, unknown> }).input = JSON.parse(jsonStr)
              } catch {
                (block as { input: Record<string, unknown> }).input = {}
              }
              toolInputBuffers.delete(idx)
            }
            if (block) resultContent.push(block)
            yield { type: 'content_block_stop', index: idx }
            break
          }

          case 'message_delta': {
            const delta = event.delta as Record<string, unknown>
            const deltaUsage = event.usage as Partial<TokenUsage> | undefined
            stopReason = (delta.stop_reason as string) ?? stopReason
            if (deltaUsage) totalUsage = accumulateUsage(totalUsage, deltaUsage)
            yield { type: 'message_delta', delta: { stop_reason: stopReason ?? '' }, usage: deltaUsage ?? {} }
            break
          }

          case 'message_stop': {
            yield { type: 'message_stop' }
            break
          }

          case 'error': {
            const errorData = event.error as Record<string, unknown>
            throw new AnthropicAPIError(0, JSON.stringify(errorData))
          }
        }
      }
    }
  } finally {
    reader.releaseLock()
  }

  return {
    content: resultContent,
    model: modelId,
    stopReason,
    usage: totalUsage,
    costUsd: calculateCost(modelId, totalUsage),
  }
}

/**
 * Non-streaming single message call (for simple queries).
 */
export async function sampleMessage(
  config: AnthropicConfig,
  messages: APIMessage[],
  systemPrompt: string,
  tools?: APIToolDef[],
  signal?: AbortSignal,
): Promise<SampleResult> {
  const baseUrl = config.baseUrl ?? DEFAULT_BASE_URL
  const maxTokens = config.maxTokens ?? DEFAULT_MAX_TOKENS

  const body: Record<string, unknown> = {
    model: config.model,
    max_tokens: maxTokens,
    system: systemPrompt,
    messages,
  }

  if (tools && tools.length > 0) {
    body.tools = tools
  }

  if (config.thinking?.type === 'enabled') {
    body.thinking = {
      type: 'enabled',
      budget_tokens: config.thinking.budgetTokens,
    }
  }

  if (config.temperature !== undefined) {
    body.temperature = config.temperature
  }

  const response = await fetch(`${baseUrl}/v1/messages`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'x-api-key': config.apiKey,
      'anthropic-version': API_VERSION,
      'anthropic-dangerous-direct-browser-access': 'true',
    },
    body: JSON.stringify(body),
    signal,
  })

  if (!response.ok) {
    const errorBody = await response.text()
    throw new AnthropicAPIError(response.status, errorBody)
  }

  const result = await response.json()
  const usage: TokenUsage = result.usage ?? { ...EMPTY_USAGE }

  return {
    content: result.content as ContentBlock[],
    model: result.model as string,
    stopReason: result.stop_reason as string | null,
    usage,
    costUsd: calculateCost(result.model, usage),
  }
}

// ── Convert Engine Messages to API Format ──────────────────────────────────

export function toAPIMessages(messages: Array<{ role: string; content: string | ContentBlock[] }>): APIMessage[] {
  return messages
    .filter(m => m.role === 'user' || m.role === 'assistant')
    .map(m => ({
      role: m.role as 'user' | 'assistant',
      content: m.content,
    }))
}

export function toAPIToolDefs(tools: Array<{ name: string; description: string; input_schema: ToolInputSchema }>): APIToolDef[] {
  return tools.map(t => ({
    name: t.name,
    description: t.description,
    input_schema: t.input_schema,
  }))
}

// ── Error Types ────────────────────────────────────────────────────────────

export class AnthropicAPIError extends Error {
  readonly statusCode: number
  readonly body: string

  constructor(statusCode: number, body: string) {
    const parsed = tryParseJSON(body)
    const msg = parsed?.error?.message ?? body
    super(`Anthropic API Error (${statusCode}): ${msg}`)
    this.name = 'AnthropicAPIError'
    this.statusCode = statusCode
    this.body = body
  }

  get isRateLimit(): boolean { return this.statusCode === 429 }
  get isOverloaded(): boolean { return this.statusCode === 529 }
  get isAuthError(): boolean { return this.statusCode === 401 }
  get isPromptTooLong(): boolean { return this.body.includes('prompt is too long') }
  get isRetryable(): boolean { return this.isRateLimit || this.isOverloaded || this.statusCode >= 500 }
}

function tryParseJSON(s: string): Record<string, Record<string, string>> | null {
  try { return JSON.parse(s) } catch { return null }
}

// ── Retry Wrapper ──────────────────────────────────────────────────────────

export async function withRetry<T>(
  fn: () => Promise<T>,
  maxRetries = 3,
  baseDelayMs = 1000,
): Promise<T> {
  let lastError: Error | undefined
  for (let attempt = 0; attempt <= maxRetries; attempt++) {
    try {
      return await fn()
    } catch (err) {
      lastError = err instanceof Error ? err : new Error(String(err))
      if (err instanceof AnthropicAPIError && !err.isRetryable) throw err
      if (attempt < maxRetries) {
        const delay = baseDelayMs * Math.pow(2, attempt) + Math.random() * 500
        await new Promise(r => setTimeout(r, delay))
      }
    }
  }
  throw lastError
}
