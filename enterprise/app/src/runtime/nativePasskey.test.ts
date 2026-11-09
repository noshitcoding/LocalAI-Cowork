import { describe, expect, it } from 'vitest'

import { nativePasskeyAuthorizationUrl, parseNativePasskeyCallback } from './nativePasskey'

const token = 'a'.repeat(43)

describe('native passkey authorization', () => {
  it('keeps account and PKCE request data in the browser fragment', () => {
    const value = nativePasskeyAuthorizationUrl('https://cowork.example.test', {
      email: 'user@example.test',
      deviceId: '08fc7215-ee55-4703-bf35-20b4eab03ae4',
      challenge: token,
      state: 's'.repeat(43),
    })
    const url = new URL(value)
    expect(url.origin + url.pathname).toBe('https://cowork.example.test/api/v1/auth/native/passkey/authorize')
    expect(url.search).toBe('')
    expect(url.hash).toContain('email=user%40example.test')
    expect(url.hash).toContain(`code_challenge=${token}`)
  })

  it('accepts only the exact callback scheme, host, path, and base64url values', () => {
    expect(parseNativePasskeyCallback(`open-cowork://auth/callback?code=${token}&state=${'s'.repeat(43)}`)).toEqual({
      code: token,
      state: 's'.repeat(43),
    })
    expect(parseNativePasskeyCallback(`https://auth/callback?code=${token}&state=${'s'.repeat(43)}`)).toBeNull()
    expect(parseNativePasskeyCallback(`open-cowork://evil/callback?code=${token}&state=${'s'.repeat(43)}`)).toBeNull()
    expect(parseNativePasskeyCallback(`open-cowork://auth/other?code=${token}&state=${'s'.repeat(43)}`)).toBeNull()
    expect(parseNativePasskeyCallback(`open-cowork://auth/callback?code=${token}%3D&state=${'s'.repeat(43)}`)).toBeNull()
  })
})
