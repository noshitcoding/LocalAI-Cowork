import { beforeEach, describe, expect, it } from 'vitest'

import { EMPTY_MOBILE_OFFLINE_STATE, loadMobileOfflineState, saveMobileOfflineState } from './mobileOfflineStore'
import { resetMobileSecureForTests } from './mobileSecure'

describe('encrypted Android offline state', () => {
  beforeEach(() => {
    localStorage.clear()
    resetMobileSecureForTests()
  })

  it('encrypts and restores the offline outbox', async () => {
    const secretRunId = '58e5435f-c495-47fb-87c7-c21fde0ca2bc'
    await saveMobileOfflineState({
      ...EMPTY_MOBILE_OFFLINE_STATE,
      outbox: [{
        id: 'b6584aef-87c6-48b6-9acb-4acdeed6d7a6',
        kind: 'cancel_run',
        runId: secretRunId,
        createdAt: '2026-08-08T12:00:00.000Z',
        attempts: 0,
      }],
    })
    const ciphertext = localStorage.getItem('open-cowork-mobile-cache-v1')
    expect(ciphertext).toBeTruthy()
    expect(ciphertext).not.toContain(secretRunId)
    const restored = await loadMobileOfflineState()
    expect(restored.outbox).toHaveLength(1)
    expect(restored.outbox[0]?.runId).toBe(secretRunId)
  })

  it('uses a fresh random IV for every save', async () => {
    await saveMobileOfflineState(EMPTY_MOBILE_OFFLINE_STATE)
    const first = localStorage.getItem('open-cowork-mobile-cache-v1')
    await saveMobileOfflineState(EMPTY_MOBILE_OFFLINE_STATE)
    expect(localStorage.getItem('open-cowork-mobile-cache-v1')).not.toBe(first)
  })

  it('rejects tampering and removes the unreadable cache', async () => {
    await saveMobileOfflineState(EMPTY_MOBILE_OFFLINE_STATE)
    const encoded = localStorage.getItem('open-cowork-mobile-cache-v1')!
    localStorage.setItem('open-cowork-mobile-cache-v1', `${encoded.slice(0, -2)}AA`)
    await expect(loadMobileOfflineState()).rejects.toThrow(/could not be opened/)
    expect(localStorage.getItem('open-cowork-mobile-cache-v1')).toBeNull()
  })
})
