import { describe, expect, it, vi } from 'vitest'

import { RemoteAuthClient } from './authClient'
import { SCHEMA_VERSION } from './contracts'

const passkeyCredential = vi.hoisted(() => ({ id: 'credential-json' }))
vi.mock('./webauthn', () => ({
  getPasskey: vi.fn(async () => passkeyCredential),
}))

const tokens = {
  schema_version: SCHEMA_VERSION,
  access_token: 'a'.repeat(43),
  access_expires_at: '2026-08-08T12:15:00Z',
  refresh_token: 'r'.repeat(43),
  refresh_expires_at: '2026-09-08T12:00:00Z',
  user_id: '44344ad5-e86a-41d7-985d-e9c40c735513',
  session_id: '721d19ac-8198-416a-be8e-258159501e4f',
}

function base64Url(bytes: Uint8Array): string {
  let binary = ''
  for (const byte of bytes) binary += String.fromCharCode(byte)
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/g, '')
}

describe('RemoteAuthClient native PKCE', () => {
  it('binds an S256 challenge to the one-time token exchange', async () => {
    const requests: Array<{ url: string; body: Record<string, string> }> = []
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input)
      requests.push({ url, body: JSON.parse(String(init?.body)) as Record<string, string> })
      if (url.endsWith('/authorize')) {
        return new Response(JSON.stringify({
          schema_version: SCHEMA_VERSION,
          code: 'c'.repeat(43),
          expires_at: '2026-08-08T12:05:00Z',
        }), { status: 200, headers: { 'content-type': 'application/json' } })
      }
      return new Response(JSON.stringify(tokens), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      })
    })
    const result = await new RemoteAuthClient('https://cowork.example.test', fetchMock as typeof fetch)
      .loginPkce({
        email: 'mobile@example.test',
        password: 'long-test-password',
        device_id: '08fc7215-ee55-4703-bf35-20b4eab03ae4',
      })

    expect(result).toEqual(tokens)
    expect(requests).toHaveLength(2)
    expect(requests[0]?.body.code_challenge_method).toBe('S256')
    expect(requests[0]?.body).not.toHaveProperty('code_verifier')
    expect(requests[1]?.body).not.toHaveProperty('password')
    const verifier = requests[1]?.body.code_verifier ?? ''
    const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(verifier))
    expect(requests[0]?.body.code_challenge).toBe(base64Url(new Uint8Array(digest)))
    expect(requests[1]?.body.device_id).toBe(requests[0]?.body.device_id)
  })
})

describe('RemoteAuthClient passkeys', () => {
  it('finishes a domain-bound one-time WebAuthn challenge', async () => {
    const requests: Array<{ url: string; body: Record<string, unknown> }> = []
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input)
      requests.push({ url, body: JSON.parse(String(init?.body)) as Record<string, unknown> })
      if (url.endsWith('/start')) {
        return new Response(JSON.stringify({
          schema_version: SCHEMA_VERSION,
          challenge_id: '8bbfba26-8514-4630-923f-3c79020cf601',
          public_key: { publicKey: { challenge: 'AQID' } },
          expires_at: '2026-08-08T12:05:00Z',
        }), { status: 200, headers: { 'content-type': 'application/json' } })
      }
      return new Response(JSON.stringify(tokens), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      })
    })

    const result = await new RemoteAuthClient('https://cowork.example.test', fetchMock as typeof fetch)
      .loginPasskey('mobile@example.test', '08fc7215-ee55-4703-bf35-20b4eab03ae4')

    expect(result).toEqual(tokens)
    expect(requests.map((request) => request.url)).toEqual([
      'https://cowork.example.test/api/v1/auth/passkeys/authenticate/start',
      'https://cowork.example.test/api/v1/auth/passkeys/authenticate/finish',
    ])
    expect(requests[0]?.body).toEqual({
      email: 'mobile@example.test',
      device_id: '08fc7215-ee55-4703-bf35-20b4eab03ae4',
    })
    expect(requests[1]?.body).toEqual({
      challenge_id: '8bbfba26-8514-4630-923f-3c79020cf601',
      credential: passkeyCredential,
    })
  })
})

describe('RemoteAuthClient browser sessions', () => {
  it('refreshes through the HttpOnly cookie endpoint without a token body', async () => {
    const browserTokens = { ...tokens }
    delete (browserTokens as Partial<typeof tokens>).refresh_token
    const fetchMock = vi.fn(async () => new Response(JSON.stringify(browserTokens), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    }))

    const result = await new RemoteAuthClient(
      'https://cowork.example.test',
      fetchMock as typeof fetch,
      true,
    ).refresh()

    expect(result.refresh_token).toBeUndefined()
    expect(fetchMock).toHaveBeenCalledWith(
      'https://cowork.example.test/api/v1/auth/browser/refresh',
      expect.objectContaining({
        method: 'POST',
        body: undefined,
        credentials: 'same-origin',
        headers: expect.objectContaining({ 'x-cowork-session-mode': 'browser-cookie' }),
      }),
    )
  })
})
