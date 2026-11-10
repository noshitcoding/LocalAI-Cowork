import { beforeEach, describe, expect, it } from 'vitest'
import { useServerProfilesStore } from './serverProfilesStore'

describe('saved server addresses', () => {
  beforeEach(() => { localStorage.clear(); useServerProfilesStore.setState({ profiles: [] }) })
  it('normalises, updates and removes profiles without storing credentials', () => {
    const store = useServerProfilesStore.getState()
    const first = store.save('https://team.example/', 'USER@example.test', 'Team')
    const second = store.save('https://team.example', 'user@example.test', 'Renamed')
    expect(second.id).toBe(first.id)
    expect(useServerProfilesStore.getState().profiles).toEqual([second])
    expect(Object.keys(JSON.parse(localStorage.getItem('open-cowork-server-profiles-v1')!)[0]).sort()).toEqual(['email', 'id', 'name', 'serverUrl'])
    store.remove(first.id)
    expect(JSON.parse(localStorage.getItem('open-cowork-server-profiles-v1')!)).toEqual([])
  })
  it('rejects embedded credentials and unencrypted remote addresses', () => {
    for (const url of ['https://user:password@server.test', 'http://server.test']) {
      expect(() => useServerProfilesStore.getState().save(url, '')).toThrow()
    }
    expect(useServerProfilesStore.getState().save('http://127.0.0.1:18082', '').serverUrl).toBe('http://127.0.0.1:18082')
  })
})
