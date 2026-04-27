import { Suspense, lazy, useEffect } from 'react'
import { MemoryRouter, Routes, Route, Navigate } from 'react-router-dom'
import Layout from './components/Layout'
import { useUiStore } from './stores/uiStore'
import { useChatStore } from './stores/chatStore'
import { useTaskStore } from './stores/taskStore'
import { useLogStore } from './stores/logStore'
import { useConfigStore } from './stores/configStore'
import { useCoworkStore } from './stores/coworkStore'
import { writeAuditEvent } from './utils/audit'
import { seedDefaultPersonalities, seedDefaultMemory } from './utils/defaultSeeds'
import { startScheduledWorker, stopScheduledWorker } from './engine/scheduledWorker'
import './App.css'

const WelcomeScreen = lazy(() => import('./components/WelcomeScreen'))
const CoworkView = lazy(() => import('./components/CoworkView'))
const SettingsView = lazy(() => import('./components/SettingsView'))

function AppRoutes() {
  const activeThreadId = useChatStore((s) => s.activeThreadId)

  return (
    <Suspense fallback={<div className="main-content" style={{ padding: 24 }}>Ansicht wird geladen...</div>}>
      <Routes>
        <Route path="/" element={<Layout />}>
          <Route index element={activeThreadId ? <CoworkView /> : <WelcomeScreen />} />
          <Route path="settings" element={
            <div className="code-mode" style={{ overflow: 'auto', height: '100%' }}>
              <SettingsView />
            </div>
          } />
          <Route path="*" element={<Navigate to="/" replace />} />
        </Route>
      </Routes>
    </Suspense>
  )
}

function App() {
  const loadChatFromDb = useChatStore((s) => s.loadFromDb)
  const loadTasksFromDb = useTaskStore((s) => s.loadFromDb)
  const theme = useUiStore((s) => s.theme)
  const setTheme = useUiStore((s) => s.setTheme)
  const preferences = useConfigStore((s) => s.preferences)
  const addLog = useLogStore((s) => s.addLog)
  const loadScheduledTasks = useCoworkStore((s) => s.loadScheduledTasks)
  const loadScheduledRuns = useCoworkStore((s) => s.loadScheduledRuns)

  useEffect(() => {
    const startedAt = performance.now()
    loadChatFromDb()
    loadTasksFromDb()
    void loadScheduledTasks()
    void loadScheduledRuns(20)
    seedDefaultPersonalities().catch(() => {})
    seedDefaultMemory().catch(() => {})
    addLog({
      level: 'info',
      area: 'runtime',
      message: 'App gestartet',
      details: { startupMs: Math.round(performance.now() - startedAt) },
    })
    void writeAuditEvent('runtime', 'app_started', {
      startupMs: Math.round(performance.now() - startedAt),
    })
    // Start scheduled tasks worker
    startScheduledWorker()
    return () => stopScheduledWorker()
  }, [addLog, loadChatFromDb, loadScheduledRuns, loadScheduledTasks, loadTasksFromDb])

  useEffect(() => {
    document.documentElement.dataset.theme = theme
    document.body.dataset.theme = theme
    document.documentElement.style.fontSize = `${Math.max(85, Math.min(120, preferences.fontScale))}%`
    document.body.classList.toggle('compact-mode', preferences.compactMode)
    void writeAuditEvent('ui', 'theme_applied', { theme })
  }, [preferences.compactMode, preferences.fontScale, theme])

  useEffect(() => {
    if (!preferences.syncThemeWithSystem) return
    const media = window.matchMedia('(prefers-color-scheme: dark)')
    const applyTheme = (matchesDark: boolean) => setTheme(matchesDark ? 'dark' : 'light')
    applyTheme(media.matches)

    const onChange = (event: MediaQueryListEvent) => applyTheme(event.matches)
    media.addEventListener('change', onChange)
    return () => media.removeEventListener('change', onChange)
  }, [preferences.syncThemeWithSystem, setTheme])

  return (
    <MemoryRouter>
      <AppRoutes />
    </MemoryRouter>
  )
}

export default App
