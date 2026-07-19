import type {
  ContentBlock,
  StreamEvent,
  TokenUsage,
  ToolInputSchema,
} from '../types'
import { hasTauriRuntime, safeInvoke } from '../../utils/safeInvoke'
import {
  EMPTY_USAGE,
  generateUUID,
} from '../types'

export type OpenAiCompatibleConfig = {
  provider: 'openai-compatible' | 'openrouter'
  apiKey: string
  model: string
  baseUrl: string
  timeoutMs?: number
  verifyTlsCertificates?: boolean
  temperature?: number
  maxTokens?: number
}

type OpenAiCompatibleChatCompletionResult = {
  status: number
  body: string
}

type OpenAiCompatibleModelsResult = {
  endpoint: string
  models: string[]
}

type OpenAiModelsResponse = {
  data?: Array<{
    id?: unknown
    name?: unknown
  }>
}

type APIMessage = {
  role: 'user' | 'assistant'
  content: string | ContentBlock[]
}

type APIToolDef = {
  name: string
  description: string
  input_schema: ToolInputSchema
}

type OpenAiCompatibleRequestMessage =
  | { role: 'system'; content: string }
  | { role: 'user'; content: string | OpenAiCompatibleContentPart[] }
  | { role: 'assistant'; content: string | null; reasoning?: string | null; tool_calls?: OpenAiCompatibleToolCall[] }
  | { role: 'tool'; content: string; tool_call_id: string }

type OpenAiCompatibleContentPart =
  | { type: 'text'; text: string }
  | { type: 'image_url'; image_url: { url: string } }

type OpenAiCompatibleToolCall = {
  id: string
  type: 'function'
  function: {
    name: string
    arguments: string
  }
}

type OpenAiCompatibleChoiceError = {
  code?: number
  message?: string
  metadata?: Record<string, unknown>
}

type OpenAiCompatibleResponse = {
  id?: string
  model?: string
  usage?: {
    prompt_tokens?: number
    completion_tokens?: number
  }
  choices?: Array<{
    finish_reason?: string | null
    error?: OpenAiCompatibleChoiceError
    message?: {
      content?: string | Array<{ type?: string; text?: string; content?: string }> | null
      reasoning?: string | null
      reasoning_content?: string | null
      reasoning_details?: unknown[] | null
      tool_calls?: Array<{
        id?: string
        function?: {
          name?: string
          arguments?: string
        }
      }>
    }
  }>
}

type OpenAiCompatibleResponseMessage = NonNullable<NonNullable<OpenAiCompatibleResponse['choices']>[number]['message']>

type RequestFailure = Error & {
  status?: number
  bodyText?: string
}

function getProviderLabel(provider: OpenAiCompatibleConfig['provider']): string {
  return provider === 'openrouter' ? 'OpenRouter' : 'OpenAI-compatible provider'
}

function buildEndpoint(baseUrl: string): string {
  const trimmed = baseUrl.trim().replace(/\/+$/, '')
  if (!trimmed) {
    throw new Error('Endpoint fehlt')
  }

  if (trimmed.endsWith('/chat/completions')) {
    return trimmed
  }

  if (trimmed.endsWith('/models')) {
    const withoutModels = trimmed.replace(/\/models$/, '')
    if (withoutModels.endsWith('/v1')) {
      return `${withoutModels}/chat/completions`
    }
    if (isServiceRootEndpoint(withoutModels)) {
      return `${withoutModels}/v1/chat/completions`
    }
    return `${withoutModels}/chat/completions`
  }

  if (trimmed.endsWith('/v1')) {
    return `${trimmed}/chat/completions`
  }

  if (isServiceRootEndpoint(trimmed)) {
    return `${trimmed}/v1/chat/completions`
  }

  return `${trimmed}/chat/completions`
}

function buildModelsEndpoint(baseUrl: string): string {
  const trimmed = baseUrl.trim().replace(/\/+$/, '')
  if (!trimmed) {
    throw new Error('Endpoint fehlt')
  }

  if (trimmed.endsWith('/models')) {
    return trimmed
  }

  if (trimmed.endsWith('/chat/completions')) {
    const withoutChatCompletions = trimmed.replace(/\/chat\/completions$/, '')
    return `${withoutChatCompletions}/models`
  }

  if (trimmed.endsWith('/v1')) {
    return `${trimmed}/models`
  }

  if (isServiceRootEndpoint(trimmed)) {
    return `${trimmed}/v1/models`
  }

  return `${trimmed}/models`
}

function isServiceRootEndpoint(value: string): boolean {
  try {
    const parsed = new URL(value)
    return parsed.pathname.replace(/\/+$/, '') === ''
  } catch {
    return false
  }
}

function createAbortSignal(timeoutMs: number | undefined, signal?: AbortSignal): { signal: AbortSignal; cleanup: () => void } {
  const controller = new AbortController()
  let timeoutHandle: ReturnType<typeof setTimeout> | null = null

  const abortFromSource = () => {
    try {
      controller.abort(signal?.reason)
    } catch {
      controller.abort()
    }
  }

  if (signal) {
    if (signal.aborted) {
      abortFromSource()
    } else {
      signal.addEventListener('abort', abortFromSource, { once: true })
    }
  }

  if (timeoutMs && Number.isFinite(timeoutMs) && timeoutMs > 0) {
    timeoutHandle = setTimeout(() => {
      controller.abort(new Error(`Request timed out after ${timeoutMs}ms`))
    }, timeoutMs)
  }

  return {
    signal: controller.signal,
    cleanup: () => {
      if (timeoutHandle) clearTimeout(timeoutHandle)
      if (signal) signal.removeEventListener('abort', abortFromSource)
    },
  }
}

function blocksToUserContent(blocks: ContentBlock[]): string | OpenAiCompatibleContentPart[] | null {
  const contentParts: OpenAiCompatibleContentPart[] = []

  for (const block of blocks) {
    if (block.type === 'text' && block.text.trim()) {
      contentParts.push({ type: 'text', text: block.text })
      continue
    }

    if (block.type === 'image') {
      contentParts.push({
        type: 'image_url',
        image_url: {
          url: `data:${block.source.media_type};base64,${block.source.data}`,
        },
      })
    }
  }

  if (contentParts.length === 0) return null
  if (contentParts.length === 1 && contentParts[0].type === 'text') {
    return contentParts[0].text
  }

  return contentParts
}

function userContentHasImageParts(content: string | OpenAiCompatibleContentPart[]): boolean {
  return Array.isArray(content) && content.some((part) => part.type === 'image_url')
}

function requestMessagesContainImages(messages: OpenAiCompatibleRequestMessage[]): boolean {
  return messages.some((message) => message.role === 'user' && userContentHasImageParts(message.content))
}

function stripImagesFromUserContent(content: string | OpenAiCompatibleContentPart[]): string | OpenAiCompatibleContentPart[] | null {
  if (typeof content === 'string') return content

  const textParts = content.filter(
    (part): part is Extract<OpenAiCompatibleContentPart, { type: 'text' }> => part.type === 'text' && part.text.trim().length > 0,
  )

  if (textParts.length === 0) return null
  if (textParts.length === 1) return textParts[0].text
  return textParts
}

function stripImagePartsFromMessages(messages: OpenAiCompatibleRequestMessage[]): OpenAiCompatibleRequestMessage[] {
  const sanitized: OpenAiCompatibleRequestMessage[] = []

  for (const message of messages) {
    if (message.role !== 'user') {
      sanitized.push(message)
      continue
    }

    const nextContent = stripImagesFromUserContent(message.content)
    if (nextContent === null) {
      continue
    }

    sanitized.push({ role: 'user', content: nextContent })
  }

  return sanitized
}

function isUnsupportedOpenRouterImageInputError(error: unknown): boolean {
  if (!(error instanceof Error)) return false

  return /OpenRouter API Error \(404\):/i.test(error.message)
    && /No endpoints found that support image input/i.test(error.message)
}

function toOpenAiCompatibleMessages(
  messages: APIMessage[],
  systemPrompt: string,
  provider: OpenAiCompatibleConfig['provider'],
): OpenAiCompatibleRequestMessage[] {
  const result: OpenAiCompatibleRequestMessage[] = []

  if (systemPrompt.trim()) {
    result.push({ role: 'system', content: systemPrompt })
  }

  for (const message of messages) {
    const blocks = typeof message.content === 'string'
      ? [{ type: 'text', text: message.content } satisfies ContentBlock]
      : message.content

    if (message.role === 'assistant') {
      const textContent = blocks
        .filter((block): block is Extract<ContentBlock, { type: 'text' }> => block.type === 'text')
        .map((block) => block.text)
        .join('\n\n')
        .trim()

      const thinkingContent = blocks
        .filter((block): block is Extract<ContentBlock, { type: 'thinking' }> => block.type === 'thinking')
        .map((block) => block.thinking)
        .join('\n\n')
        .trim()

      const toolCalls = blocks
        .filter((block): block is Extract<ContentBlock, { type: 'tool_use' }> => block.type === 'tool_use')
        .map((block) => ({
          id: block.id,
          type: 'function' as const,
          function: {
            name: block.name,
            arguments: JSON.stringify(block.input ?? {}),
          },
        }))

      if (textContent || toolCalls.length > 0 || (provider === 'openrouter' && thinkingContent)) {
        result.push({
          role: 'assistant',
          content: textContent || null,
          ...(provider === 'openrouter' && thinkingContent ? { reasoning: thinkingContent } : {}),
          ...(toolCalls.length > 0 ? { tool_calls: toolCalls } : {}),
        })
      }

      continue
    }

    let bufferedBlocks: ContentBlock[] = []
    const flushBufferedUserContent = () => {
      const content = blocksToUserContent(bufferedBlocks)
      if (content !== null) {
        result.push({ role: 'user', content })
      }
      bufferedBlocks = []
    }

    for (const block of blocks) {
      if (block.type === 'tool_result') {
        flushBufferedUserContent()
        result.push({
          role: 'tool',
          content: block.content,
          tool_call_id: block.tool_use_id,
        })
        continue
      }

      if (block.type === 'thinking' || block.type === 'tool_use') {
        continue
      }

      bufferedBlocks.push(block)
    }

    flushBufferedUserContent()
  }

  return result
}

function toOpenAiCompatibleToolDefs(tools?: APIToolDef[]): Array<{ type: 'function'; function: { name: string; description: string; parameters: ToolInputSchema } }> | undefined {
  if (!tools || tools.length === 0) return undefined

  return tools.map((tool) => ({
    type: 'function',
    function: {
      name: tool.name,
      description: tool.description,
      parameters: tool.input_schema,
    },
  }))
}

function parseToolInput(raw: string | undefined): Record<string, unknown> {
  if (!raw?.trim()) return {}

  try {
    const parsed = JSON.parse(raw)
    return parsed && typeof parsed === 'object' ? parsed as Record<string, unknown> : {}
  } catch {
    return {}
  }
}

function extractTextContent(content: string | Array<{ type?: string; text?: string; content?: string }> | null | undefined): string {
  if (typeof content === 'string') return content
  if (!Array.isArray(content)) return ''

  return content
    .map((part) => {
      if (typeof part?.text === 'string') return part.text
      if (typeof part?.content === 'string') return part.content
      return ''
    })
    .filter(Boolean)
    .join('\n')
}

function extractReasoningDetailText(value: unknown): string {
  if (!value || typeof value !== 'object') return ''

  const detail = value as Record<string, unknown>
  const directFields = ['text', 'content', 'reasoning', 'summary']
  for (const field of directFields) {
    const fieldValue = detail[field]
    if (typeof fieldValue === 'string' && fieldValue.trim()) {
      return fieldValue.trim()
    }
    if (Array.isArray(fieldValue)) {
      const text = fieldValue.map(extractReasoningDetailText).filter(Boolean).join('\n')
      if (text.trim()) return text.trim()
    }
  }

  return ''
}

function extractReasoningContent(message: OpenAiCompatibleResponseMessage | undefined): string {
  const reasoning = message?.reasoning?.trim() || message?.reasoning_content?.trim() || ''
  if (reasoning) return reasoning

  return (message?.reasoning_details ?? [])
    .map(extractReasoningDetailText)
    .filter(Boolean)
    .join('\n\n')
    .trim()
}

function formatChoiceError(providerLabel: string, error: OpenAiCompatibleChoiceError | undefined): string | null {
  if (!error) return null

  const message = typeof error.message === 'string' && error.message.trim()
    ? error.message.trim()
    : 'Unknown Error'
  const code = typeof error.code === 'number' ? ` (${error.code})` : ''

  return `${providerLabel} API Error${code}: ${message}`
}

function mapStopReason(
  finishReason: string | null | undefined,
  hasToolCalls: boolean,
): 'end_turn' | 'tool_use' | 'max_tokens' | 'stop_sequence' | null {
  if (hasToolCalls || finishReason === 'tool_calls') return 'tool_use'
  if (finishReason === 'length') return 'max_tokens'
  if (finishReason === 'stop') return 'end_turn'
  return null
}

function parseProviderModels(body: string): string[] {
  const payload = JSON.parse(body) as OpenAiModelsResponse
  const models = (Array.isArray(payload.data) ? payload.data : [])
    .map((entry) => {
      const id = typeof entry.id === 'string' ? entry.id.trim() : ''
      const name = typeof entry.name === 'string' ? entry.name.trim() : ''
      return id || name
    })
    .filter(Boolean)

  return Array.from(new Set(models)).sort()
}

function modelNameSuffix(model: string): string {
  const trimmed = model.trim()
  return trimmed.split('/').filter(Boolean).at(-1) ?? trimmed
}

function findModelSuggestion(models: string[], requestedModel: string): string | null {
  const requested = requestedModel.trim()
  if (!requested) return null

  const normalizedModels = models.map((model) => model.trim()).filter(Boolean)
  const lowerRequested = requested.toLowerCase()
  const exact = normalizedModels.find((model) => model.toLowerCase() === lowerRequested)
  if (exact) return null

  const suffix = normalizedModels.find((model) => modelNameSuffix(model).toLowerCase() === lowerRequested)
  if (suffix) return suffix

  return normalizedModels.length === 1 ? normalizedModels[0] : null
}

function isModelNotFoundFailure(error: unknown): error is RequestFailure {
  if (!(error instanceof Error)) return false

  const failure = error as RequestFailure
  if (failure.status !== 404) return false

  const bodyText = failure.bodyText ?? ''
  const normalized = `${error.message}\n${bodyText}`.toLowerCase()
  return normalized.includes('model')
    && (
      normalized.includes('notfound')
      || normalized.includes('not found')
      || normalized.includes('not exist')
      || normalized.includes('does not exist')
      || normalized.includes('not available')
    )
}

async function fetchProviderModels(
  config: OpenAiCompatibleConfig,
  headers: Record<string, string>,
  signal: AbortSignal,
): Promise<string[]> {
  if (config.verifyTlsCertificates === false && hasTauriRuntime()) {
    const result = await safeInvoke<OpenAiCompatibleModelsResult>('crew_provider_models_list', {
      request: {
        providerKind: config.provider,
        baseUrl: config.baseUrl,
        apiKey: config.apiKey,
        model: config.model,
        verifyTlsCertificates: config.verifyTlsCertificates,
      },
    })
    return Array.isArray(result.models) ? result.models : []
  }

  const response = await fetch(buildModelsEndpoint(config.baseUrl), {
    method: 'GET',
    headers,
    signal,
  })

  if (!response.ok) {
    return []
  }

  return parseProviderModels(await response.text())
}

async function resolveModelNotFoundSuggestion(
  config: OpenAiCompatibleConfig,
  headers: Record<string, string>,
  error: unknown,
  requestedModel: string,
  signal: AbortSignal,
): Promise<string | null> {
  if (!isModelNotFoundFailure(error)) return null

  try {
    return findModelSuggestion(
      await fetchProviderModels(config, headers, signal),
      requestedModel,
    )
  } catch {
    return null
  }
}

export type SampleResult = {
  content: ContentBlock[]
  model: string
  stopReason: 'end_turn' | 'tool_use' | 'max_tokens' | 'stop_sequence' | null
  usage: TokenUsage
  costUsd: number
}

export async function* streamOpenAiCompatibleMessages(
  config: OpenAiCompatibleConfig,
  messages: APIMessage[],
  systemPrompt: string,
  tools?: APIToolDef[],
  signal?: AbortSignal,
): AsyncGenerator<StreamEvent, SampleResult> {
  const providerLabel = getProviderLabel(config.provider)

  if (!config.apiKey.trim()) {
    throw new Error(`${providerLabel} API-Key fehlt.`)
  }
  if (!config.model.trim()) {
    throw new Error(`${providerLabel} Model fehlt.`)
  }

  const endpoint = buildEndpoint(config.baseUrl)
  let requestMessages = toOpenAiCompatibleMessages(messages, systemPrompt, config.provider)
  const timeoutSignal = createAbortSignal(config.timeoutMs, signal)

  try {
    const headers: Record<string, string> = {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${config.apiKey.trim()}`,
    }

    if (config.provider === 'openrouter') {
      headers['HTTP-Referer'] = 'https://localai-cowork.local'
      headers['X-Title'] = 'LocalAI Cowork'
    }

    let requestModel = config.model.trim()

    const executeRequest = async (
      messagesForRequest: OpenAiCompatibleRequestMessage[],
      model: string,
    ): Promise<OpenAiCompatibleResponse> => {
      const body: Record<string, unknown> = {
        model,
        messages: messagesForRequest,
        stream: false,
      }

      if (config.provider === 'openrouter') {
        body.reasoning = {
          enabled: true,
          exclude: false,
        }
      }

      const toolDefs = toOpenAiCompatibleToolDefs(tools)
      if (toolDefs) {
        body.tools = toolDefs
        body.tool_choice = 'auto'
      }

      if (config.temperature !== undefined) {
        body.temperature = config.temperature
      }

      if (config.maxTokens !== undefined) {
        body.max_tokens = config.maxTokens
      }

      const bodyJson = JSON.stringify(body)
      const shouldUseNativeRequest = config.verifyTlsCertificates === false && hasTauriRuntime()

      if (shouldUseNativeRequest) {
        const result = await safeInvoke<OpenAiCompatibleChatCompletionResult>('openai_compatible_chat_completion', {
          request: {
            endpoint,
            headers,
            body: bodyJson,
            timeoutMs: config.timeoutMs,
            verifyTlsCertificates: config.verifyTlsCertificates,
          },
        })

        if (result.status < 200 || result.status >= 300) {
          const excerpt = result.body.slice(0, 400)
          const requestError = new Error(`${providerLabel} API Error (${result.status}): ${excerpt || 'HTTP request failed'}`) as RequestFailure
          requestError.status = result.status
          requestError.bodyText = result.body
          throw requestError
        }

        return JSON.parse(result.body) as OpenAiCompatibleResponse
      }

      const response = await fetch(endpoint, {
        method: 'POST',
        headers,
        body: bodyJson,
        signal: timeoutSignal.signal,
      })

      if (!response.ok) {
        const bodyText = await response.text()
        const excerpt = bodyText.slice(0, 400)
        const requestError = new Error(`${providerLabel} API Error (${response.status}): ${excerpt || response.statusText}`) as RequestFailure
        requestError.status = response.status
        requestError.bodyText = bodyText
        throw requestError
      }

      return response.json() as Promise<OpenAiCompatibleResponse>
    }

    let payload: OpenAiCompatibleResponse
    const retryWithModelSuggestion = async (
      requestError: unknown,
      messagesForRetry: OpenAiCompatibleRequestMessage[],
    ): Promise<OpenAiCompatibleResponse> => {
      const suggestedModel = await resolveModelNotFoundSuggestion(
        config,
        headers,
        requestError,
        requestModel,
        timeoutSignal.signal,
      )
      if (!suggestedModel) {
        throw requestError
      }

      requestModel = suggestedModel
      return executeRequest(messagesForRetry, requestModel)
    }

    try {
      payload = await executeRequest(requestMessages, requestModel)
    } catch (error) {
      if (
        config.provider === 'openrouter'
        && requestMessagesContainImages(requestMessages)
        && isUnsupportedOpenRouterImageInputError(error)
      ) {
        requestMessages = stripImagePartsFromMessages(requestMessages)
        try {
          payload = await executeRequest(requestMessages, requestModel)
        } catch (retryError) {
          payload = await retryWithModelSuggestion(retryError, requestMessages)
        }
      } else {
        payload = await retryWithModelSuggestion(error, requestMessages)
      }
    }

    const choice = payload.choices?.[0]
    const choiceError = formatChoiceError(providerLabel, choice?.error)
    if (choiceError) {
      throw new Error(choiceError)
    }

    const message = choice?.message
    const reasoningContent = extractReasoningContent(message)
    const textContent = extractTextContent(message?.content).trim()
    const toolCalls = message?.tool_calls ?? []
    const model = payload.model ?? requestModel
    const usage: TokenUsage = {
      input_tokens: payload.usage?.prompt_tokens ?? 0,
      output_tokens: payload.usage?.completion_tokens ?? 0,
    }

    const resultContent: ContentBlock[] = []
    if (reasoningContent) {
      resultContent.push({ type: 'thinking', thinking: reasoningContent })
    }

    if (textContent) {
      resultContent.push({ type: 'text', text: textContent })
    }

    for (const toolCall of toolCalls) {
      resultContent.push({
        type: 'tool_use',
        id: toolCall.id ?? generateUUID(),
        name: toolCall.function?.name?.trim() || 'unknown_tool',
        input: parseToolInput(toolCall.function?.arguments),
      })
    }

    if (resultContent.length === 0) {
      const finishReason = choice?.finish_reason?.trim()
      const finishReasonSuffix = finishReason && finishReason !== 'stop'
        ? ` (finish_reason: ${finishReason})`
        : ''
      throw new Error(`${providerLabel} did not return a response${finishReasonSuffix}.`)
    }

    const stopReason = mapStopReason(choice?.finish_reason, toolCalls.length > 0)
    const messageId = payload.id ?? generateUUID()

    yield {
      type: 'message_start',
      message: {
        id: messageId,
        model,
        usage: { ...EMPTY_USAGE },
      },
    }

    let index = 0
    for (const block of resultContent) {
      yield {
        type: 'content_block_start',
        index,
        content_block: block,
      }

      if (block.type === 'text') {
        yield {
          type: 'content_block_delta',
          index,
          delta: {
            type: 'text_delta',
            text: block.text,
          },
        }
      }

      if (block.type === 'thinking') {
        yield {
          type: 'content_block_delta',
          index,
          delta: {
            type: 'thinking_delta',
            thinking: block.thinking,
          },
        }
      }

      yield { type: 'content_block_stop', index }
      index += 1
    }

    yield {
      type: 'message_delta',
      delta: { stop_reason: stopReason ?? '' },
      usage,
    }
    yield { type: 'message_stop' }

    return {
      content: resultContent,
      model,
      stopReason,
      usage,
      costUsd: 0,
    }
  } finally {
    timeoutSignal.cleanup()
  }
}
