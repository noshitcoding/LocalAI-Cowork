import { describe, expect, it } from 'vitest'
import { REDACTED_VALUE, redactRecord, redactText } from './redaction'

describe('sensitive data redaction', () => {
  it('redacts nested credentials without removing operational fields', () => {
    const redacted = redactRecord({
      apiKey: 'provider-secret',
      nested: {
        client_secret: 'oauth-secret',
        env: { CUSTOM_NAME: 'environment-secret' },
        status: 200,
      },
      entries: [{ password: 'password-secret', durationMs: 42 }],
    })

    expect(redacted.apiKey).toBe(REDACTED_VALUE)
    expect(redacted.nested).toEqual({
      client_secret: REDACTED_VALUE,
      env: REDACTED_VALUE,
      status: 200,
    })
    expect(redacted.entries).toEqual([{ password: REDACTED_VALUE, durationMs: 42 }])
    expect(JSON.stringify(redacted)).not.toContain('provider-secret')
    expect(JSON.stringify(redacted)).not.toContain('environment-secret')
  })

  it('redacts bearer and query credentials embedded in free text', () => {
    const redacted = redactText(
      'Authorization: Bearer header-secret https://example.test/?token=query-secret&ok=1',
    )

    expect(redacted).not.toContain('header-secret')
    expect(redacted).not.toContain('query-secret')
    expect(redacted).toContain('ok=1')
  })

  it('redacts common standalone provider-token formats', () => {
    const redacted = redactText('provider returned sk-1234567890abcdef and ghp_1234567890abcdef')

    expect(redacted).not.toContain('sk-1234567890abcdef')
    expect(redacted).not.toContain('ghp_1234567890abcdef')
  })

  it('terminates circular and excessively deep structures', () => {
    const circular: Record<string, unknown> = { status: 'active' }
    circular.self = circular

    expect(redactRecord(circular)).toEqual({ status: 'active', self: '[TRUNCATED]' })
  })
})
