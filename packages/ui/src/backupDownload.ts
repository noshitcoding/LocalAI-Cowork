import {sha256} from "@noble/hashes/sha2.js";
import {decode,readDownload,type Client,type DatabaseBackup} from "@cowork/client";
import {nativeMcpAvailable} from "./mcpBridge";

const MAX_BACKUP_BYTES=1_099_511_627_776;
const uuid=/^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
export type BackupSink={write:(bytes:Uint8Array)=>Promise<void>;close:()=>Promise<void>;abort:()=>Promise<void>};
function checked(raw:DatabaseBackup){const backup=decode("DatabaseBackup",raw);if(!uuid.test(backup.id)||!Number.isSafeInteger(backup.bytes)||backup.bytes<1||backup.bytes>MAX_BACKUP_BYTES||!/^[0-9a-f]{64}$/.test(backup.sha256)||!["sqlite3","postgres-custom"].includes(backup.format))throw new Error("backup_metadata_invalid");return backup;}
const fileName=(backup:DatabaseBackup)=>`cowork-${backup.id}.${backup.format}`;
/** Writes only to an uncommitted user-selected file. Close is the publish point. */
export async function streamVerifiedBackup(response:Response,raw:DatabaseBackup,sink:BackupSink,progress?:(bytes:number)=>void){
 let reader:ReadableStreamDefaultReader<Uint8Array>|undefined;
 try{
  const backup=checked(raw);
  if(!response.ok||!response.body||response.headers.get("content-type")!=="application/octet-stream"||response.headers.get("content-length")!==String(backup.bytes)||response.headers.get("x-cowork-sha256")!==backup.sha256||decodeURIComponent(response.headers.get("x-cowork-file-name")??"")!==fileName(backup))throw new Error("backup_integrity_failed");
  reader=response.body.getReader();const hash=sha256.create();let size=0;
  for(;;){const next=await reader.read();if(next.done)break;size+=next.value.byteLength;if(!Number.isSafeInteger(size)||size>backup.bytes)throw new Error("backup_integrity_failed");hash.update(next.value);await sink.write(next.value);progress?.(size);}
  if(size!==backup.bytes||Array.from(hash.digest(),b=>b.toString(16).padStart(2,"0")).join("")!==backup.sha256)throw new Error("backup_integrity_failed");
  await sink.close();return;
 }catch(error){try{await reader?.cancel();}catch{}try{await sink.abort();}catch{}throw error;}finally{reader?.releaseLock();}
}
export async function saveDatabaseBackup(client:Client,raw:DatabaseBackup,language:"de"|"en",progress?:(bytes:number)=>void):Promise<"saved"|"canceled"|"download_started">{
 const backup=checked(raw);
 if(nativeMcpAvailable(client)){const{invoke}=await import("@tauri-apps/api/core");const input=decode("NativeSaveDatabaseBackup",{id:backup.id,expected_bytes:backup.bytes,expected_sha256:backup.sha256,language});const outcome=decode("DatabaseBackupSaveOutcome",await invoke("save_database_backup",{input}));if(outcome.state==="canceled")return "canceled";if(outcome.state!=="saved"||outcome.bytes!==backup.bytes||outcome.sha256!==backup.sha256)throw new Error("backup_save_receipt_mismatch");return "saved";}
 const picker=(window as Window&{showSaveFilePicker?:(options:{suggestedName:string})=>Promise<{createWritable:()=>Promise<BackupSink>}>}).showSaveFilePicker;
 let sink:BackupSink|undefined;
 if(picker){try{const selected=await picker.call(window,{suggestedName:fileName(backup)});sink=await selected.createWritable();}catch(error){if(error instanceof DOMException&&error.name==="AbortError")return "canceled";throw error;}}
 else if(backup.bytes>67_108_864)throw new Error("backup_streaming_browser_required");
 const url=client.base+"/api/v2/maintenance/backups/"+encodeURIComponent(backup.id);
 let response:Response;try{response=await fetch(url,{headers:{Authorization:"Bearer "+client.token},credentials:"omit",redirect:"error",cache:"no-store"});}catch(error){try{await sink?.abort();}catch{}throw error;}
 if(sink){await streamVerifiedBackup(response,backup,sink,progress);return "saved";}
 const file=await readDownload(response,backup.sha256);if(file.size!==backup.bytes||file.file_name!==fileName(backup))throw new Error("backup_integrity_failed");const link=URL.createObjectURL(file.blob),anchor=document.createElement("a");anchor.href=link;anchor.download=file.file_name;anchor.click();setTimeout(()=>URL.revokeObjectURL(link),60_000);return "download_started";
}
