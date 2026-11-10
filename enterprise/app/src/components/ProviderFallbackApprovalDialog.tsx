import { listen } from '@tauri-apps/api/event'
import { useEffect, useMemo, useState } from 'react'
import { AlertTriangle } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { useConfigStore } from '../stores/configStore'
import { useWorkTasksStore } from '../stores/workTasksStore'
import { hasTauriRuntime, safeInvoke } from '../utils/safeInvoke'
import { showDesktopNotification } from '../utils/notifications'

type FallbackApproval = {
  id: string
  runId: string
  apiProfileId: string | null
  status: 'pending' | 'approved' | 'denied' | 'consumed'
  reason: string
  createdAt: string
}

export default function ProviderFallbackApprovalDialog() {
  const { t: tr } = useTranslation()
  const profiles = useConfigStore((state) => state.llmProfiles)
  const defaultIds = useConfigStore((state) => state.defaultLlmProfileIds)
  const notificationsEnabled = useConfigStore((state) => state.preferences.notificationsEnabled)
  const reloadTasks = useWorkTasksStore((state) => state.loadFromDb)
  const [approvals, setApprovals] = useState<FallbackApproval[]>([])
  const [selectedProfiles, setSelectedProfiles] = useState<Record<string, string>>({})
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const defaultProfileId = defaultIds.api ?? defaultIds.ollama ?? profiles[0]?.id ?? ''
  const approval = approvals[0]

  const load = async () => {
    const rows = await safeInvoke<FallbackApproval[]>('provider_fallback_list', undefined, [])
    setApprovals(rows)
  }

  useEffect(() => {
    if (!hasTauriRuntime()) return
    void load()
    let disposed = false
    let unlistenCreated: (() => void) | undefined
    let unlistenConsumed: (() => void) | undefined
    void listen<FallbackApproval>('provider-fallback-approval-created', () => {
      if (disposed) return
      void load()
      void reloadTasks()
      if (notificationsEnabled) {
        void showDesktopNotification(
          tr('OpenCowork is waiting for approval'),
          tr('All automatic Codex accounts are limited. Select an API profile for this run.'),
        )
      }
    }).then((unlisten) => { unlistenCreated = unlisten })
    void listen('provider-fallback-consumed', () => {
      if (disposed) return
      void load()
      void reloadTasks()
    }).then((unlisten) => { unlistenConsumed = unlisten })
    return () => {
      disposed = true
      unlistenCreated?.()
      unlistenConsumed?.()
    }
  }, [notificationsEnabled, reloadTasks, tr])

  const selectedProfileId = useMemo(() => (
    approval ? selectedProfiles[approval.id] ?? defaultProfileId : defaultProfileId
  ), [approval, defaultProfileId, selectedProfiles])

  if (!approval) return null

  const resolve = async (approved: boolean) => {
    setSaving(true)
    setError(null)
    try {
      await safeInvoke('provider_fallback_resolve', {
        request: {
          id: approval.id,
          approved,
          apiProfileId: approved ? selectedProfileId : null,
        },
      })
      await load()
      await reloadTasks()
    } catch (resolveError) {
      setError(resolveError instanceof Error ? resolveError.message : String(resolveError))
    } finally {
      setSaving(false)
    }
  }

  return (
    <div className="provider-fallback-banner" role="dialog" aria-modal="true" aria-labelledby="provider-fallback-title">
      <div className="provider-fallback-card">
        <AlertTriangle size={22} aria-hidden="true" />
        <div>
          <h2 id="provider-fallback-title">{tr('One-time API approval required')}</h2>
          <p>{tr('All automatically available Codex accounts are currently limited. The scheduled run remains in')} <code>waiting_approval</code> {tr('until you approve or reject a paid API profile for this run.')}</p>
          <select
            className="ui-field"
            aria-label={tr('Fallback API profile')}
            value={selectedProfileId}
            disabled={saving}
            onChange={(event) => setSelectedProfiles((current) => ({
              ...current,
              [approval.id]: event.target.value,
            }))}
          >
            {profiles.map((profile) => (
              <option key={profile.id} value={profile.id}>{profile.name}</option>
            ))}
          </select>
          {error ? <p className="form-error" role="alert">{error}</p> : null}
          <div className="actions">
            <button type="button" className="ui-button ui-button--secondary" disabled={saving} onClick={() => void resolve(false)}>{tr('Reject')}</button>
            <button type="button" className="ui-button ui-button--primary" disabled={saving || !selectedProfileId} onClick={() => void resolve(true)}>{tr('Approve once')}</button>
          </div>
        </div>
      </div>
    </div>
  )
}
