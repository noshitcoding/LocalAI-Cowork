import {z} from 'zod'
import {snapshotManifestSchema,snapshotUploadSessionSchema} from './contractArtifactRegistry'

export type SnapshotUploadFile={path:string;size:number;mode:number;modified_at:string;chunks:{digest:string;plaintext_size:number}[]}
export type SnapshotUploadRequest={idempotency_key:string;project_id:string;total_bytes:number;files:SnapshotUploadFile[];expires_at:null}
export type SnapshotUploadSession=z.infer<typeof snapshotUploadSessionSchema>
export type SnapshotManifest=z.infer<typeof snapshotManifestSchema>
export type SnapshotUploadClient={
  beginSnapshotUpload(request:SnapshotUploadRequest,signal?:AbortSignal):Promise<SnapshotUploadSession>
  uploadSnapshotChunk(manifest:string,digest:string,bytes:Blob,signal?:AbortSignal):Promise<{warnings:string[]}>
  commitSnapshot(manifest:string,signal?:AbortSignal):Promise<SnapshotManifest>
}
const CHUNK_BYTES=4*1024*1024
export function snapshotPath(file:Pick<File,'name'|'webkitRelativePath'>):string {
  const path=file.webkitRelativePath||file.name
  if(!path||path.length>4096||/[\\\u0000-\u001f:]/.test(path)||path.startsWith('/')||path.split('/').some(part=>!part||part==='.'||part==='..'))throw Error('Select ordinary files with relative paths')
  return path
}
export function snapshotSensitivePath(path:string):boolean {
  return path.split('/').some(part=>/^(\.git|\.env(?:\..*)?|node_modules|\.ssh|\.aws|\.azure|\.kube)$/i.test(part))||/\.(pem|key|pfx|p12)$/i.test(path)
}
export function snapshotRequired(privacy:string,target:{kind:string}|undefined):boolean {
  return privacy==='private_local'&&!!target&&target.kind!=='personal_device'
}
const hex=(bytes:ArrayBuffer)=>Array.from(new Uint8Array(bytes),b=>b.toString(16).padStart(2,'0')).join('')
function checkpoint(signal?:AbortSignal){if(signal?.aborted)throw new DOMException('Snapshot upload stopped. Retry with the same selected files to reconcile its original status.','AbortError')}
export async function prepareSnapshot(project:string,files:readonly File[],signal?:AbortSignal,onProgress?:(text:string)=>void){
  const ordered=[...files].sort((a,b)=>snapshotPath(a).localeCompare(snapshotPath(b),'en'))
  if(ordered.length>10000)throw Error('A browser snapshot supports at most 10,000 files')
  const total=ordered.reduce((sum,file)=>sum+file.size,0)
  if(!Number.isSafeInteger(total)||total>20*1024*1024*1024)throw Error('Snapshot exceeds the 20 GiB limit')
  const names=new Set<string>(),chunks=new Map<string,Blob>(),manifest:SnapshotUploadFile[]=[]
  for(const file of ordered){
    checkpoint(signal);const path=snapshotPath(file),key=path.toLowerCase()
    if(names.has(key))throw Error('Selected files contain duplicate or case-conflicting paths')
    names.add(key);const parts:SnapshotUploadFile['chunks']=[]
    for(let start=0;start<file.size;start+=CHUNK_BYTES){
      checkpoint(signal);onProgress?.(`Hashing ${path} · ${Math.min(start+CHUNK_BYTES,file.size)} / ${file.size} bytes`)
      const chunk=file.slice(start,start+CHUNK_BYTES),digest=hex(await crypto.subtle.digest('SHA-256',await chunk.arrayBuffer()))
      chunks.set(digest,chunk);parts.push({digest,plaintext_size:chunk.size})
    }
    manifest.push({path,size:file.size,mode:420,modified_at:new Date(file.lastModified).toISOString(),chunks:parts})
  }
  const declaration={project_id:project,total_bytes:total,files:manifest,expires_at:null}
  const signature=hex(await crypto.subtle.digest('SHA-256',new TextEncoder().encode(JSON.stringify(declaration))))
  return {declaration,chunks,signature}
}
export async function sendSnapshot(client:SnapshotUploadClient,prepared:Awaited<ReturnType<typeof prepareSnapshot>>,key:string,signal?:AbortSignal,onProgress?:(text:string)=>void):Promise<SnapshotManifest>{
  checkpoint(signal);onProgress?.('Preparing original upload…')
  const session=await client.beginSnapshotUpload({...prepared.declaration,idempotency_key:key},signal)
  const missing=new Set(session.missing_chunks)
  if(missing.size!==session.missing_chunks.length||[...missing].some(digest=>!prepared.chunks.has(digest)))throw Error('The server returned an unexpected snapshot chunk')
  for(const [index,digest] of [...missing].entries()){
    checkpoint(signal);onProgress?.(`Uploading ${index+1} / ${missing.size} chunks`)
    const bytes=prepared.chunks.get(digest)!
    if(bytes.size>session.max_chunk_bytes)throw Error('Server chunk limit is incompatible with this browser upload')
    await client.uploadSnapshotChunk(session.manifest_id,digest,bytes,signal)
  }
  checkpoint(signal);onProgress?.('Committing immutable snapshot…')
  const manifest=await client.commitSnapshot(session.manifest_id,signal)
  if(manifest.id!==session.manifest_id||manifest.project_id!==prepared.declaration.project_id||manifest.total_bytes!==prepared.declaration.total_bytes||manifest.files.length!==prepared.declaration.files.length)throw Error('Committed snapshot identity differs from the selected files')
  for(let i=0;i<manifest.files.length;i++){
    const actual=manifest.files[i],expected=prepared.declaration.files[i]
    if(actual.path!==expected.path||actual.size!==expected.size||actual.mode!==expected.mode||Date.parse(actual.modified_at)!==Date.parse(expected.modified_at)||JSON.stringify(actual.chunks.map(c=>({digest:c.digest,plaintext_size:c.plaintext_size})))!==JSON.stringify(expected.chunks))throw Error('Committed snapshot file content differs from the declaration')
  }
  return manifest
}
