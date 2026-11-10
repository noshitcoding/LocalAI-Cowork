import {useTeamText} from "./teamStrings";
import {useEffect,useState} from "react";
import {Client} from "@cowork/client";
import {productRequest} from "./productApi";
import {deliverDownload} from "./download";
import {useView} from "./view";

export type ImportedAttachment={id:string,name:string,media_type:string,size:number,sha256:string};
export function checkedAttachments(value:unknown):ImportedAttachment[]{
  if(!Array.isArray(value)||value.length>1000||!value.every(item=>item&&typeof item==="object"&&typeof item.id==="string"&&typeof item.name==="string"&&typeof item.media_type==="string"&&Number.isSafeInteger(item.size)&&item.size>=0&&item.size<=16777216&&typeof item.sha256==="string"&&/^[0-9a-f]{64}$/.test(item.sha256)))throw new Error("invalid_imported_attachments");
  return value as ImportedAttachment[];
}
export function AttachmentList({client,path,session,items}:{client:Client,path:string,session?:string,items:ImportedAttachment[]}){
  const tr=useTeamText();
  const language=useView(value=>value.language);const [busy,setBusy]=useState(false);const [error,setError]=useState<unknown>();
  async function download(item:ImportedAttachment){setBusy(true);try{const response=await fetch(client.base+"/api/v2"+path+"/"+encodeURIComponent(item.id),{headers:{Authorization:"Bearer "+(session??client.token)},cache:"no-store"});if(!response.ok)throw new Error("download_failed ("+response.status+")");await deliverDownload(await response.blob(),item.name,language,{size:item.size,sha256:item.sha256});setError(undefined);}catch(e){setError(e);}finally{setBusy(false);}}
  return items.length?<div><h4>{tr("Originalanhänge aus V1")}</h4>{items.slice(0,1000).map(item=><p key={item.id}>{item.name} · {item.size.toLocaleString()} Bytes <button disabled={busy} onClick={()=>void download(item)}>{tr("Verifiziert herunterladen")}</button></p>)}{!!error&&<p className="error" role="alert">{error instanceof Error?error.message:String(error)}</p>}</div>:null;
}
export function ImportedAttachments({client,project,order}:{client:Client,project:string,order:string}){
  const tr=useTeamText();
  const [items,setItems]=useState<ImportedAttachment[]>([]);const [error,setError]=useState(false);const [retry,setRetry]=useState(0);const path=`/projects/${encodeURIComponent(project)}/orders/${encodeURIComponent(order)}/imported-attachments`;
  useEffect(()=>{let current=true;setItems([]);setError(false);if(project&&order)void productRequest<unknown>(client,path).then(value=>{const checked=checkedAttachments(value);if(current)setItems(checked);}).catch(()=>{if(current)setError(true);});return()=>{current=false;};},[client.base,client.token,project,order,retry]);
  return <><AttachmentList client={client} path={path} items={items}/>{error&&<p className="muted">{tr("Originalanhänge konnten nicht geladen werden. ")}<button onClick={()=>setRetry(value=>value+1)}>{tr("Erneut versuchen")}</button></p>}</>;
}
