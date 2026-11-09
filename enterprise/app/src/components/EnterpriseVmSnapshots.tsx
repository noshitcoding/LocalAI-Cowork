import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { z } from 'zod'
import type { ManagedInstance, ManagementClient } from '../runtime/managementClient'

const snapshot=z.object({id:z.uuid(),state:z.enum(['retained','deleted']),revision:z.number().int().positive(),created_at:z.string()})
const job=z.object({id:z.uuid(),state:z.enum(['queued','running','held','completed']),action:z.string(),plan_sha256:z.string()})
type Original={path:string;body:Record<string,unknown>}

export default function EnterpriseVmSnapshots({client,instance,admin}:{client:ManagementClient;instance:ManagedInstance;admin:boolean}) {
  const {i18n}=useTranslation(),de=i18n.language.startsWith('de'),t=(en:string,german:string)=>de?german:en
  const base=`/organisations/${instance.organisation_id}/instances/${instance.id}`
  const [items,setItems]=useState<z.infer<typeof snapshot>[]>([]),[jobs,setJobs]=useState<z.infer<typeof job>[]>([]),[error,setError]=useState(''),[busy,setBusy]=useState(false),[revision,setRevision]=useState(0),[confirm,setConfirm]=useState(''),[original,setOriginal]=useState<Original|null>(null)
  useEffect(()=>{
    const controller=new AbortController()
    const refresh=()=>Promise.all([client.request(`${base}/vm-snapshots`,z.array(snapshot),undefined,'GET',controller.signal),client.request(`${base}/vm-snapshot-jobs`,z.array(job),undefined,'GET',controller.signal)]).then(([a,b])=>{if(!controller.signal.aborted){setItems(a);setJobs(b)}}).catch(e=>{if(!controller.signal.aborted)setError(String(e instanceof Error?e.message:e))})
    void refresh();const timer=window.setInterval(()=>void refresh(),5000)
    return()=>{controller.abort();window.clearInterval(timer)}
  },[client,base,revision])
  if(instance.backend!=='compose'||instance.policy.minimum_isolation!=='virtual_machine'||!admin)return null
  const act=async(request:Original)=>{
    if(busy)return;setBusy(true);setError('');setOriginal(request)
    try{await client.request(request.path,job,request.body);setOriginal(null);setRevision(v=>v+1)}catch(e){setError(String(e instanceof Error?e.message:e))}finally{setBusy(false)}
  }
  const blocked=busy||instance.state!=='stopped'||jobs.some(j=>j.state!=='completed')
  const body=(action:string)=>({request_id:crypto.randomUUID(),expected_instance_revision:instance.revision,action})
  return <section className="enterprise-settings-stack" aria-label={t('VM disk snapshots','VM-Disk-Snapshots')}>
    <h4>{t('Encrypted VM disk snapshots','Verschlüsselte VM-Disk-Snapshots')}</h4>
    <p>{t('Stop the instance first. A disk snapshot preserves this original VM disk; external PostgreSQL, object storage and parent control data use instance backups. Uncertain operations remain held for operator reconciliation.','Stoppe zuerst die Instanz. Ein Disk-Snapshot bewahrt die Disk dieser ursprünglichen VM; externe PostgreSQL-, Objektspeicher- und Elternkontrolldaten verwenden Instanz-Backups. Unklare Vorgänge bleiben für den Betreiberabgleich gehalten.')}</p>
    {error&&<p role="alert">{error}</p>}
    {original&&<button type="button" disabled={busy} onClick={()=>void act(original)}>{t('Retry original request','Ursprüngliche Anfrage wiederholen')}</button>}
    <button type="button" disabled={blocked} onClick={()=>void act({path:`${base}/vm-snapshots`,body:body('snapshot')})}>{t('Create disk snapshot','Disk-Snapshot erstellen')}</button>
    <label>{t('Instance name to confirm restore or deletion','Instanzname zur Bestätigung von Wiederherstellung oder Löschung')}<input value={confirm} onChange={e=>setConfirm(e.target.value)}/></label>
    <ul className="enterprise-snapshot-list">{items.filter(s=>s.state==='retained').map(s=><li key={s.id}><div><code>{s.id}</code> · {s.created_at}</div><div className="enterprise-actions">{(['restore_snapshot','delete_snapshot'] as const).map(action=><button key={action} type="button" disabled={blocked||confirm!==instance.name} onClick={()=>void act({path:`${base}/vm-snapshots`,body:{...body(action),snapshot_id:s.id,expected_snapshot_revision:s.revision,confirmation_name:confirm}})}>{action==='restore_snapshot'?t('Restore original disk','Ursprüngliche Disk wiederherstellen'):t('Delete encrypted snapshot','Verschlüsselten Snapshot löschen')}</button>)}</div></li>)}</ul>
    <ul className="enterprise-snapshot-list">{jobs.map(j=><li key={j.id}>{j.action} · {j.state} · <code>{j.id}</code></li>)}</ul>
  </section>
}
