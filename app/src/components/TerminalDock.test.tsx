import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import TerminalDock from './TerminalDock'
import { useTerminalStore } from '../stores/terminalStore'

const xtermInstances = vi.hoisted(() => [] as Array<{ emitData: (data: string) => void }>)

vi.mock('@xterm/xterm', () => ({
  Terminal: class {
    cols = 100
    rows = 24
    private handlers: Array<(data: string) => void> = []
    loadAddon = vi.fn()
    open = vi.fn((node: HTMLElement) => {
      node.setAttribute('data-xterm-open', 'true')
    })
    clear = vi.fn()
    write = vi.fn()
    focus = vi.fn()
    dispose = vi.fn()
    onData = vi.fn((handler: (data: string) => void) => {
      this.handlers.push(handler)
      return { dispose: vi.fn() }
    })

    constructor() {
      xtermInstances.push({
        emitData: (data: string) => this.handlers.forEach((handler) => handler(data)),
      })
    }
  },
}))

vi.mock('@xterm/addon-fit', () => ({
  FitAddon: class {
    fit = vi.fn()
  },
}))

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

describe('TerminalDock', () => {
  beforeEach(() => {
    resetTerminalState()
    xtermInstances.splice(0)
    vi.restoreAllMocks()
  })

  it('creates a default terminal and lets users add and close tabs', async () => {
    render(<TerminalDock threadId="thread-1" cwd="C:/repo" />)

    expect(await screen.findByRole('tab', { name: /PowerShell/i })).toBeInTheDocument()

    fireEvent.click(screen.getByLabelText('New terminal'))
    await waitFor(() => expect(screen.getAllByRole('tab')).toHaveLength(2))

    fireEvent.click(screen.getByLabelText('Close terminal'))
    await waitFor(() => expect(screen.getAllByRole('tab')).toHaveLength(1))
  })

  it('asks before sending a risky command from the command line', async () => {
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(false)
    render(<TerminalDock threadId="thread-1" cwd="C:/repo" />)

    await screen.findByRole('tab', { name: /PowerShell/i })
    fireEvent.change(screen.getByPlaceholderText('Enter command...'), {
      target: { value: 'Remove-Item -Recurse C:/tmp' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Send' }))

    expect(confirmSpy).toHaveBeenCalled()
    expect(useTerminalStore.getState().sessionsByThread['thread-1']?.[0]?.output).not.toContain('Remove-Item')
  })

  it('sends direct keyboard input from the focused terminal output', async () => {
    render(<TerminalDock threadId="thread-1" cwd="C:/repo" />)

    await screen.findByTitle('Terminal focus: type directly here')
    await waitFor(() => expect(xtermInstances).toHaveLength(1))
    xtermInstances[0].emitData('a')
    xtermInstances[0].emitData('\r')

    await waitFor(() => {
      const output = useTerminalStore.getState().sessionsByThread['thread-1']?.[0]?.output ?? ''
      expect(output).toContain('a')
      expect(output).toContain('\r')
    })
  })
})
