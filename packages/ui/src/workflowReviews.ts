import type {Budgets,WorkflowDataType,WorkflowDefinition,WorkflowReviewForm,WorkflowReviewRequest,WorkflowValueBinding} from "@cowork/client";
import {dataFieldName,defaultDataValue,enableWorkflowDataflow,sameDataType,validDataType,validDataValue,workflowDataNodes,workflowDataAncestors} from "./workflowDataflow";
import {validReviewEvidence} from "./workflowReviewEvidence";

const bytes=(value:string)=>new TextEncoder().encode(value).byteLength;
const base=(kind:WorkflowDataType)=>kind.type==="nullable"?kind.value:kind;
export function reviewPrefillType(form:WorkflowReviewForm):WorkflowDataType {
  return {type:"object",fields:Object.fromEntries(Object.entries(form.fields).map(([name,field])=>[name,field.value_type]))};
}
export function reviewOutput(form:WorkflowReviewForm):WorkflowDataType {
  return {type:"object",fields:Object.fromEntries(Object.entries(form.fields).map(([name,field])=>[name,field.required?base(field.value_type):field.value_type]))};
}
export function validReviewForm(form:WorkflowReviewForm):boolean {
  const sources=form.source_steps??[];
  if(!Array.isArray(sources)||sources.length>4||new Set(sources).size!==sources.length||sources.some(id=>typeof id!=="string"||!id.trim()||bytes(id)>120||id.includes("\0")))return false;
  if(!form.question.trim()||bytes(form.question)>4000||form.question.includes("\0")||!Object.keys(form.fields).length||Object.keys(form.fields).length>32||form.checks.length>16||!validDataType(reviewPrefillType(form))||!validDataType(reviewOutput(form)))return false;
  for(const [name,field] of Object.entries(form.fields)){
    const kind=base(field.value_type);
    if(field.required&&kind.type==="null"||!dataFieldName(name)||!field.label.trim()||bytes(field.label)>200||field.label.includes("\0")||bytes(field.help)>800||field.help.includes("\0")
      ||field.format==="date"&&kind.type!=="text"||field.format==="money_minor"&&kind.type!=="integer"
      ||(field.minimum!=null||field.maximum!=null)&&kind.type!=="integer"
      ||field.minimum!=null&&!Number.isSafeInteger(field.minimum)||field.maximum!=null&&!Number.isSafeInteger(field.maximum)
      ||field.minimum!=null&&field.maximum!=null&&field.minimum>field.maximum)return false;
  }
  return form.checks.every(check=>check.terms.length>=2&&check.terms.length<=16&&new Set(check.terms).size===check.terms.length&&!check.terms.includes(check.total)&&[...check.terms,check.total].every(name=>form.fields[name]&&(form.fields[name].required?base(form.fields[name].value_type):form.fields[name].value_type).type==="integer"));
}
export function eligibleReviewSources(definition:WorkflowDefinition,node:string):{id:string;name:string}[] {
  const ancestors=workflowDataAncestors(definition,node);
  return Object.entries(definition.documents??{}).filter(([id,step])=>ancestors.has(id)&&["read_text","ocr"].includes(step.operation.kind)).map(([id,step])=>({id,name:step.name}));
}
export type ReviewError={field:string|null;code:"schema"|"required"|"date"|"range"|"sum"};
export function reviewErrors(form:WorkflowReviewForm,value:unknown):ReviewError[] {
  if(!validReviewForm(form)||!value||typeof value!=="object"||Array.isArray(value))return [{field:null,code:"schema"}];
  const values=value as Record<string,unknown>,errors:ReviewError[]=[];
  if(Object.keys(values).some(name=>!Object.hasOwn(form.fields,name)))errors.push({field:null,code:"schema"});
  for(const [name,field] of Object.entries(form.fields)){
    const value=values[name];
    if(field.required&&(value===null||value===undefined||typeof value==="string"&&!value.trim()||Array.isArray(value)&&!value.length)){errors.push({field:name,code:"required"});continue;}
    if(!validDataValue(field.required?base(field.value_type):field.value_type,value)){errors.push({field:name,code:"schema"});continue;}
    if(value===null)continue;
    if(field.format==="date"){
      const text=String(value),match=/^(\d{4})-(\d{2})-(\d{2})$/.exec(text);
      const year=Number(match?.[1]),month=Number(match?.[2]),day=Number(match?.[3]);
      const leap=year%4===0&&(year%100!==0||year%400===0),days=[0,31,leap?29:28,31,30,31,30,31,31,30,31,30,31];
      if(!match||year<1||month<1||month>12||day<1||day>days[month])errors.push({field:name,code:"date"});
    }
    if(typeof value==="number"&&(field.minimum!=null&&value<field.minimum||field.maximum!=null&&value>field.maximum))errors.push({field:name,code:"range"});
  }
  for(const check of form.checks){
    if([...check.terms,check.total].every(name=>Number.isSafeInteger(values[name]))){
      const sum=check.terms.reduce((sum,name)=>sum+BigInt(values[name] as number),0n);
      if(sum!==BigInt(values[check.total] as number))errors.push({field:check.total,code:"sum"});
    }
  }
  return errors;
}
export function reviewAnswerValid(review:WorkflowReviewRequest|undefined|null,answer:string):boolean {
  if(!review)return true;
  if(!validReviewEvidence(review))return false;
  try{return reviewErrors(review.form,JSON.parse(answer)).length===0;}catch{return false;}
}
export function defaultReviewForm(language:string):WorkflowReviewForm {
  const de=language==="de";
  return {question:de?"Bitte prüfe und bestätige die Angaben.":"Review and confirm the values.",fields:{value:{label:de?"Angabe":"Value",help:"",value_type:{type:"text"},format:"value",required:true,minimum:null,maximum:null}},checks:[]};
}
export function addReviewStep(value:WorkflowDefinition,language:string):WorkflowDefinition {
  const nodes=workflowDataNodes(value);if(nodes.length>=64||value.mode.mode==="hierarchical"||value.roles.some(role=>role.delegate_to.length))throw new Error("workflow_review_invalid");
  const base=value.dataflow?value:enableWorkflowDataflow(value),id="review-"+crypto.randomUUID().slice(0,8),form=defaultReviewForm(language),rights={tools:["request_input"],folders:[]};
  const inputs=Object.fromEntries(Object.entries(form.fields).map(([name,field])=>[name,{value_type:field.value_type,value:{source:"literal" as const,value:defaultDataValue(field.value_type)}}]));
  return {...base,format_version:base.format_version>=4?base.format_version:3,mode:{mode:"parallel",max_parallel:base.mode.mode==="parallel"?base.mode.max_parallel:1},
    rights:{...base.rights,tools:[...new Set([...base.rights.tools,"request_input"])]},
    steps:base.steps.map((step,index)=>base.mode.mode==="sequential"&&index>0?{...step,dependencies:[...new Set([...step.dependencies,base.steps[index-1].id])]}:step),
    controls:{...base.controls,[id]:{name:language==="de"?"Angaben prüfen":"Review values",dependencies:nodes.map(node=>node.id),operation:{kind:"review",form,rights}}},
    dataflow:{...base.dataflow!,steps:{...base.dataflow!.steps,[id]:{inputs,output:reviewOutput(form)}}}};
}
export function nativeReviewWorkflow(name:string,language:string,budgets:Budgets):WorkflowDefinition {
  return addReviewStep({format_version:3,name,mode:{mode:"parallel",max_parallel:1},failure:"stop",rights:{tools:[],folders:[]},budgets:structuredClone(budgets),roles:[],steps:[],dataflow:{input:{type:"object",fields:{}},steps:{}}},language);
}
export function patchReviewForm(value:WorkflowDefinition,id:string,form:WorkflowReviewForm):WorkflowDefinition {
  const step=value.controls?.[id],data=value.dataflow;if(!step||!data||step.operation.kind!=="review")return value;
  const existing=data.steps[id]?.inputs??{},inputs:Record<string,WorkflowValueBinding>={};
  for(const [name,field] of Object.entries(form.fields))inputs[name]=existing[name]&&sameDataType(existing[name].value_type,field.value_type)?existing[name]:{value_type:field.value_type,value:{source:"literal",value:defaultDataValue(field.value_type)}};
  return {...value,controls:{...value.controls,[id]:{...step,operation:{...step.operation,form}}},dataflow:{...data,steps:{...data.steps,[id]:{inputs,output:reviewOutput(form)}}}};
}
/** Decimal text to integer cents; conversion never rounds or uses binary fractions. */
export function parseMoneyMinor(text:string):number|string {
  if(text.length>128)return text;
  const match=/^(-?)(\d+)(?:[.,](\d{1,2}))?$/.exec(text);
  if(!match)return text;
  const value=(BigInt(match[2])*100n+BigInt((match[3]??"").padEnd(2,"0")))*(match[1]? -1n:1n);
  return value>BigInt(Number.MAX_SAFE_INTEGER)||value<BigInt(Number.MIN_SAFE_INTEGER)?text:Number(value);
}
export function formatMoneyMinor(value:unknown):string {
  if(typeof value==="string")return value;
  if(typeof value!=="number"||!Number.isSafeInteger(value))return "";
  const n=BigInt(value),abs=n<0n?-n:n;
  return `${n<0n?"-":""}${abs/100n}.${String(abs%100n).padStart(2,"0")}`;
}
