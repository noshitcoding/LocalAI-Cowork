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
      previewOperations: {},
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

  it('keeps case-distinct POSIX paths separate while folding Windows and UNC paths', async () => {
    const { useDocumentWorkspaceStore } = await import('./documentWorkspaceStore')
    const store = useDocumentWorkspaceStore.getState()

    store.upsertDocumentFromPath('/work/Report.docx')
    store.upsertDocumentFromPath('/work/report.docx')
    store.upsertDocumentFromPath('C:\\Work\\Budget.xlsx')
    store.upsertDocumentFromPath('c:/work/budget.xlsx')
    store.upsertDocumentFromPath('\\\\Server\\Share\\Deck.pptx')
    store.upsertDocumentFromPath('//server/share/deck.pptx')

    const paths = useDocumentWorkspaceStore.getState().documents.map((item) => item.path)
    expect(paths).toContain('/work/Report.docx')
    expect(paths).toContain('/work/report.docx')
    expect(paths.filter((path) => path.toLowerCase().includes('budget.xlsx'))).toHaveLength(1)
    expect(paths.filter((path) => path.toLowerCase().includes('deck.pptx'))).toHaveLength(1)
    expect(paths).toHaveLength(4)
  })

  it('does not reactivate a tab when another document preview finishes late', async () => {
    let resolvePreview!: (value: unknown) => void
    safeInvokeMock.mockReturnValueOnce(new Promise((resolve) => { resolvePreview = resolve }))
    const { useDocumentWorkspaceStore } = await import('./documentWorkspaceStore')
    const previewPromise = useDocumentWorkspaceStore.getState().renderPreview('/work/a.docx')
    useDocumentWorkspaceStore.getState().upsertDocumentFromPath('/work/b.docx')

    resolvePreview({
      sourcePath: '/work/a.docx', format: 'docx', previewDir: '/cache/a',
      exportedPdfPath: null, officeApp: 'word', pages: [], warnings: [],
      generatedAt: '2026-08-30T12:00:00Z',
    })
    await previewPromise

    expect(useDocumentWorkspaceStore.getState().activePath).toBe('/work/b.docx')
    expect(useDocumentWorkspaceStore.getState().documents.find((item) => item.path === '/work/a.docx'))
      .toMatchObject({ status: 'idle', preview: { previewDir: '/cache/a' } })
  })

  it('ignores an older preview result for the same document', async () => {
    let resolveFirst!: (value: unknown) => void
    let resolveSecond!: (value: unknown) => void
    safeInvokeMock
      .mockReturnValueOnce(new Promise((resolve) => { resolveFirst = resolve }))
      .mockReturnValueOnce(new Promise((resolve) => { resolveSecond = resolve }))
    const { useDocumentWorkspaceStore } = await import('./documentWorkspaceStore')
    const first = useDocumentWorkspaceStore.getState().renderPreview('/work/report.docx')
    const second = useDocumentWorkspaceStore.getState().renderPreview('/work/report.docx')

    resolveSecond({
      sourcePath: '/work/report.docx', format: 'docx', previewDir: '/cache/new',
      exportedPdfPath: null, officeApp: 'word', pages: [], warnings: [],
      generatedAt: '2026-08-30T12:01:00Z',
    })
    await second
    resolveFirst({
      sourcePath: '/work/report.docx', format: 'docx', previewDir: '/cache/old',
      exportedPdfPath: null, officeApp: 'word', pages: [], warnings: [],
      generatedAt: '2026-08-30T12:00:00Z',
    })
    await first

    expect(useDocumentWorkspaceStore.getState().documents[0].preview?.previewDir).toBe('/cache/new')
  })
})
