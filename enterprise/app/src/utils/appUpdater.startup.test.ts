import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  check: vi.fn(),
  getVersion: vi.fn(),
  hasTauriRuntime: vi.fn(),
  relaunch: vi.fn(),
  safeInvoke: vi.fn(),
}))

vi.mock('@tauri-apps/plugin-updater', () => ({ check: mocks.check }))
vi.mock('@tauri-apps/api/app', () => ({ getVersion: mocks.getVersion }))
vi.mock('@tauri-apps/plugin-process', () => ({ relaunch: mocks.relaunch }))
vi.mock('./safeInvoke', () => ({
  hasTauriRuntime: mocks.hasTauriRuntime,
  safeInvoke: mocks.safeInvoke,
}))

import {
  checkForAppUpdate,
  getAppUpdateSnapshot,
  installAvailableAppUpdate,
  resetAppUpdaterForTests,
  retryAvailableAppUpdate,
  startAutomaticUpdateCheck,
  summarizeReleaseNotes,
} from './appUpdater'

describe('appUpdater startup behavior', () => {
  beforeEach(async () => {
    await resetAppUpdaterForTests()
    vi.clearAllMocks()
    mocks.hasTauriRuntime.mockReturnValue(true)
    mocks.getVersion.mockResolvedValue('1.2.3')
    mocks.relaunch.mockResolvedValue(undefined)
    window.localStorage.clear()
  })

  it('starts the automatic signed update check only once per app session', async () => {
    mocks.check.mockResolvedValue(null)

    startAutomaticUpdateCheck()
    startAutomaticUpdateCheck()

    await vi.waitFor(() => expect(mocks.check).toHaveBeenCalledTimes(1))
    expect(mocks.check).toHaveBeenCalledWith({ timeout: 30_000 })
    expect(getAppUpdateSnapshot().phase).toBe('up-to-date')
  })

  it('stores a safe compact first paragraph from the release notes', async () => {
    mocks.check.mockResolvedValue({
      version: '1.2.4',
      body: '**Faster startup** with [details](https://example.com).\n\nMore changes.',
      close: vi.fn().mockResolvedValue(undefined),
    })

    await checkForAppUpdate()

    expect(getAppUpdateSnapshot()).toMatchObject({
      phase: 'available',
      currentVersion: '1.2.3',
      availableVersion: '1.2.4',
      releaseNotes: 'Faster startup with details.',
    })
  })

  it('limits release notes to 800 plain-text characters', () => {
    const longBody = `<strong>Important</strong> [details](https://example.com) ${'x'.repeat(900)}\n\nIgnored.`

    const summary = summarizeReleaseNotes(longBody)

    expect(summary).not.toContain('<strong>')
    expect(summary).not.toContain('https://example.com')
    expect(summary).not.toContain('Ignored')
    expect(summary?.endsWith('…')).toBe(true)
    expect(summary?.length).toBeLessThanOrEqual(800)
  })

  it('checks again before retrying the complete safe install flow', async () => {
    const failedUpdate = {
      version: '1.2.4',
      close: vi.fn().mockResolvedValue(undefined),
      downloadAndInstall: vi.fn(),
    }
    const retriedUpdate = {
      version: '1.2.5',
      body: 'Retry release notes',
      close: vi.fn().mockResolvedValue(undefined),
      downloadAndInstall: vi.fn(async (onEvent: (event: unknown) => void) => {
        onEvent({ event: 'Started', data: { contentLength: 50 } })
        onEvent({ event: 'Progress', data: { chunkLength: 50 } })
        onEvent({ event: 'Finished' })
      }),
    }
    mocks.check.mockResolvedValueOnce(failedUpdate).mockResolvedValueOnce(retriedUpdate)
    mocks.safeInvoke
      .mockRejectedValueOnce(new Error('backup failed'))
      .mockResolvedValueOnce({
        path: 'C:\\retry-backup',
        databaseBackup: 'C:\\retry-backup\\open_cowork.db',
        localStorageBackup: 'C:\\retry-backup\\local-storage.json',
        itemCount: 1,
        createdAt: '2026-08-29T12:00:00Z',
      })

    await checkForAppUpdate()
    await installAvailableAppUpdate()
    await retryAvailableAppUpdate()

    expect(mocks.check).toHaveBeenCalledTimes(2)
    expect(failedUpdate.close).toHaveBeenCalledTimes(1)
    expect(retriedUpdate.downloadAndInstall).toHaveBeenCalledTimes(1)
    expect(mocks.relaunch).toHaveBeenCalledTimes(1)
    expect(getAppUpdateSnapshot()).toMatchObject({
      phase: 'restarting',
      availableVersion: '1.2.5',
      releaseNotes: 'Retry release notes',
    })
  })
})
