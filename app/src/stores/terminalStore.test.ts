import { beforeEach, describe, expect, it, vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'
import { hasTauriRuntime } from '../utils/safeInvoke'
import { useTerminalStore } from './terminalStore'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}))

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(),
}))

vi.mock('../utils/safeInvoke', () => ({
  hasTauriRuntime: vi.fn(() => false),
  safeInvoke: vi.fn(async (_cmd: string, _args: unknown, fallback: unknown) => fallback),
  safeInvokeVoid: vi.fn(),
}))

function resetTerminalState() {
  useTerminalStore.setState({
    backends: [],
    loading: false,
    error: null,
    sessionsByThread: {},
    activeSessionIds: {},
    dockOpenByThread: {},
    dockHeightByThread: {},
    hiddenActivityByThread: {},
    activeAiThreadId: null,
  })
}

describe('terminalStore dock state', () => {
  beforeEach(() => {
    vi.useRealTimers()
    vi.mocked(invoke).mockReset()
    vi.mocked(hasTauriRuntime).mockReturnValue(false)
    resetTerminalState()
  })

  it('marks hidden activity for hidden sessions and clears it when the dock opens', async () => {
    await useTerminalStore.getState().createSession({
      threadId: 'thread-1',
      cwd: 'C:/repo',
      kind: 'ai',
      hidden: true,
    })

    expect(useTerminalStore.getState().hiddenActivityByThread['thread-1']).toBe(true)

    useTerminalStore.getState().setDockOpen('thread-1', true)

    expect(useTerminalStore.getState().hiddenActivityByThread['thread-1']).toBe(false)
  })

  it('creates a hidden AI tab for commands while the dock is closed', async () => {
    const result = await useTerminalStore.getState().runAiCommand({
      threadId: 'thread-1',
      cwd: 'C:/repo',
      command: 'Get-Location',
      timeoutMs: 1000,
    })

    const sessions = useTerminalStore.getState().sessionsByThread['thread-1'] ?? []

    expect(result.exitCode).toBe(1)
    expect(sessions).toHaveLength(1)
    expect(sessions[0]).toMatchObject({ kind: 'ai', hidden: true })
    expect(useTerminalStore.getState().hiddenActivityByThread['thread-1']).toBe(true)
  })

  it('does not reuse the visible manual tab for AI commands while the dock is closed', async () => {
    await useTerminalStore.getState().createSession({
      threadId: 'thread-1',
      cwd: 'C:/repo',
      kind: 'manual',
      hidden: false,
    })

    await useTerminalStore.getState().runAiCommand({
      threadId: 'thread-1',
      cwd: 'C:/repo',
      command: 'Get-Location',
      timeoutMs: 1000,
    })

    const sessions = useTerminalStore.getState().sessionsByThread['thread-1'] ?? []

    expect(sessions).toHaveLength(2)
    expect(sessions[0]).toMatchObject({ kind: 'manual', hidden: false })
    expect(sessions[1]).toMatchObject({ kind: 'ai', hidden: true })
  })

  it('terminates the PTY session when an AI command times out', async () => {
    vi.useFakeTimers()
    vi.mocked(hasTauriRuntime).mockReturnValue(true)
    vi.mocked(invoke).mockResolvedValue(undefined)

    const resultPromise = useTerminalStore.getState().runAiCommand({
      threadId: 'thread-timeout',
      cwd: 'C:/repo',
      command: 'Start-Sleep -Seconds 30',
      timeoutMs: 1000,
    })
    const rejection = expect(resultPromise).rejects.toThrow('Terminal command timed out after 1000ms')

    await vi.advanceTimersByTimeAsync(1001)
    await rejection

    const sessions = useTerminalStore.getState().sessionsByThread['thread-timeout'] ?? []
    expect(invoke).toHaveBeenCalledWith('terminal_kill', {
      request: { sessionId: sessions[0].id },
    })
    expect(sessions[0]).toMatchObject({ status: 'exited', currentAiCommand: undefined })
    expect(sessions[0].output).toContain('session terminated')
  })
})
