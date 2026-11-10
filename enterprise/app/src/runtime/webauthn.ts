type PublicKeyCredentialWithJson = PublicKeyCredential & { toJSON?: () => unknown }

type PublicKeyCredentialStatics = typeof PublicKeyCredential & {
  parseCreationOptionsFromJSON?: (options: unknown) => PublicKeyCredentialCreationOptions
  parseRequestOptionsFromJSON?: (options: unknown) => PublicKeyCredentialRequestOptions
}

function record(value: unknown, name: string): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`Invalid WebAuthn ${name}`)
  }
  return value as Record<string, unknown>
}

function base64UrlToBytes(value: unknown): ArrayBuffer {
  if (typeof value !== 'string' || !value) throw new Error('Invalid WebAuthn binary value')
  const normalized = value.replace(/-/g, '+').replace(/_/g, '/')
  const padded = normalized.padEnd(Math.ceil(normalized.length / 4) * 4, '=')
  const binary = atob(padded)
  const bytes = new Uint8Array(binary.length)
  for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index)
  return bytes.buffer
}

function bytesToBase64Url(value: ArrayBuffer | ArrayBufferView | null): string | null {
  if (value === null) return null
  const bytes = ArrayBuffer.isView(value)
    ? new Uint8Array(value.buffer, value.byteOffset, value.byteLength)
    : new Uint8Array(value)
  let binary = ''
  for (const byte of bytes) binary += String.fromCharCode(byte)
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/g, '')
}

function publicKeyPayload(value: unknown): Record<string, unknown> {
  const envelope = record(value, 'challenge')
  return record(envelope.publicKey ?? envelope, 'public key options')
}

export function webauthnAvailable(): boolean {
  return typeof window !== 'undefined'
    && typeof window.PublicKeyCredential !== 'undefined'
    && typeof navigator.credentials?.create === 'function'
    && typeof navigator.credentials?.get === 'function'
}

export function webauthnAvailableForOrigin(serverUrl: string): boolean {
  if (!webauthnAvailable()) return false
  try { return new URL(serverUrl).origin === window.location.origin }
  catch { return false }
}

function requireMatchingOrigin(serverUrl?: string): void {
  if (serverUrl && !webauthnAvailableForOrigin(serverUrl)) {
    throw new Error('Passkeys must be used from the Open Cowork web app on the server domain')
  }
}

export function parseCreationOptions(value: unknown): PublicKeyCredentialCreationOptions {
  const json = publicKeyPayload(value)
  const parser = (PublicKeyCredential as PublicKeyCredentialStatics).parseCreationOptionsFromJSON
  if (typeof parser === 'function') return parser(json)

  const user = record(json.user, 'user')
  const excludeCredentials = Array.isArray(json.excludeCredentials)
    ? json.excludeCredentials.map((item) => {
        const descriptor = record(item, 'credential descriptor')
        return { ...descriptor, id: base64UrlToBytes(descriptor.id) }
      })
    : undefined
  return {
    ...json,
    challenge: base64UrlToBytes(json.challenge),
    user: { ...user, id: base64UrlToBytes(user.id) },
    excludeCredentials,
  } as PublicKeyCredentialCreationOptions
}

export function parseRequestOptions(value: unknown): PublicKeyCredentialRequestOptions {
  const json = publicKeyPayload(value)
  const parser = (PublicKeyCredential as PublicKeyCredentialStatics).parseRequestOptionsFromJSON
  if (typeof parser === 'function') return parser(json)

  const allowCredentials = Array.isArray(json.allowCredentials)
    ? json.allowCredentials.map((item) => {
        const descriptor = record(item, 'credential descriptor')
        return { ...descriptor, id: base64UrlToBytes(descriptor.id) }
      })
    : undefined
  return {
    ...json,
    challenge: base64UrlToBytes(json.challenge),
    allowCredentials,
  } as PublicKeyCredentialRequestOptions
}

export function credentialToJson(value: Credential | null): unknown {
  if (!(value instanceof PublicKeyCredential)) throw new Error('The passkey ceremony was canceled')
  const credential = value as PublicKeyCredentialWithJson
  if (typeof credential.toJSON === 'function') return credential.toJSON()

  const response = credential.response
  const base = {
    id: credential.id,
    rawId: bytesToBase64Url(credential.rawId),
    type: credential.type,
    authenticatorAttachment: credential.authenticatorAttachment,
    clientExtensionResults: credential.getClientExtensionResults(),
  }
  if ('attestationObject' in response) {
    const attestation = response as AuthenticatorAttestationResponse
    return {
      ...base,
      response: {
        clientDataJSON: bytesToBase64Url(attestation.clientDataJSON),
        attestationObject: bytesToBase64Url(attestation.attestationObject),
        transports: typeof attestation.getTransports === 'function' ? attestation.getTransports() : [],
      },
    }
  }
  const assertion = response as AuthenticatorAssertionResponse
  return {
    ...base,
    response: {
      clientDataJSON: bytesToBase64Url(assertion.clientDataJSON),
      authenticatorData: bytesToBase64Url(assertion.authenticatorData),
      signature: bytesToBase64Url(assertion.signature),
      userHandle: bytesToBase64Url(assertion.userHandle),
    },
  }
}

export async function createPasskey(publicKey: unknown, serverUrl?: string): Promise<unknown> {
  if (!webauthnAvailable()) throw new Error('Passkeys are not supported by this browser or WebView')
  requireMatchingOrigin(serverUrl)
  return credentialToJson(await navigator.credentials.create({ publicKey: parseCreationOptions(publicKey) }))
}

export async function getPasskey(publicKey: unknown, serverUrl?: string): Promise<unknown> {
  if (!webauthnAvailable()) throw new Error('Passkeys are not supported by this browser or WebView')
  requireMatchingOrigin(serverUrl)
  return credentialToJson(await navigator.credentials.get({ publicKey: parseRequestOptions(publicKey) }))
}
