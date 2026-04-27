import { extractTextContent, loadSession, type ContentBlock, type Message, type SessionRecord } from '../engine'
import type { ChatMessage, ChatThread } from '../stores/chatStore'
import { extractAttachmentsFromContent } from './chatAttachments'

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
