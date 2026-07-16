import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useMemoryStore } from './memoryStore'
import { safeInvoke } from '../utils/safeInvoke'

vi.mock('../utils/safeInvoke', () => ({
  hasTauriRuntime: vi.fn(() => false),
  safeInvoke: vi.fn(),
}))

const safeInvokeMock = vi.mocked(safeInvoke)

describe('memory provider secure fallback', () => {
  beforeEach(() => {
    localStorage.clear()
    safeInvokeMock.mockReset()
    useMemoryStore.setState({ providers: [], error: null })
  })

  it('moves legacy provider config to volatile memory and never rewrites it', async () => {
    const sentinel = 'memory-provider-config-must-not-persist'
    localStorage.setItem('open-cowork-providers-local', JSON.stringify([{
      id: 'legacy-provider',
      name: 'Legacy provider',
      provider_type: 'custom',
      config_json: JSON.stringify({ unknownCredential: sentinel }),
      enabled: true,
      last_sync_at: null,
      created_at: '2026-07-10T00:00:00.000Z',
      updated_at: '2026-07-10T00:00:00.000Z',
    }]))

    await useMemoryStore.getState().loadProviders()

    expect(useMemoryStore.getState().providers).toHaveLength(1)
    expect(useMemoryStore.getState().providers[0].config_json).toContain(sentinel)
    expect(localStorage.getItem('open-cowork-providers-local')).toBeNull()

    safeInvokeMock.mockRejectedValueOnce(new Error('desktop runtime unavailable'))
    await useMemoryStore.getState().upsertProvider({
      id: 'volatile-provider',
      name: 'Volatile provider',
      provider_type: 'custom',
      config_json: JSON.stringify({ secondCredential: sentinel }),
      enabled: true,
    })
    expect(useMemoryStore.getState().providers.some((provider) => provider.id === 'volatile-provider')).toBe(true)
    expect(JSON.stringify(localStorage)).not.toContain(sentinel)
  })
})
