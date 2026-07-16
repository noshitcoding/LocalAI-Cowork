import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Terminal as XTerm } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import '@xterm/xterm/css/xterm.css'
import { Plus, Square, Trash2, X } from 'lucide-react'
import {
  isRiskyTerminalCommand,
  useTerminalStore,
  type TerminalSession,
} from '../stores/terminalStore'
import { tr } from '../i18n'

type TerminalDockProps = {
  threadId: string
  cwd: string
}

function getSessionLabel(session: TerminalSession) {
  if (session.currentAiCommand) return 'AI running'
  if (session.kind === 'ai') return session.title || 'AI Shell'
  return session.title || session.shell
}

function getStatusLabel(session: TerminalSession | null) {
  if (!session) return 'No terminal'
  if (session.currentAiCommand) return 'AI cmd'
  switch (session.status) {
    case 'idle':
      return 'ready'
    case 'running':
      return 'running'
    case 'exited':
      return 'exited'
    case 'error':
      return 'error'
  }
}

export default function TerminalDock({ threadId, cwd }: TerminalDockProps) {
  const sessions = useTerminalStore((state) => state.sessionsByThread[threadId] ?? [])
  const activeSessionId = useTerminalStore((state) => state.activeSessionIds[threadId] ?? null)
  const height = useTerminalStore((state) => state.dockHeightByThread[threadId] ?? 280)
  const ensureListeners = useTerminalStore((state) => state.ensureListeners)
  const createSession = useTerminalStore((state) => state.createSession)
  const setActiveSession = useTerminalStore((state) => state.setActiveSession)
  const setDockHeight = useTerminalStore((state) => state.setDockHeight)
  const writeToSession = useTerminalStore((state) => state.writeToSession)
  const resizeSession = useTerminalStore((state) => state.resizeSession)
  const interruptSession = useTerminalStore((state) => state.interruptSession)
  const killSession = useTerminalStore((state) => state.killSession)
  const closeSession = useTerminalStore((state) => state.closeSession)
  const markAiIntervention = useTerminalStore((state) => state.markAiIntervention)
  const terminalContainerRef = useRef<HTMLDivElement>(null)
  const terminalRef = useRef<XTerm | null>(null)
  const fitAddonRef = useRef<FitAddon | null>(null)
  const terminalInputRef = useRef<(data: string) => void>(() => undefined)
  const renderedOutputRef = useRef<{ sessionId: string | null; length: number }>({ sessionId: null, length: 0 })
  const resizeStartRef = useRef<{ y: number; height: number } | null>(null)
  const [input, setInput] = useState('')

  const activeSession = useMemo(
    () => sessions.find((session) => session.id === activeSessionId) ?? sessions[0] ?? null,
    [activeSessionId, sessions],
  )

  useEffect(() => {
    ensureListeners()
  }, [ensureListeners])

  useEffect(() => {
    const node = terminalContainerRef.current
    if (!node) return undefined

    const terminal = new XTerm({
      cursorBlink: true,
      convertEol: true,
      fontFamily: '"Cascadia Mono", Consolas, "Liberation Mono", monospace',
      fontSize: 12,
      lineHeight: 1.15,
      scrollback: 10_000,
      theme: {
        background: '#05070d',
        foreground: '#d8dee9',
        cursor: '#f5f7fb',
        selectionBackground: '#34506f',
        black: '#141820',
        red: '#f87171',
        green: '#6ee7b7',
        yellow: '#facc15',
        blue: '#60a5fa',
        magenta: '#c084fc',
        cyan: '#67e8f9',
        white: '#e5e7eb',
      },
    })
    const fitAddon = new FitAddon()
    terminal.loadAddon(fitAddon)
    terminal.open(node)
    const dataDisposable = terminal.onData((data) => terminalInputRef.current(data))
    terminalRef.current = terminal
    fitAddonRef.current = fitAddon
    window.requestAnimationFrame(() => fitAddon.fit())

    return () => {
      dataDisposable.dispose()
      terminal.dispose()
      terminalRef.current = null
      fitAddonRef.current = null
    }
  }, [])

  useEffect(() => {
    if (!activeSession && sessions.length === 0) {
      void createSession({ threadId, cwd, title: 'PowerShell' })
    }
  }, [activeSession, createSession, cwd, sessions.length, threadId])

  useEffect(() => {
    const terminal = terminalRef.current
    if (!terminal) return
    const sessionId = activeSession?.id ?? null
    const output = activeSession?.output || tr('PowerShell ready.')
    const rendered = renderedOutputRef.current

    if (rendered.sessionId !== sessionId || output.length < rendered.length) {
      terminal.clear()
      terminal.write(output)
    } else {
      const nextOutput = output.slice(rendered.length)
      if (nextOutput) terminal.write(nextOutput)
    }
    renderedOutputRef.current = { sessionId, length: output.length }
  }, [activeSession?.id, activeSession?.output])

  useEffect(() => {
    const activeSessionId = activeSession?.id
    if (!activeSessionId) return
    const frame = window.requestAnimationFrame(() => {
      fitAddonRef.current?.fit()
      const terminal = terminalRef.current
      if (!terminal) return
      void resizeSession(activeSessionId, terminal.cols, terminal.rows)
    })
    return () => window.cancelAnimationFrame(frame)
  }, [activeSession?.id, height, resizeSession])

  useEffect(() => {
    const handleWindowResize = () => {
      fitAddonRef.current?.fit()
      const terminal = terminalRef.current
      if (!activeSession || !terminal) return
      void resizeSession(activeSession.id, terminal.cols, terminal.rows)
    }
    window.addEventListener('resize', handleWindowResize)
    return () => window.removeEventListener('resize', handleWindowResize)
  }, [activeSession, resizeSession])

  const beginResize = (event: React.PointerEvent<HTMLDivElement>) => {
    resizeStartRef.current = { y: event.clientY, height }
    event.currentTarget.setPointerCapture(event.pointerId)
  }

  const updateResize = (event: React.PointerEvent<HTMLDivElement>) => {
    const start = resizeStartRef.current
    if (!start) return
    setDockHeight(threadId, start.height + (start.y - event.clientY))
  }

  const endResize = (event: React.PointerEvent<HTMLDivElement>) => {
    resizeStartRef.current = null
    event.currentTarget.releasePointerCapture(event.pointerId)
  }

  const handleNewSession = async () => {
    const session = await createSession({ threadId, cwd, title: 'PowerShell' })
    setActiveSession(threadId, session.id)
  }

  const confirmRisk = useCallback((command: string) => {
    if (!isRiskyTerminalCommand(command)) return true
    return window.confirm('This terminal command looks risky. Send anyway?')
  }, [])

  const confirmAiIntervention = useCallback((session: TerminalSession) => {
    if (!session.currentAiCommand) return true
    if (session.currentAiCommand.intervention) return true
    const ok = window.confirm(tr('This tab is currently running an AI command. Send input to the same process anyway?'))
    if (ok) {
      markAiIntervention(session.id)
    }
    return ok
  }, [markAiIntervention])

  const sendInput = async () => {
    const command = input
    if (!activeSession || !command) return
    if (!confirmAiIntervention(activeSession)) return
    if (!confirmRisk(command)) return
    setInput('')
    await writeToSession(activeSession.id, `${command}\r`)
  }

  const sendDirectTerminalInput = useCallback(async (data: string) => {
    if (!activeSession || !data || activeSession.status === 'exited') return
    if (!confirmAiIntervention(activeSession)) return
    await writeToSession(activeSession.id, data)
  }, [activeSession, confirmAiIntervention, writeToSession])

  useEffect(() => {
    terminalInputRef.current = (data: string) => {
      void sendDirectTerminalInput(data)
    }
  }, [sendDirectTerminalInput])

  const handleTerminalPaste = (event: React.ClipboardEvent<HTMLDivElement>) => {
    if (!activeSession || activeSession.status === 'exited') return
    const text = event.clipboardData.getData('text')
    if (!text) return
    event.preventDefault()
    if (!confirmAiIntervention(activeSession)) return
    if (!confirmRisk(text)) return
    void writeToSession(activeSession.id, text)
  }

  const handlePaste = (event: React.ClipboardEvent<HTMLInputElement>) => {
    const text = event.clipboardData.getData('text')
    if (text && isRiskyTerminalCommand(text) && !window.confirm('Der eingefuegte Befehl wirkt riskant. Einfuegen?')) {
      event.preventDefault()
    }
  }

  return (
    <div className="terminal-dock" style={{ height }}>
      <div
        className="terminal-dock-resize"
        onPointerDown={beginResize}
        onPointerMove={updateResize}
        onPointerUp={endResize}
        role="separator"
        aria-label={tr('Terminal height')}
      />
      <div className="terminal-dock-tabs">
        <div className="terminal-tab-list" role="tablist" aria-label={tr('Terminal tabs')}>
          {sessions.map((session) => (
            <button
              key={session.id}
              type="button"
              role="tab"
              aria-selected={session.id === activeSession?.id}
              className={`terminal-tab${session.id === activeSession?.id ? ' active' : ''}${session.hidden ? ' hidden-activity' : ''}`}
              onClick={() => setActiveSession(threadId, session.id)}
              title={session.cwd}
            >
              <span>{getSessionLabel(session)}</span>
              <small>{session.status}</small>
            </button>
          ))}
        </div>
        <button type="button" className="terminal-icon-button" onClick={handleNewSession} title={tr('New terminal')} aria-label={tr('New terminal')}>
          <Plus size={15} />
        </button>
        <div className="terminal-dock-status">{getStatusLabel(activeSession)}</div>
        <button
          type="button"
          className="terminal-icon-button"
          onClick={() => activeSession && interruptSession(activeSession.id)}
          disabled={!activeSession || activeSession.status === 'exited'}
          title={tr('Stop')}
          aria-label={tr('Stop')}
        >
          <Square size={14} />
        </button>
        <button
          type="button"
          className="terminal-icon-button"
          onClick={() => activeSession && killSession(activeSession.id)}
          disabled={!activeSession || activeSession.status === 'exited'}
          title={tr('Kill terminal')}
          aria-label={tr('Kill terminal')}
        >
          <Trash2 size={14} />
        </button>
        <button
          type="button"
          className="terminal-icon-button"
          onClick={() => activeSession && closeSession(activeSession.id)}
          disabled={!activeSession}
          title={tr('Close terminal')}
          aria-label={tr('Close terminal')}
        >
          <X size={15} />
        </button>
      </div>
      <div
        ref={terminalContainerRef}
        className="terminal-dock-output terminal-dock-xterm"
        aria-live={activeSession?.status === 'running' ? 'polite' : undefined}
        aria-label={tr('Terminal output')}
        onPaste={handleTerminalPaste}
        title={tr('Terminal focus: type directly here')}
      />
      <div className="terminal-dock-input-row">
        <span className="terminal-prompt">{tr('PS')}</span>
        <input
          value={input}
          onChange={(event) => setInput(event.currentTarget.value)}
          onPaste={handlePaste}
          onKeyDown={(event) => {
            if (event.key === 'Enter') {
              event.preventDefault()
              void sendInput()
            }
          }}
          placeholder={activeSession?.currentAiCommand ? tr('AI command running; manual input requires confirmation') : tr('Enter command...')}
          disabled={!activeSession || activeSession.status === 'exited'}
        />
        <button type="button" className="btn-sm" onClick={() => void sendInput()} disabled={!activeSession || !input.trim()}>
          {tr('Send')}
        </button>
      </div>
    </div>
  )
}
