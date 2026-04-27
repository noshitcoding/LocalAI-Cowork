export type WebSearchSource = {
  title: string
  url: string
  snippet?: string
}

function normalizeUrl(url: string): string {
  return url.trim().toLowerCase()
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
  if (!trimmed || /^(Keine Treffer|Web-Suche fehlgeschlagen)/i.test(trimmed)) {
    return []
  }

  const parsed: WebSearchSource[] = []

  for (const block of trimmed.split(/\r?\n\r?\n+/).map((value) => value.trim()).filter(Boolean)) {
    const lines = block
      .split(/\r?\n/)
      .map((line) => line.trim())
      .filter(Boolean)

    const urlIndex = lines.findIndex((line) => /^https?:\/\//i.test(line))
    if (urlIndex === -1) {
      continue
    }

    const title = lines.slice(0, urlIndex).join(' ').replace(/^\d+\.\s*/, '').trim()
    const url = lines[urlIndex].trim()
    const snippet = lines.slice(urlIndex + 1).join(' ').trim()
    if (!url) {
      continue
    }

    parsed.push({
      title: title || url,
      url,
      snippet: snippet || undefined,
    })
  }

  return mergeWebSearchSources([], parsed)
}

export function formatWebSearchSourcesBlock(sources: WebSearchSource[]): string {
  if (sources.length === 0) return ''

  return [
    'Quellen:',
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

  const marker = '\n\nQuellen:\n'
  const inlineMarkerIndex = trimmedAnswer.lastIndexOf(marker)
  const blockStart = inlineMarkerIndex >= 0
    ? inlineMarkerIndex + marker.length
    : trimmedAnswer.startsWith('Quellen:\n')
      ? 'Quellen:\n'.length
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