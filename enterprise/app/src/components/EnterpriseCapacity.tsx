import { useEffect, useState, type FormEvent } from 'react'
import { useTranslation } from 'react-i18next'
import { ManagementClient, type ExecutionCapacity, type ManagedInstance } from '../runtime/managementClient'

export default function EnterpriseCapacity({client,orgId,canManage}:{client:ManagementClient;orgId:string;canManage:boolean}) {
  const {i18n}=useTranslation()
  const t=(en:string,de:string)=>i18n.language.startsWith('de')?de:en
  const [capacity,setCapacity]=useState<ExecutionCapacity|null>(null)
  const [instances,setInstances]=useState<ManagedInstance[]>([])
  const [draft,setDraft]=useState<{revision:number;organisation:string;user:string}|null>(null)
  const [pending,setPending]=useState<{instance:ManagedInstance;ids:string[]}|null>(null)
  const [ack,setAck]=useState(false)
  const [busy,setBusy]=useState(false)
  const [error,setError]=useState('')
  const [loadError,setLoadError]=useState('')
  const [message,setMessage]=useState('')
  const [reload,setReload]=useState(0)
  useEffect(()=>{
    const abort=new AbortController()
    let timer:ReturnType<typeof setTimeout>|undefined
    const load=async()=>{
      try {
        const [next,fleet]=await Promise.all([client.executionCapacity(orgId,abort.signal),client.instances(orgId,abort.signal)])
        if(abort.signal.aborted)return
        setCapacity(next);setInstances(fleet);setLoadError('')
        setDraft(current=>current??{revision:next.budget.revision,organisation:String(next.budget.organisation_concurrent_runs),user:String(next.budget.user_concurrent_runs)})
      }catch(cause){if(!abort.signal.aborted)setLoadError(String(cause))}
      finally{if(!abort.signal.aborted)timer=setTimeout(()=>void load(),5000)}
    }
    void load()
    return()=>{abort.abort();if(timer)clearTimeout(timer)}
  },[client,orgId,reload])
  const local=(reservation:ExecutionCapacity['reservations'][number])=>reservation.target.kind==='server_linux' && reservation.target.pool_id===null
  const eligible=instances.filter(instance=>instance.organisation_id===orgId && instance.can_manage && instance.backend==='compose'
    && instance.policy.minimum_isolation==='container' && ['ready','stopped'].includes(instance.state)
    && !instance.rollout_id && !!instance.available_actions?.length)
  const changed=!!pending && (!eligible.some(i=>i.id===pending.instance.id && i.revision===pending.instance.revision)
    || !pending.ids.every(id=>capacity?.reservations.some(r=>r.id===id && local(r))))
  const budgetValid=!!draft && /^[1-9]\d*$/.test(draft.organisation) && /^[1-9]\d*$/.test(draft.user)
    && Number.isSafeInteger(Number(draft.organisation)) && Number(draft.user)<=Number(draft.organisation)
  const save=async(event:FormEvent)=>{
    event.preventDefault()
    if(!canManage || !draft || !budgetValid || busy)return
    setBusy(true);setError('');setMessage('')
    try {
      const value=await client.setExecutionBudget(orgId,{expected_revision:draft.revision,organisation_concurrent_runs:Number(draft.organisation),user_concurrent_runs:Number(draft.user)})
      setDraft({revision:value.revision,organisation:String(value.organisation_concurrent_runs),user:String(value.user_concurrent_runs)})
      setMessage(t('Task limits saved.','Aufgabenlimits gespeichert.'));setReload(n=>n+1)
    }catch(cause){setError(String(cause))}finally{setBusy(false)}
  }
  const recover=async()=>{
    if(!canManage || !pending || !ack || changed || busy)return
    setBusy(true);setError('');setMessage('')
    try {
      const operation=await client.recoverExecutions(pending.instance,pending.ids)
      setMessage(t('Stop verification requested. Operation: ','Stopp-Prüfung angefordert. Vorgang: ')+operation.id)
      setPending(null);setAck(false);setReload(n=>n+1)
    }catch(cause){setError(String(cause))}finally{setBusy(false)}
  }
  return <section className="enterprise-settings-stack" aria-label={t('Task capacity','Aufgabenkapazität')}>
    <h2>{t('Concurrent tasks','Parallele Aufgaben')}</h2>
    {(error || loadError) && <p role="alert" className="enterprise-error">{error || loadError}</p>}
    {message && <p role="status">{message}</p>}
    <button type="button" disabled={busy} onClick={()=>{setDraft(null);setPending(null);setAck(false);setError('');setReload(n=>n+1)}}>{t('Reload capacity','Kapazität aktualisieren')}</button>
    {!capacity ? <p>{t('Loading capacity…','Kapazität wird geladen…')}</p> : <>
      <p>{t('Reserved task slots: ','Belegte Aufgabenplätze: ')}<strong>{capacity.held_runs}</strong></p>
      {draft && <form onSubmit={event=>void save(event)}>
        <label>{t('Organisation task limit','Aufgabenlimit der Organisation')}<input type="number" min="1" step="1" value={draft.organisation} disabled={!canManage || busy} onChange={event=>setDraft({...draft,organisation:event.target.value})}/></label>
        <label>{t('Task limit per member','Aufgabenlimit pro Mitglied')}<input type="number" min="1" step="1" value={draft.user} disabled={!canManage || busy} onChange={event=>setDraft({...draft,user:event.target.value})}/></label>
        {canManage && <button disabled={busy || !budgetValid}>{t('Save task limits','Aufgabenlimits speichern')}</button>}
      </form>}
      <p>{t('Operator ceilings also apply. Offline and unverified executions keep their slots reserved.','Zusätzlich gelten die Grenzen des Betreibers. Offline laufende und ungeklärte Ausführungen behalten ihre belegten Plätze.')}</p>
      {capacity.held_runs>capacity.reservations.length && <p>{t('Showing the oldest 100 reservations. Refresh after settling them to see the next entries.','Angezeigt werden die ältesten 100 Belegungen. Nach deren Abgleich zeigt eine Aktualisierung die nächsten Einträge.')}</p>}
      {!capacity.reservations.length ? <p>{t('No reserved tasks.','Keine belegten Aufgabenplätze.')}</p> : <div className="enterprise-records enterprise-capacity-records" role="region" aria-label={t('Outstanding executions','Offene Ausführungen')} tabIndex={0}><table>
        <caption>{t('Outstanding executions','Offene Ausführungen')}</caption>
        <thead><tr>{[t('Run','Aufgabe'),t('Instance','Instanz'),t('Execution location','Ausführungsort'),t('Reserved since','Belegt seit')].map(label=><th key={label} scope="col">{label}</th>)}</tr></thead>
        <tbody>{capacity.reservations.map(row=><tr key={row.id}><td>{row.run_id}</td><td>{instances.find(i=>i.id===row.instance_id)?.name??row.instance_id}</td><td>{local(row)?t('Server','Server'):t('Device or VM · executor confirmation required','Gerät oder VM · Bestätigung des ausführenden Geräts erforderlich')}</td><td>{new Date(row.created_at).toLocaleString(i18n.language)}</td></tr>)}</tbody>
      </table></div>}
      {canManage && eligible.map(instance=>{
        const ids=capacity.reservations.filter(r=>r.instance_id===instance.id && local(r)).map(r=>r.id)
        return ids.length>0 && <button type="button" key={instance.id} disabled={busy || !!error || !!loadError} onClick={()=>{setPending({instance,ids});setAck(false)}}>{t('Review server slots: ','Server-Belegungen prüfen: ')}{instance.name} ({ids.length})</button>
      })}
      {pending && <section className="enterprise-confirm enterprise-settings-stack" role="dialog" aria-modal="false" aria-label={t('Verify stopped executions','Beendete Ausführungen prüfen')}>
        <h3>{pending.instance.name}</h3>
        <p>{t('This stops the whole instance and all its server tasks. Only the selected server reservations are released after the provisioner verifies the stop. Device and VM reservations remain unchanged.','Dies stoppt die gesamte Instanz und alle ihre Server-Aufgaben. Nur die ausgewählten Server-Belegungen werden nach geprüftem Stillstand freigegeben. Geräte- und VM-Belegungen bleiben bestehen.')}</p>
        <p>{t('Selected reservations: ','Ausgewählte Belegungen: ')}{pending.ids.length}</p>
        <label className="enterprise-check"><input type="checkbox" checked={ack} disabled={busy} onChange={event=>setAck(event.target.checked)}/>{t('I acknowledge stopping the entire instance.','Ich bestätige den Stopp der gesamten Instanz.')}</label>
        {changed && <p role="alert">{t('The instance or reservations changed. Close this review and select again.','Instanz oder Belegungen haben sich geändert. Schließe die Prüfung und wähle erneut.')}</p>}
        <button type="button" disabled={busy || !ack || changed || !!error || !!loadError} onClick={()=>void recover()}>{t('Stop and verify','Stoppen und prüfen')}</button>
        <button type="button" disabled={busy} onClick={()=>{setPending(null);setAck(false)}}>{t('Close review','Prüfung schließen')}</button>
      </section>}
    </>}
  </section>
}
