import { useEffect, useRef, useState, type FormEvent } from 'react'
import { useTranslation } from 'react-i18next'
import type { ManagementClient, ManagedInstance } from '../runtime/managementClient'
import type { InstanceMember, InstanceMemberCandidate, InstanceMemberRole } from '../runtime/enterpriseContracts'

export default function EnterpriseInstanceMembers({ client, instance, onClose, onChanged }: { client: ManagementClient; instance: ManagedInstance; onClose: () => void; onChanged: () => void }) {
  const { i18n } = useTranslation()
  const t = (en: string, de: string) => i18n.language.startsWith('de') ? de : en
  const [members, setMembers] = useState<InstanceMember[]>([])
  const [candidates, setCandidates] = useState<InstanceMemberCandidate[]>([])
  const [userId, setUserId] = useState('')
  const [role, setRole] = useState<InstanceMemberRole>('viewer')
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [reload, setReload] = useState(0)
  const live = useRef(false)
  useEffect(() => { live.current = true; return () => { live.current = false } }, [])
  useEffect(() => {
    const abort = new AbortController()
    setLoading(true); setError(''); setCandidates([])
    void Promise.all([client.instanceMembers(instance, abort.signal), instance.can_manage ? client.instanceMemberCandidates(instance, abort.signal) : Promise.resolve([])])
      .then(([rows, people]) => { if (!abort.signal.aborted) { setMembers(rows); setCandidates(people); setUserId(''); setRole('viewer') } })
      .catch(cause => { if (!abort.signal.aborted) { setMembers([]); setError(String(cause)) } })
      .finally(() => { if (!abort.signal.aborted) setLoading(false) })
    return () => abort.abort()
  // Membership polling in the parent creates new instance objects.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, instance.id, instance.organisation_id, instance.can_manage, reload])
  const labels = { owner: t('Owner', 'Eigentümer'), manager: t('Instance manager', 'Instanzverwalter'), member: t('Member', 'Mitglied'), viewer: t('Read only', 'Nur lesen') }
  const run = async (work: () => Promise<unknown>) => {
    if (!instance.can_manage || loading || busy) return
    setBusy(true); setError('')
    try { await work(); if (live.current) { setReload(value => value + 1); onChanged() } }
    catch (cause) { if (live.current) setError(String(cause)) }
    finally { if (live.current) setBusy(false) }
  }
  const assign = (event: FormEvent) => { event.preventDefault(); if (candidates.some(person => person.user_id === userId)) void run(() => client.setInstanceMember(instance, userId, role)) }
  return <section className="enterprise-confirm enterprise-records" aria-label={t('Instance members', 'Instanzmitglieder')}>
    <header><h3>{t('Instance members', 'Instanzmitglieder')}: {instance.name}</h3><button type="button" onClick={onClose}>{t('Close', 'Schließen')}</button></header>
    <p>{t('Managers can operate this instance and assign members. Members can work within project permissions. Read-only members can view shared content. Assigning a role does not share every project.', 'Verwalter können diese Instanz betreiben und Mitglieder zuweisen. Mitglieder arbeiten innerhalb ihrer Projektrechte. Leser können freigegebene Inhalte ansehen. Eine Rollenzuweisung gibt nicht alle Projekte frei.')}</p>
    <button type="button" disabled={busy || loading} onClick={() => setReload(value => value + 1)}>{t('Refresh members', 'Mitglieder aktualisieren')}</button>
    {loading && <p role="status">{t('Loading members…', 'Mitglieder werden geladen…')}</p>}
    {instance.can_manage && <form onSubmit={assign}>
      <label>{t('Organisation member', 'Organisationsmitglied')}<select value={userId} disabled={loading || busy} onChange={event => { const id = event.target.value; setUserId(id); const current = members.find(row => row.user_id === id)?.role; setRole(current && current !== 'owner' ? current : 'viewer') }}><option value="">{t('Select member', 'Mitglied auswählen')}</option>{candidates.map(person => <option key={person.user_id} value={person.user_id}>{person.display_name} · {person.email}</option>)}</select></label>
      <label>{t('Instance role', 'Instanzrolle')}<select value={role} disabled={loading || busy} onChange={event => setRole(event.target.value as InstanceMemberRole)}>{(['viewer', 'member', 'manager'] as const).map(value => <option key={value} value={value}>{labels[value]}</option>)}</select></label>
      <button type="submit" disabled={loading || busy || !candidates.some(person => person.user_id === userId)}>{t('Save instance role', 'Instanzrolle speichern')}</button>
    </form>}
    <table><thead><tr><th>{t('Name', 'Name')}</th><th>{t('Instance role', 'Instanzrolle')}</th><th>{t('Access', 'Zugriff')}</th></tr></thead><tbody>{members.map(person => <tr key={person.user_id}><td>{person.display_name}<br />{person.email}</td><td>{labels[person.role]}{person.organisation_role === 'auditor' && <p>{t('Audit only; instance access denied', 'Nur Audit; Instanzzugriff gesperrt')}</p>}</td><td>{person.role === 'owner' ? t('Instance owner', 'Instanzeigentümer') : instance.can_manage && <button type="button" disabled={busy || loading} aria-label={`${t('Remove access for', 'Zuweisung entziehen für')} ${person.display_name}`} onClick={() => { void run(() => client.removeInstanceMember(instance, person.user_id)) }}>{t('Remove assignment', 'Zuweisung entziehen')}</button>}</td></tr>)}</tbody></table>
    <p>{t('Removing an assignment stops ordinary instance access. Organisation administrators may still use the separately governed support access.', 'Der Entzug beendet den regulären Instanzzugriff. Organisationsadministratoren können weiterhin den gesondert geregelten Support-Zugriff nutzen.')}</p>
    {error && <p role="alert">{error}</p>}
  </section>
}
