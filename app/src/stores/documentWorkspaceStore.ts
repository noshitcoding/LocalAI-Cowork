import { create } from 'zustand'
import { getPathName } from '../utils/chatAttachments'
import { safeInvoke } from '../utils/safeInvoke'

export type OfficeAppInfo = {
  kind: string
  displayName: string
  executablePath: string | null
  available: boolean
}

export type DocumentPreviewPage = {
  pageNumber: number
  imagePath: string
  width: number
  height: number
}

export type DocumentPreviewResponse = {
  sourcePath: string
  format: string
  previewDir: string
  exportedPdfPath: string | null
  officeApp: string | null
  pages: DocumentPreviewPage[]
  warnings: string[]
  generatedAt: string
}

export type DocumentWorkspaceItem = {
  path: string
  label: string
  format: string
  status: 'idle' | 'opening' | 'rendering' | 'saving_version' | 'error'
  preview: DocumentPreviewResponse | null
  error: string | null
  lastAction: string | null
  artifactVersionId: string | null
  updatedAt: number
}

type ArtifactVersionResponse = {
  id: string
}

type OfficeDetectResponse = {
  apps: OfficeAppInfo[]
  warnings: string[]
}

type DocumentWorkspaceState = {
  documents: DocumentWorkspaceItem[]
  activePath: string | null
  officeApps: OfficeAppInfo[]
  officeWarnings: string[]
  busy: boolean
  error: string | null
  detectOfficeApps: () => Promise<void>
  upsertDocumentFromPath: (path: string, format?: string) => void
  upsertPreview: (preview: DocumentPreviewResponse) => void
  setDocumentError: (path: string, error: string) => void
  setActiveDocument: (path: string | null) => void
  renderPreview: (path: string) => Promise<DocumentPreviewResponse>
  openDocument: (path: string) => Promise<void>
  saveVersion: (path: string) => Promise<void>
}

const SUPPORTED_DOCUMENT_REGEX = /\.(pdf|docx|pptx|xlsx)$/i

function inferFormat(path: string): string {
  const basePath = path.split(/[#]/, 1)[0] ?? ''
  const ext = basePath.split('.').pop()?.toLowerCase() ?? ''
  if (ext === 'doc' || ext === 'docx') return 'docx'
  if (ext === 'ppt' || ext === 'pptx') return 'pptx'
  if (ext === 'xls' || ext === 'xlsx') return 'xlsx'
  if (ext === 'pdf') return 'pdf'
  return ext || 'unknown'
}

function isSupportedDocumentPath(path: string): boolean {
  return SUPPORTED_DOCUMENT_REGEX.test(path.trim())
}

function normalizePath(path: string): string {
  return path.trim()
}

function createDocument(path: string, format?: string): DocumentWorkspaceItem {
  const normalized = normalizePath(path)
  return {
    path: normalized,
    label: getPathName(normalized),
    format: format ?? inferFormat(normalized),
    status: 'idle',
    preview: null,
    error: null,
    lastAction: null,
    artifactVersionId: null,
    updatedAt: Date.now(),
  }
}

function upsertItem(
  documents: DocumentWorkspaceItem[],
  path: string,
  update: Partial<DocumentWorkspaceItem>,
): DocumentWorkspaceItem[] {
  const normalized = normalizePath(path)
  const index = documents.findIndex((item) => item.path.toLowerCase() === normalized.toLowerCase())
  if (index < 0) {
    return [{ ...createDocument(normalized), ...update, updatedAt: Date.now() }, ...documents]
  }

  return documents.map((item, itemIndex) => (
    itemIndex === index ?        { ...item, ...update, updatedAt: Date.now() }
      : item
  ))
}

export const useDocumentWorkspaceStore = create<DocumentWorkspaceState>((set, get) => ({
  documents: [],
  activePath: null,
  officeApps: [],
  officeWarnings: [],
  busy: false,
  error: null,

  detectOfficeApps: async () => {
    const response = await safeInvoke<OfficeDetectResponse>(
      'office_detect_apps',
      {},
      { apps: [], warnings: ['Tauri runtime is not available.'] },
    )
    set({
      officeApps: Array.isArray(response.apps) ? response.apps : [],
      officeWarnings: Array.isArray(response.warnings) ? response.warnings : [],
    })
  },

  upsertDocumentFromPath: (path, format) => {
    const normalized = normalizePath(path)
    if (!normalized || !isSupportedDocumentPath(normalized)) return
    set((state) => ({
      activePath: normalized,
      documents: upsertItem(state.documents, normalized, {
        format: format ?? inferFormat(normalized),
        error: null,
      }),
    }))
  },

  upsertPreview: (preview) => {
    const path = normalizePath(preview.sourcePath)
    if (!path) return
    set((state) => ({
      activePath: path,
      documents: upsertItem(state.documents, path, {
        format: preview.format,
        preview,
        status: 'idle',
        error: null,
        lastAction: 'Preview aktualisiert',
      }),
    }))
  },

  setDocumentError: (path, error) => {
    const normalized = normalizePath(path)
    if (!normalized) return
    set((state) => ({
      activePath: normalized,
      documents: upsertItem(state.documents, normalized, {
        status: 'error',
        error,
        lastAction: 'Error',
      }),
    }))
  },

  setActiveDocument: (path) => set({ activePath: path ? normalizePath(path) : null }),

  renderPreview: async (path) => {
    const normalized = normalizePath(path)
    if (!normalized) throw new Error('Document path is required.')
    set((state) => ({
      busy: true,
      error: null,
      activePath: normalized,
      documents: upsertItem(state.documents, normalized, {
        status: 'rendering',
        error: null,
        lastAction: 'Preview is created',
      }),
    }))

    try {
      const preview = await safeInvoke<DocumentPreviewResponse>('document_render_preview', {
        request: {
          path: normalized,
          maxPages: 8,
          targetWidth: 1200,
        },
      })
      get().upsertPreview(preview)
      set({ busy: false, error: null })
      return preview
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      get().setDocumentError(normalized, message)
      set({ busy: false, error: message })
      throw error
    }
  },

  openDocument: async (path) => {
    const normalized = normalizePath(path)
    if (!normalized) return
    set((state) => ({
      activePath: normalized,
      documents: upsertItem(state.documents, normalized, {
        status: 'opening',
        error: null,
        lastAction: 'Wird opened',
      }),
    }))

    try {
      await safeInvoke('office_open_document', {
        request: { path: normalized },
      })
      set((state) => ({
        documents: upsertItem(state.documents, normalized, {
          status: 'idle',
          error: null,
          lastAction: 'In Office opened',
        }),
      }))
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      get().setDocumentError(normalized, message)
    }
  },

  saveVersion: async (path) => {
    const normalized = normalizePath(path)
    if (!normalized) return
    set((state) => ({
      activePath: normalized,
      documents: upsertItem(state.documents, normalized, {
        status: 'saving_version',
        error: null,
        lastAction: 'Version saved',
      }),
    }))

    try {
      const version = await safeInvoke<ArtifactVersionResponse>('fs_save_artifact_version', {
        path: normalized,
        label: getPathName(normalized),
      })
      set((state) => ({
        documents: upsertItem(state.documents, normalized, {
          status: 'idle',
          artifactVersionId: version.id,
          error: null,
          lastAction: 'Version saved',
        }),
      }))
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      get().setDocumentError(normalized, message)
    }
  },
}))

export function isDocumentWorkspacePath(path: string): boolean {
  return isSupportedDocumentPath(path)
}
