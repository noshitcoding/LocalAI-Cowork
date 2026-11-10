import {
  useCallback,
  useEffect,
  useRef,
  useState,
  useSyncExternalStore,
} from 'react'
import { Download, RotateCcw, ShieldCheck, X } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import {
  appUpdateProgressPercent,
  getAppUpdateSnapshot,
  installAvailableAppUpdate,
  retryAvailableAppUpdate,
  startAutomaticUpdateCheck,
  subscribeAppUpdater,
  type AppUpdatePhase,
} from '../utils/appUpdater'
import {
  APP_UPDATE_SNOOZE_MS,
  clearAppUpdateSnooze,
  readAppUpdateSnoozeUntil,
  storeAppUpdateSnoozeUntil,
} from './appUpdateDialogState'

const INSTALL_PHASES: readonly AppUpdatePhase[] = [
  'backing-up',
  'downloading',
  'installing',
  'restarting',
]

export default function AppUpdateDialog() {
  const { t } = useTranslation()
  const updateState = useSyncExternalStore(
    subscribeAppUpdater,
    getAppUpdateSnapshot,
    getAppUpdateSnapshot,
  )
  const [snoozeUntil, setSnoozeUntil] = useState(readAppUpdateSnoozeUntil)
  const [installationStarted, setInstallationStarted] = useState(false)
  const [retrying, setRetrying] = useState(false)
  const dialogRef = useRef<HTMLElement | null>(null)
  const primaryActionRef = useRef<HTMLButtonElement | null>(null)
  const previousFocusRef = useRef<HTMLElement | null>(null)
  const installInProgress = INSTALL_PHASES.includes(updateState.phase)
  const snoozed = snoozeUntil > Date.now()
  const visible = retrying
    || installInProgress
    || (!snoozed && (
      updateState.phase === 'available'
      || (installationStarted && updateState.phase === 'error')
    ))
  const progress = appUpdateProgressPercent(updateState)

  useEffect(() => {
    startAutomaticUpdateCheck()
  }, [])

  useEffect(() => {
    if (!installInProgress) return
    setInstallationStarted(true)
    if (snoozeUntil > 0) {
      clearAppUpdateSnooze()
      setSnoozeUntil(0)
    }
  }, [installInProgress, snoozeUntil])

  useEffect(() => {
    if (snoozeUntil <= 0) return undefined
    const remaining = snoozeUntil - Date.now()
    if (remaining <= 0) {
      setSnoozeUntil(0)
      return undefined
    }
    const timeout = window.setTimeout(() => setSnoozeUntil(0), remaining)
    return () => window.clearTimeout(timeout)
  }, [snoozeUntil])

  const dismissForOneDay = useCallback(() => {
    if (installInProgress || retrying) return
    const nextSnoozeUntil = Date.now() + APP_UPDATE_SNOOZE_MS
    storeAppUpdateSnoozeUntil(nextSnoozeUntil)
    setSnoozeUntil(nextSnoozeUntil)
  }, [installInProgress, retrying])

  useEffect(() => {
    if (!visible) return undefined
    previousFocusRef.current = document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null
    const frame = window.requestAnimationFrame(() => {
      const focusTarget = primaryActionRef.current ?? dialogRef.current
      focusTarget?.focus()
    })
    return () => {
      window.cancelAnimationFrame(frame)
      previousFocusRef.current?.focus()
      previousFocusRef.current = null
    }
  }, [visible])

  useEffect(() => {
    if (!visible) return undefined
    const frame = window.requestAnimationFrame(() => {
      if (!dialogRef.current?.contains(document.activeElement)) dialogRef.current?.focus()
    })
    return () => window.cancelAnimationFrame(frame)
  }, [updateState.phase, visible])

  useEffect(() => {
    if (!visible) return undefined
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault()
        event.stopPropagation()
        dismissForOneDay()
        return
      }
      if (event.key !== 'Tab') return

      const focusable = Array.from(dialogRef.current?.querySelectorAll<HTMLElement>(
        'button:not(:disabled), [href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])',
      ) ?? [])
      if (focusable.length === 0) {
        event.preventDefault()
        dialogRef.current?.focus()
        return
      }
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
    window.addEventListener('keydown', handleKeyDown, true)
    return () => window.removeEventListener('keydown', handleKeyDown, true)
  }, [dismissForOneDay, visible])

  if (!visible) return null

  const busy = retrying || installInProgress
  const title = updateState.phase === 'error'
    ? t('The update could not be installed')
    : installInProgress || retrying
      ? t('Installing app update')
      : t('A new app version is available')
  const status = retrying && updateState.phase === 'checking'
    ? t('Checking for updates...')
    : updateState.phase === 'backing-up'
      ? t('Backing up workspace...')
      : updateState.phase === 'downloading'
        ? t('Downloading update... {{progress}}', { progress: progress === null ? '' : `${progress}%` })
        : updateState.phase === 'installing'
          ? t('Installing update...')
          : updateState.phase === 'restarting'
            ? t('Restarting...')
            : null

  const handleInstall = () => {
    clearAppUpdateSnooze()
    setSnoozeUntil(0)
    setInstallationStarted(true)
    void installAvailableAppUpdate()
  }

  const handleRetry = async () => {
    clearAppUpdateSnooze()
    setSnoozeUntil(0)
    setRetrying(true)
    try {
      await retryAvailableAppUpdate()
    } finally {
      setRetrying(false)
    }
  }

  return (
    <div className="app-update-dialog-backdrop" role="presentation">
      <section
        ref={dialogRef}
        className="app-update-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="app-update-dialog-title"
        aria-describedby="app-update-dialog-description"
        tabIndex={-1}
      >
        <header className="app-update-dialog-header">
          <span className="app-update-dialog-icon" aria-hidden="true"><ShieldCheck size={22} /></span>
          <div>
            <span className="task-detail-kicker">{t('Signed GitHub update')}</span>
            <h2 id="app-update-dialog-title">{title}</h2>
          </div>
          <button
            type="button"
            className="app-update-dialog-close"
            onClick={dismissForOneDay}
            disabled={busy}
            aria-label={t('Remind me in 24 hours')}
          >
            <X size={18} aria-hidden="true" />
          </button>
        </header>

        <p id="app-update-dialog-description" className="app-update-dialog-description">
          {updateState.phase === 'error'
            ? t('No update was installed. Your workspace is unchanged.')
            : t('The signed update will be backed up, installed, and followed by an automatic restart.')}
        </p>

        <dl className="app-update-dialog-versions">
          <div>
            <dt>{t('Current version')}</dt>
            <dd>{updateState.currentVersion ?? '—'}</dd>
          </div>
          <div>
            <dt>{t('Available version')}</dt>
            <dd>{updateState.availableVersion ?? '—'}</dd>
          </div>
        </dl>

        {updateState.releaseNotes ? (
          <section className="app-update-dialog-notes" aria-labelledby="app-update-dialog-notes-title">
            <h3 id="app-update-dialog-notes-title">{t('What is new')}</h3>
            <p>{updateState.releaseNotes}</p>
          </section>
        ) : null}

        {status ? (
          <div className="app-update-dialog-status" aria-live="polite">
            <strong>{status}</strong>
            {updateState.phase === 'downloading' ? (
              <span
                className={`app-update-dialog-progress${progress === null ? ' is-indeterminate' : ''}`}
                role="progressbar"
                aria-label={t('Download progress')}
                aria-valuemin={0}
                aria-valuemax={100}
                aria-valuenow={progress ?? undefined}
              >
                <span style={progress === null ? undefined : { width: `${progress}%` }} />
              </span>
            ) : null}
          </div>
        ) : null}

        {updateState.phase === 'error' && updateState.error ? (
          <p className="app-update-dialog-error" role="alert">{updateState.error}</p>
        ) : null}

        {!busy ? (
          <footer className="app-update-dialog-actions">
            <button type="button" className="ui-button ui-button--secondary" onClick={dismissForOneDay}>
              {t('Later')}
            </button>
            {updateState.phase === 'error' ? (
              <button
                ref={primaryActionRef}
                type="button"
                className="ui-button ui-button--primary"
                onClick={() => void handleRetry()}
              >
                <RotateCcw size={16} aria-hidden="true" />
                {t('Try again')}
              </button>
            ) : (
              <button
                ref={primaryActionRef}
                type="button"
                className="ui-button ui-button--primary"
                onClick={handleInstall}
              >
                <Download size={16} aria-hidden="true" />
                {t('Install now')}
              </button>
            )}
          </footer>
        ) : null}
      </section>
    </div>
  )
}
