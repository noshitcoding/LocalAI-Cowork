import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useTerminalStore } from '../../stores/terminalStore'

const invokeMock = vi.fn()
const listenMock = vi.fn(async (_event?: string, _handler?: unknown) => () => {})
const runAiCommandMock = vi.fn()

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}))

vi.mock('@tauri-apps/api/event', () => ({
  listen: (event: string, handler: unknown) => listenMock(event, handler),
}))

vi.mock('../../utils/safeInvoke', () => ({
  hasTauriRuntime: vi.fn(() => false),
  safeInvoke: vi.fn(async (_cmd: string, _args: unknown, fallback: unknown) => fallback),
  safeInvokeVoid: vi.fn(),
}))

describe('Bash native sandbox integration', () => {
  beforeEach(() => {
    invokeMock.mockReset()
    listenMock.mockClear()
    runAiCommandMock.mockReset()
    invokeMock.mockResolvedValue({
      stdout: 'Hallo',
      stderr: '',
      exitCode: 0,
      status: 'completed',
      timedOut: false,
      durationMs: 12,
      sandboxId: 'native:run-bash',
      stdoutTruncated: false,
      stderrTruncated: false,
      stdoutInvalidUtf8: false,
      stderrInvalidUtf8: false,
    })
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

  it('routes Bash exclusively through sandbox_exec_command and mirrors an output-only tab', async () => {
    const { registerAllBuiltinTools, getAllTools } = await import('./registry')
    registerAllBuiltinTools()
    const tool = getAllTools().find((entry) => entry.name === 'Bash')

    const result = await tool!.call(
      { command: 'cmd /c echo Hallo', timeout: 1234, shell: 'cmd' },
      { cwd: 'C:\\sandbox\\workspace', runId: 'run-bash', threadId: 'thread-1' } as never,
    )

    expect(invokeMock).toHaveBeenCalledWith('sandbox_exec_command', {
      request: {
        runId: 'run-bash',
        command: 'cmd /c echo Hallo',
        shell: 'cmd',
        cwd: 'C:\\sandbox\\workspace',
        timeoutMs: 1234,
        streamId: expect.any(String),
      },
    })
    expect(invokeMock).not.toHaveBeenCalledWith('exec_command', expect.anything())
    expect(invokeMock).not.toHaveBeenCalledWith('terminal_write', expect.anything())
    expect(invokeMock).not.toHaveBeenCalledWith('desktop_launch_app', expect.anything())
    expect(runAiCommandMock).not.toHaveBeenCalled()
    expect(result).toMatchObject({ isError: false })
    expect(result.data).toContain('stdout:\nHallo')
    expect(useTerminalStore.getState().sessionsByThread['thread-1']?.[0]).toMatchObject({
      title: 'AI Sandbox',
      kind: 'sandbox',
    })
  })

  it('returns setup and policy failures as real tool errors without a host fallback', async () => {
    invokeMock.mockRejectedValueOnce(new Error('native sandbox setup is required'))
    const { registerAllBuiltinTools, getAllTools } = await import('./registry')
    registerAllBuiltinTools()
    const tool = getAllTools().find((entry) => entry.name === 'Bash')

    const result = await tool!.call(
      { command: 'Get-Content ..\\secret.txt' },
      { cwd: 'C:\\sandbox\\workspace', runId: 'run-denied', threadId: 'thread-1' } as never,
    )

    expect(result.isError).toBe(true)
    expect(result.data).toContain('native sandbox setup is required')
    expect(runAiCommandMock).not.toHaveBeenCalled()
    expect(invokeMock).toHaveBeenCalledTimes(1)
  })

  it('terminates the sandbox job when the engine run is aborted', async () => {
    let finishCommand: ((value: unknown) => void) | undefined
    invokeMock.mockImplementation((command: string) => {
      if (command === 'sandbox_exec_cancel') return Promise.resolve(true)
      if (command === 'sandbox_exec_command') {
        return new Promise((resolve) => { finishCommand = resolve })
      }
      return Promise.resolve(undefined)
    })
    const { registerAllBuiltinTools, getAllTools } = await import('./registry')
    registerAllBuiltinTools()
    const tool = getAllTools().find((entry) => entry.name === 'Bash')!
    const abortController = new AbortController()
    const resultPromise = tool.call(
      { command: 'Start-Sleep -Seconds 30' },
      { cwd: 'C:\\sandbox\\workspace', runId: 'run-abort', threadId: 'thread-1', abortController } as never,
    )
    await vi.waitFor(() => expect(finishCommand).toBeTypeOf('function'))
    abortController.abort()
    await vi.waitFor(() => expect(invokeMock).toHaveBeenCalledWith('sandbox_exec_cancel', { streamId: expect.any(String) }))
    finishCommand!({
      stdout: '', stderr: 'cancelled', exitCode: 130, status: 'failed', timedOut: false,
      durationMs: 3, sandboxId: 'native:run-abort', stdoutTruncated: false,
      stderrTruncated: false, stdoutInvalidUtf8: false, stderrInvalidUtf8: false,
    })
    const result = await resultPromise
    expect(result.isError).toBe(true)
  })
})
