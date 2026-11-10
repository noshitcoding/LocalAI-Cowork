import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { credentialToJson, parseCreationOptions, parseRequestOptions } from './webauthn'

const originalPublicKeyCredential = globalThis.PublicKeyCredential

function byteArray(value: BufferSource): number[] {
  return Array.from(ArrayBuffer.isView(value)
    ? new Uint8Array(value.buffer, value.byteOffset, value.byteLength)
    : new Uint8Array(value))
}

class FakePublicKeyCredential {
  id = 'credential-id'
  rawId = Uint8Array.from([6, 7]).buffer
  type = 'public-key'
  authenticatorAttachment = 'platform'
  response = {
    clientDataJSON: Uint8Array.from([8]).buffer,
    authenticatorData: Uint8Array.from([9]).buffer,
    signature: Uint8Array.from([10]).buffer,
    userHandle: null,
  }
  getClientExtensionResults() { return { credProps: { rk: true } } }
}

beforeEach(() => {
  Object.defineProperty(globalThis, 'PublicKeyCredential', {
    configurable: true,
    value: FakePublicKeyCredential,
  })
})

afterEach(() => {
  Object.defineProperty(globalThis, 'PublicKeyCredential', {
    configurable: true,
    value: originalPublicKeyCredential,
  })
})

describe('WebAuthn browser conversion', () => {
  it('decodes registration and authentication JSON options without relying on new browser parsers', () => {
    const creation = parseCreationOptions({ publicKey: {
      challenge: 'AQID',
      rp: { id: 'example.test', name: 'Open Cowork' },
      user: { id: 'BAU', name: 'user@example.test', displayName: 'User' },
      pubKeyCredParams: [{ type: 'public-key', alg: -7 }],
      excludeCredentials: [{ type: 'public-key', id: 'Bgc' }],
    } })
    const request = parseRequestOptions({ publicKey: {
      challenge: 'CAk',
      rpId: 'example.test',
      allowCredentials: [{ type: 'public-key', id: 'Cgs' }],
    } })

    expect(byteArray(creation.challenge)).toEqual([1, 2, 3])
    expect(byteArray(creation.user.id)).toEqual([4, 5])
    expect(byteArray(creation.excludeCredentials?.[0]?.id ?? new ArrayBuffer(0))).toEqual([6, 7])
    expect(byteArray(request.challenge)).toEqual([8, 9])
    expect(byteArray(request.allowCredentials?.[0]?.id ?? new ArrayBuffer(0))).toEqual([10, 11])
  })

  it('serializes assertion buffers as unpadded base64url', () => {
    expect(credentialToJson(new FakePublicKeyCredential() as unknown as Credential)).toEqual({
      id: 'credential-id',
      rawId: 'Bgc',
      type: 'public-key',
      authenticatorAttachment: 'platform',
      clientExtensionResults: { credProps: { rk: true } },
      response: {
        clientDataJSON: 'CA',
        authenticatorData: 'CQ',
        signature: 'Cg',
        userHandle: null,
      },
    })
  })
})
