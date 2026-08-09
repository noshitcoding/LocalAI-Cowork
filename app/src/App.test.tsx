import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, vi } from 'vitest'
import App from './App'
import i18n from './i18n'
import { PRODUCT_ROUTES } from './product/routeRegistry'
import { useChatStore } from './stores/chatStore'
import { useConfigStore } from './stores/configStore'
import { useUiStore } from './stores/uiStore'

async function openMainMenu() {
  fireEvent.click(screen.getByRole('button', { name: /Open main menu|Hauptmenü öffnen/ }))
  return screen.findByRole('dialog')
}

async function clickMenuRoute(label: string) {
  const menu = await openMainMenu()
  fireEvent.click(within(menu).getByRole('button', { name: new RegExp(`^${label}`) }))
}

describe('App', () => {
  beforeEach(async () => {
    await i18n.changeLanguage('en')
    Object.defineProperty(window, 'matchMedia', {
      configurable: true,
      writable: true,
      value: vi.fn().mockImplementation((query: string) => ({
        matches: false,
        media: query,
        onchange: null,
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
        addListener: vi.fn(),
        removeListener: vi.fn(),
        dispatchEvent: vi.fn(),
      })),
    })
    window.history.pushState({}, '', '/')

    useChatStore.setState({
      threads: [],
      activeThreadId: null,
      pendingApproval: [],
      busy: false,
      error: null,
    })

    useUiStore.setState({
      leftSidebarOpen: true,
      leftSidebarWidth: 260,
      appMenuOpen: false,
      appMenuSearchFocused: false,
      contextDrawerOpen: false,
      commandPaletteOpen: false,
      shortcutsOverlayOpen: false,
    })

    useConfigStore.setState((state) => ({
      preferences: {
        ...state.preferences,
        shortcutOverlayEnabled: true,
        syncThemeWithSystem: false,
      },
    }))
  })

  it('starts directly in a reduced empty chat', async () => {
    render(<App />)

    expect(await screen.findByPlaceholderText('Next instruction...', undefined, { timeout: 10_000 })).toBeInTheDocument()
    expect(screen.getByText(/Getting started is available in the main menu/)).toBeInTheDocument()
    expect(screen.getByRole('button', { name: '+ New chat' })).toBeInTheDocument()
    expect(screen.queryByText('Plan your work')).not.toBeInTheDocument()
    expect(screen.queryByLabelText('Run context')).not.toBeInTheDocument()
  })

  it('keeps the German navigation entry point localized', async () => {
    await i18n.changeLanguage('de')
    window.history.pushState({}, '', '/settings')
    render(<App />)

    expect(await screen.findByText('Keine Projekte', undefined, { timeout: 10_000 })).toBeInTheDocument()
    const menu = await openMainMenu()
    fireEvent.click(within(menu).getByRole('button', { name: /^Aufgaben/ }))

    await waitFor(() => expect(window.location.pathname).toBe('/tasks'))
    expect(await screen.findByRole('heading', { name: 'Neue Aufgabe' }, { timeout: 5000 })).toBeInTheDocument()
  })

  it('removes permanent top navigation and exposes every route in the burger menu', async () => {
    render(<App />)
    await screen.findByPlaceholderText('Next instruction...')

    expect(screen.queryByRole('navigation')).not.toBeInTheDocument()
    const menu = await openMainMenu()
    for (const route of PRODUCT_ROUTES) {
      const button = within(menu).getByRole('button', { name: new RegExp(`^${i18n.t(route.navLabelKey)}`) })
      expect(button).toHaveAttribute('data-doc-id', route.navButtonDocId)
      expect(within(button).getByText(route.shortcut)).toBeInTheDocument()
    }
  })

  it('loads direct subsection URLs and uses the leaf title', async () => {
    window.history.pushState({}, '', '/features?tab=commands')
    render(<App />)

    expect(await screen.findByRole('heading', { name: 'Slash commands' })).toHaveClass('shell-title')
    expect(screen.queryByRole('tablist')).not.toBeInTheDocument()
  })

  it('loads the developer browser and GitHub URLs', async () => {
    window.history.pushState({}, '', '/browser')
    const { unmount } = render(<App />)
    expect(await screen.findByRole('heading', { name: 'Browser' })).toHaveClass('shell-title')
    expect(await screen.findByRole('tab', { name: 'CDP' })).toBeInTheDocument()

    unmount()
    window.history.pushState({}, '', '/github')
    render(<App />)
    expect(await screen.findByRole('heading', { name: 'GitHub' })).toHaveClass('shell-title')
    expect(await screen.findByRole('button', { name: 'Choose repository' })).toBeInTheDocument()
  })

  it('navigates between main and subsection routes through the burger menu', async () => {
    render(<App />)
    await screen.findByPlaceholderText('Next instruction...')

    await clickMenuRoute('Tasks')
    await waitFor(() => expect(window.location.pathname).toBe('/tasks'))
    expect(await screen.findByRole('heading', { name: 'New task' })).toBeInTheDocument()

    await clickMenuRoute('Settings')
    await waitFor(() => expect(window.location.pathname).toBe('/settings'))
    expect(await screen.findByText('AI & model', { selector: '.shell-title' }, { timeout: 5_000 })).toBeInTheDocument()
    expect(screen.getByRole('complementary', { name: 'Workspace sidebar' })).toBeInTheDocument()

    const menu = await openMainMenu()
    const settingsSections = within(menu).getByLabelText('Settings Sections')
    expect(within(settingsSections).getAllByRole('button')).toHaveLength(10)
    fireEvent.click(within(settingsSections).getByRole('button', { name: /^AI Sandbox/ }))
    await waitFor(() => expect(window.location.search).toBe('?section=sandbox'))
    expect(await screen.findByText('AI Sandbox', { selector: '.shell-title' })).toBeInTheDocument()
  })

  it('keeps number shortcuts mapped to the registered route order', async () => {
    render(<App />)
    await screen.findByPlaceholderText('Next instruction...')

    for (const route of PRODUCT_ROUTES.slice(1)) {
      fireEvent.keyDown(window, { key: route.shortcutKey, ctrlKey: true })
      await waitFor(() => expect(window.location.pathname).toBe(route.path), { timeout: 3000 })
    }
  })

  it('opens the burger search with Ctrl+K and keeps shortcut help inside it', async () => {
    render(<App />)
    await screen.findByPlaceholderText('Next instruction...')

    fireEvent.keyDown(window, { key: 'k', ctrlKey: true })
    const menu = await screen.findByRole('dialog', { name: 'Main menu' })
    const input = within(menu).getByRole('searchbox', { name: 'Search areas and commands' })
    await waitFor(() => expect(input).toHaveFocus())

    fireEvent.change(input, { target: { value: 'GitHub' } })
    expect(within(menu).getAllByRole('button', { name: /GitHub/ }).length).toBeGreaterThan(0)

    fireEvent.keyDown(window, { key: 'Escape' })
    await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Main menu' })).not.toBeInTheDocument())

    fireEvent.keyDown(window, { key: '?', ctrlKey: true, shiftKey: true })
    const shortcutMenu = await screen.findByRole('dialog', { name: 'Main menu' })
    expect(within(shortcutMenu).getByText('Ctrl Shift B')).toBeInTheDocument()
  })

  it('keeps shell drawers mutually exclusive and closes them with Escape and backdrop', async () => {
    render(<App />)
    await screen.findByPlaceholderText('Next instruction...')

    let menu = await openMainMenu()
    fireEvent.click(within(menu).getByRole('button', { name: /Context & status/ }))
    await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Main menu' })).not.toBeInTheDocument())
    expect(screen.getByLabelText('Run context')).toBeInTheDocument()

    fireEvent.click(screen.getByRole('button', { name: 'Open main menu' }))
    menu = await screen.findByRole('dialog', { name: 'Main menu' })
    expect(screen.queryByLabelText('Run context')).not.toBeInTheDocument()

    fireEvent.keyDown(menu, { key: 'Escape' })
    await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Main menu' })).not.toBeInTheDocument())

    menu = await openMainMenu()
    const backdrop = screen.getAllByRole('button', { name: 'Close menu' })
      .find((button) => button.classList.contains('app-drawer-backdrop'))
    expect(backdrop).toBeDefined()
    fireEvent.click(backdrop!)
    await waitFor(() => expect(menu).not.toBeInTheDocument())
  })

  it('resizes the project and chat sidebar within the new limits', async () => {
    render(<App />)

    const separator = await screen.findByRole('separator', { name: 'Resize sidebar' })
    fireEvent.pointerDown(separator, { pointerId: 7, button: 0, clientX: 260 })
    await waitFor(() => expect(document.body).toHaveClass('sidebar-resize-active'))
    fireEvent.pointerMove(window, { pointerId: 7, clientX: 430 })
    fireEvent.pointerUp(window, { pointerId: 7, clientX: 430 })

    expect(useUiStore.getState().leftSidebarWidth).toBe(360)
  })

  it('opens the project and chat sidebar from the burger menu below 900 px', async () => {
    vi.mocked(window.matchMedia).mockImplementation((query: string) => ({
      matches: query === '(max-width: 899px)',
      media: query,
      onchange: null,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      addListener: vi.fn(),
      removeListener: vi.fn(),
      dispatchEvent: vi.fn(),
    }))

    render(<App />)
    await screen.findByPlaceholderText('Next instruction...')
    expect(screen.queryByRole('complementary', { name: 'Workspace sidebar' })).not.toBeInTheDocument()

    const menu = await openMainMenu()
    fireEvent.click(within(menu).getByRole('button', { name: /Projects & chats/ }))
    expect(await screen.findByRole('complementary', { name: 'Workspace sidebar' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Close sidebar' })).toBeInTheDocument()

    fireEvent.keyDown(window, { key: 'Escape' })
    await waitFor(() => {
      expect(screen.queryByRole('complementary', { name: 'Workspace sidebar' })).not.toBeInTheDocument()
    })
  })
})
