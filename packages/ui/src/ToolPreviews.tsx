import {useEffect,useState} from "react";
import type {Client,RunEvent} from "@cowork/client";
import {productDownload} from "./productApi";
import type {Key} from "./i18n";
import "./documentWorkbench.css";
type T=(key:Key)=>string;
export type PreviewImage={sha256:string,page?:number,width?:number,height?:number};
export type PreviewGroup={tool_call_id:string,name:string,images:PreviewImage[]};

const sha=/^[0-9a-f]{64}$/;
const record=(value:unknown)=>value!==null&&typeof value==="object"&&!Array.isArray(value)?value as Record<string,unknown>:undefined;
/** Rendered previews from completed document tools, oldest first. */
export function previewGroups(events:RunEvent[]):PreviewGroup[] {
  return events.flatMap(event=>{
    const payload=record(event.payload);if(!payload)return [];
    let references=payload.images,id=payload.tool_call_id,name=payload.name;
    if(event.kind==="workflow_native_tool_completed"||event.kind==="workflow_tool_completed"){
      const receipt=record(payload.receipt),tool=record(receipt?.tool),result=record(receipt?.result);
      const toolName=tool?.name;
      if(receipt?.succeeded!==true||typeof event.id!=="string"||!event.id||typeof toolName!=="string"||!["document_read","document_write","image_edit"].includes(toolName))return [];
      references=result?.cowork_images;id="workflow:"+event.id;name=toolName;
    }else if(event.kind!=="tool_completed")return [];
    if(!Array.isArray(references)||references.length>16)return [];
    const dimension=(value:unknown)=>typeof value==="number"&&Number.isSafeInteger(value)&&value>0&&value<=100_000?value:undefined;
    const images=references.flatMap(value=>{
      const image=record(value);
      return image&&typeof image.sha256==="string"&&sha.test(image.sha256)?[{sha256:image.sha256,page:dimension(image.page),width:dimension(image.width),height:dimension(image.height)}]:[];
    });
    return images.length&&typeof id==="string"?[{tool_call_id:id,name:typeof name==="string"?name:"",images}]:[];
  });
}

export async function verifiedPreviewUrl(blob:Blob,expected:string):Promise<string> {
  const bytes=await blob.arrayBuffer();
  const digest=Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256",bytes))).map(b=>b.toString(16).padStart(2,"0")).join("");
  if(digest!==expected)throw new Error("preview_integrity");
  return URL.createObjectURL(new Blob([bytes],{type:"image/png"}));
}

function Preview({client,run,image,t}:{client:Client,run:string,image:PreviewImage,t:T}) {
  const [url,setUrl]=useState<string>();
  const [failed,setFailed]=useState(false);
  useEffect(()=>{
    let current:string|undefined;let active=true;
    productDownload(client,`/runs/${encodeURIComponent(run)}/previews/${image.sha256}`).then(blob=>verifiedPreviewUrl(blob,image.sha256)).then(value=>{current=value;if(active)setUrl(value);else URL.revokeObjectURL(value);},()=>{if(active)setFailed(true);});
    return ()=>{active=false;if(current)URL.revokeObjectURL(current);};
  },[client,run,image.sha256]);
  const label=image.page?`${t("documentPreviews")} · ${image.page}`:t("documentPreviews");
  return <figure className="document-preview-thumb">{url?<a href={url} target="_blank" rel="noreferrer"><img src={url} alt={label} width={image.width} height={image.height} loading="lazy"/></a>:<p role="status">{t(failed?"previewUnavailable":"loading")}</p>}<figcaption>{image.page??""}</figcaption></figure>;
}

export function ToolPreviews({client,run,events,t}:{client:Client,run:string,events:RunEvent[],t:T}) {
  const groups=previewGroups(events);
  if(!groups.length)return null;
  return <section className="file-changes document-previews"><h2>{t("documentPreviews")}</h2>
    {groups.map(group=><article key={group.tool_call_id} className="file-change"><h4>{t((["document_read","document_write","image_edit"].includes(group.name)?group.name:"details") as Key)}</h4>
      <div className="document-preview-grid">{group.images.map(image=><Preview key={image.sha256} client={client} run={run} image={image} t={t}/>)}</div></article>)}
  </section>;
}
