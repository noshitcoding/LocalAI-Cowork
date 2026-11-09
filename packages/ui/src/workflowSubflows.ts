import {defaultRunBudgets,type Budgets,type WorkflowDataType,type WorkflowDefinition,type WorkflowEmbeddedSubflow,type WorkflowRights} from "@cowork/client";
import {dataFieldName,dataTypeAt,defaultDataValue,sameDataType,validDataType} from "./workflowDataflow";

/** Source definitions retain frozen copies; generated execution nodes stay private. */
export type SubflowDraftDefinition=WorkflowDefinition;
export type SubflowResult={step_id:string;pointer:string};
export type WorkflowSubflowDraft=WorkflowEmbeddedSubflow;
export type SubflowNode={id:string;goal:string;dependencies:string[]};

export function subflowNodes(value:SubflowDraftDefinition):SubflowNode[]{
  return [...value.steps.map(step=>({id:step.id,goal:step.goal,dependencies:[...step.dependencies]})),
    ...Object.entries(value.controls??{}).map(([id,step])=>({id,goal:step.name,dependencies:[...step.dependencies]})),
    ...Object.entries(value.subflows??{}).map(([id,step])=>({id,goal:step.name,dependencies:[...step.dependencies]})),
    ...Object.entries(value.iterations??{}).map(([id,step])=>({id,goal:step.name,dependencies:[...step.dependencies]})),
    ...Object.entries(value.documents??{}).map(([id,step])=>({id,goal:step.name,dependencies:[...step.dependencies]}))];
}
const totalTokens=(value:Budgets)=>value.max_total_tokens??defaultRunBudgets.max_total_tokens!;
export function subflowBudget(parent:Budgets,body:Budgets):Budgets{
  return {max_steps:Math.min(parent.max_steps,body.max_steps),max_seconds:Math.min(parent.max_seconds,body.max_seconds),max_output_tokens:Math.min(parent.max_output_tokens,body.max_output_tokens),max_total_tokens:Math.min(totalTokens(parent),totalTokens(body))};
}
export const intersectWorkflowRights=(parent:WorkflowRights,body:WorkflowRights):WorkflowRights=>({...(parent.current_workspace&&body.current_workspace?{current_workspace:true}:{}),tools:parent.tools.filter(id=>body.tools.includes(id)),folders:parent.folders.filter(id=>body.folders.includes(id))});
export function subflowResultType(value:SubflowDraftDefinition,result:SubflowResult):WorkflowDataType|undefined{
  if(!Object.hasOwn(value.dataflow?.steps??{},result.step_id))return undefined;
  return dataTypeAt(value.dataflow!.steps[result.step_id].output,result.pointer);
}
export function subflowResultPaths(kind:WorkflowDataType):{pointer:string;kind:WorkflowDataType}[]{
  const result:{pointer:string;kind:WorkflowDataType}[]=[];
  const visit=(kind:WorkflowDataType,pointer:string,depth:number)=>{
    if(depth>8||result.length>=256)return;
    result.push({pointer,kind});
    if(kind.type==="object")for(const [name,child] of Object.entries(kind.fields))visit(child,pointer+"/"+name,depth+1);
    if(kind.type==="list")visit(kind.item,pointer+"/0",depth+1);
  };
  if(validDataType(kind))visit(kind,"",0);
  return result;
}
export function eligibleSubflowSource(value:SubflowDraftDefinition):boolean{
  const active=new Set<object>();let nodes=0,roles=0,calls=0;
  const visit=(value:SubflowDraftDefinition,depth:number):boolean=>{
    if(!value||typeof value!=="object"||active.has(value)||depth>4||Object.keys(value.iterations??{}).length||!Array.isArray(value.steps)||!Array.isArray(value.roles)||!value.dataflow||value.dataflow.input.type!=="object"||!validDataType(value.dataflow.input)||value.mode.mode==="hierarchical")return false;
    const children=Object.entries(value.subflows??{});
    if(![2,3,4].includes(value.format_version)||(value.format_version===4)!==!!children.length||children.length&&value.mode.mode!=="parallel")return false;
    const all=subflowNodes(value);nodes+=value.steps.length+Object.keys(value.controls??{}).length+Object.keys(value.documents??{}).length+children.length*2;roles+=value.roles.length;calls+=children.length;
    if(nodes>64||roles>32||calls>16||!all.length||all.length!==new Set(all.map(node=>node.id)).size||all.some(node=>!dataFieldName(node.id)||node.id.startsWith("composed-")||node.id.startsWith("delegated-"))||Object.keys(value.dataflow.steps).length!==all.length||value.roles.some(role=>!!role.output||role.delegate_to.length>0))return false;
    if(!all.every(node=>Object.hasOwn(value.dataflow!.steps,node.id)&&validDataType(value.dataflow!.steps[node.id].output)))return false;
    active.add(value);
    for(const [id,call] of children){
      if(!call||!visit(call.definition,depth+1)){active.delete(value);return false;}
      const config=value.dataflow.steps[id],expected=call.definition.dataflow!.input;
      const args:WorkflowDataType={type:"object",fields:Object.fromEntries(Object.entries(config.inputs).map(([key,binding])=>[key,binding.value_type]))};
      const output=subflowResultType(call.definition,call.result);
      if(!sameDataType(args,expected)||!output||!sameDataType(output,config.output)){active.delete(value);return false;}
    }
    active.delete(value);return true;
  };
  return visit(value,0);
}

/** Static expansion and frozen template roles, never multiplied by list length. */
export function workflowTemplateLimits(value:WorkflowDefinition):boolean {
  const seen=new Set<object>();
  const measure=(d:WorkflowDefinition,depth:number):{nodes:number;roles:number;calls:number}|undefined=>{
    if(depth>4||seen.has(d))return undefined;seen.add(d);
    let nodes=d.steps.length+Object.keys(d.controls??{}).length+Object.keys(d.documents??{}).length+Object.keys(d.subflows??{}).length*2+Object.keys(d.iterations??{}).length*2,roles=d.roles.length,calls=Object.keys(d.subflows??{}).length;
    for(const call of Object.values(d.subflows??{})){const child=measure(call.definition,depth+1);if(!child)return undefined;nodes+=child.nodes;roles+=child.roles;calls+=child.calls;}
    for(const loop of Object.values(d.iterations??{})){if(depth||!eligibleSubflowSource(loop.definition))return undefined;const child=measure(loop.definition,0);if(!child)return undefined;roles+=child.roles;}
    seen.delete(d);return nodes<=64&&roles<=32&&calls<=16?{nodes,roles,calls}:undefined;
  };
  return !!measure(value,0);
}
export function validSubflowCall(d:WorkflowDefinition,id:string):boolean {
  const call=d.subflows?.[id],config=d.dataflow?.steps[id];if(!call||!config||!eligibleSubflowSource(call.definition))return false;
  const expected=call.definition.dataflow!.input,actual:WorkflowDataType={type:"object",fields:Object.fromEntries(Object.entries(config.inputs).map(([name,binding])=>[name,binding.value_type]))};
  const result=subflowResultType(call.definition,call.result);
  return sameDataType(expected,actual)&&!!result&&sameDataType(result,config.output);
}
function parallel(value:SubflowDraftDefinition):SubflowDraftDefinition{
  const steps=value.steps.map((step,index)=>({...step,dependencies:value.mode.mode==="sequential"&&index>0?[...new Set([...step.dependencies,value.steps[index-1].id])]:[...step.dependencies]}));
  return {...value,steps,mode:{mode:"parallel",max_parallel:value.mode.mode==="parallel"?value.mode.max_parallel:1}};
}
export function addWorkflowSubflow(value:SubflowDraftDefinition,body:SubflowDraftDefinition,result:SubflowResult,id:string):SubflowDraftDefinition{
  if(!value.dataflow||value.mode.mode==="hierarchical"||!dataFieldName(id)||id.startsWith("composed-")||id.startsWith("delegated-")||subflowNodes(value).some(node=>node.id===id)||!eligibleSubflowSource(body))throw new Error("workflow_subflow_invalid");
  const output=subflowResultType(body,result);if(!output)throw new Error("workflow_subflow_result_required");
  const args=body.dataflow!.input;if(args.type!=="object")throw new Error("workflow_subflow_invalid");
  const next=parallel(value),source=structuredClone(body);
  const call:WorkflowSubflowDraft={name:body.name,dependencies:subflowNodes(value).map(node=>node.id),definition:source,result:structuredClone(result),rights:intersectWorkflowRights(value.rights,body.rights),budgets:subflowBudget(value.budgets,body.budgets)};
  const candidate:SubflowDraftDefinition={...next,format_version:Object.keys(value.iterations??{}).length?5:4,subflows:{...value.subflows,[id]:call},dataflow:{...next.dataflow!,steps:{...next.dataflow!.steps,[id]:{inputs:Object.fromEntries(Object.entries(args.fields).map(([key,kind])=>[key,{value_type:structuredClone(kind),value:{source:"literal" as const,value:defaultDataValue(kind)}}])),output:structuredClone(output)}}}};
  if(candidate.format_version===5?!workflowTemplateLimits(candidate):!eligibleSubflowSource(candidate))throw new Error("workflow_subflow_limits");
  return candidate;
}
export function changeSubflowResult(value:SubflowDraftDefinition,id:string,result:SubflowResult):SubflowDraftDefinition{
  if(!Object.hasOwn(value.subflows??{},id))throw new Error("workflow_subflow_invalid");
  const call=value.subflows![id],kind=subflowResultType(call.definition,result);if(!kind||!value.dataflow)throw new Error("workflow_subflow_result_required");
  return {...value,subflows:{...value.subflows,[id]:{...call,result:structuredClone(result)}},dataflow:{...value.dataflow,steps:{...value.dataflow.steps,[id]:{...value.dataflow.steps[id],output:structuredClone(kind)}}}};
}
export function removeWorkflowSubflow(value:SubflowDraftDefinition,id:string):SubflowDraftDefinition{
  const subflows=Object.fromEntries(Object.entries(value.subflows??{}).filter(([key])=>key!==id).map(([key,call])=>[key,{...call,dependencies:call.dependencies.filter(dependency=>dependency!==id)}]));
  return {...value,...(value.documents?{documents:Object.fromEntries(Object.entries(value.documents).map(([key,step])=>[key,{...step,dependencies:step.dependencies.filter(dependency=>dependency!==id)}]))}:{}),format_version:Object.keys(value.iterations??{}).length?5:Object.keys(subflows).length?4:3,subflows:Object.keys(subflows).length?subflows:undefined,
    iterations:value.iterations?Object.fromEntries(Object.entries(value.iterations).map(([key,loop])=>[key,{...loop,dependencies:loop.dependencies.filter(dependency=>dependency!==id)}])):undefined,
    steps:value.steps.map(step=>({...step,dependencies:step.dependencies.filter(dependency=>dependency!==id)})),
    controls:value.controls?Object.fromEntries(Object.entries(value.controls).map(([key,step])=>[key,{...step,dependencies:step.dependencies.filter(dependency=>dependency!==id)}])):undefined,
    dataflow:value.dataflow?{...value.dataflow,steps:Object.fromEntries(Object.entries(value.dataflow.steps).filter(([key])=>key!==id))}:undefined};
}
