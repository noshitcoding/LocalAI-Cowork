import {readDownload,type Client,type DownloadedFile} from "@cowork/client";
import {productRequest,type ToolJob,type ToolRequest} from "./productApi";
export type ToolboxAvailability={programs:string[],browser_available:boolean,office_available:boolean,github_available:boolean,mode:string,documents_available?:boolean,latex_available?:boolean,office_convert_available?:boolean,document_status?:string,latex_status?:string,office_convert_status?:string};
export type ToolboxOutcome={termination:string,exit_code:number|null,stdout:{bytes:number[],truncated:boolean,invalid_utf8?:boolean},stderr:{bytes:number[],truncated:boolean,invalid_utf8?:boolean},output_complete:boolean};
export async function downloadToolboxFile(client:Client,order:string,path:string,sha256?:string):Promise<DownloadedFile>{
  const query=new URLSearchParams({path,...(sha256?{expected_sha256:sha256}:{})});
  return readDownload(await fetch(client.base+`/api/v2/orders/${encodeURIComponent(order)}/toolbox/files?${query}`,{headers:{Authorization:"Bearer "+client.token},cache:"no-store"}),sha256);
}
export function toolboxOutcome(job:ToolJob|null):ToolboxOutcome|null{
  const value=job?.outcome;if(!value||typeof value!=="object")return null;
  const outcome=value as ToolboxOutcome;
  return outcome.stdout&&Array.isArray(outcome.stdout.bytes)&&outcome.stderr&&Array.isArray(outcome.stderr.bytes)?outcome:null;
}
export function toolboxCapture(capture:ToolboxOutcome["stdout"]|undefined){return capture?new TextDecoder("utf-8").decode(new Uint8Array(capture.bytes)):"";}
export function successfulToolboxResult(job:ToolJob|null):unknown{
  const outcome=toolboxOutcome(job);
  if(job?.state!=="completed"||!outcome||outcome.termination!=="exited"||outcome.exit_code!==0||!outcome.output_complete||outcome.stdout.truncated||outcome.stdout.invalid_utf8)return undefined;
  try{return JSON.parse(toolboxCapture(outcome.stdout));}catch{return undefined;}
}
export function newToolboxRequest(action:ToolRequest["action"]):ToolRequest{return {idempotency_key:crypto.randomUUID(),action,timeout_seconds:120,output_bytes:1_048_576};}
export const toolboxAvailability=(client:Client)=>productRequest<ToolboxAvailability>(client,"/toolbox");
