import { useEffect, useState } from 'react'
import { ArrowDown, ArrowUp, Copy, LogIn, LogOut, Plus, RefreshCw, Trash2 } from 'lucide-react'
import { openUrl } from '@tauri-apps/plugin-opener'
import { useTranslation } from 'react-i18next'
import type { TFunction } from 'i18next'
import { useCodexStore, type CodexAuthProfile } from '../stores/codexStore'

function quotaLabel(profile: CodexAuthProfile, tr: TFunction): string {
  if (!profile.quotaJson) return tr('Quota not loaded yet')
  try {
    const quota = JSON.parse(profile.quotaJson) as {
      primary?: { usedPercent?: number }
      secondary?: { usedPercent?: number }
      rateLimitReachedType?: string | null
    }
    if (quota.rateLimitReachedType) return tr('Quota exhausted')
    const primary = Number(quota.primary?.usedPercent)
    const secondary = Number(quota.secondary?.usedPercent)
    const values = [primary, secondary].filter(Number.isFinite)
    return values.length > 0
      ? tr('{{percent}}% used', { percent: Math.max(...values) })
      : tr('Quota available')
  } catch {
    return tr('Quota status unknown')
  }
}

function statusLabel(status: CodexAuthProfile['status'], tr: TFunction): string {
  switch (status) {
    case 'ready': return tr('Signed in')
    case 'limited': return tr('Limited')
    case 'login_pending': return tr('Sign-in pending')
    case 'requires_reauth': return tr('New sign-in required')
    case 'unavailable': return tr('Unavailable')
    default: return tr('Signed out')
  }
}

export default function CodexAccountsPanel() {
  const { t: tr } = useTranslation()
  const [deviceLinkCopyState, setDeviceLinkCopyState] = useState<'idle' | 'copied' | 'failed'>('idle')
  const {
    runtime,
    profiles,
    deviceLogin,
    loading,
    error,
    load,
    createProfile,
    renameProfile,
    reorderProfile,
    login,
    refreshProfile,
    logout,
    removeProfile,
  } = useCodexStore()

  useEffect(() => {
    void load()
  }, [load])

  useEffect(() => {
    setDeviceLinkCopyState('idle')
  }, [deviceLogin?.verificationUrl])

  const addAndLogin = async () => {
    const id = await createProfile()
    await login(id, 'browser')
  }

  const copyDeviceLoginUrl = async () => {
    if (!deviceLogin) return
    try {
      await navigator.clipboard.writeText(deviceLogin.verificationUrl)
      setDeviceLinkCopyState('copied')
    } catch {
      setDeviceLinkCopyState('failed')
    }
  }

  return (
    <section className="panel codex-accounts-panel" aria-labelledby="codex-accounts-heading">
      <div className="panel-heading-row">
        <div>
          <h2 id="codex-accounts-heading">{tr('Use Codex')}</h2>
          <p className="hint-text">{tr('ChatGPT/Codex quota through the bundled official Codex App Server.')}</p>
        </div>
        <button type="button" className="btn-sm" onClick={() => void addAndLogin()} disabled={!runtime.available || loading}>
          <Plus size={14} /> {tr('Add account')}
        </button>
      </div>

      <div className={`llm-provider-state tone-${runtime.available ? 'success' : 'warning'}`}>
        {tr('Codex {{version}}', { version: runtime.version })} · {runtime.available ? tr('Runtime verified') : tr('Runtime unavailable')}
      </div>
      {!runtime.available && runtime.error ? (
        <p className="error" role="alert">{runtime.error}</p>
      ) : null}
      <p className="hint-text">
        {tr('Automatic uses accounts in this order and switches only when quota is exhausted. Pinned accounts never switch automatically.')}
      </p>

      {profiles.length === 0 ? (
        <p className="panel-empty">{tr('No Codex account configured yet.')}</p>
      ) : (
        <div className="llm-profile-list">
          {profiles.map((profile, index) => (
            <article key={profile.id} className="card llm-profile-card">
              <div className="panel-heading-row llm-profile-card-header">
                <label>
                  <span className="sr-only">{tr('Account name')}</span>
                  <input
                    value={profile.name}
                    onChange={(event) => void renameProfile(profile.id, event.target.value)}
                  />
                </label>
                <div className="actions">
                  <button type="button" className="btn-sm" title={tr('Move up')} disabled={index === 0} onClick={() => void reorderProfile(profile.id, -1)}><ArrowUp size={14} /></button>
                  <button type="button" className="btn-sm" title={tr('Move down')} disabled={index === profiles.length - 1} onClick={() => void reorderProfile(profile.id, 1)}><ArrowDown size={14} /></button>
                </div>
              </div>

              <dl className="settings-summary-grid">
                <div><dt>{tr('Status')}</dt><dd>{statusLabel(profile.status, tr)}</dd></div>
                <div><dt>{tr('Account')}</dt><dd>{profile.email || '—'}</dd></div>
                <div><dt>{tr('Plan')}</dt><dd>{profile.planType || '—'}</dd></div>
                <div><dt>{tr('Quota')}</dt><dd>{quotaLabel(profile, tr)}</dd></div>
                <div><dt>{tr('Reset')}</dt><dd>{profile.quotaResetAt ? new Date(profile.quotaResetAt).toLocaleString() : '—'}</dd></div>
              </dl>

              <div className="actions llm-profile-actions">
                {profile.status === 'ready' || profile.status === 'limited' ? (
                  <>
                    <button type="button" className="btn-sm" onClick={() => void refreshProfile(profile.id)}><RefreshCw size={14} /> {tr('Refresh')}</button>
                    <button type="button" className="btn-sm" onClick={() => void login(profile.id, 'browser')}><LogIn size={14} /> {tr('Sign in again')}</button>
                    <button type="button" className="btn-sm" onClick={() => void logout(profile.id)}><LogOut size={14} /> {tr('Sign out')}</button>
                  </>
                ) : (
                  <>
                    <button type="button" className="btn-sm primary" onClick={() => void login(profile.id, 'browser')}><LogIn size={14} /> {tr('Sign in in browser')}</button>
                    <button type="button" className="btn-sm" onClick={() => void login(profile.id, 'device')}>{tr('Device code')}</button>
                  </>
                )}
                <button type="button" className="btn-sm danger" onClick={() => void removeProfile(profile.id)}><Trash2 size={14} /> {tr('Remove')}</button>
              </div>
            </article>
          ))}
        </div>
      )}

      {deviceLogin ? (
        <div className="card codex-device-login-card" role="status">
          <strong>{tr('Device code: {{code}}', { code: deviceLogin.userCode })}</strong>
          <p className="hint-text">{tr('Copy the sign-in link or open it manually, then enter this code.')}</p>
          <label className="codex-device-login-field">
            <span>{tr('Sign-in link')}</span>
            <div className="codex-device-login-copy-row">
              <input
                aria-label={tr('Sign-in link')}
                value={deviceLogin.verificationUrl}
                readOnly
                onFocus={(event) => event.currentTarget.select()}
              />
              <button type="button" className="btn-sm" onClick={() => void copyDeviceLoginUrl()}>
                <Copy size={14} /> {deviceLinkCopyState === 'copied' ? tr('Link copied') : tr('Copy sign-in link')}
              </button>
            </div>
          </label>
          <div className="actions llm-profile-actions">
            <button type="button" className="btn-sm" onClick={() => void openUrl(deviceLogin.verificationUrl)}>{tr('Open sign-in page')}</button>
          </div>
          {deviceLinkCopyState === 'failed' ? (
            <p className="error" role="alert">{tr('Could not copy the link. Select it and copy it manually.')}</p>
          ) : null}
        </div>
      ) : null}
      {error ? <p className="error" role="alert">{error}</p> : null}
    </section>
  )
}
