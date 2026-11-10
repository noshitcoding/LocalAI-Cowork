import { createHash, randomBytes } from 'node:crypto'
import { chromium } from '@playwright/test'

const baseUrl = process.env.COWORK_TEST_BASE_URL
const accessToken = process.env.COWORK_TEST_ACCESS_TOKEN
const email = process.env.COWORK_TEST_EMAIL
const deviceId = process.env.COWORK_TEST_DEVICE_ID
if (!baseUrl || !accessToken || !email || !deviceId) throw new Error('missing passkey E2E environment')

const json = async (path, init = {}) => {
  const response = await fetch(`${baseUrl}${path}`, init)
  const payload = await response.json().catch(() => ({}))
  if (!response.ok) throw new Error(`${path} returned ${response.status}: ${payload.message ?? 'unknown error'}`)
  return payload
}

const authenticated = (method, body) => ({
  method,
  headers: { authorization: `Bearer ${accessToken}`, 'content-type': 'application/json' },
  ...(body === undefined ? {} : { body: JSON.stringify(body) }),
})

const verifier = randomBytes(64).toString('base64url')
const challenge = createHash('sha256').update(verifier, 'ascii').digest('base64url')
const state = randomBytes(32).toString('base64url')
const browser = await chromium.launch({ headless: true })

try {
  const context = await browser.newContext()
  const page = await context.newPage()
  const cdp = await context.newCDPSession(page)
  await cdp.send('WebAuthn.enable')
  await cdp.send('WebAuthn.addVirtualAuthenticator', {
    options: {
      protocol: 'ctap2',
      ctap2Version: 'ctap2_1',
      transport: 'internal',
      hasResidentKey: true,
      hasUserVerification: true,
      isUserVerified: true,
      automaticPresenceSimulation: true,
    },
  })
  await page.goto(`${baseUrl}/api/v1/auth/native/passkey/authorize`)

  const registered = await page.evaluate(async ({ token }) => {
    const decode = (value) => {
      const normalized = value.replace(/-/g, '+').replace(/_/g, '/')
      const binary = atob(normalized.padEnd(Math.ceil(normalized.length / 4) * 4, '='))
      return Uint8Array.from(binary, (character) => character.charCodeAt(0)).buffer
    }
    const encode = (value) => {
      if (value === null) return null
      const bytes = new Uint8Array(value)
      let binary = ''
      bytes.forEach((byte) => { binary += String.fromCharCode(byte) })
      return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/g, '')
    }
    const request = async (path, body) => {
      const response = await fetch(path, {
        method: 'POST',
        headers: { authorization: `Bearer ${token}`, 'content-type': 'application/json' },
        ...(body === undefined ? {} : { body: JSON.stringify(body) }),
      })
      if (!response.ok) throw new Error(`${path}: ${response.status} ${await response.text()}`)
      return response.json()
    }
    const started = await request('/api/v1/auth/passkeys/register/start')
    const source = started.public_key.publicKey ?? started.public_key
    const publicKey = typeof PublicKeyCredential.parseCreationOptionsFromJSON === 'function'
      ? PublicKeyCredential.parseCreationOptionsFromJSON(source)
      : {
          ...source,
          challenge: decode(source.challenge),
          user: { ...source.user, id: decode(source.user.id) },
          excludeCredentials: (source.excludeCredentials ?? []).map((item) => ({ ...item, id: decode(item.id) })),
        }
    const credential = await navigator.credentials.create({ publicKey })
    if (!credential) throw new Error('registration returned no credential')
    const serialized = typeof credential.toJSON === 'function' ? credential.toJSON() : {
      id: credential.id,
      rawId: encode(credential.rawId),
      type: credential.type,
      authenticatorAttachment: credential.authenticatorAttachment,
      clientExtensionResults: credential.getClientExtensionResults(),
      response: {
        clientDataJSON: encode(credential.response.clientDataJSON),
        attestationObject: encode(credential.response.attestationObject),
        transports: credential.response.getTransports?.() ?? [],
      },
    }
    return request('/api/v1/auth/passkeys/register/finish', {
      challenge_id: started.challenge_id,
      label: 'Chromium virtual authenticator',
      credential: serialized,
    })
  }, { token: accessToken })
  if (registered.label !== 'Chromium virtual authenticator') throw new Error('passkey registration label mismatch')

  const nativeResult = await page.evaluate(async ({ emailAddress, boundDeviceId, pkceChallenge, clientState }) => {
    const decode = (value) => {
      const normalized = value.replace(/-/g, '+').replace(/_/g, '/')
      const binary = atob(normalized.padEnd(Math.ceil(normalized.length / 4) * 4, '='))
      return Uint8Array.from(binary, (character) => character.charCodeAt(0)).buffer
    }
    const encode = (value) => {
      if (value === null) return null
      const bytes = new Uint8Array(value)
      let binary = ''
      bytes.forEach((byte) => { binary += String.fromCharCode(byte) })
      return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/g, '')
    }
    const post = async (path, body) => {
      const response = await fetch(path, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) })
      if (!response.ok) throw new Error(`${path}: ${response.status} ${await response.text()}`)
      return response.json()
    }
    const started = await post('/api/v1/auth/native/passkey/start', {
      email: emailAddress,
      device_id: boundDeviceId,
      code_challenge: pkceChallenge,
      code_challenge_method: 'S256',
      state: clientState,
      redirect_uri: 'open-cowork://auth/callback',
    })
    const source = started.public_key.publicKey ?? started.public_key
    const publicKey = typeof PublicKeyCredential.parseRequestOptionsFromJSON === 'function'
      ? PublicKeyCredential.parseRequestOptionsFromJSON(source)
      : {
          ...source,
          challenge: decode(source.challenge),
          allowCredentials: (source.allowCredentials ?? []).map((item) => ({ ...item, id: decode(item.id) })),
        }
    const credential = await navigator.credentials.get({ publicKey })
    if (!credential) throw new Error('authentication returned no credential')
    const serialized = typeof credential.toJSON === 'function' ? credential.toJSON() : {
      id: credential.id,
      rawId: encode(credential.rawId),
      type: credential.type,
      authenticatorAttachment: credential.authenticatorAttachment,
      clientExtensionResults: credential.getClientExtensionResults(),
      response: {
        clientDataJSON: encode(credential.response.clientDataJSON),
        authenticatorData: encode(credential.response.authenticatorData),
        signature: encode(credential.response.signature),
        userHandle: encode(credential.response.userHandle),
      },
    }
    const finished = await post('/api/v1/auth/native/passkey/finish', {
      challenge_id: started.challenge_id,
      authorization_id: started.authorization_id,
      credential: serialized,
    })
    const replay = await fetch('/api/v1/auth/native/passkey/finish', {
      method: 'POST', headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ challenge_id: started.challenge_id, authorization_id: started.authorization_id, credential: serialized }),
    })
    return { ...finished, challengeReplayStatus: replay.status }
  }, { emailAddress: email, boundDeviceId: deviceId, pkceChallenge: challenge, clientState: state })

  const callback = new URL(nativeResult.redirect_url)
  if (callback.protocol !== 'open-cowork:' || callback.hostname !== 'auth' || callback.pathname !== '/callback') {
    throw new Error('native passkey callback target mismatch')
  }
  if (callback.searchParams.get('state') !== state) throw new Error('native passkey state mismatch')
  if (nativeResult.challengeReplayStatus !== 401) throw new Error('WebAuthn challenge replay was not rejected')
  const code = callback.searchParams.get('code')
  const tokens = await json('/api/v1/auth/native/token', {
    method: 'POST', headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ code, code_verifier: verifier, device_id: deviceId }),
  })
  if (typeof tokens.access_token !== 'string' || tokens.access_token.length < 32) throw new Error('native passkey token exchange failed')
  const replay = await fetch(`${baseUrl}/api/v1/auth/native/token`, {
    method: 'POST', headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ code, code_verifier: verifier, device_id: deviceId }),
  })
  if (replay.status !== 401) throw new Error('native passkey authorization code replay was not rejected')

  const passkeys = await json('/api/v1/auth/passkeys', authenticated('GET'))
  if (passkeys.length !== 1 || passkeys[0].last_used_at === null) throw new Error('passkey use was not persisted')
  const badRedirect = await fetch(`${baseUrl}/api/v1/auth/native/passkey/start`, {
    method: 'POST', headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      email, device_id: deviceId, code_challenge: challenge, code_challenge_method: 'S256',
      state, redirect_uri: 'https://attacker.invalid/callback',
    }),
  })
  if (badRedirect.status !== 422) throw new Error('untrusted native redirect was not rejected')

  console.log('passkey_registration=ok')
  console.log('native_passkey_pkce=ok')
  console.log('passkey_replay_protection=ok')
  console.log('passkey_redirect_allowlist=ok')
} finally {
  await browser.close()
}
