import { Check, RefreshCw, ShieldCheck, Users } from 'lucide-react'
import { useEffect, useId, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import type { InstanceMember, ManagedInstance, ManagementClient } from '../runtime/managementClient'
import { environmentAccess, type EnvironmentAccess, type EnvironmentAccessMember } from '../runtime/environmentAccess'
import './EnterpriseEnvironmentAccess.css'

type Props = { client: ManagementClient; instance: ManagedInstance; environmentId: string; environmentName: string; canManage: boolean; blocked?: boolean }

export default function EnterpriseEnvironmentAccess(props: Props) {
  const { i18n } = useTranslation()
  const [open, setOpen] = useState(false)
  const title = i18n.language.startsWith('de') ? 'Laufzeitzugriff' : 'Runtime access'
  return <details className="enterprise-environment-access" onToggle={event => setOpen(event.currentTarget.open)}>
    <summary><Users size={15} aria-hidden="true" />{title}</summary>
    {open && <EnvironmentAccessEditor key={JSON.stringify([props.instance.organisation_id, props.instance.id, props.environmentId])} {...props} />}
  </details>
}

function EnvironmentAccessEditor({ client, instance, environmentId, environmentName, canManage, blocked = false }: Props) {
  const { i18n } = useTranslation()
  const t = (en: string, de: string) => i18n.language.startsWith('de') ? de : en
  const groupId = useId()
  const [api] = useState(() => environmentAccess(client, instance, environmentId))
  const [current, setCurrent] = useState<EnvironmentAccess | null>(null)
  const [mode, setMode] = useState<EnvironmentAccess['mode']>('inherit_instance')
  const [members, setMembers] = useState<EnvironmentAccessMember[]>([])
  const [candidates, setCandidates] = useState<InstanceMember[]>([])
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  const [saved, setSaved] = useState(false)
  const [reload, setReload] = useState(0)
  const alive = useRef(false)
  const inFlight = useRef(false)
  const scopeMatches = (value: EnvironmentAccess) => value.organisation_id === instance.organisation_id && value.instance_id === instance.id && value.environment_id === environmentId
  const eligible = candidates.filter(person => !['auditor', 'suspended'].includes(person.organisation_role))
  const unavailable = members.filter(member => !eligible.some(person => person.user_id === member.user_id))

  useEffect(() => {
    const abort = new AbortController()
    alive.current = true; setLoading(true); setCurrent(null); setError(''); setSaved(false)
    void Promise.all([api.read(abort.signal), client.instanceMembers(instance, abort.signal)]).then(([access, people]) => {
      if (abort.signal.aborted) return
      if (access.organisation_id !== instance.organisation_id || access.instance_id !== instance.id || access.environment_id !== environmentId) throw new Error('The access response belongs to another environment.')
      setCurrent(access); setMode(access.mode); setMembers(access.members); setCandidates(people)
    }).catch(cause => { if (!abort.signal.aborted) setError(cause instanceof Error ? cause.message : String(cause)) })
      .finally(() => { if (!abort.signal.aborted) setLoading(false) })
    return () => { alive.current = false; abort.abort() }
    // Instance revisions change during polling; the ACL has its own revision.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api, client, instance.id, instance.organisation_id, environmentId, reload])

  const save = async () => {
    if (!current || !canManage || blocked || loading || inFlight.current || mode === 'restricted' && unavailable.length) return
    inFlight.current = true; setSaving(true); setError(''); setSaved(false)
    try {
      const value = await api.update({ expected_revision: current.revision, mode, members: mode === 'restricted' ? members : [] })
      if (!alive.current) return
      if (!scopeMatches(value)) throw new Error('The access response belongs to another environment.')
      setCurrent(value); setMode(value.mode); setMembers(value.members); setSaved(true)
    } catch (cause) { if (alive.current) setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { inFlight.current = false; if (alive.current) setSaving(false) }
  }

  return <div className="enterprise-environment-access-editor" aria-label={`${t('Runtime access for', 'Laufzeitzugriff für')} ${environmentName}`}>
    <p>{t('Instance membership remains required. Runtime access narrows those existing permissions; it does not grant access to this instance.', 'Eine Instanzmitgliedschaft bleibt erforderlich. Der Laufzeitzugriff schränkt bestehende Rechte ein und erteilt keinen Zugang zu dieser Instanz.')}</p>
    {loading ? <p role="status">{t('Loading runtime access…', 'Laufzeitzugriff wird geladen…')}</p> : current && <form onSubmit={event => { event.preventDefault(); void save() }}>
      <fieldset disabled={!canManage || saving || blocked}>
        <legend>{t('Who can use this runtime?', 'Wer darf diese Laufzeit verwenden?')}</legend>
        <label className="enterprise-environment-access-mode"><input name={groupId} type="radio" value="inherit_instance" checked={mode === 'inherit_instance'} onChange={() => { setMode('inherit_instance'); setSaved(false) }} /><span><strong>{t('All authorised instance members', 'Alle zugelassenen Instanzmitglieder')}</strong><small>{t('Keep the instance’s existing access rules.', 'Die vorhandenen Zugriffsregeln der Instanz übernehmen.')}</small></span></label>
        <label className="enterprise-environment-access-mode"><input name={groupId} type="radio" value="restricted" checked={mode === 'restricted'} onChange={() => { setMode('restricted'); setSaved(false) }} /><span><strong>{t('Selected members only', 'Nur ausgewählte Mitglieder')}</strong><small>{t('Assign viewing or execution permission below.', 'Lese- oder Ausführungsrechte unten gezielt zuweisen.')}</small></span></label>
        {mode === 'restricted' && <div className="enterprise-environment-access-members">
          <p className="enterprise-environment-access-roles"><ShieldCheck size={15} aria-hidden="true" />{t('Viewer: inspect only. Runner: execute. No selected members means nobody can use this runtime, including administrators.', 'Leser: nur ansehen. Ausführen: Aufgaben starten. Ohne ausgewählte Mitglieder kann niemand diese Laufzeit verwenden, auch keine Administration.')}</p>
          {!eligible.length && <p>{t('No eligible instance members. Assign instance membership first.', 'Keine geeigneten Instanzmitglieder vorhanden. Vergib zuerst eine Instanzmitgliedschaft.')}</p>}
          {eligible.map(person => {
            const member = members.find(value => value.user_id === person.user_id)
            return <div key={person.user_id} className="enterprise-environment-access-person"><label><input type="checkbox" checked={!!member} onChange={event => { setMembers(values => event.target.checked ? [...values, { user_id: person.user_id, role: 'viewer' }] : values.filter(value => value.user_id !== person.user_id)); setSaved(false) }} /><span><strong>{person.display_name}</strong><small>{person.email}</small></span></label>{member && <select aria-label={`${t('Runtime role for', 'Laufzeitrolle für')} ${person.display_name}`} value={member.role} onChange={event => { const role = event.target.value as EnvironmentAccessMember['role']; setMembers(values => values.map(value => value.user_id === person.user_id ? { ...value, role } : value)); setSaved(false) }}><option value="viewer">{t('Viewer · inspect', 'Leser · ansehen')}</option><option value="runner">{t('Runner · execute', 'Ausführen · Aufgaben starten')}</option></select>}</div>
          })}
          {unavailable.map(member => <div className="enterprise-environment-access-unavailable" key={member.user_id}><p>{t('No longer an eligible instance member', 'Kein zugelassenes Instanzmitglied mehr')}: <code>{member.user_id}</code></p><button type="button" onClick={() => { setMembers(values => values.filter(value => value.user_id !== member.user_id)); setSaved(false) }}>{t('Remove unavailable assignment', 'Nicht verfügbare Zuweisung entfernen')}</button></div>)}
        </div>}
      </fieldset>
      {blocked && <p className="enterprise-environment-access-notice">{t('Wait for the current runtime operation to finish before changing access.', 'Warte, bis der aktuelle Laufzeitvorgang abgeschlossen ist, bevor du den Zugriff änderst.')}</p>}
      <p className="enterprise-environment-access-hint">{t('The server checks the current permissions and blocks changes while tasks or infrastructure operations are unsettled.', 'Der Server prüft die aktuellen Berechtigungen und sperrt Änderungen, solange Aufgaben oder Infrastrukturvorgänge ungeklärt sind.')}</p>
      {canManage && <button className="ui-button ui-button--primary" type="submit" disabled={saving || blocked || mode === 'restricted' && !!unavailable.length}><ShieldCheck size={14} aria-hidden="true" />{saving ? t('Saving…', 'Wird gespeichert…') : t('Save runtime access', 'Laufzeitzugriff speichern')}</button>}
    </form>}
    {error && <p className="enterprise-environment-access-error" role="alert">{error}</p>}
    {saved && <p className="enterprise-environment-access-saved" role="status"><Check size={15} aria-hidden="true" />{t('Runtime access saved.', 'Laufzeitzugriff gespeichert.')}</p>}
    <button className="ui-button ui-button--secondary" type="button" disabled={saving || loading} onClick={() => setReload(value => value + 1)}><RefreshCw size={14} aria-hidden="true" />{t('Reload current access', 'Aktuellen Zugriff neu laden')}</button>
  </div>
}
