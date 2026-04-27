import { convertFileSrc } from '@tauri-apps/api/core'
import type { ContentBlockImage } from '../engine'

export const MAX_CHAT_ATTACHMENTS = 25

export type ChatAttachmentKind = 'file' | 'folder'

export type ChatAttachment = {
  path: string
  kind: ChatAttachmentKind
  label?: string
  mediaType?: string
  dataUrl?: string
  source?: 'path' | 'inline'
}

type FileWithPath = File & { path?: string }

export function normalizeDialogSelection(selection: string | string[] | null): string[] {
  if (!selection) return []
  return Array.isArray(selection) ? selection : [selection]
}

export function mergeAttachments(
  existing: ChatAttachment[],
  additions: ChatAttachment[],
): { next: ChatAttachment[]; rejectedCount: number } {
  const seen = new Set(existing.map((item) => `${item.kind}::${item.path.toLowerCase()}`))
  const uniqueAdditions = additions.filter((item) => {
    const key = `${item.kind}::${item.path.toLowerCase()}`
    if (seen.has(key)) return false
    seen.add(key)
    return true
  })

  const room = Math.max(0, MAX_CHAT_ATTACHMENTS - existing.length)
  const accepted = uniqueAdditions.slice(0, room)
  return {
    next: [...existing, ...accepted],
    rejectedCount: uniqueAdditions.length - accepted.length,
  }
}

export function getPathName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path
}

export function getAttachmentDisplayName(attachment: ChatAttachment): string {
  const label = attachment.label?.trim()
  if (label) return label
  return getPathName(attachment.path)
}

const IMAGE_ATTACHMENT_PATH_REGEX = /\.(avif|bmp|gif|ico|jpe?g|png|svg|webp)$/i

export function isImageAttachmentPath(path: string): boolean {
  const normalized = path.trim().split(/[?#]/, 1)[0]
  return IMAGE_ATTACHMENT_PATH_REGEX.test(normalized)
}

export function isImageAttachment(attachment: ChatAttachment): boolean {
  if (attachment.kind !== 'file') return false
  if (attachment.mediaType?.toLowerCase().startsWith('image/')) return true
  if (/^data:image\//i.test(attachment.dataUrl?.trim() ?? '')) return true
  return isImageAttachmentPath(attachment.path)
}

export function hasLocalAttachmentPath(attachment: ChatAttachment): boolean {
  return attachment.source !== 'inline' && !(attachment.dataUrl?.trim()) && attachment.path.trim().length > 0
}

function filePathToFileUrl(path: string): string {
  const normalized = path.replace(/\\/g, '/')
  const withLeadingSlash = /^[a-zA-Z]:/.test(normalized) ? `/${normalized}` : normalized
  return encodeURI(`file://${withLeadingSlash}`)
}

export function getAttachmentPreviewSrc(path: string): string {
  const trimmed = path.trim()
  if (!trimmed) return ''
  if (/^(?:https?:|data:|asset:|file:\/\/)/i.test(trimmed)) {
    return trimmed
  }

  try {
    return convertFileSrc(trimmed)
  } catch {
    return filePathToFileUrl(trimmed)
  }
}

export function getAttachmentPreviewSrcForAttachment(attachment: ChatAttachment): string {
  const inlineDataUrl = attachment.dataUrl?.trim()
  if (inlineDataUrl) {
    return inlineDataUrl
  }

  return getAttachmentPreviewSrc(attachment.path)
}

export function formatAttachmentContext(attachments: ChatAttachment[]): string {
  if (attachments.length === 0) return ''

  const lines = attachments.map((item, index) => {
    const label = item.kind === 'folder' ? 'Ordner' : 'Datei'
    return `${index + 1}. ${label}: ${item.path}`
  })

  return [`Verbundene Pfade (${attachments.length}):`, ...lines].join('\n')
}

const ATTACHMENT_CONTEXT_BLOCK_REGEX = /(?:^|\n)Verbundene Pfade \(\d+\):\n((?:\d+\.\s+(?:Datei|Ordner):[^\n]*(?:\n|$))+)/i

const GENERATED_ATTACHMENT_SECTION_REGEX = /(?:^|\n\n)(?:Datei-Metadaten \(ohne Volltext\):|Retrieval-Kontext \(selektiv gelesen\):|Nicht analysierbare Anhaenge:)/i

function stripGeneratedAttachmentSections(content: string): string {
  const trimmed = content.trim()
  const markerIndex = trimmed.search(GENERATED_ATTACHMENT_SECTION_REGEX)
  if (markerIndex === -1) {
    return trimmed
  }

  return trimmed.slice(0, markerIndex).trim()
}

export function extractAttachmentsFromContent(content: string): {
  content: string
  attachments: ChatAttachment[]
} {
  if (!content.trim()) {
    return { content: '', attachments: [] }
  }

  const match = ATTACHMENT_CONTEXT_BLOCK_REGEX.exec(content)
  if (!match) {
    return { content: stripGeneratedAttachmentSections(content), attachments: [] }
  }

  const attachments = match[1]
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => {
      const entry = line.match(/^\d+\.\s+(Datei|Ordner):\s*(.+)$/i)
      if (!entry) return null
      return {
        kind: entry[1].toLowerCase() === 'ordner' ? 'folder' : 'file',
        path: entry[2].trim(),
      } satisfies ChatAttachment
    })
    .filter((item): item is ChatAttachment => item !== null && item.path.length > 0)

  const cleanedContent = content
    .replace(match[0], '')
    .replace(/\n{3,}/g, '\n\n')
    .trim()

  return {
    content: stripGeneratedAttachmentSections(cleanedContent),
    attachments,
  }
}

function fileUriToWindowsPath(value: string): string {
  const trimmed = value.trim()
  if (!trimmed.toLowerCase().startsWith('file://')) return ''

  let decoded = trimmed.replace(/^file:\/\//i, '')
  try {
    decoded = decodeURIComponent(decoded)
  } catch {
    // Malformed URI segments (e.g. lone %) must not crash drop handling.
    return ''
  }

  if (/^[a-zA-Z]:/.test(decoded)) {
    return decoded.replace(/\//g, '\\')
  }

  if (decoded.startsWith('/')) {
    const maybeDrive = decoded.slice(1)
    if (/^[a-zA-Z]:/.test(maybeDrive)) {
      return maybeDrive.replace(/\//g, '\\')
    }
  }

  return decoded.replace(/\//g, '\\')
}

export function extractFileAttachmentsFromFileList(fileList: FileList | null): ChatAttachment[] {
  if (!fileList || fileList.length === 0) return []

  const paths: ChatAttachment[] = []
  for (const file of Array.from(fileList)) {
    const filePath = (file as FileWithPath).path
    if (typeof filePath === 'string' && filePath.trim().length > 0) {
      paths.push({ path: filePath, kind: 'file' })
    }
  }

  return paths
}

export function extractFileAttachmentsFromUriList(uriList: string): ChatAttachment[] {
  return uriList
    .split(/\r?\n/)
    .filter((line) => !line.trim().startsWith('#'))
    .map((line) => fileUriToWindowsPath(line))
    .filter((path) => path.length > 0)
    .map((path) => ({ path, kind: 'file' as const }))
}

function createAttachmentId(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID()
  }

  return `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
}

function guessImageExtension(mediaType: string | undefined): string {
  switch ((mediaType ?? '').toLowerCase()) {
    case 'image/jpeg':
      return 'jpg'
    case 'image/webp':
      return 'webp'
    case 'image/gif':
      return 'gif'
    case 'image/svg+xml':
      return 'svg'
    case 'image/bmp':
      return 'bmp'
    case 'image/avif':
      return 'avif'
    default:
      return 'png'
  }
}

function blobToDataUrl(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => {
      if (typeof reader.result === 'string') {
        resolve(reader.result)
        return
      }

      reject(new Error('Datei konnte nicht als Data-URL gelesen werden.'))
    }
    reader.onerror = () => reject(reader.error ?? new Error('Datei konnte nicht gelesen werden.'))
    reader.readAsDataURL(blob)
  })
}

function parseBase64DataUrl(dataUrl: string): { mediaType: string; data: string } | null {
  const match = dataUrl.trim().match(/^data:([^;]+);base64,(.+)$/s)
  if (!match) return null

  return {
    mediaType: match[1],
    data: match[2],
  }
}

async function readAttachmentDataUrl(attachment: ChatAttachment): Promise<string | null> {
  const inlineDataUrl = attachment.dataUrl?.trim()
  if (inlineDataUrl) {
    return inlineDataUrl
  }

  const previewSrc = getAttachmentPreviewSrcForAttachment(attachment)
  if (!previewSrc) return null

  const response = await fetch(previewSrc)
  if (!response.ok) {
    throw new Error(`Bild konnte nicht geladen werden (${response.status}).`)
  }

  const blob = await response.blob()
  return await blobToDataUrl(blob)
}

export async function createInlineImageAttachment(file: File): Promise<ChatAttachment> {
  const mediaType = file.type || 'image/png'
  const extension = guessImageExtension(mediaType)
  const dataUrl = await blobToDataUrl(file)
  const fileName = file.name?.trim() || `clipboard-image-${createAttachmentId()}.${extension}`

  return {
    path: fileName,
    kind: 'file',
    label: fileName,
    mediaType,
    dataUrl,
    source: 'inline',
  }
}

export async function toImageContentBlock(attachment: ChatAttachment): Promise<ContentBlockImage | null> {
  if (!isImageAttachment(attachment)) return null

  const dataUrl = await readAttachmentDataUrl(attachment)
  if (!dataUrl) return null

  const parsed = parseBase64DataUrl(dataUrl)
  if (!parsed) return null

  return {
    type: 'image',
    source: {
      type: 'base64',
      media_type: parsed.mediaType,
      data: parsed.data,
    },
  }
}

export async function toImageContentBlocks(attachments: ChatAttachment[]): Promise<ContentBlockImage[]> {
  const results = await Promise.allSettled(
    attachments
      .filter((attachment) => attachment.kind === 'file' && isImageAttachment(attachment))
      .map((attachment) => toImageContentBlock(attachment)),
  )

  return results.flatMap((result) => {
    if (result.status !== 'fulfilled' || !result.value) {
      return []
    }

    return [result.value]
  })
}
