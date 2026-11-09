import type {WorkflowDataType} from "@cowork/client";

export const automationInputMessage=(code:string,language:"de"|"en")=>{
  const messages:Record<string,[string,string]>={
    automation_inputs_invalid:["Prüfe die Feldzuordnungen. Ziele müssen vorhanden sein und dürfen sich nicht überschneiden.","Review field bindings. Targets must exist and cannot overlap."],
    automation_inputs_workflow_required:["Die Zuordnung benötigt eine Aufgabe mit typisiertem Workflow.","Bindings require a task with a typed workflow."],
    automation_inputs_value_missing:["Ein zugeordnetes Ereignisfeld oder ein gewählter Listeneintrag fehlt.","A mapped event field or selected list item is missing."],
    automation_inputs_type_mismatch:["Die abgeleiteten Werte passen nicht zu den Workflow-Eingaben.","The derived values do not match the workflow inputs."],
    automation_inputs_workflow_unavailable:["Die gebundene Workflowversion ist nicht mehr verfügbar oder wurde gesperrt.","The bound workflow version is unavailable or revoked."],
    automation_inputs_snapshot_mismatch:["Die Eingaben stimmen nicht mehr mit ihrem ursprünglichen Ereignisbeleg überein.","Inputs no longer match the original event receipt."],
  };
  return messages[code]?.[language==="de"?0:1]??code;
};
export function automationInputTargets(kind:WorkflowDataType,saved:unknown):{pointer:string;label:string;kind:WorkflowDataType}[]{
  const result:{pointer:string;label:string;kind:WorkflowDataType}[]=[];
  const visit=(kind:WorkflowDataType,value:unknown,pointer:string,labels:string[],depth:number)=>{
    if(result.length>=512||depth>8)return;
    result.push({pointer,label:labels.join(" → "),kind});
    if(kind.type==="object")for(const [name,child] of Object.entries(kind.fields))visit(child,value&&typeof value==="object"&&Object.hasOwn(value,name)?(value as Record<string,unknown>)[name]:undefined,`${pointer}/${name}`,[...labels,name],depth+1);
    if(kind.type==="list"&&Array.isArray(value))value.slice(0,kind.max_items).forEach((item,index)=>visit(kind.item,item,`${pointer}/${index}`,[...labels,String(index+1)],depth+1));
  };
  visit(kind,saved,"",[],0);return result;
}
export const eventFieldPointer=(name:string)=>`/${name.replaceAll("~","~0").replaceAll("/","~1")}`;
export const automationTargetConflict=(target:string,bindings:Record<string,string>,ignore?:string)=>Object.keys(bindings).filter(key=>key!==ignore).some(key=>key===target||!key||!target||key.startsWith(`${target}/`)||target.startsWith(`${key}/`));
