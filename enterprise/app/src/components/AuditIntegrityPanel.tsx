import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { safeInvoke } from '../utils/safeInvoke'

export type AuditIntegrityReport = {
  version: 2
  status: 'empty' | 'ok' | 'legacy_clean' | 'legacy_damaged' | 'tampered' | 'unavailable' | 'recovering'
  trustLevel: 'verified' | 'legacy_unverified' | 'broken' | 'unavailable'
  signedEvents: number
  legacyEvents: number
  malformedEvents: number
  firstProblemLine: number | null
  errorCode: string | null
  assessmentId: string | null
  recoveryAllowed: boolean
  recovery: { recoveryId: string; filesPreserved: number; bytesPreserved: number } | null
}

export default function AuditIntegrityPanel() {
  const { t } = useTranslation()
  const [report, setReport] = useState<AuditIntegrityReport | null>(null)
  const [busy, setBusy] = useState(false)
  const [confirming, setConfirming] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const refresh = async () => {
    setBusy(true)
    setConfirming(false)
    setError(null)
    try { setReport(await safeInvoke<AuditIntegrityReport | null>('audit_integrity_inspect', undefined, null)) }
    catch { setError(t('Audit inspection failed. Retry when storage is available.')) }
    finally { setBusy(false) }
  }
  useEffect(() => { void refresh() }, []) // eslint-disable-line react-hooks/exhaustive-deps

  const recover = async () => {
    if (!report?.assessmentId || !report.recoveryAllowed) return
    setBusy(true)
    setError(null)
    try {
      const next = await safeInvoke<AuditIntegrityReport>('audit_recovery_start', { assessmentId: report.assessmentId })
      setReport(next)
      setConfirming(false)
    } catch {
      setConfirming(false)
      setError(t('Audit recovery could not finish. Inspect again before retrying.'))
    } finally { setBusy(false) }
  }

  const labels = {
    empty: t('No audit events'), ok: t('Verified audit chain'), legacy_clean: t('Unverified legacy history'),
    legacy_damaged: t('Damaged legacy history'), tampered: t('Audit integrity failed'),
    unavailable: t('Audit integrity unavailable'), recovering: t('Audit recovery pending'),
  }

  return <section className="panel" aria-labelledby="audit-integrity-title">
    <h2 id="audit-integrity-title" className="settings-section-title">{t('Audit integrity')}</h2>
    <p role="status">{report ? labels[report.status] : t('Not checked')}</p>
    {report && <dl className="about-cowork-details">
      <div><dt>{t('Signed events')}</dt><dd>{report.signedEvents}</dd></div>
      <div><dt>{t('Legacy events')}</dt><dd>{report.legacyEvents}</dd></div>
      <div><dt>{t('Malformed events')}</dt><dd>{report.malformedEvents}</dd></div>
      {report.firstProblemLine != null && <div><dt>{t('First affected line')}</dt><dd>{report.firstProblemLine}</dd></div>}
      {report.errorCode && <div><dt>{t('Reason code')}</dt><dd>{report.errorCode}</dd></div>}
    </dl>}
    {error && <p role="alert">{error}</p>}
    {confirming && <div className="card" role="group" aria-label={t('Confirm audit recovery')}>
      <p>{t('Preserve the old protocol unchanged and start a new verified chain? Earlier events remain unverified; recovery does not prove their authenticity.')}</p>
      <div className="actions">
        <button type="button" disabled={busy} onClick={() => void recover()}>{t('Preserve history and restart audit')}</button>
        <button type="button" disabled={busy} onClick={() => setConfirming(false)}>{t('Cancel')}</button>
      </div>
    </div>}
    <div className="actions">
      <button type="button" className="btn-sm" disabled={busy} onClick={() => void refresh()}>{busy ? t('Checking...') : t('Inspect audit integrity')}</button>
      {report?.recoveryAllowed && !confirming && <button type="button" className="btn-sm" disabled={busy} onClick={() => setConfirming(true)}>{t('Review audit recovery')}</button>}
      {report?.recovery && <button type="button" className="btn-sm" disabled={busy} onClick={() => {
        void safeInvoke('audit_recovery_open_archive', { recoveryId: report.recovery?.recoveryId })
          .catch(() => setError(t('The preserved audit archive could not be opened.')))
      }}>{t('Open preserved audit archive')}</button>}
    </div>
  </section>
}
