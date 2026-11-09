import {decode,type Client,type HistoricalContentProject as CanonicalContentProject,type HistoricalContentCatalogManifest,type HistoricalContentCatalogPage,type HistoricalPreviewSummary} from "@cowork/client";
export type HistoricalStream="quarantine_manifest"|"quarantine_database"|"mapping"|"preview"|"cold_target_snapshot";
export const historicalStreams:HistoricalStream[]=["quarantine_manifest","quarantine_database","mapping","preview","cold_target_snapshot"];
export type HistoricalPins=Record<HistoricalStream|string,string>;
export type ImportLimit={installation_id:string,revision:number,limit_bytes:number,reserved_bytes:number};
export type ImportRecord={id:string,request_id:string,state:string,revision:number,declared_bytes:number,received_bytes:number,reserved_bytes:number,files:{stream:HistoricalStream,size:number,sha256:string}[],pins:HistoricalPins,source_profile:string|null,original_rows:number|null,preview_format?:string};
export type ContentProject={source_id:string,name:string,orders:{source_id:string,source_kind:string,title:string,draft:string,messages:unknown[],artifacts:unknown[]}[],documents:{source_id:string,title:string,content:string}[],files:{source_id:string,name:string,size:number|null,sha256:string|null,reason:string}[]};
export type CatalogEntry={source:ContentProject,pages:{stream:string,chunk_index:number,size:number,sha256:string}[],content_bytes:number,unsupported_reason:string|null,ordinal?:number,activation_receipt_sha256?:string,editable_selection_closed?:boolean};
export type ProjectTarget={kind:"new_personal"}|{kind:"existing_personal",project_id:string,expected_revision:number}|{kind:"existing_organization",project_id:string,expected_revision:number,expected_profile_revision:number}|{kind:"history_only",reason:string};
export type PreviewInput={request_id:string,expected_import_revision:number,expected_limit_revision:number,projects:{source_project_id:string,target:ProjectTarget}[],history_only_file_ids:string[],catalog_sha256?:string,catalog_count?:number,remainder_history_only_reason?:string};
export type ImportPreview={id:string,import_id:string,plan_sha256:string,state:string,source_profile:string,original_rows:number,projected_bytes:number,limit:ImportLimit,history_only_file_ids:string[],catalog_sha256?:string|null,catalog_count?:number|null,remainder_history_only_reason?:string|null,projects:{source:ContentProject,project_id:string,target:ProjectTarget,order_ids:string[],document_ids:string[],file_ids:string[],unsupported_reason:string|null}[]};
export type ActivationReceipt={preview_id:string,import_id:string,plan_sha256:string,source_sha256:string,activated_at:number,charged_bytes:number,objects:{source_id:string,kind:string,id:string,project_id:string}[]};
export type BeginInput={request_id:string,expected_limit_revision:number,pins:HistoricalPins,files:{stream:HistoricalStream,size:number,sha256:string}[],preview_format?:string};
export async function historicalPreviewFormat(file:File,pin:string):Promise<string>{
 if(file.size===0||file.size>256*1024*1024)throw new Error("historical_preview_format_invalid");
 const bytes=await file.arrayBuffer();
 const digest=Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256",bytes))).map(b=>b.toString(16).padStart(2,"0")).join("");
 if(digest!==pin)throw new Error("original_pins_required");
 const value:unknown=JSON.parse(new TextDecoder("utf-8",{fatal:true}).decode(bytes));
 if(!value||typeof value!=="object"||!("format" in value)||!(["localai-cowork-quarantine-preview/1","localai-cowork-quarantine-preview/2"] as unknown[]).includes(value.format))throw new Error("historical_preview_format_invalid");
 return String(value.format);
}
export type FileOperation={preview_id:string,import_id:string,plan_sha256:string,source_id:string,target_id:string,project_id:string,order_id:string,name:string,media_type:string,backend_id:string,size:number,sha256:string,request_sha256:string,state:string,created_at:number};
export type UnsentFileReceipt={format:string,original_operation:FileOperation,released_bytes:number,retained_reserved_bytes:number,original_charge_id:string|null,closed_at:number,closure_sha256:string};
export interface HistoricalImportApi {
  native:boolean;scope:string;
  json<T>(path:string,method?:string,input?:unknown):Promise<T>;
  upload<T>(path:string,bytes:Blob):Promise<T>;
  binary(path:string):Promise<ArrayBuffer>;
  selectBundle?(requestId:string):Promise<ImportRecord|null>;
  stageNativeFile?(preview:string,source:string):Promise<FileOperation|null>;
}
export function organizationHistoricalImport(client:Client,session:string,actor:string,organization:string,project:string):HistoricalImportApi {
 const prefix=`/api/v2/historical-import/${encodeURIComponent(organization)}/${encodeURIComponent(project)}`;
 async function request(path:string,method:string,body?:BodyInit,contentType?:string){
  const response=await fetch(client.base+prefix+path,{method,headers:{Authorization:`Bearer ${session}`,...(contentType?{"Content-Type":contentType}:{})},body,cache:"no-store"});
  if(!response.ok){let error="historical_import_failed";try{const value=await response.json();error=String(value.code??value.error??error);}catch{}throw new Error(`${error} (${response.status})`);}return response;
 }
 return {native:false,scope:JSON.stringify([client.base,"organization",actor,organization,project]),
  json:async<T>(path:string,method="GET",body?:unknown)=>(await request(path,method,body===undefined?undefined:JSON.stringify(body),body===undefined?undefined:"application/json")).json() as Promise<T>,
  upload:async<T>(path:string,bytes:Blob)=>(await request(path,"POST",bytes,"application/octet-stream")).json() as Promise<T>,
  binary:async(path:string)=>(await request(path,"GET")).arrayBuffer(),
 };
}
/** Native owner headers remain inside IPC. Renderer never receives their values. */
export function nativeHistoricalImport(client:Client,language:"de"|"en"="de"):HistoricalImportApi {
 const invoke=async<T>(command:string,args:Record<string,unknown>)=>{const api=await import("@tauri-apps/api/core");return api.invoke<T>(command,args);};
 return {native:true,scope:JSON.stringify([client.base,"personal-native"]),
  json:<T>(path:string,method="GET",input?:unknown)=>invoke<T>("historical_import_request",{path,method,input:input??null,language}),
  upload:async<T>(path:string,bytes:Blob)=>invoke<T>("historical_import_upload_chunk",{path,bytes:Array.from(new Uint8Array(await bytes.arrayBuffer()))}),
  binary:async(path:string)=>(path.includes("/history/")||path.includes("/content/catalog/")||path.startsWith("/previews/"))?Uint8Array.from(atob(await invoke<string>("historical_import_read_history_bytes",{path})),c=>c.charCodeAt(0)).buffer:new Uint8Array(await invoke<number[]>("historical_import_read_page",{path})).buffer,
  selectBundle:(requestId:string)=>invoke<ImportRecord|null>("historical_import_select",{requestId,language}),
  stageNativeFile:(preview:string,source:string)=>invoke<FileOperation|null>("historical_import_stage_file",{preview,source,language}),
 };
}
async function originalJson<T>(api:HistoricalImportApi,path:(page:number)=>string,pins:{size:number,sha256:string}[],maximum:number):Promise<T>{
 const size=pins.reduce((n,p)=>n+p.size,0);if(size>maximum||pins.some(p=>p.size>1024*1024))throw new Error("historical_content_integrity");
 const all=new Uint8Array(size);let offset=0;
 for(let page=0;page<pins.length;page++){const bytes=new Uint8Array(await api.binary(path(page)));const sha=Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256",bytes))).map(b=>b.toString(16).padStart(2,"0")).join("");if(bytes.length!==pins[page].size||sha!==pins[page].sha256)throw new Error("historical_content_integrity");all.set(bytes,offset);offset+=bytes.length;}
 if(offset!==size)throw new Error("historical_content_integrity");return JSON.parse(new TextDecoder("utf-8",{fatal:true}).decode(all)) as T;
}
export type CatalogSlice={manifest:HistoricalContentCatalogManifest,offset:number,next_offset:number|null,entries:CatalogEntry[],editable_selection_closed:boolean};
const contentProject=(value:CanonicalContentProject):ContentProject=>({...value,files:value.files.map(f=>({...f,size:f.size??null,sha256:f.sha256??null}))});
/** One bounded page, with an explicit continuation; never a full-catalogue loop. */
export async function historicalCatalog(api:HistoricalImportApi,id:string,offset=0,limit=5):Promise<CatalogSlice>{
 const page:HistoricalContentCatalogPage=decode("HistoricalContentCatalogPage",await api.json<unknown>(`/imports/${id}/content/catalog?offset=${offset}&limit=${limit}`));
 if(page.offset!==offset||page.entries.length>limit||page.entries.some((v,i)=>v.ordinal!==offset+i))throw new Error("historical_content_integrity");
 const activated=page.activated_parts??{},closed=page.editable_selection_closed===true;
 if(Object.entries(activated).some(([source,sha])=>!page.entries.some(item=>item.source_id===source)||!/^[0-9a-f]{64}$/.test(sha)))throw new Error("historical_content_integrity");
 const entries:CatalogEntry[]=[];
 for(const item of page.entries){const value=decode("HistoricalContentCatalogEntry",await originalJson<unknown>(api,index=>`/imports/${id}/content/catalog/${item.ordinal}/${index}`,item.metadata_pages,16*1024*1024));if(value.source.source_id!==item.source_id||value.source.name!==item.name||value.source.orders.length!==item.orders||value.source.documents.length!==item.documents||value.source.files.length!==item.files||value.content_bytes!==item.content_bytes)throw new Error("historical_content_integrity");entries.push({...value,source:contentProject(value.source),unsupported_reason:value.unsupported_reason??null,ordinal:item.ordinal,editable_selection_closed:closed,...(Object.prototype.hasOwnProperty.call(activated,item.source_id)?{activation_receipt_sha256:activated[item.source_id]}:{})});}
 return {...page,next_offset:page.next_offset??null,entries,editable_selection_closed:closed};
}
/** Versioned bounded transport preserves the exact existing immutable plan. */
export async function historicalPreview(api:HistoricalImportApi,preview:string,create?:{import_id:string,input:PreviewInput}):Promise<ImportPreview>{
 const summary:HistoricalPreviewSummary=decode("HistoricalPreviewSummary",await api.json<unknown>(create?`/imports/${create.import_id}/preview/paged`:`/previews/${preview}/summary`,create?"POST":"GET",create?.input));
 if(!create&&summary.id!==preview||summary.format!=="localai-cowork-current-preview-summary/1")throw new Error("historical_content_integrity");
 const projects:ImportPreview["projects"]=[];let bytes=0;
 for(const item of summary.projects){bytes+=item.pages.reduce((n,p)=>n+p.size,0);if(bytes>256*1024*1024)throw new Error("historical_content_integrity");const value=decode("HistoricalPreparedProject",await originalJson<unknown>(api,page=>`/previews/${summary.id}/projects/${item.ordinal}/${page}`,item.pages,16*1024*1024));if(value.source.source_id!==item.source_id||value.project_id!==item.project_id||JSON.stringify(value.target)!==JSON.stringify(item.target))throw new Error("historical_content_integrity");projects.push({...value,source:contentProject(value.source),unsupported_reason:value.unsupported_reason??null});}
 return {...summary,projects};
}
export async function historicalProjectBody(api:HistoricalImportApi,id:string,index:number,entry:CatalogEntry):Promise<ContentProject>{
 const pages:Uint8Array[]=[];for(let i=0;i<entry.pages.length;i++){const bytes=new Uint8Array(await api.binary(`/imports/${id}/content/${index}/${i}`));const digest=Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256",bytes))).map(b=>b.toString(16).padStart(2,"0")).join("");if(bytes.length!==entry.pages[i].size||digest!==entry.pages[i].sha256)throw new Error("historical_content_integrity");pages.push(bytes);}
 const length=pages.reduce((n,p)=>n+p.length,0);if(length!==entry.content_bytes||length>256*1024*1024)throw new Error("historical_content_integrity");const bytes=new Uint8Array(length);let offset=0;for(const page of pages){bytes.set(page,offset);offset+=page.length;}const value=JSON.parse(new TextDecoder("utf-8",{fatal:true}).decode(bytes)) as ContentProject;if(value.source_id!==entry.source.source_id)throw new Error("historical_content_integrity");return value;
}
