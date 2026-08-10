import RFB from '@novnc/novnc'
import { Clipboard, ClipboardPaste, Keyboard, LockKeyhole, MonitorOff, RefreshCw, X } from 'lucide-react'
import { useCallback, useEffect, useRef, useState, type FormEvent } from 'react'

import type { DesktopSession } from '../runtime/contracts'
import type { RemoteRuntimeClient } from '../runtime/runtimeClient'

type ConnectionMode = 'view' | 'control'
type ConnectionState = 'connecting' | 'connected' | 'disconnected' | 'error'

type RemoteDesktopViewerProps = {
  client: RemoteRuntimeClient
  runId: string
  session: DesktopSession
  onStop?: () => Promise<void> | void
  onSessionChanged?: () => Promise<void> | void
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, milliseconds))
}

export default function RemoteDesktopViewer({
  client,
  runId,
  session,
  onStop,
  onSessionChanged,
}: RemoteDesktopViewerProps) {
  const targetRef = useRef<HTMLDivElement | null>(null)
  const rfbRef = useRef<RFB | null>(null)
  const generationRef = useRef(0)
  const mountedRef = useRef(true)
  const [connectionState, setConnectionState] = useState<ConnectionState>('disconnected')
  const [mode, setMode] = useState<ConnectionMode>('view')
  const [error, setError] = useState<string | null>(null)
  const [reauthVisible, setReauthVisible] = useState(false)
  const [password, setPassword] = useState('')
  const [reauthenticating, setReauthenticating] = useState(false)
  const [clipboardText, setClipboardText] = useState('')
  const [remoteClipboard, setRemoteClipboard] = useState('')

  const disconnectCurrent = useCallback(async () => {
    const rfb = rfbRef.current
    rfbRef.current = null
    generationRef.current += 1
    if (rfb) {
      await new Promise<void>((resolve) => {
        let resolved = false
        const finish = () => {
          if (resolved) return
          resolved = true
          resolve()
        }
        rfb.addEventListener('disconnect', finish, { once: true })
        try {
          rfb.disconnect()
        } catch {
          finish()
        }
        window.setTimeout(finish, 900)
      })
    }
    targetRef.current?.replaceChildren()
  }, [])

  const connect = useCallback(async (
    nextMode: ConnectionMode,
    reauthenticationToken?: string,
    retry = 0,
  ): Promise<void> => {
    setConnectionState('connecting')
    setError(null)
    try {
      const ticket = await client.createDesktopStreamTicket(
        runId,
        session.id,
        nextMode === 'control',
        reauthenticationToken,
      )
      await disconnectCurrent()
      if (!mountedRef.current || !targetRef.current) return
      const generation = generationRef.current + 1
      generationRef.current = generation
      const rfb = new RFB(targetRef.current, client.desktopStreamUrl(session.id, ticket.token), {
        shared: nextMode === 'view',
      })
      rfbRef.current = rfb
      rfb.viewOnly = nextMode === 'view'
      rfb.scaleViewport = true
      rfb.clipViewport = true
      rfb.resizeSession = false
      rfb.qualityLevel = 6
      rfb.compressionLevel = 4
      rfb.background = '#111827'
      rfb.addEventListener('connect', () => {
        if (generationRef.current !== generation) return
        setMode(nextMode)
        setConnectionState('connected')
        if (nextMode === 'control') rfb.focus({ preventScroll: true })
        void onSessionChanged?.()
      })
      rfb.addEventListener('disconnect', (event) => {
        if (generationRef.current !== generation) return
        rfbRef.current = null
        setConnectionState(event.detail.clean ? 'disconnected' : 'error')
        if (!event.detail.clean) setError('The remote desktop connection ended unexpectedly.')
        void onSessionChanged?.()
      })
      rfb.addEventListener('clipboard', (event) => {
        if (generationRef.current !== generation) return
        setRemoteClipboard(event.detail.text)
        if (navigator.clipboard?.writeText) {
          void navigator.clipboard.writeText(event.detail.text).catch(() => undefined)
        }
      })
      rfb.addEventListener('securityfailure', (event) => {
        if (generationRef.current !== generation) return
        setError(event.detail.reason ?? 'The remote desktop rejected the connection.')
        setConnectionState('error')
      })
    } catch (cause) {
      if (nextMode === 'view' && retry < 4 && mountedRef.current) {
        await delay(250 * (retry + 1))
        return connect(nextMode, undefined, retry + 1)
      }
      setConnectionState('error')
      setError(messageOf(cause))
      throw cause
    }
  }, [client, disconnectCurrent, onSessionChanged, runId, session.id])

  useEffect(() => {
    mountedRef.current = true
    void connect('view').catch(() => undefined)
    return () => {
      mountedRef.current = false
      void disconnectCurrent()
    }
  }, [connect, disconnectCurrent])

  const takeControl = async (event: FormEvent) => {
    event.preventDefault()
    if (!password || reauthenticating) return
    setReauthenticating(true)
    setError(null)
    try {
      const grant = await client.reauthenticateDesktopControl(password)
      setPassword('')
      setReauthVisible(false)
      await connect('control', grant.token)
    } catch (cause) {
      setError(messageOf(cause))
    } finally {
      setPassword('')
      setReauthenticating(false)
    }
  }

  const releaseControl = async () => {
    setConnectionState('connecting')
    await disconnectCurrent()
    setMode('view')
    await connect('view').catch(() => undefined)
  }

  const readLocalClipboard = async () => {
    if (!navigator.clipboard?.readText) return
    try {
      setClipboardText(await navigator.clipboard.readText())
    } catch (cause) {
      setError(messageOf(cause))
    }
  }

  return (
    <section className="remote-desktop-panel" aria-label="Remote desktop">
      <header className="remote-desktop-toolbar">
        <div>
          <strong>{mode === 'control' ? 'You are controlling this desktop' : 'Live desktop (view only)'}</strong>
          <span className={`remote-connection-state state-${connectionState}`}>{connectionState}</span>
        </div>
        <div className="remote-desktop-actions">
          {mode === 'view' ? (
            <button className="ui-button ui-button--primary ui-button--sm" type="button" onClick={() => setReauthVisible(true)} disabled={connectionState !== 'connected'}>
              <LockKeyhole size={14} /> Take control
            </button>
          ) : (
            <>
              <button className="ui-button ui-button--secondary ui-button--sm" type="button" onClick={() => rfbRef.current?.sendCtrlAltDel()}>
                <Keyboard size={14} /> Ctrl+Alt+Del
              </button>
              <button className="ui-button ui-button--secondary ui-button--sm" type="button" onClick={() => { void releaseControl() }}>
                <X size={14} /> Release control
              </button>
            </>
          )}
          <button className="ui-button ui-button--ghost ui-button--sm" type="button" onClick={() => { void connect(mode).catch(() => undefined) }} disabled={connectionState === 'connecting' || mode === 'control'} title={mode === 'control' ? 'Release control before reconnecting' : 'Reconnect'}>
            <RefreshCw size={14} /> Reconnect
          </button>
          {onStop ? (
            <button className="ui-button ui-button--danger ui-button--sm" type="button" onClick={() => { void onStop() }}>
              <MonitorOff size={14} /> End desktop
            </button>
          ) : null}
        </div>
      </header>

      {reauthVisible ? (
        <form className="remote-reauth" onSubmit={takeControl}>
          <label htmlFor={`desktop-password-${session.id}`}>Confirm your account password to take control</label>
          <input
            id={`desktop-password-${session.id}`}
            type="password"
            autoComplete="current-password"
            value={password}
            onChange={(event) => setPassword(event.target.value)}
            autoFocus
          />
          <button className="ui-button ui-button--primary ui-button--sm" type="submit" disabled={!password || reauthenticating}>
            {reauthenticating ? 'Confirming…' : 'Confirm and take control'}
          </button>
          <button className="ui-button ui-button--ghost ui-button--sm" type="button" onClick={() => { setPassword(''); setReauthVisible(false) }}>
            Cancel
          </button>
        </form>
      ) : null}

      {error ? <div className="remote-inline-error" role="alert">{error}</div> : null}
      <div className="remote-desktop-viewport" ref={targetRef} aria-label="Remote desktop display" />

      {mode === 'control' ? (
        <div className="remote-clipboard-tools">
          <textarea value={clipboardText} onChange={(event) => setClipboardText(event.target.value)} placeholder="Text to paste into the remote desktop" aria-label="Clipboard text to send" />
          <button className="ui-button ui-button--secondary ui-button--sm" type="button" onClick={() => { void readLocalClipboard() }}>
            <Clipboard size={14} /> Read local clipboard
          </button>
          <button className="ui-button ui-button--secondary ui-button--sm" type="button" onClick={() => rfbRef.current?.clipboardPasteFrom(clipboardText)} disabled={!clipboardText}>
            <ClipboardPaste size={14} /> Paste remotely
          </button>
          {remoteClipboard ? <span title={remoteClipboard}>Remote clipboard received</span> : null}
        </div>
      ) : null}
    </section>
  )
}
