import type {KnowledgeProvenance, WriteKnowledge,KnowledgeImportRequest,KnowledgeImportResult,Client} from "@cowork/client";
import {decode} from "@cowork/client";
import {PersistentRequestLatch,requestJournalEndpoint,type RequestJournalStorage} from "./requestJournal";
import type {Knowledge} from "./productApi";

export type KnowledgeImportSource={name:string,text:string,sha256:string,original_sha256?:string,extraction_method?:string};
export type KnowledgeImportChunk={source:string,index:number,total:number,start:number,end:number,firstLine:number,lastLine:number,document:KnowledgeImportDocument};
export type KnowledgeImportDocument=WriteKnowledge&{id:null,expected_revision:0,provenance:KnowledgeProvenance,tags:string[]};
export type {KnowledgeImportRequest,KnowledgeImportResult};
const encoder=new TextEncoder();
export const knowledgeImportLimits={sources:20,bytes:1_048_576,chunks:200,chunkCharacters:4000} as const;
export function knowledgeImportLatch(client:Client,project:string,storage?:RequestJournalStorage){return new PersistentRequestLatch<KnowledgeImportRequest>("cowork-v2:knowledge-import:v1:"+encodeURIComponent(JSON.stringify([requestJournalEndpoint(client.base),project])),value=>{const parsed=decode("KnowledgeImportRequest",value);if(!parsed.documents.length||parsed.documents.length>200||parsed.documents.some(document=>document.id!==null||document.expected_revision!==0||document.provenance?.origin!=="imported"||!document.provenance.sources.length))throw Error("request_journal_invalid");return parsed;},storage);}
const byteLength=(text:string)=>encoder.encode(text).byteLength;
export function boundedKnowledgeText(text:string,bytes:number){let result="",used=0;for(const character of text){const size=byteLength(character);if(used+size>bytes)break;result+=character;used+=size;}return result;}
export async function knowledgeImportSource(name:string,bytes:Uint8Array):Promise<KnowledgeImportSource>{
  if(bytes.length>knowledgeImportLimits.bytes)throw new Error("knowledge_import_size_limit");
  const text=new TextDecoder("utf-8",{fatal:true}).decode(bytes);
  if(!text.trim()||text.includes("\0"))throw new Error("knowledge_import_text_required");
  const sha256=Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256",bytes as Uint8Array<ArrayBuffer>))).map(byte=>byte.toString(16).padStart(2,"0")).join("");
  return {name:boundedKnowledgeText(name.replace(/[\u0000-\u001f\u007f]/g," ").trim()||"Text",200),text,sha256};
}
export const binaryKnowledgeFormat=(name:string)=>/\.(pdf|docx|xlsx|pptx)$/i.exec(name)?.[1].toLowerCase();
export async function extractKnowledgeSource(client:Client,project:string,name:string,bytes:Uint8Array<ArrayBuffer>,operationId:string):Promise<KnowledgeImportSource>{
  if(!binaryKnowledgeFormat(name)||!bytes.length||bytes.length>16*1024*1024)throw Error("knowledge_extraction_file_limit");
  const digest=Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256",bytes))).map(byte=>byte.toString(16).padStart(2,"0")).join("");
  const transportName="source."+binaryKnowledgeFormat(name),query=new URLSearchParams({file_name:transportName,operation_id:operationId});
  let response:Response;try{response=await fetch(client.base+`/api/v2/projects/${encodeURIComponent(project)}/knowledge/extract?`+query,{method:"POST",headers:{Authorization:"Bearer "+client.token,"Content-Type":"application/octet-stream"},body:new Blob([bytes]),cache:"no-store"});}catch{throw Error("knowledge_extraction_failed");}
  if(!response.ok)throw Error("knowledge_extraction_failed");
  const value=await response.json() as {format:string,source_sha256:string,text_sha256:string,text:string,method:string};
  if(!value||value.format!==binaryKnowledgeFormat(name)||value.source_sha256!==digest||typeof value.text!=="string"||new TextEncoder().encode(value.text).length>400000||value.method!=="bounded-office-text/1")throw Error("knowledge_extraction_integrity");
  const extracted=await knowledgeImportSource(name,new TextEncoder().encode(value.text));if(extracted.sha256!==value.text_sha256)throw Error("knowledge_extraction_integrity");return {...extracted,original_sha256:digest,extraction_method:value.method};
}
export function knowledgeImportTags(text:string){
  const tags=text.split(",").map(value=>value.trim()).filter(Boolean);
  if(tags.length>32||tags.some(tag=>byteLength(tag)>80||tag.includes("\0"))||new Set(tags.map(tag=>tag.toLowerCase())).size!==tags.length)throw new Error("invalid_knowledge_tags");
  return tags;
}
/** Preview and persistence use these exact chunks, including all original whitespace and Unicode. */
export function previewKnowledgeImport(sources:KnowledgeImportSource[],options:{kind:Knowledge["kind"],tags:string[],chunkCharacters:number}):KnowledgeImportChunk[]{
  const size=options.chunkCharacters;
  if(!Number.isInteger(size)||size<500||size>knowledgeImportLimits.chunkCharacters)throw new Error("invalid_knowledge_chunk_size");
  if(!sources.length||sources.length>knowledgeImportLimits.sources)throw new Error("knowledge_import_source_limit");
  if(sources.reduce((sum,source)=>sum+byteLength(source.text),0)>knowledgeImportLimits.bytes)throw new Error("knowledge_import_size_limit");
  const result:KnowledgeImportChunk[]=[];
  for(const source of sources){
    if(!/^[0-9a-f]{64}$/.test(source.sha256)||!source.text.trim()||source.text.includes("\0"))throw new Error("knowledge_import_text_required");
    const characters=Array.from(source.text),parts:{content:string,start:number,end:number,firstLine:number,lastLine:number}[]=[];
    let start=0,line=1,lastNonBlank=characters.length-1;
    while(lastNonBlank>=0&&!characters[lastNonBlank].trim())lastNonBlank--;
    while(start<characters.length){
      let end=Math.min(start+size,characters.length);
      if(end<characters.length){
        const minimum=start+Math.floor(size*.65);
        for(let index=end-1;index>=minimum;index--)if(characters[index]==="\n"){end=index+1;break;}
        // Keep at least one nonblank character in a final whitespace tail.
        if(end>lastNonBlank){
          if(characters.length-lastNonBlank>size)throw new Error("knowledge_import_blank_chunk");
          end=lastNonBlank;
        }
      }
      const content=characters.slice(start,end).join("");
      if(!content.trim())throw new Error("knowledge_import_blank_chunk");
      parts.push({content,start,end,firstLine:line,lastLine:line+(content.match(/\n/g)?.length??0)});
      line+=content.match(/\n/g)?.length??0;start=end;
    }
    for(const [index,part] of parts.entries()){
      const title=boundedKnowledgeText(source.name,260)+(parts.length>1?` · ${index+1}/${parts.length}`:"");
      result.push({source:source.name,index:index+1,total:parts.length,start:part.start,end:part.end,firstLine:part.firstLine,lastLine:part.lastLine,
        document:{id:null,expected_revision:0,kind:options.kind,title,content:part.content,tags:options.tags,
          provenance:{format:1,origin:"imported",sources:[{format:1,source_uri:`urn:cowork:knowledge-import:${source.sha256}`,revision:`sha256:${source.sha256}`,sha256:source.sha256,locator:`${source.name}; characters ${part.start+1}-${part.end}; lines ${part.firstLine}-${part.lastLine}`,verification:"declared"},...(source.original_sha256?[{format:1 as const,source_uri:`urn:cowork:knowledge-file:${source.original_sha256}`,revision:`sha256:${source.original_sha256}`,sha256:source.original_sha256,locator:`${source.name}; extracted text; ${source.extraction_method??"unknown"}; original binary SHA256`,verification:"declared" as const}]:[])]}}});
      if(result.length>knowledgeImportLimits.chunks)throw new Error("knowledge_import_chunk_limit");
    }
  }
  return result;
}
