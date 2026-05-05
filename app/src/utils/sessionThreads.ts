import { extractTextContent, type ContentBlock, type Message } from '../engine/types'
import { loadSession, type SessionRecord } from '../engine/services/sessionPersistence'
import type { ChatMessage, ChatThread } from '../stores/chatStore'
import { extractAttachmentsFromContent } from './chatAttachments'

const STORED_CHAT_MESSAGE_KIND = 'open-cowork-chat-message'

function createFallbackUuid(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID()
  }
  return `legacy-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
}

function clipText(content: string, maxLength = 400): string {
  const trimmed = content.trim()
  if (trimmed.length <= maxLength) return trimmed
  return `${trimmed.slice(0, maxLength)}...`
}

function stringifySnippet(value: unknown, maxLength = 180): string {
  try {
    return clipText(JSON.stringify(value), maxLength)
  } catch {
    return ''
  }
}

function summarizeContentBlocks(blocks: ContentBlock[]): string {
  const summaries: string[] = []
  const thinkingBlocks: string[] = []

  for (const block of blocks) {
    switch (block.type) {
      case 'tool_use': {
        const inputSnippet = stringifySnippet(block.input)
        summaries.push(inputSnippet ? `Tool-Aufruf: ${block.name} ${inputSnippet}` : `Tool-Aufruf: ${block.name}`)
        break
      }
      case 'tool_result': {
        const label = block.is_error ? 'Tool-Fehler' : 'Tool-Ergebnis'
        summaries.push(`${label}: ${clipText(block.content)}`)
        break
      }
      case 'thinking':
        if (block.thinking.trim()) {
          thinkingBlocks.push(block.thinking.trim())
        }
        break
    }
  }

  if (summaries.length > 0) {
    return summaries.join('\n\n')
  }

  if (thinkingBlocks.length > 0) {
    return `Analyse: ${clipText(thinkingBlocks.join('\n\n'))}`
  }

  return ''
}

function resolveReadableContent(message: Message): string {
  const textContent = extractTextContent(message).trim()
  if (textContent) {
    return textContent
  }

  if ('content' in message && Array.isArray(message.content)) {
    const summarizedBlocks = summarizeContentBlocks(message.content)
    if (summarizedBlocks) {
      return summarizedBlocks
    }
  }

  return `[${message.type}]`
}

function shouldExposeSerializedMessage(message: Message): boolean {
  return 'content' in message && Array.isArray(message.content) && message.content.some((block) => block.type !== 'text')
}

function resolveDisplayRole(message: Message): ChatMessage['role'] {
  if (
    message.type === 'user' &&
    Array.isArray(message.content) &&
    message.content.length > 0 &&
    message.content.every((block) => block.type === 'tool_result')
  ) {
    return 'assistant'
  }

  if (message.type === 'user') {
    return 'user'
  }

  if (message.type === 'assistant' || message.type === 'tool_use_summary') {
    return 'assistant'
  }

  return 'system'
}

function isChatRole(value: unknown): value is ChatMessage['role'] {
  return value === 'user' || value === 'assistant' || value === 'system'
}

function parseStoredChatMessagePayload(rawContent: string): ChatMessage | null {
  const trimmed = rawContent.trim()
  if (!trimmed.startsWith('{') || !trimmed.includes(STORED_CHAT_MESSAGE_KIND)) {
    return null
  }

  try {
    const parsed = JSON.parse(trimmed) as unknown
    if (!parsed || typeof parsed !== 'object') return null

    const payload = parsed as Record<string, unknown>
    if (payload.kind !== STORED_CHAT_MESSAGE_KIND || !payload.message || typeof payload.message !== 'object') {
      return null
    }

    const message = payload.message as Record<string, unknown>
    if (!isChatRole(message.role)) {
      return null
    }

    return {
      id: typeof message.id === 'string' ? message.id : createFallbackUuid(),
      role: message.role,
      content: typeof message.content === 'string' ? message.content : '',
      timestamp: typeof message.timestamp === 'number' ? message.timestamp : Date.now(),
      attachments: Array.isArray(message.attachments) ? message.attachments as ChatMessage['attachments'] : undefined,
      visibleInChat: typeof message.visibleInChat === 'boolean' ? message.visibleInChat : undefined,
      debugContent: typeof message.debugContent === 'string' ? message.debugContent : undefined,
      thinkingContent: typeof message.thinkingContent === 'string' ? message.thinkingContent : undefined,
      verboseContent: typeof message.verboseContent === 'string' ? message.verboseContent : undefined,
      liveToolCalls: Array.isArray(message.liveToolCalls) ? message.liveToolCalls as ChatMessage['liveToolCalls'] : undefined,
      crewLive: message.crewLive && typeof message.crewLive === 'object'
        ? message.crewLive as ChatMessage['crewLive']
        : undefined,
      streaming: false,
    }
  } catch {
    return null
  }
}

export function serializeChatMessageForStorage(message: ChatMessage): string {
  return JSON.stringify({
    kind: STORED_CHAT_MESSAGE_KIND,
    version: 1,
    message: {
      id: message.id,
      role: message.role,
      content: typeof message.content === 'string' ? message.content : '',
      timestamp: message.timestamp,
      attachments: message.attachments,
      visibleInChat: message.visibleInChat,
      debugContent: message.debugContent,
      thinkingContent: message.thinkingContent,
      verboseContent: message.verboseContent,
      liveToolCalls: message.liveToolCalls,
      crewLive: message.crewLive,
      streaming: false,
    },
  })
}

function toChatMessage(message: Message, index: number, persistedRawContent?: string): ChatMessage {
  const role = resolveDisplayRole(message)
  const timestamp = 'timestamp' in message && typeof message.timestamp === 'number'
    ? message.timestamp
    : Date.now() + index
  const visibleContent = resolveReadableContent(message)
  const extracted = role === 'user'
    ? extractAttachmentsFromContent(visibleContent)
    : { content: visibleContent, attachments: [] }
  const serializedMessage = shouldExposeSerializedMessage(message)
    ? (persistedRawContent ?? JSON.stringify(message))
    : undefined

  return {
    id: 'uuid' in message && typeof message.uuid === 'string'
      ? message.uuid
      : `${timestamp}-${index}`,
    role,
    content: extracted.content,
    timestamp,
    attachments: extracted.attachments.length > 0 ? extracted.attachments : undefined,
    debugContent: extracted.attachments.length > 0
      ? visibleContent
      : serializedMessage,
  }
}

export function parsePersistedSessionMessage(rawContent: string): Message | null {
  const trimmed = rawContent.trim()
  if (!trimmed.startsWith('{') || (!trimmed.includes('"type"') && !trimmed.includes('"role"'))) {
    return null
  }

  try {
    const parsed = JSON.parse(trimmed) as unknown
    if (!parsed || typeof parsed !== 'object') return null

    const message = parsed as Record<string, unknown>
    const timestamp = typeof message.timestamp === 'number' ? message.timestamp : Date.now()
    const uuid = typeof message.uuid === 'string' ? message.uuid : createFallbackUuid()

    if (typeof message.type === 'string') {
      return message as Message
    }

    const role = typeof message.role === 'string' ? message.role : ''
    const textContent = typeof message.content === 'string' ? message.content : ''

    if (role === 'user') {
      return {
        type: 'user',
        uuid,
        content: [{ type: 'text', text: textContent }],
        timestamp,
      }
    }

    if (role === 'assistant') {
      return {
        type: 'assistant',
        uuid,
        content: [{ type: 'text', text: textContent }],
        model: typeof message.model === 'string' ? message.model : 'legacy',
        stopReason: null,
        usage: { input_tokens: 0, output_tokens: 0 },
        timestamp,
      }
    }

    if (role === 'system') {
      return {
        type: 'system',
        uuid,
        content: textContent,
        timestamp,
      }
    }

    return null
  } catch {
    return null
  }
}

export function hydrateStoredMessage(record: {
  id: string
  role: string
  content: string
  timestamp: number
}): ChatMessage {
  const storedMessage = parseStoredChatMessagePayload(record.content)
  if (storedMessage) {
    return {
      ...storedMessage,
      id: record.id,
      timestamp: record.timestamp,
    }
  }

  const parsedMessage = parsePersistedSessionMessage(record.content)
  if (parsedMessage) {
    return {
      ...toChatMessage(parsedMessage, 0, record.content),
      id: record.id,
      timestamp: record.timestamp,
    }
  }

  return {
    id: record.id,
    role: record.role as ChatMessage['role'],
    content: typeof record.content === 'string' ? record.content : '',
    timestamp: record.timestamp,
  }
}

export function toChatMessages(messages: Message[]): ChatMessage[] {
  return messages.map((message, index) => toChatMessage(message, index))
}

export function toChatThread(session: SessionRecord): ChatThread {
  return {
    id: session.id,
    title: session.title,
    messages: toChatMessages(session.messages),
    createdAt: session.createdAt,
    updatedAt: session.updatedAt,
  }
}

export async function resolveSessionRecord(
  sessionId: string,
  loadSessionById: (sessionId: string) => Promise<SessionRecord | null>,
): Promise<SessionRecord | null> {
  return await loadSessionById(sessionId) ?? await loadSession(sessionId)
}
