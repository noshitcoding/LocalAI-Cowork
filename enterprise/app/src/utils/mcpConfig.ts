import type { McpServerConfig } from '../stores/configStore'

export function mcpWorkingDirectoryRequired(server: McpServerConfig): boolean {
  if (server.kind === 'builtin' || (server.command.trim() === 'node' && server.args.trim() === 'scripts/mcp/duckduckgo-websearch-server.mjs')) return false
  const absolute = (value: string) => /^(?:[a-z]:[\\/]|\\\\|\/)/i.test(value)
  if (server.cwd?.trim()) return !absolute(server.cwd.trim())
  const args = (server.args.match(/"[^"]*"|'[^']*'|\S+/g) ?? []).map((part) => part.replace(/^["']|["']$/g, ''))
  return [server.command, ...args].some((value) => !absolute(value) && !value.startsWith('-') && !value.startsWith('@') && !value.includes('://') && (/[\\/]/.test(value) || /\.(?:[cm]?js|py)$/.test(value) || value === '.' || value === '..'))
}

/** Import data only. Never infer the ambient application working directory. */
export function parseMcpConfiguration(raw: string, fileDirectory?: string): McpServerConfig[] {
  const parsed: unknown = JSON.parse(raw)
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) throw new Error('JSON must be an object')
  const root = parsed as Record<string, unknown>
  const candidate = root.mcpServers ?? root.servers ?? root
  if (!candidate || typeof candidate !== 'object' || Array.isArray(candidate)) throw new Error('No MCP server found')
  const servers: McpServerConfig[] = []
  for (const [name, entry] of Object.entries(candidate)) {
    if (!entry || typeof entry !== 'object' || Array.isArray(entry)) continue
    const value = entry as Record<string, unknown>
    if (value.type && value.type !== 'stdio') continue
    const builtin = value.kind === 'builtin' && value.builtinId === 'duckduckgo-websearch'
    if (!builtin && typeof value.command !== 'string') continue
    const args = Array.isArray(value.args)
      ? value.args.map(String).map((arg) => /\s/.test(arg) ? JSON.stringify(arg) : arg).join(' ')
      : typeof value.args === 'string' ? value.args : ''
    servers.push({
      name,
      kind: builtin ? 'builtin' : 'stdio',
      builtinId: builtin ? 'duckduckgo-websearch' : undefined,
      command: builtin ? 'builtin:duckduckgo-websearch' : String(value.command),
      args,
      cwd: typeof value.cwd === 'string' && value.cwd.trim() ? value.cwd.trim() : fileDirectory,
      env: value.env && typeof value.env === 'object' && !Array.isArray(value.env)
        ? Object.fromEntries(Object.entries(value.env).map(([key, val]) => [key, String(val)])) : {},
    })
  }
  if (!servers.length) throw new Error('No stdio MCP server with command found')
  return servers
}
