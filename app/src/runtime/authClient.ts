import { z } from 'zod'

import { passkeyChallengeSchema, SCHEMA_VERSION } from './contracts'
import { normalizeServerUrl, RuntimeHttpError } from './runtimeClient'
import { getPasskey } from './webauthn'

const authTokensSchema = z.object({
  schema_version: z.number().int(),
  access_token: z.string().min(32),
  access_expires_at: z.string().datetime({ offset: true }),
  // Browser sessions intentionally omit this field. Their rotating refresh token
  // is held only in a same-origin HttpOnly cookie.
  refresh_token: z.string().min(32).optional(),
  refresh_expires_at: z.string().datetime({ offset: true }),
  user_id: z.string().uuid(),
  session_id: z.string().uuid(),
})
export type AuthTokens = z.infer<typeof authTokensSchema>

const IS_WEB_APP = import.meta.env.MODE === 'web' || import.meta.env.VITE_COWORK_WEB === 'true'
const BROWSER_SESSION_HEADER = { 'x-cowork-session-mode': 'browser-cookie' } as const

export interface PasswordCredentials {
  email: string
  password: string
  device_id: string
  second_factor?: string | null
}

export interface BootstrapCredentials extends PasswordCredentials {
  display_name: string
  bootstrap_token: string
}

const nativeAuthorizationSchema = z.object({
  schema_version: z.number().int(),
  code: z.string().min(32),
  expires_at: z.string().datetime({ offset: true }),
})

const oidcConfigurationSchema = z.object({
  schema_version: z.number().int(),
  enabled: z.boolean(),
})

const oidcAuthorizationSchema = z.object({
  schema_version: z.number().int(),
  authorization_url: z.string().url(),
  expires_at: z.string().datetime({ offset: true }),
})

function base64Url(bytes: Uint8Array): string {
  let binary = ''
  for (const byte of bytes) binary += String.fromCharCode(byte)
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/g, '')
}

function randomPkceVerifier(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(64))
  return base64Url(bytes)
}

export async function createPkcePair(): Promise<{ verifier: string; challenge: string }> {
  const verifier = randomPkceVerifier()
  const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(verifier))
  return { verifier, challenge: base64Url(new Uint8Array(digest)) }
}

export class RemoteAuthClient {
  readonly #baseUrl: string
  readonly #fetch: typeof globalThis.fetch
  readonly #browserSession: boolean

  constructor(
    baseUrl: string,
    fetchImplementation = globalThis.fetch.bind(globalThis),
    browserSession = IS_WEB_APP,
  ) {
    this.#baseUrl = normalizeServerUrl(baseUrl)
    this.#fetch = fetchImplementation
    this.#browserSession = browserSession
  }

  bootstrap(credentials: BootstrapCredentials): Promise<AuthTokens> {
    const { bootstrap_token, ...body } = credentials
    return this.#tokens('/api/v1/auth/bootstrap', body, bootstrap_token)
  }

  login(credentials: PasswordCredentials): Promise<AuthTokens> {
    return this.#tokens('/api/v1/auth/login', credentials)
  }

  acceptInvitation(request: {
    token: string
    display_name: string
    password: string
    device_id: string
  }): Promise<AuthTokens> {
    return this.#tokens('/api/v1/auth/invitations/accept', request)
  }

  async loginPasskey(email: string, deviceId: string): Promise<AuthTokens> {
    const startResponse = await this.#fetch(
      `${this.#baseUrl}/api/v1/auth/passkeys/authenticate/start`,
      {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ email, device_id: deviceId }),
      },
    )
    if (!startResponse.ok) throw await RuntimeHttpError.fromResponse(startResponse)
    const challenge = passkeyChallengeSchema.parse(await startResponse.json())
    if (challenge.schema_version !== SCHEMA_VERSION) {
      throw new Error(`Unsupported passkey schema version ${challenge.schema_version}`)
    }
    const credential = await getPasskey(challenge.public_key, this.#baseUrl)
    return this.#tokens('/api/v1/auth/passkeys/authenticate/finish', {
      challenge_id: challenge.challenge_id,
      credential,
    })
  }

  async loginPkce(credentials: PasswordCredentials): Promise<AuthTokens> {
    const { verifier, challenge } = await createPkcePair()
    const authorizationResponse = await this.#fetch(
      `${this.#baseUrl}/api/v1/auth/native/authorize`,
      {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({
          ...credentials,
          code_challenge: challenge,
          code_challenge_method: 'S256',
        }),
      },
    )
    if (!authorizationResponse.ok) throw await RuntimeHttpError.fromResponse(authorizationResponse)
    const authorization = nativeAuthorizationSchema.parse(await authorizationResponse.json())
    if (authorization.schema_version !== SCHEMA_VERSION) {
      throw new Error(`Unsupported authorization schema version ${authorization.schema_version}`)
    }
    return this.exchangeNativeCode(authorization.code, verifier, credentials.device_id)
  }

  exchangeNativeCode(code: string, verifier: string, deviceId: string): Promise<AuthTokens> {
    return this.#tokens('/api/v1/auth/native/token', {
      code,
      code_verifier: verifier,
      device_id: deviceId,
    })
  }

  async oidcEnabled(): Promise<boolean> {
    const response = await this.#fetch(`${this.#baseUrl}/api/v1/auth/oidc/config`)
    if (!response.ok) throw await RuntimeHttpError.fromResponse(response)
    return oidcConfigurationSchema.parse(await response.json()).enabled
  }

  async startOidcAuthorization(request: {
    device_id: string
    code_challenge: string
    client_state: string
    redirect_uri: string
  }, accessToken?: string): Promise<{ authorization_url: string; expires_at: string }> {
    const response = await this.#fetch(`${this.#baseUrl}/api/v1/auth/oidc${accessToken ? '/link' : ''}/start`, {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
        ...(accessToken ? { authorization: `Bearer ${accessToken}` } : {}),
      },
      body: JSON.stringify({ ...request, code_challenge_method: 'S256' }),
    })
    if (!response.ok) throw await RuntimeHttpError.fromResponse(response)
    return oidcAuthorizationSchema.parse(await response.json())
  }

  refresh(refreshToken?: string): Promise<AuthTokens> {
    if (this.#browserSession) {
      return this.#tokens('/api/v1/auth/browser/refresh')
    }
    if (!refreshToken) throw new Error('A native refresh token is required')
    return this.#tokens('/api/v1/auth/refresh', { refresh_token: refreshToken })
  }

  async logout(accessToken: string): Promise<void> {
    const response = await this.#fetch(`${this.#baseUrl}/api/v1/auth/logout`, {
      method: 'POST',
      headers: {
        authorization: `Bearer ${accessToken}`,
        ...(this.#browserSession ? BROWSER_SESSION_HEADER : {}),
      },
      credentials: this.#browserSession ? 'same-origin' : undefined,
    })
    if (!response.ok) throw await RuntimeHttpError.fromResponse(response)
  }

  async #tokens(path: string, body?: unknown, bearerToken?: string): Promise<AuthTokens> {
    const headers: Record<string, string> = { 'content-type': 'application/json' }
    if (bearerToken) headers.authorization = `Bearer ${bearerToken}`
    if (this.#browserSession) Object.assign(headers, BROWSER_SESSION_HEADER)
    const response = await this.#fetch(`${this.#baseUrl}${path}`, {
      method: 'POST',
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
      credentials: this.#browserSession ? 'same-origin' : undefined,
    })
    if (!response.ok) throw await RuntimeHttpError.fromResponse(response)
    const tokens = authTokensSchema.parse(await response.json())
    if (tokens.schema_version !== SCHEMA_VERSION) {
      throw new Error(`Unsupported auth schema version ${tokens.schema_version}`)
    }
    return tokens
  }
}
