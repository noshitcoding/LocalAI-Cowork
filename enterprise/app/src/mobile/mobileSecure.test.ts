import { beforeEach, describe, expect, it } from 'vitest'

import {
  hasMobilePin,
  mobileSecureDelete,
  mobileSecureGet,
  mobileSecureSet,
  resetMobileSecureForTests,
  setMobilePin,
  verifyMobilePin,
} from './mobileSecure'

describe('mobile secure storage and app PIN', () => {
  beforeEach(() => resetMobileSecureForTests())

  it('roundtrips and removes namespaced secrets', async () => {
    await mobileSecureSet('account', 'token', 'classified')
    expect(await mobileSecureGet('account', 'token')).toBe('classified')
    expect(await mobileSecureGet('other', 'token')).toBeNull()
    await mobileSecureDelete('account', 'token')
    expect(await mobileSecureGet('account', 'token')).toBeNull()
  })

  it('derives and verifies a salted PIN without storing the PIN', async () => {
    expect(await hasMobilePin()).toBe(false)
    await setMobilePin('781204')
    expect(await hasMobilePin()).toBe(true)
    expect(await verifyMobilePin('781204')).toBe(true)
    expect(await verifyMobilePin('781205')).toBe(false)
    const verifier = await mobileSecureGet('app_lock', 'pin_verifier_v1')
    expect(verifier).not.toContain('781204')
  })

  it('rejects weak or malformed app PINs', async () => {
    await expect(setMobilePin('12345')).rejects.toThrow(/6 to 12 digits/)
    await expect(setMobilePin('password')).rejects.toThrow(/6 to 12 digits/)
  })
})
