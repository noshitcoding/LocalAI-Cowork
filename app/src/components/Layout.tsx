import { Suspense, useCallback, useEffect, useRef, useState } from 'react'
import type { CSSProperties, KeyboardEvent as ReactKeyboardEvent, PointerEvent as ReactPointerEvent } from 'react'
import { Outlet, NavLink, useLocation, useNavigate } from 'react-router-dom'
import {
  ArrowLeft,
  Blocks,
  Command,
  FolderKanban,
  ListTodo,
  Menu,
  MessagesSquare,
  Moon,
  PanelsTopLeft,
  Settings2,
  Sun,
  UsersRound,
} from 'lucide-react'
import { useTranslation } from 'react-i18next'
import {
  LEFT_SIDEBAR_MAX_WIDTH,
  LEFT_SIDEBAR_MIN_WIDTH,
  clampLeftSidebarWidth,
  useUiStore,
} from '../stores/uiStore'
import { useConfigStore } from '../stores/configStore'
import LeftSidebar from './LeftSidebar'
import CommandPalette from './CommandPalette'
import LanguageSwitcher from './LanguageSwitcher'
import { tr } from '../i18n'
import { PRODUCT_ROUTES, getProductRouteById, getProductRouteByShortcutKey, type ProductRoute } from '../product/routeRegistry'

const COMPACT_SIDEBAR_MEDIA_QUERY = '(max-width: 900px)'

const PRODUCT_ROUTE_ICONS = {
  cowork: MessagesSquare,
  tasks: ListTodo,
  crew: UsersRound,
  projects: FolderKanban,
  features: Blocks,
  settings: Settings2,
} as const

function ViewLoadingState() {
  const { t } = useTranslation()

  return (
    <div className="view-loading-state" aria-busy="true" aria-live="polite">
      <div className="view-loading-bar" aria-hidden="true">
        <span />
      </div>
      <span>{t('common.preparingView')}</span>
    </div>
  )
}

export default function Layout() {
  const { t, i18n } = useTranslation()
  const navigate = useNavigate()
  const location = useLocation()
  const settingsFocused = location.pathname === getProductRouteById('settings').path
  
  const {
    leftSidebarOpen,
    leftSidebarWidth,
    theme,
    toggleLeftSidebar,
    setLeftSidebarWidth,
    toggleTheme,
    setCommandPaletteOpen,
    commandPaletteOpen,
    shortcutsOverlayOpen,
    setShortcutsOverlayOpen,
    setActiveMode,
  } = useUiStore()
  
  const focusMode = useConfigStore((s) => s.preferences.focusMode)
  const shortcutOverlayEnabled = useConfigStore((s) => s.preferences.shortcutOverlayEnabled)
  const leftSidebarFrameRef = useRef<HTMLDivElement | null>(null)
  const leftSidebarResizeRef = useRef<{ pointerId: number; left: number } | null>(null)
  const [leftSidebarResizing, setLeftSidebarResizing] = useState(false)
  const [compactSidebar, setCompactSidebar] = useState(() => (
    typeof window !== 'undefined' && window.matchMedia?.(COMPACT_SIDEBAR_MEDIA_QUERY).matches === true
  ))
  const [compactSidebarOpen, setCompactSidebarOpen] = useState(false)
  const resolvedLeftSidebarWidth = clampLeftSidebarWidth(leftSidebarWidth)
  const resolvedLeftSidebarOpen = compactSidebar ? compactSidebarOpen : leftSidebarOpen
  const workspaceSidebarVisible = resolvedLeftSidebarOpen && !focusMode && !settingsFocused
  const leftSidebarFrameStyle = {
    '--left-sidebar-width': `${resolvedLeftSidebarWidth}px`,
  } as CSSProperties

  const navigateToProductRoute = useCallback((route: ProductRoute) => {
    if (route.activeMode) {
      setActiveMode(route.activeMode)
    }
    navigate(route.path)
  }, [navigate, setActiveMode])

  const handleToggleLeftSidebar = useCallback(() => {
    if (settingsFocused) return
    if (compactSidebar) {
      setCompactSidebarOpen((open) => !open)
      return
    }
    toggleLeftSidebar()
  }, [compactSidebar, settingsFocused, toggleLeftSidebar])

  useEffect(() => {
    if (typeof window === 'undefined' || !window.matchMedia) return undefined

    const mediaQuery = window.matchMedia(COMPACT_SIDEBAR_MEDIA_QUERY)
    const handleChange = (event: MediaQueryListEvent) => {
      setCompactSidebar(event.matches)
      if (event.matches) setCompactSidebarOpen(false)
    }

    mediaQuery.addEventListener('change', handleChange)
    return () => mediaQuery.removeEventListener('change', handleChange)
  }, [])

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const modifierPressed = event.ctrlKey || event.metaKey
      if (!modifierPressed) {
        if (event.key === 'Escape' && commandPaletteOpen) {
          setCommandPaletteOpen(false)
        }
        if (event.key === 'Escape' && compactSidebarOpen) {
          setCompactSidebarOpen(false)
        }
        return
      }

      if (event.key.toLowerCase() === 'k') {
        event.preventDefault()
        setCommandPaletteOpen(true)
      }

      if (event.shiftKey && event.key.toLowerCase() === 'b') {
        event.preventDefault()
        handleToggleLeftSidebar()
      }

      const shortcutRoute = event.shiftKey ? undefined : getProductRouteByShortcutKey(event.key)
      if (shortcutRoute) {
        event.preventDefault()
        navigateToProductRoute(shortcutRoute)
      }

      if (event.shiftKey && event.key.toLowerCase() === 'l') {
        event.preventDefault()
        toggleTheme()
      }

      if (event.shiftKey && event.key === '?') {
        event.preventDefault()
        if (shortcutOverlayEnabled) {
          setShortcutsOverlayOpen(true)
        }
      }
    }

    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [commandPaletteOpen, compactSidebarOpen, handleToggleLeftSidebar, navigateToProductRoute, setCommandPaletteOpen, setShortcutsOverlayOpen, shortcutOverlayEnabled, toggleTheme])

  useEffect(() => {
    if (!leftSidebarResizing) return undefined

    const handlePointerMove = (event: PointerEvent) => {
      const resize = leftSidebarResizeRef.current
      if (!resize || event.pointerId !== resize.pointerId) return

      event.preventDefault()
      setLeftSidebarWidth(event.clientX - resize.left)
    }

    const finishResize = (event: PointerEvent) => {
      const resize = leftSidebarResizeRef.current
      if (resize && event.pointerId !== resize.pointerId) return

      leftSidebarResizeRef.current = null
      setLeftSidebarResizing(false)
    }

    document.body.classList.add('sidebar-resize-active')
    window.addEventListener('pointermove', handlePointerMove, { passive: false })
    window.addEventListener('pointerup', finishResize)
    window.addEventListener('pointercancel', finishResize)
    return () => {
      document.body.classList.remove('sidebar-resize-active')
      window.removeEventListener('pointermove', handlePointerMove)
      window.removeEventListener('pointerup', finishResize)
      window.removeEventListener('pointercancel', finishResize)
    }
  }, [leftSidebarResizing, setLeftSidebarWidth])

  const handleLeftSidebarResizePointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return

    const rect = leftSidebarFrameRef.current?.getBoundingClientRect()
    if (!rect) return

    event.preventDefault()
    event.stopPropagation()
    leftSidebarResizeRef.current = { pointerId: event.pointerId, left: rect.left }
    setLeftSidebarResizing(true)
    setLeftSidebarWidth(event.clientX - rect.left)
  }

  const handleLeftSidebarResizeKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    const step = event.shiftKey ? 32 : 16
    let nextWidth: number | null = null

    if (event.key === 'ArrowLeft') nextWidth = resolvedLeftSidebarWidth - step
    if (event.key === 'ArrowRight') nextWidth = resolvedLeftSidebarWidth + step
    if (event.key === 'Home') nextWidth = LEFT_SIDEBAR_MIN_WIDTH
    if (event.key === 'End') nextWidth = LEFT_SIDEBAR_MAX_WIDTH

    if (nextWidth === null) return
    event.preventDefault()
    setLeftSidebarWidth(nextWidth)
  }

  const shortcuts = [
    { label: t('shortcuts.commandPalette'), keys: 'Ctrl+K' },
    ...PRODUCT_ROUTES.map((route) => ({
      label: t(route.shortcutLabelKey),
      keys: route.shortcut,
    })),
    { label: t('shortcuts.sidebar'), keys: 'Ctrl+Shift+B' },
    { label: t('shortcuts.theme'), keys: 'Ctrl+Shift+L' },
    { label: t('shortcuts.show'), keys: 'Ctrl+Shift+?' },
  ]

  return (
    <div className="app-shell" data-doc-id="element:/app/shell">
      <div className="top-bar">
        <div className="top-bar-brand">
          {settingsFocused ? (
            <button
              type="button"
              className="top-icon-button"
              data-doc-id="button:/app/shell/back-to-cowork"
              onClick={() => navigateToProductRoute(getProductRouteById('cowork'))}
              title={t('layout.backToCowork')}
              aria-label={t('layout.backToCowork')}
            >
              <ArrowLeft size={17} strokeWidth={2} />
            </button>
          ) : (
            <button
              type="button"
              className="top-icon-button"
              data-doc-id="button:/app/shell/toggle-sidebar"
              onClick={handleToggleLeftSidebar}
              title={t('layout.sidebarShortcut')}
              aria-label={t('layout.toggleSidebar')}
              aria-controls="workspace-sidebar-frame"
              aria-expanded={workspaceSidebarVisible}
            >
              <Menu size={17} strokeWidth={2} />
            </button>
          )}
          <span className="brand-mark" aria-hidden="true"><PanelsTopLeft size={16} strokeWidth={2.2} /></span>
          <span className="brand-name">{t('app.name')}</span>
        </div>

        <nav className="top-tabs" data-doc-id="element:/app/top-navigation" aria-label={t('layout.mainNavigation')}>
          {PRODUCT_ROUTES.map((route) => {
            const RouteIcon = PRODUCT_ROUTE_ICONS[route.id]
            return (
              <NavLink
                key={route.id}
                to={route.path}
                end={route.path === '/'}
                className={({isActive}) => 'top-tab' + (isActive ? ' active' : '')}
                data-doc-id={route.navButtonDocId}
                onClick={() => {
                  if (route.activeMode) {
                    setActiveMode(route.activeMode)
                  }
                  if (compactSidebar) {
                    setCompactSidebarOpen(false)
                  }
                }}
              >
                <RouteIcon size={15} strokeWidth={1.9} aria-hidden="true" />
                <span>{t(route.navLabelKey)}</span>
              </NavLink>
            )
          })}
        </nav>

        <div className="top-bar-actions">
          <LanguageSwitcher />
          <button type="button" className="top-icon-button" data-doc-id="button:/app/shell/toggle-theme" onClick={toggleTheme} title={t('layout.themeShortcut')} aria-label={t('layout.toggleTheme')}>
            {theme === 'light' ? <Moon size={16} strokeWidth={2} /> : <Sun size={16} strokeWidth={2} />}
          </button>
          <button type="button" className="top-command-button" data-doc-id="button:/app/shell/open-command-palette" onClick={() => setCommandPaletteOpen(true)} title={t('layout.commandPaletteShortcut')} aria-label={t('layout.openCommandPalette')}>
            <Command size={15} strokeWidth={2} />
            <kbd>{tr("Ctrl K")}</kbd>
          </button>
        </div>
      </div>

      <div className="app-body">
        {compactSidebar && compactSidebarOpen && !focusMode && !settingsFocused && (
          <button
            type="button"
            className="sidebar-backdrop"
            onClick={() => setCompactSidebarOpen(false)}
            aria-label={t('layout.closeSidebar')}
          />
        )}
        {workspaceSidebarVisible && (
          <div
            id="workspace-sidebar-frame"
            ref={leftSidebarFrameRef}
            className={`left-sidebar-frame${leftSidebarResizing ? ' is-resizing' : ''}${compactSidebar ? ' is-compact' : ''}`}
            style={leftSidebarFrameStyle}
          >
            <LeftSidebar />
            <div
              className="left-sidebar-resize-handle"
              role="separator"
              aria-label={t('layout.resizeSidebar')}
              aria-orientation="vertical"
              aria-valuemin={LEFT_SIDEBAR_MIN_WIDTH}
              aria-valuemax={LEFT_SIDEBAR_MAX_WIDTH}
              aria-valuenow={resolvedLeftSidebarWidth}
              tabIndex={0}
              onPointerDown={handleLeftSidebarResizePointerDown}
              onKeyDown={handleLeftSidebarResizeKeyDown}
            />
          </div>
        )}

        <div className="main-content" key={i18n.resolvedLanguage ?? i18n.language}>
          <Suspense fallback={<ViewLoadingState />}>
            <Outlet />
          </Suspense>
        </div>
      </div>

      <CommandPalette />

      {shortcutsOverlayOpen && (
        <div className="command-palette-overlay">
          <button
            type="button"
            className="command-palette-backdrop"
            aria-label={tr("Close shortcuts")}
            onClick={() => setShortcutsOverlayOpen(false)}
          />
          <div
            className="command-palette"
            data-doc-id="element:/app/shortcut-overlay"
            role="dialog"
            aria-modal="true"
            aria-label={t('shortcuts.title')}
          >
            <div className="command-palette-header">
              <strong className="shortcut-overlay-title">{t('shortcuts.title')}</strong>
              <button type="button" data-doc-id="button:/app/shortcut-overlay/close" onClick={() => setShortcutsOverlayOpen(false)}>{tr("Esc")}</button>
            </div>
            <ul className="command-palette-list">
              {shortcuts.map((s, i) => (
                <li key={i}>
                  <button type="button" onClick={() => setShortcutsOverlayOpen(false)}>
                    <span>{s.label}</span>
                    <kbd>{s.keys}</kbd>
                  </button>
                </li>
              ))}
            </ul>
          </div>
        </div>
      )}
    </div>
  )
}
