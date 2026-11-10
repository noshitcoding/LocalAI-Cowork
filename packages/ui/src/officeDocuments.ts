import {readDownload,type Client,type DownloadedFile} from "@cowork/client";
import {productRequest} from "./productApi";
export type DocumentVersion={id:string,document_id:string,revision:number,path:string,file_name:string,sha256:string,size:number,created_at:number,operation:string,job_id:string,parent_sha256:string|null};
export type OfficeItem={paragraph?:number,text?:string,truncated?:boolean,sheet?:string,cell?:string,value?:string|number|boolean|null,type?:string,number_format?:string,slide?:number,shape?:number,name?:string};
export type OfficeModel={format:"docx"|"xlsx"|"pptx",source_sha256:string,input:string,items:OfficeItem[],total:number,offset:number,next_offset:number|null};
export const officeFormat=(path:string)=>/\.(docx|xlsx|pptx)$/i.exec(path)?.[1].toLowerCase() as OfficeModel["format"]|undefined;
export const documentVersions=(client:Client,order:string,path:string)=>productRequest<DocumentVersion[]>(client,`/orders/${encodeURIComponent(order)}/toolbox/document-versions?`+new URLSearchParams({path}));
export async function downloadDocumentVersion(client:Client,order:string,version:Pick<DocumentVersion,"id"|"sha256">):Promise<DownloadedFile>{return readDownload(await fetchResponse(client,order,version.id),version.sha256);}
async function fetchResponse(client:Client,order:string,id:string){return fetch(client.base+`/api/v2/orders/${encodeURIComponent(order)}/toolbox/document-versions/${encodeURIComponent(id)}`,{headers:{Authorization:"Bearer "+client.token},cache:"no-store"});}
export function officePatchAction(model:OfficeModel,patches:Record<string,unknown>[],output:string){
  if(!/^[0-9a-f]{64}$/.test(model.source_sha256)||!patches.length||patches.length>5000||officeFormat(output)!==model.format)throw new Error("invalid_office_patch");
  return {kind:"office",operation:"edit_"+model.format,input:model.input,output,text:JSON.stringify({source_sha256:model.source_sha256,patches})};
}
