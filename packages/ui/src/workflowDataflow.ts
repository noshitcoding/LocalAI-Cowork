import type {WorkflowDataType,WorkflowDefinition,WorkflowStepData} from "@cowork/client";
import messages from "./workflowMessages.json";
import {documentOutputType} from "./workflowDocumentTypes";

export function workflowDataError(error:unknown,language:"de"|"en"):string {
  const message=error instanceof Error?error.message:String(error);
  return (message.startsWith("workflow_data_")||message.startsWith("workflow_control_")||message.startsWith("workflow_current_workspace_")||message.startsWith("workflow_document_")||message.startsWith("workflow_native_document_")||message.startsWith("workflow_table_"))?(messages[language] as Record<string,string>)[message]??message:message;
}

export const dataFieldName=(value:string)=>/^[A-Za-z0-9_-]{1,64}$/.test(value)&&!["__proto__","prototype","constructor"].includes(value);
export const sameDataType=(a:WorkflowDataType,b:WorkflowDataType):boolean=>{
  const canonical=(value:unknown):string=>JSON.stringify(value,(_key,item)=>item&&typeof item==="object"&&!Array.isArray(item)?Object.fromEntries(Object.entries(item).sort(([a],[b])=>a.localeCompare(b))):item);
  return canonical(a)===canonical(b);
};
export function validDataType(kind:WorkflowDataType):boolean {
  let count=0;
  const visit=(type:WorkflowDataType,depth:number):boolean=>{
    if(++count>256||depth>8)return false;
    if(type.type==="object")return Object.keys(type.fields).length<=64&&Object.entries(type.fields).every(([key,value])=>dataFieldName(key)&&visit(value,depth+1));
    if(type.type==="list")return Number.isInteger(type.max_items)&&type.max_items>=1&&type.max_items<=1000&&visit(type.item,depth+1);
    if(type.type==="nullable")return type.value.type!=="null"&&type.value.type!=="nullable"&&visit(type.value,depth+1);
    return ["text","number","integer","boolean","null"].includes(type.type);
  };
  return visit(kind,0);
}
export function validDataValue(kind:WorkflowDataType,value:unknown):boolean {
  if(!validDataType(kind))return false;
  let count=0;
  const bounded=(value:unknown,depth:number):boolean=>++count<=4096&&depth<=16&&(Array.isArray(value)?value.every(item=>bounded(item,depth+1)):!!value&&typeof value==="object"?Object.values(value).every(item=>bounded(item,depth+1)):true);
  try{if(!bounded(value,0)||new TextEncoder().encode(JSON.stringify(value)).byteLength>256*1024)return false;}catch{return false;}
  const check=(kind:WorkflowDataType,value:unknown):boolean=>{
    switch(kind.type){
      case "text":return typeof value==="string"&&Array.from(value).length<=16384&&!value.includes("\0");
      case "number":return typeof value==="number"&&Number.isFinite(value);
      case "integer":return typeof value==="number"&&Number.isSafeInteger(value);
      case "boolean":return typeof value==="boolean";
      case "null":return value===null;
      case "nullable":return value===null||check(kind.value,value);
      case "list":return Array.isArray(value)&&value.length<=kind.max_items&&value.every(item=>check(kind.item,item));
      case "object":return !!value&&typeof value==="object"&&!Array.isArray(value)&&Object.keys(value).length===Object.keys(kind.fields).length&&Object.entries(kind.fields).every(([key,type])=>Object.hasOwn(value,key)&&check(type,(value as Record<string,unknown>)[key]));
    }
  };
  return check(kind,value);
}
export function dataTypeAt(kind:WorkflowDataType,pointer:string):WorkflowDataType|undefined {
  if(!pointer)return kind;
  if(pointer.length>1024||!pointer.startsWith("/"))return undefined;
  let current:WorkflowDataType|undefined=kind;
  for(const token of pointer.slice(1).split("/")){
    if(!token||token.includes("~")||!current)return undefined;
    if(current.type==="object")current=Object.hasOwn(current.fields,token)?current.fields[token]:undefined;
    else if(current.type==="list"&&/^(0|[1-9][0-9]*)$/.test(token)&&Number(token)<current.max_items)current=current.item;
    else return undefined;
  }
  return current;
}
export function defaultDataValue(kind:WorkflowDataType):unknown {
  switch(kind.type){
    case "text":return "";
    case "number":case "integer":return 0;
    case "boolean":return false;
    case "null":case "nullable":return null;
    case "list":return [];
    case "object":return Object.fromEntries(Object.entries(kind.fields).map(([key,type])=>[key,defaultDataValue(type)]));
  }
}
export const defaultStepData=(native=false):WorkflowStepData=>({inputs:{},output:native?{type:"text"}:{type:"object",fields:{result:{type:"text"}}}});
export function workflowDataNodes(definition:WorkflowDefinition){return [...definition.steps,...Object.entries(definition.controls??{}).map(([id,step])=>({id,goal:step.name,role_id:"",dependencies:step.dependencies,execution:undefined})),...Object.entries(definition.subflows??{}).map(([id,step])=>({id,goal:step.name,role_id:"",dependencies:step.dependencies,execution:undefined})),...Object.entries(definition.iterations??{}).map(([id,step])=>({id,goal:step.name,role_id:"",dependencies:step.dependencies,execution:undefined})),...Object.entries(definition.documents??{}).map(([id,step])=>({id,goal:step.name,role_id:"",dependencies:step.dependencies,execution:undefined}))];}
export function enableWorkflowDataflow(definition:WorkflowDefinition):WorkflowDefinition {
  if(definition.dataflow)return structuredClone(definition);
  return {...definition,format_version:2,roles:definition.roles.map(role=>({...role,output:undefined})),dataflow:{input:{type:"object",fields:{}},steps:Object.fromEntries(definition.steps.map(step=>[step.id,defaultStepData(!!step.execution)]))}};
}
export function reconcileWorkflowDataflow(next:WorkflowDefinition,previous:WorkflowDefinition):WorkflowDefinition {
  if(!next.dataflow)return next;
  return {...next,dataflow:{...next.dataflow,steps:Object.fromEntries(workflowDataNodes(next).map(step=>{
    const old=previous.steps.find(old=>old.id===step.id),saved=next.dataflow!.steps[step.id];
    if(next.documents?.[step.id]&&saved)return [step.id,{...saved,output:documentOutputType(next.documents[step.id].operation)}];
    return [step.id,!saved?defaultStepData(!!step.execution):!!old?.execution!==!!step.execution?{...saved,inputs:step.execution?{}:saved.inputs,output:defaultStepData(!!step.execution).output}:saved];
  }))}};
}
export function workflowDataAncestors(definition:WorkflowDefinition,id:string):Set<string>{
  const direct=(id:string):string[]=>{
    const index=definition.steps.findIndex(step=>step.id===id);if(index<0)return [...(definition.controls?.[id]?.dependencies??definition.subflows?.[id]?.dependencies??definition.iterations?.[id]?.dependencies??definition.documents?.[id]?.dependencies??[])];
    const step=definition.steps[index],result=[...step.dependencies];
    if(definition.mode.mode==="sequential"&&index>0)result.push(definition.steps[index-1].id);
    if(definition.mode.mode==="hierarchical"&&definition.mode.manager_step_id===id)result.push(...definition.steps.filter(step=>step.id!==id).map(step=>step.id));
    return result;
  };
  const result=new Set<string>(),pending=direct(id);
  while(pending.length){const id=pending.pop()!;if(!result.has(id)){result.add(id);pending.push(...direct(id));}}
  return result;
}
export function validWorkflowDataflow(definition:WorkflowDefinition):boolean {
  const data=definition.dataflow;
  if(!data)return definition.format_version===1;
  if(![2,3,4,5].includes(definition.format_version)||data.input.type!=="object"||!validDataType(data.input)||Object.keys(data.steps).length!==workflowDataNodes(definition).length||definition.roles.some(role=>!!role.output||role.delegate_to.length>0))return false;
  return workflowDataNodes(definition).every(step=>{
    const config=data.steps[step.id];if(!config||!validDataType(config.output)||Object.keys(config.inputs).length>32)return false;
    if(step.execution?(config.output.type!=="text"||Object.keys(config.inputs).length>0):!definition.controls?.[step.id]&&!definition.subflows?.[step.id]&&!definition.iterations?.[step.id]&&config.output.type!=="object")return false;
    const policy=config.context_policy??"inherit";
    if(!["inherit","bound_inputs_only"].includes(policy)||policy==="bound_inputs_only"&&!definition.steps.some(model=>model.id===step.id&&!model.execution))return false;
    const ancestors=workflowDataAncestors(definition,step.id);
    return Object.entries(config.inputs).every(([key,binding])=>{
      if(!dataFieldName(key)||!validDataType(binding.value_type))return false;
      const source=binding.value;
      if(source.source==="literal")return validDataValue(binding.value_type,source.value);
      const type=source.source==="input"?dataTypeAt(data.input,source.pointer):ancestors.has(source.step_id)&&data.steps[source.step_id]?dataTypeAt(data.steps[source.step_id].output,source.pointer):undefined;
      return !!type&&sameDataType(type,binding.value_type);
    });
  });
}
export function workflowInputForStart(definition:WorkflowDefinition,value:unknown):unknown {
  if(!definition.dataflow)return undefined;
  if(!validDataValue(definition.dataflow.input,value))throw new Error("workflow_data_type_mismatch");
  return structuredClone(value);
}
