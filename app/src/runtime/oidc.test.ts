import { describe, expect, it } from 'vitest'

import { parseOidcWebCallback } from './oidc'

const code = 'c'.repeat(43)
const state = 's'.repeat(43)

describe('OIDC client callback', () => {
  it('accepts only the canonical origin and exact callback path', () => {
    expect(parseOidcWebCallback(
      `https://cowork.example.test/auth/callback?code=${code}&state=${state}`,
      'https://cowork.example.test',
    )).toEqual({ code, state })
    expect(parseOidcWebCallback(
      `https://attacker.example/auth/callback?code=${code}&state=${state}`,
      'https://cowork.example.test',
    )).toBeNull()
    expect(parseOidcWebCallback(
      `https://cowork.example.test/auth/other?code=${code}&state=${state}`,
      'https://cowork.example.test',
    )).toBeNull()
  })

  it('rejects malformed one-time codes and states', () => {
    expect(parseOidcWebCallback(
      `https://cowork.example.test/auth/callback?code=short&state=${state}`,
      'https://cowork.example.test',
    )).toBeNull()
    expect(parseOidcWebCallback(
      `https://cowork.example.test/auth/callback?code=${code}&state=${state}%3D`,
      'https://cowork.example.test',
    )).toBeNull()
  })
})
