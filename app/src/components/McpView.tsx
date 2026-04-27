import { useEffect, useState } from 'react'
import { useConfigStore } from '../stores/configStore'
import type { McpServerConfig } from '../stores/configStore'
import { safeInvoke } from '../utils/safeInvoke'

type McpTool = {
  name: string
  description: string
}

type McpProbeResponse = {
  serverName: string
  protocolVersion: string | null
  serverInfo: string | null
  tools: McpTool[]
}

type McpCallResponse = {
  serverName: string
  toolName: string
  success: boolean
  result: string
  error: string | null
}

type McpRuntimeServerStatus = {
  name: string
  command: string
  args: string[]
  pid: number | null
  startedAt: string
  lastError: string | null
}

const SCREENSHOT_MCP_COMMAND = 'open-cowork-screenshot-mcp'

type ClaudeMcpServer = {
  type?: string
  command?: string
  args?: string[] | string
  env?: Record<string, string>
}

function splitArgs(value: string): string[] {
  const matches = value.match(/"[^"]*"|'[^']*'|\S+/g) ?? []
  return matches.map((part) => part.replace(/^["']|["']$/g, ''))
}

function parseEnv(value: string): Record<string, string> {
  if (!value.trim()) return {}
  const parsed = JSON.parse(value) as unknown
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
    throw new Error('Env muss ein JSON-Objekt sein')
  }

  return Object.fromEntries(
    Object.entries(parsed).map(([key, entry]) => [key, String(entry)]),
  )
}

function parseMcpJson(raw: string): McpServerConfig[] {
  const parsed = JSON.parse(raw) as unknown
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
    throw new Error('JSON muss ein Objekt sein')
  }

  const root = parsed as Record<string, unknown>
  const candidate = root.mcpServers ?? root.servers ?? root
  if (!candidate || typeof candidate !== 'object' || Array.isArray(candidate)) {
    throw new Error('Keine MCP-Server gefunden')
  }

  const servers: McpServerConfig[] = []
  for (const [name, value] of Object.entries(candidate as Record<string, unknown>)) {
    if (!value || typeof value !== 'object' || Array.isArray(value)) continue
    const server = value as ClaudeMcpServer
    if (server.type && server.type !== 'stdio') continue
    if (!server.command) continue
    servers.push({
      name,
      command: server.command,
      args: Array.isArray(server.args) ? server.args.join(' ') : (server.args ?? ''),
      env: server.env ?? {},
    })
  }

  if (servers.length === 0) {
    throw new Error('Keine stdio-MCP-Server mit command gefunden')
  }

  return servers
}

function exampleJson(): string {
  return JSON.stringify(
    {
      mcpServers: {
        filesystem: {
          command: 'npx',
          args: ['-y', '@modelcontextprotocol/server-filesystem', '.'],
          env: {},
        },
      },
    },
    null,
    2,
  )
}

export default function McpView() {
  const {
    mcpServer,
    mcpServers,
    activeMcpServerName,
    preferences,
    setMcpServer,
    setActiveMcpServer,
    importMcpServers,
    deleteMcpServer,
  } = useConfigStore()
  const servers = mcpServers.length > 0 ? mcpServers : [mcpServer]
  const [probe, setProbe] = useState<McpProbeResponse | null>(null)
  const [callResult, setCallResult] = useState<McpCallResponse | null>(null)
  const [selectedTool, setSelectedTool] = useState<string>('')
  const [toolArgs, setToolArgs] = useState<string>('{}')
  const [envText, setEnvText] = useState<string>(() => JSON.stringify(mcpServer.env ?? {}, null, 2))
  const [importText, setImportText] = useState<string>('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [notice, setNotice] = useState<string | null>(null)
  const [status, setStatus] = useState<'unknown' | 'online' | 'offline'>('unknown')
  const [runtimeServers, setRuntimeServers] = useState<McpRuntimeServerStatus[]>([])
  const isScreenshotServer = mcpServer.command.trim().toLowerCase() === SCREENSHOT_MCP_COMMAND
  const runtimeActive = runtimeServers.some((server) => server.name === mcpServer.name)

  const refreshRuntimeServers = async () => {
    const rows = await safeInvoke<McpRuntimeServerStatus[]>('mcp_runtime_list', undefined, [])
    setRuntimeServers(rows)
  }

  useEffect(() => {
    void refreshRuntimeServers()
  }, [])

  const selectServer = (name: string) => {
    setActiveMcpServer(name)
    const next = servers.find((server) => server.name === name)
    setEnvText(JSON.stringify(next?.env ?? {}, null, 2))
    setProbe(null)
    setCallResult(null)
    setSelectedTool('')
    setError(null)
  }

  const currentEnv = () => parseEnv(envText)

  const handleProbe = async () => {
    setBusy(true)
    setError(null)
    setNotice(null)
    try {
      const env = currentEnv()
      setMcpServer({ env })
      const response = await safeInvoke<McpProbeResponse>('mcp_probe', {
        request: {
          name: mcpServer.name,
          command: mcpServer.command,
          args: splitArgs(mcpServer.args),
          env,
        },
      })
      setProbe(response)
      setStatus('online')
      if (response.tools.length > 0) {
        setSelectedTool(response.tools[0].name)
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
      setProbe(null)
      setStatus('offline')
    } finally {
      setBusy(false)
    }
  }

  const handleRuntimeStart = async () => {
    setBusy(true)
    setError(null)
    setNotice(null)
    try {
      const env = currentEnv()
      setMcpServer({ env })
      await safeInvoke<McpRuntimeServerStatus>('mcp_runtime_start', {
        request: {
          name: mcpServer.name,
          command: mcpServer.command,
          args: splitArgs(mcpServer.args),
          env,
        },
      })
      await refreshRuntimeServers()
      setNotice(`Runtime-Server gestartet: ${mcpServer.name}`)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  const handleRuntimeStop = async () => {
    setBusy(true)
    setError(null)
    setNotice(null)
    try {
      const stopped = await safeInvoke<boolean>('mcp_runtime_stop', { name: mcpServer.name }, false)
      await refreshRuntimeServers()
      setNotice(stopped ? `Runtime-Server gestoppt: ${mcpServer.name}` : `Runtime-Server war nicht aktiv: ${mcpServer.name}`)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  const handleRuntimeRestart = async () => {
    setBusy(true)
    setError(null)
    setNotice(null)
    try {
      const env = currentEnv()
      setMcpServer({ env })
      await safeInvoke<McpRuntimeServerStatus>('mcp_runtime_restart', {
        request: {
          name: mcpServer.name,
          command: mcpServer.command,
          args: splitArgs(mcpServer.args),
          env,
        },
      })
      await refreshRuntimeServers()
      setNotice(`Runtime-Server neugestartet: ${mcpServer.name}`)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  const handleToolCall = async () => {
    if (!selectedTool) return
    setBusy(true)
    setError(null)
    setCallResult(null)
    try {
      const parsedArgs = JSON.parse(toolArgs) as Record<string, unknown>
      const env = currentEnv()
      setMcpServer({ env })

      const response = await safeInvoke<McpCallResponse>('mcp_call_tool', {
        request: {
          name: mcpServer.name,
          command: mcpServer.command,
          args: splitArgs(mcpServer.args),
          env,
          toolName: selectedTool,
          toolArgs: parsedArgs,
        },
      })
      setCallResult(response)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  const quickMcpCall = async (toolName: string, toolArgs: Record<string, unknown>) => {
    setBusy(true)
    setError(null)
    setCallResult(null)
    try {
      const env = currentEnv()
      setMcpServer({ env })

      const response = await safeInvoke<McpCallResponse>('mcp_call_tool', {
        request: {
          name: mcpServer.name,
          command: mcpServer.command,
          args: splitArgs(mcpServer.args),
          env,
          toolName,
          toolArgs,
        },
      })
      setCallResult(response)
      if (response.success) {
        setStatus('online')
      }
    } catch (err) {
      setStatus('offline')
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  const handleImport = () => {
    setError(null)
    setNotice(null)
    try {
      const imported = parseMcpJson(importText)
      importMcpServers(imported)
      setEnvText(JSON.stringify(imported[0]?.env ?? {}, null, 2))
      setNotice(`${imported.length} MCP-Server importiert`)
      setImportText('')
      setProbe(null)
      setCallResult(null)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  return (
    <div className="mcp-view">
      <h1>MCP Server</h1>
      <p className="hint-text">Model Context Protocol Server verwalten und testen</p>

      {/* ── Import ─────────────── */}
      {preferences.mcpAllowManualImport && (
        <div className="panel">
          <h2>📥 Claude-Code-JSON importieren</h2>
          <p className="hint-text">Unterstützt .mcp.json und Claude-Desktop-Format mit mcpServers. HTTP/SSE-Server werden ignoriert.</p>
          <textarea
            className="json-import"
            rows={6}
            value={importText}
            onChange={(e) => setImportText(e.target.value)}
            placeholder={exampleJson()}
          />
          <div className="actions">
            <button type="button" disabled={!importText.trim()} onClick={handleImport}>JSON importieren</button>
            <button type="button" className="btn-secondary" onClick={() => setImportText(exampleJson())}>Beispiel einfügen</button>
          </div>
        </div>
      )}

      {/* ── Server Config ──────── */}
      <div className="panel">
        <div className="panel-heading-row">
          <h2>⚙️ Server-Konfiguration</h2>
          <span className={`status-badge ${status}`}>
            {status === 'online' ? '● Online' : status === 'offline' ? '● Offline' : '● Unbekannt'}
          </span>
        </div>
        <div className="grid">
          <label>
            Aktiver Server
            <select value={activeMcpServerName} onChange={(e) => selectServer(e.target.value)}>
              {servers.map((s) => <option key={s.name} value={s.name}>{s.name}</option>)}
            </select>
          </label>
          <label>
            Name
            <input value={mcpServer.name} onChange={(e) => setMcpServer({ name: e.target.value })} />
          </label>
          <label>
            Command
            <input value={mcpServer.command} onChange={(e) => setMcpServer({ command: e.target.value })} style={{ fontFamily: 'monospace' }} />
          </label>
          <label>
            Args
            <input value={mcpServer.args} onChange={(e) => setMcpServer({ args: e.target.value })} style={{ fontFamily: 'monospace' }} />
          </label>
        </div>
        <label style={{ display: 'flex', flexDirection: 'column', gap: 4, fontSize: 13, fontWeight: 500, color: 'var(--text-secondary)', marginBottom: 12 }}>
          Env JSON
          <textarea
            className="json-import"
            rows={3}
            value={envText}
            onChange={(e) => setEnvText(e.target.value)}
            placeholder='{"API_KEY":"..."}'
            disabled={!preferences.mcpEnvEditorEnabled}
          />
        </label>
        <div className="actions">
          <button disabled={busy} onClick={handleProbe}>
            {busy ? '⏳ Probe läuft...' : '🔌 Server verbinden & Tools laden'}
          </button>
          <button type="button" className="btn-secondary" disabled={servers.length <= 1} onClick={() => deleteMcpServer(mcpServer.name)}>
            Server löschen
          </button>
        </div>

        <div className="actions" style={{ marginTop: 10 }}>
          <button type="button" disabled={busy || runtimeActive} onClick={handleRuntimeStart}>
            {runtimeActive ? 'Runtime aktiv' : '▶ Runtime starten'}
          </button>
          <button type="button" className="btn-secondary" disabled={busy || !runtimeActive} onClick={handleRuntimeStop}>
            ■ Runtime stoppen
          </button>
          <button type="button" className="btn-secondary" disabled={busy || !runtimeActive} onClick={handleRuntimeRestart}>
            ↻ Runtime neu starten
          </button>
          <button type="button" className="btn-secondary" disabled={busy} onClick={() => void refreshRuntimeServers()}>
            Runtime aktualisieren
          </button>
        </div>
      </div>

      <div className="panel">
        <h2>🧠 Runtime-Server ({runtimeServers.length})</h2>
        {runtimeServers.length === 0 ? (
          <p className="hint-text">Aktuell sind keine persistenten MCP-Server gestartet.</p>
        ) : (
          <ul className="tool-list">
            {runtimeServers.map((server) => (
              <li key={server.name} className="tool-item">
                <strong>{server.name}</strong>
                <span style={{ marginLeft: 8, fontSize: 12 }}>
                  pid={server.pid ?? '—'} · gestartet {new Date(server.startedAt).toLocaleString('de-DE')}
                </span>
                <div style={{ fontFamily: 'monospace', fontSize: 12, marginTop: 4 }}>
                  {server.command} {server.args.join(' ')}
                </div>
                {server.lastError && (
                  <div className="error" style={{ marginTop: 4 }}>{server.lastError}</div>
                )}
              </li>
            ))}
          </ul>
        )}
      </div>

      {/* ── Notices ─────────────── */}
      {notice && <p className="success">{notice}</p>}
      {error && <p className="error">{error}</p>}

      {/* ── Screenshot Quick Actions ─────────────── */}
      {isScreenshotServer && (
        <div className="panel">
          <h2>📸 Screenshot Schnellstart</h2>
          <p className="hint-text">Nimmt standardmäßig alle Bildschirme auf und speichert PNG-Dateien im App-Datenordner.</p>
          <div className="actions">
            <button type="button" disabled={busy} onClick={() => quickMcpCall('capture_screenshot', { allScreens: true })}>
              {busy ? '⏳ Aufnahme läuft...' : 'Alle Bildschirme aufnehmen'}
            </button>
            <button type="button" className="btn-secondary" disabled={busy} onClick={() => quickMcpCall('list_screens', {})}>
              Bildschirme anzeigen
            </button>
          </div>
        </div>
      )}

      {/* ── Server List ────────── */}
      <div className="panel">
        <h2>📋 Importierte Server ({servers.length})</h2>
        <ul className="tool-list">
          {servers.map((s) => (
            <li key={s.name} className="tool-item">
              <strong>{s.name}</strong> — <span style={{ fontFamily: 'monospace', fontSize: 12 }}>{s.command} {s.args}</span>
            </li>
          ))}
        </ul>
      </div>

      {/* ── Probe Results ──────── */}
      {probe && (
        <>
          <div className="panel">
            <h2>ℹ️ Server-Info</h2>
            <div className="card">
              <p><strong>Server:</strong> {probe.serverName}</p>
              <p><strong>Protocol:</strong> {probe.protocolVersion ?? '—'}</p>
              <p><strong>Info:</strong> {probe.serverInfo ?? '—'}</p>
            </div>
          </div>

          <div className="panel">
            <h2>🔧 Verfügbare Tools ({probe.tools.length})</h2>
            <ul className="tool-list">
              {probe.tools.map((tool) => (
                <li key={tool.name} className="tool-item">
                  <strong>{tool.name}</strong>
                  {tool.description && <span> — {tool.description}</span>}
                </li>
              ))}
            </ul>
          </div>

          {probe.tools.length > 0 && (
            <div className="panel tool-execute">
              <h2>▶️ Tool ausführen</h2>
              <div className="grid">
                <label>
                  Tool
                  <select value={selectedTool} onChange={(e) => setSelectedTool(e.target.value)}>
                    {probe.tools.map((t) => <option key={t.name} value={t.name}>{t.name}</option>)}
                  </select>
                </label>
                <label>
                  Argumente (JSON)
                  <textarea rows={3} value={toolArgs} onChange={(e) => setToolArgs(e.target.value)} placeholder='{"path": "."}' />
                </label>
              </div>
              <div className="actions">
                <button disabled={busy || !selectedTool} onClick={handleToolCall}>
                  {busy ? '⏳ Aufruf läuft...' : 'Tool aufrufen'}
                </button>
              </div>
            </div>
          )}
        </>
      )}

      {/* ── Tool Result ────────── */}
      {callResult && (
        <div className="panel">
          <h2>📤 Tool-Ergebnis</h2>
          <div className="card">
            <p><strong>Tool:</strong> {callResult.toolName}</p>
            <p><strong>Erfolg:</strong> {callResult.success ? '✓ Ja' : '✗ Nein'}</p>
          </div>
          {callResult.error && <p className="error" style={{ marginTop: 8 }}>{callResult.error}</p>}
          <pre className="tool-result">{callResult.result}</pre>
        </div>
      )}
    </div>
  )
}
