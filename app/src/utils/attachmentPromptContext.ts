import {
  formatAttachmentContext,
  getAttachmentDisplayName,
  getPathName,
  isImageAttachment,
  type ChatAttachment,
} from './chatAttachments'
import { safeInvoke, safeInvokeVoid } from './safeInvoke'

type ImportedAttachmentResponse = {
  originalPath: string
  importedPath: string
  fileName: string
  sizeBytes: number
}

type ExtractTextLimitedResponse = {
  text: string
  chars: number
  truncated: boolean
}

type FsAttachmentMetadataEntry = {
  path: string
  fileName: string
  extension: string | null
  language: string | null
  sizeBytes: number
}

type FsAttachmentMetadataResponse = {
  rootPath: string
  rootKind: 'file' | 'folder'
  totalFiles: number
  returnedFiles: number
  truncated: boolean
  files: FsAttachmentMetadataEntry[]
}

type RetrievalCandidate = {
  readPath: string
  displayPath: string
  origin: string
  extension: string | null
  language: string | null
  sizeBytes: number
}

const FOLDER_METADATA_LIMIT = 1200
const MAX_RANKING_LINES = 120
const MAX_FOLDER_FILES_TO_READ = 36
const MAX_DIRECT_FILES_TO_READ = 12
const RETRIEVAL_SNIPPETS_PER_FILE = 4
const ATTACHMENT_TEXT_PASS_1 = 40_000
const ATTACHMENT_TEXT_PASS_2 = 80_000
const SNIPPET_WINDOW = 400
const DIRECT_FILE_FULL_READ_LIMIT = 80_000
const STOP_WORDS = new Set([
  'the', 'and', 'oder', 'und', 'with', 'from', 'that', 'this', 'eine', 'einer', 'einem', 'eines',
  'ein', 'der', 'die', 'das', 'for', 'auf', 'mit', 'als', 'von', 'to', 'wie', 'what', 'where',
  'wenn', 'then', 'else', 'bitte', 'kann', 'kannst', 'soll', 'sollte', 'please', 'into', 'durch',
])

export type AttachmentPromptBuildResult = {
  context: string
  parsedFiles: number
  failedFiles: Array<{ path: string; error: string }>
}

async function allowFolderAttachments(attachments: ChatAttachment[]): Promise<void> {
  const roots = new Map<string, string>()

  for (const item of attachments) {
    if (item.kind === 'folder') {
      roots.set(item.path.toLowerCase(), item.path)
    }
  }

  for (const root of roots.values()) {
    await safeInvokeVoid('fs_add_allowed_folder', { path: root })
  }
}

function extractQueryTerms(query: string): string[] {
  const terms = query
    .toLowerCase()
    .replace(/[^a-z0-9_\-.\s]/gi, ' ')
    .split(/\s+/)
    .filter((token) => token.length >= 3 && !STOP_WORDS.has(token))

  return Array.from(new Set(terms)).slice(0, 14)
}

function scoreCandidate(candidate: RetrievalCandidate, queryTerms: string[], rawQuery: string): number {
  const haystack = `${candidate.displayPath} ${candidate.origin}`.toLowerCase()
  let score = 0

  for (const term of queryTerms) {
    if (haystack.includes(term)) {
      score += 8
    }
  }

  if (rawQuery.includes('test') && candidate.displayPath.toLowerCase().includes('test')) score += 5
  if (rawQuery.includes('config') && candidate.displayPath.toLowerCase().includes('config')) score += 5
  if ((rawQuery.includes('fehler') || rawQuery.includes('error')) && candidate.displayPath.toLowerCase().includes('log')) score += 3

  if (candidate.language) score += 1
  if ((candidate.extension ?? '').includes('md')) score -= 2

  return score
}

function trimSnippet(input: string): string {
  return input.replace(/\s+/g, ' ').trim()
}

function collectSnippets(text: string, queryTerms: string[], maxSnippets: number): string[] {
  if (!text) return []

  if (queryTerms.length === 0) {
    const preview = trimSnippet(text.slice(0, SNIPPET_WINDOW * 2))
    return preview ? [preview] : []
  }

  const lower = text.toLowerCase()
  const snippets: string[] = []
  const usedOffsets: number[] = []

  for (const term of queryTerms) {
    if (snippets.length >= maxSnippets) break
    const idx = lower.indexOf(term)
    if (idx < 0) continue
    if (usedOffsets.some((offset) => Math.abs(offset - idx) < SNIPPET_WINDOW)) continue

    const start = Math.max(0, idx - SNIPPET_WINDOW)
    const end = Math.min(text.length, idx + term.length + SNIPPET_WINDOW)
    const snippet = trimSnippet(text.slice(start, end))
    if (!snippet) continue

    snippets.push(snippet)
    usedOffsets.push(idx)
  }

  return snippets
}

function pushCandidate(target: RetrievalCandidate, entries: Map<string, RetrievalCandidate>): void {
  if (!entries.has(target.readPath)) {
    entries.set(target.readPath, target)
  }
}

export async function buildAttachmentPromptContext(
  attachments: ChatAttachment[],
  retrievalQuery: string,
): Promise<AttachmentPromptBuildResult> {
  const imageAttachments = attachments.filter((item) => item.kind === 'file' && isImageAttachment(item))
  const contextualAttachments = attachments.filter((item) => item.kind === 'folder' || !isImageAttachment(item))
  const baseSections: string[] = []
  const pathContext = formatAttachmentContext(contextualAttachments)
  if (pathContext) {
    baseSections.push(pathContext)
  }
  if (imageAttachments.length > 0) {
    baseSections.push([
      `Image-attachments (${imageAttachments.length}):`,
      ...imageAttachments.map((item, index) => `${index + 1}. Image: ${getAttachmentDisplayName(item)}`),
    ].join('\n'))
  }
  const baseContext = baseSections.join('\n\n')
  const fileAttachments = contextualAttachments.filter((item) => item.kind === 'file')
  const folderAttachments = contextualAttachments.filter((item) => item.kind === 'folder')
  const queryTerms = extractQueryTerms(retrievalQuery)
  const normalizedQuery = retrievalQuery.toLowerCase()
  const wantsFullFileList = /alle\s+datei|complete(e|en)?\s+dateiliste|list(e)?\s+mit\s+allen\s+dateien|all\s+files|full\s+file\s+list/.test(normalizedQuery)

  if (fileAttachments.length === 0 && folderAttachments.length === 0) {
    return {
      context: baseContext,
      parsedFiles: 0,
      failedFiles: [],
    }
  }

  const metadataLines: string[] = []
  const retrievalLines: string[] = []
  const failedFiles: Array<{ path: string; error: string }> = []
  const importedFiles = new Map<string, ImportedAttachmentResponse>()
  const candidates = new Map<string, RetrievalCandidate>()
  let parsedFiles = 0
  let deepReads = 0

  try {
    await allowFolderAttachments(attachments)
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error)
    failedFiles.push({ path: '(Folder-attachments)', error: `Folder permission failed: ${message}` })
  }

  for (const item of fileAttachments) {
    try {
      const imported = await safeInvoke<ImportedAttachmentResponse>('fs_import_attachment', {
        path: item.path,
      })
      importedFiles.set(item.path, imported)

      const metadata = await safeInvoke<FsAttachmentMetadataResponse>('fs_collect_attachment_metadata', {
        path: imported.importedPath,
        maxEntries: 1,
      })
      const first = metadata.files[0]
      if (first) {
        metadataLines.push(
          `- File ${getPathName(imported.originalPath)} | Groesse ${first.sizeBytes} B | Sprache ${first.language ?? 'unknown'}`,
        )
        pushCandidate(
          {
            readPath: imported.importedPath,
            displayPath: imported.originalPath,
            origin: `File-attachment ${imported.originalPath}`,
            extension: first.extension,
            language: first.language,
            sizeBytes: first.sizeBytes,
          },
          candidates,
        )
      }
    } catch (error) {
      failedFiles.push({
        path: item.path,
        error: `Import into app folder failed: ${error instanceof Error ? error.message : String(error)}`,
      })
    }
  }

  for (const folder of folderAttachments) {
    try {
      const metadata = await safeInvoke<FsAttachmentMetadataResponse>('fs_collect_attachment_metadata', {
        path: folder.path,
        maxEntries: wantsFullFileList ? 50_000 : FOLDER_METADATA_LIMIT,
      })
      metadataLines.push(
        `- Folder ${folder.path} | total files ${metadata.totalFiles} | inspected ${metadata.returnedFiles}${metadata.truncated ? ' (truncated)' : ''}`,
      )

      if (wantsFullFileList && metadata.files.length > 0) {
        metadataLines.push(...metadata.files.map((entry: FsAttachmentMetadataEntry) => `  - ${entry.path}`))
      }

      for (const entry of metadata.files) {
        pushCandidate(
          {
            readPath: entry.path,
            displayPath: entry.path,
            origin: `Folder-attachment ${folder.path}`,
            extension: entry.extension,
            language: entry.language,
            sizeBytes: entry.sizeBytes,
          },
          candidates,
        )
      }
    } catch (error) {
      failedFiles.push({
        path: folder.path,
        error: error instanceof Error ? error.message : String(error),
      })
    }
  }

  const ranked = Array.from(candidates.values())
    .map((candidate) => ({
      candidate,
      score: scoreCandidate(candidate, queryTerms, normalizedQuery),
    }))
    .sort((a, b) => b.score - a.score)

  if (!wantsFullFileList && ranked.length > 0) {
    const visibleRanked = ranked.slice(0, MAX_RANKING_LINES)
    retrievalLines.push('Selektierte Kandidaten (Ranking):')
    retrievalLines.push(
      ...visibleRanked.map(
        ({ candidate, score }, index) =>
          `${index + 1}. ${getPathName(candidate.displayPath)} | Score ${score} | Sprache ${candidate.language ?? 'unknown'} | ${candidate.origin}`,
      ),
    )
    if (ranked.length > visibleRanked.length) {
      retrievalLines.push(`... ${ranked.length - visibleRanked.length} additional candidates hidden`)
    }
  }

  // Direct file attachments: read full text instead of snippets
  const directFilePaths = new Set(fileAttachments.map((f) => f.path.toLowerCase()))
  const isDirectFile = (candidate: RetrievalCandidate) =>
    directFilePaths.has(candidate.displayPath.toLowerCase()) ||
    fileAttachments.some((f) => candidate.readPath.toLowerCase().endsWith(f.path.split(/[\\/]/).pop()!.toLowerCase()))

  let folderReads = 0
  let directReads = 0
  let skippedByReadLimit = 0

  for (const { candidate } of ranked) {
    if (wantsFullFileList) {
      // For full-list queries, metadata is enough; full-text retrieval is skipped.
      continue
    }

    const directCandidate = isDirectFile(candidate)
    if (directCandidate) {
      if (directReads >= MAX_DIRECT_FILES_TO_READ) {
        skippedByReadLimit += 1
        continue
      }
      directReads += 1
    } else {
      if (folderReads >= MAX_FOLDER_FILES_TO_READ) {
        skippedByReadLimit += 1
        continue
      }
      folderReads += 1
    }

    try {
      if (directCandidate) {
        // Full text for directly attached files (PDFs, docs, etc.)
        const fullRead = await safeInvoke<ExtractTextLimitedResponse>('fs_extract_text_limited', {
          path: candidate.readPath,
          maxChars: DIRECT_FILE_FULL_READ_LIMIT,
        })
        parsedFiles += 1
        if (fullRead.text.trim().length > 0) {
          retrievalLines.push(`Volltext von ${candidate.displayPath}${fullRead.truncated ? ' (truncated)' : ''}:`)
          retrievalLines.push(fullRead.text)
        }
      } else {
        // Snippet-based retrieval for folder candidates
        const pass1 = await safeInvoke<ExtractTextLimitedResponse>('fs_extract_text_limited', {
          path: candidate.readPath,
          maxChars: ATTACHMENT_TEXT_PASS_1,
        })
        parsedFiles += 1

        let snippets = collectSnippets(pass1.text, queryTerms, RETRIEVAL_SNIPPETS_PER_FILE)

        if (snippets.length === 0 && pass1.truncated) {
          const pass2 = await safeInvoke<ExtractTextLimitedResponse>('fs_extract_text_limited', {
            path: candidate.readPath,
            maxChars: ATTACHMENT_TEXT_PASS_2,
          })
          deepReads += 1
          snippets = collectSnippets(pass2.text, queryTerms, RETRIEVAL_SNIPPETS_PER_FILE)
        }

        if (snippets.length === 0) {
          continue
        }

        retrievalLines.push(`Treffer in ${candidate.displayPath}:`)
        retrievalLines.push(...snippets.map((snippet, idx) => `- Snippet ${idx + 1}: ${snippet}`))
      }
    } catch (error) {
      failedFiles.push({
        path: candidate.displayPath,
        error: error instanceof Error ? error.message : String(error),
      })
    }
  }

  if (retrievalLines.length > 0) {
    retrievalLines.push(`Iterative deepening active: ${deepReads} file(s) with a second read pass.`)
    if (skippedByReadLimit > 0) {
      retrievalLines.push(
        `Retrieval limited for performance reasons: ${skippedByReadLimit} file(s) were skipped in this pass.`,
      )
    }
  }

  const failedBlock =
    failedFiles.length > 0
      ? [
          'Unprocessable attachments:',
          ...failedFiles.map((entry) => `- ${entry.path}: ${entry.error}`),
        ].join('\n')
      : ''

  const metadataBlock =
      metadataLines.length > 0 ? ['File metadata (without full text):', ...metadataLines].join('\n') : ''

  const analysisBlock =
    retrievalLines.length > 0 ? ['Retrieval-Context (selektiv geread):', ...retrievalLines].join('\n') : ''

  const context = [baseContext, metadataBlock, analysisBlock, failedBlock]
    .filter((part) => part.trim().length > 0)
    .join('\n\n')

  return {
    context,
    parsedFiles,
    failedFiles,
  }
}
