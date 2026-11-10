import {useQuery} from "@tanstack/react-query";
import type {AutomationEventSource,AutomationWorkflowInputs,Client,WorkspaceTask} from "@cowork/client";
import {useView} from "./view";
import {workspaceManagementKey} from "./workspaceManagementApi";
import {automationInputTargets,eventFieldPointer,automationTargetConflict} from "./automationInputUi";
import {WorkflowDataValuePreview} from "./WorkflowDataflowEditor";

export function AutomationWorkflowInputsEditor({client,project,task,source,value,onChange,eventFields}:{client:Client;project:string;task:WorkspaceTask|undefined;source:AutomationEventSource;value:AutomationWorkflowInputs|null;onChange:(value:AutomationWorkflowInputs|null)=>void;eventFields:string[]}){
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const execution=task?.value.execution;
  const workflow=execution?.kind==="workflow"?execution:null;
  const version=useQuery({queryKey:["automation-workflow-input-schema",...workspaceManagementKey(client),project,workflow?.workflow_id,workflow?.version_id],enabled:!!workflow,retry:false,queryFn:async()=>{
    if(!workflow)throw new Error("automation_inputs_workflow_required");
    const result=await client.workflowVersion(project,workflow.workflow_id,workflow.version_id);
    if(result.project_id!==project||result.workflow_id!==workflow.workflow_id||result.reference.id!==workflow.version_id)throw new Error("workflowUnavailable");return result;
  }});
  const type=version.data?.definition.dataflow?.input;
  if(!workflow)return <p>{tr("Ereignisfelder können mit einer gespeicherten Aufgabe verknüpft werden, die einen typisierten Workflow verwendet.","Event fields can be bound to a saved task that uses a typed workflow.")}{value&&<button type="button" onClick={()=>onChange(null)}>{tr("Unpassende Zuordnung entfernen","Remove incompatible bindings")}</button>}</p>;
  if(version.isError)return <p role="alert">{tr("Workflow-Eingaben konnten nicht geladen werden.","Could not load workflow input fields.")}</p>;
  if(version.isPending)return <p>{tr("Workflow-Eingaben werden geladen …","Loading workflow inputs …")}</p>;
  if(!type)return <p>{tr("Dieser Ablauf hat noch keine typisierten Eingabefelder. Richte sie zuerst in einer neuen Workflowversion ein.","This workflow has no typed inputs yet. Configure them in a new workflow version first.")}{value&&<button type="button" onClick={()=>onChange(null)}>{tr("Zuordnung entfernen","Remove bindings")}</button>}</p>;
  const targets=automationInputTargets(type,workflow.inputs),bindings=value?.bindings??{};
  const knownSources=new Map<string,string>([["",tr("Gesamtes Ereignis","Entire event")],...eventFields.filter(Boolean).map(name=>[eventFieldPointer(name),name] as [string,string])]);
  if(source==="file")for(const [pointer,de,en] of [["/path","Dateipfad","File path"],["/sha256","Inhaltsfingerprint","Content fingerprint"],["/previous_sha256","Vorheriger Inhaltsfingerprint","Previous content fingerprint"],["/folder_id","Freigegebener Ordner","Permitted folder"],["/resource_id","Native Ordnerkennung","Native folder identifier"],["/change","Änderungsart","Change kind"],["/size","Dateigröße","File size"]])knownSources.set(pointer,tr(de,en));
  const patch=(next:Record<string,string>)=>onChange({format_version:1,bindings:next});
  return <fieldset className="product-item"><legend>{tr("Workflow-Eingaben aus Ereignisfeldern","Workflow inputs from event fields")}</legend>
    <label className="product-check"><input type="checkbox" checked={!!value} onChange={event=>onChange(event.target.checked?{format_version:1,bindings:{}}:null)}/>{tr("Eingaben ausdrücklich zuordnen","Map inputs explicitly")}</label>
    {value?<><p>{tr("Nicht zugeordnete Eingaben behalten die festen Aufgabenwerte. Die übrigen Ereignisfelder werden nicht zusätzlich in den Auftragstext kopiert. Felder, die fehlen oder den falschen Typ haben, blockieren den Start.","Unmapped inputs keep their saved task values. Other event fields are not additionally copied into the task prompt. Missing fields or wrong types block the start.")}</p>
      {Object.entries(bindings).map(([target,pointer],index)=><div className="product-item" key={index}><label>{tr("Workflow-Eingabefeld","Workflow input field")}<select aria-label={tr("Workflow-Eingabefeld","Workflow input field")} value={target} onChange={event=>{const next=event.target.value;if(automationTargetConflict(next,bindings,target))return;patch(Object.fromEntries(Object.entries(bindings).map(([key,value])=>[key===target?next:key,value])));}}>{!targets.some(item=>item.pointer===target)&&<option value={target}>{target} · {tr("Nicht mehr vorhanden","No longer available")}</option>}{targets.map(item=><option key={item.pointer} value={item.pointer} disabled={automationTargetConflict(item.pointer,bindings,target)}>{item.label||tr("Gesamte Workflow-Eingabe","Entire workflow input")}</option>)}</select></label>
        <label>{tr("Ereignisfeld","Event field")}<select aria-label={tr("Ereignisfeld","Event field")} value={knownSources.has(pointer)?pointer:"@custom"} onChange={event=>patch({...bindings,[target]:event.target.value==="@custom"?"/":event.target.value})}>{Array.from(knownSources).map(([key,label])=><option key={key} value={key}>{label}</option>)}<option value="@custom">{tr("Anderes / verschachteltes Feld …","Other / nested field …")}</option></select></label>
        {!knownSources.has(pointer)&&<label>{tr("Pfad im Ereignis, z. B. /invoice/total","Event path, e.g. /invoice/total")}<input required maxLength={512} value={pointer} onChange={event=>patch({...bindings,[target]:event.target.value})}/></label>}
        <button type="button" onClick={()=>patch(Object.fromEntries(Object.entries(bindings).filter(([key])=>key!==target)))}>{tr("Zuordnung entfernen","Remove binding")}</button></div>)}
      <button type="button" disabled={Object.keys(bindings).length>=64||targets.every(item=>automationTargetConflict(item.pointer,bindings))} onClick={()=>{const next=targets.find(item=>item.pointer&&!automationTargetConflict(item.pointer,bindings))??targets.find(item=>!automationTargetConflict(item.pointer,bindings));if(next)patch({...bindings,[next.pointer]:source==="file"?"/path":eventFields[0]?eventFieldPointer(eventFields[0]):"/subject"});}}>{tr("Feld zuordnen","Map field")}</button>
      <details><summary>{tr("Feste Aufgabenwerte anzeigen","Show fixed task values")}</summary><WorkflowDataValuePreview value={workflow.inputs}/></details>
    </>:<p>{tr("Ohne diese Zuordnung verwendet der bestehende Ereignispfad die festen Aufgabenwerte und fügt Ereignisdaten als Kontext an.","Without these bindings, the existing event path uses fixed task values and appends event data as context.")}</p>}
  </fieldset>;
}
