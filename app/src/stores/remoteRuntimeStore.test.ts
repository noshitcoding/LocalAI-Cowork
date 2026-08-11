import { beforeEach, describe, expect, it, vi } from 'vitest'

import { getCredential, resetVolatileCredentialsForTests } from '../security/credentialVault'
import { SCHEMA_VERSION } from '../runtime/contracts'
import {
  remoteAccessToken,
  resetRemoteRuntimeStoreForTests,
  useRemoteRuntimeStore,
} from './remoteRuntimeStore'

const USER_ID = '86d26246-719d-4eda-864f-4caa7381d4a0'
const SESSION_ID = '48718cee-c474-4a0c-871f-a0a7159c1296'

function tokenResponse(suffix: string) {
  return {
    schema_version: SCHEMA_VERSION,
    access_token: `access-${suffix}-${'a'.repeat(40)}`,
    access_expires_at: new Date(Date.now() + 5 * 60_000).toISOString(),
    refresh_token: `refresh-${suffix}-${'r'.repeat(40)}`,
    refresh_expires_at: new Date(Date.now() + 24 * 60 * 60_000).toISOString(),
    user_id: USER_ID,
    session_id: SESSION_ID,
  }
}

function authorizationResponse() {
  return {
    schema_version: SCHEMA_VERSION,
    code: `authorization-${'c'.repeat(40)}`,
    expires_at: new Date(Date.now() + 5 * 60_000).toISOString(),
  }
}

describe('remote runtime account', () => {
  beforeEach(() => {
    window.localStorage.clear()
    resetVolatileCredentialsForTests()
    resetRemoteRuntimeStoreForTests()
    vi.restoreAllMocks()
  })

  it('keeps access and refresh tokens out of localStorage', async () => {
    const tokens = tokenResponse('login')
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(authorizationResponse()), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(tokens), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      }))
    vi.stubGlobal('fetch', fetchMock)

    await useRemoteRuntimeStore.getState().login(
      'https://cowork.example.test/',
      'USER@example.test',
      'correct horse battery staple',
    )

    expect(useRemoteRuntimeStore.getState()).toMatchObject({
      status: 'authenticated',
      serverUrl: 'https://cowork.example.test',
      email: 'user@example.test',
      accessToken: tokens.access_token,
    })
    expect(window.localStorage.getItem('open-cowork-remote-connection-v1')).toContain('cowork.example.test')
    expect(JSON.stringify({ ...window.localStorage })).not.toContain(tokens.access_token)
    expect(JSON.stringify({ ...window.localStorage })).not.toContain(tokens.refresh_token)
    expect(await getCredential({ scope: 'remote_server', ownerId: 'primary', field: 'refresh_token' }))
      .toBe(tokens.refresh_token)
  })

  it('coalesces concurrent refreshes and persists the rotated token', async () => {
    const loginTokens = tokenResponse('login')
    const refreshTokens = tokenResponse('rotated')
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(authorizationResponse()), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(loginTokens), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(refreshTokens), { status: 200 }))
    vi.stubGlobal('fetch', fetchMock)

    await useRemoteRuntimeStore.getState().login(
      'https://cowork.example.test',
      'user@example.test',
      'password',
    )
    useRemoteRuntimeStore.setState({ accessExpiresAt: new Date(0).toISOString() })

    const [first, second] = await Promise.all([remoteAccessToken(), remoteAccessToken()])

    expect(first).toBe(refreshTokens.access_token)
    expect(second).toBe(refreshTokens.access_token)
    expect(fetchMock).toHaveBeenCalledTimes(3)
    expect(await getCredential({ scope: 'remote_server', ownerId: 'primary', field: 'refresh_token' }))
      .toBe(refreshTokens.refresh_token)
  })

  it('accepts an invitation without persisting either session token in localStorage', async () => {
    const tokens = tokenResponse('invitation')
    const fetchMock = vi.fn(async () => new Response(JSON.stringify(tokens), {
      status: 201,
      headers: { 'content-type': 'application/json' },
    }))
    vi.stubGlobal('fetch', fetchMock)

    await useRemoteRuntimeStore.getState().acceptInvitation(
      'https://cowork.example.test',
      'invited@example.test',
      'Invited User',
      'correct horse battery staple',
      `invitation-${'i'.repeat(40)}`,
    )

    expect(fetchMock).toHaveBeenCalledWith(
      'https://cowork.example.test/api/v1/auth/invitations/accept',
      expect.objectContaining({
        method: 'POST',
        body: expect.stringContaining('Invited User'),
      }),
    )
    expect(useRemoteRuntimeStore.getState()).toMatchObject({
      status: 'authenticated', email: 'invited@example.test', userId: USER_ID,
    })
    expect(JSON.stringify({ ...window.localStorage })).not.toContain(tokens.access_token)
    expect(JSON.stringify({ ...window.localStorage })).not.toContain(tokens.refresh_token)
    expect(await getCredential({ scope: 'remote_server', ownerId: 'primary', field: 'refresh_token' }))
      .toBe(tokens.refresh_token)
  })
})
