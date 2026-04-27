import { beforeEach, describe, expect, it, vi } from 'vitest'

const invokeMock = vi.fn()

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}))

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async () => () => {}),
}))

describe('filesystem tool extensions', () => {
  beforeEach(() => {
    invokeMock.mockReset()
  })

  it('registers CreateDirectory with aliases for Ollama fallback parsing', async () => {
    const { registerAllBuiltinTools, getAllTools, getToolDefinitions } = await import('./registry')
    registerAllBuiltinTools()

    const tool = getAllTools().find((entry) => entry.name === 'CreateDirectory')
    expect(tool).toBeTruthy()
    expect(tool?.aliases).toContain('create_directory')
    expect(tool?.aliases).toContain('mkdir')

    const defs = getToolDefinitions()
    const createDirDef = defs.find((entry) => entry.name === 'CreateDirectory')
    expect(createDirDef?.aliases).toContain('create_directory')
    expect(createDirDef?.aliases).toContain('mkdir')
  })

  it('calls the backend create directory command', async () => {
    const { registerAllBuiltinTools, getAllTools } = await import('./registry')
    registerAllBuiltinTools()

    invokeMock.mockResolvedValue({
      path: 'C:\\workspace\\sorted',
      created: true,
    })

    const tool = getAllTools().find((entry) => entry.name === 'CreateDirectory')
    expect(tool).toBeTruthy()

    const result = await tool!.call(
      { path: 'C:\\workspace\\sorted' },
      { cwd: 'C:\\workspace', runId: 'run-create' } as never,
    )

    expect(result.data).toBe('Verzeichnis erstellt: C:\\workspace\\sorted')
    expect(invokeMock).toHaveBeenCalledWith('fs_create_directory', {
      path: 'C:\\workspace\\sorted',
      runId: 'run-create',
    })
  })

  it('calls the backend move path command', async () => {
    const { registerAllBuiltinTools, getAllTools } = await import('./registry')
    registerAllBuiltinTools()

    invokeMock.mockResolvedValue({
      sourcePath: 'C:\\workspace\\Downloads\\file.txt',
      destinationPath: 'C:\\workspace\\Archiv\\file.txt',
      itemKind: 'file',
      createdParent: true,
      replacedExisting: false,
    })

    const tool = getAllTools().find((entry) => entry.name === 'MovePath')
    expect(tool).toBeTruthy()

    const result = await tool!.call(
      {
        source_path: 'C:\\workspace\\Downloads\\file.txt',
        destination_path: 'C:\\workspace\\Archiv\\file.txt',
        overwrite: false,
      },
      { cwd: 'C:\\workspace', runId: 'run-move' } as never,
    )

    expect(result.data).toContain('file verschoben: C:\\workspace\\Downloads\\file.txt -> C:\\workspace\\Archiv\\file.txt')
    expect(result.data).toContain('Zielordner wurde automatisch erstellt.')
    expect(invokeMock).toHaveBeenCalledWith('fs_move_path', {
      sourcePath: 'C:\\workspace\\Downloads\\file.txt',
      destinationPath: 'C:\\workspace\\Archiv\\file.txt',
      overwrite: false,
      runId: 'run-move',
    })
  })

  it('calls the backend copy path command', async () => {
    const { registerAllBuiltinTools, getAllTools } = await import('./registry')
    registerAllBuiltinTools()

    invokeMock.mockResolvedValue({
      sourcePath: 'C:\\workspace\\src',
      destinationPath: 'C:\\workspace\\backup\\src',
      itemKind: 'directory',
      createdParent: false,
      replacedExisting: true,
    })

    const tool = getAllTools().find((entry) => entry.name === 'CopyPath')
    expect(tool).toBeTruthy()

    const result = await tool!.call(
      {
        source_path: 'C:\\workspace\\src',
        destination_path: 'C:\\workspace\\backup\\src',
        overwrite: true,
      },
      { cwd: 'C:\\workspace', runId: 'run-copy' } as never,
    )

    expect(result.data).toContain('directory kopiert: C:\\workspace\\src -> C:\\workspace\\backup\\src')
    expect(result.data).toContain('Bestehendes Ziel wurde ersetzt.')
    expect(invokeMock).toHaveBeenCalledWith('fs_copy_path', {
      sourcePath: 'C:\\workspace\\src',
      destinationPath: 'C:\\workspace\\backup\\src',
      overwrite: true,
      runId: 'run-copy',
    })
  })

  it('updates the shell cwd when bash reports a new working directory', async () => {
    const { registerAllBuiltinTools, getAllTools } = await import('./registry')
    registerAllBuiltinTools()

    invokeMock.mockResolvedValue({
      stdout: '',
      stderr: '',
      exitCode: 0,
      currentCwd: 'C:\\workspace\\nested',
    })

    const tool = getAllTools().find((entry) => entry.name === 'Bash')
    expect(tool).toBeTruthy()

    let appState = { cwd: 'C:\\workspace' }

    const result = await tool!.call(
      { command: 'Set-Location nested' },
      {
        cwd: 'C:\\workspace',
        runId: 'run-bash',
        setAppState: (updater: (prev: typeof appState) => typeof appState) => {
          appState = updater(appState)
        },
      } as never,
    )

    expect(result.data).toContain('current cwd: C:\\workspace\\nested')
    expect(appState.cwd).toBe('C:\\workspace\\nested')
    expect(invokeMock).toHaveBeenCalledWith('exec_command', {
      command: 'Set-Location nested',
      cwd: 'C:\\workspace',
      timeoutMs: 30000,
      streamId: expect.any(String),
      runId: 'run-bash',
    })
  })

  it('infers the shell cwd for simple cd commands when the backend reports no cwd', async () => {
    const { registerAllBuiltinTools, getAllTools } = await import('./registry')
    registerAllBuiltinTools()

    invokeMock.mockResolvedValue({
      stdout: '',
      stderr: '',
      exitCode: 0,
    })

    const tool = getAllTools().find((entry) => entry.name === 'Bash')
    expect(tool).toBeTruthy()

    let appState = { cwd: 'C:\\workspace\\OrdnerA' }

    const result = await tool!.call(
      { command: 'cd ..; pwd' },
      {
        cwd: 'C:\\workspace\\OrdnerA',
        runId: 'run-bash-fallback',
        setAppState: (updater: (prev: typeof appState) => typeof appState) => {
          appState = updater(appState)
        },
      } as never,
    )

    expect(result.data).toContain('current cwd: C:\\workspace')
    expect(appState.cwd).toBe('C:\\workspace')
  })

  it('mirrors inferred cwd into stdout for pwd-style commands without stdout', async () => {
    const { registerAllBuiltinTools, getAllTools } = await import('./registry')
    registerAllBuiltinTools()

    invokeMock.mockResolvedValue({
      stdout: '',
      stderr: '',
      exitCode: 0,
    })

    const tool = getAllTools().find((entry) => entry.name === 'Bash')
    expect(tool).toBeTruthy()

    const result = await tool!.call(
      { command: 'cd ..; pwd' },
      {
        cwd: 'C:\\workspace\\OrdnerA',
        runId: 'run-bash-pwd',
        setAppState: () => undefined,
      } as never,
    )

    expect(result.data).toContain('stdout:\nC:\\workspace')
    expect(result.data).toContain('current cwd: C:\\workspace')
  })

  it('captures desktop screenshots and injects them as image attachments', async () => {
    const { registerAllBuiltinTools, getAllTools, getToolDefinitions } = await import('./registry')
    registerAllBuiltinTools()

    invokeMock.mockResolvedValue({
      dataUrl: 'data:image/png;base64,AAA',
      width: 1280,
      height: 720,
      x: 0,
      y: 0,
      primary: true,
      deviceName: '\\\\.\\DISPLAY1',
      imageWidth: 1280,
      imageHeight: 720,
      coordinateOverlay: true,
    })

    const tool = getAllTools().find((entry) => entry.name === 'DesktopScreenshot')
    expect(tool).toBeTruthy()

    const result = await tool!.call(
      {},
      { cwd: 'C:\\workspace', runId: 'run-desktop-screenshot' } as never,
    )

    expect(result.data).toContain('Desktop screenshot aufgenommen: Bild 1280x720')
    expect(result.data).toContain('Koordinatenraster')
    expect(result.data).toContain('Attachment fuer visuelle Analyse')
    expect(invokeMock).toHaveBeenCalledWith('desktop_capture_primary_annotated_screenshot')
    expect(result.newMessages).toHaveLength(1)
    expect(result.newMessages?.[0]).toEqual(expect.objectContaining({
      type: 'attachment',
      attachmentType: 'tool_result',
      content: [
        expect.objectContaining({ type: 'text' }),
        expect.objectContaining({
          type: 'image',
          source: expect.objectContaining({
            media_type: 'image/png',
            data: 'AAA',
          }),
        }),
      ],
    }))

    const defs = getToolDefinitions()
    expect(defs.find((entry) => entry.name === 'DesktopScreenshot')).toBeTruthy()
  })

  it('translates display-relative desktop click coordinates using the primary display origin', async () => {
    const { registerAllBuiltinTools, getAllTools } = await import('./registry')
    registerAllBuiltinTools()

    invokeMock.mockImplementation((command: string, payload?: Record<string, unknown>) => {
      if (command === 'desktop_primary_display') {
        return Promise.resolve({
          primary: true,
          x: 1920,
          y: 0,
          width: 1280,
          height: 720,
          deviceName: '\\\\.\\DISPLAY1',
        })
      }

      if (command === 'desktop_click') {
        expect(payload).toEqual({
          request: {
            x: 2500,
            y: 180,
            button: 'left',
            doubleClick: false,
          },
        })

        return Promise.resolve({
          ok: true,
          action: 'click',
        })
      }

      if (command === 'desktop_capture_primary_annotated_screenshot') {
        return Promise.resolve({
          dataUrl: 'data:image/png;base64,CCC',
          width: 1280,
          height: 720,
          x: 1920,
          y: 0,
          primary: true,
          deviceName: '\\\\.\\DISPLAY1',
          imageWidth: 1280,
          imageHeight: 720,
          coordinateOverlay: true,
        })
      }

      throw new Error(`unexpected invoke command ${command}`)
    })

    const tool = getAllTools().find((entry) => entry.name === 'DesktopClick')
    expect(tool).toBeTruthy()

    const result = await tool!.call(
      { x: 580, y: 180 },
      { cwd: 'C:\\workspace', runId: 'run-desktop-click' } as never,
    )

    expect(result.data).toContain('Display-Koordinaten (580, 180) wurden mit Display-Ursprung (1920, 0) zu Bildschirmkoordinaten (2500, 180) umgerechnet')
    expect(result.newMessages).toHaveLength(1)
  })

  it('calls the backend desktop keypress command', async () => {
    const { registerAllBuiltinTools, getAllTools } = await import('./registry')
    registerAllBuiltinTools()

    invokeMock.mockImplementation((command: string) => {
      if (command === 'desktop_keypress') {
        return Promise.resolve({
          ok: true,
          action: 'keypress',
        })
      }

      if (command === 'desktop_capture_primary_annotated_screenshot') {
        return Promise.resolve({
          dataUrl: 'data:image/png;base64,BBB',
          width: 1280,
          height: 720,
          x: 0,
          y: 0,
          primary: true,
          deviceName: '\\\\.\\DISPLAY1',
          imageWidth: 1280,
          imageHeight: 720,
          coordinateOverlay: true,
        })
      }

      throw new Error(`unexpected invoke command ${command}`)
    })

    const tool = getAllTools().find((entry) => entry.name === 'DesktopKeypress')
    expect(tool).toBeTruthy()

    const result = await tool!.call(
      { keys: ['CTRL', 'L'] },
      { cwd: 'C:\\workspace', runId: 'run-desktop-keypress' } as never,
    )

    expect(result.data).toContain('Tastendruck wurde gesendet: CTRL + L')
    expect(result.data).toContain('Verifikations-Screenshot wurde angehaengt')
    expect(invokeMock).toHaveBeenCalledWith('desktop_keypress', {
      request: {
        keys: ['CTRL', 'L'],
      },
    })
    expect(invokeMock).toHaveBeenCalledWith('desktop_capture_primary_annotated_screenshot')
    expect(result.newMessages).toHaveLength(1)
  })

  it('calls the backend delete file command with confirmation', async () => {
    const { registerAllBuiltinTools, getAllTools } = await import('./registry')
    registerAllBuiltinTools()

    invokeMock.mockResolvedValue(undefined)

    const tool = getAllTools().find((entry) => entry.name === 'DeleteFile')
    expect(tool).toBeTruthy()
    expect(tool?.aliases).toContain('delete_file')
    expect(tool?.aliases).toContain('rm')
    expect(tool?.isDestructive?.({ file_path: '', confirm: true })).toBe(true)

    const result = await tool!.call(
      { file_path: 'C:\\workspace\\temp.txt', confirm: true },
      { cwd: 'C:\\workspace', runId: 'run-delete' } as never,
    )

    expect(result.data).toContain('Datei geloescht: C:\\workspace\\temp.txt')
    expect(invokeMock).toHaveBeenCalledWith('fs_delete_file', {
      path: 'C:\\workspace\\temp.txt',
      confirmToken: 'DELETE',
    })
  })

  it('rejects delete without confirm flag', async () => {
    const { registerAllBuiltinTools, getAllTools } = await import('./registry')
    registerAllBuiltinTools()

    const tool = getAllTools().find((entry) => entry.name === 'DeleteFile')
    expect(tool).toBeTruthy()

    const result = await tool!.call(
      { file_path: 'C:\\workspace\\temp.txt', confirm: false },
      { cwd: 'C:\\workspace', runId: 'run-delete' } as never,
    )

    expect(result.data).toContain('confirm muss auf true gesetzt werden')
    expect(invokeMock).not.toHaveBeenCalled()
  })

  it('returns file metadata via FileInfo tool', async () => {
    const { registerAllBuiltinTools, getAllTools } = await import('./registry')
    registerAllBuiltinTools()

    invokeMock.mockResolvedValue({
      rootPath: 'C:\\workspace\\config.json',
      rootKind: 'file',
      totalFiles: 1,
      returnedFiles: 1,
      truncated: false,
      files: [{
        path: 'C:\\workspace\\config.json',
        fileName: 'config.json',
        extension: 'json',
        language: 'JSON',
        sizeBytes: 2048,
      }],
    })

    const tool = getAllTools().find((entry) => entry.name === 'FileInfo')
    expect(tool).toBeTruthy()
    expect(tool?.aliases).toContain('stat')

    const result = await tool!.call(
      { path: 'C:\\workspace\\config.json' },
      { cwd: 'C:\\workspace', runId: 'run-info' } as never,
    )

    expect(result.data).toContain('Datei: config.json')
    expect(result.data).toContain('Extension: .json')
    expect(result.data).toContain('Sprache: JSON')
    expect(invokeMock).toHaveBeenCalledWith('fs_collect_attachment_metadata', {
      path: 'C:\\workspace\\config.json',
      maxEntries: 1,
      runId: 'run-info',
    })
  })

  it('renames a file via RenameFile tool', async () => {
    const { registerAllBuiltinTools, getAllTools } = await import('./registry')
    registerAllBuiltinTools()

    invokeMock.mockResolvedValue({
      sourcePath: 'C:\\workspace\\old.txt',
      destinationPath: 'C:\\workspace\\new.txt',
      itemKind: 'file',
      createdParent: false,
      replacedExisting: false,
    })

    const tool = getAllTools().find((entry) => entry.name === 'RenameFile')
    expect(tool).toBeTruthy()
    expect(tool?.aliases).toContain('rename')

    const result = await tool!.call(
      { path: 'C:\\workspace\\old.txt', new_name: 'new.txt' },
      { cwd: 'C:\\workspace', runId: 'run-rename' } as never,
    )

    expect(result.data).toContain('file umbenannt: C:\\workspace\\old.txt -> new.txt')
    expect(invokeMock).toHaveBeenCalledWith('fs_move_path', expect.objectContaining({
      sourcePath: 'C:\\workspace\\old.txt',
      overwrite: false,
      runId: 'run-rename',
    }))
  })

  it('ListDir uses native fs_collect_attachment_metadata IPC', async () => {
    const { registerAllBuiltinTools, getAllTools } = await import('./registry')
    registerAllBuiltinTools()

    invokeMock.mockResolvedValue({
      rootPath: 'C:\\workspace',
      rootKind: 'folder',
      totalFiles: 3,
      returnedFiles: 3,
      truncated: false,
      files: [
        { path: 'C:\\workspace\\index.ts', fileName: 'index.ts', extension: 'ts', language: 'TypeScript', sizeBytes: 1024 },
        { path: 'C:\\workspace\\README.md', fileName: 'README.md', extension: 'md', language: 'Markdown', sizeBytes: 512 },
        { path: 'C:\\workspace\\config.json', fileName: 'config.json', extension: 'json', language: 'JSON', sizeBytes: 256 },
      ],
    })

    const tool = getAllTools().find((entry) => entry.name === 'ListDir')
    expect(tool).toBeTruthy()

    const result = await tool!.call(
      { path: 'C:\\workspace' },
      { cwd: 'C:\\workspace', runId: 'run-ls' } as never,
    )

    expect(result.data).toContain('Verzeichnis: C:\\workspace')
    expect(result.data).toContain('3 Dateien')
    expect(result.data).toContain('index.ts')
    expect(result.data).toContain('TypeScript')
    // Should NOT call exec_command (PowerShell) — must use native IPC
    expect(invokeMock).toHaveBeenCalledWith('fs_collect_attachment_metadata', expect.objectContaining({
      path: 'C:\\workspace',
      runId: 'run-ls',
    }))
    expect(invokeMock).not.toHaveBeenCalledWith('exec_command', expect.anything())
  })

  it('registers all expected filesystem tools', async () => {
    const { registerAllBuiltinTools, getAllTools } = await import('./registry')
    registerAllBuiltinTools()

    const allTools = getAllTools()
    const expectedNames = [
      'Read', 'Write', 'Edit', 'Append', 'MultiEdit',
      'ListDir', 'Glob', 'Grep',
      'CreateDirectory', 'MovePath', 'CopyPath', 'DeleteFile',
      'FileInfo', 'RenameFile',
    ]

    for (const name of expectedNames) {
      const found = allTools.find((t) => t.name === name)
      expect(found, `Tool "${name}" should be registered`).toBeTruthy()
    }
  })
})
