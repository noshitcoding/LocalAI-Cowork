import { act, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import i18n from '../i18n'

const updater = vi.hoisted(() => ({
  install: vi.fn(),
  retry: vi.fn(),
  start: vi.fn(),
  listeners: new Set<() => void>(),
  snapshot: {} as {
    phase: string
    currentVersion: string | null
    availableVersion: string | null
    releaseNotes: string | null
    downloadedBytes: number
    contentLength: number | null
    backupPath: string | null
    error: string | null
  },
}))

vi.mock('../utils/appUpdater', () => ({
  appUpdateProgressPercent: (snapshot: typeof updater.snapshot) => (
    snapshot.contentLength
      ? Math.round((snapshot.downloadedBytes / snapshot.contentLength) * 100)
      : null
  ),
  getAppUpdateSnapshot: () => updater.snapshot,
  installAvailableAppUpdate: updater.install,
  retryAvailableAppUpdate: updater.retry,
  startAutomaticUpdateCheck: updater.start,
  subscribeAppUpdater: (listener: () => void) => {
    updater.listeners.add(listener)
    return () => updater.listeners.delete(listener)
  },
}))

import AppUpdateDialog from './AppUpdateDialog'
import {
  APP_UPDATE_SNOOZE_MS,
  APP_UPDATE_SNOOZE_STORAGE_KEY,
  resetAppUpdateDialogForTests,
} from './appUpdateDialogState'

const NOW = new Date('2026-08-29T12:00:00Z').getTime()

function updateSnapshot(patch: Partial<typeof updater.snapshot> = {}) {
  updater.snapshot = {
    phase: 'available',
    currentVersion: '1.2.3',
    availableVersion: '1.2.4',
    releaseNotes: 'Faster startup and safer updates.',
    downloadedBytes: 0,
    contentLength: null,
    backupPath: null,
    error: null,
    ...patch,
  }
}

function publishSnapshot(patch: Partial<typeof updater.snapshot>) {
  act(() => {
    updateSnapshot({ ...updater.snapshot, ...patch })
    updater.listeners.forEach((listener) => listener())
  })
}

describe('AppUpdateDialog', () => {
  beforeEach(async () => {
    vi.useFakeTimers()
    vi.setSystemTime(NOW)
    vi.clearAllMocks()
    updater.listeners.clear()
    window.localStorage.clear()
    resetAppUpdateDialogForTests()
    updateSnapshot()
    await i18n.changeLanguage('en')
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('checks on mount and shows versions plus compact release notes', () => {
    render(<AppUpdateDialog />)

    expect(updater.start).toHaveBeenCalledTimes(1)
    expect(screen.getByRole('dialog', { name: 'A new app version is available' })).toBeInTheDocument()
    expect(screen.getByText('1.2.3')).toBeInTheDocument()
    expect(screen.getByText('1.2.4')).toBeInTheDocument()
    expect(screen.getByText('Faster startup and safer updates.')).toBeInTheDocument()
  })

  it('suppresses every available version for exactly 24 hours after Later', () => {
    render(<AppUpdateDialog />)

    fireEvent.click(screen.getByRole('button', { name: 'Later' }))

    expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
    expect(window.localStorage.getItem(APP_UPDATE_SNOOZE_STORAGE_KEY)).toBe(String(NOW + APP_UPDATE_SNOOZE_MS))

    publishSnapshot({ availableVersion: '1.2.5', releaseNotes: 'An even newer version.' })
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument()

    act(() => vi.advanceTimersByTime(APP_UPDATE_SNOOZE_MS - 1))
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument()

    act(() => vi.advanceTimersByTime(1))
    expect(screen.getByRole('dialog')).toBeInTheDocument()
    expect(screen.getByText('1.2.5')).toBeInTheDocument()
  })

  it.each(['close button', 'Escape'] as const)('uses the same 24-hour snooze for %s', (method) => {
    render(<AppUpdateDialog />)

    if (method === 'close button') {
      fireEvent.click(screen.getByRole('button', { name: 'Remind me in 24 hours' }))
    } else {
      fireEvent.keyDown(window, { key: 'Escape' })
    }

    expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
    expect(window.localStorage.getItem(APP_UPDATE_SNOOZE_STORAGE_KEY)).toBe(String(NOW + APP_UPDATE_SNOOZE_MS))
  })

  it('does not dismiss when the backdrop is clicked', () => {
    const { container } = render(<AppUpdateDialog />)

    fireEvent.click(container.querySelector('.app-update-dialog-backdrop')!)

    expect(screen.getByRole('dialog')).toBeInTheDocument()
    expect(window.localStorage.getItem(APP_UPDATE_SNOOZE_STORAGE_KEY)).toBeNull()
  })

  it.each(['unsupported', 'up-to-date', 'error'] as const)('stays silent for startup phase %s', (phase) => {
    updateSnapshot({ phase, error: phase === 'error' ? 'network unavailable' : null })

    render(<AppUpdateDialog />)

    expect(updater.start).toHaveBeenCalledTimes(1)
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
  })

  it('shows protected install progress and keeps the dialog open', () => {
    render(<AppUpdateDialog />)
    fireEvent.click(screen.getByRole('button', { name: 'Install now' }))

    expect(updater.install).toHaveBeenCalledTimes(1)

    publishSnapshot({ phase: 'backing-up' })
    expect(screen.getByText('Backing up workspace...')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Remind me in 24 hours' })).toBeDisabled()

    publishSnapshot({ phase: 'downloading', downloadedBytes: 60, contentLength: 100 })
    expect(screen.getByText('Downloading update... 60%')).toBeInTheDocument()
    expect(screen.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '60')

    fireEvent.keyDown(window, { key: 'Escape' })
    expect(screen.getByRole('dialog')).toBeInTheDocument()
    expect(window.localStorage.getItem(APP_UPDATE_SNOOZE_STORAGE_KEY)).toBeNull()

    publishSnapshot({ phase: 'installing' })
    expect(screen.getByText('Installing update...')).toBeInTheDocument()

    publishSnapshot({ phase: 'restarting' })
    expect(screen.getByText('Restarting...')).toBeInTheDocument()
  })

  it('keeps install errors visible and offers a fresh retry or Later', async () => {
    updater.retry.mockResolvedValue(undefined)
    render(<AppUpdateDialog />)
    fireEvent.click(screen.getByRole('button', { name: 'Install now' }))
    publishSnapshot({ phase: 'backing-up' })
    publishSnapshot({ phase: 'error', error: 'backup failed' })

    expect(screen.getByRole('alert')).toHaveTextContent('backup failed')
    expect(screen.getByRole('button', { name: 'Later' })).toBeEnabled()

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Try again' }))
    })

    expect(updater.retry).toHaveBeenCalledTimes(1)
  })
})
