export type WebSearchSource = {
  title: string
  url: string
  snippet?: string
}

function normalizeUrl(url: string): string {
  const trimmed = url.trim()
  if (!trimmed) return ''

  try {
    return `url:${new URL(trimmed).href}`
  } catch {
    return `invalid:${trimmed}`
  }
}

export function mergeWebSearchSources(
  existing: WebSearchSource[],
  incoming: WebSearchSource[],
): WebSearchSource[] {
  const merged: WebSearchSource[] = []
  const seen = new Set<string>()

  for (const source of [...existing, ...incoming]) {
    const key = normalizeUrl(source.url)
    if (!key || seen.has(key)) continue
    seen.add(key)
    merged.push(source)
  }

  return merged
}

export function parseWebSearchSourcesFromToolResult(result: string): WebSearchSource[] {
  const trimmed = result.trim()
  if (!trimmed || /^(No results|Web-Suche failed)/i.test(trimmed)) {
    return []
  }

  const parsed: WebSearchSource[] = []
  let titleLines: string[] = []
  let url = ''
  let snippetLines: string[] = []

  const finishSource = () => {
    if (url) {
      const title = titleLines.join(' ').replace(/^\d+[.)]\s*/, '').trim()
      const snippet = snippetLines.join(' ').trim()
      parsed.push({ title: title || url, url, snippet: snippet || undefined })
    }
    titleLines = []
    url = ''
    snippetLines = []
  }

  for (const rawLine of trimmed.split(/\r\n|\r|\n/)) {
    const line = rawLine.trim()
    if (!line) {
      if (url) finishSource()
      continue
    }

    const isUrl = /^https?:\/\/\S+$/i.test(line)
    const isNumberedTitle = /^\d+[.)]\s+\S/.test(line)
    if ((isNumberedTitle || isUrl) && url) {
      finishSource()
    }

    if (isUrl) {
      url = line
    } else if (url) {
      snippetLines.push(line)
    } else {
      titleLines.push(line)
    }
  }
  finishSource()

  return mergeWebSearchSources([], parsed)
}

export function formatWebSearchSourcesBlock(sources: WebSearchSource[]): string {
  if (sources.length === 0) return ''

  return [
    'Sources:',
    ...sources.map((source, index) => {
      const lines = [`${index + 1}. ${source.title}`, source.url]
      if (source.snippet) {
        lines.push(`   ${source.snippet}`)
      }
      return lines.join('\n')
    }),
  ].join('\n\n')
}

export function appendWebSearchSources(answer: string, sources: WebSearchSource[]): string {
  const trimmedAnswer = answer.trim()
  const normalizedSources = mergeWebSearchSources([], sources)

  if (normalizedSources.length === 0) {
    return trimmedAnswer
  }

  if (normalizedSources.every((source) => trimmedAnswer.includes(source.url))) {
    return trimmedAnswer
  }

  const block = formatWebSearchSourcesBlock(normalizedSources)
  return trimmedAnswer ? `${trimmedAnswer}\n\n${block}` : block
}

export function extractWebSearchSources(answer: string): {
  content: string
  sources: WebSearchSource[]
} {
  const trimmedAnswer = answer.trim()
  if (!trimmedAnswer) {
    return { content: '', sources: [] }
  }

  const marker = '\n\nSources:\n'
  const inlineMarkerIndex = trimmedAnswer.lastIndexOf(marker)
  const blockStart = inlineMarkerIndex >= 0
    ? inlineMarkerIndex + marker.length
    : trimmedAnswer.startsWith('Sources:\n')
      ? 'Sources:\n'.length
      : -1

  if (blockStart === -1) {
    return { content: trimmedAnswer, sources: [] }
  }

  const content = inlineMarkerIndex >= 0
    ? trimmedAnswer.slice(0, inlineMarkerIndex).trim()
    : ''
  const sourcesBlock = trimmedAnswer.slice(blockStart).trim()
  const sources = parseWebSearchSourcesFromToolResult(sourcesBlock)

  if (sources.length === 0) {
    return { content: trimmedAnswer, sources: [] }
  }

  return { content, sources }
}
