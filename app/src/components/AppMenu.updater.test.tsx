import { fireEvent, render, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import i18n from '../i18n'
import { useUiStore } from '../stores/uiStore'

const updater = vi.hoisted(() => ({
  check: vi.fn(),
  install: vi.fn(),
  start: vi.fn(),
  snapshot: {
    phase: 'available',
    currentVersion: '1.2.3',
    availableVersion: '1.2.4',
    downloadedBytes: 0,
    contentLength: null,
    backupPath: null,
    error: null,
  },
}))

vi.mock('../utils/appUpdater', () => ({
  appUpdateProgressPercent: () => null,
  checkForAppUpdate: updater.check,
  getAppUpdateSnapshot: () => updater.snapshot,
  installAvailableAppUpdate: updater.install,
  startAutomaticUpdateCheck: updater.start,
  subscribeAppUpdater: () => () => {},
}))

import AppMenu from './AppMenu'

describe('AppMenu updater', () => {
  beforeEach(async () => {
    vi.clearAllMocks()
    await i18n.changeLanguage('en')
    useUiStore.setState({
      appMenuOpen: true,
      appMenuSearchFocused: false,
      shortcutsOverlayOpen: false,
    })
  })

  it('installs an available update with one action from the burger menu', () => {
    render(
      <MemoryRouter>
        <AppMenu open compactSidebar={false} onOpenWorkspaceSidebar={vi.fn()} />
      </MemoryRouter>,
    )

    const installButton = screen.getByRole('button', { name: /Install update 1.2.4/ })
    fireEvent.click(installButton)

    expect(updater.install).toHaveBeenCalledTimes(1)
    expect(updater.check).not.toHaveBeenCalled()
    expect(updater.start).toHaveBeenCalledTimes(1)
  })
})
