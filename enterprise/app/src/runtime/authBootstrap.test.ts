import { describe, expect, it, vi } from 'vitest'
import { getBootstrapAvailability } from './authBootstrap'

function jsonResponse(value: unknown, status = 200): Response {
  return new Response(JSON.stringify(value), { status, headers: { 'content-type': 'application/json' } })
}

describe('public bootstrap availability', () => {
  it.each([true, false])('reads %s through a public uncached GET without account credentials', async available => {
    const fetchMock = vi.fn(async (_input: RequestInfo | URL, _init?: RequestInit) => jsonResponse({ bootstrap_available: available }))
    expect(await getBootstrapAvailability('https://cowork.example.test/?ignored=1#ignored', undefined, fetchMock)).toBe(available)
    expect(fetchMock).toHaveBeenCalledTimes(1)
    const [url, init] = fetchMock.mock.calls[0]!
    expect(url).toBe('https://cowork.example.test/api/v1/auth/bootstrap/status')
    expect(init).toEqual({ method: 'GET', credentials: 'omit', cache: 'no-store', redirect: 'error', signal: expect.any(AbortSignal) })
    expect(init?.headers).toBeUndefined()
    expect(init?.body).toBeUndefined()
  })

  it.each([null, true, {}, [], { bootstrap_available: null }, { bootstrap_available: 'true' }, { bootstrap_available: 1 }])('rejects malformed status %j instead of assuming availability', async value => {
    const fetchMock = vi.fn(async () => jsonResponse(value))
    await expect(getBootstrapAvailability('https://cowork.example.test', undefined, fetchMock)).rejects.toThrow('Invalid server setup status')
  })

  it('propagates server failures instead of inferring setup availability', async () => {
    const fetchMock = vi.fn(async () => jsonResponse({ error: 'auth_unavailable', message: 'Authentication is temporarily unavailable' }, 503))
    await expect(getBootstrapAvailability('https://cowork.example.test', undefined, fetchMock)).rejects.toMatchObject({ status: 503, code: 'auth_unavailable' })
  })

  it('propagates a network failure and malformed JSON', async () => {
    const fetchMock = vi.fn(async () => { throw new Error('Network unavailable') })
    await expect(getBootstrapAvailability('https://cowork.example.test', undefined, fetchMock)).rejects.toThrow('Network unavailable')
    const invalidJson = vi.fn(async () => new Response('not json', { status: 200 }))
    await expect(getBootstrapAvailability('https://cowork.example.test', undefined, invalidJson)).rejects.toThrow()
  })

  it('preserves caller cancellation and refuses URLs containing credentials', async () => {
    const controller = new AbortController()
    const fetchMock = vi.fn(async (_input: RequestInfo | URL, _init?: RequestInit) => jsonResponse({ bootstrap_available: false }))
    await getBootstrapAvailability('https://cowork.example.test', controller.signal, fetchMock)
    const signal = fetchMock.mock.calls[0]![1]!.signal!
    expect(signal.aborted).toBe(false)
    controller.abort()
    expect(signal.aborted).toBe(true)
    fetchMock.mockClear()
    await expect(getBootstrapAvailability('https://user:secret@cowork.example.test', undefined, fetchMock)).rejects.toThrow('must not contain credentials')
    expect(fetchMock).not.toHaveBeenCalled()
  })
})
