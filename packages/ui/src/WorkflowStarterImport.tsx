import {useMemo,useRef,useState,type ReactNode} from "react";
import type {ProviderProfile,WorkflowDefinition} from "@cowork/client";
import {convertV1Pipeline,createStarterWorkflow,readV1Pipelines,workflowImportLimit,workflowStarters,WorkflowImportError,type StarterId,type WorkflowModelSelection} from "./workflowStarters";

export function workflowImportMessage(error:unknown,language:"de"|"en"):string {
  const messages:Record<WorkflowImportError["code"],[string,string]>={
    invalid_json:["Die Datei enthält kein gültiges Pipeline-JSON.","The file does not contain valid pipeline JSON."],
    unsupported_shape:["Diese Pipeline-Struktur wird nicht unterstützt. Schritte benötigen ein gültiges prompt oder JSON-args.","This pipeline shape is unsupported. Steps require a valid prompt or JSON args."],
    unsupported_field:["Zusätzliche Felder werden nicht importiert. Verwende ausschließlich V1-Pipeline-Datensätze ohne Konfigurationen oder Zugangsdaten.","Additional fields cannot be imported. Use V1 pipeline records without configurations or credentials."],
    unsupported_tool:["Dieses V1-Werkzeug benötigt eine eigene geprüfte Umsetzung. Importiert werden nur Modellschritte ohne tool oder mit tool: ollama.","This V1 tool requires a separately verified conversion. Only model steps without tool or with tool: ollama are supported."],
    zero_context:["zero_context ist nicht übertragbar: Native Workflows geben frühere Schrittresultate als Kontext weiter.","zero_context cannot be converted: native workflows pass earlier step results as context."],
    invalid_text:["Ein Name oder Schritttext fehlt, ist zu lang oder enthält ungültige Zeichen.","A name or step text is missing, too long, or contains invalid characters."],
    limit:["Grenzen: 1 MiB JSON, 100 Pipeline-Datensätze und 1–64 Schritte je Pipeline.","Limits: 1 MiB JSON, 100 pipeline records, and 1–64 steps per pipeline."],
    draft_limit:["Der konvertierte Entwurf überschreitet 990.000 UTF-8-Bytes. Teile die Pipeline in kleinere Workflows auf.","The converted draft exceeds 990,000 UTF-8 bytes. Split the pipeline into smaller workflows."],
    model_missing:["Wähle ein vorhandenes V2-Modell ausdrücklich aus.","Explicitly choose an existing V2 model."],
    model_changed:["Die Modellrevision ist nicht mehr verfügbar. Wähle das aktuelle Modell und prüfe den Entwurf erneut.","The model revision is unavailable. Choose the current model and review the draft again."],
  };
  const message=error instanceof WorkflowImportError?messages[error.code][language==="de"?0:1]:language==="de"?"Der Import konnte nicht gelesen werden.":"The import could not be read.";
  return error instanceof WorkflowImportError&&error.at?`${language==="de"?"Schritt":"Step"} ${error.at}: ${message}`:message;
}

export function WorkflowStarterImport({providers,language,disabled,onCreate,summary}:{providers:ProviderProfile[];language:"de"|"en";disabled:boolean;onCreate:(definition:WorkflowDefinition)=>void;summary:(definition:WorkflowDefinition)=>ReactNode}) {
  const tr=(de:string,en:string)=>language==="de"?de:en;
  const [selectedModel,setSelectedModel]=useState(""),[starter,setStarter]=useState<StarterId>("research"),[objective,setObjective]=useState(workflowStarters[0].goal[language]);
  const [source,setSource]=useState(""),[pipelineIndex,setPipelineIndex]=useState(0),[preview,setPreview]=useState<WorkflowDefinition|null>(null),[error,setError]=useState(""),[reading,setReading]=useState(false);
  const readEpoch=useRef(0);
  const parsed=useMemo(()=>{try{return {pipelines:readV1Pipelines(source),error:null};}catch(error){return {pipelines:[],error};}},[source]);
  function selection():WorkflowModelSelection {if(!selectedModel)throw new WorkflowImportError("model_missing");try{return JSON.parse(selectedModel) as WorkflowModelSelection;}catch{throw new WorkflowImportError("model_missing");}}
  function changeSource(value:string){readEpoch.current++;setReading(false);setSource(value);setPipelineIndex(0);setPreview(null);setError("");}
  async function readFile(file:File|undefined){
    if(!file||disabled)return;const epoch=++readEpoch.current;setReading(true);setPreview(null);setError("");
    try{if(file.size>workflowImportLimit)throw new WorkflowImportError("limit");const text=new TextDecoder("utf-8",{fatal:true}).decode(await file.arrayBuffer());if(epoch!==readEpoch.current)return;setSource(text);setPipelineIndex(0);}
    catch(error){if(epoch===readEpoch.current){setSource("");setError(workflowImportMessage(error,language));}}
    finally{if(epoch===readEpoch.current)setReading(false);}
  }
  function buildImport(){if(parsed.error)throw parsed.error;const pipeline=parsed.pipelines[pipelineIndex];if(!pipeline)throw new WorkflowImportError("unsupported_shape");return convertV1Pipeline(pipeline,selection(),providers);}
  function reviewImport(){if(disabled||reading)return;try{setPreview(buildImport());setError("");}catch(error){setPreview(null);setError(workflowImportMessage(error,language));}}
  function createImport(){if(disabled||reading||!preview)return;try{const current=buildImport();if(JSON.stringify(current)!==JSON.stringify(preview))throw new WorkflowImportError("model_changed");onCreate(current);setError("");}catch(error){setError(workflowImportMessage(error,language));}}
  function createStarter(){if(disabled||reading)return;try{onCreate(createStarterWorkflow(starter,selection(),providers,language,objective));setError("");}catch(error){setError(workflowImportMessage(error,language));}}
  return <details className="workflow-editor-card"><summary>{tr("Startervorlage oder V1-Pipeline übernehmen","Use a starter template or V1 pipeline")}</summary>
    <p>{tr("Erstelle einen unabhängigen Entwurf. Veröffentlichung und Ausführung verwenden anschließend die vorhandenen Prüfungen und Freigaben.","Create an independent draft. Publication and execution then use the existing validation and approvals.")}</p>
    <fieldset disabled={disabled||reading}><label>{tr("V2-Modell für die Übernahme","V2 model for this draft")}<select value={selectedModel} onChange={event=>{setSelectedModel(event.target.value);setPreview(null);setError("");}}><option value="">{tr("Modell ausdrücklich auswählen","Explicitly choose a model")}</option>{providers.map(provider=><option key={provider.id+":"+provider.revision} value={JSON.stringify({id:provider.id,revision:provider.revision})}>{provider.name} · {provider.model} · {tr("Revision","revision")} {provider.revision}</option>)}</select></label>
      {!providers.length&&<p role="status">{tr("Richte zuerst ein Modell unter Einstellungen → Teams & Tools ein.","Configure a model in Settings → Teams & Tools first.")}</p>}
      <section><h3>{tr("Startervorlagen","Starter templates")}</h3><label>{tr("Vorlage","Template")}<select value={starter} onChange={event=>{const id=event.target.value as StarterId;setStarter(id);setObjective(workflowStarters.find(preset=>preset.id===id)!.goal[language]);}}>{workflowStarters.map(preset=><option value={preset.id} key={preset.id}>{preset.name[language]}</option>)}</select></label>
        <label>{tr("Ziel","Objective")}<textarea rows={3} maxLength={8000} value={objective} onChange={event=>setObjective(event.target.value)}/></label><p className="muted">{tr("Planen → Umsetzen → Prüfen. Enthält ausschließlich Artefaktwerkzeuge und Rückfragen; zusätzliche Werkzeuge wählst du später bewusst aus.","Plan → Execute → Review. Includes artifact tools and questions only; explicitly select any additional tools later.")}</p><button type="button" disabled={!selectedModel||!objective.trim()} onClick={createStarter}>{tr("Starter als Entwurf öffnen","Open starter as draft")}</button></section>
      <section><h3>{tr("V1-Pipeline importieren","Import a V1 pipeline")}</h3><p>{tr("Ein V1-RpcPipeline-Datensatz oder eine Liste solcher Datensätze. Keine vollständigen App-Backups, Gateway-Konfigurationen oder Zugangsdaten einfügen.","Use one V1 RpcPipeline record or a list of records. Do not paste full app backups, gateway configurations, or credentials.")}</p>
        <label>{tr("Pipeline-Datei (UTF-8 JSON)","Pipeline file (UTF-8 JSON)")}<input type="file" accept="application/json,.json" onChange={event=>void readFile(event.target.files?.[0])}/></label><label>{tr("Pipeline-JSON","Pipeline JSON")}<textarea rows={6} maxLength={workflowImportLimit} value={source} placeholder={'{"name":"Pipeline","steps_json":"[{\\"tool\\":\\"ollama\\",\\"prompt\\":\\"...\\"}]","zero_context":false}'} onChange={event=>changeSource(event.target.value)}/></label>
        {parsed.pipelines.length>0&&<label>{tr("Zu übernehmende Pipeline","Pipeline to import")}<select value={pipelineIndex} onChange={event=>{setPipelineIndex(Number(event.target.value));setPreview(null);setError("");}}>{parsed.pipelines.map((pipeline,index)=><option key={index} value={index}>{index+1}. {pipeline.name}</option>)}</select></label>}
        {parsed.pipelines[pipelineIndex]?.description&&<p>{tr("V1-Beschreibung (Metadaten): ","V1 description (metadata): ")}{parsed.pipelines[pipelineIndex].description}</p>}
        <p className="muted">{tr("Unterstützt: Modell-, Planner-, Web-Fetch- und Web-Suchschritte sowie zero_context. Gateway-/Werkzeugnamen bleiben wie in V1 reine Modellhinweise. Webschritte benötigen Browserrechte und eine konkrete Aktionsfreigabe; keine V1-Konfiguration oder Zugangsdaten werden übernommen.","Supported: model, planner, web fetch, web search, and zero_context. Gateway/tool names remain inert model labels as in V1. Web steps require Browser rights and exact action approval; V1 configurations and credentials are not transferred.")}</p>
        <button type="button" disabled={!source.trim()||!selectedModel} onClick={reviewImport}>{tr("Konvertierung prüfen","Review conversion")}</button>
        <button type="button" disabled={!source} onClick={()=>changeSource("")}>{tr("Importtext verwerfen","Clear import text")}</button>
      </section>
    </fieldset>{reading&&<p role="status">{tr("Datei wird gelesen …","Reading file …")}</p>}{error&&<p role="alert">{error}</p>}
    {preview&&<section className="workflow-editor-card"><h3>{tr("Vorschau des nativen Workflows","Native workflow preview")}</h3><p>{tr("Modellschritte besitzen keine Werkzeugrechte. Webschritte verlangen ausdrücklich Browserrechte auf allen Ebenen. zero_context trennt frühere Schrittresultate, erhält aber die Ausführungsreihenfolge. V1-ID und Beschreibung werden nicht zu Anweisungen.","Model steps have no tool permissions. Web steps explicitly require Browser rights at every layer. zero_context isolates earlier step results while retaining execution order. V1 IDs and description are not instructions.")}</p>{summary(preview)}<button type="button" disabled={disabled||reading} onClick={createImport}>{tr("Als neuen Entwurf öffnen","Open as new draft")}</button></section>}
  </details>;
}
