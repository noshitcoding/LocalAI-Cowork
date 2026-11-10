import type {WorkflowDefinition,WorkflowEmbeddedIteration,WorkflowIterationArgument,WorkflowValueBinding,WorkflowDataType} from "@cowork/client";
import {dataFieldName,dataTypeAt,defaultDataValue,sameDataType,validDataType,validDataValue,workflowDataNodes} from "./workflowDataflow";
import {eligibleSubflowSource,subflowResultType,workflowTemplateLimits,intersectWorkflowRights,type SubflowResult} from "./workflowSubflows";

const own=(value:object,key:string)=>Object.hasOwn(value,key);
export function iterationInputType(d:WorkflowDefinition,id:string):WorkflowDataType|undefined {
  const config=d.dataflow?.steps[id];
  return config?{type:"object",fields:Object.fromEntries(Object.entries(config.inputs).map(([name,binding])=>[name,binding.value_type]))}:undefined;
}
export function iterationOutputType(d:WorkflowDefinition,id:string):WorkflowDataType|undefined {
  const call=d.iterations?.[id],collection=call&&d.dataflow?.steps[id]?.inputs[call.collection_input]?.value_type;
  const item=call&&subflowResultType(call.definition,call.result);
  return collection?.type==="list"&&item?{type:"list",item,max_items:collection.max_items}:undefined;
}
export function validWorkflowIterations(d:WorkflowDefinition):boolean {
  const loops=Object.entries(d.iterations??{});
  if(!loops.length)return d.format_version!==5;
  if(d.format_version!==5||d.mode.mode!=="parallel"||!d.dataflow||loops.length>16||!workflowTemplateLimits(d))return false;
  if([...d.steps.map(step=>step.id),...Object.keys(d.controls??{}),...Object.keys(d.subflows??{}),...Object.keys(d.documents??{}),...loops.map(([id])=>id),...d.roles.map(role=>role.id)].some(id=>["iteration-","composed-","delegated-"].some(prefix=>id.startsWith(prefix))))return false;
  return loops.every(([id,call])=>{
    if(!dataFieldName(id)||!call.name.trim()||call.name.length>200||!eligibleSubflowSource(call.definition)||!Number.isInteger(call.max_parallel)||call.max_parallel<1||call.max_parallel>32||new Set(call.dependencies).size!==call.dependencies.length)return false;
    const input=iterationInputType(d,id),collection=d.dataflow!.steps[id]?.inputs[call.collection_input]?.value_type;
    const bodyInput=call.definition.dataflow?.input,output=iterationOutputType(d,id);
    if(!input||collection?.type!=="list"||bodyInput?.type!=="object"||!output||!validDataType(output)||!sameDataType(output,d.dataflow!.steps[id].output)||Object.keys(call.arguments).length!==Object.keys(bodyInput.fields).length)return false;
    return Object.entries(bodyInput.fields).every(([name,expected])=>{
      if(!own(call.arguments,name))return false;
      const arg=call.arguments[name];
      if(arg.source==="literal")return validDataValue(expected,arg.value);
      const actual=arg.source==="item"?dataTypeAt(collection.item,arg.pointer??""):arg.source==="index"?{type:"integer"} as WorkflowDataType:dataTypeAt(input,arg.pointer);
      return !!actual&&sameDataType(expected,actual);
    });
  });
}
export function defaultIterationArguments(body:WorkflowDefinition):Record<string,WorkflowIterationArgument> {
  const input=body.dataflow?.input;if(input?.type!=="object")throw new Error("workflow_iteration_invalid");
  return Object.fromEntries(Object.entries(input.fields).map(([name,kind])=>[name,{source:"literal",value:defaultDataValue(kind)}]));
}
export function addWorkflowIteration(d:WorkflowDefinition,body:WorkflowDefinition,result:SubflowResult,id:string,collection:WorkflowValueBinding,args:Record<string,WorkflowIterationArgument>):WorkflowDefinition {
  if(!d.dataflow||d.mode.mode==="hierarchical"||!dataFieldName(id)||["iteration-","composed-","delegated-"].some(prefix=>id.startsWith(prefix))||workflowDataNodes(d).some(node=>node.id===id)||!eligibleSubflowSource(body)||collection.value_type.type!=="list")throw new Error("workflow_iteration_invalid");
  const item=subflowResultType(body,result);if(!item)throw new Error("workflow_iteration_result_required");
  const call:WorkflowEmbeddedIteration={name:body.name,dependencies:workflowDataNodes(d).map(node=>node.id),definition:structuredClone(body),collection_input:"items",arguments:structuredClone(args),result:structuredClone(result),max_parallel:2,
    rights:intersectWorkflowRights(d.rights,body.rights),budgets:structuredClone(d.budgets)};
  const steps=d.steps.map((step,index)=>({...step,dependencies:d.mode.mode==="sequential"&&index>0?[...new Set([...step.dependencies,d.steps[index-1].id])]:[...step.dependencies]}));
  const next:WorkflowDefinition={...d,steps,format_version:5,mode:{mode:"parallel",max_parallel:d.mode.mode==="parallel"?d.mode.max_parallel:1},iterations:{...d.iterations,[id]:call},dataflow:{...d.dataflow,steps:{...d.dataflow.steps,[id]:{inputs:{items:structuredClone(collection)},output:{type:"list",item:structuredClone(item),max_items:collection.value_type.max_items}}}}};
  if(!validWorkflowIterations(next))throw new Error("workflow_iteration_invalid");
  return next;
}
export function changeIterationResult(d:WorkflowDefinition,id:string,result:SubflowResult):WorkflowDefinition {
  const call=d.iterations?.[id];if(!call||!d.dataflow)throw new Error("workflow_iteration_invalid");
  const next={...d,iterations:{...d.iterations,[id]:{...call,result:structuredClone(result)}}};
  const output=iterationOutputType(next,id);if(!output)throw new Error("workflow_iteration_result_required");
  return {...next,dataflow:{...d.dataflow,steps:{...d.dataflow.steps,[id]:{...d.dataflow.steps[id],output}}}};
}
export function removeWorkflowIteration(d:WorkflowDefinition,id:string):WorkflowDefinition {
  const iterations=Object.fromEntries(Object.entries(d.iterations??{}).filter(([key])=>key!==id).map(([key,call])=>[key,{...call,dependencies:call.dependencies.filter(dependency=>dependency!==id)}]));
  return {...d,...(d.documents?{documents:Object.fromEntries(Object.entries(d.documents).map(([key,step])=>[key,{...step,dependencies:step.dependencies.filter(dependency=>dependency!==id)}]))}:{}),format_version:Object.keys(iterations).length?5:Object.keys(d.subflows??{}).length?4:3,iterations:Object.keys(iterations).length?iterations:undefined,
    steps:d.steps.map(step=>({...step,dependencies:step.dependencies.filter(dependency=>dependency!==id)})),
    controls:d.controls?Object.fromEntries(Object.entries(d.controls).map(([key,step])=>[key,{...step,dependencies:step.dependencies.filter(dependency=>dependency!==id)}])):undefined,
    subflows:d.subflows?Object.fromEntries(Object.entries(d.subflows).map(([key,call])=>[key,{...call,dependencies:call.dependencies.filter(dependency=>dependency!==id)}])):undefined,
    gates:d.gates?Object.fromEntries(Object.entries(d.gates).filter(([key])=>key!==id)):undefined,
    dataflow:d.dataflow?{...d.dataflow,steps:Object.fromEntries(Object.entries(d.dataflow.steps).filter(([key])=>key!==id))}:undefined};
}
