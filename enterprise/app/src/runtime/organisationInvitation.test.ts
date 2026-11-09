import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { clearInvitation, invitationLink, pendingInvitation, rememberInvitation } from './organisationInvitation'

describe('organisation invitation navigation', () => {
  beforeEach(() => { sessionStorage.clear(); vi.useFakeTimers(); vi.setSystemTime(new Date('2026-09-27T00:00:00Z')) })
  afterEach(() => vi.useRealTimers())
  const token = 't'.repeat(43)
  it('keeps the invitation secret out of the path and query string', () => {
    const url = new URL(invitationLink('https://management.example/', token))
    expect(url.pathname).toBe('/join')
    expect(url.search).toBe('')
    expect(new URLSearchParams(url.hash.slice(1)).get('invitation')).toBe(token)
  })
  it('resumes only on the same server and expires after fifteen minutes', () => {
    rememberInvitation('https://management.example', token, '2026-09-28T00:00:00Z')
    expect(pendingInvitation('https://other.example')).toBe('')
    expect(pendingInvitation('https://management.example/')).toBe(token)
    vi.advanceTimersByTime(15 * 60 * 1000)
    expect(pendingInvitation('https://management.example')).toBe('')
    expect(sessionStorage.length).toBe(0)
  })
  it('honours the invitation expiry and explicit cancellation', () => {
    rememberInvitation('https://management.example', token, '2026-09-27T00:01:00Z')
    vi.advanceTimersByTime(60 * 1000)
    expect(pendingInvitation('https://management.example')).toBe('')
    rememberInvitation('https://management.example', token, '2026-09-28T00:00:00Z')
    clearInvitation()
    expect(pendingInvitation('https://management.example')).toBe('')
  })
})
