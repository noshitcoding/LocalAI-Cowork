import { useCallback, useEffect, useMemo, useState } from 'react'
import type { MouseEvent as ReactMouseEvent, WheelEvent as ReactWheelEvent } from 'react'
import {
  ArrowLeft,
  ArrowRight,
  Braces,
  Circle,
  Code2,
  Copy,
  Crosshair,
  ExternalLink,
  Globe2,
  LoaderCircle,
  MessageSquarePlus,
  MousePointer2,
  Network,
  Play,
  RefreshCw,
  Send,
  Square,
  SquareTerminal,
  Trash2,
} from 'lucide-react'
import { safeInvoke } from '../utils/safeInvoke'

type BrowserSessionInfo = {
  active: boolean
  browserName: string
  debuggerPort: number
  profilePath: string
}

type BrowserConsoleEntry = {
  level: string
  message: string
  timestamp: number
}

type BrowserNetworkEntry = {
  url: string
  method: string
  status: number
  kind: string
  durationMs: number
  transferSize: number
  timestamp: number
}

type BrowserSnapshot = {
  active: boolean
  url: string
  title: string
  viewportWidth: number
  viewportHeight: number
  deviceScaleFactor: number
  screenshotDataUrl: string
  dom: string
  text: string
  activeElement: string
  consoleEntries: BrowserConsoleEntry[]
  networkEntries: BrowserNetworkEntry[]
}

type ElementInspection = {
  selector: string
  tagName: string
  id: string
  classes: string[]
  text: string
  attributes: Record<string, string>
  x: number
  y: number
  width: number
  height: number
}

type BrowserAnnotation = {
  id: string
  url: string
  selector: string
  element: string
  note: string
  x: number
  y: number
  width: number
  height: number
  createdAt: number
}

type InspectorTab = 'annotations' | 'console' | 'network' | 'dom' | 'cdp'
type BrowserMode = 'interact' | 'annotate'

const EMPTY_SESSION: BrowserSessionInfo = {
  active: false,
  browserName: '',
  debuggerPort: 0,
  profilePath: '',
}

const ANNOTATION_STORAGE_KEY = 'localai-cowork:developer-browser:annotations:v1'

function readAnnotations(): BrowserAnnotation[] {
  try {
    const parsed = JSON.parse(localStorage.getItem(ANNOTATION_STORAGE_KEY) ?? '[]')
    return Array.isArray(parsed) ? parsed : []
  } catch {
    return []
  }
}

function formatBytes(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return '—'
  if (value < 1024) return `${Math.round(value)} B`
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`
  return `${(value / (1024 * 1024)).toFixed(1)} MB`
}

function hostLabel(value: string): string {
  try {
    return new URL(value).host
  } catch {
    return value
  }
}

function buildAnnotationPrompt(snapshot: BrowserSnapshot | null, annotations: BrowserAnnotation[]): string {
  const pageAnnotations = annotations.filter((annotation) => annotation.url === snapshot?.url)
  const lines = pageAnnotations.map((annotation, index) => (
    `${index + 1}. ${annotation.selector} (${annotation.element}): ${annotation.note}`
  ))
  return [
    `Open and inspect ${snapshot?.url ?? 'the current page'} in the developer browser.`,
    snapshot?.title ? `Page title: ${snapshot.title}` : '',
    'Address these element-specific browser comments:',
    ...lines,
    '',
    'After making the smallest necessary code changes, reload the page and verify every comment.',
  ].filter(Boolean).join('\n')
}

export default function DeveloperBrowserView() {
  const [session, setSession] = useState<BrowserSessionInfo>(EMPTY_SESSION)
  const [snapshot, setSnapshot] = useState<BrowserSnapshot | null>(null)
  const [url, setUrl] = useState('http://127.0.0.1:5173')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [mode, setMode] = useState<BrowserMode>('interact')
  const [inspectorTab, setInspectorTab] = useState<InspectorTab>('annotations')
  const [annotations, setAnnotations] = useState<BrowserAnnotation[]>(readAnnotations)
  const [pendingElement, setPendingElement] = useState<ElementInspection | null>(null)
  const [annotationNote, setAnnotationNote] = useState('')
  const [textInput, setTextInput] = useState('')
  const [copyStatus, setCopyStatus] = useState('')
  const [cdpMethod, setCdpMethod] = useState('Runtime.evaluate')
  const [cdpParams, setCdpParams] = useState('{\n  "expression": "document.title",\n  "returnByValue": true\n}')
  const [cdpResult, setCdpResult] = useState('Run an allowed CDP command to inspect its response.')

  const pageAnnotations = useMemo(
    () => annotations.filter((annotation) => annotation.url === snapshot?.url),
    [annotations, snapshot?.url],
  )

  useEffect(() => {
    localStorage.setItem(ANNOTATION_STORAGE_KEY, JSON.stringify(annotations.slice(-500)))
  }, [annotations])

  const loadSnapshot = useCallback(async () => {
    const next = await safeInvoke<BrowserSnapshot>('developer_browser_snapshot')
    setSnapshot(next)
    if (next.url && next.url !== 'about:blank') setUrl(next.url)
  }, [])

  useEffect(() => {
    let cancelled = false
    void safeInvoke<BrowserSessionInfo>('developer_browser_status', undefined, EMPTY_SESSION)
      .then(async (next) => {
        if (cancelled) return
        setSession(next)
        if (next.active) {
          try {
            await loadSnapshot()
          } catch {
            // A stale browser process is surfaced when the user explicitly restarts it.
          }
        }
      })
    return () => {
      cancelled = true
    }
  }, [loadSnapshot])

  const run = useCallback(async (operation: () => Promise<void>, refresh = true) => {
    setBusy(true)
    setError(null)
    try {
      await operation()
      if (refresh) await loadSnapshot()
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setBusy(false)
    }
  }, [loadSnapshot])

  const startBrowser = () => run(async () => {
    const next = await safeInvoke<BrowserSessionInfo>('developer_browser_start')
    setSession(next)
    await safeInvoke('developer_browser_navigate', { request: { url } })
  })

  const stopBrowser = () => run(async () => {
    await safeInvoke('developer_browser_stop')
    setSession(EMPTY_SESSION)
    setSnapshot(null)
    setPendingElement(null)
  }, false)

  const navigate = () => run(async () => {
    if (!session.active) {
      const next = await safeInvoke<BrowserSessionInfo>('developer_browser_start')
      setSession(next)
    }
    await safeInvoke('developer_browser_navigate', { request: { url } })
  })

  const history = (direction: 'back' | 'forward') => run(async () => {
    await safeInvoke('developer_browser_history', { request: { direction } })
  })

  const reload = () => run(async () => {
    await safeInvoke('developer_browser_reload')
  })

  const pagePoint = (
    event: ReactMouseEvent<HTMLDivElement>,
  ): { x: number; y: number } | null => {
    if (!snapshot) return null
    const bounds = event.currentTarget.getBoundingClientRect()
    if (bounds.width <= 0 || bounds.height <= 0) return null
    return {
      x: ((event.clientX - bounds.left) / bounds.width) * snapshot.viewportWidth,
      y: ((event.clientY - bounds.top) / bounds.height) * snapshot.viewportHeight,
    }
  }

  const handleScreenshotClick = (event: ReactMouseEvent<HTMLDivElement>) => {
    const point = pagePoint(event)
    if (!point || busy) return
    if (mode === 'annotate') {
      void run(async () => {
        const inspected = await safeInvoke<ElementInspection>('developer_browser_inspect', {
          request: { ...point, doubleClick: false },
        })
        setPendingElement(inspected)
        setAnnotationNote('')
        setInspectorTab('annotations')
      }, false)
      return
    }
    void run(async () => {
      await safeInvoke('developer_browser_click', {
        request: { ...point, doubleClick: event.detail > 1 },
      })
    })
  }

  const handleScreenshotWheel = (event: ReactWheelEvent<HTMLDivElement>) => {
    if (!snapshot || busy || mode !== 'interact') return
    event.preventDefault()
    const bounds = event.currentTarget.getBoundingClientRect()
    const x = ((event.clientX - bounds.left) / bounds.width) * snapshot.viewportWidth
    const y = ((event.clientY - bounds.top) / bounds.height) * snapshot.viewportHeight
    void run(async () => {
      await safeInvoke('developer_browser_scroll', {
        request: {
          x,
          y,
          deltaX: event.deltaX,
          deltaY: event.deltaY,
        },
      })
    })
  }

  const typeText = () => run(async () => {
    if (!textInput) return
    await safeInvoke('developer_browser_type_text', { request: { text: textInput } })
    setTextInput('')
  })

  const keypress = (key: string) => run(async () => {
    await safeInvoke('developer_browser_keypress', { request: { key } })
  })

  const runCdpCommand = () => run(async () => {
    let params: unknown
    try {
      params = JSON.parse(cdpParams)
    } catch (cause) {
      throw new Error(
        `Invalid CDP parameter JSON: ${cause instanceof Error ? cause.message : String(cause)}`,
        { cause },
      )
    }
    const result = await safeInvoke<unknown>('developer_browser_cdp_call', {
      request: { method: cdpMethod, params },
    })
    setCdpResult(JSON.stringify(result, null, 2))
  }, false)

  const saveAnnotation = () => {
    if (!snapshot || !pendingElement || !annotationNote.trim()) return
    setAnnotations((current) => [...current, {
      id: crypto.randomUUID(),
      url: snapshot.url,
      selector: pendingElement.selector,
      element: pendingElement.tagName,
      note: annotationNote.trim(),
      x: pendingElement.x,
      y: pendingElement.y,
      width: pendingElement.width,
      height: pendingElement.height,
      createdAt: Date.now(),
    }])
    setPendingElement(null)
    setAnnotationNote('')
  }

  const copyPrompt = async () => {
    const prompt = buildAnnotationPrompt(snapshot, annotations)
    await navigator.clipboard.writeText(prompt)
    setCopyStatus('Copied')
    window.setTimeout(() => setCopyStatus(''), 1800)
  }

  return (
    <main className="developer-browser-view">
      <header className="developer-browser-header">
        <div className="developer-browser-session">
          <span className={session.active ? 'is-active' : ''}>
            <Circle size={9} fill="currentColor" aria-hidden="true" />
            {session.active ? `${session.browserName} · CDP ${session.debuggerPort}` : 'Stopped'}
          </span>
          {session.active ? (
            <button type="button" className="ui-button ui-button--secondary" onClick={stopBrowser} disabled={busy}>
              <Square size={14} aria-hidden="true" /> Stop
            </button>
          ) : (
            <button type="button" className="ui-button ui-button--primary" onClick={startBrowser} disabled={busy}>
              <Play size={14} aria-hidden="true" /> Start browser
            </button>
          )}
        </div>
      </header>

      <section className="developer-browser-toolbar" aria-label="Browser controls">
        <button type="button" onClick={() => history('back')} disabled={!session.active || busy} aria-label="Back">
          <ArrowLeft size={16} />
        </button>
        <button type="button" onClick={() => history('forward')} disabled={!session.active || busy} aria-label="Forward">
          <ArrowRight size={16} />
        </button>
        <button type="button" onClick={reload} disabled={!session.active || busy} aria-label="Reload">
          <RefreshCw size={16} className={busy ? 'spin' : ''} />
        </button>
        <form onSubmit={(event) => { event.preventDefault(); navigate() }}>
          <Globe2 size={16} aria-hidden="true" />
          <input value={url} onChange={(event) => setUrl(event.target.value)} aria-label="Browser URL" />
          <button type="submit" disabled={busy} aria-label="Navigate"><Send size={15} /></button>
        </form>
        <div className="developer-browser-mode-toggle">
          <button type="button" className={mode === 'interact' ? 'active' : ''} onClick={() => setMode('interact')}>
            <MousePointer2 size={15} /> Interact
          </button>
          <button type="button" className={mode === 'annotate' ? 'active' : ''} onClick={() => setMode('annotate')}>
            <Crosshair size={15} /> Annotate
          </button>
        </div>
      </section>

      {error && <div className="developer-browser-error" role="alert">{error}</div>}

      <div className="developer-browser-workspace">
        <section className="developer-browser-stage" aria-label="Rendered page">
          {snapshot ? (
            <>
              <div className="developer-browser-page-meta">
                <span><strong>{snapshot.title || 'Untitled'}</strong>{hostLabel(snapshot.url)}</span>
                <span>{Math.round(snapshot.viewportWidth)} × {Math.round(snapshot.viewportHeight)} · {snapshot.activeElement || 'no focus'}</span>
              </div>
              <div
                className={`developer-browser-screenshot mode-${mode}`}
                onClick={handleScreenshotClick}
                onWheel={handleScreenshotWheel}
                role="application"
                aria-label="Interactive browser preview"
              >
                <img src={snapshot.screenshotDataUrl} alt={`Rendered page: ${snapshot.title || snapshot.url}`} draggable={false} />
                {pageAnnotations.map((annotation, index) => (
                  <button
                    type="button"
                    key={annotation.id}
                    className="developer-browser-annotation-marker"
                    style={{
                      left: `${(annotation.x / snapshot.viewportWidth) * 100}%`,
                      top: `${(annotation.y / snapshot.viewportHeight) * 100}%`,
                      width: `${Math.max(1, (annotation.width / snapshot.viewportWidth) * 100)}%`,
                      height: `${Math.max(1, (annotation.height / snapshot.viewportHeight) * 100)}%`,
                    }}
                    title={annotation.note}
                    onClick={(event) => {
                      event.stopPropagation()
                      setInspectorTab('annotations')
                    }}
                  >
                    <span>{index + 1}</span>
                  </button>
                ))}
                {pendingElement && (
                  <div
                    className="developer-browser-pending-marker"
                    style={{
                      left: `${(pendingElement.x / snapshot.viewportWidth) * 100}%`,
                      top: `${(pendingElement.y / snapshot.viewportHeight) * 100}%`,
                      width: `${Math.max(1, (pendingElement.width / snapshot.viewportWidth) * 100)}%`,
                      height: `${Math.max(1, (pendingElement.height / snapshot.viewportHeight) * 100)}%`,
                    }}
                  />
                )}
              </div>
              <div className="developer-browser-input-bar">
                <input
                  value={textInput}
                  onChange={(event) => setTextInput(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === 'Enter') {
                      event.preventDefault()
                      void typeText()
                    }
                  }}
                  placeholder="Type into the focused page element"
                  aria-label="Text to type into page"
                />
                <button type="button" onClick={typeText} disabled={!textInput || busy}>Type</button>
                <button type="button" onClick={() => keypress('Enter')} disabled={busy}>Enter</button>
                <button type="button" onClick={() => keypress('Tab')} disabled={busy}>Tab</button>
                <button type="button" onClick={() => keypress('Escape')} disabled={busy}>Esc</button>
              </div>
            </>
          ) : (
            <div className="developer-browser-empty">
              {busy ? <LoaderCircle size={30} className="spin" /> : <Globe2 size={34} />}
              <h2>{busy ? 'Starting Chromium…' : 'Start the developer browser'}</h2>
              <p>Open a localhost route or any HTTP(S) page without sharing your normal browser profile.</p>
              {!busy && (
                <button type="button" className="ui-button ui-button--primary" onClick={startBrowser}>
                  <Play size={14} /> Start and open URL
                </button>
              )}
            </div>
          )}
        </section>

        <aside className="developer-browser-inspector" aria-label="Developer tools">
          <div className="developer-browser-inspector-tabs" role="tablist">
            <button type="button" role="tab" aria-selected={inspectorTab === 'annotations'} className={inspectorTab === 'annotations' ? 'active' : ''} onClick={() => setInspectorTab('annotations')}>
              <MessageSquarePlus size={14} /> Notes <span>{pageAnnotations.length}</span>
            </button>
            <button type="button" role="tab" aria-selected={inspectorTab === 'console'} className={inspectorTab === 'console' ? 'active' : ''} onClick={() => setInspectorTab('console')}>
              <SquareTerminal size={14} /> Console <span>{snapshot?.consoleEntries.length ?? 0}</span>
            </button>
            <button type="button" role="tab" aria-selected={inspectorTab === 'network'} className={inspectorTab === 'network' ? 'active' : ''} onClick={() => setInspectorTab('network')}>
              <Network size={14} /> Network <span>{snapshot?.networkEntries.length ?? 0}</span>
            </button>
            <button type="button" role="tab" aria-selected={inspectorTab === 'dom'} className={inspectorTab === 'dom' ? 'active' : ''} onClick={() => setInspectorTab('dom')}>
              <Braces size={14} /> DOM
            </button>
            <button type="button" role="tab" aria-selected={inspectorTab === 'cdp'} className={inspectorTab === 'cdp' ? 'active' : ''} onClick={() => setInspectorTab('cdp')}>
              <Code2 size={14} /> CDP
            </button>
          </div>

          <div className="developer-browser-inspector-body">
            {inspectorTab === 'annotations' && (
              <div className="developer-browser-annotations">
                {pendingElement && (
                  <div className="developer-browser-note-editor">
                    <span className="developer-browser-selector">{pendingElement.selector}</span>
                    <small>{pendingElement.text || `<${pendingElement.tagName}>`}</small>
                    <textarea
                      value={annotationNote}
                      onChange={(event) => setAnnotationNote(event.target.value)}
                      placeholder="Describe the expected change for this element…"
                      autoFocus
                    />
                    <div>
                      <button type="button" onClick={() => setPendingElement(null)}>Cancel</button>
                      <button type="button" className="primary" onClick={saveAnnotation} disabled={!annotationNote.trim()}>Add comment</button>
                    </div>
                  </div>
                )}
                {pageAnnotations.length === 0 && !pendingElement && (
                  <div className="developer-browser-panel-empty">
                    <Crosshair size={24} />
                    <strong>No page comments</strong>
                    <span>Switch to Annotate and select a rendered element.</span>
                  </div>
                )}
                {pageAnnotations.map((annotation, index) => (
                  <article key={annotation.id}>
                    <span className="developer-browser-note-number">{index + 1}</span>
                    <div>
                      <code>{annotation.selector}</code>
                      <p>{annotation.note}</p>
                    </div>
                    <button
                      type="button"
                      aria-label={`Delete annotation ${index + 1}`}
                      onClick={() => setAnnotations((current) => current.filter((item) => item.id !== annotation.id))}
                    >
                      <Trash2 size={14} />
                    </button>
                  </article>
                ))}
                {pageAnnotations.length > 0 && (
                  <button type="button" className="developer-browser-copy-prompt" onClick={copyPrompt}>
                    <Copy size={14} /> {copyStatus || 'Copy comments as Cowork prompt'}
                  </button>
                )}
              </div>
            )}

            {inspectorTab === 'console' && (
              <div className="developer-browser-console">
                {(snapshot?.consoleEntries ?? []).map((entry, index) => (
                  <div key={`${entry.timestamp}-${index}`} className={`level-${entry.level || 'log'}`}>
                    <span>{entry.level || 'log'}</span>
                    <code>{entry.message}</code>
                  </div>
                ))}
                {(snapshot?.consoleEntries.length ?? 0) === 0 && (
                  <div className="developer-browser-panel-empty"><SquareTerminal size={24} /><strong>Console is quiet</strong></div>
                )}
              </div>
            )}

            {inspectorTab === 'network' && (
              <div className="developer-browser-network">
                <div className="developer-browser-network-head">
                  <span>Request</span><span>Status</span><span>Type</span><span>Time</span><span>Size</span>
                </div>
                {(snapshot?.networkEntries ?? []).map((entry, index) => (
                  <div key={`${entry.url}-${entry.timestamp}-${index}`}>
                    <span title={entry.url}>{entry.method || 'GET'} {entry.url}</span>
                    <span className={entry.status >= 400 ? 'is-error' : ''}>{entry.status || '—'}</span>
                    <span>{entry.kind || 'resource'}</span>
                    <span>{Math.round(entry.durationMs || 0)} ms</span>
                    <span>{formatBytes(entry.transferSize)}</span>
                  </div>
                ))}
                {(snapshot?.networkEntries.length ?? 0) === 0 && (
                  <div className="developer-browser-panel-empty"><Network size={24} /><strong>No requests captured</strong></div>
                )}
              </div>
            )}

            {inspectorTab === 'dom' && (
              <div className="developer-browser-dom">
                <div>
                  <Code2 size={15} /> DOM snapshot
                  {snapshot?.url && (
                    <a href={snapshot.url} target="_blank" rel="noreferrer"><ExternalLink size={13} /> Open externally</a>
                  )}
                </div>
                <pre>{snapshot?.dom || 'No DOM snapshot available.'}</pre>
              </div>
            )}

            {inspectorTab === 'cdp' && (
              <form className="developer-browser-cdp" onSubmit={(event) => { event.preventDefault(); void runCdpCommand() }}>
                <div>
                  <strong>Chrome DevTools Protocol</strong>
                  <small>Allowed domains: Runtime, DOM, CSS, Network, Page, Performance, Accessibility, Emulation, Input, Log, and Overlay.</small>
                </div>
                <label>
                  Method
                  <input value={cdpMethod} onChange={(event) => setCdpMethod(event.target.value)} placeholder="Runtime.evaluate" />
                </label>
                <label>
                  Parameters (JSON)
                  <textarea value={cdpParams} onChange={(event) => setCdpParams(event.target.value)} rows={9} spellCheck={false} />
                </label>
                <button type="submit" disabled={!session.active || !cdpMethod.trim() || busy}>
                  <Play size={14} /> Run CDP command
                </button>
                <pre>{cdpResult}</pre>
              </form>
            )}
          </div>
        </aside>
      </div>
    </main>
  )
}
