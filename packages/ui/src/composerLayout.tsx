import {useState,type ReactNode} from "react";
import {defaultRunBudgets,type AccessMode,type Budgets,type WorkflowOperation,type WorkflowVersion,type RunRecord} from "@cowork/client";
import type {Key} from "./i18n";
export type ComposerContext={project:string;order:string;run:RunRecord|undefined;version:WorkflowVersion|undefined;selection?:{workflow_id:string;version_id:string;inputs:unknown;access_mode:AccessMode;budgets:Budgets}};
const canonical=(value:unknown):string=>JSON.stringify(value,(_key,item:unknown)=>item&&typeof item==="object"&&!Array.isArray(item)?Object.fromEntries(Object.keys(item).sort().map(key=>[key,(item as Record<string,unknown>)[key]])):item);
export function confirmedWorkflowStart(operation:WorkflowOperation|null,context?:ComposerContext):boolean {
  if(operation?.request.kind!=="start"||operation.state!=="accepted"||operation.receipt?.kind!=="started")return false;
  const request=operation.request.request,receipt=operation.receipt,spec=receipt.spec,binding=spec.workflow;
  if(spec.project_id!==operation.project_id||spec.order_id!==request.order_id||spec.prompt!==request.prompt||spec.access_mode!==(request.access_mode??"confirm_mutations")||!binding||binding.workflow_id!==request.workflow_id||binding.version_id!==request.version_id||request.expected_binding&&canonical(request.expected_binding)!==canonical(binding))return false;
  if(!context)return true;
  const {run,version}=context;
  if(context.project!==operation.project_id||context.order!==request.order_id||!run||run.id!==receipt.run_id||run.created_at!==receipt.created_at||!version||version.project_id!==context.project||version.workflow_id!==request.workflow_id||version.reference.id!==request.version_id||version.reference.sha256!==binding.definition_sha256)return false;
  const requested={...defaultRunBudgets,...request.budgets},defined={...defaultRunBudgets,...version.definition.budgets},effective={...defaultRunBudgets,...spec.budgets};
  if((Object.keys(defaultRunBudgets) as Array<keyof typeof defaultRunBudgets>).some(key=>effective[key]!==Math.min(requested[key],defined[key])))return false;
  return canonical({...run.spec,budgets:{...defaultRunBudgets,...run.spec.budgets}})===canonical({...spec,budgets:effective});
}
/** A displayed authenticated snapshot run is itself an accepted start record.
 * Match the current composer before changing presentation; never synthesize a legacy receipt.
 */
export function confirmedSnapshotWorkflowStart({project,order,run,version,selection}:ComposerContext):boolean {
  const workspace=run?.spec.input_workspace,binding=run?.spec.workflow;
  if(!run||!version||!selection||!workspace?.id||!workspace.input.snapshot_id||!binding
    ||![workspace.input.sha256,workspace.input.manifest_sha256].every(hash=>/^[a-f0-9]{64}$/.test(hash))
    ||run.spec.project_id!==project||run.spec.order_id!==order||version.project_id!==project
    ||binding.workflow_id!==selection.workflow_id||binding.version_id!==selection.version_id
    ||version.workflow_id!==binding.workflow_id||version.reference.id!==binding.version_id||version.reference.sha256!==binding.definition_sha256
    ||run.spec.access_mode!==selection.access_mode||canonical(binding.inputs)!==canonical(selection.inputs))return false;
  const requested={...defaultRunBudgets,...selection.budgets},defined={...defaultRunBudgets,...version.definition.budgets},effective={...defaultRunBudgets,...run.spec.budgets};
  return (Object.keys(defaultRunBudgets) as Array<keyof typeof defaultRunBudgets>).every(key=>effective[key]===Math.min(requested[key],defined[key]));
}
export function canCompactWorkflowComposer({operation,context,uncertainPrepare,pendingSingleRequest,draft,saved,saving,conflict,error,loading}: {
  operation:WorkflowOperation|null;context:ComposerContext;uncertainPrepare:boolean;pendingSingleRequest:boolean;
  draft:string;saved:string;saving:boolean;conflict:boolean;error:unknown;loading:boolean;
}):boolean {
  return (confirmedWorkflowStart(operation,context)||!operation&&confirmedSnapshotWorkflowStart(context))&&!uncertainPrepare&&!pendingSingleRequest&&draft===saved&&!saving&&!conflict&&!error&&!loading;
}
/** A stable form subtree; opening this presentation never sends a command. */
export function ComposerDisclosure({compact,snapshot=false,t,children}:{compact:boolean;snapshot?:boolean;t:(key:Key)=>string;children:ReactNode}) {
  const [expanded,setExpanded]=useState(false);
  return <details className={compact?"locked-composer":"composer-disclosure"} open={!compact||expanded}>
    <summary hidden={!compact} onClick={event=>{event.preventDefault();setExpanded(value=>!value);}}>{t(snapshot?"workflowSnapshotComposer":"workflowLockedComposer")}</summary>
    <p hidden={!compact} className="muted">{t(snapshot?"workflowSnapshotComposerHint":"workflowLockedComposerHint")}</p>{children}</details>;
}
