export function stripModelThinking(content: string): string {
  return content
    .replace(/<think>[\s\S]*?(?:<\/think>|$)/gi, '')
    .replace(/^\s*thinking:[\s\S]*?\n\n/gi, '')
    .trim()
}

export function sanitizeAssistantContent(rawContent: string, verboseMode: boolean): string {
  const withoutThinking = stripModelThinking(rawContent)

  // If stripping thinking tags left the content empty, the model likely put its
  // entire answer inside <think> tags (common with reasoning models like gemma4).
  // Fall back to showing the thinking content as the visible response.
  const effective = withoutThinking.trim()
    ? withoutThinking
    : extractThinkingFallback(rawContent)

  const withoutSystemContext = verboseMode ? effective : effective.replace(
    /\[SYSTEM-KONTEXT\][\s\S]*?\[\/SYSTEM-KONTEXT\]\s*/gi,
    ''
  )
  const withoutAttachmentContext = verboseMode ? withoutSystemContext : withoutSystemContext.replace(
    /(?:^|\n)Verbundene Pfade \(\d+\):\n(?:\d+\.\s+(?:Datei|Ordner):[^\n]*(?:\n|$))+/gi,
    '\n'
  )

  return withoutAttachmentContext
    .replace(/\n{3,}/g, '\n\n')
    .trim()
}

function extractThinkingFallback(rawContent: string): string {
  const blocks: string[] = []
  const thinkRegex = /<think>([\s\S]*?)<\/think>/gi
  let match: RegExpExecArray | null
  while ((match = thinkRegex.exec(rawContent)) !== null) {
    blocks.push(match[1].trim())
  }
  // Handle unclosed <think> tag (model didn't close it)
  if (blocks.length === 0) {
    const unclosed = rawContent.match(/<think>([\s\S]*)$/i)
    if (unclosed?.[1]) {
      blocks.push(unclosed[1].trim())
    }
  }
  return blocks.join('\n\n')
}

export function extractThinkingContent(rawContent: string): string {
  const blocks: string[] = []
  const thinkRegex = /<think>([\s\S]*?)(?:<\/think>|$)/gi
  let match: RegExpExecArray | null

  while ((match = thinkRegex.exec(rawContent)) !== null) {
    blocks.push(match[1].trim())
  }

  const thinkingPrefix = rawContent.match(/^\s*thinking:\s*([\s\S]*?)(?:\n\n|$)/i)
  if (thinkingPrefix?.[1]) {
    blocks.unshift(thinkingPrefix[1].trim())
  }

  return blocks.filter(Boolean).join('\n\n')
}

export function limitRollingLines(content: string, maxLines: number): string {
  const lines = content.split(/\r?\n/)
  if (lines.length <= maxLines) return content
  return lines.slice(-maxLines).join('\n')
}

export function buildModelDebugContent(rawContent: string, visibleContent: string): string | undefined {
  const rawWithoutThinking = stripModelThinking(rawContent)
  return rawWithoutThinking !== visibleContent ? rawWithoutThinking : undefined
}

const OLLAMA_REQUEST_PREVIEW_MARKER = '[OLLAMA REQUEST PREVIEW]'

export function splitPromptDebugContent(debugContent: string | undefined): {
  promptDebug?: string
  ollamaRequestPreview?: string
} {
  const normalized = (debugContent || '').trim()
  if (!normalized) return {}

  const markerIndex = normalized.indexOf(OLLAMA_REQUEST_PREVIEW_MARKER)
  if (markerIndex < 0) {
    return { promptDebug: normalized }
  }

  const promptDebug = normalized.slice(0, markerIndex).trim()
  const ollamaRequestPreview = normalized
    .slice(markerIndex + OLLAMA_REQUEST_PREVIEW_MARKER.length)
    .trim()

  return {
    promptDebug: promptDebug || undefined,
    ollamaRequestPreview: ollamaRequestPreview || undefined,
  }
}

export function resolveDisplayedThinkingContent(
  messageThinkingContent: string | undefined,
  liveThinkingContent: string | undefined,
  options?: { streaming?: boolean; preferLive?: boolean },
): string | undefined {
  const messageThinking = (messageThinkingContent || '').trim()
  const liveThinking = (liveThinkingContent || '').trim()

  if (!options?.streaming || !options.preferLive || !liveThinking) {
    return messageThinking || undefined
  }

  if (!messageThinking || liveThinking.length >= messageThinking.length) {
    return liveThinking
  }

  return messageThinking
}

export function resolveDisplayedAssistantContent(
  content: string,
  thinkingContent: string | undefined,
): string {
  const normalizedContent = content.trim()
  const normalizedThinking = (thinkingContent || '').trim()

  if (!normalizedContent) {
    return ''
  }

  if (normalizedThinking && normalizedContent === normalizedThinking) {
    return ''
  }

  return content
}

type AssistantPresentationOptions = {
  verboseMode: boolean
  fallbackText?: string
  thinkingContent?: string
}

export function resolveAssistantPresentation(
  rawContent: string,
  options: AssistantPresentationOptions,
): { content: string; thinkingContent?: string; debugContent?: string } {
  const visibleContent = sanitizeAssistantContent(rawContent, options.verboseMode)
  const thinkingContent = (extractThinkingContent(rawContent) || options.thinkingContent || '').trim()
  const fallbackText = (options.fallbackText || '').trim()
  const resolvedContent = visibleContent || thinkingContent || fallbackText

  return {
    content: resolvedContent,
    thinkingContent: thinkingContent || undefined,
    debugContent: rawContent
      ? (options.verboseMode ? rawContent : buildModelDebugContent(rawContent, resolvedContent))
      : undefined,
  }
}
