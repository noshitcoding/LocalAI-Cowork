import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { getCredential, resetVolatileCredentialsForTests } from '../security/credentialVault'
import { SCHEMA_VERSION } from '../runtime/contracts'
import {
  remoteAccessToken,
  remoteRuntimeClient,
  remoteTokenProvider,
  remoteDeviceId,
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

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((next) => { resolve = next })
  return { promise, resolve }
}

describe('remote runtime account', () => {
  beforeEach(() => {
    window.localStorage.clear()
    resetVolatileCredentialsForTests()
    resetRemoteRuntimeStoreForTests()
    vi.restoreAllMocks()
  })
  afterEach(() => { resetRemoteRuntimeStoreForTests(); vi.useRealTimers() })

  it('does not open the second-factor step for rejected passwords or arbitrary login errors', async () => {
    const fetchMock=vi.fn().mockResolvedValue(new Response(JSON.stringify({error:'unauthorized',message:'invalid email or password'}),{status:401}))
    vi.stubGlobal('fetch',fetchMock)
    await expect(useRemoteRuntimeStore.getState().login('https://cowork.example.test','user@example.test','wrong-password')).rejects.toThrow('invalid email or password')
    expect(useRemoteRuntimeStore.getState()).toMatchObject({status:'error',pendingPasswordLogin:null,userId:null,accessToken:null})
    expect(useRemoteRuntimeStore.getState().hasRecentPrimaryLogin()).toBe(false)
  })

  it('keeps the password private while the password-confirmed second-factor step is pending', async () => {
    const password='private-test-password-never-persisted'
    const fetchMock=vi.fn().mockResolvedValue(new Response(JSON.stringify({error:'unauthorized',message:'TOTP or recovery code is required'}),{status:401}))
    vi.stubGlobal('fetch',fetchMock)
    await useRemoteRuntimeStore.getState().login('https://cowork.example.test',' USER@example.test ',password)
    expect(useRemoteRuntimeStore.getState()).toMatchObject({status:'second_factor_required',error:null,pendingPasswordLogin:{serverUrl:'https://cowork.example.test',email:'user@example.test'},accessToken:null,userId:null})
    expect(JSON.stringify(useRemoteRuntimeStore.getState())).not.toContain(password)
    expect(JSON.stringify({...localStorage})).not.toContain(password)
    expect(await getCredential({scope:'remote_server',ownerId:'primary',field:'refresh_token'})).toBeNull()
    await expect(remoteAccessToken()).rejects.toThrow('Complete or cancel')
    expect(await useRemoteRuntimeStore.getState().restore()).toBe(false)
    expect(fetchMock).toHaveBeenCalledTimes(1)
  })

  it('retains a bounded pending login after a rejected code and authenticates only after the server accepts both factors', async () => {
    const fetchMock=vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify({error:'unauthorized',message:'TOTP or recovery code is required'}),{status:401}))
      .mockResolvedValueOnce(new Response(JSON.stringify({error:'unauthorized',message:'TOTP or recovery code is invalid or already used'}),{status:401}))
      .mockResolvedValueOnce(new Response(JSON.stringify(authorizationResponse())))
      .mockResolvedValueOnce(new Response(JSON.stringify(tokenResponse('verified'))))
    vi.stubGlobal('fetch',fetchMock)
    await useRemoteRuntimeStore.getState().login('https://cowork.example.test','user@example.test','private-password')
    const pending=useRemoteRuntimeStore.getState().pendingPasswordLogin
    await expect(useRemoteRuntimeStore.getState().completePasswordLogin('000000')).rejects.toThrow('invalid or already used')
    expect(useRemoteRuntimeStore.getState()).toMatchObject({status:'second_factor_required',pendingPasswordLogin:pending,userId:null,accessToken:null})
    await useRemoteRuntimeStore.getState().completePasswordLogin('single-use-recovery')
    expect(useRemoteRuntimeStore.getState()).toMatchObject({status:'authenticated',pendingPasswordLogin:null,userId:USER_ID})
    expect(useRemoteRuntimeStore.getState().hasRecentPrimaryLogin()).toBe(true)
    expect(JSON.parse(fetchMock.mock.calls[0][1].body).second_factor).toBeNull()
    expect(JSON.parse(fetchMock.mock.calls[1][1].body)).toMatchObject({password:'private-password',second_factor:'000000'})
    expect(JSON.parse(fetchMock.mock.calls[2][1].body)).toMatchObject({password:'private-password',second_factor:'single-use-recovery'})
  })

  it('clears the private pending password on cancellation, expiry and a server switch', async () => {
    const fetchMock=vi.fn(()=>Promise.resolve(new Response(JSON.stringify({error:'unauthorized',message:'TOTP or recovery code is required'}),{status:401})))
    vi.stubGlobal('fetch',fetchMock)
    await useRemoteRuntimeStore.getState().login('https://cowork.example.test','user@example.test','private-password')
    await useRemoteRuntimeStore.getState().cancelPasswordLogin()
    expect(useRemoteRuntimeStore.getState()).toMatchObject({status:'signed_out',pendingPasswordLogin:null})
    await expect(useRemoteRuntimeStore.getState().completePasswordLogin('123456')).rejects.toThrow('expired')
    await useRemoteRuntimeStore.getState().login('https://cowork.example.test','user@example.test','private-password')
    useRemoteRuntimeStore.getState().setConnection('https://other.example.test','other@example.test')
    await expect(useRemoteRuntimeStore.getState().completePasswordLogin('123456')).rejects.toThrow('expired')
    vi.useFakeTimers()
    await useRemoteRuntimeStore.getState().login('https://cowork.example.test','user@example.test','private-password')
    await vi.advanceTimersByTimeAsync(5*60_000+1)
    expect(useRemoteRuntimeStore.getState()).toMatchObject({status:'signed_out',pendingPasswordLogin:null})
    await expect(useRemoteRuntimeStore.getState().completePasswordLogin('123456')).rejects.toThrow('expired')
    expect(fetchMock).toHaveBeenCalledTimes(3)
  })

  it('keeps the same challenge visible during the OTP request and after network failure without duplicate submissions', async () => {
    let fail!: (reason:Error)=>void
    const fetchMock=vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify({error:'unauthorized',message:'TOTP or recovery code is required'}),{status:401}))
      .mockImplementationOnce(()=>new Promise<Response>((_resolve,reject)=>{fail=reject}))
    vi.stubGlobal('fetch',fetchMock)
    await useRemoteRuntimeStore.getState().login('https://cowork.example.test','user@example.test','private-password')
    const metadata=useRemoteRuntimeStore.getState().pendingPasswordLogin
    const work=useRemoteRuntimeStore.getState().completePasswordLogin('123456')
    const rejected=expect(work).rejects.toThrow('Network offline')
    expect(useRemoteRuntimeStore.getState()).toMatchObject({status:'authenticating',pendingPasswordLogin:metadata})
    await expect(useRemoteRuntimeStore.getState().completePasswordLogin('123456')).rejects.toThrow('no longer awaiting')
    await vi.waitFor(()=>expect(fetchMock).toHaveBeenCalledTimes(2))
    fail(new Error('Network offline'))
    await rejected
    expect(useRemoteRuntimeStore.getState()).toMatchObject({status:'second_factor_required',pendingPasswordLogin:metadata,error:'Network offline',accessToken:null})
  })

  it('ignores an accepted second-factor response arriving after cancellation', async () => {
    const response=deferred<Response>()
    const fetchMock=vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify({error:'unauthorized',message:'TOTP or recovery code is required'}),{status:401}))
      .mockResolvedValueOnce(new Response(JSON.stringify(authorizationResponse())))
      .mockImplementationOnce(()=>response.promise)
    vi.stubGlobal('fetch',fetchMock)
    await useRemoteRuntimeStore.getState().login('https://cowork.example.test','user@example.test','private-password')
    const complete=useRemoteRuntimeStore.getState().completePasswordLogin('123456')
    const rejected=expect(complete).rejects.toThrow('Authentication changed')
    await vi.waitFor(()=>expect(fetchMock).toHaveBeenCalledTimes(3))
    await useRemoteRuntimeStore.getState().cancelPasswordLogin()
    response.resolve(new Response(JSON.stringify(tokenResponse('late'))))
    await rejected
    expect(useRemoteRuntimeStore.getState()).toMatchObject({status:'signed_out',pendingPasswordLogin:null,userId:null,accessToken:null})
    expect(useRemoteRuntimeStore.getState().hasRecentPrimaryLogin()).toBe(false)
  })

  it('does not cancel a newer authenticated account when an obsolete second-factor form submits', async () => {
    useRemoteRuntimeStore.setState({serverUrl:'https://new.example.test',email:'new@example.test',status:'authenticated',userId:USER_ID,accessToken:'new-account-token'})
    await expect(useRemoteRuntimeStore.getState().completePasswordLogin('123456')).rejects.toThrow('expired')
    expect(useRemoteRuntimeStore.getState()).toMatchObject({status:'authenticated',serverUrl:'https://new.example.test',accessToken:'new-account-token'})
  })

  it('does not extend primary proof through refresh and does not manufacture it from a restored session or fleet ticket', async () => {
    const fetchMock=vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(authorizationResponse())))
      .mockResolvedValueOnce(new Response(JSON.stringify(tokenResponse('primary'))))
      .mockResolvedValueOnce(new Response(JSON.stringify(tokenResponse('refresh'))))
      .mockResolvedValueOnce(new Response(JSON.stringify(tokenResponse('restored'))))
      .mockResolvedValueOnce(new Response(JSON.stringify(tokenResponse('fleet'))))
    vi.stubGlobal('fetch',fetchMock)
    await useRemoteRuntimeStore.getState().login('https://cowork.example.test','user@example.test','private-password')
    expect(useRemoteRuntimeStore.getState().hasRecentPrimaryLogin()).toBe(true)
    vi.useFakeTimers();vi.setSystemTime(Date.now()+11*60_000)
    await remoteAccessToken()
    expect(useRemoteRuntimeStore.getState().hasRecentPrimaryLogin()).toBe(false)
    // Simulate a new renderer that retains only its secure refresh credential.
    resetRemoteRuntimeStoreForTests()
    useRemoteRuntimeStore.setState({serverUrl:'https://cowork.example.test',email:'user@example.test'})
    expect(await useRemoteRuntimeStore.getState().restore()).toBe(true)
    expect(useRemoteRuntimeStore.getState().hasRecentPrimaryLogin()).toBe(false)
    await useRemoteRuntimeStore.getState().loginFleet('https://cowork.example.test','user@example.test','ticket')
    expect(useRemoteRuntimeStore.getState().hasRecentPrimaryLogin()).toBe(false)
  })

  it('registers an organisation invitation without granting client-supplied identity or role', async () => {
    const tokens = tokenResponse('organisation-invitation')
    const fetchMock = vi.fn().mockResolvedValue(new Response(JSON.stringify(tokens), { status: 201 }))
    vi.stubGlobal('fetch', fetchMock)
    await useRemoteRuntimeStore.getState().registerOrganisationInvitation('https://management.example', ' Invited@Example.test ', 'Invited user', 'test-password-long', 't'.repeat(43))
    const [url, options] = fetchMock.mock.calls[0]
    expect(url).toBe('https://management.example/api/v1/auth/organisation-invitations/register')
    expect(JSON.parse(options.body)).toEqual({ token: 't'.repeat(43), display_name: 'Invited user', password: 'test-password-long', device_id: expect.any(String) })
    expect(options.redirect).toBe('error')
    expect(useRemoteRuntimeStore.getState()).toMatchObject({ status: 'authenticated', email: 'invited@example.test', userId: USER_ID })
    expect(JSON.stringify(localStorage)).not.toContain(tokens.refresh_token)
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
    expect(fetchMock.mock.calls.every(call => call[1].redirect === 'error')).toBe(true)
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

  it('replaces an invalid persisted device id with a server-compatible UUID', () => {
    window.localStorage.setItem('open-cowork-remote-device-v1', 'not-a-uuid')

    const deviceId = remoteDeviceId()

    expect(deviceId).toMatch(/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i)
    expect(deviceId).not.toBe('not-a-uuid')
    expect(window.localStorage.getItem('open-cowork-remote-device-v1')).toBe(deviceId)
  })

  it('keeps a stable session device id when browser storage is unavailable', () => {
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => { throw new Error('blocked') })
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new Error('blocked') })

    const first = remoteDeviceId()
    const second = remoteDeviceId()

    expect(first).toMatch(/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i)
    expect(second).toBe(first)
  })

  it('cannot restore a session after logout while a refresh is still in flight', async () => {
    const loginTokens = tokenResponse('login')
    const rotatedTokens = tokenResponse('rotated')
    const refreshResponse = deferred<Response>()
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(authorizationResponse()), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(loginTokens), { status: 200 }))
      .mockImplementationOnce(() => refreshResponse.promise)
      .mockResolvedValueOnce(new Response(null, { status: 204 }))
    vi.stubGlobal('fetch', fetchMock)

    await useRemoteRuntimeStore.getState().login('https://cowork.example.test', 'user@example.test', 'password')
    useRemoteRuntimeStore.setState({ accessExpiresAt: new Date(0).toISOString() })
    const pendingRefresh = remoteAccessToken()
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(3))

    const logout = useRemoteRuntimeStore.getState().logout()
    refreshResponse.resolve(new Response(JSON.stringify(rotatedTokens), { status: 200 }))

    await expect(pendingRefresh).rejects.toThrow('Authentication changed')
    await logout
    expect(useRemoteRuntimeStore.getState()).toMatchObject({
      status: 'signed_out', accessToken: null, accessExpiresAt: null,
    })
    expect(await getCredential({ scope: 'remote_server', ownerId: 'primary', field: 'refresh_token' }))
      .toBeNull()
  })

  it('does not apply a refresh response from the previously selected server', async () => {
    const loginTokens = tokenResponse('login')
    const staleTokens = tokenResponse('stale')
    const refreshResponse = deferred<Response>()
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(authorizationResponse()), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(loginTokens), { status: 200 }))
      .mockImplementationOnce(() => refreshResponse.promise)
    vi.stubGlobal('fetch', fetchMock)

    await useRemoteRuntimeStore.getState().login('https://first.example.test', 'user@example.test', 'password')
    useRemoteRuntimeStore.setState({ accessExpiresAt: new Date(0).toISOString() })
    const pendingRefresh = remoteAccessToken()
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(3))

    useRemoteRuntimeStore.getState().setConnection('https://second.example.test', 'other@example.test')
    refreshResponse.resolve(new Response(JSON.stringify(staleTokens), { status: 200 }))

    await expect(pendingRefresh).rejects.toThrow('Authentication changed')
    expect(useRemoteRuntimeStore.getState()).toMatchObject({
      serverUrl: 'https://second.example.test',
      status: 'signed_out',
      accessToken: null,
    })
    expect(await getCredential({ scope: 'remote_server', ownerId: 'primary', field: 'refresh_token' }))
      .toBeNull()
  })
  it('rejects a late login after a different server is selected', async () => {
    const pending = deferred<Response>()
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(authorizationResponse())))
      .mockImplementationOnce(() => pending.promise)
    vi.stubGlobal('fetch', fetchMock)
    const login = useRemoteRuntimeStore.getState().login('https://old.example.test', 'old@example.test', 'password')
    const rejected = expect(login).rejects.toThrow('Authentication changed')
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2))
    useRemoteRuntimeStore.getState().setConnection('https://new.example.test', 'new@example.test')
    pending.resolve(new Response(JSON.stringify(tokenResponse('old'))))
    await rejected
    expect(useRemoteRuntimeStore.getState()).toMatchObject({ serverUrl: 'https://new.example.test', status: 'signed_out', accessToken: null })
    expect(await getCredential({ scope: 'remote_server', ownerId: 'primary', field: 'refresh_token' })).toBeNull()
  })

  it('never supplies new credentials to an old client, even after switching back to the same URL', async () => {
    useRemoteRuntimeStore.setState({ serverUrl: 'https://old.example.test', status: 'authenticated', accessToken: 'old', accessExpiresAt: new Date(Date.now() + 60_000).toISOString() })
    const provider = remoteTokenProvider()
    const client = remoteRuntimeClient()
    useRemoteRuntimeStore.getState().setConnection('https://new.example.test', 'new@example.test')
    useRemoteRuntimeStore.getState().setConnection('https://old.example.test', 'different@example.test')
    useRemoteRuntimeStore.setState({ status: 'authenticated', accessToken: 'new', accessExpiresAt: new Date(Date.now() + 60_000).toISOString() })
    const fetchMock = vi.fn()
    vi.stubGlobal('fetch', fetchMock)
    await expect(provider()).rejects.toThrow('Authentication changed')
    await expect(client.listRuns()).rejects.toThrow('Authentication changed')
    expect(fetchMock).not.toHaveBeenCalled()
  })

  it('does not sign out a new server when the previous refresh fails late', async () => {
    const pending = deferred<Response>()
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(authorizationResponse())))
      .mockResolvedValueOnce(new Response(JSON.stringify(tokenResponse('old'))))
      .mockImplementationOnce(() => pending.promise)
    vi.stubGlobal('fetch', fetchMock)
    await useRemoteRuntimeStore.getState().login('https://old.example.test', 'old@example.test', 'password')
    useRemoteRuntimeStore.setState({ accessExpiresAt: new Date(0).toISOString() })
    const refresh = remoteAccessToken()
    const rejected = expect(refresh).rejects.toThrow('Authentication changed')
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(3))
    useRemoteRuntimeStore.getState().setConnection('https://new.example.test', 'new@example.test')
    useRemoteRuntimeStore.setState({ status: 'authenticated', accessToken: 'new-token', accessExpiresAt: new Date(Date.now() + 60_000).toISOString() })
    pending.resolve(new Response(JSON.stringify({ message: 'Expired old session' }), { status: 401 }))
    await rejected
    expect(useRemoteRuntimeStore.getState()).toMatchObject({ status: 'authenticated', accessToken: 'new-token', error: null })
  })

})
