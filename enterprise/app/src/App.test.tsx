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
  return screen.findByRole('dialog', undefined, { timeout: 5_000 })
}

async function clickMenuRoute(label: string) {
  const menu = await openMainMenu()
  fireEvent.click(within(menu).getByRole('button', { name: new RegExp(`^${label}`) }))
}

describe('App', () => {
  beforeEach(async () => {
    delete (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__
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

  it('keeps attachments visible behind the plus and expands chat settings on demand', async () => {
    render(<App />)
    await screen.findByPlaceholderText('Next instruction...', undefined, { timeout: 10_000 })

    const settingsToggle = screen.getByRole('button', { name: 'Chat settings' })
    const attachmentToggle = screen.getByRole('button', { name: 'Add files or folders' })
    expect(settingsToggle).toHaveAttribute('aria-expanded', 'false')
    expect(screen.queryByRole('region', { name: 'Chat settings' })).not.toBeInTheDocument()

    fireEvent.click(attachmentToggle)
    const attachmentMenu = screen.getByRole('menu', { name: 'Add files or folders' })
    expect(within(attachmentMenu).getByRole('menuitem', { name: 'Files' })).toBeInTheDocument()
    expect(within(attachmentMenu).getByRole('menuitem', { name: 'Folder' })).toBeInTheDocument()

    fireEvent.click(settingsToggle)
    expect(screen.queryByRole('menu', { name: 'Add files or folders' })).not.toBeInTheDocument()
    const settingsPanel = screen.getByRole('region', { name: 'Chat settings' })
    expect(within(settingsPanel).getByRole('group', { name: 'Runner' })).toBeInTheDocument()
    const fileAccess = within(settingsPanel).getByRole('combobox', { name: 'Access for new files and folders' })
    expect(fileAccess).toHaveTextContent('File access: read only')

    fireEvent.click(fileAccess)
    const fileAccessOptions = within(settingsPanel).getByRole('listbox', { name: 'Access for new files and folders' })
    expect(within(fileAccessOptions).getByRole('option', { name: 'File access: read only' })).toBeInTheDocument()
    expect(within(fileAccessOptions).getByRole('option', { name: 'File access: read and edit' })).toBeInTheDocument()

    fireEvent.keyDown(fileAccess, { key: 'Escape' })
    expect(within(settingsPanel).queryByRole('listbox', { name: 'Access for new files and folders' })).not.toBeInTheDocument()
    expect(screen.getByRole('region', { name: 'Chat settings' })).toBeInTheDocument()

    fireEvent.keyDown(document, { key: 'Escape' })
    expect(screen.queryByRole('region', { name: 'Chat settings' })).not.toBeInTheDocument()
  })

  it('keeps the German navigation entry point localized', async () => {
    await i18n.changeLanguage('de')
    window.history.pushState({}, '', '/settings')
    render(<App />)

    expect(await screen.findByLabelText('KI & Modell', { selector: '.settings-view' }, { timeout: 10_000 })).toBeInTheDocument()
    const menu = await openMainMenu()
    fireEvent.click(within(menu).getByRole('button', { name: /^Aufgaben/ }))

    await waitFor(() => expect(window.location.pathname).toBe('/tasks'))
    expect(await screen.findByRole('heading', { name: 'Neue Aufgabe' }, { timeout: 5000 })).toBeInTheDocument()
  }, 15_000)

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
    expect(screen.getByRole('dialog', { name: 'Main menu' })).toBeInTheDocument()

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
    expect(screen.getByRole('dialog', { name: 'Main menu' })).toBeInTheDocument()

    const crewButton = within(menu).getByRole('button', { name: /^Crew/ })
    fireEvent.click(crewButton)
    await waitFor(() => expect(window.location.pathname).toBe('/crew'))
    await waitFor(() => expect(menu.querySelector('[data-doc-id="button:/app/top-navigation/crew"]')).toHaveAttribute('aria-current', 'page'))
    const crewSections = within(menu).getByLabelText('Crew Sections')
    const crewMembersButton = within(crewSections).getByRole('button', { name: /^Crew members/ })
    fireEvent.click(crewMembersButton)
    await waitFor(() => expect(window.location.search).toBe('?section=members'))
    await waitFor(() => expect(within(crewSections).getByRole('button', { name: /^Crew members/ })).toHaveAttribute('aria-current', 'page'))
    expect(await screen.findByText('Crew members', { selector: '.shell-title' })).toBeInTheDocument()
    expect(screen.getByRole('dialog', { name: 'Main menu' })).toBeInTheDocument()
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
    expect(screen.getByRole('dialog', { name: 'Main menu' })).toBeInTheDocument()

    fireEvent.click(within(menu).getByRole('button', { name: 'Close menu' }))
    await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Main menu' })).not.toBeInTheDocument())

    fireEvent.keyDown(window, { key: '?', ctrlKey: true, shiftKey: true })
    const shortcutMenu = await screen.findByRole('dialog', { name: 'Main menu' })
    expect(within(shortcutMenu).getByText('Ctrl Shift B')).toBeInTheDocument()
  })

  it('keeps the burger menu open until its close button is used', async () => {
    render(<App />)
    await screen.findByPlaceholderText('Next instruction...')

    const menuButton = screen.getByRole('button', { name: 'Open main menu' })
    const menu = await openMainMenu()
    fireEvent.click(within(menu).getByRole('button', { name: /Context & status/ }))
    expect(screen.getByRole('dialog', { name: 'Main menu' })).toBeInTheDocument()
    expect(screen.getByLabelText('Run context')).toBeInTheDocument()

    fireEvent.click(menuButton)
    expect(screen.getByRole('dialog', { name: 'Main menu' })).toBeInTheDocument()

    fireEvent.keyDown(menu, { key: 'Escape' })
    expect(screen.getByRole('dialog', { name: 'Main menu' })).toBeInTheDocument()

    expect(screen.queryByTestId('app-menu-backdrop')).not.toBeInTheDocument()
    screen.getByRole('textbox', { name: 'Message input' }).focus()
    expect(screen.getByRole('textbox', { name: 'Message input' })).toHaveFocus()
    expect(screen.getByRole('dialog', { name: 'Main menu' })).toBeInTheDocument()

    fireEvent.click(within(menu).getByRole('button', { name: 'Close menu' }))
    await waitFor(() => expect(menu).not.toBeInTheDocument())
    await waitFor(() => expect(menuButton).toHaveFocus())
    expect(screen.getByLabelText('Run context')).toBeInTheDocument()
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
    expect(screen.queryByRole('dialog', { name: 'Main menu' })).not.toBeInTheDocument()

    fireEvent.keyDown(window, { key: 'Escape' })
    await waitFor(() => {
      expect(screen.queryByRole('complementary', { name: 'Workspace sidebar' })).not.toBeInTheDocument()
    })
  })

  it('closes the compact modal on route, subsection and search selection and focuses the destination', async () => {
    vi.mocked(window.matchMedia).mockImplementation((query: string) => ({
      matches: query === '(max-width: 899px)', media: query, onchange: null,
      addEventListener: vi.fn(), removeEventListener: vi.fn(), addListener: vi.fn(), removeListener: vi.fn(), dispatchEvent: vi.fn(),
    }))
    window.history.pushState({}, '', '/settings')
    render(<App />)
    await screen.findByText('AI & model', { selector: '.shell-title' })
    let menu = await openMainMenu()
    expect(menu).toHaveAttribute('aria-modal', 'true')
    fireEvent.click(within(menu).getByRole('button', { name: /^AI Sandbox/ }))
    await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Main menu' })).not.toBeInTheDocument())
    await waitFor(() => expect(screen.getByText('AI Sandbox', { selector: '.shell-title' })).toHaveFocus())

    menu = await openMainMenu()
    fireEvent.change(within(menu).getByRole('searchbox'), { target: { value: 'Browser' } })
    fireEvent.click(within(menu).getAllByRole('button', { name: /Browser/ })[0])
    await waitFor(() => expect(window.location.pathname).toBe('/browser'))
    expect(screen.queryByRole('dialog', { name: 'Main menu' })).not.toBeInTheDocument()

    menu = await openMainMenu()
    fireEvent.click(within(menu).getByRole('button', { name: /^Tasks/ }))
    await waitFor(() => expect(window.location.pathname).toBe('/tasks'))
    await waitFor(() => expect(screen.getByText('Tasks', { selector: '.shell-title' })).toHaveFocus())
  }, 15_000)

  it('contains compact-menu focus and restores the trigger after Escape', async () => {
    vi.mocked(window.matchMedia).mockImplementation((query: string) => ({
      matches: query === '(max-width: 899px)', media: query, onchange: null,
      addEventListener: vi.fn(), removeEventListener: vi.fn(), addListener: vi.fn(), removeListener: vi.fn(), dispatchEvent: vi.fn(),
    }))
    render(<App />)
    await screen.findByPlaceholderText('Next instruction...')
    const trigger = screen.getByRole('button', { name: 'Open main menu' })
    const menu = await openMainMenu()
    await waitFor(() => expect(within(menu).getByRole('searchbox')).toHaveFocus())
    const close = within(menu).getByRole('button', { name: 'Close menu' })
    close.focus()
    fireEvent.keyDown(close, { key: 'Tab', shiftKey: true })
    expect(within(menu).getByRole('button', { name: /Dark theme|Light theme/ })).toHaveFocus()
    fireEvent.keyDown(document.activeElement!, { key: 'Tab' })
    expect(close).toHaveFocus()
    fireEvent.keyDown(window, { key: 'Escape' })
    await waitFor(() => expect(menu).not.toBeInTheDocument())
    await waitFor(() => expect(trigger).toHaveFocus())
  })
})
