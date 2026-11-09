import type {DownloadedFile,FileEntry,WorkflowDefinition,WorkflowValueBinding} from "@cowork/client";
import {dataFieldName} from "./workflowDataflow";

export const documentSourceByteLimit=32*1024*1024;
export type DocumentSourceBinding={step:string;name:string;pathField:string;hashField:string};

/** Only expose direct, declared text inputs for this task's own workspace. */
export function documentSourceBindings(definition:WorkflowDefinition):DocumentSourceBinding[]{
  const input=definition.dataflow?.input;
  if(input?.type!=="object"||!definition.rights.current_workspace)return [];
  const fields=input.fields;
  function field(binding:WorkflowValueBinding|undefined):string|undefined{
    if(binding?.value_type.type!=="text"||binding.value.source!=="input")return;
    const pointer=binding.value.pointer;
    if(!pointer.startsWith("/"))return;
    const name=pointer.slice(1);
    return dataFieldName(name)&&Object.hasOwn(fields,name)&&fields[name].type==="text"?name:undefined;
  }
  const found=Object.entries(definition.documents??{}).flatMap(([step,document])=>{
    if(document.folder.kind!=="current_workspace"||!document.rights.current_workspace||!["ocr","read_text","preview"].includes(document.operation.kind)||!("sha256_input" in document.operation))return [];
    const data=definition.dataflow?.steps[step],pathField=field(data?.inputs[document.operation.path_input]),hashField=field(data?.inputs[document.operation.sha256_input]);
    return pathField&&hashField&&pathField!==hashField?[{step,name:document.name,pathField,hashField}]:[];
  });
  // A path shared with a different digest input cannot be changed independently.
  return found.filter(binding=>!found.some(other=>other!==binding&&[binding.pathField,binding.hashField].some(name=>name===other.pathField||name===other.hashField)&&(binding.pathField!==other.pathField||binding.hashField!==other.hashField)));
}

export function relativeDocumentPath(path:string):boolean{
  return path.length>0&&path.length<=16384&&!/[\\:\u0000-\u001f\u007f]/.test(path)&&path.split("/").every(part=>!!part&&part!=="."&&part!=="..");
}
export function selectableDocumentFile(entry:FileEntry):boolean{
  return entry.kind==="file"&&relativeDocumentPath(entry.path)&&Number.isSafeInteger(entry.size)&&entry.size>=0&&entry.size<=documentSourceByteLimit;
}

/** One state update binds the verified path and digest; all other values survive. */
export function bindDocumentSource(value:unknown,binding:DocumentSourceBinding,path:string,file:DownloadedFile):Record<string,unknown>{
  if(!value||typeof value!=="object"||Array.isArray(value)||!dataFieldName(binding.pathField)||!dataFieldName(binding.hashField)||binding.pathField===binding.hashField||!relativeDocumentPath(path)||file.file_name!==path.split("/").at(-1)||!Number.isSafeInteger(file.size)||file.size<0||file.size>documentSourceByteLimit||file.blob.size!==file.size||!/^[a-f0-9]{64}$/.test(file.sha256))throw new Error("document_source_invalid");
  return {...value,[binding.pathField]:path,[binding.hashField]:file.sha256};
}

/** A late download may never change another workflow, connection or frozen plan. */
export class DocumentSourceSelection{
  private pending=false;
  private closed=false;
  close(){this.closed=true;}
  async select({entry,binding,download,current,isCurrent,apply}:{entry:FileEntry;binding:DocumentSourceBinding;download:(path:string)=>Promise<DownloadedFile>;current:()=>unknown;isCurrent:()=>boolean;apply:(value:Record<string,unknown>)=>void}):Promise<boolean>{
    if(this.closed||this.pending||!isCurrent())return false;
    if(!selectableDocumentFile(entry))throw new Error("document_source_invalid");
    this.pending=true;
    try{
      const file=await download(entry.path);
      if(this.closed||!isCurrent())return false;
      apply(bindDocumentSource(current(),binding,entry.path,file));
      return true;
    }finally{this.pending=false;}
  }
}
