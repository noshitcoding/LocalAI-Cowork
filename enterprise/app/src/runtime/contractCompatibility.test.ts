import { describe, expect, it } from 'vitest'

import { protocolVersionsCompatible } from './contracts'

describe('distributed protocol compatibility', () => {
  it('allows a v1 client to use a v2 server that advertises N-1 support', () => {
    expect(protocolVersionsCompatible(1, 1, 2, 1)).toBe(true)
  })

  it('does not pretend a v2 client can use a v1 server', () => {
    expect(protocolVersionsCompatible(2, 1, 1, 1)).toBe(false)
  })

  it('rejects versions outside the adjacent compatibility window', () => {
    expect(protocolVersionsCompatible(1, 1, 3, 2)).toBe(false)
    expect(protocolVersionsCompatible(3, 2, 1, 1)).toBe(false)
  })
})
