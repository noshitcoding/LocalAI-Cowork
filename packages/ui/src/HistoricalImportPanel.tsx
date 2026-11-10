import {useEffect,useRef,useState} from "react";
import {historicalCatalog,historicalPreview,historicalPreviewFormat,historicalProjectBody,historicalStreams,type ActivationReceipt,type BeginInput,type CatalogEntry,type CatalogSlice,type FileOperation,type HistoricalImportApi,type HistoricalPins,type ImportLimit,type ImportPreview,type ImportRecord,type PreviewInput,type ProjectTarget,type UnsentFileReceipt} from "./historicalImport";
import {useView} from "./view";
import {HistoricalHistoryBrowser} from "./HistoricalHistoryBrowser";

type TargetOption={id:string,name:string,revision:number,profile_revision?:number};
type Journal={native_request_id?:string,begin?:BeginInput,import_id?:string,preview_input?:PreviewInput,preview_id?:string,plan_sha256?:string};
type OriginalPlanRef=Pick<ImportPreview,"id"|"import_id"|"plan_sha256">;
const bytes=(n:number)=>`${(n/1024/1024).toLocaleString(undefined,{maximumFractionDigits:2})} MiB`;
function errorCode(error:unknown):string {
 if(error instanceof Error)return error.message;
 if(error&&typeof error==="object"&&"code" in error)return String(error.code);
 return String(error);
}
function notFound(error:unknown){return /^not_found(?: \(404\))?$/.test(errorCode(error));}
export function HistoricalImportPanel({api,targets,onActivated}:{api:HistoricalImportApi,targets:TargetOption[],onActivated?:()=>Promise<void>}){
 const language=useView(v=>v.language),t=(de:string,en:string)=>language==="de"?de:en;
 const key="cowork.m9-product-import.v1:"+api.scope,epoch=useRef(0);
 const [limit,setLimit]=useState<ImportLimit>(),[record,setRecord]=useState<ImportRecord>(),[catalog,setCatalog]=useState<CatalogEntry[]>([]),[mapping,setMapping]=useState<Record<string,ProjectTarget>>({}),[historyFiles,setHistoryFiles]=useState<string[]>([]),[preview,setPreview]=useState<ImportPreview>(),[receipt,setReceipt]=useState<ActivationReceipt>(),[journal,setJournal]=useState<Journal>({}),[files,setFiles]=useState<Partial<Record<string,File>>>({}),[pins,setPins]=useState(""),[busy,setBusy]=useState(false),[error,setError]=useState(""),[progress,setProgress]=useState(""),[body,setBody]=useState<string>(),[confirmed,setConfirmed]=useState(false),[limitGiB,setLimitGiB]=useState("16"),[operations,setOperations]=useState<Record<string,FileOperation>>({}),[closures,setClosures]=useState<Record<string,UnsentFileReceipt>>({});
 const [catalogSlice,setCatalogSlice]=useState<CatalogSlice>(),[remainder,setRemainder]=useState(false);
 const persist=(value:Journal)=>{localStorage.setItem(key,JSON.stringify(value));setJournal(value);};
 const live=(generation:number)=>{if(epoch.current!==generation)throw new Error("historical_context_changed");};
 const stateLabel=(state:string)=>{const labels:Record<string,string>={receiving:t("Übertragung läuft","Receiving"),verified:t("Geprüft","Verified"),preview:t("Vorschau","Preview"),active:t("Aktiviert","Activated"),partially_active:t("Inhaltsteile aktiviert; weitere Originalteile erhalten","Content parts activated; other original parts retained"),prepared:t("Vorbereitet","Prepared"),uncertain:t("Ursprünglicher Effekt ungewiss","Original effect uncertain"),received:t("Dauerhaft bestätigt","Durably confirmed"),unsent_closed:t("Nachweislich ungesendet abgeschlossen","Proved unsent and closed")};return labels[state]??state;};
 async function work(action:(generation:number)=>Promise<void>){if(busy)return;const generation=epoch.current;setBusy(true);setError("");try{await action(generation);}catch(e){if(epoch.current===generation)setError(errorCode(e));}finally{if(epoch.current===generation)setBusy(false);}}
 async function fileStatus(g:number,plan:OriginalPlanRef,source:string){
  let op:FileOperation;
  try{op=await api.json<FileOperation>(`/previews/${plan.id}/files/${encodeURIComponent(source)}/stage`);}catch(e){if(!notFound(e))throw e;live(g);return undefined;}
  live(g);if(op.preview_id!==plan.id||op.import_id!==plan.import_id||op.source_id!==source||op.plan_sha256!==plan.plan_sha256||!/^[0-9a-f]{64}$/.test(op.request_sha256))throw new Error("historical_response_invalid");
  setOperations(current=>({...current,[source]:op}));return op;
 }
 async function load(g:number,original:Journal){
  const current=await api.json<ImportLimit>("/limit");live(g);setLimit(current);
  if(original.import_id){const value=await api.json<ImportRecord>(`/imports/${original.import_id}`);live(g);setRecord(value);if(value.state!=="receiving")await catalogue(g,value,0);}
  if(original.preview_id){const value=await historicalPreview(api,original.preview_id);live(g);setPreview(value);if(value.state==="active"){const result=await api.json<ActivationReceipt>(`/previews/${value.id}/receipt`);live(g);setReceipt(result);}else{for(const p of value.projects.filter(p=>p.target.kind!=="history_only"))for(const f of p.source.files.filter(f=>!value.history_only_file_ids.includes(f.source_id)))await fileStatus(g,value,f.source_id);}}
 }
 async function catalogue(g:number,value:ImportRecord,offset:number){const page=await historicalCatalog(api,value.id,offset,5);live(g);if(page.manifest.source_sha256!==value.pins.quarantine_database||record?.id===value.id&&catalogSlice&&catalogSlice.manifest.sha256!==page.manifest.sha256)throw new Error("historical_content_integrity");const closed=page.editable_selection_closed||value.state==="active",entries=page.entries.map(entry=>({...entry,editable_selection_closed:closed})),blocked=new Set(entries.filter(entry=>entry.activation_receipt_sha256||entry.editable_selection_closed).map(entry=>entry.source.source_id)),blockedFiles=new Set(entries.filter(entry=>blocked.has(entry.source.source_id)).flatMap(entry=>entry.source.files.map(file=>file.source_id)));setMapping(current=>closed?{}:Object.fromEntries(Object.entries(current).filter(([source])=>!blocked.has(source))));setHistoryFiles(current=>closed?[]:current.filter(source=>!blockedFiles.has(source)));setCatalogSlice({...page,entries,editable_selection_closed:closed});setCatalog(entries);}
 useEffect(()=>{
  epoch.current++;const g=epoch.current;setRecord(undefined);setCatalog([]);setPreview(undefined);setReceipt(undefined);setBody(undefined);setError("");setProgress("");setBusy(false);setConfirmed(false);setOperations({});setClosures({});setMapping({});setHistoryFiles([]);setFiles({});setPins("");
  setCatalogSlice(undefined);setRemainder(false);
  let saved:Journal={};try{const raw=localStorage.getItem(key);if(raw)saved=JSON.parse(raw) as Journal;}catch{}setJournal(saved);
  void load(g,saved).catch(e=>{if(epoch.current===g)setError(errorCode(e));});return()=>{epoch.current++;};
 },[api.scope]);
 async function upload(g:number){
  if(api.native&&api.selectBundle){const requestId=journal.native_request_id??crypto.randomUUID();persist({...journal,native_request_id:requestId});const value=await api.selectBundle(requestId);live(g);if(!value)return;const retained={native_request_id:requestId,import_id:value.id};persist(retained);setRecord(value);await load(g,retained);return;}
  let original=journal.begin;
  if(!original){
   if(!limit)throw new Error("import_limit_required");const selected=JSON.parse(pins) as HistoricalPins;
   for(const stream of [...historicalStreams,"export_manifest"]){if(!/^[0-9a-f]{64}$/.test(selected[stream]??""))throw new Error("original_pins_required");}
   const previewFile=files.preview;if(!previewFile)throw new Error("select_all_five_original_files");
   const preview_format=await historicalPreviewFormat(previewFile,selected.preview);live(g);
   const maximums:Record<string,number>={quarantine_manifest:2*1024*1024,quarantine_database:12*1024*1024*1024,mapping:(preview_format.endsWith("/2")?256:2)*1024*1024,preview:256*1024*1024,cold_target_snapshot:512*1024*1024};
   original={request_id:crypto.randomUUID(),expected_limit_revision:limit.revision,pins:selected,preview_format,files:historicalStreams.map(stream=>{const file=files[stream];if(!file)throw new Error("select_all_five_original_files");if(file.size===0||file.size>maximums[stream])throw new Error("historical_original_file_limit");return{stream,size:file.size,sha256:selected[stream]};})};persist({begin:original});
  }
  const value=await api.json<ImportRecord>("/begin","POST",original);live(g);const retained={...journal,begin:original,import_id:value.id};persist(retained);setRecord(value);
  if(value.state!=="receiving"){await load(g,retained);return;}
  for(const declaration of original.files){const file=files[declaration.stream];if(!file||file.size!==declaration.size)throw new Error("reselect_same_original_files");for(let index=0;index<Math.ceil(file.size/1048576);index++){live(g);const chunk=file.slice(index*1048576,(index+1)*1048576);const received=await api.upload<{import_id:string,stream:string,chunk_index:number,size:number}>(`/imports/${value.id}/chunks/${declaration.stream}/${index}`,chunk);live(g);if(received.import_id!==value.id||received.stream!==declaration.stream||received.chunk_index!==index||received.size!==chunk.size)throw new Error("original_chunk_ack_invalid");setProgress(`${declaration.stream}: ${bytes(Math.min(file.size,(index+1)*1048576))} / ${bytes(file.size)}`);}}
  const verified=await api.json<ImportRecord>(`/imports/${value.id}/verify`,"POST");live(g);setRecord(verified);await load(g,retained);setProgress("");
 }
 async function review(g:number){
  if(!record||!limit)throw new Error("verified_import_required");
  if(!catalogSlice)throw new Error("verified_import_required");
  if(!journal.preview_input&&catalogSlice.editable_selection_closed)throw new Error("historical_editable_selection_closed");
  const chosen=Object.entries(mapping).map(([source_project_id,target])=>({source_project_id,target}));
  if(!journal.preview_input&&(chosen.length===0||chosen.length<catalogSlice.manifest.count&&!remainder))throw new Error("explicit_project_mapping_required");
  const original:PreviewInput=journal.preview_input??{request_id:crypto.randomUUID(),expected_import_revision:record.revision,expected_limit_revision:limit.revision,projects:chosen,history_only_file_ids:historyFiles,catalog_count:catalogSlice.manifest.count,catalog_sha256:catalogSlice.manifest.sha256,...(remainder?{remainder_history_only_reason:t("Alle nicht ausgewählten Originalteile ausdrücklich vorerst als unveränderte Historie behalten; spätere Aktivierung erfordert einen getrennten Plan mit frischen Rechten.","Explicitly retain every unselected original part as unchanged history for now; later activation requires a separate plan with fresh permissions.")}: {})};
  persist({...journal,preview_input:original});const value=await historicalPreview(api,"",{import_id:record.id,input:original});live(g);persist({...journal,preview_input:original,preview_id:value.id,plan_sha256:value.plan_sha256});setPreview(value);setConfirmed(false);setOperations({});setClosures({});
 }
 async function activate(g:number){
  if(!preview||!confirmed)throw new Error("explicit_activation_required");
  const original=await api.json<ActivationReceipt>(`/previews/${preview.id}/activate`,"POST",{plan_sha256:preview.plan_sha256,expected_limit_revision:preview.limit.revision,explicit_activation:true});live(g);
  if(original.preview_id!==preview.id||original.plan_sha256!==preview.plan_sha256)throw new Error("original_activation_receipt_invalid");setReceipt(original);await onActivated?.();
 }
 const target=(entry:CatalogEntry,value:string)=>{
  if(entry.activation_receipt_sha256||entry.editable_selection_closed)return;
  if(value===""){const current={...mapping};delete current[entry.source.source_id];setMapping(current);return;}
  if(value==="history"){setMapping({...mapping,[entry.source.source_id]:{kind:"history_only",reason:entry.unsupported_reason??t("Ausdrücklich unverändert als Historie behalten","Explicitly retain unchanged as history")}});return;}
  if(value==="new"){setMapping({...mapping,[entry.source.source_id]:{kind:"new_personal"}});return;}
  const found=targets.find(t=>t.id===value);if(!found)return;
  setMapping({...mapping,[entry.source.source_id]:api.native?{kind:"existing_personal",project_id:found.id,expected_revision:found.revision}:{kind:"existing_organization",project_id:found.id,expected_revision:found.revision,expected_profile_revision:found.profile_revision??0}});
 };
 async function stage(g:number,plan:ImportPreview,source:string,selected?:File){
  const old=await fileStatus(g,plan,source);
  if(old?.state==="received"){setProgress(t("Die ursprüngliche Dateiübertragung ist bereits dauerhaft bestätigt.","The original file transfer is already durably confirmed."));return;}
  if(old?.state==="unsent_closed")throw new Error("historical_import_unsent_closed");
  const result=api.stageNativeFile?await api.stageNativeFile(plan.id,source):selected?await api.upload<FileOperation>(`/previews/${plan.id}/files/${encodeURIComponent(source)}/stage`,selected):null;
  live(g);if(!result)return;
  if(result.preview_id!==plan.id||result.source_id!==source||result.plan_sha256!==plan.plan_sha256||result.state!=="received")throw new Error("historical_response_invalid");
  setOperations(current=>({...current,[source]:result}));setProgress(t("Originalübertragung abgeglichen.","Original transfer reconciled."));
 }
 async function closeUnsent(g:number,plan:OriginalPlanRef,source:string){
  const op=await fileStatus(g,plan,source);if(!op)throw new Error("original_file_operation_required");
  if(!api.native&&!window.confirm(t("Diesen ursprünglichen Dateivorgang ausdrücklich als ungesendet abschließen? Der Server verlangt den dauerhaften Nachweis, dass kein Versand zugelassen wurde. Nur nie geschriebene Dateibytes werden freigegeben; Originaldaten und Belege bleiben erhalten.","Explicitly close this original file operation as unsent? The server requires durable proof that dispatch was never admitted. Only never-written file bytes are released; original data and receipts are retained.")))return;
  const value=await api.json<UnsentFileReceipt>(`/previews/${plan.id}/files/${encodeURIComponent(source)}/close-unsent`,"POST",{expected_request_sha256:op.request_sha256});live(g);
  if(value.format!=="historical-original-unsent-close/1"||value.original_operation.request_sha256!==op.request_sha256||value.original_operation.preview_id!==plan.id||value.original_operation.source_id!==source||value.released_bytes!==op.size||!/^[0-9a-f]{64}$/.test(value.closure_sha256))throw new Error("historical_response_invalid");
  setClosures(current=>({...current,[source]:value}));await fileStatus(g,plan,source);const policy=await api.json<ImportLimit>("/limit");live(g);setLimit(policy);setConfirmed(false);
 }
 async function newPlan(g:number){
  if(!record)return;
  if(!window.confirm(t("Eine getrennte Vorschau mit frischen Zuordnungen erstellen? Der bisherige Plan und seine Dateioperationen bleiben erhalten; ungewisse Effekte werden dadurch nicht abgeschlossen.","Create a separate preview with fresh mappings? The previous plan and its file operations are retained; this does not close uncertain effects.")))return;
  const value=await api.json<ImportRecord>(`/imports/${record.id}`),policy=await api.json<ImportLimit>("/limit");live(g);
  const retained={native_request_id:journal.native_request_id,begin:journal.begin,import_id:record.id};persist(retained);setRecord(value);setLimit(policy);setPreview(undefined);setReceipt(undefined);setConfirmed(false);setOperations({});setClosures({});setMapping({});setHistoryFiles([]);setProgress("");await onActivated?.();
  live(g);setRemainder(false);await catalogue(g,value,0);
 }
 const errors:Record<string,string>={
  import_limit_required:t("Zuerst ein aktuelles Importlimit konfigurieren.","Configure a current import limit first."),
  original_pins_required:t("Alle sechs geprüften ursprünglichen SHA-256-Pins angeben.","Provide all six verified original SHA-256 pins."),
  select_all_five_original_files:t("Alle fünf ursprünglichen Dateien auswählen.","Select all five original files."),
  reselect_same_original_files:t("Die gleichen ursprünglichen Dateien erneut auswählen.","Select the same original files again."),
  explicit_project_mapping_required:t("Für jedes ursprüngliche Projekt ein Ziel oder Historie ausdrücklich auswählen.","Explicitly select a target or retained history for every original project."),
  explicit_import_limit_required:t("Ein Speicherlimit zwischen 1 und 256 GiB eingeben.","Enter a storage limit between 1 and 256 GiB."),
  historical_import_unsent_closed:t("Dieser Dateivorgang wurde nachweislich ungesendet abgeschlossen. Für eine Übernahme eine getrennte neue Vorschau erstellen.","This file operation was proved unsent and closed. Create a separate new preview to import it."),
  historical_preview_format_invalid:t("Das gepinnte ursprüngliche Previewformat ist unbekannt oder ungültig.","The pinned original preview format is unknown or invalid."),
  historical_original_file_limit:t("Eine Originaldatei überschreitet die Grenze ihres tatsächlichen Previewprofils (/1: Mapping 2 MiB; /2: Mapping 256 MiB).","An original file exceeds its actual preview profile limit (/1: mapping 2 MiB; /2: mapping 256 MiB)."),
  historical_file_too_large:t("Aktuelle Teamdateien dürfen höchstens 32 MiB groß sein.","Current team files may be at most 32 MiB."),
  historical_context_changed:t("Der aktuelle Importzugang hat sich geändert.","The current import context changed."),
  historical_editable_selection_closed:t("Dieser Import ist abgeschlossen. Die vollständige Originalhistorie bleibt lesbar; weitere bearbeitbare Zuordnungen sind gesperrt.","This import is complete. The complete original history remains readable; further editable mappings are closed."),
  uncertain_effect:t("Der ursprüngliche Effekt ist ungewiss. Ein erneuter Versand oder eine Freigabe als ungesendet ist gesperrt.","The original effect is uncertain. Another dispatch or release as unsent is blocked."),
  forbidden:t("Die aktuellen Rechte erlauben diesen Vorgang nicht.","Current permissions do not allow this operation."),
  cancelled:t("Die ausdrückliche Bestätigung wurde abgebrochen.","Explicit confirmation was cancelled."),
 };
 const errorText=errors[error.replace(/ \(\d+\)$/,"")]??t("Der Importvorgang konnte nicht abgeschlossen werden.","The import operation could not be completed.");
 const fileEntries=preview?preview.projects.filter(p=>p.target.kind!=="history_only").flatMap(p=>p.source.files).filter(f=>!preview.history_only_file_ids.includes(f.source_id)):[];
 const recoveryPlan:OriginalPlanRef|undefined=!preview&&journal.preview_id&&journal.import_id&&journal.plan_sha256?{id:journal.preview_id,import_id:journal.import_id,plan_sha256:journal.plan_sha256}:undefined;
 return <section className="product-panel">
  <h3>{t("Originalhistorie übernehmen","Import original history")}</h3>
  <p>{t("Die vollständige Quarantäne bleibt privat erhalten. Schritt 4 erzeugt nach deiner Bestätigung aktuelle, bearbeitbare Inhalte. Historische Konten, Schlüssel, Freigaben, Zeitpläne und Läufe verleihen keine Ausführungsrechte.","The complete quarantine is retained privately. After your confirmation, step 4 creates current editable content. Historical accounts, keys, approvals, schedules and runs grant no execution permissions.")}</p>
  {limit&&<p>{t("Betreiberlimit","Operator limit")}: {bytes(limit.limit_bytes)} · {t("reserviert","reserved")}: {bytes(limit.reserved_bytes)} · {t("verfügbar","available")}: {bytes(Math.max(0,limit.limit_bytes-limit.reserved_bytes))} · {t("Revision","Revision")} {limit.revision}</p>}
  {error&&<p className="error" role="alert">{errorText} <code>{error}</code></p>}{progress&&<p role="status">{progress}</p>}
  {api.native&&<details><summary>{t("Betreiberlimit ausdrücklich konfigurieren","Explicitly configure the operator limit")}</summary>
   <label>{t("Speicherlimit in GiB (1–256)","Storage limit in GiB (1–256)")}<input type="number" min="1" max="256" value={limitGiB} disabled={busy} onChange={e=>setLimitGiB(e.target.value)}/></label>
   <p>{t("Erwartete aktuelle Revision","Expected current revision")}: {limit?.revision??0}. {t("Die native Bestätigung bindet den genauen Betrag; vorhandene Originaldaten werden nicht gelöscht.","Native confirmation binds the exact amount; existing original data is retained.")}</p>
   <button disabled={busy} onClick={()=>void work(async g=>{const gib=Number(limitGiB);if(!Number.isFinite(gib)||gib<1||gib>256)throw new Error("explicit_import_limit_required");await api.json<ImportLimit>("/limit","POST",{expected_revision:limit?.revision??0,limit_bytes:Math.floor(gib*1024*1024*1024)});live(g);await load(g,journal);})}>{t("Limit nativ bestätigen / Originalkonfiguration abgleichen","Confirm the limit natively / reconcile original configuration")}</button>
  </details>}
  {!api.native&&!limit&&<p>{t("Der Betreiber muss zuerst das Installationslimit konfigurieren:","The operator must first configure the installation limit:")} <code>cowork-daemon historical-import-storage 0 &lt;{t("Speicherbytes","storage-bytes")}&gt;</code>. {t("Anschließend denselben Vorgang laden.","Then reload the same operation.")}</p>}
  {!receipt&&<>
   {!api.native&&<><label>{t("Originale SHA-256-Pins aus dem geprüften M9-Paket","Original SHA-256 pins from the verified M9 package")}<textarea disabled={busy||!!journal.begin} value={pins} onChange={e=>setPins(e.target.value)} placeholder='{"quarantine_manifest":"…","quarantine_database":"…","export_manifest":"…","mapping":"…","preview":"…","cold_target_snapshot":"…"}'/></label>{historicalStreams.map(stream=><label key={stream}>{stream}<input type="file" disabled={busy} onChange={e=>setFiles({...files,[stream]:e.target.files?.[0]})}/></label>)}</>}
   {(!record||record.state==="receiving")&&<button disabled={busy||!limit} onClick={()=>void work(upload)}>{api.native?t("Geprüftes M9-Paket nativ auswählen / fortsetzen","Select verified M9 package natively / continue"):journal.begin?t("Dieselben Originaldateien weiter übertragen","Continue transferring the same original files"):t("Quarantäne mit Originalpins aufnehmen","Import quarantine with original pins")}</button>}
   {record&&<p>{t("Originalvorgang","Original operation")}: <code>{record.id}</code> · {stateLabel(record.state)} · {bytes(record.received_bytes)} / {bytes(record.declared_bytes)} · {record.original_rows??"…"} {t("Originalzeilen","original rows")}</p>}
   {catalogSlice&&record&&<><p>{t("Inhaltsteile","Content parts")}: {catalogSlice.offset+1}–{catalogSlice.offset+catalog.length} / {catalogSlice.manifest.count} · {t("ausdrücklich zugeordnet","explicitly mapped")}: {Object.keys(mapping).length}</p><button disabled={busy||catalogSlice.offset===0} onClick={()=>void work(g=>catalogue(g,record,Math.max(0,catalogSlice.offset-5)))}>{t("Vorherige Inhaltsteile","Previous content parts")}</button><button disabled={busy||catalogSlice.next_offset==null} onClick={()=>void work(g=>catalogue(g,record,catalogSlice.next_offset??0))}>{t("Weitere Inhaltsteile","More content parts")}</button></>}
   {catalog.map((entry,index)=><article className="product-item" key={entry.source.source_id}>
    <strong>{entry.source.name}</strong><p>{entry.source.orders.length} {t("Chats/Threads","chats/threads")} · {entry.source.documents.length} {t("Dokumente","documents")} · {entry.source.files.length} {t("Dateihinweise","file references")}</p>
    {entry.activation_receipt_sha256?<p role="status">{t("Bereits aktiviert. Eine erneute bearbeitbare Zuordnung ist gesperrt.","Already activated. Another editable mapping is blocked.")} <code>{entry.activation_receipt_sha256}</code></p>:entry.editable_selection_closed&&<p role="status">{t("Abgeschlossener Import: Dieser Originalteil bleibt als Historie lesbar; neue bearbeitbare Zuordnungen sind gesperrt.","Completed import: this original part remains readable as history; new editable mappings are closed.")}</p>}
    {entry.unsupported_reason&&<p role="status">{t("Dieses aktuelle Inhaltsformat wird nicht unterstützt; die vollständige Originalhistorie bleibt erhalten.","This current content format is unsupported; the complete original history is retained.")} <code>{entry.unsupported_reason}</code></p>}
    <label>{t("Aktuelles Ziel","Current target")}<select disabled={busy||!!journal.preview_input||!!entry.activation_receipt_sha256||entry.editable_selection_closed} value={mapping[entry.source.source_id]?.kind==="history_only"?"history":mapping[entry.source.source_id]?.kind==="new_personal"?"new":"project_id" in (mapping[entry.source.source_id]??{})?(mapping[entry.source.source_id] as {project_id:string}).project_id:""} onChange={e=>target(entry,e.target.value)}>
     <option value="">{t("Ziel ausdrücklich wählen","Explicitly select a target")}</option><option value="history">{t("Unverändert als Historie behalten","Retain unchanged as history")}</option>
     {!entry.unsupported_reason&&<>{api.native&&<option value="new">{t("Neues persönliches Projekt","New personal project")}</option>}{targets.map(v=><option key={v.id} value={v.id}>{v.name} · {t("Revision","Revision")} {v.revision}</option>)}</>}
    </select></label>
    {!entry.unsupported_reason&&<button disabled={busy} onClick={()=>void work(async g=>{if(!record)return;const value=await historicalProjectBody(api,record.id,entry.ordinal??index,entry);live(g);setBody(JSON.stringify(value,null,2));})}>{t("Originale Inhaltstexte ansehen","View original content text")}</button>}
    {entry.source.files.map(file=><label className="product-check" key={file.source_id}><input type="checkbox" disabled={busy||!!journal.preview_input||!!entry.activation_receipt_sha256||entry.editable_selection_closed} checked={historyFiles.includes(file.source_id)} onChange={e=>setHistoryFiles(e.target.checked?[...historyFiles,file.source_id]:historyFiles.filter(id=>id!==file.source_id))}/>{file.name}: {t("ausschließlich als Dateihinweis behalten. Sonst vor Aktivierung die echte Originaldatei auswählen und bestätigen.","retain only as a file reference. Otherwise select and confirm the actual original file before activation.")}</label>)}
   </article>)}
   {!!catalog.length&&!preview&&<><label className="product-check"><input type="checkbox" checked={remainder} disabled={busy||!!journal.preview_input||catalogSlice?.editable_selection_closed} onChange={e=>setRemainder(e.target.checked)}/>{t("Alle jetzt nicht ausgewählten Teile ausdrücklich vorerst als unveränderte Historie behalten. Weitere unaktivierte Teile können später aus demselben Import mit frischen Rechten übernommen werden; bereits aktivierte Teile werden nicht doppelt angelegt.","Explicitly retain all currently unselected parts as unchanged history for now. Other unactivated parts can be imported later from the same archive with fresh permissions; already activated parts are not created twice.")}</label><button disabled={busy||!journal.preview_input&&catalogSlice?.editable_selection_closed} onClick={()=>void work(review)}>{journal.preview_input?t("Dieselbe Vorschau abgleichen","Reconcile the same preview"):t("Ausgewählten Inhaltsbatch als unveränderlichen Plan prüfen","Review selected content batch as an immutable plan")}</button></>}
   {recoveryPlan&&<details open><summary>{t("Ursprüngliche Dateioperationen wiederaufnehmen","Recover original file operations")}</summary><p>{t("Auch wenn die alte Zielansicht heute nicht mehr zulässig ist, bleiben deine ursprünglichen Dateioperationen am aktuell berechtigten Importzugang abrufbar. Ein ungewisser Versand wird nicht wiederholt. Neue Inhalte brauchen einen neuen Plan mit aktuellen Rechten.","Even if the old target view is no longer authorized, your original file operations remain retrievable through the currently authorized import context. An uncertain dispatch is not repeated. New content requires a new plan using current permissions.")}</p>
    {catalog.flatMap(p=>p.source.files).map(file=>{const op=operations[file.source_id],closure=closures[file.source_id];return <div key={file.source_id}><strong>{file.name}</strong>{op&&<p>{stateLabel(op.state)} · <code>{op.request_sha256}</code></p>}<button disabled={busy} onClick={()=>void work(async g=>{const value=await fileStatus(g,recoveryPlan,file.source_id);if(!value)setProgress(t("Kein gespeicherter ursprünglicher Dateivorgang.","No stored original file operation."));})}>{t("Originalstatus abrufen","Retrieve original status")}</button>{(op?.state==="prepared"||op?.state==="unsent_closed")&&<button disabled={busy} onClick={()=>void work(g=>closeUnsent(g,recoveryPlan,file.source_id))}>{op.state==="unsent_closed"?t("Originalen Abschlussbeleg abrufen","Retrieve original closure receipt"):t("Ungesendet-Nachweis prüfen und ausdrücklich abschließen","Verify unsent proof and explicitly close")}</button>}{closure&&<details><summary>{t("Originaler Abschlussbeleg","Original closure receipt")}</summary><pre>{JSON.stringify(closure,null,2)}</pre></details>}</div>;})}
    <button disabled={busy} onClick={()=>void work(newPlan)}>{t("Getrennte Vorschau mit frischen Zuordnungen erstellen","Create separate preview with fresh mappings")}</button>
   </details>}
   {preview&&<>
    <p>{t("Plan","Plan")} <code>{preview.plan_sha256}</code> · {t("zusätzliche Inhaltsreservation","additional content reservation")} {bytes(preview.projected_bytes)}</p>
    {fileEntries.map(file=>{const op=operations[file.source_id],closure=closures[file.source_id];return <div className="product-item" key={file.source_id}>
     <strong>{file.name}</strong>{op&&<p>{stateLabel(op.state)} · <code>{op.request_sha256}</code></p>}
     {api.stageNativeFile?<button disabled={busy||op?.state==="unsent_closed"} onClick={()=>void work(g=>stage(g,preview,file.source_id))}>{t("Originaldatei nativ auswählen / abgleichen","Select original file natively / reconcile")}</button>:<label>{t("Echte Originaldatei auswählen (höchstens 32 MiB)","Select actual original file (up to 32 MiB)")}<input type="file" disabled={busy||op?.state==="unsent_closed"} onChange={e=>{const selected=e.target.files?.[0];if(selected)void work(async g=>{if(selected.size>32*1024*1024)throw new Error("historical_file_too_large");await stage(g,preview,file.source_id,selected);});}}/></label>}
     <button disabled={busy} onClick={()=>void work(async g=>{const result=await fileStatus(g,preview,file.source_id);if(!result)setProgress(t("Für diese Datei gibt es noch keinen dauerhaften ursprünglichen Vorgang.","This file has no durable original operation yet."));})}>{t("Gespeicherten Originalstatus abrufen","Retrieve stored original status")}</button>
     {(op?.state==="prepared"||op?.state==="unsent_closed")&&<button disabled={busy} onClick={()=>void work(g=>closeUnsent(g,preview,file.source_id))}>{op.state==="unsent_closed"?t("Originalen Abschlussbeleg abrufen","Retrieve original closure receipt"):t("Ungesendet-Nachweis prüfen und ausdrücklich abschließen","Verify unsent proof and explicitly close")}</button>}
     {op?.state==="prepared"&&<p>{t("Der Server schließt nur ohne ursprüngliche Versandzulassung ab. Ein schon zugelassener oder ungewisser Versand bleibt gesperrt.","The server closes only an operation without original dispatch admission. An already admitted or uncertain dispatch remains blocked.")}</p>}
     {closure&&<details><summary>{t("Originaler Abschlussbeleg","Original closure receipt")} · {t("freigegeben","released")}: {bytes(closure.released_bytes)} · {t("Originalmetadata weiterhin reserviert","original metadata still reserved")}: {bytes(closure.retained_reserved_bytes)}</summary><pre>{JSON.stringify(closure,null,2)}</pre></details>}
    </div>;})}
    <label className="product-check"><input type="checkbox" checked={confirmed} disabled={busy} onChange={e=>setConfirmed(e.target.checked)}/>{t("Genau diesen Plan jetzt mit meinen aktuellen Rechten als bearbeitbare Inhalte aktivieren (Schritt 4).","Activate exactly this plan now as editable content using my current permissions (step 4).")}</label>
    <button className="primary" disabled={busy||!confirmed||fileEntries.some(f=>operations[f.source_id]?.state==="unsent_closed")} onClick={()=>void work(activate)}>{t("Aktivieren / Originalaktivierung abgleichen","Activate / reconcile original activation")}</button>
    <button disabled={busy} onClick={()=>void work(async g=>{const value=await api.json<ActivationReceipt>(`/previews/${preview.id}/receipt`);live(g);setReceipt(value);})}>{t("Gespeicherte Originalquittung abrufen","Retrieve stored original receipt")}</button>
    <button disabled={busy} onClick={()=>void work(newPlan)}>{t("Getrennte Vorschau mit frischen Zuordnungen erstellen","Create separate preview with fresh mappings")}</button>
   </>}
  </>}
  {record&&record.state!=="receiving"&&<HistoricalHistoryBrowser api={api} record={record} limit={limit}/>}
  {body&&<details open><summary>{t("Originale Inhaltstexte","Original content text")}</summary><pre>{body}</pre><button onClick={()=>setBody(undefined)}>{t("Schließen","Close")}</button></details>}
  {receipt&&<><p role="status">{t("Aktivierung dauerhaft bestätigt.","Activation durably confirmed.")} {receipt.objects.length} {t("Zuordnungen","mappings")} · {new Date(receipt.activated_at).toLocaleString()}</p><details><summary>{t("Originalquittung","Original receipt")}</summary><pre>{JSON.stringify(receipt,null,2)}</pre></details><button disabled={busy} onClick={()=>void work(newPlan)}>{t("Weitere unaktivierte Teile aus demselben Import auswählen","Select other unactivated parts from the same import")}</button></>}
  <button disabled={busy} onClick={()=>void work(g=>load(g,journal))}>{t("Denselben Vorgang laden","Reload the same operation")}</button>
  <button disabled={busy} onClick={()=>{if(!window.confirm(t("Einen getrennten neuen Import beginnen? Der bisherige Originalvorgang und alle ungewissen Effekte bleiben erhalten.","Begin a separate new import? The previous original operation and all uncertain effects are retained.")))return;persist({});setRecord(undefined);setCatalog([]);setPreview(undefined);setReceipt(undefined);setMapping({});setHistoryFiles([]);setBody(undefined);setConfirmed(false);setProgress("");setOperations({});setClosures({});}}>{t("Getrennten neuen Import beginnen","Begin separate new import")}</button>
 </section>;
}
