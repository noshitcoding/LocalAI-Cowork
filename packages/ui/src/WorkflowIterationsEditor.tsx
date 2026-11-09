import {WorkflowWorkspaceRight} from "./WorkflowWorkspaceRight";
import {useState} from "react";
import {useQuery} from "@tanstack/react-query";
import type {Client,WorkflowDefinition,WorkflowEmbeddedIteration,WorkflowIterationArgument,WorkflowValueBinding,WorkflowDataType} from "@cowork/client";
import {useView} from "./view";
import {workspaceManagementKey} from "./workspaceManagementApi";
import {DataTypeSummary,WorkflowDataValueEditor} from "./WorkflowDataflowEditor";
import {RunBudgets} from "./RunBudgets";
import {de,en} from "./i18n";
import {completeWorkflowBudgets} from "./workflowUi";
import {dataTypeAt,defaultDataValue,sameDataType,workflowDataNodes} from "./workflowDataflow";
import {eligibleSubflowSource,subflowNodes,subflowResultPaths,subflowBudget} from "./workflowSubflows";
import {addWorkflowIteration,changeIterationResult,defaultIterationArguments,iterationInputType,removeWorkflowIteration,validWorkflowIterations} from "./workflowIterations";

export function WorkflowIterationsEditor({client,project,value,onChange}:{client:Client;project:string;value:WorkflowDefinition;onChange:(value:WorkflowDefinition)=>void}){
  const language=useView(state=>state.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const [offset,setOffset]=useState(0),[workflow,setWorkflow]=useState(""),[version,setVersion]=useState(""),[resultNode,setResultNode]=useState(""),[resultPointer,setResultPointer]=useState(""),[collectionKey,setCollectionKey]=useState(""),[primary,setPrimary]=useState(""),[error,setError]=useState("");
  const library=useQuery({queryKey:["subflow-library",...workspaceManagementKey(client),project,offset],queryFn:()=>client.workflows(project,offset),retry:false});
  const entry=library.data?.items.find(entry=>entry.id===workflow),selected=entry?.versions.find(ref=>ref.id===version);
  const source=useQuery({queryKey:["subflow-version",...workspaceManagementKey(client),project,workflow,version],enabled:!!selected&&!entry?.revoked,queryFn:async()=>{
    const result=await client.workflowVersion(project,workflow,version);
    if(result.project_id!==project||result.workflow_id!==workflow||result.reference.id!==version||result.reference.sha256!==selected?.sha256)throw new Error("workflow_iteration_source_changed");
    return result;
  },retry:false});
  const body=source.data?.definition,nodes=body?subflowNodes(body):[],chosen=resultNode||nodes.at(-1)?.id||"";
  const eligible=!!body&&!!selected&&!entry?.revoked&&!source.isError&&source.data?.reference.sha256===selected.sha256&&eligibleSubflowSource(body);
  const collections:{key:string;label:string;binding:WorkflowValueBinding}[]=[];
  if(value.dataflow){
    for(const option of subflowResultPaths(value.dataflow.input).filter(option=>option.kind.type==="list"))collections.push({key:JSON.stringify(["input",option.pointer]),label:tr("Eingabe","Input")+": "+(option.pointer||"/"),binding:{value_type:option.kind,value:{source:"input",pointer:option.pointer}}});
    for(const node of workflowDataNodes(value))for(const option of subflowResultPaths(value.dataflow.steps[node.id]?.output??{type:"null"}).filter(option=>option.kind.type==="list"))collections.push({key:JSON.stringify([node.id,option.pointer]),label:node.goal+": "+(option.pointer||tr("gesamtes Ergebnis","entire result")),binding:{value_type:option.kind,value:{source:"step",step_id:node.id,pointer:option.pointer}}});
  }
  const collection=collections.find(option=>option.key===collectionKey)??collections[0],item=collection?.binding.value_type.type==="list"?collection.binding.value_type.item:undefined;
  const fields=body?.dataflow?.input.type==="object"?body.dataflow.input.fields:{};
  const matching=Object.entries(fields).filter(([,kind])=>!!item&&sameDataType(kind,item)),primaryName=primary||matching[0]?.[0]||"none";
  function add(){if(!body||!collection)return;try{
    const args=defaultIterationArguments(body);if(primaryName&&item&&fields[primaryName]&&sameDataType(fields[primaryName],item))args[primaryName]={source:"item"};
    onChange(addWorkflowIteration(value,body,{step_id:chosen,pointer:resultPointer},"list-"+crypto.randomUUID().slice(0,12),collection.binding,args));setError("");
  }catch{setError(tr("Prüfe Listentyp, Vorlage und Rückgabefeld.","Review the list type, template and return field."));}}
  function patch(id:string,patch:Partial<WorkflowEmbeddedIteration>){const call=value.iterations?.[id];if(call)onChange({...value,iterations:{...value.iterations,[id]:{...call,...patch}}});}
  return <section className="product-stack"><h3>{tr("Listen verarbeiten","Process lists")}</h3>
    <p>{tr("Führe eine eingefrorene Vorlage für jeden Eintrag aus. Ergebnisse bleiben in der ursprünglichen Reihenfolge. Alle Einträge teilen die Grenzen dieses Ablaufs.","Run a frozen template for each entry. Results retain their original order. All entries share this workflow's limits.")}</p>
    {!value.dataflow&&<p role="status">{tr("Richte zuerst typisierte Eingaben im Datenbereich ein.","Configure typed inputs in the data section first.")}</p>}
    {value.mode.mode==="hierarchical"&&<p role="status">{tr("Verwende für Listen einen Graphen mit ausdrücklichen Abhängigkeiten.","Use an explicit dependency graph for lists.")}</p>}
    <label>{tr("Vorlage für Listeneinträge","Template for list entries")}<select aria-label={tr("Vorlage für Listeneinträge","Template for list entries")} value={workflow} onChange={event=>{const selected=library.data?.items.find(entry=>entry.id===event.target.value);setWorkflow(event.target.value);setVersion(selected?.versions.at(-1)?.id??"");setResultNode("");setResultPointer("");setPrimary("");}}><option value="">{tr("Workflow wählen","Choose workflow")}</option>{library.data?.items.map(entry=><option key={entry.id} value={entry.id} disabled={entry.revoked}>{entry.name}{entry.revoked?tr(" · widerrufen"," · revoked"):""}</option>)}</select></label>
    {entry&&<label>{tr("Version für Listeneinträge","Version for list entries")}<select aria-label={tr("Version für Listeneinträge","Version for list entries")} value={version} onChange={event=>{setVersion(event.target.value);setResultNode("");setResultPointer("");setPrimary("");}}>{entry.versions.map(version=><option key={version.id} value={version.id}>{version.revision} · {version.name}</option>)}</select></label>}
    {library.data&&<div className="workflow-toolbar"><button type="button" disabled={!offset} onClick={()=>setOffset(Math.max(0,offset-50))}>{tr("Vorherige Vorlagen","Previous templates")}</button><button type="button" disabled={library.data.next_offset==null} onClick={()=>{if(library.data?.next_offset!=null)setOffset(library.data.next_offset);}}>{tr("Weitere Vorlagen","More templates")}</button></div>}
    {(library.isError||source.isError)&&<p role="alert">{tr("Die ausgewählte Version ist nicht verfügbar. Lade die Bibliothek erneut.","The selected version is unavailable. Reload the library.")}</p>}
    {body&&!eligible&&<p role="status">{tr("Wähle eine Vorlage mit typisierten Eingaben und Ergebnissen ohne weitere Listen oder dynamische Delegation.","Choose a template with typed inputs and results, without nested lists or dynamic delegation.")}</p>}
    <label>{tr("Liste aus einer Eingabe oder einem Ergebnis","List from an input or result")}<select aria-label={tr("Liste aus einer Eingabe oder einem Ergebnis","List from an input or result")} value={collection?.key??""} onChange={event=>{setCollectionKey(event.target.value);setPrimary("");}}><option value="" disabled>{tr("Typisierte Liste wählen","Choose a typed list")}</option>{collections.map(option=><option key={option.key} value={option.key}>{option.label}</option>)}</select></label>
    {!collections.length&&<p role="status">{tr("Lege im Datenbereich zuerst eine Liste als Eingabe oder Schrittergebnis fest.","Declare a list input or step result in the data section first.")}</p>}
    {eligible&&body&&<><IterationResult definition={body} node={chosen} pointer={resultPointer} onChange={(node,pointer)=>{setResultNode(node);setResultPointer(pointer);}}/>
      <label>{tr("Gesamten Eintrag an dieses Vorlagenfeld binden","Bind the entire entry to this template field")}<select aria-label={tr("Gesamten Eintrag an dieses Vorlagenfeld binden","Bind the entire entry to this template field")} value={primaryName} onChange={event=>setPrimary(event.target.value||"none")}><option value="none">{tr("Eingaben nach dem Einfügen festlegen","Configure inputs after insertion")}</option>{matching.map(([name])=><option key={name} value={name}>{name}</option>)}</select></label>
      <p className="muted">{tr("Weitere Felder, Index und gemeinsame Werte lassen sich unter Eingaben pro Eintrag zuordnen. Gleichnamige Felder werden nicht automatisch übernommen.","Map additional fields, the index and shared values under Inputs per entry. Matching field names are not copied automatically.")}</p></>}
    <button type="button" disabled={!eligible||!collection||!value.dataflow||value.mode.mode==="hierarchical"||source.isFetching} onClick={add}>{tr("Version als Listenablauf kopieren","Copy version as list workflow")}</button>
    {error&&<p role="alert">{error}</p>}
    {Object.entries(value.iterations??{}).map(([id,call])=>{
      const input=iterationInputType(value,id),collection=value.dataflow?.steps[id]?.inputs[call.collection_input]?.value_type,item=collection?.type==="list"?collection.item:undefined,fields=call.definition.dataflow?.input.type==="object"?call.definition.dataflow.input.fields:{};
      return <fieldset className="product-item" key={id}><legend>{call.name}</legend>
        <label>{tr("Name der Listenverarbeitung","List workflow name")}<input required maxLength={200} value={call.name} onChange={event=>patch(id,{name:event.target.value})}/></label>
        <fieldset><legend>{tr("Erst nach diesen Schritten starten","Start after these steps")}</legend>{workflowDataNodes(value).filter(node=>node.id!==id).map(node=><label className="product-check" key={node.id}><input type="checkbox" checked={call.dependencies.includes(node.id)} onChange={event=>patch(id,{dependencies:event.target.checked?[...call.dependencies,node.id]:call.dependencies.filter(dependency=>dependency!==node.id)})}/>{node.goal}</label>)}</fieldset>
        <label>{tr("Gleichzeitig aktive Einträge","Concurrent active entries")}<input type="number" min={1} max={32} step={1} required value={call.max_parallel} onChange={event=>patch(id,{max_parallel:Number(event.target.value)})}/></label>
        <p className="muted">{tr("Wartende Einträge behalten ihren Platz. Bei einem Platz werden Einträge nacheinander in Eingangsreihenfolge bearbeitet. Zusätzlich gelten die Parallelitätsgrenzen des gesamten Ablaufs.","Waiting entries keep their slot. One slot processes entries sequentially in input order. The overall workflow's concurrency limits also apply.")}</p>
        <h4>{tr("Eingaben pro Eintrag","Inputs per entry")}</h4>
        {Object.entries(fields).map(([name,kind])=><IterationArgument key={name} name={name} kind={kind} item={item} input={input} value={call.arguments[name]??{source:"literal",value:defaultDataValue(kind)}} onChange={arg=>patch(id,{arguments:{...call.arguments,[name]:arg}})}/>)}
        <IterationResult definition={call.definition} node={call.result.step_id} pointer={call.result.pointer} onChange={(step_id,pointer)=>{try{onChange(changeIterationResult(value,id,{step_id,pointer}));}catch{setError(tr("Wähle ein vorhandenes Rückgabefeld.","Choose an existing return field."));}}}/>
        <p>{tr("Die Liste und gemeinsame Eingaben verbindest du im Datenbereich dieses Bausteins.","Connect the list and shared inputs in this component's data section.")}</p>
        <fieldset><legend>{tr("Rechte der Listenverarbeitung","List workflow rights")}</legend><WorkflowWorkspaceRight value={call.rights} available={!!value.rights.current_workspace&&!!call.definition.rights.current_workspace} onChange={rights=>patch(id,{rights})}/>{(["tools","folders"] as const).map(kind=><fieldset key={kind}><legend>{kind==="tools"?tr("Werkzeuge","Tools"):tr("Ordner","Folders")}</legend>{[...new Set([...value.rights[kind].filter(right=>call.definition.rights[kind].includes(right)),...call.rights[kind]])].map(right=><label className="product-check" key={right}><input type="checkbox" checked={call.rights[kind].includes(right)} onChange={event=>patch(id,{rights:{...call.rights,[kind]:event.target.checked?[...call.rights[kind],right]:call.rights[kind].filter(id=>id!==right)}})}/>{right}</label>)}</fieldset>)}</fieldset>
        <RunBudgets value={completeWorkflowBudgets(call.budgets)} setValue={budgets=>patch(id,{budgets})} disabled={false} t={key=>(language==="de"?de:en)[key]} contextTokens={undefined}/>
        <p className="muted">{tr("Gemeinsame Obergrenzen dieser Liste","Shared upper limits for this list")}: {(()=>{const limit=subflowBudget(value.budgets,call.budgets);return `${limit.max_steps} ${tr("Schritte","steps")} · ${limit.max_total_tokens} ${tr("Token","tokens")} · ${limit.max_seconds} ${tr("Sekunden","seconds")}`;})()}. {tr("Für jeden einzelnen Eintrag gelten zusätzlich die Grenzen der eingefrorenen Vorlage.","The frozen template's limits also apply to each individual entry.")}</p>
        <details><summary>{tr("Eingefrorene Vorlage","Frozen template")}</summary><p>{call.definition.name}</p><ol>{subflowNodes(call.definition).map(node=><li key={node.id}>{node.goal}</li>)}</ol></details>
        <button type="button" onClick={()=>onChange(removeWorkflowIteration(value,id))}>{tr("Listenablauf entfernen","Remove list workflow")}</button>
      </fieldset>;
    })}
    {!!Object.keys(value.iterations??{}).length&&!validWorkflowIterations(value)&&<p role="alert">{tr("Prüfe Listentyp, Eingaben pro Eintrag, Rückgabe und Grenzen.","Review the list type, inputs per entry, return value and limits.")}</p>}
  </section>;
}
function IterationResult({definition,node,pointer,onChange}:{definition:WorkflowDefinition;node:string;pointer:string;onChange:(node:string,pointer:string)=>void}){
  const language=useView(state=>state.language),tr=(de:string,en:string)=>language==="de"?de:en,output=definition.dataflow?.steps[node]?.output;
  return <div><label>{tr("Ergebnisschritt pro Eintrag","Result step per entry")}<select aria-label={tr("Ergebnisschritt pro Eintrag","Result step per entry")} value={node} onChange={event=>onChange(event.target.value,"")}>{subflowNodes(definition).map(node=><option key={node.id} value={node.id}>{node.goal}</option>)}</select></label><label>{tr("Rückgabefeld pro Eintrag","Return field per entry")}<select aria-label={tr("Rückgabefeld pro Eintrag","Return field per entry")} value={pointer} onChange={event=>onChange(node,event.target.value)}>{output&&subflowResultPaths(output).map(option=><option key={option.pointer} value={option.pointer}>{option.pointer||tr("Gesamtes Ergebnis","Entire result")}</option>)}</select></label></div>;
}
function IterationArgument({name,kind,item,input,value,onChange}:{name:string;kind:WorkflowDataType;item:WorkflowDataType|undefined;input:WorkflowDataType|undefined;value:WorkflowIterationArgument;onChange:(value:WorkflowIterationArgument)=>void}){
  const language=useView(state=>state.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const paths=(source:WorkflowDataType|undefined)=>source?subflowResultPaths(source).filter(option=>sameDataType(option.kind,kind)):[];
  function select(source:WorkflowIterationArgument["source"]){onChange(source==="literal"?{source,value:defaultDataValue(kind)}:source==="index"?{source}:source==="item"?{source,pointer:paths(item)[0]?.pointer??""}:{source,pointer:paths(input)[0]?.pointer??""});}
  const pointer=value.source==="item"?value.pointer??"":value.source==="input"?value.pointer:undefined,source=value.source==="item"?item:value.source==="input"?input:undefined;
  const current=source&&pointer!==undefined?dataTypeAt(source,pointer):undefined;
  return <fieldset><legend>{name}</legend><DataTypeSummary kind={kind}/><label>{tr("Quelle für dieses Vorlagenfeld","Source for this template field")}<select aria-label={tr("Quelle für dieses Vorlagenfeld","Source for this template field")} value={value.source} onChange={event=>select(event.target.value as WorkflowIterationArgument["source"])}><option value="item" disabled={!item}>{tr("Aktueller Listeneintrag","Current list entry")}</option><option value="index" disabled={kind.type!=="integer"}>{tr("Ursprünglicher Index ab 0","Original index starting at 0")}</option><option value="input" disabled={!input}>{tr("Gemeinsam übergebene Eingabe","Shared explicit input")}</option><option value="literal">{tr("Konstanter Wert","Constant value")}</option></select></label>
    {value.source==="literal"?<WorkflowDataValueEditor label={tr("Wert dieses Felds","Value for this field")} kind={kind} value={value.value} onChange={value=>onChange({source:"literal",value})}/>:pointer!==undefined&&<label>{tr("Feld im gewählten Wert; leer für den gesamten Wert","Field in selected value; empty for the entire value")}<input value={pointer} maxLength={1024} placeholder="/amount" onChange={event=>onChange(value.source==="item"?{source:"item",pointer:event.target.value}:{source:"input",pointer:event.target.value})}/>{(!current||!sameDataType(current,kind))&&<small role="alert">{tr("Dieser Pfad fehlt oder hat einen anderen Typ.","This path is missing or has a different type.")}</small>}</label>}
  </fieldset>;
}
