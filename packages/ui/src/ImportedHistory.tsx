import {useEffect,useState} from "react";
import {Client} from "@cowork/client";
import {productRequest} from "./productApi";
import {deliverDownload,type DeliveryStatus} from "./download";
import {useView} from "./view";
import {ImportedAttachments} from "./ImportedAttachments";

type Message={id:string,role:"system"|"user"|"assistant"|"tool",content:string,created_at:number};
type Artifact={id:string,name:string,media_type:string,size:number,sha256:string,content:string,created_at:number};
type History={project_id:string,order_id:string,messages:Message[],artifacts:Artifact[],revision:number,imported_at:number};
type Props={client:Client,project:string,order:string};
type Loaded={client:Client,project:string,order:string,history:History|null,error:boolean,loading:boolean};
const record=(value:unknown):value is Record<string,unknown>=>typeof value==="object"&&value!==null&&!Array.isArray(value);
const integer=(value:unknown):value is number=>typeof value==="number"&&Number.isSafeInteger(value)&&value>=0;
function checked(value:unknown,project:string,order:string):History|null{
  if(value===null)return null;
  if(!record(value)||value.project_id!==project||value.order_id!==order||!integer(value.revision)||!integer(value.imported_at)||!Array.isArray(value.messages)||value.messages.length>10_000||!Array.isArray(value.artifacts)||value.artifacts.length>1_000)throw new Error("invalid_history");
  if(!value.messages.every(item=>record(item)&&typeof item.id==="string"&&["system","user","assistant","tool"].includes(String(item.role))&&typeof item.content==="string"&&item.content.length<=1_048_576&&integer(item.created_at)))throw new Error("invalid_history");
  if(!value.artifacts.every(item=>record(item)&&typeof item.id==="string"&&typeof item.name==="string"&&typeof item.media_type==="string"&&integer(item.size)&&item.size<=16_777_216&&typeof item.sha256==="string"&&/^[0-9a-f]{64}$/.test(item.sha256)&&typeof item.content==="string"&&integer(item.created_at)))throw new Error("invalid_history");
  return value as unknown as History;
}

/** Imported records remain text and downloads; no imported execution is resumed. */
export function ImportedHistory({client,project,order}:Props){
  const language=useView(value=>value.language);
  const [loaded,setLoaded]=useState<Loaded|null>(null);
  const [retry,setRetry]=useState(0);
  const [visible,setVisible]=useState(25);
  const [visibleFiles,setVisibleFiles]=useState(25);
  const [saving,setSaving]=useState<string|null>(null);
  const [delivery,setDelivery]=useState<{id:string,status:DeliveryStatus|"failed"}|null>(null);
  const de=language==="de";
  useEffect(()=>{
    let current=true;
    setLoaded({client,project,order,history:null,error:false,loading:true});
    setVisible(25);setVisibleFiles(25);setSaving(null);setDelivery(null);
    if(!project||!order){setLoaded({client,project,order,history:null,error:false,loading:false});return;}
    void productRequest<unknown>(client,`/projects/${encodeURIComponent(project)}/orders/${encodeURIComponent(order)}/imported-history`)
      .then(value=>{const history=checked(value,project,order);if(current)setLoaded({client,project,order,history,error:false,loading:false});})
      .catch(()=>{if(current)setLoaded({client,project,order,history:null,error:true,loading:false});});
    return()=>{current=false;};
  },[client,project,order,retry]);
  const scoped=loaded?.client===client&&loaded.project===project&&loaded.order===order?loaded:null;
  if(!project||!order)return null;
  if(!scoped||scoped.loading)return <p role="status">{de?"Importverlauf wird geladen …":"Loading imported history …"}</p>;
  if(scoped.error)return <div><p role="alert">{de?"Der Importverlauf konnte nicht geladen werden.":"The imported history could not be loaded."}</p><button onClick={()=>setRetry(value=>value+1)}>{de?"Erneut versuchen":"Retry"}</button></div>;
  const history=scoped.history;
  if(!history||!history.messages.length&&!history.artifacts.length)return <ImportedAttachments client={client} project={project} order={order}/>;
  async function download(artifact:Artifact){
    setSaving(artifact.id);setDelivery(null);
    try{
      const status=await deliverDownload(new Blob([artifact.content],{type:artifact.media_type}),artifact.name,language,{size:artifact.size,sha256:artifact.sha256});
      setDelivery({id:artifact.id,status});
    }catch{setDelivery({id:artifact.id,status:"failed"});}finally{setSaving(null);}
  }
  const roleName=(role:Message["role"])=>de?{system:"System",user:"Benutzer",assistant:"Assistent",tool:"Werkzeug"}[role]:{system:"System",user:"User",assistant:"Assistant",tool:"Tool"}[role];
  const outcome=delivery?.status==="saved"?(de?"Datei gespeichert.":"File saved."):delivery?.status==="canceled"?(de?"Speichern abgebrochen.":"Save canceled."):delivery?.status==="browser"?(de?"Download an den Browser übergeben.":"Download handed to the browser."):delivery?.status==="failed"?(de?"Die Datei konnte nicht verifiziert oder gespeichert werden.":"The file could not be verified or saved."):null;
  return <section className="imported-history">
    <h3>{de?"Importierter Verlauf":"Imported history"}</h3>
    <p className="muted">{de?"Archivierte Nachrichten und Dateien. Der Import startet keine Läufe oder Werkzeuge.":"Archived messages and files. Importing does not start runs or tools."}</p>
    {history.messages.slice(0,visible).map((message,index)=><article className="product-item" key={`${message.id}:${index}`}><strong>{roleName(message.role)}</strong><pre className="product-preview" style={{whiteSpace:"pre-wrap",overflowWrap:"anywhere"}}>{message.content}</pre></article>)}
    {visible<history.messages.length&&<button onClick={()=>setVisible(value=>value+25)}>{de?"Weitere Nachrichten anzeigen":"Show more messages"} ({history.messages.length-visible})</button>}
    {history.artifacts.length>0&&<><h4>{de?"Importierte Dateien":"Imported files"}</h4><ul>{history.artifacts.slice(0,visibleFiles).map((artifact,index)=><li key={`${artifact.id}:${index}`}><span>{artifact.name} · {artifact.size.toLocaleString(language)} {de?"Bytes":"bytes"}</span> <button disabled={saving!==null} onClick={()=>void download(artifact)}>{saving===artifact.id?(de?"Wird gespeichert …":"Saving …"):(de?"Herunterladen":"Download")}</button></li>)}</ul>{visibleFiles<history.artifacts.length&&<button onClick={()=>setVisibleFiles(value=>value+25)}>{de?"Weitere Dateien anzeigen":"Show more files"} ({history.artifacts.length-visibleFiles})</button>}</>}
    {outcome&&<p role={delivery?.status==="failed"?"alert":"status"}>{outcome}</p>}
    <ImportedAttachments client={client} project={project} order={order}/>
  </section>;
}
