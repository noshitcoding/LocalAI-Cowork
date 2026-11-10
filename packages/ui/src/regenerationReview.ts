import {decode,type BranchOrder,type Client,type Order,type RunRecord,type RunRequest,type WorkflowRunRequest,type WorkflowBinding,type WorkflowDefinition,type WorkflowInputSnapshot} from "@cowork/client";
import {PersistentRequestLatch,type RequestJournalStorage} from "./requestJournal";
import {workspaceJournalKey} from "./workspaceManagementApi";

type WorkflowSource=Pick<WorkflowBinding,"workflow_id"|"version_id"|"definition_sha256">;
export type RegenerationIntent={branch:BranchOrder;template:RunRequest;workflow_source?:WorkflowSource};
export const singleStartJournalKey=(client:Client,project:string,order:string)=>workspaceJournalKey(client,JSON.stringify(["single-start",project,order]));
export const answerCanRegenerate=(run:RunRecord)=>!run.spec.file_action&&(run.spec.workflow
  ?!run.spec.provider&&!!run.spec.input_workspace&&run.spec.executor==="local-v2"&&!run.spec.environment&&!run.spec.mcp_tools?.length
  :!!run.spec.provider);

export function decodeRegenerationIntent(raw:unknown):RegenerationIntent{
  const value=raw as RegenerationIntent;
  if(!value||typeof value!=="object"||Object.keys(value).some(key=>!["branch","template","workflow_source"].includes(key)))throw new Error("request_journal_invalid");
  const branch=decode("BranchOrder",value.branch),template=decode("RunRequest",value.template),source=value.workflow_source;
  if("kind" in template){
    if(!source||Object.keys(source).sort().join(",")!=="definition_sha256,version_id,workflow_id"
      ||source.workflow_id!==template.workflow_id||source.version_id!==template.version_id
      ||typeof source.definition_sha256!=="string"||!/^[a-f0-9]{64}$/.test(source.definition_sha256)
      ||template.expected_binding!=null||template.input_source&&template.inputs!=null)throw new Error("execution_plan_original_mismatch");
    return {branch,template,workflow_source:structuredClone(source)};
  }
  if(source!==undefined)throw new Error("execution_plan_original_mismatch");
  return {branch,template};
}

type WorkflowContext=Pick<WorkflowRunRequest,"order_id"|"base_revision"|"idempotency_key"|"access_mode"|"budgets"|"agent_config"|"folders"|"native_tools"|"mcp_tools"|"environment">;
function safeWorkflowInput(value:unknown,depth=0):boolean{
  if(depth>64)return false;
  if(typeof value==="number")return Number.isFinite(value)&&(!Number.isInteger(value)||Number.isSafeInteger(value));
  if(Array.isArray(value))return value.every(v=>safeWorkflowInput(v,depth+1));
  if(value&&typeof value==="object")return Object.values(value).every(v=>safeWorkflowInput(v,depth+1));
  return true;
}
/** The immutable version is reused; its old scope-dependent plan is never a grant. */
export function workflowRegenerationIntent(run:RunRecord,context:WorkflowContext,prompt:string,snapshot?:WorkflowInputSnapshot):Pick<RegenerationIntent,"template"|"workflow_source">{
  if(!answerCanRegenerate(run)||!run.spec.workflow)throw new Error("regeneration_requires_model_answer");
  if(context.environment||context.native_tools?.length||context.mcp_tools?.length)throw new Error("snapshot_process_or_connector_adapter_required");
  const binding=run.spec.workflow;
  if(snapshot&&(snapshot.source.run_id!==run.id||snapshot.project_id!==run.spec.project_id||snapshot.workflow_id!==binding.workflow_id
    ||snapshot.version_id!==binding.version_id||snapshot.definition_sha256!==binding.definition_sha256))throw new Error("workflow_input_source_mismatch");
  if(!snapshot&&!safeWorkflowInput(binding.inputs))throw new Error("regeneration_workflow_input_transport_required");
  const template=decode("WorkflowRunRequest",{...structuredClone(context),kind:"workflow",prompt,
    workflow_id:binding.workflow_id,version_id:binding.version_id,...(snapshot?{input_source:structuredClone(snapshot.source)}:{inputs:structuredClone(binding.inputs)}),
    folders:structuredClone(context.folders?.filter(folder=>!folder.order_id))});
  return {template,workflow_source:{workflow_id:binding.workflow_id,version_id:binding.version_id,definition_sha256:binding.definition_sha256}};
}

function sorted(value:unknown):unknown{
  if(Array.isArray(value))return value.map(sorted);
  if(value&&typeof value==="object")return Object.fromEntries(Object.entries(value).filter(([,v])=>v!==undefined).sort(([a],[b])=>a.localeCompare(b)).map(([k,v])=>[k,sorted(v)]));
  return value;
}

function orderBoundFolders(definition:WorkflowDefinition,depth=0):boolean{
  if(depth>8)return true;
  const children=[...Object.values(definition.subflows??{}),...Object.values(definition.iterations??{})];
  const rights=[definition.rights,...definition.roles.map(role=>role.rights),...definition.steps.map(step=>step.rights),...children.map(child=>child.rights)];
  return rights.some(rights=>rights.folders.some(id=>id.startsWith("workspace:")))||children.some(child=>orderBoundFolders(child.definition,depth+1));
}

/** Durable hand-off only. The child chat's existing review owns confirmation and dispatch. */
export async function prepareRegenerationReview(client:Client,source:Pick<Order,"id"|"project_id">,original:RegenerationIntent,storage?:RequestJournalStorage):Promise<Order>{
  const validated=decodeRegenerationIntent(structuredClone(original)),template=validated.template;
  if(template.order_id!==source.id)throw new Error("execution_plan_original_mismatch");
  if("kind" in template){
    const version=await client.workflowVersion(source.project_id,template.workflow_id,template.version_id);
    if(version.project_id!==source.project_id||version.workflow_id!==template.workflow_id||version.reference.id!==template.version_id
      ||version.reference.sha256!==validated.workflow_source?.definition_sha256)throw new Error("execution_plan_original_mismatch");
    if(orderBoundFolders(version.definition))throw new Error("regeneration_workflow_workspace_binding_required");
    if(template.input_source){
      const snapshot=await client.workflowInputSnapshot(template.input_source.run_id);
      if(snapshot.project_id!==source.project_id||snapshot.workflow_id!==template.workflow_id||snapshot.version_id!==template.version_id
        ||snapshot.definition_sha256!==validated.workflow_source?.definition_sha256||snapshot.source.sha256!==template.input_source.sha256)throw new Error("workflow_input_source_mismatch");
    }
  }
  const child=await client.one(`/orders/${encodeURIComponent(source.id)}/regenerate`,"Order","POST",original.branch);
  if(child.project_id!==source.project_id||child.id===source.id)throw new Error("workspace_receipt_mismatch");
  // Keep revision 1 and the original key even if a lost branch ACK is recovered later.
  const request=decode("RunRequest",{...template,order_id:child.id,base_revision:1});
  const journal=new PersistentRequestLatch<RunRequest>(singleStartJournalKey(client,child.project_id,child.id),v=>decode("RunRequest",v),storage);
  const saved=journal.prepare(()=>request);
  if(JSON.stringify(sorted(saved))!==JSON.stringify(sorted(request)))throw new Error("execution_plan_original_mismatch");
  return child;
}
