import { beforeEach, describe, expect, it, vi } from 'vitest'

const safeInvokeMock = vi.fn()

vi.mock('../utils/safeInvoke', () => ({
  safeInvoke: (...args: unknown[]) => safeInvokeMock(...args),
}))

describe('documentWorkspaceStore', () => {
  beforeEach(async () => {
    safeInvokeMock.mockReset()
    const { useDocumentWorkspaceStore } = await import('./documentWorkspaceStore')
    useDocumentWorkspaceStore.setState({
      documents: [],
      activePath: null,
      officeApps: [],
      officeWarnings: [],
      busy: false,
      error: null,
    })
  })

  it('tracks only supported document paths', async () => {
    const { useDocumentWorkspaceStore } = await import('./documentWorkspaceStore')
    const store = useDocumentWorkspaceStore.getState()

    store.upsertDocumentFromPath('C:\\work\\deck.pptx')
    store.upsertDocumentFromPath('C:\\work\\screenshot.png')

    const state = useDocumentWorkspaceStore.getState()
    expect(state.documents).toHaveLength(1)
    expect(state.documents[0]).toMatchObject({
      path: 'C:\\work\\deck.pptx',
      format: 'pptx',
      status: 'idle',
    })
    expect(state.activePath).toBe('C:\\work\\deck.pptx')
  })

  it('detects Office apps through the Tauri command', async () => {
    safeInvokeMock.mockResolvedValueOnce({
      apps: [
        {
          kind: 'word',
          displayName: 'Microsoft Word',
          executablePath: 'C:\\Office\\WINWORD.EXE',
          available: true,
        },
      ],
      warnings: [],
    })

    const { useDocumentWorkspaceStore } = await import('./documentWorkspaceStore')
    await useDocumentWorkspaceStore.getState().detectOfficeApps()

    expect(safeInvokeMock).toHaveBeenCalledWith(
      'office_detect_apps',
      {},
      { apps: [], warnings: ['Tauri runtime is not available.'] },
    )
    expect(useDocumentWorkspaceStore.getState().officeApps[0]).toMatchObject({
      kind: 'word',
      available: true,
    })
  })

  it('renders and stores document previews through document_render_preview', async () => {
    safeInvokeMock.mockResolvedValueOnce({
      sourcePath: 'C:\\work\\report.docx',
      format: 'docx',
      previewDir: 'C:\\cache\\report',
      exportedPdfPath: 'C:\\cache\\report\\office-export.pdf',
      officeApp: 'word',
      pages: [
        {
          pageNumber: 1,
          imagePath: 'C:\\cache\\report\\pages\\page-1.png',
          width: 1200,
          height: 1600,
        },
      ],
      warnings: [],
      generatedAt: '2026-06-03T12:00:00Z',
    })

    const { useDocumentWorkspaceStore } = await import('./documentWorkspaceStore')
    const preview = await useDocumentWorkspaceStore.getState().renderPreview('C:\\work\\report.docx')

    expect(safeInvokeMock).toHaveBeenCalledWith('document_render_preview', {
      request: {
        path: 'C:\\work\\report.docx',
        maxPages: 8,
        targetWidth: 1200,
      },
    })
    expect(preview.pages).toHaveLength(1)
    expect(useDocumentWorkspaceStore.getState().documents[0]).toMatchObject({
      path: 'C:\\work\\report.docx',
      format: 'docx',
      status: 'idle',
      lastAction: 'Preview aktualisiert',
    })
  })

  it('opens documents through office_open_document and saves versions', async () => {
    safeInvokeMock
      .mockResolvedValueOnce({ launched: true })
      .mockResolvedValueOnce({ id: 'version-1' })

    const { useDocumentWorkspaceStore } = await import('./documentWorkspaceStore')
    const store = useDocumentWorkspaceStore.getState()

    store.upsertDocumentFromPath('C:\\work\\budget.xlsx')
    await store.openDocument('C:\\work\\budget.xlsx')
    await store.saveVersion('C:\\work\\budget.xlsx')

    expect(safeInvokeMock).toHaveBeenNthCalledWith(1, 'office_open_document', {
      request: { path: 'C:\\work\\budget.xlsx' },
    })
    expect(safeInvokeMock).toHaveBeenNthCalledWith(2, 'fs_save_artifact_version', {
      path: 'C:\\work\\budget.xlsx',
      label: 'budget.xlsx',
    })
    expect(useDocumentWorkspaceStore.getState().documents[0]).toMatchObject({
      artifactVersionId: 'version-1',
      lastAction: 'Version saved',
    })
  })
})
