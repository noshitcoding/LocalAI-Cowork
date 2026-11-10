import type {Budgets,WorkflowDefinition,WorkflowDocumentOperation,WorkflowDocumentStep,WorkflowValueBinding} from "@cowork/client";
import {dataFieldName,defaultDataValue,sameDataType,workflowDataNodes} from "./workflowDataflow";
import {documentInputs,documentInputTypes,documentOutputType,documentTool} from "./workflowDocumentTypes";
import {validTableRows,validTableSchema} from "./workflowTables";

export function validWorkflowDocuments(value:WorkflowDefinition):boolean {
  const documents=Object.entries(value.documents??{});
  if(!documents.length)return true;
  if(![3,4,5].includes(value.format_version)||value.mode.mode!=="parallel"||!value.dataflow)return false;
  return documents.every(([id,step])=>{
    const operation=step.operation,inputs=documentInputs(operation),config=value.dataflow!.steps[id];
    if(!dataFieldName(id)||id.startsWith("delegated-")||!step.name.trim()||step.name.length>200||step.name.includes("\0")
      ||new Set(step.dependencies).size!==step.dependencies.length||!config||new Set(inputs).size!==inputs.length
      ||!inputs.every(dataFieldName)||Object.keys(config.inputs).length!==inputs.length
      ||!Object.entries(documentInputTypes(operation)).every(([name,kind])=>Object.hasOwn(config.inputs,name)&&sameDataType(config.inputs[name].value_type,kind))
      ||!sameDataType(config.output,documentOutputType(operation)))return false;
    const tool=documentTool(operation);
    if(!step.rights.tools.includes(tool)||!value.rights.tools.includes(tool))return false;
    if(step.folder.kind==="current_workspace"){
      if(!step.rights.current_workspace||!value.rights.current_workspace)return false;
    }else if(!step.folder.folder_id||!step.rights.folders.includes(step.folder.folder_id)||!value.rights.folders.includes(step.folder.folder_id))return false;
    if(operation.kind==="read_text"&&(!Number.isSafeInteger(operation.offset)||operation.offset<0||operation.offset>1_000_000_000))return false;
    if((operation.kind==="preview"||operation.kind==="ocr")&&(operation.pages.length>16||new Set(operation.pages).size!==operation.pages.length||operation.pages.some(page=>!Number.isInteger(page)||page<1||page>10_000)))return false;
    if(operation.kind==="ocr"&&(!operation.languages.length||operation.languages.length>2||new Set(operation.languages).size!==operation.languages.length
      ||operation.languages.some(language=>!["deu","eng"].includes(language))||!Number.isInteger(operation.dpi)||operation.dpi<100||operation.dpi>400
      ||![0,90,180,270].includes(operation.rotation)||!Number.isSafeInteger(operation.offset)||operation.offset<0||operation.offset>1_000_000_000
      ||!Number.isInteger(operation.max_words)||operation.max_words<1||operation.max_words>100))return false;
    if(operation.kind!=="create"){
      const sha=config.inputs[operation.sha256_input].value;
      if(sha.source==="literal"&&(typeof sha.value!=="string"||!/^[a-f0-9]{64}$/.test(sha.value)))return false;
    }else{
      const path=config.inputs[operation.path_input].value;
      if(path.source==="literal"&&(typeof path.value!=="string"||!path.value.toLowerCase().endsWith("."+operation.format)))return false;
    }
    if(operation.kind==="append_table_rows"){
      if(!validTableSchema(operation))return false;
      const rows=config.inputs[operation.rows_input].value;
      if(rows.source==="literal"&&!validTableRows(operation,rows.value))return false;
      const source=config.inputs[operation.path_input].value,output=config.inputs[operation.output_path_input].value;
      if([source,output].some(path=>path.source==="literal"&&(typeof path.value!=="string"||!path.value.toLowerCase().endsWith(".xlsx"))))return false;
      if(source.source==="literal"&&output.source==="literal"&&source.value===output.value)return false;
    }
    return true;
  });
}

/** Preserve compatible declared bindings; initialize new types with visible empty values. */
export function setDocumentOperation(value:WorkflowDefinition,id:string,operation:WorkflowDocumentOperation):WorkflowDefinition {
  const step=value.documents?.[id],data=value.dataflow;
  if(!step||!data)throw new Error("workflow_document_invalid");
  const old=data.steps[id]?.inputs??{},inputs:Record<string,WorkflowValueBinding>={};
  for(const [name,kind] of Object.entries(documentInputTypes(operation)))inputs[name]=old[name]&&sameDataType(old[name].value_type,kind)?structuredClone(old[name]):{value_type:kind,value:{source:"literal",value:defaultDataValue(kind)}};
  return {...value,documents:{...value.documents,[id]:{...step,operation,rights:{...step.rights,tools:[documentTool(operation)]}}},
    dataflow:{...data,steps:{...data.steps,[id]:{inputs,output:documentOutputType(operation)}}}};
}

export function addDocumentStep(value:WorkflowDefinition,id:string,operation:WorkflowDocumentOperation,name:string):WorkflowDefinition {
  if(!value.dataflow||value.mode.mode==="hierarchical"||!dataFieldName(id)||workflowDataNodes(value).some(node=>node.id===id)||workflowDataNodes(value).length>=64)throw new Error("workflow_document_invalid");
  const steps=value.mode.mode==="sequential"?value.steps.map((step,index)=>({...step,dependencies:index?[...new Set([...step.dependencies,value.steps[index-1].id])]:step.dependencies})):value.steps;
  const step:WorkflowDocumentStep={name,dependencies:workflowDataNodes(value).map(node=>node.id),folder:{kind:"current_workspace"},operation,
    rights:{current_workspace:true,tools:[documentTool(operation)],folders:[]},budgets:structuredClone(value.budgets)};
  const next:WorkflowDefinition={...value,steps,format_version:value.format_version>=4?value.format_version:3,mode:value.mode.mode==="parallel"?value.mode:{mode:"parallel",max_parallel:1},documents:{...value.documents,[id]:step}};
  return setDocumentOperation(next,id,operation);
}

export function nativeDocumentWorkflow(name:string,stepName:string,markdown:string,budgets:Budgets):WorkflowDefinition {
  const operation:WorkflowDocumentOperation={kind:"create",format:"docx",path_input:"path",content_input:"content"};
  const rights={current_workspace:true,tools:["document_write"],folders:[]};
  return {format_version:3,name,mode:{mode:"parallel",max_parallel:1},failure:"stop",rights:structuredClone(rights),budgets:structuredClone(budgets),roles:[],steps:[],
    documents:{"create-document":{name:stepName,dependencies:[],folder:{kind:"current_workspace"},operation,rights:structuredClone(rights),budgets:structuredClone(budgets)}},
    dataflow:{input:{type:"object",fields:{}},steps:{"create-document":{inputs:{path:{value_type:{type:"text"},value:{source:"literal",value:"report.docx"}},content:{value_type:{type:"text"},value:{source:"literal",value:markdown}}},output:documentOutputType(operation)}}}};
}
