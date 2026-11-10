import {useEffect,useMemo,useRef,useState} from "react";
import {Client,RequestError,type DirectoryListing,type FileEntry,type WorkflowDefinition} from "@cowork/client";
import {DocumentSourceSelection,documentSourceBindings,relativeDocumentPath,selectableDocumentFile} from "./documentSourceSelection";

type Props={client:Client;order:string;selection:string;definition:WorkflowDefinition;value:unknown;disabled:boolean;language:"de"|"en";onChange:(value:Record<string,unknown>)=>void;onChecking:(id:string,pending:boolean)=>void};
export function WorkflowDocumentSources(props:Props){
  const {client,order,selection,definition,language,disabled}=props,tr=(de:string,en:string)=>language==="de"?de:en;
  const bindings=useMemo(()=>documentSourceBindings(definition),[definition]);
  const scope=useMemo(()=>({client,base:client.base,token:client.token,order,selection,definition,session:new DocumentSourceSelection()}),[client,client.base,client.token,order,selection,definition]);
  const latest=useRef({props,scope});latest.current={props,scope};
  const [open,setOpen]=useState(false),[path,setPath]=useState("."),[refresh,setRefresh]=useState(0),[target,setTarget]=useState("");
  const [listing,setListing]=useState<{scope:typeof scope;path:string;value:DirectoryListing}>(),[loading,setLoading]=useState(false),[pending,setPending]=useState<{scope:typeof scope;id:string}>(),[notice,setNotice]=useState(""),[error,setError]=useState("");
  const pendingId=useRef<string|undefined>(undefined);
  useEffect(()=>{
    const session=new DocumentSourceSelection();scope.session=session;
    setNotice("");setError("");setPath(".");setTarget("");
    return()=>{session.close();if(pendingId.current){latest.current.props.onChecking(pendingId.current,false);pendingId.current=undefined;}};
  },[scope]);
  useEffect(()=>{
    let active=true;
    if(!open||disabled)return;
    setLoading(true);setError("");
    async function load(){
      await client.workspace(order);
      if(!active||latest.current.scope!==scope)return;
      const value=await client.workspaceFiles(order,path);
      if(!active||latest.current.scope!==scope)return;
      if(value.path!==path||value.folder_id!=="workspace:"+order)throw new Error("document_source_invalid");
      setListing({scope,path,value});
    }
    void load().catch(reason=>{if(active&&latest.current.scope===scope)setError(reason instanceof RequestError&&reason.details.code==="workspace_busy"?tr("Die Arbeitsdateien werden gerade verwendet. Versuche es danach erneut.","The working files are in use. Try again when they are available."):tr("Die Arbeitsdateien konnten nicht geladen werden. Aktualisiere die Auswahl.","Working files could not be loaded. Refresh the selection."));}).finally(()=>{if(active&&latest.current.scope===scope)setLoading(false);});
    return()=>{active=false;};
  },[open,disabled,client,order,path,refresh,scope,language]);
  if(!bindings.length)return null;
  const checking=pending?.scope===scope;
  const binding=bindings.find(item=>item.step===target)??bindings[0],entries=listing?.scope===scope&&listing.path===path&&!loading&&!error?listing.value.entries:[];
  async function choose(entry:FileEntry){
    if(disabled||pendingId.current||!entries.includes(entry))return;
    const id=crypto.randomUUID();pendingId.current=id;setPending({scope,id});props.onChecking(id,true);setNotice("");setError("");
    try{
      const applied=await scope.session.select({entry,binding,download:path=>client.downloadWorkspace(order,{path}),current:()=>latest.current.props.value,isCurrent:()=>latest.current.scope===scope&&!latest.current.props.disabled,apply:value=>latest.current.props.onChange(value)});
      if(applied&&latest.current.scope===scope)setNotice(tr("Datei geprüft und mit dem Ablauf verbunden: ","File verified and connected to the workflow: ")+entry.path);
    }catch(reason){if(latest.current.scope===scope)setError(reason instanceof RequestError&&reason.details.code==="workspace_busy"?tr("Die Arbeitsdateien werden gerade verwendet. Es wurde keine neue Quelle verbunden.","The working files are in use. No new source was connected."):tr("Die Datei konnte nicht geprüft werden. Die bisherigen Eingaben bleiben erhalten. Aktualisiere die Auswahl und versuche es erneut.","The file could not be verified. Existing inputs are retained. Refresh the selection and try again."));}
    finally{props.onChecking(id,false);if(pendingId.current===id)pendingId.current=undefined;if(latest.current.scope===scope)setPending(undefined);}
  }
  return <details className="workflow-document-sources product-item" open={open} onToggle={event=>setOpen(event.currentTarget.open)}>
    <summary>{tr("Dokument aus Arbeitsdateien wählen","Choose a document from working files")}</summary>
    <p>{tr("Wähle eine vorhandene Datei dieses Chats. Der Inhalt wird geprüft; Dateipfad und Inhaltsbezug werden gemeinsam verbunden. Die Ausführung beginnt erst nach deiner Planbestätigung.","Choose an existing file from this chat. Its content is verified; path and content digest are connected together. Execution begins only after you confirm the plan.")}</p>
    <fieldset disabled={disabled||checking}>
      {bindings.length>1?<label>{tr("Dokumentschritt","Document step")}<select value={binding.step} onChange={event=>{setTarget(event.target.value);setNotice("");}}>{bindings.map(item=><option key={item.step} value={item.step}>{item.name}</option>)}</select></label>:<p><strong>{binding.name}</strong></p>}
      <p>{path}</p><div className="actions">{path!=="."&&<button type="button" disabled={loading} onClick={()=>setPath(path.includes("/")?path.slice(0,path.lastIndexOf("/")):".")}>{tr("Übergeordneter Ordner","Parent folder")}</button>}<button type="button" disabled={loading} onClick={()=>setRefresh(value=>value+1)}>{tr("Dateiauswahl aktualisieren","Refresh file selection")}</button></div>
      {loading&&<p role="status">{tr("Arbeitsdateien werden geladen …","Loading working files …")}</p>}
      <ul className="working-file-list">{entries.map(entry=><li key={entry.path}><button type="button" className="quiet" disabled={entry.kind==="directory"?!relativeDocumentPath(entry.path):!selectableDocumentFile(entry)} onClick={()=>{setNotice("");entry.kind==="directory"?setPath(entry.path):void choose(entry);}}>{entry.name}{entry.kind==="directory"?"/":` · ${entry.size} B`}</button></li>)}</ul>
      {!loading&&!error&&listing?.scope===scope&&listing.path===path&&entries.length===0&&<p>{tr("Hier sind noch keine Dateien vorhanden. Importiere zuerst ein Dokument über die Arbeitsdateien dieses Chats.","There are no files here yet. Import a document through this chat's working files first.")}</p>}
      {listing?.scope===scope&&listing.path===path&&listing.value.truncated&&<p role="status">{tr("Die Liste ist ein Ausschnitt. Öffne den passenden Unterordner.","This list is an excerpt. Open the appropriate subfolder.")}</p>}
      <p className="muted">{tr("Dokumentquellen bis 32 MiB. Verknüpfungen und gesperrte Dateien sind nicht auswählbar.","Document sources up to 32 MiB. Links and blocked files cannot be selected.")}</p>
    </fieldset>
    {checking&&<p role="status">{tr("Dateiinhalt wird geprüft …","Verifying file content …")}</p>}{notice&&<p role="status">{notice}</p>}{error&&<p role="alert">{error}</p>}
  </details>;
}
