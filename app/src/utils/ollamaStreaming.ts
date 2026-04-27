import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

export type ChatTurnResponse = {
  endpoint: string
  model: string
  assistantMessage: string
  requiresApproval: boolean
  proposedPlan: string[]
}

function normalizeChatTurnResponse(raw: ChatTurnResponse): ChatTurnResponse {
  const proposedPlan = Array.isArray(raw?.proposedPlan)
    ? raw.proposedPlan.filter((step): step is string => typeof step === 'string')
    : []

  return {
    endpoint: typeof raw?.endpoint === 'string' ? raw.endpoint : '',
    model: typeof raw?.model === 'string' ? raw.model : '',
    assistantMessage: typeof raw?.assistantMessage === 'string' ? raw.assistantMessage : '',
    requiresApproval: Boolean(raw?.requiresApproval),
    proposedPlan,
  }
}

export type ChatTurnRequest = {
  prompt: string
  history: Array<{ role: string; content: string }>
  config: unknown
}

type OllamaClientConfig = {
  baseUrl?: string
  model?: string
  timeoutMs?: number
  temperature?: number
}

type OllamaStreamLine = {
  response?: string
  done?: boolean
  error?: string
}

const DEFAULT_OLLAMA_BASE_URL = 'http://127.0.0.1:11434'
const DEFAULT_OLLAMA_MODEL = 'gpt-oss:20b'
const DEFAULT_OLLAMA_TIMEOUT_MS = 600000

const RISKY_PHRASES = [
  'rm -rf',
  'format c:',
  'format d:',
  'drop table',
  'drop database',
  'truncate table',
  'delete from',
  'shutdown /s',
  'shutdown -h',
  'shutdown now',
  'kill -9',
  'taskkill /f',
  'remove-item -recurse',
  'del /s /q',
  'rmdir /s',
  'registry delete',
  'reg delete',
  'netsh advfirewall',
  'iptables -f',
  'chmod 777',
  'mkfs.',
  'dd if=',
]

type OllamaChatChunk = {
  streamId: string
  chunk: string
}

type TauriWindow = Window & {
  __TAURI_INTERNALS__?: {
    invoke?: unknown
  }
  __TAURI_IPC__?: unknown
}

function createStreamId(): string {
  if (typeof crypto !== 'undefined' && 'randomUUID' in crypto) {
    return crypto.randomUUID()
  }
  return `${Date.now()}-${Math.random().toString(36).slice(2, 10)}`
}

function normalizeClientConfig(config: unknown): Required<OllamaClientConfig> {
  const parsed = (config ?? {}) as OllamaClientConfig
  const normalizedBaseUrl = (parsed.baseUrl ?? '').trim().replace(/\/+$/, '')
  const normalizedModel = (parsed.model ?? '').trim()

  return {
    baseUrl: normalizedBaseUrl || DEFAULT_OLLAMA_BASE_URL,
    model: normalizedModel || DEFAULT_OLLAMA_MODEL,
    timeoutMs: Math.max(1000, Number(parsed.timeoutMs ?? DEFAULT_OLLAMA_TIMEOUT_MS) || DEFAULT_OLLAMA_TIMEOUT_MS),
    temperature: Number.isFinite(parsed.temperature) ? Number(parsed.temperature) : 0.25,
  }
}

function buildChatPrompt(prompt: string, history: Array<{ role: string; content: string }>): string {
  const historyText = history
    .slice(-8)
    .map((message) => `${message.role}: ${message.content}`)
    .join('\n')

  return [
    'Du bist Open_Cowork, ein lokaler Assistenz-Agent. Antworte knapp, klar und auf Deutsch.',
    'Wenn die Aufgabe riskante oder destruktive Aktionen enthalten koennte, schlage einen Plan vor und kennzeichne das als approval-beduerftig.',
    'WICHTIGE REGELN:',
    "- Gib niemals Platzhalter- oder Warte-Antworten wie 'ich analysiere', 'bitte warten', 'kommt gleich' oder aehnliches aus.",
    '- Gib immer direkt eine finale, inhaltliche Antwort in genau dieser Nachricht.',
    '- Erfinde niemals Dokumentinhalte.',
    '- Wenn nur ein Dateipfad vorhanden ist, aber kein extrahierter Dokumenttext im Prompt steht, sage klar, dass der Inhalt nicht vorliegt und bitte um dokumentierten Textauszug oder aktivierte Dateianalyse.',
    '',
    'Kontextverlauf:',
    historyText,
    `Nutzer: ${prompt}`,
    '',
    'Antwort:',
  ].join('\n')
}

function parseSteps(raw: string): string[] {
  const parsed = raw
    .split('\n')
    .map((line) => line.trim().replace(/^[\d.)\-\s]+/, '').trim())
    .filter(Boolean)

  if (parsed.length === 0) {
    return [raw.trim()]
  }

  return parsed
}

function detectRiskyAction(text: string): boolean {
  const normalized = text.toLowerCase()
  return RISKY_PHRASES.some((phrase) => normalized.includes(phrase))
}

function buildResponse(config: Required<OllamaClientConfig>, prompt: string, assistantMessage: string): ChatTurnResponse {
  const requiresApproval = detectRiskyAction(prompt) || detectRiskyAction(assistantMessage)
  const proposedPlan = requiresApproval
    ? parseSteps(assistantMessage).slice(0, 6)
    : []

  return {
    endpoint: config.baseUrl,
    model: config.model,
    assistantMessage,
    requiresApproval,
    proposedPlan,
  }
}

function parseStreamLine(line: string): OllamaStreamLine | null {
  const normalized = line.trim().replace(/^data:\s*/, '')
  if (!normalized) return null

  try {
    return JSON.parse(normalized) as OllamaStreamLine
  } catch {
    return null
  }
}

async function callOllamaGenerate(
  config: Required<OllamaClientConfig>,
  request: ChatTurnRequest,
  streaming: boolean,
  onChunk?: (chunk: string) => void,
): Promise<string> {
  const abortController = new AbortController()
  const timeoutHandle = window.setTimeout(() => abortController.abort(), config.timeoutMs)

  try {
    const response = await fetch(`${config.baseUrl}/api/generate`, {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
      },
      body: JSON.stringify({
        model: config.model,
        prompt: buildChatPrompt(request.prompt, request.history),
        stream: streaming,
        options: {
          temperature: config.temperature,
        },
      }),
      signal: abortController.signal,
    })

    if (!response.ok) {
      const errorBody = (await response.text()).trim()
      throw new Error(
        errorBody
          ? `Ollama returned status ${response.status}: ${errorBody}`
          : `Ollama returned status ${response.status}`,
      )
    }

    if (!streaming) {
      const payload = await response.json() as { response?: string }
      return payload.response ?? ''
    }

    if (!response.body) {
      return ''
    }

    const reader = response.body.getReader()
    const decoder = new TextDecoder()
    let buffered = ''
    let assistantMessage = ''

    while (true) {
      const { done, value } = await reader.read()
      if (done) break

      buffered += decoder.decode(value, { stream: true })

      while (true) {
        const newlineIndex = buffered.indexOf('\n')
        if (newlineIndex < 0) break

        const line = buffered.slice(0, newlineIndex)
        buffered = buffered.slice(newlineIndex + 1)
        const parsed = parseStreamLine(line)
        if (!parsed) continue
        if (parsed.error) throw new Error(parsed.error)

        const chunk = parsed.response ?? ''
        if (chunk) {
          assistantMessage += chunk
          onChunk?.(chunk)
        }
      }
    }

    const trailing = parseStreamLine(buffered)
    if (trailing?.error) throw new Error(trailing.error)
    if (trailing?.response) {
      assistantMessage += trailing.response
      onChunk?.(trailing.response)
    }

    return assistantMessage
  } catch (error) {
    if (error instanceof DOMException && error.name === 'AbortError') {
      throw new Error(`Zeitueberschreitung nach ${config.timeoutMs}ms beim Aufruf von ${config.baseUrl}/api/generate`)
    }

    const message = error instanceof Error ? error.message : String(error)
    if (message.toLowerCase().includes('failed to fetch')) {
      throw new Error(
        `Ollama ist aus dem Browser nicht erreichbar (${config.baseUrl}). Pruefe Endpoint, HTTPS/CORS und ob Ollama laeuft.`,
      )
    }

    throw error instanceof Error ? error : new Error(message)
  } finally {
    window.clearTimeout(timeoutHandle)
  }
}

async function streamChatTurnViaHttp(
  request: ChatTurnRequest,
  onChunk: (chunk: string) => void,
): Promise<ChatTurnResponse> {
  const config = normalizeClientConfig(request.config)
  let assistantMessage = await callOllamaGenerate(config, request, true, onChunk)

  if (!assistantMessage.trim()) {
    assistantMessage = await callOllamaGenerate(config, request, false)
  }

  if (!assistantMessage.trim()) {
    throw new Error('Das Modell hat keine sichtbare Antwort geliefert. Bitte Modell/Prompt pruefen.')
  }

  return buildResponse(config, request.prompt, assistantMessage)
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
      'Tauri-Bridge nicht verfuegbar. Die App laeuft vermutlich nicht als Tauri-Desktop-App. Starte Open_Cowork mit "npm run tauri dev" (oder als gebaute Desktop-App).',
    )
  }

  return error instanceof Error ? error : new Error(message)
}

async function invokeChatTurnFallback(request: ChatTurnRequest): Promise<ChatTurnResponse> {
  if (!canUseTauriInvoke()) {
    throw normalizeTauriInvokeError('window.__TAURI_INTERNALS__ is undefined')
  }

  try {
    const response = await invoke<ChatTurnResponse>('chat_turn', {
      request,
    })
    return normalizeChatTurnResponse(response)
  } catch (error) {
    throw normalizeTauriInvokeError(error)
  }
}

export async function streamChatTurn(
  request: ChatTurnRequest,
  onChunk: (chunk: string) => void,
): Promise<ChatTurnResponse> {
  if (!canUseTauriInvoke()) {
    return await streamChatTurnViaHttp(request, onChunk)
  }

  const streamId = createStreamId()
  let unlisten: (() => void) | null = null

  try {
    unlisten = await listen<OllamaChatChunk>('ollama-chat-chunk', (event) => {
      if (event.payload.streamId === streamId) {
        onChunk(event.payload.chunk)
      }
    })
  } catch {
    // Fallback for environments where window event subscriptions are unavailable.
    return await invokeChatTurnFallback(request)
  }

  try {
    try {
      const response = await invoke<ChatTurnResponse>('chat_turn_stream', {
        request: {
          ...request,
          streamId,
        },
      })
      return normalizeChatTurnResponse(response)
    } catch (error) {
      throw normalizeTauriInvokeError(error)
    }
  } catch (streamError) {
    try {
      return await invokeChatTurnFallback(request)
    } catch (fallbackError) {
      const streamMessage = streamError instanceof Error ? streamError.message : String(streamError)
      const fallbackMessage = normalizeTauriInvokeError(fallbackError).message
      throw new Error(`${streamMessage}\nFallback fehlgeschlagen: ${fallbackMessage}`)
    }
  } finally {
    if (unlisten) {
      unlisten()
    }
  }
}
