import { render, screen } from '@testing-library/react'
import { MemoryRouter, Route, Routes } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import i18n from '../i18n'
import { useUiStore } from '../stores/uiStore'

const updater = vi.hoisted(() => ({
  start: vi.fn(),
  snapshot: {
    phase: 'unsupported',
    currentVersion: null,
    availableVersion: null,
    releaseNotes: null,
    downloadedBytes: 0,
    contentLength: null,
    backupPath: null,
    error: null,
  },
}))

vi.mock('../utils/appUpdater', () => ({
  appUpdateProgressPercent: () => null,
  getAppUpdateSnapshot: () => updater.snapshot,
  installAvailableAppUpdate: vi.fn(),
  retryAvailableAppUpdate: vi.fn(),
  startAutomaticUpdateCheck: updater.start,
  subscribeAppUpdater: () => () => {},
}))

vi.mock('./LeftSidebar', () => ({ default: () => <div data-testid="left-sidebar" /> }))
vi.mock('./AppMenu', () => ({
  default: ({ open }: { open: boolean }) => <div data-testid="app-menu-state">{open ? 'open' : 'closed'}</div>,
}))
vi.mock('./AppContextDrawer', () => ({ default: () => <div data-testid="context-drawer" /> }))

import Layout from './Layout'

describe('Layout updater integration', () => {
  beforeEach(async () => {
    vi.clearAllMocks()
    await i18n.changeLanguage('en')
    useUiStore.setState({
      appMenuOpen: false,
      contextDrawerOpen: false,
      leftSidebarOpen: false,
    })
  })

  it('starts the global update check while the app menu is closed', () => {
    render(
      <MemoryRouter>
        <Routes>
          <Route element={<Layout />}>
            <Route index element={<div>Workspace</div>} />
          </Route>
        </Routes>
      </MemoryRouter>,
    )

    expect(screen.getByTestId('app-menu-state')).toHaveTextContent('closed')
    expect(screen.getByText('Workspace')).toBeInTheDocument()
    expect(updater.start).toHaveBeenCalledTimes(1)
  })
})
