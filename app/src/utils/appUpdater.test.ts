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
} from './appUpdater'

describe('appUpdater', () => {
  beforeEach(async () => {
    await resetAppUpdaterForTests()
    vi.clearAllMocks()
    mocks.hasTauriRuntime.mockReturnValue(true)
    mocks.getVersion.mockResolvedValue('1.2.3')
    mocks.relaunch.mockResolvedValue(undefined)
    window.localStorage.clear()
  })

  it('reports an available signed GitHub update', async () => {
    mocks.check.mockResolvedValue({
      version: '1.2.4',
      close: vi.fn().mockResolvedValue(undefined),
    })

    await checkForAppUpdate()

    expect(mocks.check).toHaveBeenCalledWith({ timeout: 30_000 })
    expect(getAppUpdateSnapshot()).toMatchObject({
      phase: 'available',
      currentVersion: '1.2.3',
      availableVersion: '1.2.4',
    })
  })

  it('backs up workspace state before downloading, installing, and restarting', async () => {
    const calls: string[] = []
    const update = {
      version: '1.2.4',
      close: vi.fn().mockResolvedValue(undefined),
      downloadAndInstall: vi.fn(async (onEvent: (event: unknown) => void) => {
        calls.push('download')
        onEvent({ event: 'Started', data: { contentLength: 100 } })
        onEvent({ event: 'Progress', data: { chunkLength: 60 } })
        onEvent({ event: 'Finished' })
      }),
    }
    mocks.check.mockResolvedValue(update)
    mocks.safeInvoke.mockImplementation(async () => {
      calls.push('backup')
      return {
        path: 'C:\\backup',
        databaseBackup: 'C:\\backup\\open_cowork.db',
        localStorageBackup: 'C:\\backup\\local-storage.json',
        itemCount: 1,
        createdAt: '2026-07-29T12:00:00Z',
      }
    })
    mocks.relaunch.mockImplementation(async () => {
      calls.push('relaunch')
    })
    window.localStorage.setItem('open-cowork-config', '{"safe":true}')
    window.localStorage.setItem('open-cowork-providers-local', '{"apiKey":"must-not-back-up"}')
    window.localStorage.setItem('unrelated-site', 'ignored')

    await checkForAppUpdate()
    await installAvailableAppUpdate()

    expect(calls).toEqual(['backup', 'download', 'relaunch'])
    expect(mocks.safeInvoke).toHaveBeenCalledWith('update_backup_create', {
      request: {
        targetVersion: '1.2.4',
        localStorage: {
          'open-cowork-config': '{"safe":true}',
        },
      },
    })
    expect(update.downloadAndInstall).toHaveBeenCalledWith(expect.any(Function), { timeout: 900_000 })
    expect(getAppUpdateSnapshot()).toMatchObject({
      phase: 'restarting',
      downloadedBytes: 60,
      contentLength: 100,
      backupPath: 'C:\\backup',
    })
  })

  it('does not download when the mandatory backup fails', async () => {
    const update = {
      version: '1.2.4',
      close: vi.fn().mockResolvedValue(undefined),
      downloadAndInstall: vi.fn(),
    }
    mocks.check.mockResolvedValue(update)
    mocks.safeInvoke.mockRejectedValue(new Error('backup failed'))

    await checkForAppUpdate()
    await installAvailableAppUpdate()

    expect(update.downloadAndInstall).not.toHaveBeenCalled()
    expect(mocks.relaunch).not.toHaveBeenCalled()
    expect(getAppUpdateSnapshot()).toMatchObject({
      phase: 'error',
      error: 'backup failed',
    })
  })
})
