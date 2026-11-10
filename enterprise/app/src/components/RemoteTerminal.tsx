import { FitAddon } from '@xterm/addon-fit'
import { Terminal as XTerm } from '@xterm/xterm'
import '@xterm/xterm/css/xterm.css'
import { Keyboard, PlugZap, RotateCcw, X } from 'lucide-react'
import { useCallback, useEffect, useRef, useState } from 'react'

import type { RemoteRuntimeClient } from '../runtime/runtimeClient'

type RemoteTerminalProps = {
  client: RemoteRuntimeClient
  runId: string
  onClose?: () => void
}

type ConnectionState = 'connecting' | 'connected' | 'closed' | 'failed'

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

export default function RemoteTerminal({ client, runId, onClose }: RemoteTerminalProps) {
  const containerRef = useRef<HTMLDivElement>(null)
  const terminalRef = useRef<XTerm | null>(null)
  const fitRef = useRef<FitAddon | null>(null)
  const socketRef = useRef<WebSocket | null>(null)
  const generationRef = useRef(0)
  const [connectionState, setConnectionState] = useState<ConnectionState>('connecting')
  const [error, setError] = useState<string | null>(null)

  const send = useCallback((value: string) => {
    const socket = socketRef.current
    if (socket?.readyState === WebSocket.OPEN) socket.send(new TextEncoder().encode(value))
  }, [])

  const connect = useCallback(async () => {
    const terminal = terminalRef.current
    const fit = fitRef.current
    if (!terminal || !fit) return
    const generation = ++generationRef.current
    socketRef.current?.close(1000, 'reconnecting')
    socketRef.current = null
    setConnectionState('connecting')
    setError(null)
    terminal.reset()
    terminal.writeln('\x1b[38;5;45mOpening isolated server terminal…\x1b[0m')
    try {
      fit.fit()
      const ticket = await client.createTerminalSession(runId, {
        columns: Math.max(20, Math.min(400, terminal.cols)),
        rows: Math.max(5, Math.min(200, terminal.rows)),
      })
      if (generationRef.current !== generation) return
      const socket = new WebSocket(client.terminalStreamUrl(ticket.session_id, ticket.token))
      socket.binaryType = 'arraybuffer'
      socketRef.current = socket
      socket.onopen = () => {
        if (generationRef.current !== generation) return
        setConnectionState('connected')
        terminal.focus()
      }
      socket.onmessage = (event) => {
        if (event.data instanceof ArrayBuffer) terminal.write(new Uint8Array(event.data))
        else if (event.data instanceof Blob) {
          void event.data.arrayBuffer().then((data) => terminal.write(new Uint8Array(data)))
        } else terminal.write(String(event.data))
      }
      socket.onerror = () => {
        if (generationRef.current !== generation) return
        setConnectionState('failed')
        setError('The terminal WebSocket failed.')
      }
      socket.onclose = (event) => {
        if (generationRef.current !== generation) return
        socketRef.current = null
        setConnectionState((current) => current === 'failed' ? current : 'closed')
        if (event.code !== 1000) {
          terminal.writeln(`\r\n\x1b[31mTerminal disconnected (${event.code}).\x1b[0m`)
        }
      }
    } catch (cause) {
      if (generationRef.current !== generation) return
      setConnectionState('failed')
      setError(messageOf(cause))
      terminal.writeln(`\r\n\x1b[31m${messageOf(cause)}\x1b[0m`)
    }
  }, [client, runId])

  useEffect(() => {
    const container = containerRef.current
    if (!container) return
    const terminal = new XTerm({
      cursorBlink: true,
      fontFamily: '"Cascadia Mono", Consolas, "Liberation Mono", monospace',
      fontSize: 12,
      lineHeight: 1.15,
      scrollback: 5_000,
      theme: {
        background: '#05070d',
        foreground: '#d8dee9',
        cursor: '#53d3bd',
        selectionBackground: '#34506f',
      },
    })
    const fit = new FitAddon()
    terminal.loadAddon(fit)
    terminal.open(container)
    terminalRef.current = terminal
    fitRef.current = fit
    const input = terminal.onData(send)
    const resize = new ResizeObserver(() => {
      try { fit.fit() } catch { /* The terminal may be disposing. */ }
    })
    resize.observe(container)
    const frame = window.requestAnimationFrame(() => { void connect() })
    return () => {
      generationRef.current += 1
      window.cancelAnimationFrame(frame)
      resize.disconnect()
      input.dispose()
      socketRef.current?.close(1000, 'terminal view closed')
      socketRef.current = null
      terminal.dispose()
      terminalRef.current = null
      fitRef.current = null
    }
  }, [connect, send])

  return (
    <section className="remote-terminal-panel" aria-label="Server terminal">
      <header className="remote-terminal-toolbar">
        <div><Keyboard size={15} /><strong>Server terminal</strong><span className={`terminal-status status-${connectionState}`}>{connectionState}</span></div>
        <div>
          <button className="ui-button ui-button--secondary ui-button--sm" type="button" onClick={() => { void connect() }}><RotateCcw size={14} /> Reconnect</button>
          {onClose ? <button className="ui-button ui-button--ghost ui-button--sm" type="button" onClick={onClose}><X size={14} /> Close</button> : null}
        </div>
      </header>
      {error ? <div className="remote-inline-error" role="alert">{error}</div> : null}
      <div ref={containerRef} className="remote-terminal-output" />
      <div className="remote-terminal-keys" aria-label="Terminal shortcut keys">
        <button type="button" onClick={() => send('\u0003')}>Ctrl+C</button>
        <button type="button" onClick={() => send('\u0004')}>Ctrl+D</button>
        <button type="button" onClick={() => send('\u001b')}>Esc</button>
        <button type="button" onClick={() => send('\t')}>Tab</button>
        <button type="button" onClick={() => send('\u001b[A')}>↑</button>
        <button type="button" onClick={() => send('\u001b[B')}>↓</button>
        <span><PlugZap size={13} /> Workspace sandbox · no network</span>
      </div>
    </section>
  )
}
