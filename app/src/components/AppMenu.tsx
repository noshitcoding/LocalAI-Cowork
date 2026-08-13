import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react'
import { useLocation, useNavigate } from 'react-router-dom'
import {
  Blocks,
  Command,
  Download,
  FolderKanban,
  Globe2,
  GitPullRequest,
  Keyboard,
  ListTodo,
  MessagesSquare,
  Moon,
  PanelLeft,
  PanelRight,
  Search,
  Server,
  Settings2,
  Sparkles,
  Sun,
  UsersRound,
  X,
} from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { PRODUCT_ROUTES, type ProductRoute, type ProductSubroute } from '../product/routeRegistry'
import { useCommandRegistry } from '../stores/commandRegistryStore'
import { useUiStore } from '../stores/uiStore'
import { tr } from '../i18n'
import LanguageSwitcher from './LanguageSwitcher'
import {
  appUpdateProgressPercent,
  checkForAppUpdate,
  getAppUpdateSnapshot,
  installAvailableAppUpdate,
  startAutomaticUpdateCheck,
  subscribeAppUpdater,
} from '../utils/appUpdater'

const PRODUCT_ROUTE_ICONS = {
  cowork: MessagesSquare,
  tasks: ListTodo,
  crew: UsersRound,
  projects: FolderKanban,
  features: Blocks,
  browser: Globe2,
  github: GitPullRequest,
  server: Server,
  settings: Settings2,
} as const

type AppMenuProps = {
  open: boolean
  compactSidebar: boolean
  onOpenWorkspaceSidebar: () => void
  onClose: () => void
}

function routeSubroutes(route: ProductRoute): readonly ProductSubroute[] {
  return 'subroutes' in route && Array.isArray(route.subroutes) ? route.subroutes : []
}

function buildSubroutePath(route: ProductRoute, subroute: ProductSubroute): string {
  const params = new URLSearchParams()
  if (!subroute.default) params.set(subroute.queryKey, subroute.queryValue)
  const query = params.toString()
  return query ? `${route.path}?${query}` : route.path
}

function normalizeSearch(value: string): string {
  return value.trim().toLocaleLowerCase()
}

export default function AppMenu({
  open,
  compactSidebar,
  onOpenWorkspaceSidebar,
  onClose,
}: AppMenuProps) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const location = useLocation()
  const drawerRef = useRef<HTMLElement | null>(null)
  const searchRef = useRef<HTMLInputElement | null>(null)
  const scrollRef = useRef<HTMLDivElement | null>(null)
  const [query, setQuery] = useState('')
  const registeredCommands = useCommandRegistry((state) => state.commands)
  const executeCommand = useCommandRegistry((state) => state.executeCommand)
  const updateState = useSyncExternalStore(
    subscribeAppUpdater,
    getAppUpdateSnapshot,
    getAppUpdateSnapshot,
  )
  const updateProgress = appUpdateProgressPercent(updateState)
  const {
    theme,
    appMenuSearchFocused,
    shortcutsOverlayOpen,
    setActiveMode,
    setAppMenuSearchFocused,
    setContextDrawerOpen,
    setShortcutsOverlayOpen,
    toggleTheme,
  } = useUiStore()

  const activeRoute = PRODUCT_ROUTES.find((route) => route.path === location.pathname) ?? PRODUCT_ROUTES[0]
  const activeSubroutes = routeSubroutes(activeRoute)
  const activeParams = useMemo(() => new URLSearchParams(location.search), [location.search])
  const normalizedQuery = normalizeSearch(query)

  useEffect(() => {
    startAutomaticUpdateCheck()
  }, [])

  const matchingRoutes = useMemo(() => {
    if (!normalizedQuery) return []
    return PRODUCT_ROUTES.flatMap((route) => {
      const parentMatch = normalizeSearch(`${t(route.navLabelKey)} ${route.commandLabel}`).includes(normalizedQuery)
      const children = routeSubroutes(route)
        .filter((subroute) => normalizeSearch(`${tr(subroute.labelKey)} ${tr(subroute.descriptionKey ?? '')}`).includes(normalizedQuery))
        .map((subroute) => ({
          id: `${route.id}:${subroute.id}`,
          label: tr(subroute.labelKey),
          description: t(route.navLabelKey),
          path: buildSubroutePath(route, subroute),
          route,
        }))
      return [
        ...(parentMatch ? [{
          id: route.id,
          label: t(route.navLabelKey),
          description: route.commandLabel,
          path: route.path,
          route,
        }] : []),
        ...children,
      ]
    })
  }, [normalizedQuery, t])

  const matchingCommands = useMemo(() => {
    if (!normalizedQuery) return []
    const commandQuery = normalizedQuery.replace(/^\//, '')
    return registeredCommands.filter((command) => (
      command.command.toLocaleLowerCase().includes(normalizedQuery)
      || tr(command.label).toLocaleLowerCase().includes(commandQuery)
      || tr(command.description).toLocaleLowerCase().includes(commandQuery)
    )).slice(0, 12)
  }, [normalizedQuery, registeredCommands])

  useEffect(() => {
    if (!open) {
      setQuery('')
      return
    }
    window.requestAnimationFrame(() => {
      if (appMenuSearchFocused) {
        searchRef.current?.focus()
        setAppMenuSearchFocused(false)
      } else {
        searchRef.current?.focus()
      }
    })
  }, [appMenuSearchFocused, open, setAppMenuSearchFocused])

  useEffect(() => {
    if (!open) return
    const previousOverflow = document.body.style.overflow
    document.body.style.overflow = 'hidden'
    return () => {
      document.body.style.overflow = previousOverflow
    }
  }, [open])

  const navigateTo = (route: ProductRoute, path: string = route.path) => {
    if (route.activeMode) setActiveMode(route.activeMode)
    setQuery('')
    navigate(path)
    window.requestAnimationFrame(() => scrollRef.current?.scrollTo?.({ top: 0 }))
  }

  const handleDrawerKeyDown = (event: React.KeyboardEvent<HTMLElement>) => {
    if (event.key !== 'Tab') return
    const focusable = drawerRef.current?.querySelectorAll<HTMLElement>(
      'button:not(:disabled), input:not(:disabled), select:not(:disabled), summary, [href], [tabindex]:not([tabindex="-1"])',
    )
    if (!focusable?.length) return
    const first = focusable[0]
    const last = focusable[focusable.length - 1]
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault()
      last.focus()
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault()
      first.focus()
    }
  }

  if (!open) return null

  const renderRoute = (route: ProductRoute) => {
    const Icon = PRODUCT_ROUTE_ICONS[route.id as keyof typeof PRODUCT_ROUTE_ICONS]
    const active = activeRoute.id === route.id

    return (
      <div key={route.id} className="app-menu-route-group">
        <button
          type="button"
          className={`app-menu-route${active ? ' active' : ''}`}
          data-doc-id={route.navButtonDocId}
          aria-current={active ? 'page' : undefined}
          onClick={() => navigateTo(route)}
        >
          <Icon size={16} strokeWidth={1.8} aria-hidden="true" />
          <span>{t(route.navLabelKey)}</span>
          <kbd>{route.shortcut}</kbd>
        </button>
      </div>
    )
  }

  return (
    <div className="app-menu-layer">
      <div className="app-drawer-backdrop" data-testid="app-menu-backdrop" aria-hidden="true" />
      <aside
        ref={drawerRef}
        id="app-menu-drawer"
        className="app-menu-drawer"
        role="dialog"
        aria-modal="true"
        aria-label={tr('Main menu')}
        onKeyDown={handleDrawerKeyDown}
      >
        <header className="app-menu-header">
          <strong>{tr('Workspace')}</strong>
          <button type="button" onClick={onClose} aria-label={tr('Close menu')}>
            <X size={17} aria-hidden="true" />
          </button>
        </header>

        <label className="app-menu-search">
          <Search size={15} aria-hidden="true" />
          <input
            ref={searchRef}
            type="search"
            value={query}
            onChange={(event) => setQuery(event.currentTarget.value)}
            placeholder={tr('Search areas and commands...')}
            aria-label={tr('Search areas and commands')}
          />
          <kbd>Ctrl K</kbd>
        </label>

        <div ref={scrollRef} className="app-menu-scroll">
          {normalizedQuery ? (
            <section className="app-menu-search-results" aria-label={tr('Search results')}>
              {matchingRoutes.map((result) => (
                <button key={result.id} type="button" onClick={() => navigateTo(result.route, result.path)}>
                  <Search size={14} aria-hidden="true" />
                  <span><strong>{result.label}</strong><small>{result.description}</small></span>
                </button>
              ))}
              {matchingCommands.map((command) => (
                <button
                  key={command.id}
                  type="button"
                  onClick={() => {
                    void executeCommand(command.id)
                    setQuery('')
                  }}
                >
                  <Command size={14} aria-hidden="true" />
                  <span><strong>{command.command || tr(command.label)}</strong><small>{tr(command.description)}</small></span>
                </button>
              ))}
              {matchingRoutes.length === 0 && matchingCommands.length === 0 && (
                <p className="app-menu-empty">{tr('No results for')} “{query}”</p>
              )}
            </section>
          ) : (
            <>
              {activeSubroutes.length > 0 && (
                <section className="app-menu-section app-menu-context-section" aria-labelledby="app-menu-current-sections">
                  <h2 id="app-menu-current-sections">{t(activeRoute.navLabelKey)} · {tr('Sections')}</h2>
                  <div className="app-menu-subroutes app-menu-context-subroutes" aria-label={`${t(activeRoute.navLabelKey)} ${tr('Sections')}`}>
                    {activeSubroutes.map((subroute) => {
                      const selectedValue = activeParams.get(subroute.queryKey)
                      const selected = selectedValue === subroute.queryValue || (!selectedValue && subroute.default)
                      return (
                        <button
                          key={subroute.id}
                          type="button"
                          className={selected ? 'active' : undefined}
                          aria-current={selected ? 'page' : undefined}
                          onClick={() => navigateTo(activeRoute, buildSubroutePath(activeRoute, subroute))}
                        >
                          <span>{tr(subroute.labelKey)}</span>
                          {subroute.descriptionKey ? <small>{tr(subroute.descriptionKey)}</small> : null}
                        </button>
                      )
                    })}
                  </div>
                </section>
              )}
              <section className="app-menu-section" aria-labelledby="app-menu-workspace">
                <h2 id="app-menu-workspace">{tr('Workspace')}</h2>
                {PRODUCT_ROUTES.filter((route) => route.group === 'workspace').map(renderRoute)}
              </section>
              <section className="app-menu-section" aria-labelledby="app-menu-development">
                <h2 id="app-menu-development">{tr('Development')}</h2>
                {PRODUCT_ROUTES.filter((route) => route.group === 'development').map(renderRoute)}
              </section>
              <section className="app-menu-section" aria-labelledby="app-menu-system">
                <h2 id="app-menu-system">{tr('System')}</h2>
                {PRODUCT_ROUTES.filter((route) => route.group === 'system').map(renderRoute)}
              </section>

              <section className="app-menu-section" aria-labelledby="app-menu-workspace-tools">
                <h2 id="app-menu-workspace-tools">{tr('Workspace tools')}</h2>
                {updateState.phase !== 'unsupported' && (
                  <button
                    type="button"
                    className={`app-menu-utility app-menu-update is-${updateState.phase}`}
                    onClick={() => {
                      if (updateState.phase === 'available') {
                        void installAvailableAppUpdate()
                      } else {
                        void checkForAppUpdate()
                      }
                    }}
                    disabled={['checking', 'backing-up', 'downloading', 'installing', 'restarting'].includes(updateState.phase)}
                    aria-live="polite"
                  >
                    <Download size={16} aria-hidden="true" />
                    <span>
                      <strong>
                        {updateState.phase === 'available'
                          ? tr('Install update {{version}}', { version: updateState.availableVersion ?? '' })
                          : updateState.phase === 'checking'
                            ? tr('Checking for updates...')
                            : updateState.phase === 'backing-up'
                              ? tr('Backing up workspace...')
                              : updateState.phase === 'downloading'
                                ? tr('Downloading update... {{progress}}', { progress: updateProgress === null ? '' : `${updateProgress}%` })
                                : updateState.phase === 'installing'
                                  ? tr('Installing update...')
                                  : updateState.phase === 'restarting'
                                    ? tr('Restarting...')
                                    : updateState.phase === 'up-to-date'
                                      ? tr('Version {{version}} is current', { version: updateState.currentVersion ?? '' })
                                      : updateState.phase === 'error'
                                        ? tr('Update failed — try again')
                                        : tr('Check for updates')}
                      </strong>
                      <small>
                        {updateState.phase === 'available'
                          ? tr('One click installs the signed update and restarts the app.')
                          : updateState.phase === 'error'
                            ? tr('No update was installed. Your workspace is unchanged.')
                            : tr('Signed updates from GitHub Releases')}
                      </small>
                      {updateState.phase === 'downloading' && updateProgress !== null ? (
                        <span className="app-menu-update-progress" aria-hidden="true">
                          <span style={{ width: `${updateProgress}%` }} />
                        </span>
                      ) : null}
                    </span>
                  </button>
                )}
                <button
                  type="button"
                  className="app-menu-utility"
                  onClick={() => setContextDrawerOpen(true, true)}
                >
                  <PanelRight size={16} aria-hidden="true" />
                  <span><strong>{tr('Context & status')}</strong><small>{tr('Metrics, runs, tools and outputs')}</small></span>
                </button>
                {(compactSidebar || !useUiStore.getState().leftSidebarOpen) && (
                  <button
                    type="button"
                    className="app-menu-utility"
                    onClick={() => {
                      onOpenWorkspaceSidebar()
                    }}
                  >
                    <PanelLeft size={16} aria-hidden="true" />
                    <span><strong>{tr('Projects & chats')}</strong><small>{tr('Open workspace sidebar')}</small></span>
                  </button>
                )}
                <details className="app-menu-details">
                  <summary><Sparkles size={16} aria-hidden="true" />{tr('Getting started')}</summary>
                  <ol>
                    <li>{tr('Choose a model in the chat controls.')}</li>
                    <li>{tr('Connect a folder or add files when needed.')}</li>
                    <li>{tr('Describe the outcome and send your instruction.')}</li>
                  </ol>
                </details>
                <details
                  className="app-menu-details"
                  open={shortcutsOverlayOpen}
                  onToggle={(event) => setShortcutsOverlayOpen(event.currentTarget.open)}
                >
                  <summary><Keyboard size={16} aria-hidden="true" />{tr('Shortcuts')}</summary>
                  <dl className="app-menu-shortcuts">
                    <div><dt>{tr('Search')}</dt><dd>Ctrl K</dd></div>
                    <div><dt>{tr('Projects & chats')}</dt><dd>Ctrl Shift B</dd></div>
                    <div><dt>{tr('Theme')}</dt><dd>Ctrl Shift L</dd></div>
                    <div><dt>{tr('Shortcut overview')}</dt><dd>Ctrl Shift ?</dd></div>
                  </dl>
                </details>
              </section>
            </>
          )}
        </div>

        <footer className="app-menu-footer">
          <LanguageSwitcher />
          <button type="button" onClick={toggleTheme}>
            {theme === 'light' ? <Moon size={16} aria-hidden="true" /> : <Sun size={16} aria-hidden="true" />}
            <span>{theme === 'light' ? tr('Dark theme') : tr('Light theme')}</span>
          </button>
        </footer>
      </aside>
    </div>
  )
}
