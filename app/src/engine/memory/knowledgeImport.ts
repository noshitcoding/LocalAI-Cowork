export type KnowledgeImportEntry = {
  id: string
  scope: 'shared'
  category: 'knowledge'
  key: string
  content: string
  confidence: number
}

const DEFAULT_CHUNK_CHARS = 4000
const DEFAULT_OVERLAP_CHARS = 320
const MAX_SOURCE_CHARS = 500_000
const MAX_CHUNKS = 128

function normalizeKnowledgeText(value: string): string {
  return value
    .replace(/\0/g, '')
    .replace(/\r\n?/g, '\n')
    .replace(/[ \t]+\n/g, '\n')
    .replace(/\n{4,}/g, '\n\n\n')
    .trim()
}

function safeTitle(value: string): string {
  return value.trim().replace(/\s+/g, ' ').slice(0, 120) || 'Knowledge import'
}

export function chunkKnowledgeText(
  value: string,
  chunkChars = DEFAULT_CHUNK_CHARS,
  overlapChars = DEFAULT_OVERLAP_CHARS,
): string[] {
  const text = normalizeKnowledgeText(value).slice(0, MAX_SOURCE_CHARS)
  if (!text) return []

  const characters = Array.from(text)
  const boundedChunkChars = Math.max(800, Math.min(12_000, chunkChars))
  const boundedOverlap = Math.max(0, Math.min(Math.floor(boundedChunkChars / 3), overlapChars))
  const chunks: string[] = []
  let start = 0

  while (start < characters.length && chunks.length < MAX_CHUNKS) {
    let end = Math.min(characters.length, start + boundedChunkChars)
    if (end < characters.length) {
      const minimumBoundary = start + Math.floor(boundedChunkChars * 0.6)
      for (let index = end; index > minimumBoundary; index -= 1) {
        if (characters[index - 1] === '\n' || characters[index - 1] === ' ') {
          end = index
          break
        }
      }
    }

    const chunk = characters.slice(start, end).join('').trim()
    if (chunk) chunks.push(chunk)
    if (end >= characters.length) break
    const nextStart = Math.max(start + 1, end - boundedOverlap)
    start = nextStart
  }

  return chunks
}

export function buildKnowledgeImportEntries(
  title: string,
  text: string,
  idFactory: () => string = () => crypto.randomUUID(),
): KnowledgeImportEntry[] {
  const normalizedTitle = safeTitle(title)
  const chunks = chunkKnowledgeText(text)
  return chunks.map((content, index) => ({
    id: idFactory(),
    scope: 'shared',
    category: 'knowledge',
    key: chunks.length === 1 ? normalizedTitle : `${normalizedTitle} (${index + 1}/${chunks.length})`,
    content,
    confidence: 1,
  }))
}
