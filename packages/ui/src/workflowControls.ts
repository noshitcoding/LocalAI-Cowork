import type {WorkflowDefinition,WorkflowPredicate,WorkflowDataType,WorkflowValueBinding,WorkflowControlOperation} from "@cowork/client";
import {dataFieldName,dataTypeAt,sameDataType,validDataValue,workflowDataAncestors} from "./workflowDataflow";
import {eligibleReviewSources,reviewOutput,validReviewForm} from "./workflowReviews";
export function validWorkflowControls(d:WorkflowDefinition):boolean {
  const controls=d.controls??{},gates=d.gates??{};
  if(![3,4,5].includes(d.format_version))return !Object.keys(controls).length&&!Object.keys(gates).length;
  if(d.mode.mode!=="parallel"||!d.dataflow)return false;
  const nodes=new Set([...d.steps.map(s=>s.id),...Object.keys(controls),...Object.keys(d.subflows??{}),...Object.keys(d.iterations??{}),...Object.keys(d.documents??{})]);
  for(const [id,gate] of Object.entries(gates))if(!nodes.has(id)||controls[gate.step_id]?.operation?.kind!=="condition"||!workflowDataAncestors(d,id).has(gate.step_id))return false;
  for(const [id,step] of Object.entries(controls)){
    const config=d.dataflow.steps[id];
    if(!dataFieldName(id)||id.startsWith("delegated-")||!step.name.trim()||step.name.length>200||!config||new Set(step.dependencies).size!==step.dependencies.length)return false;
    const op=step.operation;
    if(op.kind==="review"&&(!validReviewForm(op.form)||op.rights.current_workspace||op.rights.folders.length||op.rights.tools.length!==1||op.rights.tools[0]!=="request_input"||!d.rights.tools.includes("request_input")||!sameDataType(config.output,reviewOutput(op.form))||Object.keys(config.inputs).length!==Object.keys(op.form.fields).length||Object.entries(op.form.fields).some(([name,field])=>!config.inputs[name]||!sameDataType(config.inputs[name].value_type,field.value_type))))return false;
    if(op.kind==="review"&&(op.form.source_steps??[]).some(source=>!eligibleReviewSources(d,id).some(item=>item.id===source)))return false;
    if(op.kind==="wait"){
      const binding=config.inputs[op.seconds_input];
      if(Object.keys(config.inputs).length!==1||!Object.hasOwn(config.inputs,op.seconds_input)||binding?.value_type?.type!=="integer"||!sameDataType(config.output,{type:"object",fields:{wake_at:{type:"integer"}}}))return false;
      if(binding.value.source==="literal"&&(!Number.isSafeInteger(binding.value.value)||Number(binding.value.value)<1||Number(binding.value.value)>86400||Number(binding.value.value)>=d.budgets.max_seconds-1))return false;
    }
    if(op.kind==="condition"&&(config.output.type!=="boolean"||!validPredicate(op.predicate,config.inputs)))return false;
    if(op.kind==="compose"&&!sameDataType(config.output,{type:"object",fields:Object.fromEntries(Object.entries(config.inputs).map(([key,binding])=>[key,binding.value_type]))}))return false;
    if(op.kind==="select"||op.kind==="collect"){
      if(Object.keys(config.inputs).length||(op.kind==="select"&&op.candidates.length<1)||op.candidates.length>64||new Set(op.candidates.map(c=>JSON.stringify(c))).size!==op.candidates.length)return false;
      const output=op.kind==="collect"&&config.output.type==="list"&&config.output.max_items>=op.candidates.length?config.output.item:op.kind==="select"?config.output:undefined;
      const ancestors=workflowDataAncestors(d,id);
      if(!output||op.candidates.some(c=>{const source=d.dataflow?.steps[c.step_id]&&dataTypeAt(d.dataflow.steps[c.step_id].output,c.pointer);return !ancestors.has(c.step_id)||!source||!sameDataType(source,output);} ))return false;
    }
  }
  return true;
}
function validPredicate(p:WorkflowPredicate,inputs:Record<string,WorkflowValueBinding>,depth=0,count={value:0}):boolean {
  if(depth>4||++count.value>32)return false;
  if(p.operator==="all"||p.operator==="any")return p.predicates.length>0&&p.predicates.length<=16&&p.predicates.every(v=>validPredicate(v,inputs,depth+1,count));
  if(p.operator==="not")return validPredicate(p.predicate,inputs,depth+1,count);
  const type=inputs[p.input]?.value_type;if(!type)return false;
  if(p.operator==="is_true")return type.type==="boolean";
  if(p.operator==="is_null")return type.type==="null"||type.type==="nullable";
  if(p.operator==="contains")return type.type==="text"&&validDataValue(type,p.value);
  if(p.operator==="greater"||p.operator==="less")return ["number","integer"].includes(type.type)&&validDataValue(type,p.value);
  return validDataValue(type,p.value);
}
export function controlOutput(operation:WorkflowControlOperation,inputs:Record<string,WorkflowValueBinding>,candidate?:WorkflowDataType):WorkflowDataType {
  if(operation.kind==="review")return reviewOutput(operation.form);
  if(operation.kind==="wait")return {type:"object",fields:{wake_at:{type:"integer"}}};
  if(operation.kind==="condition")return {type:"boolean"};
  if(operation.kind==="compose")return {type:"object",fields:Object.fromEntries(Object.entries(inputs).map(([k,v])=>[k,v.value_type]))};
  if(operation.kind==="collect")return {type:"list",item:candidate??{type:"text"},max_items:Math.max(1,operation.candidates.length)};
  return candidate??{type:"text"};
}

/** Remove ordering edges; dangling data and condition references require review. */
export function removeWorkflowNode(d:WorkflowDefinition,id:string):WorkflowDefinition {
  return {...d,...(d.documents?{documents:Object.fromEntries(Object.entries(d.documents).filter(([key])=>key!==id).map(([key,step])=>[key,{...step,dependencies:step.dependencies.filter(dep=>dep!==id)}]))}:{}),iterations:d.iterations?Object.fromEntries(Object.entries(d.iterations).map(([key,loop])=>[key,{...loop,dependencies:loop.dependencies.filter(dep=>dep!==id)}])):undefined,subflows:d.subflows?Object.fromEntries(Object.entries(d.subflows).map(([key,call])=>[key,{...call,dependencies:call.dependencies.filter(dep=>dep!==id)}])):undefined,steps:d.steps.filter(s=>s.id!==id).map(s=>({...s,dependencies:s.dependencies.filter(dep=>dep!==id)})),controls:d.controls?Object.fromEntries(Object.entries(d.controls).filter(([key])=>key!==id).map(([key,s])=>[key,{...s,dependencies:s.dependencies.filter(dep=>dep!==id)}])):undefined,gates:d.gates?Object.fromEntries(Object.entries(d.gates).filter(([key])=>key!==id)):undefined};
}
