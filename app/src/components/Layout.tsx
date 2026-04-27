import { useEffect } from 'react'
import { Outlet, NavLink, useNavigate } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import { useUiStore } from '../stores/uiStore'
import { useConfigStore } from '../stores/configStore'
import LeftSidebar from './LeftSidebar'
import CommandPalette from './CommandPalette'
import RightSidebar from './RightSidebar'

export default function Layout() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  
  const {
    leftSidebarOpen,
    rightSidebarOpen,
    toggleLeftSidebar,
    toggleRightSidebar,
    toggleTheme,
    setCommandPaletteOpen,
    commandPaletteOpen,
    shortcutsOverlayOpen,
    setShortcutsOverlayOpen,
  } = useUiStore()
  
  const focusMode = useConfigStore((s) => s.preferences.focusMode)
  const shortcutOverlayEnabled = useConfigStore((s) => s.preferences.shortcutOverlayEnabled)

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const modifierPressed = event.ctrlKey || event.metaKey
      if (!modifierPressed) {
        if (event.key === 'Escape' && commandPaletteOpen) {
          setCommandPaletteOpen(false)
        }
        return
      }

      if (event.key.toLowerCase() === 'k') {
        event.preventDefault()
        setCommandPaletteOpen(true)
      }

      if (event.shiftKey && event.key.toLowerCase() === 'b') {
        event.preventDefault()
        toggleLeftSidebar()
      }

      if (event.key === '1') {
        event.preventDefault()
        navigate('/')
      }

      if (event.key === '2') {
        event.preventDefault()
        navigate('/settings')
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
  }, [commandPaletteOpen, navigate, setCommandPaletteOpen, setShortcutsOverlayOpen, shortcutOverlayEnabled, toggleLeftSidebar, toggleTheme])

  return (
    <div className="app-shell">
      {/* Top Bar */}
      <div className="top-bar">
        <div className="top-bar-brand">
          <button type="button" className="btn-toggle-sidebar" onClick={toggleLeftSidebar} title="Sidebar (Ctrl+Shift+B)">
            ☰
          </button>
          <span className="brand-icon">✦</span>
          <span className="brand-name">Open_Cowork</span>
        </div>

        <div className="top-tabs">
          <NavLink to="/" end className={({isActive}) => `top-tab${isActive ? ' active' : ''}`}>
            {t('Cowork')}
          </NavLink>
          <NavLink to="/settings" className={({isActive}) => `top-tab${isActive ? ' active' : ''}`}>
            {t('Settings')}
          </NavLink>
        </div>

        <div className="top-bar-actions">
          <button type="button" className="btn-toggle-sidebar" onClick={toggleRightSidebar} title="Rechte Sidebar">
            ☷
          </button>
          <button type="button" className="btn-toggle-sidebar" onClick={toggleTheme} title="Theme (Ctrl+Shift+L)">
            🌗
          </button>
          <button type="button" className="btn-toggle-sidebar" onClick={() => setCommandPaletteOpen(true)} title="Command Palette (Ctrl+K)">
            ⌘K
          </button>
        </div>
      </div>

      {/* Body: sidebar + main */}
      <div className="app-body">
        {leftSidebarOpen && !focusMode && (
          <LeftSidebar />
        )}

        <div className="main-content">
          <Outlet />
        </div>

        {rightSidebarOpen && !focusMode && (
          <RightSidebar />
        )}
      </div>

      <CommandPalette />

      {shortcutsOverlayOpen && (
        <div className="command-palette-overlay" onClick={() => setShortcutsOverlayOpen(false)}>
          <div className="command-palette" onClick={(e) => e.stopPropagation()}>
            <div className="command-palette-header">
              <strong style={{ flex: 1, fontSize: 15 }}>Shortcuts</strong>
              <button type="button" onClick={() => setShortcutsOverlayOpen(false)}>Esc</button>
            </div>
            <ul className="command-palette-list">
              {[
                { label: 'Command Palette', keys: 'Ctrl+K' },
                { label: 'Arbeitsbereich', keys: 'Ctrl+1' },
                { label: 'Einstellungen', keys: 'Ctrl+2' },
                { label: 'Sidebar ein-/ausblenden', keys: 'Ctrl+Shift+B' },
                { label: 'Theme wechseln', keys: 'Ctrl+Shift+L' },
                { label: 'Shortcuts anzeigen', keys: 'Ctrl+Shift+?' },
              ].map((s, i) => (
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
