import {useEffect,useRef,useState} from 'react'
import {useTranslation} from 'react-i18next'
import {z} from 'zod'
import type {RemoteRuntimeClient} from '../runtime/runtimeClient'
import {prepareSnapshot,sendSnapshot,snapshotPath,snapshotSensitivePath} from '../runtime/snapshotUpload'
import {useRemoteRuntimeStore} from '../stores/remoteRuntimeStore'

const journalSchema=z.object({key:z.uuid(),signature:z.string().regex(/^[0-9a-f]{64}$/)})
export default function RemoteSnapshotUpload({client,projectId,onReady}:{client:RemoteRuntimeClient;projectId:string;onReady:(id:string|null)=>void}){
  const {i18n}=useTranslation(),tr=(en:string,de:string)=>i18n.language.startsWith('de')?de:en
  const user=useRemoteRuntimeStore(s=>s.userId),server=useRemoteRuntimeStore(s=>s.serverUrl)
  const [files,setFiles]=useState<File[]>([]),[selected,setSelected]=useState<Set<number>>(new Set()),[empty,setEmpty]=useState(false),[consent,setConsent]=useState(false)
  const [busy,setBusy]=useState(false),[progress,setProgress]=useState(''),[error,setError]=useState(''),[ready,setReady]=useState(false)
  const controller=useRef<AbortController|null>(null),mounted=useRef(true)
  useEffect(()=>{mounted.current=true;return()=>{mounted.current=false;controller.current?.abort()}},[])
  useEffect(()=>{setReady(false);onReady(null)},[files,selected,empty,consent,onReady])
  const chosen=files.filter((_file,index)=>selected.has(index)),bytes=chosen.reduce((sum,file)=>sum+file.size,0)
  function choose(next:File[]){
    try{for(const file of next)snapshotPath(file);setFiles(next);setSelected(new Set(next.flatMap((file,index)=>snapshotSensitivePath(snapshotPath(file))?[]:[index])));setEmpty(false);setConsent(false);setError('')}
    catch(reason){setError(String(reason))}
  }
  async function upload(newIntent=false){
    if(busy||!consent||(!empty&&chosen.length===0))return
    if(!user||server!==client.enterpriseSource){setError(tr('Open a verified account on this server before uploading.','Melde dich vor dem Upload mit einem bestätigten Konto an diesem Server an.'));return}
    setBusy(true);setReady(false);onReady(null);setError('');const abort=new AbortController();controller.current=abort
    try{
      const prepared=await prepareSnapshot(projectId,empty?[]:chosen,abort.signal,text=>{if(mounted.current)setProgress(text)})
      const storageKey='cowork-snapshot-upload:1:'+JSON.stringify([client.enterpriseSource,user,projectId])
      const raw=sessionStorage.getItem(storageKey),original=raw?journalSchema.parse(JSON.parse(raw)):null
      const intent=!newIntent&&original?.signature===prepared.signature?original:{key:crypto.randomUUID(),signature:prepared.signature}
      // Only a fingerprint and the original request ID are retained, never file bytes or tokens.
      sessionStorage.setItem(storageKey,JSON.stringify(intent))
      const manifest=await sendSnapshot(client,prepared,intent.key,abort.signal,text=>{if(mounted.current)setProgress(text)})
      if(mounted.current){setReady(true);setProgress(tr('Snapshot verified. The run can now start.','Snapshot geprüft. Der Lauf kann jetzt starten.'));onReady(manifest.id)}
    }catch(reason){if(mounted.current)setError(reason instanceof Error?reason.message:String(reason))}
    finally{if(mounted.current)setBusy(false);if(controller.current===abort)controller.current=null}
  }
  return <section className="remote-snapshot-upload" aria-label={tr('Explicit file snapshot','Dateien ausdrücklich freigeben')}>
    <h3>{tr('Private files for this run','Private Dateien für diesen Lauf')}</h3>
    <p>{tr('Files stay local until you select and explicitly upload a copy. Your project remains private. The selected execution target receives this immutable snapshot.','Dateien bleiben lokal, bis du eine Kopie auswählst und ausdrücklich hochlädst. Dein Projekt bleibt privat. Das gewählte Ausführungsziel erhält diesen unveränderlichen Snapshot.')}</p>
    <label>{tr('Select files','Dateien auswählen')}<input type="file" multiple disabled={busy} onChange={event=>choose(Array.from(event.currentTarget.files??[]))}/></label>
    <label>{tr('Select a folder','Ordner auswählen')}<input type="file" multiple {...{webkitdirectory:''}} disabled={busy} onChange={event=>choose(Array.from(event.currentTarget.files??[]))}/></label>
    {!!files.length&&<fieldset disabled={busy||empty}><legend>{tr('Files to transfer','Zu übertragende Dateien')}</legend><ul>{files.map((file,index)=><li key={index}><label><input type="checkbox" checked={selected.has(index)} onChange={event=>{const next=new Set(selected);if(event.target.checked)next.add(index);else next.delete(index);setSelected(next);setConsent(false)}}/>{snapshotPath(file)} · {file.size} B{snapshotSensitivePath(snapshotPath(file))?' · '+tr('Potential credentials or generated dependencies; excluded by default','Mögliche Zugangsdaten oder generierte Abhängigkeiten; zunächst ausgeschlossen'):''}</label></li>)}</ul></fieldset>}
    <label><input type="checkbox" checked={empty} disabled={busy} onChange={event=>{setEmpty(event.target.checked);setConsent(false)}}/>{tr('Run without project files (empty snapshot)','Ohne Projektdateien ausführen (leerer Snapshot)')}</label>
    <p>{empty?0:chosen.length} {tr('files','Dateien')} · {empty?0:bytes} B · {client.enterpriseSource}</p>
    <label><input type="checkbox" checked={consent} disabled={busy} onChange={event=>setConsent(event.target.checked)}/>{tr('I approve transferring this snapshot to the displayed server and execution target.','Ich gebe die Übertragung dieses Snapshots an den angezeigten Server und das Ausführungsziel frei.')}</label>
    <button type="button" disabled={busy||!consent||(!empty&&chosen.length===0)} onClick={()=>void upload()}>{ready?tr('Verify original snapshot again','Originalen Snapshot erneut prüfen'):tr('Upload approved snapshot','Freigegebenen Snapshot hochladen')}</button>
    {busy&&<button type="button" onClick={()=>controller.current?.abort()}>{tr('Stop upload','Upload anhalten')}</button>}
    {!busy&&error.includes('expired')&&<button type="button" disabled={!consent||(!empty&&chosen.length===0)} onClick={()=>void upload(true)}>{tr('Begin a new upload after expiry','Nach Ablauf eine neue Übertragung beginnen')}</button>}
    {progress&&<p role="status">{progress}</p>}{error&&<p role="alert">{error} {tr('Select the same files and retry to reconcile the original upload.','Wähle dieselben Dateien und versuche es erneut, um den ursprünglichen Upload abzugleichen.')}</p>}
  </section>
}
