import { Suspense, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { CSSProperties, KeyboardEvent as ReactKeyboardEvent, PointerEvent as ReactPointerEvent } from 'react'
import { Outlet, useLocation, useNavigate } from 'react-router-dom'
import { Menu } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import {
  LEFT_SIDEBAR_MAX_WIDTH,
  LEFT_SIDEBAR_MIN_WIDTH,
  clampLeftSidebarWidth,
  useUiStore,
} from '../stores/uiStore'
import { useConfigStore } from '../stores/configStore'
import { useChatStore } from '../stores/chatStore'
import LeftSidebar from './LeftSidebar'
import AppMenu from './AppMenu'
import AppContextDrawer from './AppContextDrawer'
import { tr } from '../i18n'
import {
  PRODUCT_ROUTES,
  getProductRouteByShortcutKey,
  type ProductRoute,
  type ProductSubroute,
} from '../product/routeRegistry'

const COMPACT_SIDEBAR_MEDIA_QUERY = '(max-width: 899px)'

function ViewLoadingState() {
  const { t } = useTranslation()

  return (
    <div className="view-loading-state" aria-busy="true" aria-live="polite">
      <div className="view-loading-bar" aria-hidden="true"><span /></div>
      <span>{t('common.preparingView')}</span>
    </div>
  )
}

function routeSubroutes(route: ProductRoute): readonly ProductSubroute[] {
  return 'subroutes' in route && Array.isArray(route.subroutes) ? route.subroutes : []
}

export default function Layout() {
  const { t, i18n } = useTranslation()
  const navigate = useNavigate()
  const location = useLocation()
  const activeThreadId = useChatStore((state) => state.activeThreadId)
  const threads = useChatStore((state) => state.threads)
  const {
    leftSidebarOpen,
    leftSidebarWidth,
    appMenuOpen,
    contextDrawerOpen,
    toggleLeftSidebar,
    setLeftSidebarWidth,
    toggleTheme,
    setAppMenuOpen,
    setContextDrawerOpen,
    setShortcutsOverlayOpen,
    closeShellOverlays,
    setActiveMode,
  } = useUiStore()
  const focusMode = useConfigStore((state) => state.preferences.focusMode)
  const shortcutOverlayEnabled = useConfigStore((state) => state.preferences.shortcutOverlayEnabled)
  const menuButtonRef = useRef<HTMLButtonElement | null>(null)
  const leftSidebarFrameRef = useRef<HTMLDivElement | null>(null)
  const leftSidebarResizeRef = useRef<{ pointerId: number; left: number } | null>(null)
  const previousMenuOpenRef = useRef(false)
  const [leftSidebarResizing, setLeftSidebarResizing] = useState(false)
  const [compactSidebar, setCompactSidebar] = useState(() => (
    typeof window !== 'undefined' && window.matchMedia?.(COMPACT_SIDEBAR_MEDIA_QUERY).matches === true
  ))
  const [compactSidebarOpen, setCompactSidebarOpen] = useState(false)
  const resolvedLeftSidebarWidth = clampLeftSidebarWidth(leftSidebarWidth)
  const resolvedLeftSidebarOpen = compactSidebar ? compactSidebarOpen : leftSidebarOpen
  const workspaceSidebarVisible = resolvedLeftSidebarOpen && !focusMode
  const leftSidebarFrameStyle = {
    '--left-sidebar-width': `${resolvedLeftSidebarWidth}px`,
  } as CSSProperties

  const activeRoute = PRODUCT_ROUTES.find((route) => route.path === location.pathname) ?? PRODUCT_ROUTES[0]
  const shellTitle = useMemo(() => {
    if (activeRoute.id === 'cowork') {
      const activeThread = threads.find((thread) => thread.id === activeThreadId)
      return activeThread?.title === 'New chat'
        ? tr('New chat')
        : activeThread?.title?.trim() || t(activeRoute.titleKey)
    }

    const params = new URLSearchParams(location.search)
    const subroutes = routeSubroutes(activeRoute)
    const selectedSubroute = subroutes.find((subroute) => (
      params.get(subroute.queryKey) === subroute.queryValue
    )) ?? subroutes.find((subroute) => subroute.default)

    return selectedSubroute
      ? tr(selectedSubroute.labelKey)
      : t(activeRoute.titleKey)
  }, [activeRoute, activeThreadId, location.search, t, threads])

  const navigateToProductRoute = useCallback((route: ProductRoute) => {
    if (route.activeMode) setActiveMode(route.activeMode)
    navigate(route.path)
  }, [navigate, setActiveMode])

  useEffect(() => {
    const openSandboxSettings = () => {
      setActiveMode('settings')
      navigate('/settings?section=sandbox')
    }
    window.addEventListener('lacowork-open-sandbox-settings', openSandboxSettings)
    return () => window.removeEventListener('lacowork-open-sandbox-settings', openSandboxSettings)
  }, [navigate, setActiveMode])

  const handleToggleLeftSidebar = useCallback(() => {
    if (compactSidebar) {
      closeShellOverlays()
      setCompactSidebarOpen((open) => !open)
      return
    }
    toggleLeftSidebar()
  }, [closeShellOverlays, compactSidebar, toggleLeftSidebar])

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
    closeShellOverlays()
    setCompactSidebarOpen(false)
  }, [closeShellOverlays, location.pathname, location.search])

  useEffect(() => {
    if (previousMenuOpenRef.current && !appMenuOpen) {
      window.requestAnimationFrame(() => menuButtonRef.current?.focus())
    }
    previousMenuOpenRef.current = appMenuOpen
  }, [appMenuOpen])

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        if (appMenuOpen || contextDrawerOpen) closeShellOverlays()
        if (compactSidebarOpen) setCompactSidebarOpen(false)
        return
      }

      const modifierPressed = event.ctrlKey || event.metaKey
      if (!modifierPressed) return

      if (event.key.toLowerCase() === 'k') {
        event.preventDefault()
        setCompactSidebarOpen(false)
        setAppMenuOpen(true, true)
        return
      }

      if (event.shiftKey && event.key.toLowerCase() === 'b') {
        event.preventDefault()
        handleToggleLeftSidebar()
        return
      }

      const shortcutRoute = event.shiftKey ? undefined : getProductRouteByShortcutKey(event.key)
      if (shortcutRoute) {
        event.preventDefault()
        navigateToProductRoute(shortcutRoute)
        return
      }

      if (event.shiftKey && event.key.toLowerCase() === 'l') {
        event.preventDefault()
        toggleTheme()
        return
      }

      if (event.shiftKey && event.key === '?' && shortcutOverlayEnabled) {
        event.preventDefault()
        setCompactSidebarOpen(false)
        setShortcutsOverlayOpen(true)
      }
    }

    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [
    appMenuOpen,
    closeShellOverlays,
    compactSidebarOpen,
    contextDrawerOpen,
    handleToggleLeftSidebar,
    navigateToProductRoute,
    setAppMenuOpen,
    setShortcutsOverlayOpen,
    shortcutOverlayEnabled,
    toggleTheme,
  ])

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

  return (
    <div className="app-shell" data-doc-id="element:/app/shell">
      <header className="top-bar">
        <button
          ref={menuButtonRef}
          type="button"
          className="top-menu-button"
          data-doc-id="button:/app/shell/open-menu"
          onClick={() => {
            setCompactSidebarOpen(false)
            setAppMenuOpen(!appMenuOpen)
          }}
          aria-label={tr('Open main menu')}
          aria-controls="app-menu-drawer"
          aria-expanded={appMenuOpen}
        >
          <Menu size={17} strokeWidth={1.9} aria-hidden="true" />
        </button>
        <h1 className="shell-title" title={shellTitle}>{shellTitle}</h1>
      </header>

      <div className="app-body">
        {compactSidebar && compactSidebarOpen && !focusMode && (
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

        <main className="main-content" key={i18n.resolvedLanguage ?? i18n.language}>
          <Suspense fallback={<ViewLoadingState />}><Outlet /></Suspense>
        </main>
      </div>

      <AppMenu
        open={appMenuOpen}
        compactSidebar={compactSidebar || focusMode}
        onOpenWorkspaceSidebar={handleToggleLeftSidebar}
      />
      <AppContextDrawer open={contextDrawerOpen} onClose={() => setContextDrawerOpen(false)} />
    </div>
  )
}
