import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useTerminalStore } from '../../stores/terminalStore'

const invokeMock = vi.fn()
const runAiCommandMock = vi.fn()

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}))

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async () => () => {}),
}))

vi.mock('../../utils/safeInvoke', () => ({
  hasTauriRuntime: vi.fn(() => false),
  safeInvoke: vi.fn(async (_cmd: string, _args: unknown, fallback: unknown) => fallback),
  safeInvokeVoid: vi.fn(),
}))

describe('Bash terminal integration', () => {
  beforeEach(() => {
    invokeMock.mockReset()
    invokeMock.mockResolvedValue(undefined)
    runAiCommandMock.mockReset()
    useTerminalStore.setState({
      backends: [],
      loading: false,
      error: null,
      sessionsByThread: {},
      activeSessionIds: {},
      dockOpenByThread: {},
      dockHeightByThread: {},
      hiddenActivityByThread: {},
      activeAiThreadId: 'thread-1',
      runAiCommand: runAiCommandMock as never,
    })
  })

  it('routes Bash through the active terminal thread instead of exec_command', async () => {
    runAiCommandMock.mockResolvedValue({
      sessionId: 'term-ai-1',
      stdout: 'hello from terminal',
      stderr: '',
      exitCode: 0,
      currentCwd: 'C:\\workspace\\nested',
      interruptedByUser: true,
      timedOut: false,
    })

    const { registerAllBuiltinTools, getAllTools } = await import('./registry')
    registerAllBuiltinTools()
    const tool = getAllTools().find((entry) => entry.name === 'Bash')
    expect(tool).toBeTruthy()

    let appState = { cwd: 'C:\\workspace' }
    const onProgress = vi.fn()

    const result = await tool!.call(
      { command: 'Set-Location nested; Write-Output hello', timeout: 1234 },
      {
        cwd: 'C:\\workspace',
        runId: 'run-terminal-bash',
        setAppState: (updater: (prev: typeof appState) => typeof appState) => {
          appState = updater(appState)
        },
      } as never,
      onProgress,
    )

    expect(runAiCommandMock).toHaveBeenCalledWith({
      threadId: 'thread-1',
      command: 'Set-Location nested; Write-Output hello',
      cwd: 'C:\\workspace',
      timeoutMs: 1234,
    })
    expect(invokeMock).toHaveBeenCalledWith('shell_command_validate', {
      command: 'Set-Location nested; Write-Output hello',
      cwd: 'C:\\workspace',
      runId: 'run-terminal-bash',
    })
    expect(invokeMock).not.toHaveBeenCalledWith('exec_command', expect.anything())
    expect(appState.cwd).toBe('C:\\workspace\\nested')
    expect(result.data).toContain('stdout:\nhello from terminal')
    expect(result.data).toContain('note: user manually intervened')
    expect(result.data).toContain('current cwd: C:\\workspace\\nested')
    expect(result.data).toContain('exit code: 0')
    expect(onProgress).toHaveBeenCalledWith(expect.objectContaining({
      data: expect.objectContaining({ output: 'terminal: starting command' }),
    }))
    expect(onProgress).toHaveBeenCalledWith(expect.objectContaining({
      data: expect.objectContaining({ output: 'cwd: C:\\workspace\\nested' }),
    }))
  })

  it('does not write a command to the PTY when backend policy validation fails', async () => {
    invokeMock.mockRejectedValueOnce(new Error('command blocked: path traversal'))

    const { registerAllBuiltinTools, getAllTools } = await import('./registry')
    registerAllBuiltinTools()
    const tool = getAllTools().find((entry) => entry.name === 'Bash')

    const result = await tool!.call(
      { command: 'Get-Content ..\\secret.txt' },
      {
        cwd: 'C:\\workspace',
        runId: 'run-terminal-denied',
        setAppState: vi.fn(),
      } as never,
    )

    expect(runAiCommandMock).not.toHaveBeenCalled()
    expect(result.data).toContain('command blocked: path traversal')
  })
})
