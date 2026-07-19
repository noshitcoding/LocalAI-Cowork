import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, vi } from 'vitest'
import App from './App'
import i18n from './i18n'
import { PRODUCT_ROUTES } from './product/routeRegistry'
import { useChatStore } from './stores/chatStore'
import { useConfigStore } from './stores/configStore'
import { useUiStore } from './stores/uiStore'

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
      leftSidebarWidth: 320,
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

  it('starts directly in an empty chat', async () => {
    render(<App />)
    expect(await screen.findByPlaceholderText('Next instruction...', undefined, { timeout: 10_000 })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: '+ New chat' })).toBeInTheDocument()
  })

  it('keeps the German Cowork entry point consistently localized', async () => {
    await i18n.changeLanguage('de')
    render(<App />)

    expect(await screen.findByRole('button', { name: 'Projekte verwalten' }, { timeout: 10_000 })).toBeInTheDocument()
    expect(screen.getByText('Keine Projekte')).toBeInTheDocument()
    expect(await screen.findByRole('button', { name: 'Erstelle einen klaren Plan mit 5 Schritten für die aktuelle Aufgabe.' }, { timeout: 10_000 })).toBeInTheDocument()
    expect(await screen.findByRole('button', { name: 'Analysiere die letzten Änderungen und nenne Risiken.' }, { timeout: 10_000 })).toBeInTheDocument()
    expect(await screen.findByRole('button', { name: 'Formuliere die nächsten konkreten To-dos mit Priorität.' }, { timeout: 10_000 })).toBeInTheDocument()

    fireEvent.click(screen.getByRole('link', { name: 'Aufgaben' }))
    expect(await screen.findByRole('heading', { name: 'Aufgaben' })).toBeInTheDocument()
  })

  it('shows top navigation links', async () => {
    render(<App />)
    for (const route of PRODUCT_ROUTES) {
      const link = await screen.findByRole('link', { name: i18n.t(route.navLabelKey) })
      expect(link).toHaveAttribute('href', route.path)
      expect(link).toHaveAttribute('data-doc-id', route.navButtonDocId)
    }
  })

  it('loads direct feature URLs', async () => {
    window.history.pushState({}, '', '/features')
    render(<App />)
    expect(await screen.findByRole('heading', { name: 'Tools and knowledge' })).toBeInTheDocument()
  })

  it('loads the tasks page without crashing', async () => {
    render(<App />)
    fireEvent.click(await screen.findByRole('link', { name: 'Tasks' }, { timeout: 10_000 }))
    expect(await screen.findByRole('heading', { name: 'Tasks' }, { timeout: 10_000 })).toBeInTheDocument()
    expect(await screen.findByRole('heading', { name: 'New task' }, { timeout: 10_000 })).toBeInTheDocument()
  })

  it('uses a focused settings layout and restores the workspace sidebar on return', async () => {
    window.history.pushState({}, '', '/settings')
    render(<App />)

    expect(await screen.findByRole('heading', { name: 'AI & model' })).toBeInTheDocument()
    expect(screen.queryByRole('complementary', { name: 'Workspace sidebar' })).not.toBeInTheDocument()
    expect(useUiStore.getState().leftSidebarOpen).toBe(true)

    fireEvent.click(screen.getByRole('button', { name: 'Back to Cowork' }))

    await waitFor(() => expect(window.location.pathname).toBe('/'))
    expect(await screen.findByRole('complementary', { name: 'Workspace sidebar' })).toBeInTheDocument()
    expect(useUiStore.getState().leftSidebarOpen).toBe(true)
  })

  it('maps number shortcuts to the top navigation order', async () => {
    render(<App />)
    await screen.findByRole('link', { name: 'Cowork' })

    for (const route of PRODUCT_ROUTES.slice(1)) {
      fireEvent.keyDown(window, { key: route.shortcutKey, ctrlKey: true })
      await waitFor(() => expect(window.location.pathname).toBe(route.path), { timeout: 3000 })
    }
  })

  it('uses route registry labels in shortcuts and command palette navigation', async () => {
    render(<App />)
    await screen.findByRole('link', { name: 'Cowork' })

    fireEvent.keyDown(window, { key: '?', ctrlKey: true, shiftKey: true })
    const shortcutDialog = await screen.findByRole('dialog', { name: 'Shortcuts' })
    expect(shortcutDialog).toHaveAttribute('data-doc-id', 'element:/app/shortcut-overlay')
    for (const route of PRODUCT_ROUTES) {
      expect(within(shortcutDialog).getByText(i18n.t(route.shortcutLabelKey))).toBeInTheDocument()
      expect(within(shortcutDialog).getByText(route.shortcut)).toBeInTheDocument()
    }
    fireEvent.click(within(shortcutDialog).getByRole('button', { name: 'Esc' }))

    fireEvent.keyDown(window, { key: 'k', ctrlKey: true })
    const input = await screen.findByPlaceholderText('Search command or / for commands...')
    for (const route of PRODUCT_ROUTES) {
      fireEvent.change(input, { target: { value: route.commandLabel } })
      await waitFor(() => {
        expect(screen.getAllByText(route.commandLabel).length).toBeGreaterThan(0)
      })
    }
  })

  it('resizes the left sidebar from the separator', async () => {
    render(<App />)

    const separator = await screen.findByRole('separator', { name: 'Resize sidebar' })
    fireEvent.pointerDown(separator, { pointerId: 7, button: 0, clientX: 320 })

    await waitFor(() => expect(document.body).toHaveClass('sidebar-resize-active'))

    fireEvent.pointerMove(window, { pointerId: 7, clientX: 430 })
    fireEvent.pointerUp(window, { pointerId: 7, clientX: 430 })

    expect(useUiStore.getState().leftSidebarWidth).toBe(430)
  })

  it('uses an accessible drawer instead of a clipped sidebar on compact desktop windows', async () => {
    vi.mocked(window.matchMedia).mockImplementation((query: string) => ({
      matches: query === '(max-width: 900px)',
      media: query,
      onchange: null,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      addListener: vi.fn(),
      removeListener: vi.fn(),
      dispatchEvent: vi.fn(),
    }))

    render(<App />)

    const toggle = await screen.findByRole('button', { name: 'Toggle sidebar' })
    expect(toggle).toHaveAttribute('aria-expanded', 'false')
    expect(screen.queryByRole('complementary', { name: 'Workspace sidebar' })).not.toBeInTheDocument()

    fireEvent.click(toggle)
    expect(await screen.findByRole('complementary', { name: 'Workspace sidebar' })).toBeInTheDocument()
    expect(toggle).toHaveAttribute('aria-expanded', 'true')
    expect(screen.getByRole('button', { name: 'Close sidebar' })).toBeInTheDocument()

    fireEvent.keyDown(window, { key: 'Escape' })
    await waitFor(() => {
      expect(screen.queryByRole('complementary', { name: 'Workspace sidebar' })).not.toBeInTheDocument()
    })
  })
})
