import {decode,readDownload,RequestError,verifyWorkspaceTransferManifest,type Client,type ContractTypes,type DownloadedFile} from "./index";

const shaPattern=/^[a-f0-9]{64}$/;
const scope=(order:string,run:string)=>`/orders/${encodeURIComponent(order)}/runs/${encodeURIComponent(run)}`;
const integer=(value:number,maximum:number)=>Number.isSafeInteger(value)&&value>=0&&value<=maximum;
const invalid=()=>new Error("managed_result_integrity");

function entry(value:unknown,order:string,run:string,attempt?:string):ContractTypes["OwnerManagedResultEntry"] {
  const result=decode("OwnerManagedResultEntry",value),version=result.version;
  if(result.order_id!==order||result.run_id!==run||!result.process_attempt_id||result.process_attempt_id.length>128
    ||attempt!==undefined&&result.process_attempt_id!==attempt)throw invalid();
  if(version&&(!version.id||version.id.length>128||!integer(version.revision,Number.MAX_SAFE_INTEGER)||version.revision===0
    ||!integer(version.created_at,8_640_000_000_000_000)||version.created_at===0||!shaPattern.test(version.manifest_sha256)
    ||!integer(version.file_count,10_000)||!integer(version.total_bytes,512*1024*1024)))throw invalid();
  return result;
}
function portablePath(path:string):boolean {
  const encoder=new TextEncoder();
  if(!path||encoder.encode(path).length>1024||path.split("/").length>65)return false;
  return path.split("/").every(part=>{
    const base=part.split(".")[0].toUpperCase();
    return !!part&&part!=="."&&part!==".."&&part.length<=255&&encoder.encode(part).length<=255
      &&!/[. ]$/.test(part)&&!part.toLowerCase().startsWith(".cowork-")&&!/[\\:<>"|?*\p{Cc}]/u.test(part)
      &&!/^(CON|PRN|AUX|NUL|CONIN\$|CONOUT\$|(?:COM|LPT)[1-9¹²³])$/.test(base);
  });
}
async function localManifest(value:unknown,order:string,run:string,versionId:string):Promise<ContractTypes["OwnerManagedResultManifest"]> {
  const result=decode("OwnerManagedResultManifest",value),bound=entry(result.entry,order,run),version=bound.version;
  if(!version||version.state!=="ready"||version.id!==versionId)throw invalid();
  const manifest=result.manifest;
  if(manifest.files.length+manifest.directories.length>10_000||!integer(manifest.total_bytes,512*1024*1024))throw invalid();
  if(version.manifest_sha256!==manifest.sha256||version.file_count!==manifest.files.length||version.total_bytes!==manifest.total_bytes)throw invalid();
  const names=new Set<string>(),directories=new Set<string>();
  const parentPresent=(path:string)=>!path.includes("/")||directories.has(path.slice(0,path.lastIndexOf("/")));
  for(const path of manifest.directories) {
    const alias=path.toLowerCase().toUpperCase();
    if(!portablePath(path)||names.has(alias)||!parentPresent(path))throw invalid();
    names.add(alias);directories.add(path);
  }
  let total=0;
  for(const file of manifest.files) {
    const alias=file.path.toLowerCase().toUpperCase();
    if(!portablePath(file.path)||names.has(alias)||!parentPresent(file.path)||!integer(file.size,64*1024*1024)||!shaPattern.test(file.sha256)
      ||file.chunks.length!==Math.ceil(file.size/(4*1024*1024)))throw invalid();
    names.add(alias);
    let offset=0;
    for(const chunk of file.chunks) {
      if(chunk.offset!==offset||chunk.size!==Math.min(4*1024*1024,file.size-offset)||!shaPattern.test(chunk.sha256))throw invalid();
      offset+=chunk.size;
    }
    total+=file.size;
    if(!integer(total,512*1024*1024))throw invalid();
  }
  if(total!==manifest.total_bytes)throw invalid();
  await verifyWorkspaceTransferManifest(manifest);
  return result;
}

export async function ownerManagedResults(client:Client,order:string,run:string):Promise<ContractTypes["OwnerManagedResultList"]> {
  const result=await client.one(`${scope(order,run)}/managed-results`,"OwnerManagedResultList");
  if(result.entries.length>128)throw invalid();
  const attempts=new Set<string>(),versions=new Set<string>();
  for(const value of result.entries) {
    const current=entry(value,order,run);
    if(attempts.has(current.process_attempt_id)||current.version&&versions.has(current.version.id))throw invalid();
    attempts.add(current.process_attempt_id);if(current.version)versions.add(current.version.id);
  }
  return result;
}
export async function receiveOwnerManagedResult(client:Client,order:string,run:string,attempt:string):Promise<ContractTypes["OwnerManagedResultEntry"]> {
  // A failed/lost POST is never automatically retried. A later GET may inspect
  // the original durable receive intent before the user chooses to continue.
  const result=entry(await client.one(`${scope(order,run)}/managed-results/${encodeURIComponent(attempt)}/receive`,"OwnerManagedResultEntry","POST"),order,run,attempt);
  if(result.version?.state!=="ready")throw new Error("managed_result_receive_unconfirmed");
  return result;
}
export async function ownerManagedResultManifest(client:Client,order:string,run:string,version:string,signal?:AbortSignal):Promise<ContractTypes["OwnerManagedResultManifest"]> {
  signal?.throwIfAborted();
  const response=await fetch(client.base+`/api/v2${scope(order,run)}/managed-result-versions/${encodeURIComponent(version)}/manifest`,{method:"GET",headers:{Authorization:"Bearer "+client.token},cache:"no-store",signal});
  const value:unknown=await response.json();
  signal?.throwIfAborted();
  if(!response.ok)throw new RequestError(decode("ApiError",value),response.status);
  const result=await localManifest(value,order,run,version);
  signal?.throwIfAborted();
  return result;
}
export async function downloadOwnerManagedResultFile(client:Client,order:string,run:string,selected:ContractTypes["OwnerManagedResultManifest"],path:string,signal?:AbortSignal):Promise<DownloadedFile> {
  signal?.throwIfAborted();
  const originalId=selected.entry.version?.id;
  if(!originalId)throw invalid();
  const original=await localManifest(structuredClone(selected),order,run,originalId);
  signal?.throwIfAborted();
  const current=await ownerManagedResultManifest(client,order,run,originalId,signal);
  if(current.entry.process_attempt_id!==original.entry.process_attempt_id||current.entry.version?.revision!==original.entry.version?.revision
    ||current.manifest.sha256!==original.manifest.sha256)throw new Error("managed_result_changed");
  const file=current.manifest.files.find(file=>file.path===path);
  if(!file)throw invalid();
  signal?.throwIfAborted();
  const bytes=new Uint8Array(file.size);
  for(const chunk of file.chunks) {
    signal?.throwIfAborted();
    const query=new URLSearchParams({path,offset:String(chunk.offset),expected_sha256:chunk.sha256});
    const response=await fetch(client.base+`/api/v2${scope(order,run)}/managed-result-versions/${encodeURIComponent(originalId)}/chunk?${query}`,{headers:{Authorization:"Bearer "+client.token},cache:"no-store",signal});
    if(response.ok&&response.headers.get("content-length")!==String(chunk.size)) {
      await response.body?.cancel();throw new Error("download_integrity");
    }
    const received=await readDownload(response,chunk.sha256);
    if(received.size!==chunk.size)throw new Error("download_integrity");
    bytes.set(new Uint8Array(await received.blob.arrayBuffer()),chunk.offset);
  }
  signal?.throwIfAborted();
  const digest=Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256",bytes))).map(b=>b.toString(16).padStart(2,"0")).join("");
  signal?.throwIfAborted();
  if(digest!==file.sha256)throw new Error("download_integrity");
  return {...decode("DownloadMetadata",{file_name:file.path.slice(file.path.lastIndexOf("/")+1),size:file.size,sha256:digest}),blob:new Blob([bytes],{type:"application/octet-stream"})};
}
