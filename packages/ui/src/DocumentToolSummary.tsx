import type {FolderGrant} from "@cowork/client";
import type {Key} from "./i18n";
type T=(key:Key)=>string;
type Location={folder_id?:string,path:string,expected_sha256?:string};
export type DocumentToolSummaryModel={operation:string,inputs:Location[],output?:Location,counts:string[],steps:string[],markdown?:string};

export const documentTools=["document_read","document_write","image_edit"] as const;
export const isDocumentTool=(name:string)=>(documentTools as readonly string[]).includes(name);
const record=(value:unknown):Record<string,unknown>=>value&&typeof value==="object"&&!Array.isArray(value)?value as Record<string,unknown>:{};
const location=(value:unknown):Location|undefined=>{const v=record(value);return typeof v.path==="string"?{path:v.path,folder_id:typeof v.folder_id==="string"?v.folder_id:undefined,expected_sha256:typeof v.expected_sha256==="string"?v.expected_sha256:undefined}:undefined;};

/** Readable approval facts from raw document tool arguments; unknown fields stay in the JSON details. */
export function summarizeDocumentTool(name:string,raw:unknown):DocumentToolSummaryModel {
  const args=record(raw),spec=record(args.spec);
  if(name==="image_edit"){
    const steps=Array.isArray(args.steps)?args.steps.map(step=>String(record(step).op??"?")):[];
    const overlays=Array.isArray(args.overlays)?args.overlays.map(location).filter((l):l is Location=>!!l):[];
    return {operation:"pipeline",inputs:[location(args.input),...overlays].filter((l):l is Location=>!!l),output:location(args.output),counts:[],steps};
  }
  if(name==="document_read"){
    const input=location(args);
    return {operation:String(args.operation??""),inputs:input?[input]:[],counts:typeof args.topic==="string"?["topic: "+args.topic]:[],steps:[]};
  }
  const counts=Object.entries(spec).flatMap(([key,value])=>Array.isArray(value)?[`${key}: ${value.length}`]:typeof value==="string"&&["format","action","title"].includes(key)?[`${key}: ${value}`]:[]);
  const inputs=Array.isArray(args.inputs)?args.inputs.map(location).filter((l):l is Location=>!!l):[];
  return {operation:String(args.operation??""),inputs,output:location(args.output),counts,steps:[],markdown:typeof spec.markdown==="string"?spec.markdown:undefined};
}

export function DocumentToolSummary({name,args,folders,t}:{name:string,args:unknown,folders:FolderGrant[],t:T}) {
  const summary=summarizeDocumentTool(name,args);
  const working=folders.find(f=>!!f.order_id);
  const folder=(id?:string)=>{const grant=id?folders.find(f=>f.id===id):working;return grant?(grant.order_id?t("workingFiles"):grant.name):t("forbidden");};
  return <div className="document-tool-summary">
    <dl className="file-scope">
      <dt>{t("documentOperation")}</dt><dd>{summary.operation}</dd>
      {summary.inputs.map((input,i)=><div key={i} className="document-tool-input"><dt>{i===0?t("documentInputs"):""}</dt><dd>{folder(input.folder_id)} · {input.path}{input.expected_sha256&&<small> · {input.expected_sha256.slice(0,12)}</small>}</dd></div>)}
      {summary.output&&<><dt>{t("documentOutput")}</dt><dd>{folder(summary.output.folder_id)} · {summary.output.path}</dd></>}
      {!!summary.counts.length&&<><dt>{t("documentContent")}</dt><dd>{summary.counts.join(" · ")}</dd></>}
    </dl>
    {summary.output&&<p>{t("documentNoOverwrite")}</p>}
    {!!summary.steps.length&&<><h4>{t("imageSteps")}</h4><ol>{summary.steps.map((step,i)=><li key={i}>{step}</li>)}</ol></>}
    {summary.markdown&&<details><summary>{t("documentContent")}</summary><pre>{summary.markdown}</pre></details>}
    <details><summary>{t("diagnostics")}</summary><pre>{JSON.stringify(args,null,2)}</pre></details>
  </div>;
}
