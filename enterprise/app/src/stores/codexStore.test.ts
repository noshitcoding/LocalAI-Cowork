import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useCodexStore, type CodexAuthProfile } from './codexStore'

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  openUrl: vi.fn(),
  hasTauriRuntime: vi.fn(() => true),
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => mocks.invoke(...args),
}))

vi.mock('@tauri-apps/plugin-opener', () => ({
  openUrl: (...args: unknown[]) => mocks.openUrl(...args),
}))

vi.mock('../utils/safeInvoke', () => ({
  hasTauriRuntime: mocks.hasTauriRuntime,
}))

const profile: CodexAuthProfile = {
  id: 'codex-test',
  name: 'Codex test',
  email: null,
  accountId: null,
  planType: null,
  priority: 0,
  status: 'signed_out',
  cooldownUntil: null,
  quotaJson: null,
  quotaResetAt: null,
  createdAt: '2026-08-10T00:00:00.000Z',
  updatedAt: '2026-08-10T00:00:00.000Z',
}

describe('codexStore login', () => {
  beforeEach(() => {
    mocks.invoke.mockReset()
    mocks.openUrl.mockReset()
    mocks.hasTauriRuntime.mockReturnValue(true)
    useCodexStore.setState({
      profiles: [profile],
      deviceLogin: null,
      error: null,
    })
  })

  it('keeps device-code login in the app without opening a browser', async () => {
    mocks.invoke.mockResolvedValue({
      type: 'chatgptDeviceCode',
      loginId: 'login-device',
      verificationUrl: 'https://auth.openai.com/codex/device',
      userCode: 'ABCD-1234',
    })

    await useCodexStore.getState().login(profile.id, 'device')

    expect(mocks.invoke).toHaveBeenCalledWith('codex_login_start', {
      profileId: profile.id,
      flow: 'device',
    })
    expect(mocks.openUrl).not.toHaveBeenCalled()
    expect(useCodexStore.getState().deviceLogin).toEqual({
      profileId: profile.id,
      verificationUrl: 'https://auth.openai.com/codex/device',
      userCode: 'ABCD-1234',
    })
  })

  it('still opens the normal browser login explicitly', async () => {
    mocks.invoke.mockResolvedValue({
      type: 'chatgpt',
      loginId: 'login-browser',
      authUrl: 'https://chatgpt.com/auth/codex',
    })

    await useCodexStore.getState().login(profile.id, 'browser')

    expect(mocks.openUrl).toHaveBeenCalledWith('https://chatgpt.com/auth/codex')
  })

  it('serializes rapid profile renames while updating the input optimistically', async () => {
    let resolveFirst!: () => void
    const firstSave = new Promise<void>((resolve) => { resolveFirst = resolve })
    mocks.invoke
      .mockImplementationOnce(() => firstSave)
      .mockResolvedValueOnce(undefined)

    const firstRename = useCodexStore.getState().renameProfile(profile.id, 'First name')
    await vi.waitFor(() => expect(mocks.invoke).toHaveBeenCalledTimes(1))
    const secondRename = useCodexStore.getState().renameProfile(profile.id, 'Final name')

    expect(useCodexStore.getState().profiles[0]?.name).toBe('Final name')
    expect(mocks.invoke).toHaveBeenCalledTimes(1)

    resolveFirst()
    await Promise.all([firstRename, secondRename])

    expect(mocks.invoke).toHaveBeenCalledTimes(2)
    expect(mocks.invoke.mock.calls[1]?.[1]).toMatchObject({
      profile: { id: profile.id, name: 'Final name' },
    })
    expect(useCodexStore.getState().profiles[0]?.name).toBe('Final name')
  })
})
