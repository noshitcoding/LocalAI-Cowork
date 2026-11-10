import {useEffect,useRef,useState} from "react";
import {decode,type BuildHistoricalHistoryIndex,type HistoricalHistoryIndex,type HistoricalHistoryTable,type HistoricalHistoryRow,type HistoricalHistoryCell,type HistoricalHistoryCellsPage,type HistoricalHistoryTablesPage,type HistoricalHistoryRowsPage} from "@cowork/client";
import {type HistoricalImportApi,type ImportRecord,type ImportLimit} from "./historicalImport";
import {useView} from "./view";

const digest=async(bytes:Uint8Array)=>Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256",Uint8Array.from(bytes).buffer))).map(b=>b.toString(16).padStart(2,"0")).join("");
const raw=(s:string)=>Uint8Array.from(atob(s),c=>c.charCodeAt(0));
const hex=(b:Uint8Array)=>Array.from(b).map(v=>v.toString(16).padStart(2,"0")).join(" ");
const amount=(n:number)=>`${n.toLocaleString()} B`;
type ByteCell=Extract<HistoricalHistoryCell,{kind:"text_bytes"|"blob"}>;
function preview(cell:HistoricalHistoryCell):string {
 if(cell.kind==="null")return "NULL";
 if(cell.kind==="integer")return cell.decimal;
 if(cell.kind==="real_bits")return `0x${cell.ieee754_binary64_be}`;
 const bytes=raw(cell.preview_base64);
 if(cell.kind==="text_bytes"){try{return new TextDecoder("utf-8",{fatal:true}).decode(bytes);}catch{}}
 return hex(bytes);
}
function message(error:unknown){return error instanceof Error?error.message:error&&typeof error==="object"&&"code" in error?String(error.code):String(error);}
function missing(error:unknown){return /^not_found(?: \(404\))?$/.test(message(error));}
export function HistoricalHistoryBrowser({api,record,limit}:{api:HistoricalImportApi,record:ImportRecord,limit?:ImportLimit}) {
 const language=useView(v=>v.language),t=(de:string,en:string)=>language==="de"?de:en;
 const epoch=useRef(0),key=`cowork.m9-history-index.v1:${api.scope}:${record.id}`;
 const [index,setIndex]=useState<HistoricalHistoryIndex>(),[tables,setTables]=useState<HistoricalHistoryTablesPage>(),[selected,setSelected]=useState<HistoricalHistoryTable>(),[rows,setRows]=useState<HistoricalHistoryRowsPage>(),[row,setRow]=useState<HistoricalHistoryRow>(),[cells,setCells]=useState<HistoricalHistoryCellsPage>(),[value,setValue]=useState<{column:number,page:number,bytes:Uint8Array,kind:string}>(),[busy,setBusy]=useState(false),[error,setError]=useState(""),[absent,setAbsent]=useState(false),[confirmed,setConfirmed]=useState(false);
 const base=`/imports/${record.id}/history`;
 const live=(g:number)=>{if(g!==epoch.current)throw new Error("historical_context_changed");};
 async function work(action:(g:number)=>Promise<void>){if(busy)return;const g=epoch.current;setBusy(true);setError("");try{await action(g);}catch(e){if(g===epoch.current)setError(message(e));}finally{if(g===epoch.current)setBusy(false);}}
 function validateIndex(v:HistoricalHistoryIndex){if(v.import_id!==record.id||v.rows!==record.original_rows||v.source_profile!==record.source_profile||Object.keys(v.pins).some(k=>v.pins[k as keyof typeof v.pins]!==record.pins[k]))throw new Error("historical_content_integrity");}
 async function tablePage(g:number,root:HistoricalHistoryIndex,offset:number){
  const v=decode("HistoricalHistoryTablesPage",await api.json<unknown>(`${base}/tables?offset=${offset}&limit=10`));live(g);
  if(v.index_sha256!==root.index_sha256||v.offset!==offset||v.tables.some((table,i)=>table.ordinal!==offset+i))throw new Error("historical_content_integrity");setTables(v);
 }
 async function load(g:number){
  let v:HistoricalHistoryIndex;try{v=decode("HistoricalHistoryIndex",await api.json<unknown>(base));}catch(e){if(!missing(e))throw e;live(g);setAbsent(true);return;}
  live(g);validateIndex(v);setIndex(v);setAbsent(false);await tablePage(g,v,0);
 }
 useEffect(()=>{epoch.current++;const g=epoch.current;setIndex(undefined);setTables(undefined);setSelected(undefined);setRows(undefined);setRow(undefined);setCells(undefined);setValue(undefined);setBusy(false);setError("");setAbsent(false);setConfirmed(false);void load(g).catch(e=>{if(g===epoch.current)setError(message(e));});return()=>{epoch.current++;};},[api.scope,record.id]);
 async function build(g:number){
  if(!limit||!confirmed)throw new Error("explicit_history_index_required");
  const saved=localStorage.getItem(key);const request:BuildHistoricalHistoryIndex=saved?decode("BuildHistoricalHistoryIndex",JSON.parse(saved)):{request_id:crypto.randomUUID(),expected_import_revision:record.revision,expected_limit_revision:limit.revision};
  if(!saved)localStorage.setItem(key,JSON.stringify(request));
  const v=decode("HistoricalHistoryIndex",await api.json<unknown>(`${base}/index`,"POST",request));live(g);validateIndex(v);setIndex(v);setAbsent(false);await tablePage(g,v,0);
 }
 async function rowPage(g:number,table:HistoricalHistoryTable,offset:number){
  if(!index)return;const v=decode("HistoricalHistoryRowsPage",await api.json<unknown>(`${base}/tables/${table.ordinal}/rows?offset=${offset}&limit=10`));live(g);
  if(v.index_sha256!==index.index_sha256||v.table_ordinal!==table.ordinal||v.offset!==offset||v.rows.some((r,i)=>r.position!==offset+i+1||r.columns!==table.columns.length))throw new Error("historical_content_integrity");
  setSelected(table);setRows(v);setRow(undefined);setCells(undefined);setValue(undefined);
 }
 async function cellPage(g:number,r:HistoricalHistoryRow,page:number){
  if(!selected)return;const v=decode("HistoricalHistoryCellsPage",await api.json<unknown>(`${base}/tables/${selected.ordinal}/rows/${r.position}/cells/${page}`));live(g);
  if(v.table_ordinal!==selected.ordinal||v.position!==r.position||v.rowid!==r.rowid||v.page_index!==page||v.cells.some((c,i)=>c.column_index!==page*16+i))throw new Error("historical_content_integrity");setRow(r);setCells(v);setValue(undefined);
 }
 async function bytePage(g:number,column:number,cell:ByteCell,page:number):Promise<Uint8Array>{
  if(!selected||!row||!cell.pages[page])throw new Error("historical_content_integrity");
  const b=new Uint8Array(await api.binary(`${base}/tables/${selected.ordinal}/rows/${row.position}/cells/${column}/bytes/${page}`));live(g);
  if(b.length>1024*1024||b.length!==cell.pages[page].size||await digest(b)!==cell.pages[page].sha256)throw new Error("historical_content_integrity");live(g);return b;
 }
 async function showBytes(g:number,column:number,cell:ByteCell,page:number){const b=await bytePage(g,column,cell,page);live(g);setValue({column,page,bytes:b,kind:cell.kind});}
 async function download(g:number,column:number,cell:ByteCell){
  if(!selected||!row||cell.size>16*1024*1024)throw new Error("historical_content_integrity");
  const b=new Uint8Array(cell.size);let offset=0;
  for(let page=0;page<cell.pages.length;page++){const original=await bytePage(g,column,cell,page);if(original.length>b.length-offset)throw new Error("historical_content_integrity");b.set(original,offset);offset+=original.length;}
  if(offset!==cell.size||await digest(b)!==cell.sha256)throw new Error("historical_content_integrity");live(g);
  const url=URL.createObjectURL(new Blob([b],{type:"application/octet-stream"}));const link=document.createElement("a");link.download=`${selected.name}.${selected.columns[column]}.${row.rowid}.${cell.kind}.bin`.replace(/[^\p{L}\p{N}_.-]/gu,"_");link.href=url;link.click();setTimeout(()=>URL.revokeObjectURL(url),30000);
 }
 let rendered="";if(value){const b=value.bytes.subarray(0,Math.min(value.bytes.length,64*1024));try{rendered=value.kind==="text_bytes"?new TextDecoder("utf-8",{fatal:true}).decode(b):hex(b);}catch{rendered=hex(b);}}
 const currentCell=cells?.cells.find(c=>c.column_index===value?.column)?.value;
 const errors:Record<string,string>={forbidden:t("Die aktuellen Eigentümer-/Organisationsrechte erlauben diesen Vorgang nicht.","Current owner/organization permissions do not allow this operation."),conflict:t("Der ursprüngliche Auftrag passt nicht mehr zur aktuellen Import- oder Limitrevision. Originalstatus laden; falls noch kein Index besteht, einen getrennten aktuellen Auftrag vorbereiten.","The original request no longer matches the current import or limit revision. Load original status; if no index exists, prepare a separate current request."),budget_exceeded:t("Der zusätzliche Index passt nicht in das aktuell verfügbare Speicherbudget.","The additional index exceeds the currently available storage budget."),workspace_busy:t("Die begrenzte Indexaufnahme ist derzeit belegt oder geschlossen.","Bounded index intake is currently busy or closed."),historical_content_integrity:t("Originalbindung oder SHA-256-Prüfung fehlgeschlagen. Die Bytes werden nicht verwendet.","Original binding or SHA-256 validation failed. The bytes are not used."),explicit_history_index_required:t("Den zusätzlichen lesenden Index ausdrücklich bestätigen.","Explicitly confirm the additional read-only index."),historical_context_changed:t("Der aktuelle Importzugang hat sich geändert.","The current import context changed.")};
 return <section className="product-panel" aria-label={t("Vollständige Originalhistorie","Complete original history")}>
  <h4>{t("Alle originalen Tabellen durchsuchen","Browse all original tables")}</h4>
  <p>{t("Akteure, ACLs, Freigaben, Zeitpläne und Runverläufe sind ausschließlich historische Daten. Jede Seite verlangt deine aktuellen Leserechte; kein Originaldatensatz wird als heutiges Konto oder Ausführungsrecht übernommen.","Actors, ACLs, approvals, schedules and run histories are historical data only. Every page requires your current read permissions; no original record becomes a current account or execution permission.")}</p>
  {error&&<p className="error" role="alert">{errors[error.replace(/ \(\d+\)$/,"")]??t("Die Historie konnte nicht geladen werden.","The history could not be loaded.")} <code>{error}</code></p>}
  {absent&&<>
   <p>{t("Für diesen geprüften Import gibt es noch keinen vollständigen Browserindex. Der ursprüngliche Import bleibt unverändert. Die zusätzlichen verschlüsselten Seiten werden atomar unter dem aktuellen Betreiber- und Organisationsbudget reserviert.","This verified import does not yet have a complete browser index. The original import remains unchanged. Additional encrypted pages are reserved atomically under the current operator and organization budget.")}</p>
   <label className="product-check"><input type="checkbox" disabled={busy} checked={confirmed} onChange={e=>setConfirmed(e.target.checked)}/>{t("Zusätzlichen lesenden Index ausdrücklich erstellen.","Explicitly create the additional read-only index.")}</label>
   <button disabled={busy||!confirmed||!limit} onClick={()=>void work(build)}>{t("Index erstellen / Originalabschluss abrufen","Build index / retrieve original completion")}</button>
   <button disabled={busy} onClick={()=>{localStorage.removeItem(key);setError("");}}>{t("Getrennten Indexauftrag mit aktuellen Revisionen vorbereiten","Prepare separate index request with current revisions")}</button>
  </>}
  {index&&<>
   <p>{index.tables.length} {t("Tabellen","tables")} · {index.rows.toLocaleString()} {t("Originalzeilen","original rows")} · {index.cells.toLocaleString()} {t("Zellen","cells")} · {amount(index.reserved_bytes)} {t("Indexreservation","index reservation")}</p>
   <details><summary>{t("Ursprünglicher Indexabschluss und Pins","Original index completion and pins")}</summary><pre>{JSON.stringify(index,null,2)}</pre></details>
   {tables&&<><ul>{tables.tables.map(v=><li key={v.ordinal}><button disabled={busy} onClick={()=>void work(g=>rowPage(g,v,0))}>{v.name} · {v.rows.toLocaleString()} {t("Zeilen","rows")}</button></li>)}</ul><button disabled={busy||tables.offset===0} onClick={()=>void work(g=>tablePage(g,index,Math.max(0,tables.offset-10)))}>{t("Vorherige Tabellen","Previous tables")}</button><button disabled={busy||tables.next_offset==null} onClick={()=>void work(g=>tablePage(g,index,tables.next_offset??0))}>{t("Weitere Tabellen","More tables")}</button></>}
   {selected&&rows&&<>
    <h5>{selected.name}</h5><p>{t("Originale kanonische Tabellen-SHA","Original canonical table SHA")}: <code>{selected.canonical_sha256}</code> · {amount(selected.canonical_bytes)}</p>{selected.rows===0&&<p>{t("Die ursprüngliche Tabelle ist leer.","The original table is empty.")}</p>}
    <ul>{rows.rows.map(v=><li key={v.position}><button disabled={busy} onClick={()=>void work(g=>cellPage(g,v,0))}>{t("Position","Position")} {v.position} · {t("Original-rowid","Original rowid")} <code>{v.rowid}</code> · {v.columns} {t("Spalten","columns")}</button></li>)}</ul><button disabled={busy||rows.offset===0} onClick={()=>void work(g=>rowPage(g,selected,Math.max(0,rows.offset-10)))}>{t("Vorherige Zeilen","Previous rows")}</button><button disabled={busy||rows.next_offset==null} onClick={()=>void work(g=>rowPage(g,selected,rows.next_offset??0))}>{t("Weitere Zeilen","More rows")}</button>
   </>}
   {selected&&row&&cells&&<>
    <p>{t("Originale Zeilen-SHA","Original row SHA")}: <code>{row.original_sha256}</code></p>
    <table><thead><tr><th>{t("Spalte","Column")}</th><th>{t("Originaltyp","Original type")}</th><th>{t("Wert / begrenzte Bytevorschau","Value / bounded byte preview")}</th></tr></thead><tbody>{cells.cells.map(c=><tr key={c.column_index}><td>{selected.columns[c.column_index]}</td><td>{c.value.kind}</td><td><pre>{preview(c.value)}</pre>{(c.value.kind==="text_bytes"||c.value.kind==="blob")&&<><p>{amount(c.value.size)} · <code>{c.value.sha256}</code></p>{c.value.pages.length>0&&<button disabled={busy} onClick={()=>void work(g=>showBytes(g,c.column_index,c.value as ByteCell,0))}>{t("Originalbyte-Seite ausdrücklich laden","Explicitly load original byte page")}</button>}<button disabled={busy} onClick={()=>void work(g=>download(g,c.column_index,c.value as ByteCell))}>{t("Alle Originalbytes geprüft herunterladen","Download all verified original bytes")}</button></>}</td></tr>)}</tbody></table>
    <button disabled={busy||cells.page_index===0} onClick={()=>void work(g=>cellPage(g,row,cells.page_index-1))}>{t("Vorherige Spalten","Previous columns")}</button><button disabled={busy||cells.page_index+1>=row.cell_pages.length} onClick={()=>void work(g=>cellPage(g,row,cells.page_index+1))}>{t("Weitere Spalten","More columns")}</button>
   </>}
   {value&&currentCell&&(currentCell.kind==="text_bytes"||currentCell.kind==="blob")&&<details open><summary>{t("Originalbyte-Seite","Original byte page")} {value.page+1} · {amount(value.bytes.length)}</summary><p>{t("Anzeige höchstens 64 KiB; ungültiges UTF-8 wird als Hex dargestellt. Der Download erhält sämtliche Originalbytes.","Display is limited to 64 KiB; invalid UTF-8 is shown as hex. The download preserves every original byte.")}</p><pre>{rendered}</pre><button disabled={busy||value.page===0} onClick={()=>void work(g=>showBytes(g,value.column,currentCell,value.page-1))}>{t("Vorherige Byteseite","Previous byte page")}</button><button disabled={busy||value.page+1>=currentCell.pages.length} onClick={()=>void work(g=>showBytes(g,value.column,currentCell,value.page+1))}>{t("Weitere Byteseite","More byte pages")}</button></details>}
  </>}
  <button disabled={busy} onClick={()=>void work(load)}>{t("Originalen Indexstatus laden","Load original index status")}</button>{busy&&<p role="status">{t("Originalhistorie wird verarbeitet …","Processing original history …")}</p>}
 </section>;
}
