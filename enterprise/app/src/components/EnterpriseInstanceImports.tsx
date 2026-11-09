import { useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { z } from 'zod'
import type { ManagementClient, ManagedInstance, InstanceBackup } from '../runtime/managementClient'
import type { ImportIntent, ImportReceipt, InstanceImport, InstanceRoute } from '../runtime/instanceImportContracts'
import { acceptImportReceipt, readImportJournal, retainImportIntent, type ImportJournal, type ImportJournalScope } from '../runtime/instanceImportJournal'

type Props = { client: ManagementClient; orgId: string; serverUrl: string; userId: string; instances: ManagedInstance[]; canManage: boolean; onChanged: () => void; onOpen: (instance: ManagedInstance) => void }
const message = (reason: unknown) => reason instanceof Error ? reason.message : String(reason)

export default function EnterpriseInstanceImports({ client, orgId, serverUrl, userId, instances, canManage, onChanged, onOpen }: Props) {
  const { i18n } = useTranslation()
  const t = (en: string, de: string) => i18n.language.startsWith('de') ? de : en
  const scope = useMemo<ImportJournalScope>(() => ({ server: serverUrl, user: userId, organisation: orgId }), [serverUrl, userId, orgId])
  const contextKey = JSON.stringify(scope)
  const context = useRef(contextKey)
  context.current = contextKey
  const mounted = useRef(true)
  const writing = useRef<string | null>(null)
  const [journal, setJournal] = useState<ImportJournal | null>(null)
  const [journalError, setJournalError] = useState('')
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const [sourceId, setSourceId] = useState('')
  const [targetId, setTargetId] = useState('')
  const [backupId, setBackupId] = useState('')
  const [backups, setBackups] = useState<InstanceBackup[]>([])
  const [backupLoading, setBackupLoading] = useState(false)
  const [acknowledged, setAcknowledged] = useState(false)
  const [importId, setImportId] = useState('')
  const [view, setView] = useState<InstanceImport | null>(null)
  const [route, setRoute] = useState<InstanceRoute | null>(null)
  const [loading, setLoading] = useState(false)
  const [cutoverAcknowledged, setCutoverAcknowledged] = useState(false)
  const [abortAcknowledged, setAbortAcknowledged] = useState(false)
  const active = (captured: string) => mounted.current && context.current === captured

  useEffect(() => {
    mounted.current = true
    writing.current = null; setBusy(false); setLoading(false)
    setJournal(null); setJournalError(''); setError(''); setView(null); setRoute(null)
    setSourceId(''); setTargetId(''); setBackupId(''); setBackups([]); setAcknowledged(false)
    setCutoverAcknowledged(false); setAbortAcknowledged(false); setImportId('')
    try {
      const saved = readImportJournal(scope)
      setJournal(saved)
      const latest = saved.accepted.at(-1)
      if (latest) setImportId(latest.receipt.id)
    } catch (reason) { setJournalError(message(reason)) }
    return () => { mounted.current = false }
  }, [scope])

  const eligible = instances.filter(value => value.organisation_id === orgId && value.backend === 'compose' && value.state === 'stopped')
  const source = eligible.find(value => value.id === sourceId)
  const targets = eligible.filter(value => value.id !== sourceId && value.owner_user_id === source?.owner_user_id && value.execution_required && value.current_release)
  const target = targets.find(value => value.id === targetId)
  const selectedBackup = backups.find(value => value.id === backupId && value.instance_id === sourceId && value.organisation_id === orgId)
  useEffect(() => { setAcknowledged(false) }, [source?.revision, target?.revision, sourceId, targetId, backupId])

  useEffect(() => {
    setBackups([]); setBackupId(''); setBackupLoading(false)
    if (!source || !canManage) return
    const abort = new AbortController(), captured = contextKey
    setBackupLoading(true)
    void client.backups(source, abort.signal).then(values => {
      if (active(captured) && !abort.signal.aborted) setBackups(values.filter(value => value.instance_id === source.id && value.organisation_id === orgId))
    }).catch(reason => { if (active(captured) && !abort.signal.aborted) setError(message(reason)) })
      .finally(() => { if (active(captured) && !abort.signal.aborted) setBackupLoading(false) })
    return () => abort.abort()
  }, [client, orgId, sourceId, canManage, contextKey, source?.revision])

  async function refresh(id: string) {
    const captured = contextKey
    setLoading(true); setError(''); setCutoverAcknowledged(false); setAbortAcknowledged(false)
    try {
      z.uuid().parse(id)
      const value = await client.instanceImport(orgId, id)
      if (value.id !== id || value.organisation_id !== orgId) throw new Error('Import response scope changed')
      const currentRoute = await client.instanceRoute(orgId, value.source_instance_id)
      if (currentRoute.organisation_id !== orgId || currentRoute.source_instance_id !== value.source_instance_id) throw new Error('Import route scope changed')
      if (active(captured)) { setView(value); setRoute(currentRoute) }
    } catch (reason) { if (active(captured)) { setView(null); setRoute(null); setError(message(reason)) } }
    finally { if (active(captured)) setLoading(false) }
  }

  async function send(intent: ImportIntent) {
    if (!canManage || writing.current || !journal || journalError) return
    const captured = contextKey
    const writeId = crypto.randomUUID()
    writing.current = writeId; setBusy(true); setError('')
    try {
      // Persist exact immutable body/key BEFORE the first POST. Reload never POSTs.
      const retained = await retainImportIntent(scope, intent)
      if (!active(captured)) return
      setJournal(retained)
      let receipt: ImportReceipt
      if (intent.kind === 'prepare') receipt = await client.prepareInstanceImport(orgId, intent.body)
      else if (intent.kind === 'cutover') receipt = await client.cutoverInstanceImport(orgId, intent.import_id, intent.body)
      else receipt = await client.abortInstanceImport(orgId, intent.import_id, intent.body)
      // Save an old scope's received receipt, but never display it in a new scope.
      const accepted = await acceptImportReceipt(scope, intent, receipt)
      if (active(captured)) {
        setJournal(accepted); setImportId(receipt.id); setAcknowledged(false)
        setCutoverAcknowledged(false); setAbortAcknowledged(false); setView(null); setRoute(null)
        onChanged()
        await refresh(receipt.id)
      }
    } catch (reason) { if (active(captured)) setError(message(reason)) }
    finally { if (writing.current === writeId) { writing.current = null; if (active(captured)) setBusy(false) } }
  }

  const disabled = busy || loading || !journal || !!journalError || !canManage
  const pending = journal?.pending
  const canCutover = view?.phase === 'qualified' && route?.target_instance_id === view.source_instance_id
  const canAbort = view?.phase === 'source_quiescent' || view?.phase === 'export_verified'
  const openTarget = view?.phase === 'source_retained' && route?.import_id === view.id && route.target_instance_id === view.target_instance_id
    ? instances.find(value => value.id === view.target_instance_id && value.organisation_id === orgId && value.can_open) : undefined
  const phases: Record<InstanceImport['phase'], string> = {
    prepared: t('Preparing source stop', 'Quellstopp wird vorbereitet'), source_quiescent: t('Source stopped', 'Quelle gestoppt'),
    export_verified: t('Original backup verified', 'Originalsicherung geprüft'), target_restored_paused: t('Target restored and paused', 'Ziel wiederhergestellt und pausiert'),
    qualified: t('Target checks completed; awaiting switch', 'Zielprüfungen abgeschlossen; Wechsel ausstehend'), source_retained: t('Route switched; source retained', 'Route umgestellt; Quelle erhalten'), aborted: t('Import canceled; both instances stopped', 'Import abgebrochen; beide Instanzen gestoppt'),
  }
  return <section aria-label={t('Instance imports', 'Instanzimporte')}>
    <h2>{t('Move into a new instance', 'In eine neue Instanz übernehmen')}</h2>
    <p>{t('Choose an original source backup and a fresh stopped target in this organisation. The server checks its fixed host, capacity and owner. Source and target stay stopped during review. Existing runs and schedules require review; they are not restarted by an import.', 'Wähle eine Original-Quellsicherung und ein frisches, gestopptes Ziel dieser Organisation. Der Server prüft dessen feste Hostbindung, Kapazität und Eigentümer. Quelle und Ziel bleiben während der Prüfung gestoppt. Bestehende Läufe und Zeitpläne erfordern eine Prüfung; ein Import startet sie nicht erneut.')}</p>
    {journalError && <p role="alert">{t('The saved import journal cannot be read. Preserve it for review before sending another action.', 'Das gespeicherte Importjournal ist nicht lesbar. Bewahre es zur Prüfung auf, bevor du einen weiteren Vorgang sendest.')} {journalError}</p>}
    {error && <p role="alert" className="enterprise-error">{error}</p>}
    {pending && <div className="enterprise-confirm"><h3>{t('Unconfirmed import action', 'Unbestätigter Importvorgang')}</h3><p>{t('The original request is retained. Checking status or reconnecting sends no action. An explicit retry sends the same body and key to retrieve its original receipt.', 'Die ursprüngliche Anfrage bleibt erhalten. Statusprüfung und Wiederverbindung senden keinen Vorgang. Ein ausdrücklicher erneuter Versuch sendet denselben Inhalt und Schlüssel, um die ursprüngliche Quittung abzurufen.')}</p><code>{pending.body.idempotency_key}</code><button type="button" disabled={disabled} onClick={() => void send(pending)}>{t('Retry retained action', 'Gespeicherten Vorgang erneut anfragen')}</button></div>}
    {canManage && !pending && <form onSubmit={event => {
      event.preventDefault()
      if (disabled || !acknowledged || !source || !target || !selectedBackup || backupLoading) return
      void send({ kind: 'prepare', body: { source_instance_id: source.id, target_instance_id: target.id, source_revision: source.revision, target_revision: target.revision, backup_id: selectedBackup.id, idempotency_key: crypto.randomUUID() } })
    }}>
      <label>{t('Import source', 'Importquelle')}<select value={sourceId} disabled={disabled} onChange={event => { setSourceId(event.target.value); setTargetId(''); setError('') }}><option value="">{t('Select stopped source', 'Gestoppte Quelle wählen')}</option>{eligible.map(value => <option key={value.id} value={value.id}>{value.name}</option>)}</select></label>
      <label>{t('New import target', 'Neues Importziel')}<select value={targetId} disabled={disabled || !source} onChange={event => setTargetId(event.target.value)}><option value="">{t('Select fresh enrolled target', 'Frisches Ziel mit zentralen Limits wählen')}</option>{targets.map(value => <option key={value.id} value={value.id}>{value.name}</option>)}</select></label>
      <label>{t('Original source backup', 'Original-Quellsicherung')}<select value={backupId} disabled={disabled || backupLoading || !source} onChange={event => setBackupId(event.target.value)}><option value="">{t('Select backup', 'Sicherung wählen')}</option>{backups.map(value => <option key={value.id} value={value.id}>{value.created_at} · {value.id}</option>)}</select></label>
      <label><input type="checkbox" checked={acknowledged} disabled={disabled} onChange={event => setAcknowledged(event.target.checked)} />{t('I authorize preparing this stopped source and target for import. A later route switch needs a separate confirmation.', 'Ich erlaube die Importvorbereitung dieser gestoppten Quelle und dieses Ziels. Der spätere Routenwechsel benötigt eine eigene Bestätigung.')}</label>
      <button disabled={disabled || !source || !target || !selectedBackup || !acknowledged || backupLoading}>{t('Prepare import', 'Import vorbereiten')}</button>
    </form>}
    <form onSubmit={event => { event.preventDefault(); if (!busy && !loading) void refresh(importId) }}><label>{t('Import ID', 'Import-ID')}<input value={importId} disabled={busy || loading} onChange={event => { setImportId(event.target.value); setView(null); setRoute(null); setCutoverAcknowledged(false); setAbortAcknowledged(false) }} /></label><button disabled={busy || loading || !z.uuid().safeParse(importId).success}>{t('Check import status', 'Importstatus prüfen')}</button></form>
    {loading && <p role="status">{t('Checking import…', 'Import wird geprüft…')}</p>}
    {view && route && <article><h3>{phases[view.phase]}</h3><p>{t('Source', 'Quelle')}: <code>{view.source_instance_id}</code> · {t('Target', 'Ziel')}: <code>{view.target_instance_id}</code></p><p>{t('Current destination', 'Aktuelles Routenziel')}: <code>{route.target_instance_id}</code></p>
      {canManage && !pending && canCutover && <form className="enterprise-confirm" onSubmit={event => {
        event.preventDefault()
        if (!disabled && cutoverAcknowledged) void send({ kind: 'cutover', import_id: view.id, source_instance_id: view.source_instance_id, body: { expected_revision: view.revision, expected_route_revision: route.revision, target_instance_id: view.target_instance_id, idempotency_key: crypto.randomUUID() } })
      }}><p>{t('Switch the source route to this checked target. The source remains retained and the target stays stopped. Starting the target and resuming schedules are separate actions.', 'Stelle die Quellroute auf dieses geprüfte Ziel um. Die Quelle bleibt erhalten, das Ziel gestoppt. Zielstart und Freigabe der Zeitpläne sind eigene Vorgänge.')}</p><label><input type="checkbox" checked={cutoverAcknowledged} disabled={disabled} onChange={event => setCutoverAcknowledged(event.target.checked)} />{t('I confirm switching to this target.', 'Ich bestätige den Wechsel zu diesem Ziel.')}</label><button disabled={disabled || !cutoverAcknowledged}>{t('Switch to checked target', 'Zum geprüften Ziel wechseln')}</button></form>}
      {canManage && !pending && canAbort && <form className="enterprise-confirm" onSubmit={event => {
        event.preventDefault()
        if (!disabled && abortAcknowledged) void send({ kind: 'abort', import_id: view.id, source_instance_id: view.source_instance_id, target_instance_id: view.target_instance_id, body: { expected_revision: view.revision, idempotency_key: crypto.randomUUID() } })
      }}><p>{t('Cancellation is possible only before target work is claimed. Both instances remain stopped and their capacity remains held. Later or uncertain effects require operator review.', 'Abbruch ist nur möglich, bevor Zielarbeit übernommen wurde. Beide Instanzen bleiben gestoppt und ihre Kapazität gebucht. Spätere oder unklare Wirkungen erfordern eine Betreiberprüfung.')}</p><label><input type="checkbox" checked={abortAcknowledged} disabled={disabled} onChange={event => setAbortAcknowledged(event.target.checked)} />{t('I confirm canceling before target work.', 'Ich bestätige den Abbruch vor der Zielarbeit.')}</label><button disabled={disabled || !abortAcknowledged}>{t('Cancel import before target work', 'Import vor Zielarbeit abbrechen')}</button></form>}
      {openTarget && <button type="button" disabled={busy} onClick={() => onOpen(openTarget)}>{t('Open imported target', 'Übernommenes Ziel öffnen')}</button>}
    </article>}
    {!!journal?.accepted.length && <details><summary>{t('Accepted import receipts', 'Angenommene Importquittungen')}</summary><ol>{journal.accepted.map(entry => <li key={entry.intent.body.idempotency_key}><code>{entry.receipt.id}</code> · {entry.intent.kind} · {entry.receipt.revision}</li>)}</ol></details>}
  </section>
}
