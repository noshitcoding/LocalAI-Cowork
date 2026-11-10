import {validWorkflowDocuments} from "./workflowDocuments";
import {validWorkflowIterations} from "./workflowIterations";
import {validSubflowCall,workflowTemplateLimits} from "./workflowSubflows";
import {validWorkflowControls} from "./workflowControls";
import {defaultRunBudgets, type Budgets, type WorkflowDefinition, type WorkflowLibraryEntry, type Client, type WorkflowRunRequest} from "@cowork/client";
import {validWorkflowDataflow} from "./workflowDataflow";

export const workflowRolePattern = "[A-Za-z0-9_\\-]+";
export const workflowTools = ["create_artifact", "list_artifacts", "read_artifact", "request_input", "delegate_agents", "list_files", "read_text_file", "stat_file", "write_text_file", "create_directory", "rename_file", "remove_file", "restore_file", "execute_program", "browser", "office", "github", "document_read", "document_write", "image_edit"] as const;
export function completeWorkflowBudgets(value: Budgets): Required<Budgets> {
  return {...value,max_steps:value.max_steps,max_seconds:value.max_seconds,max_output_tokens:value.max_output_tokens,max_total_tokens:value.max_total_tokens??defaultRunBudgets.max_total_tokens};
}
export function workflowSelection(workflowId: string, versionId: string): string {
  return JSON.stringify([workflowId, versionId]);
}
export function findWorkflowSelection(items: WorkflowLibraryEntry[], value: string) {
  for (const workflow of items) {
    for (const version of workflow.versions) {
      if (workflowSelection(workflow.id, version.id) === value) return {workflow, version};
    }
  }
  return undefined;
}
export function editableWorkflow(definition: WorkflowDefinition,depth=0): boolean {
  if(depth>4)return false;
  const calls=[...Object.values(definition.subflows??{}),...Object.values(definition.iterations??{})];
  return [definition.rights, ...definition.roles.map(r => r.rights), ...definition.steps.map(s => s.rights),...Object.values(definition.documents??{}).map(s=>s.rights),...calls.map(call=>call.rights)]
      .every(rights => rights.tools.every(tool => workflowTools.includes(tool as typeof workflowTools[number])||/^mcp_[a-f0-9]{40}$/.test(tool)))&&calls.every(call=>editableWorkflow(call.definition,depth+1));
}
/** A name collision has no unambiguous binding. Never infer one from a later repair. */
export function renameWorkflowRole(definition:WorkflowDefinition,index:number,id:string){
  const role=definition.roles[index];
  if(!role||id===role.id)return {definition,collision:false};
  if(definition.roles.some((other,i)=>i!==index&&other.id===id))return {definition,collision:true};
  const ambiguous=definition.roles.filter(other=>other.id===role.id).length!==1;
  return {collision:false,definition:{...definition,roles:definition.roles.map((other,i)=>({...other,id:i===index?id:other.id,delegate_to:other.delegate_to.map(target=>target===role.id&&!ambiguous?id:target)})),
    steps:definition.steps.map(step=>step.role_id===role.id?{...step,role_id:ambiguous?"":id}:step)}};
}
/** Validation for editor feedback only. The service remains authoritative. */
export function workflowRoleInstructionsValid(instructions:string):boolean{return !!instructions.trim()&&!instructions.includes("\0")&&new TextEncoder().encode(instructions).byteLength<=256000;}
export function validWorkflowEditor(definition: WorkflowDefinition,depth=0): boolean {
  if(depth>4)return false;
  const calls=Object.entries(definition.subflows??{});
  const loops=Object.entries(definition.iterations??{});
  if(!validWorkflowIterations(definition)||!workflowTemplateLimits(definition))return false;
  if(!loops.length&&(definition.format_version===4)!==!!calls.length)return false;
  if(calls.some(([id,call])=>!call.name.trim()||call.name.length>200||!validSubflowCall(definition,id)||!validWorkflowEditor(call.definition,depth+1)))return false;
  if(loops.some(([,loop])=>!validWorkflowEditor(loop.definition,depth+1)))return false;
  if(!validWorkflowDataflow(definition)||!validWorkflowControls(definition)||!validWorkflowDocuments(definition))return false;
  const ids = new Set(definition.roles.map(role => role.id));
  const nodes = new Map<string,{id:string;dependencies:string[]}>([...definition.steps.map(step => [step.id, step] as const),...Object.entries(definition.controls??{}).map(([id,step])=>[id,{id,dependencies:step.dependencies}] as const),...calls.map(([id,step])=>[id,{id,dependencies:step.dependencies}] as const),...loops.map(([id,step])=>[id,{id,dependencies:step.dependencies}] as const),...Object.entries(definition.documents??{}).map(([id,step])=>[id,{id,dependencies:step.dependencies}] as const)]);
  if (!definition.name.trim() || definition.name.length > 200 || ids.size !== definition.roles.length
      || nodes.size !== definition.steps.length+Object.keys(definition.controls??{}).length+calls.length+loops.length+Object.keys(definition.documents??{}).length || (!ids.size&&definition.steps.length>0) || !nodes.size || ids.size > 32 || nodes.size > 64
      || definition.roles.some(role => !/^[A-Za-z0-9_-]{1,64}$/.test(role.id) || !workflowRoleInstructionsValid(role.instructions) || !role.provider_id || role.provider_revision < 1)
      || definition.steps.some(step => !step.goal.trim() || !ids.has(step.role_id) || new Set(step.dependencies).size !== step.dependencies.length)) return false;
  for(const step of definition.steps){if(step.execution){if(!step.rights.tools.includes("browser")||!definition.rights.tools.includes("browser")||!definition.roles.find(role=>role.id===step.role_id)?.rights.tools.includes("browser"))return false;if(step.execution.kind==="web_fetch"){try{const url=new URL(step.execution.url);if(!["http:","https:"].includes(url.protocol)||url.username||url.password)return false;}catch{return false;}}else if(!step.execution.query.trim())return false;}}
  const visiting = new Set<string>(); const done = new Set<string>();
  if(definition.mode.mode==="hierarchical"&&definition.mode.manager_step_id&&(!nodes.has(definition.mode.manager_step_id)||nodes.size<2))return false;
  if(definition.roles.some(role=>role.delegate_to.some(id=>!ids.has(id))))return false;
  const visit = (id: string): boolean => {
    if (done.has(id)) return true;
    if (visiting.has(id)) return false;
    const node = nodes.get(id); if (!node) return false;
    visiting.add(id);
    const dependencies = [...node.dependencies];
    if (definition.mode.mode === "sequential") {
      const index = definition.steps.findIndex(step=>step.id===node.id); if (index > 0) dependencies.push(definition.steps[index - 1].id);
    }
    if(definition.mode.mode==="hierarchical"&&id===definition.mode.manager_step_id)dependencies.push(...definition.steps.filter(step=>step.id!==id).map(step=>step.id));
    if (!dependencies.every(visit)) return false;
    visiting.delete(id); done.add(id); return true;
  };
  return [...nodes.keys()].every(visit);
}
/** Keep the same request across uncertain acknowledgement. Never mint another key on retry. */
export class WorkflowRequestLatch<T> {
  private frozen: T | null = null;
  get pending(): boolean {return this.frozen !== null;}
  prepare(build: () => T): T {
    if (this.frozen === null) this.frozen = structuredClone(build());
    return structuredClone(this.frozen);
  }
  clear(): void {this.frozen = null;}
}

export async function prepareWorkflowStart(client:Client,input:WorkflowRunRequest):Promise<WorkflowRunRequest> {
  const checked=await client.preflight(input);
  if(!("workflow" in checked)||!checked.ready||checked.workflow.workflow_id!==input.workflow_id||checked.workflow.version_id!==input.version_id)throw new Error("workflowUnavailable");
  return structuredClone({...input,expected_binding:checked.workflow});
}
