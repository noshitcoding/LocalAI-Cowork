import {WorkflowWorkspaceRight} from "./WorkflowWorkspaceRight";
import {useState} from "react";
import {useQuery} from "@tanstack/react-query";
import type {Client,WorkflowVersion} from "@cowork/client";
import {useView} from "./view";
import {workspaceManagementKey} from "./workspaceManagementApi";
import {DataTypeSummary} from "./WorkflowDataflowEditor";
import {RunBudgets} from "./RunBudgets";
import {de,en} from "./i18n";
import {completeWorkflowBudgets} from "./workflowUi";
import {addWorkflowSubflow,changeSubflowResult,eligibleSubflowSource,removeWorkflowSubflow,subflowBudget,subflowNodes,subflowResultPaths,type SubflowDraftDefinition} from "./workflowSubflows";

/** Copies a selected immutable library version into the parent source. */
export function WorkflowSubflowsEditor({client,project,value,onChange}:{client:Client;project:string;value:SubflowDraftDefinition;onChange:(value:SubflowDraftDefinition)=>void}){
  const language=useView(state=>state.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const [offset,setOffset]=useState(0),[workflow,setWorkflow]=useState(""),[version,setVersion]=useState(""),[resultNode,setResultNode]=useState(""),[pointer,setPointer]=useState(""),[error,setError]=useState("");
  const library=useQuery({queryKey:["subflow-library",...workspaceManagementKey(client),project,offset],queryFn:()=>client.workflows(project,offset),retry:false});
  const entry=library.data?.items.find(entry=>entry.id===workflow);
  const selected=entry?.versions.find(ref=>ref.id===version);
  const source=useQuery({queryKey:["subflow-version",...workspaceManagementKey(client),project,workflow,version],enabled:!!selected&&!entry?.revoked,queryFn:async()=>{
    const result=await client.workflowVersion(project,workflow,version);
    if(result.project_id!==project||result.workflow_id!==workflow||result.reference.id!==version||result.reference.sha256!==selected?.sha256)throw new Error("workflow_subflow_source_changed");
    return result;
  },retry:false});
  const definition=source.data?.definition as SubflowDraftDefinition|undefined;
  const nodes=definition?subflowNodes(definition):[],chosen=resultNode||nodes.at(-1)?.id||"";
  const output=definition?.dataflow?.steps[chosen]?.output;
  const eligible=!!definition&&!!entry&&!entry.revoked&&!!selected&&!source.isError&&source.data?.reference.sha256===selected.sha256&&eligibleSubflowSource(definition);
  function add(){
    if(!definition)return;
    try{onChange(addWorkflowSubflow(value,definition,{step_id:chosen,pointer},"subflow-"+crypto.randomUUID().slice(0,12)));setError("");}catch(error){setError(error instanceof Error?error.message:String(error));}
  }
  function patch(id:string,patch:Partial<NonNullable<SubflowDraftDefinition["subflows"]>[string]>){const call=value.subflows?.[id];if(call)onChange({...value,subflows:{...value.subflows,[id]:{...call,...patch}}});}
  return <section className="product-stack"><h3>{tr("Unterabläufe","Subflows")}</h3>
    <p>{tr("Füge eine veröffentlichte Version als eingefrorene Kopie ein. Spätere Änderungen der Vorlage ändern diese Kopie nicht. Eingaben werden ausdrücklich im Datenbereich verbunden.","Insert a published version as a frozen copy. Later template changes do not change this copy. Connect inputs explicitly in the data section.")}</p>
    {!value.dataflow&&<p role="status">{tr("Richte zuerst typisierte Übergaben im Datenbereich ein.","Configure typed bindings in the data section first.")}</p>}
    {value.mode.mode==="hierarchical"&&<p role="alert">{tr("Wechsle für Unterabläufe zu einem Graphen mit ausdrücklichen Abhängigkeiten.","Use an explicit dependency graph for subflows.")}</p>}
    <label>{tr("Vorlage aus dieser Projektbibliothek","Template from this project library")}<select aria-label={tr("Vorlage aus dieser Projektbibliothek","Template from this project library")} value={workflow} onChange={event=>{const entry=library.data?.items.find(entry=>entry.id===event.target.value);setWorkflow(event.target.value);setVersion(entry?.versions.at(-1)?.id??"");setResultNode("");setPointer("");setError("");}}><option value="">{tr("Workflow wählen","Choose workflow")}</option>{library.data?.items.map(entry=><option key={entry.id} value={entry.id} disabled={entry.revoked}>{entry.name}{entry.revoked?tr(" · widerrufen"," · revoked"):""}</option>)}</select></label>
    {entry&&<label>{tr("Version der Vorlage","Template version")}<select aria-label={tr("Version der Vorlage","Template version")} value={version} onChange={event=>{setVersion(event.target.value);setResultNode("");setPointer("");}}>{entry.versions.map(ref=><option key={ref.id} value={ref.id}>{ref.revision} · {ref.name}</option>)}</select></label>}
    {(library.isError||source.isError)&&<p role="alert">{tr("Die gewählte Version ist nicht verfügbar. Lade die Bibliothek erneut.","The selected version is unavailable. Reload the library.")}</p>}
    {library.data&&<div className="workflow-toolbar"><button type="button" disabled={!offset} onClick={()=>setOffset(Math.max(0,offset-50))}>{tr("Vorherige Vorlagen","Previous templates")}</button><button type="button" disabled={library.data.next_offset==null} onClick={()=>{if(library.data?.next_offset!=null)setOffset(library.data.next_offset);}}>{tr("Weitere Vorlagen","More templates")}</button></div>}
    {definition&&!eligible&&<p role="alert">{tr("Die Vorlage benötigt passende typisierte Ein- und Ausgaben und eine unterstützte Graphstruktur.","The template needs typed inputs and outputs and a supported graph structure.")}</p>}
    {eligible&&definition&&<><ResultChoice definition={definition} node={chosen} pointer={pointer} onChange={(node,pointer)=>{setResultNode(node);setPointer(pointer);}}/>{output&&<DataTypeSummary kind={output}/>}<SourceSummary version={source.data!}/></>}
    <button type="button" disabled={!eligible||!value.dataflow||value.mode.mode==="hierarchical"||source.isFetching||!!entry?.revoked} onClick={add}>{tr("Version als Unterablauf kopieren","Copy version as subflow")}</button>
    {error&&<p role="alert">{error==="workflow_subflow_limits"?tr("Der gesamte Ablauf überschreitet die Grenze für Knoten, Rollen oder Verschachtelung.","The entire workflow exceeds the node, role or nesting limit."):tr("Prüfe Vorlage, Ergebnis und typisierte Eingaben.","Review the template, result and typed inputs.")}</p>}
    {Object.entries(value.subflows??{}).map(([id,call])=><fieldset className="product-item" key={id}><legend>{call.name}</legend>
      <label>{tr("Name des Unterablaufs","Subflow name")}<input required maxLength={200} value={call.name} onChange={event=>patch(id,{name:event.target.value})}/></label>
      <fieldset><legend>{tr("Erst nach diesen Schritten starten","Start after these steps")}</legend>{subflowNodes(value).filter(node=>node.id!==id).map(node=><label className="product-check" key={node.id}><input type="checkbox" checked={call.dependencies.includes(node.id)} onChange={event=>patch(id,{dependencies:event.target.checked?[...call.dependencies,node.id]:call.dependencies.filter(dependency=>dependency!==node.id)})}/>{node.goal}</label>)}</fieldset>
      <ResultChoice definition={call.definition} node={call.result.step_id} pointer={call.result.pointer} onChange={(step_id,pointer)=>{try{onChange(changeSubflowResult(value,id,{step_id,pointer}));}catch{setError("workflow_subflow_result_required");}}}/>
      <fieldset><legend>{tr("Rechte dieses Aufrufs","Rights for this call")}</legend>
        <WorkflowWorkspaceRight value={call.rights} available={!!value.rights.current_workspace&&!!call.definition.rights.current_workspace} onChange={rights=>patch(id,{rights})}/><p className="muted">{tr("Dieser Aufruf kann die Rechte des Elternablaufs und der eingefrorenen Definition weiter einschränken.","This call can further narrow the rights of the parent workflow and frozen definition.")}</p>
        {(["tools","folders"] as const).map(kind=><fieldset key={kind}><legend>{kind==="tools"?tr("Werkzeuge","Tools"):tr("Ordner","Folders")}</legend>{[...new Set([...value.rights[kind].filter(id=>call.definition.rights[kind].includes(id)),...call.rights[kind]])].map(right=><label className="product-check" key={right}><input type="checkbox" checked={call.rights[kind].includes(right)} onChange={event=>patch(id,{rights:{...call.rights,[kind]:event.target.checked?[...call.rights[kind],right]:call.rights[kind].filter(id=>id!==right)}})}/>{right}{(!value.rights[kind].includes(right)||!call.definition.rights[kind].includes(right))&&<small>{tr(" · durch übergeordnete Rechte gesperrt"," · blocked by enclosing rights")}</small>}</label>)}</fieldset>)}
      </fieldset>
      <RunBudgets value={completeWorkflowBudgets(call.budgets)} setValue={budgets=>patch(id,{budgets})} disabled={false} t={key=>(language==="de"?de:en)[key]} contextTokens={undefined}/>
      <p className="muted">{tr("Wirksame Obergrenzen","Effective upper limits")}: {(()=>{const limit=subflowBudget(subflowBudget(value.budgets,call.definition.budgets),call.budgets);return `${limit.max_seconds} ${tr("Sekunden","seconds")} · ${limit.max_steps} ${tr("Schritte","steps")} · ${limit.max_total_tokens} ${tr("Token","tokens")}`;})()}. {tr("Alle Unterabläufe teilen zusätzlich das verbleibende Budget des Elternlaufs.","All subflows also share the parent run's remaining budget.")}</p>
      <details><summary>{tr("Eingefrorene Definition und Grenzen","Frozen definition and limits")}</summary><p>{call.definition.name}</p><p>{tr("Zeitrahmen in Sekunden","Time limit in seconds")}: {call.budgets.max_seconds} · {tr("Schritte","Steps")}: {call.budgets.max_steps}</p><ol>{subflowNodes(call.definition).map(node=><li key={node.id}>{node.goal}</li>)}</ol><p>{tr("Rechte und Budgets bleiben durch den Elternlauf begrenzt.","The parent run continues to limit rights and budgets.")}</p></details>
      <button type="button" onClick={()=>onChange(removeWorkflowSubflow(value,id))}>{tr("Unterablauf entfernen","Remove subflow")}</button>
    </fieldset>)}
  </section>;
}
function ResultChoice({definition,node,pointer,onChange}:{definition:SubflowDraftDefinition;node:string;pointer:string;onChange:(node:string,pointer:string)=>void}){
  const language=useView(state=>state.language),tr=(de:string,en:string)=>language==="de"?de:en,output=definition.dataflow?.steps[node]?.output;
  return <div><label>{tr("Schritt für die Rückgabe","Return step")}<select aria-label={tr("Schritt für die Rückgabe","Return step")} value={node} onChange={event=>onChange(event.target.value,"")}>{subflowNodes(definition).map(step=><option key={step.id} value={step.id}>{step.goal}</option>)}</select></label><label>{tr("Rückgabefeld","Return field")}<select aria-label={tr("Rückgabefeld","Return field")} value={pointer} onChange={event=>onChange(node,event.target.value)}>{output&&subflowResultPaths(output).map(option=><option key={option.pointer} value={option.pointer}>{option.pointer||tr("Gesamtes Ergebnis","Entire result")}</option>)}</select></label></div>;
}
function SourceSummary({version}:{version:WorkflowVersion}){
  const language=useView(state=>state.language);return <p className="muted">{language==="de"?"Ausgewählte Quelle":"Selected source"}: {version.reference.name} · {version.reference.revision}</p>;
}
