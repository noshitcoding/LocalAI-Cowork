import { afterEach, describe, expect, it, vi } from 'vitest'
import { detectServerKind } from './serverConnection'

describe('server connection discovery', () => {
  afterEach(() => vi.unstubAllGlobals())
  it('recognises an assurance challenge without accepting unrelated forbidden responses', async () => {
    const fetch=vi.fn().mockResolvedValueOnce(new Response(null,{status:404}))
      .mockResolvedValueOnce(new Response(JSON.stringify({ error:'mfa_required' }),{status:403}))
      .mockResolvedValueOnce(new Response(null,{status:404})).mockResolvedValueOnce(new Response('{}',{status:403}))
    vi.stubGlobal('fetch',fetch)
    expect(await detectServerKind('https://management.example',async()=>'secret')).toBe('management')
    await expect(detectServerKind('https://management.example',async()=>'secret')).rejects.toThrow()
  })
  it('recognises management when the legacy version route is absent', async () => {
    const fetch = vi.fn().mockResolvedValueOnce(new Response(null, { status: 404 }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ schema_version: 3, platform_admin: true, items: [] })))
    vi.stubGlobal('fetch', fetch)
    expect(await detectServerKind('https://fleet.example', async () => 'secret')).toBe('management')
    expect(fetch).toHaveBeenCalledTimes(2)
    expect(fetch).toHaveBeenCalledWith('https://fleet.example/management/v1/organisations', expect.objectContaining({ redirect: 'error' }))
  })
  it('verifies the version of legacy workspace servers', async () => {
    const fetch = vi.fn().mockResolvedValueOnce(new Response(JSON.stringify({ api_version: 'v1', schema_version: 2, minimum_compatible_schema_version: 1, build_version: '0.3.12' })))
    vi.stubGlobal('fetch', fetch)
    expect(await detectServerKind('https://workspace.example/base', async () => 'secret')).toBe('workspace')
    expect(fetch.mock.calls[0][0]).toBe('https://workspace.example/base/api/v1/version')
    expect(fetch).toHaveBeenCalledTimes(1)
  })
  it('does not reinterpret auth errors or unrelated HTML as workspace servers', async () => {
    const fetch = vi.fn().mockResolvedValueOnce(new Response('{}', { status: 401 }))
      .mockResolvedValueOnce(new Response('<html>proxy error</html>'))
    vi.stubGlobal('fetch', fetch)
    await expect(detectServerKind('https://fleet.example', async () => 'secret')).rejects.toThrow()
    await expect(detectServerKind('https://fleet.example', async () => 'secret')).rejects.toThrow()
    expect(fetch).toHaveBeenCalledTimes(2)
  })
})
