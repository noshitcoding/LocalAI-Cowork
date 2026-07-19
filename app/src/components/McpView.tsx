import { useEffect, useState } from 'react'
import { RefreshCw, Square } from 'lucide-react'
import { useConfigStore } from '../stores/configStore'
import type { McpServerConfig } from '../stores/configStore'
import { safeInvoke } from '../utils/safeInvoke'
import { tr } from '../i18n'

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

const SCREENSHOT_MCP_COMMAND = 'localai-cowork-screenshot-mcp'

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
    throw new Error('No MCP server found')
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
    throw new Error('No stdio MCP server with command found')
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
    setMcpServerEnv,
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
  const isscreenshotServer = mcpServer.command.trim().toLowerCase() === SCREENSHOT_MCP_COMMAND
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
      await setMcpServerEnv(env)
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
      await setMcpServerEnv(env)
      await safeInvoke<McpRuntimeServerStatus>('mcp_runtime_start', {
        request: {
          name: mcpServer.name,
          command: mcpServer.command,
          args: splitArgs(mcpServer.args),
          env,
        },
      })
      await refreshRuntimeServers()
      setNotice(`${tr("Runtime server started:")} ${mcpServer.name}`)
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
      setNotice(stopped ? `${tr("Runtime server stopped:")} ${mcpServer.name}` : `${tr("Runtime server was not active:")} ${mcpServer.name}`)
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
      await setMcpServerEnv(env)
      await safeInvoke<McpRuntimeServerStatus>('mcp_runtime_restart', {
        request: {
          name: mcpServer.name,
          command: mcpServer.command,
          args: splitArgs(mcpServer.args),
          env,
        },
      })
      await refreshRuntimeServers()
      setNotice(`${tr("Runtime server restarted:")} ${mcpServer.name}`)
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
      await setMcpServerEnv(env)

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
      await setMcpServerEnv(env)

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

  const handleImport = async () => {
    setError(null)
    setNotice(null)
    try {
      const imported = parseMcpJson(importText)
      await importMcpServers(imported)
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
      <h1>{tr("MCP Server")}</h1>
      <p className="hint-text">{tr("Manage and test Model Context Protocol servers")}</p>

      {preferences.mcpAllowManualImport && (
        <div className="panel">
          <h2>{tr("Import Claude Code JSON")}</h2>
          <p className="hint-text">{tr("Supports .mcp.json and Claude Desktop format with mcpServers. HTTP/SSE servers are ignored.")}</p>
          <textarea
            className="json-import"
            rows={6}
            value={importText}
            onChange={(e) => setImportText(e.target.value)}
            placeholder={exampleJson()}
          />
          <div className="actions">
            <button type="button" disabled={!importText.trim()} onClick={() => void handleImport()}>{tr("Import JSON")}</button>
            <button type="button" className="btn-secondary" onClick={() => setImportText(exampleJson())}>{tr("Insert example")}</button>
          </div>
        </div>
      )}

      <div className="panel">
        <div className="panel-heading-row">
          <h2>{tr("Server configuration")}</h2>
          <span className={`status-badge ${status}`}>
            {status === 'online' ? tr("Online") : status === 'offline' ? tr("Offline") : tr("Unknown")}
          </span>
        </div>
        <div className="grid">
          <label>{tr("Active server")}<select value={activeMcpServerName} onChange={(e) => selectServer(e.target.value)}>
              {servers.map((s) => <option key={s.name} value={s.name}>{s.name}</option>)}
            </select>
          </label>
          <label>{tr("Name")}<input value={mcpServer.name} onChange={(e) => setMcpServer({ name: e.target.value })} />
          </label>
          <label>{tr("Command")}<input className="mcp-mono-input" value={mcpServer.command} onChange={(e) => setMcpServer({ command: e.target.value })} />
          </label>
          <label>{tr("Args")}<input className="mcp-mono-input" value={mcpServer.args} onChange={(e) => setMcpServer({ args: e.target.value })} />
          </label>
        </div>
        <label className="mcp-env-field">{tr("Env JSON")}<textarea
            className="json-import"
            rows={3}
            value={envText}
            onChange={(e) => setEnvText(e.target.value)}
            placeholder={tr("{\"API_KEY\":\"...\"}")}
            disabled={!preferences.mcpEnvEditorEnabled}
          />
        </label>
        <div className="actions">
          <button disabled={busy} onClick={handleProbe}>
            {busy ? tr('Probe running...') : tr('Connect server and load tools')}
          </button>
          <button type="button" className="btn-secondary" disabled={servers.length <= 1} onClick={() => void deleteMcpServer(mcpServer.name)}>{tr("Delete server")}</button>
        </div>

        <div className="actions mcp-runtime-actions">
          <button type="button" disabled={busy || runtimeActive} onClick={handleRuntimeStart}>
            {runtimeActive ? tr('Runtime active') : tr('Start runtime')}
          </button>
          <button type="button" className="btn-secondary" disabled={busy || !runtimeActive} onClick={handleRuntimeStop}>
            <Square size={15} aria-hidden="true" />
            {tr("Stop runtime")}
          </button>
          <button type="button" className="btn-secondary" disabled={busy || !runtimeActive} onClick={handleRuntimeRestart}>
            <RefreshCw size={15} aria-hidden="true" />
            {tr("Restart runtime")}
          </button>
          <button type="button" className="btn-secondary" disabled={busy} onClick={() => void refreshRuntimeServers()}>
            <RefreshCw size={15} aria-hidden="true" />
            {tr("Refresh runtime")}
          </button>
        </div>
      </div>

      <div className="panel">
        <h2>{tr("Runtime servers")} ({runtimeServers.length})</h2>
        {runtimeServers.length === 0 ? (
          <p className="hint-text">{tr("No persistent MCP servers are currently started.")}</p>
        ) : (
          <ul className="tool-list">
            {runtimeServers.map((server) => (
              <li key={server.name} className="tool-item">
                <strong>{server.name}</strong>
                <span className="mcp-runtime-meta">{tr("pid=")}{server.pid ?? '-'} / {tr("started")} {new Date(server.startedAt).toLocaleString('de-DE')}
                </span>
                <div className="mcp-command-line">
                  {server.command} {server.args.join(' ')}
                </div>
                {server.lastError && (
                  <div className="error mcp-runtime-error">{server.lastError}</div>
                )}
              </li>
            ))}
          </ul>
        )}
      </div>

      {notice && <p className="success">{notice}</p>}
      {error && <p className="error">{error}</p>}

      {isscreenshotServer && (
        <div className="panel">
          <h2>{tr("Screenshot quick start")}</h2>
          <p className="hint-text">{tr("Captures all screens by default and saves PNG files in the app data folder.")}</p>
          <div className="actions">
            <button type="button" disabled={busy} onClick={() => quickMcpCall('capture_screenshot', { allscreens: true })}>
              {busy ? tr('Capture running...') : tr('Capture all screens')}
            </button>
            <button type="button" className="btn-secondary" disabled={busy} onClick={() => quickMcpCall('list_screens', {})}>{tr("Show screens")}</button>
          </div>
        </div>
      )}

      <div className="panel">
        <h2>{tr("Imported servers")} ({servers.length})</h2>
        <ul className="tool-list">
          {servers.map((s) => (
            <li key={s.name} className="tool-item">
              <strong>{s.name}</strong> - <span className="mcp-command-inline">{s.command} {s.args}</span>
            </li>
          ))}
        </ul>
      </div>

      {probe && (
        <>
          <div className="panel">
            <h2>{tr("Server info")}</h2>
            <div className="card">
              <p><strong>{tr("Server:")}</strong> {probe.serverName}</p>
              <p><strong>{tr("Protocol:")}</strong> {probe.protocolVersion ?? '-'}</p>
              <p><strong>{tr("Info:")}</strong> {probe.serverInfo ?? '-'}</p>
            </div>
          </div>

          <div className="panel">
            <h2>{tr("Available tools")} ({probe.tools.length})</h2>
            <ul className="tool-list">
              {probe.tools.map((tool) => (
                <li key={tool.name} className="tool-item">
                  <strong>{tool.name}</strong>
                  {tool.description && <span> - {tool.description}</span>}
                </li>
              ))}
            </ul>
          </div>

          {probe.tools.length > 0 && (
            <div className="panel tool-execute">
              <h2>{tr("Execute tool")}</h2>
              <div className="grid">
                <label>{tr("Tool")}<select value={selectedTool} onChange={(e) => setSelectedTool(e.target.value)}>
                    {probe.tools.map((t) => <option key={t.name} value={t.name}>{t.name}</option>)}
                  </select>
                </label>
                <label>{tr("Argumente (JSON)")}<textarea rows={3} value={toolArgs} onChange={(e) => setToolArgs(e.target.value)} placeholder={tr("{\"path\": \".\"}")} />
                </label>
              </div>
              <div className="actions">
                <button disabled={busy || !selectedTool} onClick={handleToolCall}>
                  {busy ? tr('Call running...') : tr('Call tool')}
                </button>
              </div>
            </div>
          )}
        </>
      )}

      {callResult && (
        <div className="panel">
          <h2>{tr("Tool result")}</h2>
          <div className="card">
            <p><strong>{tr("Tool:")}</strong> {callResult.toolName}</p>
            <p><strong>{tr("Success:")}</strong> {callResult.success ? tr("yes") : tr("no")}</p>
          </div>
          {callResult.error && <p className="error mcp-call-error">{callResult.error}</p>}
          <pre className="tool-result">{callResult.result}</pre>
        </div>
      )}
    </div>
  )
}
