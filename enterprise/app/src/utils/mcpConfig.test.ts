import { describe, expect, it } from 'vitest'
import { mcpWorkingDirectoryRequired, parseMcpConfiguration } from './mcpConfig'

describe('MCP import working directory', () => {
  const raw = JSON.stringify({ mcpServers: { fixture: { command: 'node', args: ['scripts/server.mjs', 'a b'], env: { TOKEN: 'sentinel' } } } })
  it('uses the selected JSON parent only for a file import', () => {
    expect(parseMcpConfiguration(raw, 'C:\\Fixture')[0]).toMatchObject({ cwd: 'C:\\Fixture', command: 'node', args: 'scripts/server.mjs "a b"' })
    expect(parseMcpConfiguration(raw)[0].cwd).toBeUndefined()
    expect(mcpWorkingDirectoryRequired(parseMcpConfiguration(raw)[0])).toBe(true)
    expect(mcpWorkingDirectoryRequired(parseMcpConfiguration(raw, 'C:\\Fixture')[0])).toBe(false)
  })
  it('preserves explicit cwd and builtin identity; skips HTTP', () => {
    const parsed = parseMcpConfiguration(JSON.stringify({ mcpServers: {
      builtin: { kind: 'builtin', builtinId: 'duckduckgo-websearch' },
      custom: { command: 'node', cwd: 'D:\\Explicit' },
      remote: { type: 'http', url: 'https://example.test' },
    } }), 'C:\\Imported')
    expect(parsed).toHaveLength(2)
    expect(parsed[0]).toMatchObject({ kind: 'builtin', builtinId: 'duckduckgo-websearch' })
    expect(mcpWorkingDirectoryRequired(parsed[0])).toBe(false)
    expect(parsed[1].cwd).toBe('D:\\Explicit')
  })
})
