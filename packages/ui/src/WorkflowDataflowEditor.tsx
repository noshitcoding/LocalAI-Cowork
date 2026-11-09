import {iterationOutputType} from "./workflowIterations";
import {useEffect,useState} from "react";
import {useQuery} from "@tanstack/react-query";
import type {Client,WorkspaceTaskExecution,WorkflowDataType,WorkflowDefinition,WorkflowValueBinding,WorkflowStepData} from "@cowork/client";
import {workspaceManagementKey} from "./workspaceManagementApi";
import {useView} from "./view";
import {dataFieldName,dataTypeAt,defaultDataValue,defaultStepData,enableWorkflowDataflow,sameDataType,validDataType,validWorkflowDataflow,workflowDataAncestors,workflowDataNodes} from "./workflowDataflow";

type Kind=WorkflowDataType["type"];
const kindLabels:Record<Kind,[string,string]>={text:["Text","Text"],number:["Zahl","Number"],integer:["Ganzzahl","Integer"],boolean:["Wahr/Falsch","True/false"],null:["Kein Wert","Null"],object:["Objekt mit Feldern","Object with fields"],list:["Liste","List"],nullable:["Optionaler Wert (oder null)","Nullable value"]};
function newType(type:Kind):WorkflowDataType {
  if(type==="object")return {type,fields:{}};
  if(type==="list")return {type,item:{type:"text"},max_items:20};
  if(type==="nullable")return {type,value:{type:"text"}};
  return {type};
}
export function DataTypeSummary({kind,depth=0}:{kind:WorkflowDataType;depth?:number}){
  const language=useView(v=>v.language);
  if(depth>8)return null;
  return <div>{kindLabels[kind.type][language==="de"?0:1]}{kind.type==="object"&&<ul>{Object.entries(kind.fields).map(([key,type])=><li key={key}><strong>{key}</strong>: <DataTypeSummary kind={type} depth={depth+1}/></li>)}</ul>}{kind.type==="list"&&<> (≤ {kind.max_items}) <DataTypeSummary kind={kind.item} depth={depth+1}/></>}{kind.type==="nullable"&&<>: <DataTypeSummary kind={kind.value} depth={depth+1}/></>}</div>;
}
export function WorkflowDataflowSummary({definition}:{definition:WorkflowDefinition}){
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en,data=definition.dataflow;
  if(!data)return null;
  return <details><summary>{tr("Eingabetypen und Datenverbindungen","Input types and data bindings")}</summary><p>{tr("Workflow-Eingabe","Workflow input")}</p><DataTypeSummary kind={data.input}/>{workflowDataNodes(definition).map((step,index)=>{
    const config=data.steps[step.id];if(!config)return null;
    return <section className="product-item" key={step.id}><h4>{index+1}. {step.goal}</h4>{definition.steps.some(model=>model.id===step.id&&!model.execution)&&<p>{tr("Modellkontext","Model context")}: {config.context_policy==="bound_inputs_only"?tr("Nur verbundene Eingaben","Bound inputs only"):tr("Chat- und Projektkontext übernehmen","Inherit chat and project context")}</p>}<p>{tr("Ergebnis","Result")}</p><DataTypeSummary kind={config.output}/><ul>{Object.entries(config.inputs).map(([key,binding])=><li key={key}><strong>{key}</strong>: {binding.value.source==="input"?`${tr("Workflow-Eingabe","Workflow input")} ${binding.value.pointer||"/"}`:binding.value.source==="step"?`${tr("Schritt","Step")} ${workflowDataNodes(definition).findIndex(step=>binding.value.source==="step"&&step.id===binding.value.step_id)+1} ${binding.value.pointer||"/"}`:tr("Fester Wert","Constant value")} · <DataTypeSummary kind={binding.value_type}/></li>)}</ul></section>;
  })}</details>;
}
function nextName(names:string[],base="field"){let index=1;while(names.includes(base+index))index++;return base+index;}
function FieldName({value,used,onChange}:{value:string;used:string[];onChange:(value:string)=>void}){
  const language=useView(v=>v.language),[text,setText]=useState(value);useEffect(()=>setText(value),[value]);
  return <label>{language==="de"?"Feldname":"Field name"}<input required maxLength={64} value={text} onChange={event=>{const text=event.target.value;setText(text);const valid=dataFieldName(text)&&(!used.includes(text)||text===value);event.target.setCustomValidity(valid?"":language==="de"?"Wähle einen eindeutigen Feldnamen mit Buchstaben, Ziffern, Bindestrich oder Unterstrich.":"Choose a unique field name using letters, digits, hyphens or underscores.");if(valid)onChange(text);}}/></label>;
}
export function WorkflowDataTypeEditor({label,value,onChange,objectOnly=false,depth=0}:{label:string;value:WorkflowDataType;onChange:(value:WorkflowDataType)=>void;objectOnly?:boolean;depth?:number}){
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
  if(depth>8)return <p role="alert">{tr("Die Typdefinition ist zu tief verschachtelt.","The type definition is nested too deeply.")}</p>;
  const rename=(old:string,name:string)=>{if(value.type==="object")onChange({...value,fields:Object.fromEntries(Object.entries(value.fields).map(([key,type])=>[key===old?name:key,type]))});};
  return <fieldset className="product-item"><legend>{label}</legend>{!objectOnly&&<label>{tr("Datentyp","Data type")}<select aria-label={tr("Datentyp","Data type")} value={value.type} onChange={event=>onChange(newType(event.target.value as Kind))}>{(Object.keys(kindLabels) as Kind[]).map(kind=><option key={kind} value={kind}>{kindLabels[kind][language==="de"?0:1]}</option>)}</select></label>}
    {value.type==="object"&&<>{Object.entries(value.fields).map(([name,type],index)=><div className="product-item" key={index}><FieldName value={name} used={Object.keys(value.fields)} onChange={next=>rename(name,next)}/><WorkflowDataTypeEditor label={tr("Typ dieses Feldes","Field type")} value={type} depth={depth+1} onChange={type=>onChange({...value,fields:{...value.fields,[name]:type}})}/><button type="button" onClick={()=>onChange({...value,fields:Object.fromEntries(Object.entries(value.fields).filter(([key])=>key!==name))})}>{tr("Feld entfernen","Remove field")}</button></div>)}<button type="button" disabled={depth>=8||Object.keys(value.fields).length>=64} onClick={()=>onChange({...value,fields:{...value.fields,[nextName(Object.keys(value.fields))]:{type:"text"}}})}>{tr("Feld ergänzen","Add field")}</button></>}
    {value.type==="list"&&<><label>{tr("Höchstens so viele Einträge","Maximum number of items")}<input type="number" required min={1} max={1000} value={value.max_items} onChange={event=>onChange({...value,max_items:Number(event.target.value)})}/></label><WorkflowDataTypeEditor label={tr("Typ eines Listeneintrags","List item type")} value={value.item} depth={depth+1} onChange={item=>onChange({...value,item})}/></>}
    {value.type==="nullable"&&<WorkflowDataTypeEditor label={tr("Typ, wenn ein Wert vorliegt","Type when a value is present")} value={value.value} depth={depth+1} onChange={type=>onChange({...value,value:type})}/>}
  </fieldset>;
}
export function WorkflowDataValueEditor({label,kind,value,onChange,depth=0}:{label:string;kind:WorkflowDataType;value:unknown;onChange:(value:unknown)=>void;depth?:number}){
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
  if(depth>16||!validDataType(kind))return <p role="alert">{tr("Prüfe zuerst die Typdefinition.","Review the type definition first.")}</p>;
  if(kind.type==="object"){
    const object=value&&typeof value==="object"&&!Array.isArray(value)?value as Record<string,unknown>:{};
    return <fieldset className="product-item"><legend>{label}</legend>{Object.entries(kind.fields).map(([key,type])=><WorkflowDataValueEditor key={key} label={key} kind={type} value={object[key]} depth={depth+1} onChange={next=>onChange({...object,[key]:next})}/>)}</fieldset>;
  }
  if(kind.type==="list"){
    const values=Array.isArray(value)?value:[];
    return <fieldset className="product-item"><legend>{label}</legend>{values.map((item,index)=><div className="product-item" key={index}><WorkflowDataValueEditor label={`${index+1}`} kind={kind.item} value={item} depth={depth+1} onChange={next=>onChange(values.map((item,i)=>i===index?next:item))}/><button type="button" onClick={()=>onChange(values.filter((_,i)=>i!==index))}>{tr("Eintrag entfernen","Remove item")}</button></div>)}<button type="button" disabled={values.length>=kind.max_items} onClick={()=>onChange([...values,defaultDataValue(kind.item)])}>{tr("Listeneintrag ergänzen","Add list item")}</button><small>{values.length} / {kind.max_items}</small></fieldset>;
  }
  if(kind.type==="nullable")return <fieldset className="product-item"><legend>{label}</legend><label className="product-check"><input type="checkbox" checked={value===null} onChange={event=>onChange(event.target.checked?null:defaultDataValue(kind.value))}/>{tr("Kein Wert (null)","No value (null)")}</label>{value!==null&&<WorkflowDataValueEditor label={label} kind={kind.value} value={value} onChange={onChange} depth={depth+1}/>}</fieldset>;
  if(kind.type==="null")return <p>{label}: null</p>;
  if(kind.type==="boolean")return <label>{label}<select aria-label={label} value={typeof value==="boolean"?String(value):""} required onChange={event=>onChange(event.target.value==="true")}><option value="" disabled>{tr("Bitte auswählen","Choose a value")}</option><option value="false">{tr("Falsch","False")}</option><option value="true">{tr("Wahr","True")}</option></select></label>;
  if(kind.type==="text")return <label>{label}<textarea rows={2} maxLength={16384} value={typeof value==="string"?value:""} onChange={event=>onChange(event.target.value)}/></label>;
  return <label>{label}<input type="number" required step={kind.type==="integer"?1:"any"} min={kind.type==="integer"?-Number.MAX_SAFE_INTEGER:undefined} max={kind.type==="integer"?Number.MAX_SAFE_INTEGER:undefined} value={typeof value==="number"?value:""} onChange={event=>onChange(event.target.value===""?undefined:event.target.valueAsNumber)}/></label>;
}

export function WorkflowDataValuePreview({value,depth=0}:{value:unknown;depth?:number}){
  if(depth>16)return null;
  if(Array.isArray(value))return <ol>{value.map((item,index)=><li key={index}><WorkflowDataValuePreview value={item} depth={depth+1}/></li>)}</ol>;
  if(value&&typeof value==="object")return <dl>{Object.entries(value).map(([key,item])=><div key={key}><dt>{key}</dt><dd><WorkflowDataValuePreview value={item} depth={depth+1}/></dd></div>)}</dl>;
  return <span style={{whiteSpace:"pre-wrap",overflowWrap:"anywhere"}}>{value===null?"null":String(value??"")}</span>;
}

export function WorkflowTaskInputs({client,project,execution,onChange,disabled}:{client:Client;project:string;execution:Extract<WorkspaceTaskExecution,{kind:"workflow"}>;onChange:(value:unknown)=>void;disabled:boolean}){
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const version=useQuery({queryKey:["typed-task-workflow",...workspaceManagementKey(client),project,execution.workflow_id,execution.version_id],enabled:!!execution.workflow_id&&!!execution.version_id,retry:false,queryFn:async()=>{
    const version=await client.workflowVersion(project,execution.workflow_id,execution.version_id);
    if(version.project_id!==project||version.workflow_id!==execution.workflow_id||version.reference.id!==execution.version_id)throw new Error("workflowUnavailable");return version;
  }});
  const type=version.data?.definition.dataflow?.input;
  useEffect(()=>{if(type&&execution.inputs===undefined&&!disabled)onChange(defaultDataValue(type));},[type,execution.inputs,disabled,onChange]);
  if(version.isError)return <p role="alert">{tr("Workflow-Eingaben konnten nicht geladen werden.","Could not load workflow input fields.")}</p>;
  if(!type)return null;
  return <section className="product-stack"><WorkflowDataValueEditor label={tr("Gespeicherte Workflow-Eingaben","Saved workflow inputs")} kind={type} value={execution.inputs??defaultDataValue(type)} onChange={onChange}/><p className="muted">{tr("Diese Werte gehören zur gespeicherten Aufgabenrevision und werden bei jedem Start verwendet.","These values belong to the saved task revision and are used for every start.")}</p></section>;
}

function BindingEditor({definition,node,name,value,onChange,fixedType=false}:{definition:WorkflowDefinition;node:string;name:string;value:WorkflowValueBinding;onChange:(value:WorkflowValueBinding)=>void;fixedType?:boolean}){
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en,data=definition.dataflow!,source=value.value;
  const ancestors=workflowDataAncestors(definition,node),selected=source.source==="step"?"step:"+source.step_id:source.source;
  const pointer=source.source==="literal"?"":source.pointer;
  const sourceType=source.source==="input"?dataTypeAt(data.input,source.pointer):source.source==="step"&&data.steps[source.step_id]?dataTypeAt(data.steps[source.step_id].output,source.pointer):undefined;
  function chooseSource(selection:string){
    if(selection==="literal"){onChange({...value,value:{source:"literal",value:defaultDataValue(value.value_type)}});return;}
    const kind=selection==="input"?data.input:data.steps[selection.slice(5)]?.output;
    if(kind)onChange({value_type:fixedType?value.value_type:structuredClone(kind),value:selection==="input"?{source:"input",pointer:""}:{source:"step",step_id:selection.slice(5),pointer:""}});
  }
  function choosePointer(pointer:string){if(source.source==="literal")return;const type=source.source==="input"?dataTypeAt(data.input,pointer):data.steps[source.step_id]?dataTypeAt(data.steps[source.step_id].output,pointer):undefined;onChange({value_type:fixedType?value.value_type:type?structuredClone(type):value.value_type,value:{...source,pointer}});}
  return <fieldset><legend>{name}</legend><label>{tr("Quelle","Source")}<select aria-label={tr("Quelle","Source")} value={selected} onChange={event=>chooseSource(event.target.value)}><option value="literal">{tr("Fester Wert","Constant value")}</option><option value="input">{tr("Workflow-Eingabe","Workflow input")}</option>{source.source==="step"&&!ancestors.has(source.step_id)&&<option value={selected}>{tr("Quelle ist keine Abhängigkeit","Source is not a dependency")}</option>}{workflowDataNodes(definition).filter(step=>ancestors.has(step.id)).map(step=><option key={step.id} value={"step:"+step.id}>{tr("Ergebnis","Result")} {workflowDataNodes(definition).findIndex(item=>item.id===step.id)+1}: {step.goal}</option>)}</select></label>
    {source.source==="literal"?<>{fixedType?<DataTypeSummary kind={value.value_type}/>:<WorkflowDataTypeEditor label={tr("Typ des festen Wertes","Constant value type")} value={value.value_type} onChange={type=>onChange({value_type:type,value:{source:"literal",value:defaultDataValue(type)}})}/>}<WorkflowDataValueEditor label={tr("Wert","Value")} kind={value.value_type} value={source.value} onChange={next=>onChange({...value,value:{source:"literal",value:next}})}/></>:<><label>{tr("Feldpfad; leer für den gesamten Wert","Field path; empty for the entire value")}<input aria-label={tr("Feldpfad; leer für den gesamten Wert","Field path; empty for the entire value")} maxLength={1024} placeholder="/total" value={pointer} onChange={event=>choosePointer(event.target.value)}/></label><small>{tr("Beispiel: /total oder /items/0. Der erwartete Typ wird beim Auswählen gebunden.","Example: /total or /items/0. Selection binds the expected type.")}</small>{(!sourceType||!sameDataType(sourceType,value.value_type))&&<p role="alert">{tr("Dieser Feldpfad fehlt oder sein Typ wurde geändert. Wähle den Bezug erneut.","This field path is missing or its type changed. Select the binding again.")}</p>}</>}
  </fieldset>;
}

export function WorkflowDataflowEditor({value,onChange}:{value:WorkflowDefinition;onChange:(value:WorkflowDefinition)=>void}){
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const data=value.dataflow;
  if(!data)return <section className="product-item"><h3>{tr("Typisierte Datenübergaben","Typed data bindings")}</h3><p>{tr("Lege Workflow-Eingaben und Ergebnisfelder pro Schritt fest. Diese Variante ersetzt die Ergebnisformate der Rollen. Dynamische Delegation muss vorher deaktiviert werden.","Declare workflow inputs and result fields for each step. This replaces role-level output formats. Disable dynamic delegation first.")}</p><button type="button" disabled={value.roles.some(role=>role.delegate_to.length>0)} onClick={()=>onChange(enableWorkflowDataflow(value))}>{tr("Typisierte Übergaben einrichten","Configure typed bindings")}</button></section>;
  function patchStep(id:string,patch:Partial<WorkflowStepData>){if(data){const next={...data.steps[id],...patch};if(value.controls?.[id]?.operation.kind==="compose")next.output={type:"object",fields:Object.fromEntries(Object.entries(next.inputs).map(([key,binding])=>[key,binding.value_type]))};const definition={...value,dataflow:{...data,steps:{...data.steps,[id]:next}}};if(value.iterations?.[id]){const output=iterationOutputType(definition,id);if(output)next.output=output;}onChange(definition);}}
  return <section className="product-stack"><h3>{tr("Typisierte Datenübergaben","Typed data bindings")}</h3><p>{tr("Felder sind verpflichtend; ein optionaler Wert wird ausdrücklich als null angegeben. Verbindungen übernehmen nur deklarierte Werte. Fehlende Daten und falsche Typen stoppen den betroffenen Schritt vor einem weiteren Modellaufruf.","Fields are required; nullable values explicitly use null. Bindings pass declared values. Missing data and type mismatches stop the affected step before a further model call.")}</p>
    {!validWorkflowDataflow(value)&&<p role="alert">{tr("Prüfe Typen, Datenquellen und Schritt-Abhängigkeiten. Für jeden Modellschritt wird ein Objekt als Ergebnis benötigt.","Review types, data sources and step dependencies. Model steps require an object result.")}</p>}
    {value.roles.some(role=>role.output)&&<button type="button" onClick={()=>onChange({...value,roles:value.roles.map(role=>({...role,output:undefined}))})}>{tr("Konkurrierende Rollen-Ergebnisformate entfernen","Remove conflicting role output formats")}</button>}
    <WorkflowDataTypeEditor label={tr("Workflow-Eingabefelder","Workflow input fields")} value={data.input} objectOnly onChange={input=>onChange({...value,dataflow:{...data,input}})}/>
    {workflowDataNodes(value).map((step,index)=>{const config=data.steps[step.id]??defaultStepData(!!step.execution),fixed=!!value.subflows?.[step.id]||!!value.documents?.[step.id]||value.controls?.[step.id]?.operation.kind==="review";return <section className="product-item" key={step.id}><h4>{index+1}. {step.goal||step.role_id}</h4>{step.execution?<p>{tr("Dieser Browserbaustein verwendet das feste Ziel aus der Schrittkonfiguration und liefert Text. Nachfolgende Schritte können diesen Text binden.","This browser step uses its fixed configured target and returns text. Following steps can bind that text.")}</p>:<>
      {value.steps.some(model=>model.id===step.id&&!model.execution)&&<><label>{tr("Kontext für dieses Modell","Context for this model")}<select value={config.context_policy??"inherit"} onChange={event=>patchStep(step.id,{context_policy:event.target.value==="bound_inputs_only"?"bound_inputs_only":undefined})}><option value="inherit">{tr("Chat- und Projektkontext übernehmen","Inherit chat and project context")}</option><option value="bound_inputs_only">{tr("Nur verbundene Eingaben","Bound inputs only")}</option></select></label>{config.context_policy==="bound_inputs_only"&&<p className="muted">{tr("Rollenanweisung, Schrittziel und Werkzeugmetadaten bleiben enthalten. Frühere Chats, Projektwissen und zusätzliche Agentvorlagen werden nicht übernommen.","Role instructions, step goal and tool metadata remain included. Earlier chats, project knowledge and additional agent templates are not inherited.")}</p>}</>}
      {value.controls?.[step.id]||value.subflows?.[step.id]||value.iterations?.[step.id]||value.documents?.[step.id]?<><p>{value.documents?.[step.id]?tr("Fester Ergebnistyp der Dokumentaktion","Fixed document result type"):tr("Ergebnistyp des Kontrollbausteins","Control result type")}</p><DataTypeSummary kind={config.output}/></>:<WorkflowDataTypeEditor label={tr("Ergebnisfelder dieses Schritts","Step result fields")} value={config.output} objectOnly onChange={output=>patchStep(step.id,{output})}/>}
      {!["select","collect"].includes(value.controls?.[step.id]?.operation.kind??"")&&<><h5>{tr("Eingaben dieses Schritts","Step inputs")}</h5>{Object.entries(config.inputs).map(([name,binding],index)=><div className="product-item" key={index}>{fixed||value.iterations?.[step.id]?.collection_input===name?<strong>{name}</strong>:<FieldName value={name} used={Object.keys(config.inputs)} onChange={next=>patchStep(step.id,{inputs:Object.fromEntries(Object.entries(config.inputs).map(([key,binding])=>[key===name?next:key,binding]))})}/>}<BindingEditor fixedType={fixed} definition={value} node={step.id} name={name} value={binding} onChange={binding=>patchStep(step.id,{inputs:{...config.inputs,[name]:binding}})}/>{!fixed&&<button type="button" onClick={()=>patchStep(step.id,{inputs:Object.fromEntries(Object.entries(config.inputs).filter(([key])=>key!==name))})}>{tr("Eingabe entfernen","Remove input")}</button>}</div>)}<button type="button" disabled={fixed||Object.keys(config.inputs).length>=32} onClick={()=>patchStep(step.id,{inputs:{...config.inputs,[nextName(Object.keys(config.inputs),"input")]:{value_type:{type:"text"},value:{source:"literal",value:""}}}})}>{tr("Schritteingabe ergänzen","Add step input")}</button></>}
    </>}</section>;})}
    <button type="button" disabled={Object.keys(value.documents??{}).length>0||Object.keys(value.iterations??{}).length>0||Object.keys(value.subflows??{}).length>0||Object.keys(value.controls??{}).length>0||Object.keys(value.gates??{}).length>0} onClick={()=>onChange({...value,format_version:1,dataflow:undefined})}>{tr("Typisierte Bindungen aus diesem Entwurf entfernen","Remove typed bindings from this draft")}</button>
  </section>;
}
