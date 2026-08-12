import { lazy, useEffect, useState, type ReactNode } from 'react'
import { BrowserRouter, Routes, Route, Navigate } from 'react-router-dom'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { confirm as confirmDialog } from '@tauri-apps/plugin-dialog'
import Layout from './components/Layout'
import { useUiStore } from './stores/uiStore'
import { useChatStore } from './stores/chatStore'
import { useTaskStore } from './stores/taskStore'
import { useLogStore } from './stores/logStore'
import { useConfigStore } from './stores/configStore'
import { useCoworkStore } from './stores/coworkStore'
import { useCrewStore } from './stores/crewStore'
import { useCrewRuntimeStore } from './stores/crewRuntimeStore'
import { useEngineStore } from './stores/engineStore'
import { useWorkTasksStore } from './stores/workTasksStore'
import { useProjectStore } from './stores/projectStore'
import { handleCrewTaskMessage } from './engine/crew/crewHandler'
import { writeAuditEvent } from './utils/audit'
import { seedDefaultPersonalities, seedDefaultMemory } from './utils/defaultSeeds'
import { startScheduledWorker, stopScheduledWorker } from './engine/scheduledWorker'
import { hasTauriRuntime, safeInvoke } from './utils/safeInvoke'
import { PRODUCT_ROUTES, type ProductRouteId, type ProductRoutePath } from './product/routeRegistry'
import { initializeCredentialVault } from './security/credentialMigration'
import i18n from './i18n'
import BackendSetupDialog from './components/BackendSetupDialog'
import { useBackendDefaultsStore } from './stores/backendDefaultsStore'
import ProviderFallbackApprovalDialog from './components/ProviderFallbackApprovalDialog'
import { reconcileDurableLocalRuns } from './runtime/localDaemonChat'
import { reconcileDurableLocalEntities } from './runtime/localDaemonEntities'
import './App.css'

const CoworkView = lazy(() => import('./components/CoworkView'))
const SettingsView = lazy(() => import('./components/SettingsView'))
const TasksView = lazy(() => import('./components/TasksView'))
const CrewView = lazy(() => import('./components/CrewView'))
const ProjectView = lazy(() => import('./components/ProjectView'))
const FeaturesView = lazy(() => import('./components/FeaturesView'))
const DeveloperBrowserView = lazy(() => import('./components/DeveloperBrowserView'))
const GitHubView = lazy(() => import('./components/GitHubView'))
const RemoteServerView = lazy(() => import('./components/RemoteServerView'))
const MobileApp = lazy(() => import('./mobile/MobileApp'))
const OidcCallbackPage = lazy(() => import('./components/OidcCallbackPage'))
const IS_ANDROID_SHELL = import.meta.env.VITE_COWORK_ANDROID === 'true'

type BackendPolicyState = {
  flags: Record<string, boolean>
  denyRules: string[]
  enabledToolIds: string[]
  activeToolsetPolicyId?: string
  toolsetPolicies?: Array<{
    id: string
    label: string
    description: string
    riskLevel: string
    toolIds: string[]
  }>
}

function hideBootLoader(): void {
  const loader = document.getElementById('boot-loader')
  if (!loader) return

  loader.classList.add('boot-loader-hidden')
  window.setTimeout(() => loader.remove(), 220)
}

function RouteReady({ children }: { children: ReactNode }) {
  useEffect(() => {
    const frame = window.requestAnimationFrame(() => hideBootLoader())
    return () => window.cancelAnimationFrame(frame)
  }, [])

  return children
}

function nestedRoutePath(path: ProductRoutePath): string {
  return path.replace(/^\//, '')
}

function ProductRouteReady({ routeId }: { routeId: ProductRouteId }) {
  let content: ReactNode

  switch (routeId) {
    case 'cowork':
      content = <CoworkView />
      break
    case 'settings':
      content = <div className="code-mode settings-route-frame"><SettingsView /></div>
      break
    case 'tasks':
      content = <div className="code-mode" style={{ overflow: 'auto', height: '100%' }}><TasksView /></div>
      break
    case 'crew':
      content = <CrewView />
      break
    case 'projects':
      content = <ProjectView />
      break
    case 'features':
      content = <div className="code-mode" style={{ overflow: 'auto', height: '100%' }}><FeaturesView /></div>
      break
    case 'browser':
      content = <DeveloperBrowserView />
      break
    case 'github':
      content = <GitHubView />
      break
    case 'server':
      content = <RemoteServerView />
      break
    default:
      content = <CoworkView />
  }

  return <RouteReady>{content}</RouteReady>
}

function hasRunningWork(): boolean {
  const chatState = useChatStore.getState()
  const legacyTasks = useTaskStore.getState().tasks
  const workTasks = useWorkTasksStore.getState().tasks
  const crews = useCrewStore.getState().crews
  const engineStatus = useEngineStore.getState().status
  const activeDurableThreadIds = new Set(chatState.threads
    .filter((thread) => thread.messages.some((message) => (
      message.durableRunId
      && !['completed', 'failed', 'canceled', 'expired', 'interrupted'].includes(message.durableRunState ?? '')
    )))
    .map((thread) => thread.id))

  return chatState.threads.some((thread) => thread.messages.some((message) => message.streaming && !message.durableRunId))
    || legacyTasks.some((task) => task.status === 'running' || task.status === 'waiting_approval')
    || workTasks.some((task) => (
      (task.status === 'running' || task.status === 'waiting_approval')
      && (!task.threadId || !activeDurableThreadIds.has(task.threadId))
    ))
    || crews.some((crew) => crew.status === 'running' || crew.status === 'awaiting-approval')
    || engineStatus === 'streaming'
    || engineStatus === 'tool_running'
    || engineStatus === 'waiting_approval'
}

function shouldConfirmAppClose(): boolean {
  return useConfigStore.getState().preferences.confirmOnCloseWithRunningTasks
}

async function confirmAppClose(): Promise<boolean> {
  const runningWork = hasRunningWork()
  const message = runningWork
    ? i18n.t('close.runningWork')
    : i18n.t('close.confirm')

  try {
    return await confirmDialog(message, {
      title: i18n.t('close.title'),
      kind: runningWork ? 'warning' : 'info',
      okLabel: i18n.t('common.close'),
      cancelLabel: i18n.t('common.cancel'),
    })
  } catch (error) {
    console.warn('Close confirmation dialog failed:', error)
    try {
      return window.confirm(message)
    } catch {
      return false
    }
  }
}

function AppRoutes() {
  return (
    <Routes>
      <Route path="/" element={<Layout />}>
        <Route path="auth/callback" element={<OidcCallbackPage />} />
        {PRODUCT_ROUTES.map((route) => (
          route.path === '/'
            ? <Route key={route.id} index element={<ProductRouteReady routeId={route.id} />} />
            : <Route key={route.id} path={nestedRoutePath(route.path)} element={<ProductRouteReady routeId={route.id} />} />
        ))}
        <Route path="*" element={<Navigate to="/" replace />} />
      </Route>
    </Routes>
  )
}

function DesktopApp() {
  const existingInstallationAtBoot = useState(() => (
    typeof window !== 'undefined'
      && ['open-cowork-config', 'engine-store', 'open-cowork-work-tasks']
        .some((key) => window.localStorage.getItem(key) !== null)
  ))[0]
  const [credentialsReady, setCredentialsReady] = useState(false)
  const [credentialError, setCredentialError] = useState(false)
  const [credentialRetry, setCredentialRetry] = useState(0)
  const loadChatFromDb = useChatStore((s) => s.loadFromDb)
  const loadTasksFromDb = useTaskStore((s) => s.loadFromDb)
  const loadWorkTasksFromDb = useWorkTasksStore((s) => s.loadFromDb)
  const loadProjectsFromDb = useProjectStore((s) => s.loadFromDb)
  const theme = useUiStore((s) => s.theme)
  const setTheme = useUiStore((s) => s.setTheme)
  const preferences = useConfigStore((s) => s.preferences)
  const addLog = useLogStore((s) => s.addLog)
  const loadScheduledTasks = useCoworkStore((s) => s.loadScheduledTasks)
  const loadScheduledRuns = useCoworkStore((s) => s.loadScheduledRuns)
  const setPolicySnapshot = useCoworkStore((s) => s.setPolicySnapshot)
  const ensureCrewRuntimeReady = useCrewRuntimeStore((s) => s.ensureReady)
  const loadBackendDefaults = useBackendDefaultsStore((s) => s.load)

  useEffect(() => {
    if (!hasTauriRuntime()) return
    void ensureCrewRuntimeReady()
  }, [ensureCrewRuntimeReady])

  useEffect(() => {
    let cancelled = false
    void initializeCredentialVault()
      .then(() => {
        if (!cancelled) setCredentialsReady(true)
      })
      .catch(() => {
        if (cancelled) return
        setCredentialError(true)
        hideBootLoader()
      })
    return () => {
      cancelled = true
    }
  }, [credentialRetry])

  useEffect(() => {
    if (!credentialsReady) return
    const startedAt = performance.now()
    const chatLoad = loadChatFromDb()
    const workTaskLoad = loadWorkTasksFromDb()
    void Promise.all([chatLoad, workTaskLoad])
      .then(async () => {
        await reconcileDurableLocalEntities()
        await reconcileDurableLocalRuns()
      })
      .catch((error) => console.warn('[startup] Chat/task/local-run reconciliation failed', error))
    void loadTasksFromDb().catch((error) => console.warn('[startup] Task loading failed', error))
    void loadProjectsFromDb()
    void loadScheduledTasks()
    void loadScheduledRuns(20)
    void loadBackendDefaults(existingInstallationAtBoot)
    void safeInvoke<BackendPolicyState | null>('policy_get', undefined, null)
      .then((policy) => {
        if (!policy) return
        setPolicySnapshot(
          policy.flags,
          policy.denyRules ?? [],
          policy.enabledToolIds ?? [],
          policy.activeToolsetPolicyId,
          policy.toolsetPolicies
        )
      })
      .catch(() => {})
    seedDefaultPersonalities().catch(() => {})
    seedDefaultMemory().catch(() => {})
    addLog({
      level: 'info',
      area: 'runtime',
      message: 'App started',
      details: { startupMs: Math.round(performance.now() - startedAt) },
    })
    void writeAuditEvent('runtime', 'app_started', {
      startupMs: Math.round(performance.now() - startedAt),
    })
    // Start scheduled tasks worker
    startScheduledWorker()
    return () => stopScheduledWorker()
  }, [addLog, credentialsReady, existingInstallationAtBoot, loadBackendDefaults, loadChatFromDb, loadProjectsFromDb, loadScheduledRuns, loadScheduledTasks, loadTasksFromDb, loadWorkTasksFromDb, setPolicySnapshot])

  useEffect(() => {
    document.documentElement.dataset.theme = theme
    document.body.dataset.theme = theme
    document.documentElement.style.fontSize = `${Math.max(85, Math.min(120, preferences.fontScale))}%`
    document.body.classList.toggle('compact-mode', preferences.compactMode)

    // Register crew task message handler
    useEngineStore.getState().setCrewTaskMessageHandler(handleCrewTaskMessage)
    return () => {
      useEngineStore.getState().setCrewTaskMessageHandler(null)
    }
  }, [theme, preferences.compactMode, preferences.fontScale])

  useEffect(() => {
    void writeAuditEvent('ui', 'theme_applied', { theme })
  }, [theme])

  useEffect(() => {
    if (!preferences.syncThemeWithSystem) return
    const media = window.matchMedia('(prefers-color-scheme: dark)')
    const applyTheme = (matchesDark: boolean) => setTheme(matchesDark ? 'dark' : 'light')
    applyTheme(media.matches)

    const onChange = (event: MediaQueryListEvent) => applyTheme(event.matches)
    media.addEventListener('change', onChange)
    return () => media.removeEventListener('change', onChange)
  }, [preferences.syncThemeWithSystem, setTheme])

  useEffect(() => {
    const beforeUnload = (event: BeforeUnloadEvent) => {
      if (!shouldConfirmAppClose() || !hasRunningWork()) return
      event.preventDefault()
      event.returnValue = ''
    }
    if (!hasTauriRuntime()) {
      window.addEventListener('beforeunload', beforeUnload)
    }

    let unlistenClose: (() => void) | null = null
    let closeListenerDisposed = false
    if (hasTauriRuntime()) {
      const appWindow = getCurrentWindow()
      let closePromptOpen = false
      let closeConfirmed = false

      void appWindow.onCloseRequested(async (event) => {
        if (closeConfirmed || !shouldConfirmAppClose()) return

        event.preventDefault()
        if (closePromptOpen) return

        closePromptOpen = true
        const confirmed = await confirmAppClose()
        closePromptOpen = false

        if (confirmed) {
          closeConfirmed = true
          try {
            // Force-close to avoid re-entrancy issues (close() emits closeRequested again).
            await appWindow.destroy()
          } catch (error) {
            closeConfirmed = false
            console.warn('Closing the app window failed:', error)
          }
        }
      }).then((unlisten) => {
        if (closeListenerDisposed) {
          unlisten()
          return
        }
        unlistenClose = unlisten
      }).catch(() => {})
    }

    return () => {
      closeListenerDisposed = true
      window.removeEventListener('beforeunload', beforeUnload)
      unlistenClose?.()
    }
  }, [])

  if (!credentialsReady) {
    return credentialError ? (
      <main className="credential-startup-error" role="alert">
        <h1>{i18n.t('credentials.errorTitle')}</h1>
        <p>{i18n.t('credentials.errorBody')}</p>
        <button type="button" className="btn-sm" onClick={() => {
          setCredentialError(false)
          setCredentialRetry((value) => value + 1)
        }}>
          {i18n.t('credentials.retry')}
        </button>
      </main>
    ) : null
  }

  return (
    <BrowserRouter>
      <AppRoutes />
      <BackendSetupDialog />
      <ProviderFallbackApprovalDialog />
    </BrowserRouter>
  )
}

function App() {
  if (IS_ANDROID_SHELL) return <MobileApp />
  return <DesktopApp />
}

export default App
