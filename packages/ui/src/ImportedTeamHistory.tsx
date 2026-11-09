import {useTeamText} from "./teamStrings";
import {useEffect,useState} from "react";
import {Client} from "@cowork/client";
import {productRequest} from "./productApi";
import {AttachmentList,checkedAttachments,type ImportedAttachment} from "./ImportedAttachments";

type History={project_id:string,order_id:string,inert:true,source_project_id:string,messages:{id:string,role:string,content:string}[],artifacts:{id:string,name:string,content:string}[],attachments:ImportedAttachment[],source_entities:{kind:string,source_id:string,name:string}[]};
function checked(value:unknown,project:string,order:string):History{
  const item=value as History;
  if(!item||item.project_id!==project||item.order_id!==order||item.inert!==true||typeof item.source_project_id!=="string"||!Array.isArray(item.messages)||item.messages.length>10000||!item.messages.every(message=>message&&typeof message.id==="string"&&typeof message.role==="string"&&typeof message.content==="string")||!Array.isArray(item.artifacts)||item.artifacts.length>1000||!item.artifacts.every(artifact=>artifact&&typeof artifact.id==="string"&&typeof artifact.name==="string"&&typeof artifact.content==="string")||!Array.isArray(item.source_entities))throw new Error("invalid_imported_history");
  checkedAttachments(item.attachments);return item;
}
export function ImportedTeamHistory({client,session,team,project,order}:{client:Client,session:string,team:string,project:string,order:string}){
  const tr=useTeamText();
  const [history,setHistory]=useState<History|null>(null);const [visible,setVisible]=useState(25);const [error,setError]=useState<unknown>();const base=`/teams/${team}/projects/${project}/import-v1`;
  useEffect(()=>{let current=true;setHistory(null);setVisible(25);setError(undefined);if(team&&project&&order)void productRequest<unknown>(client,`/teams/${team}/projects/${project}/orders/${order}/imported-history`,"GET",undefined,session).then(value=>{const history=checked(value,project,order);if(current)setHistory(history);}).catch(e=>{if(current&&!(e instanceof Error&&e.message.endsWith(" (404)")))setError(e);});return()=>{current=false;};},[client.base,session,team,project,order]);
  if(!history)return error?<p className="muted">{tr("Importverlauf konnte nicht geladen werden.")}</p>:null;
  return <section><h3>{tr("Übernommener V1-Verlauf")}</h3><p className="muted">{tr("Ursprüngliches Projekt: ")}{history.source_project_id}</p><p className="muted">{tr("Historische Inhalte. Der Import hat keine Modell- oder Werkzeugaktionen gestartet.")}</p>{history.messages.slice(0,visible).map(message=><article className="product-item" key={message.id}><strong>{message.role}</strong><pre className="product-preview" style={{whiteSpace:"pre-wrap"}}>{message.content}</pre></article>)}{visible<history.messages.length&&<button onClick={()=>setVisible(value=>value+25)}>{tr("Weitere Nachrichten")}</button>}{history.artifacts.map(artifact=><details key={artifact.id}><summary>{artifact.name}</summary><pre className="product-preview">{artifact.content}</pre></details>)}<AttachmentList client={client} session={session} path={base+"/attachments"} items={history.attachments}/>{history.source_entities.length>0&&<details><summary>{tr("Herkunft aus V1")}</summary>{history.source_entities.map((entity,index)=><p key={index}>{entity.kind} · {entity.name} · {entity.source_id}</p>)}</details>}</section>;
}
