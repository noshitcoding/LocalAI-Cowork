import {decode,RequestError,type DownloadMetadata,type NativeSaveRequest,type UiLanguage} from "@cowork/client";

export type DeliveryStatus="saved"|"canceled"|"browser";
export const deliveryStatusKey=(status:DeliveryStatus)=>status==="saved"?"fileSaved":status==="canceled"?"saveCanceled":"downloadPrepared";

export function safeFileName(value:string):string {
  let name=value.replace(/[\\/:*?"<>|\u0000-\u001f\u007f\u202a-\u202e\u2066-\u2069]/g,"_").trim().replace(/[. ]+$/g,"");
  if(!name||/^(con|prn|aux|nul|conin\$|conout\$|com[0-9¹²³]|lpt[0-9¹²³])(?:\.|$)/i.test(name)||name.toLowerCase().startsWith(".cowork-"))name="download_"+name;
  let bounded="";const encoder=new TextEncoder();
  for(const character of name){if(encoder.encode(bounded+character).length>240)break;bounded+=character;}
  return bounded.replace(/[. ]+$/g,"")||"download";
}

function uncertain():RequestError {
  return new RequestError({code:"save_uncertain",message:"The native save result could not be confirmed. Inspect the selected destination before retrying.",retryable:false,correlation_id:crypto.randomUUID()},0);
}

/** Browser handoff is distinct from a verified native save or a canceled native picker. */
export async function deliverDownload(blob:Blob,fileName:string,language:UiLanguage,expected?:Pick<DownloadMetadata,"sha256"|"size">,signal?:AbortSignal):Promise<DeliveryStatus> {
  signal?.throwIfAborted();
  if(blob.size>67_108_864||expected&&blob.size!==expected.size)throw new Error("download_integrity");
  const bytes=await blob.arrayBuffer();
  signal?.throwIfAborted();
  const sha256=Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256",bytes))).map(b=>b.toString(16).padStart(2,"0")).join("");
  signal?.throwIfAborted();
  if(expected&&sha256!==expected.sha256)throw new Error("download_integrity");
  const file=decode("DownloadMetadata",{file_name:safeFileName(fileName),size:blob.size,sha256});
  if("__TAURI_INTERNALS__" in window){
    const {invoke}=await import("@tauri-apps/api/core");
    signal?.throwIfAborted();
    const request:NativeSaveRequest={file,language};
    let result:unknown;
    // After native dispatch, preserve its original outcome even if the view
    // goes away. An abort cannot turn a possibly completed save into a retry.
    try {result=await invoke("save_file",new Uint8Array(bytes),{headers:{"x-cowork-save":encodeURIComponent(JSON.stringify(request))}});}
    catch(error){let details;try{details=decode("ApiError",error);}catch{throw uncertain();}throw new RequestError(details,0);}
    let outcome;
    try {outcome=decode("NativeSaveOutcome",result);}catch{throw uncertain();}
    if(outcome.status==="canceled")return "canceled";
    if(outcome.file.sha256!==sha256||outcome.file.size!==blob.size)throw uncertain();
    return "saved";
  }
  signal?.throwIfAborted();
  const url=URL.createObjectURL(blob);
  const anchor=document.createElement("a");
  anchor.href=url;anchor.download=file.file_name;anchor.hidden=true;
  const previous=document.activeElement;
  document.body.append(anchor);
  try {anchor.click();} finally {
    anchor.remove();
    if(previous instanceof HTMLElement&&previous.isConnected)previous.focus({preventScroll:true});
    setTimeout(()=>URL.revokeObjectURL(url),60_000);
  }
  return "browser";
}
