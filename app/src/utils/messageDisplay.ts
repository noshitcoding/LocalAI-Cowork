type ThinkingTagPair = readonly [string, string]

const THINKING_TAG_PAIRS: ThinkingTagPair[] = [
  ['<think>', '</think>'],
  ['<thinking>', '</thinking>'],
  ['<reason>', '</reason>'],
  ['<reasoning>', '</reasoning>'],
  ['<thought>', '</thought>'],
  ['<|begin_of_thought|>', '<|end_of_thought|>'],
  ['\u25c1think\u25b7', '\u25c1/think\u25b7'],
]

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

function startTagPattern(tag: string): string {
  const match = tag.match(/^<([^>\s]+)>$/)
  if (!match) return escapeRegExp(tag)
  return `<${escapeRegExp(match[1])}(?:\\s[^>]*)?>`
}

export function stripModelThinking(content: string): string {
  const stripped = THINKING_TAG_PAIRS.reduce((current, [startTag, endTag]) => {
    const pattern = new RegExp(`${startTagPattern(startTag)}[\\s\\S]*?(?:${escapeRegExp(endTag)}|$)`, 'gi')
    return current.replace(pattern, '')
  }, content)

  return stripped
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
    /\[SYSTEM-CONTEXT\][\s\S]*?\[\/SYSTEM-CONTEXT\]\s*/gi,
    ''
  )
  const withoutAttachmentContext = verboseMode ? withoutSystemContext : withoutSystemContext.replace(
    /(?:^|\n)Connectede Pfade \(\d+\):\n(?:\d+\.\s+(?:File|Folder):[^\n]*(?:\n|$))+/gi,
    '\n'
  )

  return withoutAttachmentContext
    .replace(/\n{3,}/g, '\n\n')
    .trim()
}

function extractThinkingFallback(rawContent: string): string {
  return extractThinkingContent(rawContent)
}

export function extractThinkingContent(rawContent: string): string {
  const blocks: string[] = []

  for (const [startTag, endTag] of THINKING_TAG_PAIRS) {
    const pattern = new RegExp(`${startTagPattern(startTag)}([\\s\\S]*?)(?:${escapeRegExp(endTag)}|$)`, 'gi')
    let match: RegExpExecArray | null
    while ((match = pattern.exec(rawContent)) !== null) {
      blocks.push(match[1].trim())
    }
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
