import {Client,type WorkflowDefinition,type WorkflowDraftContent,type WorkflowDraftSnapshot,type WorkflowOperation,type WorkflowVersion,type SaveWorkflowDraft} from "@cowork/client";
import {workflowDraftKey,restoreWorkflowEditor} from "./workflowJournal";
import {completeWorkflowBudgets,editableWorkflow} from "./workflowUi";

export type SnapshotReason="pending"|"binding"|"unsupported"|null;
export type WorkflowSnapshotRead={operation:WorkflowOperation;snapshot:WorkflowDraftSnapshot|null;version:WorkflowVersion|null;definition:WorkflowDefinition|null;reason:SnapshotReason};
const rights=(value:WorkflowDefinition["rights"])=>({tools:[...new Set(value.tools)].sort(),folders:[...new Set(value.folders)].sort()});
/** Compare domain meaning only. Never apply these normalizations to saved editor data. */
export function workflowDefinitionKey(value:WorkflowDefinition){
  return workflowDraftKey({...value,budgets:completeWorkflowBudgets(value.budgets),rights:rights(value.rights),
    roles:value.roles.map(role=>({...role,budgets:completeWorkflowBudgets(role.budgets),rights:rights(role.rights),delegate_to:[...new Set(role.delegate_to)].sort()})),
    steps:value.steps.map(step=>({...step,budgets:completeWorkflowBudgets(step.budgets),rights:rights(step.rights)}))});
}
export const workflowSnapshotIdentity=(operation:WorkflowOperation)=>workflowDraftKey({
  id:operation.id,project_id:operation.project_id,intent_id:operation.intent_id,request_sha256:operation.request_sha256,request:operation.request,draft_snapshot:operation.draft_snapshot
});
/** Server GET recomputes the full Rust content hash; bind that receipt, without inventing a JSON hash algorithm. */
export async function readWorkflowOperationSnapshot(client:Client,project:string,expected:WorkflowOperation):Promise<WorkflowSnapshotRead>{
  const frozen=structuredClone(expected);
  const current=await client.workflowOperation(project,frozen.id);
  const invalid:WorkflowSnapshotRead={operation:frozen,snapshot:null,version:null,definition:null,reason:"binding"};
  if(current.project_id!==project||frozen.project_id!==project||workflowSnapshotIdentity(current)!==workflowSnapshotIdentity(frozen))return invalid;
  if(current.state==="pending")return {...invalid,operation:current,reason:"pending"};
  if(current.request.kind!=="create"&&current.request.kind!=="publish")return {...invalid,operation:current};
  const ref=current.draft_snapshot;
  if(!ref||!ref.draft_id||!ref.snapshot_id||!/^[0-9a-f]{64}$/.test(ref.sha256))return {...invalid,operation:current};
  const snapshot=await client.workflowDraftSnapshot(project,ref.draft_id,ref.snapshot_id);
  if(snapshot.project_id!==project||snapshot.draft_id!==ref.draft_id||snapshot.id!==ref.snapshot_id||snapshot.sha256!==ref.sha256)return {...invalid,operation:current};
  const result:WorkflowSnapshotRead={operation:current,snapshot,version:null,definition:null,reason:"binding"};
  const definition=restoreWorkflowEditor(snapshot.content);
  if(!definition||!editableWorkflow(definition))return {...result,reason:"unsupported"};
  if(workflowDefinitionKey(definition)!==workflowDefinitionKey(current.request.request.definition))return result;
  const origin=snapshot.content.origin??null;
  if(current.request.kind==="create"){
    if(origin)return result;
  }else{
    if(!origin||origin.workflow_id!==current.request.workflow_id||origin.base_revision!==current.request.request.base_revision||!origin.version_id||origin.base_revision<1)return result;
    const version=await client.workflowVersion(project,origin.workflow_id,origin.version_id);
    result.version=version;
    if(version.project_id!==project||version.workflow_id!==origin.workflow_id||version.reference.id!==origin.version_id
      ||version.reference.revision<1||version.reference.revision>origin.base_revision||version.reference.name!==version.definition.name
      ||!/^[0-9a-f]{64}$/.test(version.reference.sha256))return {...result,version:null};
  }
  return {...result,definition,reason:null};
}
/** An old client/project/operation read cannot create a draft in the next context. */
export class WorkflowSnapshotReader{
  private active=true;private sequence=0;private expected:WorkflowOperation;
  constructor(private client:Client,private project:string,expected:WorkflowOperation){this.expected=structuredClone(expected);}
  activate(){this.active=true;this.sequence++;}
  dispose(){this.active=false;this.sequence++;}
  async read(){const sequence=++this.sequence;try{const value=await readWorkflowOperationSnapshot(this.client,this.project,this.expected);return this.active&&sequence===this.sequence?value:null;}catch(error){if(!this.active||sequence!==this.sequence)return null;throw error;}}
}
export function independentSnapshotContent(value:WorkflowSnapshotRead):WorkflowDraftContent{
  if(value.reason!==null||!value.snapshot||!value.definition||value.operation.state==="pending")throw new Error("workflowSnapshotReadOnly");
  return structuredClone({...value.snapshot.content,archived:false});
}

/** A user-requested copy freezes a fresh draft CAS, independently of the source operation. */
export function prepareIndependentSnapshotCopy(value:WorkflowSnapshotRead):SaveWorkflowDraft{
  return {id:crypto.randomUUID(),base_revision:0,idempotency_key:crypto.randomUUID(),content:independentSnapshotContent(value)};
}
