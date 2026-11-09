import { describe, expect, it } from 'vitest'
import { durableToolPolicy } from './permissionPolicy'

describe('durable execution permission boundary', () => {
  it('requires approval for mutations in Standard mode', () => {
    expect(durableToolPolicy('default')).toBe('confirm_mutations')
  })
  it('requires an explicit bypass before granting autonomy', () => {
    expect(durableToolPolicy('bypass')).toBe('autonomous')
  })
  it.each(['strict', 'plan', 'unrecognized', '', null, undefined])('fails closed for %s', (mode) => {
    expect(durableToolPolicy(mode)).toBe('read_only')
  })
})
