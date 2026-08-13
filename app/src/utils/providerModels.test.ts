import { describe, expect, it } from 'vitest'
import { normalizeProviderModels, resolveProviderModelFromCatalog } from './providerModels'

describe('provider model catalog resolution', () => {
  it('maps a stale base model alias to the unique fully qualified runtime model', () => {
    expect(resolveProviderModelFromCatalog(
      'google/gemma-4-31B-it',
      ['RedHatAI/gemma-4-31B-it-FP8-block'],
    )).toBe('RedHatAI/gemma-4-31B-it-FP8-block')
  })

  it('preserves an exact catalog model while normalizing its casing', () => {
    expect(resolveProviderModelFromCatalog(
      'redhatai/gemma-4-31b-it-fp8-block',
      ['RedHatAI/gemma-4-31B-it-FP8-block'],
    )).toBe('RedHatAI/gemma-4-31B-it-FP8-block')
  })

  it('does not guess when several unrelated models are available', () => {
    expect(resolveProviderModelFromCatalog(
      'custom/model',
      ['vendor/first', 'vendor/second'],
    )).toBe('custom/model')
  })

  it('deduplicates model ids case-insensitively', () => {
    expect(normalizeProviderModels([' Model/A ', 'model/a', 'Model/B'])).toEqual(['Model/A', 'Model/B'])
  })
})
