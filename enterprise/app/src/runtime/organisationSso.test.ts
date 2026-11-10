import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { RemoteAuthClient, type AuthTokens } from './authClient'
import { completePendingOidc, takeOidcDestination } from './oidc'

vi.mock('./nativePasskey',()=>({nativePasskeyAvailable:()=>false,parseNativePasskeyCallback:()=>null}))
const org='07fce15a-8c7f-4aa4-bb8b-a07d821015a5'
const server='https://management.example'
const key='open-cowork-oidc-pending-v1'
const state='s'.repeat(43)
const code='c'.repeat(43)
describe('Organisation-bound SSO clients',()=>{
  beforeEach(()=>{sessionStorage.clear();history.replaceState({},'',`/auth/callback?code=${code}&state=${state}`)})
  afterEach(()=>{vi.restoreAllMocks();sessionStorage.clear();history.replaceState({},'','/')})
  it('uses the same organisation scope for discovery and authenticated account linking',async()=>{
    const fetchMock=vi.fn().mockResolvedValueOnce(new Response(JSON.stringify({enabled:true,organisation_id:org,organisation_name:'Company',provider_label:'SSO'}))).mockResolvedValueOnce(new Response(JSON.stringify({schema_version:2,authorization_url:'https://identity.example/authorize',expires_at:'2026-10-01T10:00:00Z'})))
    const client=new RemoteAuthClient(server,fetchMock)
    expect(await client.oidcEnabled(org)).toBe(true)
    await client.startOidcAuthorization({device_id:org,client_state:state,code_challenge:code,redirect_uri:'open-cowork://auth/callback'},'token',org)
    expect(fetchMock.mock.calls[0][0]).toBe(`${server}/api/v1/auth/organisations/${org}/oidc/config`)
    expect(fetchMock.mock.calls[1][0]).toBe(`${server}/api/v1/auth/organisations/${org}/oidc/link/start`)
    expect(fetchMock.mock.calls[1][1].headers.authorization).toBe('Bearer token')
    await expect(client.oidcEnabled('../other')).rejects.toThrow()
    expect(fetchMock).toHaveBeenCalledTimes(2)
  })
  it('restores the chosen organisation only after exchanging a valid callback, once and on its server',async()=>{
    const exchange=vi.spyOn(RemoteAuthClient.prototype,'exchangeNativeCode').mockResolvedValue({access_token:'test'} as AuthTokens)
    sessionStorage.setItem(key,JSON.stringify({serverUrl:server,deviceId:org,verifier:'v'.repeat(43),state,expiresAt:Date.now()+60000,native:false,organisation:org}))
    expect(await completePendingOidc(server)).toEqual({access_token:'test'})
    expect(exchange).toHaveBeenCalledWith(code,'v'.repeat(43),org)
    expect(sessionStorage.getItem(key)).toBeNull()
    expect(takeOidcDestination(server)).toBe(`/server?organisation=${org}`)
    expect(takeOidcDestination(server)).toBe('/server')
  })
  it('does not exchange a pending authorization belonging to another server',async()=>{
    const exchange=vi.spyOn(RemoteAuthClient.prototype,'exchangeNativeCode')
    sessionStorage.setItem(key,JSON.stringify({serverUrl:server,deviceId:org,verifier:'v'.repeat(43),state,expiresAt:Date.now()+60000,native:false,organisation:org}))
    expect(await completePendingOidc('https://another.example')).toBeNull()
    expect(exchange).not.toHaveBeenCalled()
    expect(takeOidcDestination('https://another.example')).toBe('/server')
    expect(sessionStorage.getItem(key)).toBeNull()
  })
})
