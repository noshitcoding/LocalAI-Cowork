import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { z } from 'zod'
import { ManagementClient, type ManagedInstance } from '../runtime/managementClient'

const record=z.record(z.string(),z.unknown())
const snapshot=z.object({profile:record.nullable(),reservations:z.array(record),qualified:z.boolean(),legacy_unqualified:z.boolean()})
const original=z.object({operation:record.nullable()})
const uuid=z.uuid()
type Pending={request:string;requestSha:string;specSha:string;purpose:'activate'|'disable'|'release';instanceRevision:number;profileRevision:number}
function ordered(value:unknown):unknown {
  if(Array.isArray(value))return value.map(ordered)
  if(value!==null&&typeof value==='object')return Object.fromEntries(Object.entries(value).sort(([a],[b])=>a.localeCompare(b,'en',{usage:'sort'})).map(([k,v])=>[k,ordered(v)]))
  if(typeof value==='number'&&(!Number.isFinite(value)||!Number.isSafeInteger(value)))throw new Error('Specification integers must be exact')
  return value
}
async function hash(text:string){const bytes=new TextEncoder().encode(text);return Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',bytes.buffer)),v=>v.toString(16).padStart(2,'0')).join('')}

export default function EnterpriseDiskQuotas({client,instance,admin,serverUrl,userId,onClose}:{client:ManagementClient;instance:ManagedInstance;admin:boolean;serverUrl:string;userId:string;onClose:()=>void}) {
  const {i18n}=useTranslation();const t=(en:string,de:string)=>i18n.language.startsWith('de')?de:en
  const base=`/organisations/${instance.organisation_id}/instances/${instance.id}/disk-quota`
  const key=`cowork.disk-plan:${encodeURIComponent(serverUrl)}:${userId}:${instance.organisation_id}:${instance.id}`
  const [view,setView]=useState<z.infer<typeof snapshot>|null>(null)
  const [spec,setSpec]=useState<Record<string,unknown>|null>(null)
  const [pending,setPending]=useState<Pending|null>(null)
  const [operation,setOperation]=useState<Record<string,unknown>|null>(null)
  const [ack,setAck]=useState(false);const [busy,setBusy]=useState(false);const [error,setError]=useState('')
  const generation=useRef(0);const active=useRef(true);const sending=useRef(false)
  useEffect(()=>{active.current=true;generation.current++;setView(null);setSpec(null);setOperation(null);setAck(false);setError('')
    try{const saved=localStorage.getItem(key);if(saved){const p=JSON.parse(saved) as Pending;uuid.parse(p.request);if(!/^[a-f0-9]{64}$/.test(p.requestSha)||!/^[a-f0-9]{64}$/.test(p.specSha))throw Error('invalid saved original');setPending(p)}else setPending(null)}catch{setError(t('Saved original identifier is invalid.','Der gespeicherte Originalbezug ist ungültig.'))}
    const controller=new AbortController();const captured=generation.current
    void client.request(base,snapshot,undefined,'GET',controller.signal).then(v=>{if(active.current&&generation.current===captured)setView(v)}).catch(e=>{if(!controller.signal.aborted&&generation.current===captured)setError(String(e))})
    return()=>{active.current=false;generation.current++;controller.abort()}
  },[client,base,key])
  const refresh=async()=>{const captured=generation.current;const v=await client.request(base,snapshot);if(active.current&&generation.current===captured)setView(v)}
  const load=async(p:Pending)=>{
    const captured=generation.current;const result=await client.request(`${base}/operations/${p.request}`,original)
    if(!active.current||generation.current!==captured)return null
    if(result.operation&&result.operation.request_sha256!==p.requestSha)throw Error(t('Original request hash conflict.','Konflikt mit dem Hash der Originalanfrage.'))
    setOperation(result.operation);return result.operation
  }
  const body=(p:Pending,s:Record<string,unknown>)=>({request_id:p.request,expected_instance_revision:p.instanceRevision,expected_profile_revision:p.profileRevision,
    purpose:p.purpose,specification:ordered(s),confirmation:'RESERVE_ORIGINAL_TOTAL_PHYSICAL_DISK'})
  const run=async(work:()=>Promise<void>)=>{if(sending.current)return;sending.current=true;setBusy(true);setError('');try{
    if(navigator.locks)await navigator.locks.request(key,async()=>{await work()});else await work()
  }catch(e){if(active.current)setError(String(e))}finally{sending.current=false;if(active.current)setBusy(false)}}
  const prepare=(purpose:Pending['purpose'])=>void run(async()=>{
    if(!admin||!ack||!spec||!uuid.safeParse(userId).success)return
    const profileRevision=typeof view?.profile?.revision==='number'?view.profile.revision:0
    if(!Number.isSafeInteger(profileRevision)||!Number.isSafeInteger(instance.revision))throw Error('Revision exceeds exact browser representation')
    const p:Pending=pending??{request:crypto.randomUUID(),requestSha:'',specSha:await hash(JSON.stringify(ordered(spec))),purpose,
      instanceRevision:instance.revision,profileRevision}
    if(p.purpose!==purpose||p.specSha!==await hash(JSON.stringify(ordered(spec))))throw Error(t('Reopen the unchanged original specification.','Bitte die unveränderte Originalspezifikation erneut öffnen.'))
    const request=body(p,spec);const digest=await hash(JSON.stringify(request))
    if(p.requestSha&&p.requestSha!==digest)throw Error(t('Original request changed; no resend.','Originalanfrage verändert; kein erneutes Senden.'))
    p.requestSha=digest;localStorage.setItem(key,JSON.stringify(p));setPending(p)
    const beforeRead=generation.current;const known=await load(p)
    if(!active.current||generation.current!==beforeRead)return
    if(known)return
    const captured=generation.current;const next=await client.request(base,record,request)
    if(next.request_sha256!==p.requestSha)throw Error('Server original request digest differs')
    if(active.current&&generation.current===captured)setOperation(next);await refresh()
  })
  const commit=()=>void run(async()=>{
    if(!admin||!pending||!ack)return
    const beforeRead=generation.current;const known=await load(pending);if(!active.current||generation.current!==beforeRead||!known||known.state!=='observed'||typeof known.id!=='string')return
    const receipt=await client.request(`${base}/commit/${uuid.parse(known.id)}`,record,{})
    if(receipt.request_id!==pending.request||receipt.instance_id!==instance.id||receipt.organisation_id!==instance.organisation_id)throw Error('Original commit receipt scope differs')
    await load(pending);await refresh()
  })
  return <section aria-label={t('Total physical disk quota','Gesamte physische Diskquote')}>
    <h3>{t('Total physical disk quota','Gesamte physische Diskquote')}: {instance.name}</h3>
    <button type="button" disabled={busy} onClick={onClose}>{t('Close','Schließen')}</button>
    {error&&<p role="alert">{error}</p>}
    <p>{t('Fully allocated ext4 filesystems cover database/WAL, object data, Docker workspaces/layers, retained generations, scratch, backups and the entire configured snapshot maximum. A PVC request or shared bucket is insufficient.',
      'Vollständig allokierte ext4-Dateisysteme umfassen Datenbank/WAL, Objekte, Docker-Workspaces und Images, aufbewahrte Generationen, temporäre Daten, Backups und die gesamte konfigurierte Snapshotreserve. Eine PVC-Anfrage oder ein geteilter Bucket reicht dafür nicht.')}</p>
    {view&&<><p role="status">{view.legacy_unqualified?t('Existing placement is unqualified; no automatic adoption or data move.','Bestehende Platzierung ist unqualifiziert; keine automatische Übernahme oder Datenverschiebung.'):
      t('Profile: ','Profil: ')+String(view.profile?.state)} · {t('Policy maximum: ','Policy-Maximum: ')}{instance.policy.limits.storage_gib} GiB</p>
      <ul>{view.reservations.map(r=><li key={String(r.placement_id)}>{String(r.kind)} · {String(r.size_bytes)} {t('reserved bytes','reservierte Bytes')} · {String(r.state)} · <code>{String(r.filesystem_uuid)}</code></li>)}</ul></>}
    <p>{t('Prepare reserves before physical IO. The operator runs the delivered installer and captures the original measured proof. Commit checks the current administrator, login, policy and instance revisions again.',
      'Vorbereiten reserviert vor dem physischen IO. Der Betreiber führt den mitgelieferten Installer aus und erfasst den gemessenen Originalbeleg. Übernehmen prüft Administrator, Anmeldung, Policy und Instanzrevision erneut.')}</p>
    <label>{t('Public original placement specification (UTF-8 JSON, at most 1 MiB; no keys)','Öffentliche Originalplatzierung (UTF-8-JSON, höchstens 1 MiB; keine Schlüssel)')}
      <input type="file" accept="application/json,.json" disabled={!admin||busy} onChange={e=>void run(async()=>{const f=e.target.files?.[0];if(!f)return;if(f.size>1048576)throw Error('Specification exceeds 1 MiB')
        const value=record.parse(JSON.parse(new TextDecoder('utf-8',{fatal:true}).decode(await f.arrayBuffer())));ordered(value)
        if(value.instance_id!==instance.id||value.organisation_id!==instance.organisation_id)throw Error('Specification belongs to another target');setSpec(value);setAck(false)})}/></label>
    <label><input type="checkbox" checked={ack} disabled={!admin||busy} onChange={e=>setAck(e.target.checked)}/>{t('I reviewed this exact original placement and total reservation. Disable keeps reservations; release needs an independent positive physical teardown receipt.',
      'Ich habe diese genaue Originalplatzierung und Gesamtreservation geprüft. Deaktivieren behält die Reservation; Freigeben benötigt einen positiven physischen Abbaubeleg.')}</label>
    {(['activate','disable','release'] as const).map(p=><button key={p} type="button" disabled={!admin||busy||!ack||!spec||!!pending&&pending.purpose!==p} onClick={()=>prepare(p)}>
      {p==='activate'?t('Prepare qualification','Qualifikation vorbereiten'):p==='disable'?t('Prepare disable','Deaktivierung vorbereiten'):t('Prepare physical release','Physische Freigabe vorbereiten')}</button>)}
    {pending&&<><p>{t('Original request: ','Originalanfrage: ')}<code>{pending.request}</code></p><p><code>{pending.requestSha}</code></p>
      <button disabled={busy} type="button" onClick={()=>void run(async()=>{await load(pending);await refresh()})}>{t('Recover original response','Originalantwort wiederherstellen')}</button>
      <button disabled={!admin||busy||!ack||operation?.state!=='observed'} type="button" onClick={commit}>{t('Commit original observation','Originalbeobachtung übernehmen')}</button>
      {operation&&<p>{t('Original state: ','Originalzustand: ')}{String(operation.state)} · {t('Plan: ','Plan: ')}<code>{String(operation.id)}</code></p>}
      {operation?.state==='committed'&&<button type="button" disabled={busy} onClick={()=>{localStorage.removeItem(key);setPending(null);setOperation(null);setSpec(null);setAck(false)}}>{t('Start a new explicit operation','Neuen ausdrücklichen Vorgang beginnen')}</button>}</>}
  </section>
}
