import { useEffect, useState } from 'react'
import { ArrowRight, Layers, Pause, Play, ShieldCheck, Square } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { type ManagementClient, type ManagedInstance, type InstanceRelease, type InstanceRollout, type RolloutControl } from '../runtime/managementClient'
import './EnterpriseRollouts.css'

export default function EnterpriseRollouts({ client, orgId, canManage }: { client: ManagementClient; orgId: string; canManage: boolean }) {
  const { i18n } = useTranslation()
  const t = (en: string, de: string) => i18n.language.startsWith('de') ? de : en
  const [instances, setInstances] = useState<ManagedInstance[]>([])
  const [plans, setPlans] = useState<InstanceRollout[]>([])
  const [selected, setSelected] = useState<ManagedInstance[]>([])
  const [releases, setReleases] = useState<InstanceRelease[]>([])
  const [name, setName] = useState('')
  const [release, setRelease] = useState('')
  const [batch, setBatch] = useState(1)
  const [acknowledged, setAcknowledged] = useState(false)
  const [loading, setLoading] = useState(true)
  const [catalogLoading, setCatalogLoading] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [connectionError, setConnectionError] = useState('')
  const [reload, setReload] = useState(0)
  const [control, setControl] = useState<{ plan: InstanceRollout; action: RolloutControl } | null>(null)

  useEffect(() => {
    const abort = new AbortController()
    let timer: ReturnType<typeof setTimeout> | undefined
    const load = async () => {
      try {
        const [rows, values] = await Promise.all([client.rollouts(orgId, abort.signal), client.instances(orgId, abort.signal)])
        if (!abort.signal.aborted) { setPlans(rows.filter(row => row.organisation_id === orgId)); setInstances(values.filter(row => row.organisation_id === orgId)); setConnectionError('') }
      } catch (cause) { if (!abort.signal.aborted) setConnectionError(String(cause)) }
      finally { if (!abort.signal.aborted) { setLoading(false); timer = setTimeout(() => { void load() }, 3000) } }
    }
    void load()
    return () => { abort.abort(); clearTimeout(timer) }
  }, [client, orgId, reload])

  useEffect(() => {
    setReleases([]); setRelease(''); setAcknowledged(false)
    if (!selected.length) { setCatalogLoading(false); return }
    const abort = new AbortController()
    setCatalogLoading(true)
    void Promise.all(selected.map(instance => client.releases(instance, abort.signal)))
      .then(lists => {
        if (abort.signal.aborted) return
        const choices = lists[0].filter(candidate => lists.every((values, index) => values.some(value => value.id === candidate.id && value.qualified && value.source_release === selected[index].current_release)))
        setReleases(choices)
      })
      .catch(cause => { if (!abort.signal.aborted) setError(String(cause)) })
      .finally(() => { if (!abort.signal.aborted) setCatalogLoading(false) })
    return () => abort.abort()
  }, [client, selected])

  const stale = selected.some(value => !instances.some(current => current.id === value.id && current.revision === value.revision && !current.rollout_id))
  const eligible = instances.filter(value => value.can_manage && !value.rollout_id && value.backend === 'compose' && value.policy.minimum_isolation === 'container' && value.current_release && (value.state === 'ready' && value.available_actions?.includes('stop') || value.state === 'stopped' && value.available_actions?.includes('upgrade')))
  const disabled = loading || busy || !!connectionError
  const run = async (work: () => Promise<void>) => {
    setBusy(true); setError('')
    try { await work(); setReload(value => value + 1) } catch (cause) { setError(String(cause)) } finally { setBusy(false) }
  }
  const controlStale = !!control && !plans.some(plan => plan.id === control.plan.id && plan.revision === control.plan.revision)
  const labels: Record<RolloutControl, string> = { pause: t('Pause', 'Pausieren'), resume: t('Resume', 'Fortsetzen'), cancel: t('Cancel rollout', 'Rollout abbrechen') }
  const stateLabel=(value:string)=>({running:t('Running','Läuft'),paused:t('Paused','Pausiert'),canceling:t('Canceling','Wird abgebrochen'),canceled:t('Canceled','Abgebrochen'),succeeded:t('Completed','Abgeschlossen'),waiting:t('Waiting','Wartet'),active:t('In progress','In Bearbeitung'),failed:t('Failed','Fehlgeschlagen')})[value]??value

  return <section className="enterprise-rollouts" aria-label={t('Batch updates', 'Gruppenupdates')}>
    <header className="enterprise-rollouts-heading"><div><span><Layers size={15} aria-hidden="true" />{t('Maintenance','Wartung')}</span><h2>{t('Batch updates', 'Gruppenupdates')}</h2><p>{t('Update several instances in a controlled sequence.','Aktualisiere mehrere Instanzen in einer kontrollierten Reihenfolge.')}</p></div></header>
    <p className="enterprise-rollouts-note"><ShieldCheck size={17} aria-hidden="true" /><span>{t('Updates run in the selected order, one group at a time. Each installation requires a qualified source/version pair and creates its own backup. Any failed or uncertain operation pauses the rollout.', 'Updates folgen der gewählten Reihenfolge, jeweils eine Gruppe nach der anderen. Jede Installation erfordert einen geprüften Versionswechsel und erstellt eine eigene Sicherung. Fehlgeschlagene oder ungeklärte Vorgänge pausieren den Rollout.')}</span></p>
    {error && <p role="alert" className="enterprise-error">{error}</p>}
    {connectionError && <p role="alert">{t('Connection interrupted. Retrying automatically.', 'Verbindung unterbrochen. Der Verbindungsaufbau wird automatisch wiederholt.')} {connectionError}</p>}
    {canManage && <form className="enterprise-rollout-create" onSubmit={event => {
      event.preventDefault()
      if (disabled || stale || !acknowledged || !selected.length || !releases.some(value => value.id === release)) return
      void run(async () => {
        await client.createRollout(orgId, { name: name.trim(), release, batch_size: batch, targets: selected.map(value => ({ instance_id: value.id, expected_revision: value.revision })), idempotency_key: crypto.randomUUID() })
        setSelected([]); setName(''); setAcknowledged(false)
      })
    }}>
      <h3>{t('Plan an update','Update planen')}</h3>
      <label className="enterprise-rollout-wide">{t('Rollout name', 'Rollout-Name')}<input required maxLength={120} value={name} disabled={disabled} placeholder={t('e.g. Team workspace update','z. B. Update der Team-Arbeitsplätze')} onChange={event => setName(event.target.value)} /></label>
      <fieldset className="enterprise-rollout-wide" disabled={disabled}><legend>{t('Instances in selection order', 'Instanzen in Auswahlreihenfolge')}</legend>
        {!eligible.length && <p>{t('No eligible instances. Complete the current operations and qualify a compatible release first.', 'Keine geeigneten Instanzen. Schließe laufende Vorgänge ab und prüfe zunächst eine kompatible Version.')}</p>}
        {eligible.map(instance => <label key={instance.id} className="enterprise-check"><input type="checkbox" checked={selected.some(value => value.id === instance.id)} disabled={selected.length >= 40 && !selected.some(value => value.id === instance.id)} onChange={event => { setSelected(values => event.target.checked ? [...values, instance] : values.filter(value => value.id !== instance.id)); setAcknowledged(false) }} />{instance.name} · {instance.state}</label>)}
        {selected.length > 0 && <ol>{selected.map(value => <li key={value.id}>{value.name}</li>)}</ol>}
        {selected.length > 0 && <button type="button" onClick={() => setSelected([])}>{t('Clear selection', 'Auswahl leeren')}</button>}
      </fieldset>
      <label>{t('Qualified release', 'Geprüfte Version')}<select required value={release} disabled={disabled || catalogLoading} onChange={event => { setRelease(event.target.value); setAcknowledged(false) }}><option value="">{t('Select a release qualified for every instance', 'Für alle Instanzen geprüfte Version auswählen')}</option>{releases.map(value => <option key={value.id} value={value.id}>{value.name} · {value.id.slice(0, 12)}</option>)}</select></label>
      <label>{t('Instances per group', 'Instanzen pro Gruppe')}<input type="number" min={1} max={10} required value={batch} disabled={disabled} onChange={event => { setBatch(Number(event.target.value)); setAcknowledged(false) }} /></label>
      <label className="enterprise-check enterprise-rollout-ack enterprise-rollout-wide"><input type="checkbox" checked={acknowledged} onChange={event => setAcknowledged(event.target.checked)} />{t('Allow maintenance downtime: running instances are stopped and restarted after their update; stopped instances remain stopped. Active tasks may be interrupted; users and devices must reconnect.', 'Wartungsunterbrechung erlauben: Laufende Instanzen werden gestoppt und nach ihrem Update neu gestartet; gestoppte Instanzen bleiben gestoppt. Aktive Aufgaben können unterbrochen werden; Nutzer und Geräte müssen sich erneut verbinden.')}</label>
      {stale && <p role="status">{t('The selected instances changed. Clear the selection and choose them again.', 'Die gewählten Instanzen haben sich geändert. Leere die Auswahl und wähle sie erneut.')}</p>}
      <button className="ui-button ui-button--primary" disabled={disabled || stale || !acknowledged || !selected.length || !release || catalogLoading}>{t('Start rollout', 'Rollout starten')}<ArrowRight size={15} aria-hidden="true" /></button>
    </form>}
    {loading && <p role="status">{t('Loading rollouts…', 'Rollouts werden geladen…')}</p>}
    {!loading && !plans.length && <div className="enterprise-rollouts-empty"><Layers size={24} aria-hidden="true" /><strong>{t('No rollouts yet.', 'Noch keine Rollouts vorhanden.')}</strong><p>{t('Planned and completed updates appear here.','Geplante und abgeschlossene Updates erscheinen hier.')}</p></div>}
    {plans.map(plan => <article key={plan.id} className="enterprise-rollout"><header><h3>{plan.name}</h3><span className="enterprise-rollout-status" data-state={plan.state}>{stateLabel(plan.state)}</span></header><div className="enterprise-rollout-summary"><span>{plan.targets.filter(value => value.state === 'succeeded').length}/{plan.targets.length} {t('instances completed','Instanzen abgeschlossen')}</span><code title={plan.release}>{t('Version','Version')} · {plan.release.slice(0, 12)}</code></div><progress aria-label={t('Completed instances','Abgeschlossene Instanzen')} value={plan.targets.filter(value=>value.state==='succeeded').length} max={plan.targets.length||1}/>
      {plan.error_code && <p role="status">{t('Review the failed instance operation. Cancel this plan after reconciliation before creating a new rollout; failed actions are never automatically retried.', 'Prüfe den fehlgeschlagenen Instanzvorgang. Brich diesen Plan nach dem Abgleich ab, bevor du einen neuen Rollout anlegst; fehlgeschlagene Aktionen werden nie automatisch wiederholt.')} <code>{plan.error_code}</code></p>}
      {canManage && <div className="enterprise-actions">{(plan.state === 'running' ? ['pause', 'cancel'] : plan.state === 'paused' ? ['resume', 'cancel'] : []).map(action => <button className={`ui-button ${action==='cancel'?'ui-button--ghost':'ui-button--secondary'}`} key={action} type="button" disabled={disabled || action === 'resume' && plan.targets.some(value => value.state === 'failed')} onClick={() => setControl({ plan, action: action as RolloutControl })}>{action==='pause'?<Pause size={14} aria-hidden="true" />:action==='resume'?<Play size={14} aria-hidden="true" />:<Square size={14} aria-hidden="true" />}{labels[action as RolloutControl]}</button>)}</div>}
      <ol className="enterprise-rollout-targets">{plan.targets.map(value => <li key={value.instance_id}><span><small>{t('Group', 'Gruppe')} {Math.floor(value.ordinal / plan.batch_size) + 1}</small><strong>{value.instance_name}</strong></span><span className="enterprise-rollout-target-state">{stateLabel(value.state)}{value.next_action ? ` · ${{stop:t('Stop','Stoppen'),upgrade:t('Update','Aktualisieren'),start:t('Start','Starten')}[value.next_action]}` : ''}{value.error_code ? <code>{value.error_code}</code> : null}</span></li>)}</ol>
    </article>)}
    {control && <form className="enterprise-confirm enterprise-rollout-confirm" onSubmit={event => { event.preventDefault(); if (disabled || controlStale) return; void run(async () => { await client.controlRollout(control.plan, control.action); setControl(null) }) }}>
      <h3>{labels[control.action]}: {control.plan.name}</h3><p>{t('Already running operations finish or require reconciliation. Canceling prevents new steps and releases each instance only after its operation is settled. Installed versions are not rolled back automatically, and stopped instances may need a manual start.', 'Bereits laufende Vorgänge enden oder erfordern einen Abgleich. Abbrechen verhindert neue Schritte und gibt jede Instanz erst nach Abschluss ihres Vorgangs frei. Installierte Versionen werden nicht automatisch zurückgesetzt; gestoppte Instanzen müssen gegebenenfalls manuell gestartet werden.')}</p>
      {controlStale && <p role="status">{t('This rollout changed. Close and reopen the confirmation.', 'Der Rollout hat sich geändert. Schließe die Bestätigung und öffne sie erneut.')}</p>}
      <button className={`ui-button ${control.action==='cancel'?'ui-button--danger':'ui-button--primary'}`} disabled={disabled || controlStale}>{t('Confirm rollout action', 'Rollout-Aktion bestätigen')}</button><button className="ui-button ui-button--secondary" type="button" onClick={() => setControl(null)}>{t('Back', 'Zurück')}</button>
    </form>}
  </section>
}
