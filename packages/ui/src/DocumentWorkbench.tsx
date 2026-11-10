import {useEffect,useRef,useState,type FormEvent} from "react";
import {useQuery,useQueryClient} from "@tanstack/react-query";
import {uploadMetadata,RequestError,type AccessMode,type Client,type DocumentPreparation,type DownloadedFile,type FileUpload,type ToolCall} from "@cowork/client";
import {useView} from "./view";
import {de,en,type Key} from "./i18n";
import {DocumentPreview} from "./DocumentPreview";
import {DocumentToolSummary} from "./DocumentToolSummary";
import {ToolPreviews} from "./ToolPreviews";
import {FileUploads} from "./FileUploads";
import {deliverDownload} from "./download";
import {createDocumentTool,documentActionKey,readDocumentAction,retainDocumentAction,discardDocumentAction,type PendingDocumentAction} from "./documentActions";
import {documentOfficeModel} from "./documentInspector";
import {OfficeBinaryEditor} from "./OfficeBinaryEditor";
import {officeEditorDraftKey} from "./officeEditorDraft";
import type {OfficeModel} from "./officeDocuments";
import {toolboxAvailability} from "./toolboxWorkbench";
import {PdfFormEditor,pdfFormFields,type PdfField} from "./PdfFormEditor";
import {definitiveRequestRejection} from "./requestJournal";
import {ImagePipelineEditor} from "./ImagePipelineEditor";
import {imageDraft,type ImageDraft} from "./imagePipeline";
import {DocumentDiagnostics} from "./DocumentDiagnostics";
import {LatexProjectEditor,downloadLatexSource,type LatexSource} from "./LatexProjectEditor";
import {latexProject,latexDiagnosticFile,latexProjectMatchesTool,type LatexProject} from "./latexProject";

const record=(value:unknown):Record<string,any>=>value!==null&&typeof value==="object"&&!Array.isArray(value)?value as Record<string,any>:{};
const terminal=(state?:string)=>["completed","failed","canceled","expired"].includes(state??"");

export function DocumentWorkbench({client,order,mode,onImport}:{client:Client,order:string,mode:AccessMode,onImport?:(upload:FileUpload)=>void}){
  const language=useView(v=>v.language),tr=(a:string,b:string)=>language==="de"?a:b,t=(k:Key)=>language==="de"?de[k]:en[k];
  const qc=useQueryClient(),storageKey=documentActionKey(client.base,order);
  const [initial]=useState(()=>{try{return {pending:readDocumentAction(localStorage,storageKey),error:""};}catch{return {pending:null,error:"document_action_journal_invalid"};}});
  const [pending,setPending]=useState<PendingDocumentAction|null>(initial.pending),[prepared,setPrepared]=useState<DocumentPreparation|null>(null);
  const [busy,setBusy]=useState(false),[error,setError]=useState(initial.error),[runId,setRunId]=useState<string|null>(null);
  const [journalBroken,setJournalBroken]=useState(!!initial.error);
  const [directory,setDirectory]=useState("."),[source,setSource]=useState(""),[selected,setSelected]=useState<{path:string,file:DownloadedFile}|null>(null);
  const workingFiles=useRef<HTMLHeadingElement>(null);
  const [format,setFormat]=useState("docx"),[title,setTitle]=useState(""),[body,setBody]=useState(""),[output,setOutput]=useState("report.docx");
  const [assets,setAssets]=useState(""),[pdfAction,setPdfAction]=useState("extract_pages"),[pdfValue,setPdfValue]=useState("1");
  const [inspection,setInspection]=useState<{model:OfficeModel;folder:string}|null>(null);
  const [pdfInspection,setPdfInspection]=useState<{folder:string;path:string;sha256:string;fields:PdfField[]}|null>(null);
  const [convertFormat,setConvertFormat]=useState("pdf"),[showDraftHistory,setShowDraftHistory]=useState(false);
  const [image,setImage]=useState<ImageDraft|null>(null);
  const [latex,setLatex]=useState<LatexProject|null>(null),[latexSource,setLatexSource]=useState<LatexSource|null>(null);
  const [draftId,setDraftId]=useState(()=>{try{return localStorage.getItem(storageKey+":draft")??crypto.randomUUID();}catch{return crypto.randomUUID();}}),[draftRevision,setDraftRevision]=useState(0),[notice,setNotice]=useState("");
  const draftSave=useRef<Parameters<Client["saveDocumentDraft"]>[1]|null>(null);
  const draftHistory=useQuery({queryKey:["document-draft-history",client.base,order,draftId],queryFn:()=>client.documentDraftSnapshots(order,draftId),enabled:showDraftHistory,retry:false});
  const availability=useQuery({queryKey:["toolbox",client.base,"documents"],queryFn:()=>toolboxAvailability(client),retry:false,refetchInterval:q=>q.state.data?.document_status==="probing"?1000:10000});
  const workspace=useQuery({queryKey:["document-workspace",client.base,order],queryFn:()=>client.workspace(order),retry:false});
  const listing=useQuery({queryKey:["document-files",client.base,order,directory],queryFn:()=>client.workspaceFiles(order,directory),enabled:!!workspace.data,retry:false,refetchInterval:4000});
  const detail=useQuery({queryKey:["document-action-run",client.base,order,runId],queryFn:async()=>{
    const value=await client.detail(runId!);if(value.run.spec.order_id!==order)throw new Error("document_run_integrity");return value;
  },enabled:!!runId,retry:false,refetchInterval:q=>terminal(q.state.data?.run.state)?false:1000});
  const events=useQuery({queryKey:["document-action-events",client.base,order,runId,detail.data?.run.revision],queryFn:()=>client.events(runId!,0),enabled:!!runId,retry:false});
  const result=record(events.data?.filter(e=>e.kind==="tool_completed").at(-1)?.payload),engine=record(record(result.result).engine??result.result),data=record(engine.result);
  const locked=busy||!!pending||journalBroken||availability.data?.documents_available!==true,folder=workspace.data?.folder.id;
  const diagnosticProject=latex&&prepared&&latexProjectMatchesTool(latex,prepared.tool)?latex:null;
  useEffect(()=>{
    if(!prepared||prepared.tool.name!=="document_read"||record(prepared.tool.arguments).operation!=="inspect")return;
    const args=record(prepared.tool.arguments);
    if(!data.source_sha256)return;
    if(data.source_sha256!==args.expected_sha256){setError("document_source_integrity");return;}
    if(data.format==="pdf")setPdfInspection({folder:args.folder_id,path:args.path,sha256:data.source_sha256,fields:pdfFormFields(data.form_fields)});
    try{const model=documentOfficeModel(data,args.path,args.expected_sha256);if(model)setInspection({model,folder:args.folder_id});}catch(e){setError(String(e));}
  },[events.data]);

  async function perform(fn:()=>Promise<void>){if(busy)return;setBusy(true);setError("");try{await fn();}catch(e){setError(e instanceof Error?e.message:String(e));}finally{setBusy(false);}}
  async function recover(){if(!pending)return;await perform(async()=>{const value=await client.documentPreparationByKey(order,pending.key);setPrepared(value);if(value.run_id)setRunId(value.run_id);});}
  useEffect(()=>{let live=true;if(initial.pending)void client.documentPreparationByKey(order,initial.pending.key).then(p=>{if(live){setPrepared(p);setRunId(p.run_id??null);}},e=>{if(live)setError(String(e));});return()=>{live=false;};},[client,order]);
  async function prepare(tool:ToolCall){if(locked)return;await perform(async()=>{
    const operation:PendingDocumentAction={key:crypto.randomUUID(),phase:"prepare"};
    // Retain only the operation reference; document content stays in the server draft.
    retainDocumentAction(localStorage,storageKey,operation);setPending(operation);setPrepared(null);setRunId(null);
    const basis=await client.order(order);
    let value:DocumentPreparation;
    try{value=await client.prepareDocumentAction(order,{version:2,base_revision:basis.revision,idempotency_key:operation.key,access_mode:mode,tool});}
    catch(reason){if(reason instanceof RequestError&&definitiveRequestRejection(reason.status)){discardDocumentAction(localStorage,storageKey,operation);setPending(null);}throw reason;}
    setPrepared(value);await qc.invalidateQueries({queryKey:["orders"]});
  });}
  async function commit(){if(!pending||!prepared)return;await perform(async()=>{
    const next:PendingDocumentAction={...pending,phase:"commit"};retainDocumentAction(localStorage,storageKey,next);setPending(next);
    const run=await client.commitDocumentAction(order,prepared.id);setRunId(run.id);await qc.invalidateQueries({queryKey:["orders"]});
  });}
  function release(){if(busy||!pending||pending.phase==="commit"&&!terminal(detail.data?.run.state))return;try{discardDocumentAction(localStorage,storageKey,pending);setPending(null);setPrepared(null);setRunId(null);setError("");}catch(e){setError(tr("Die Aktionsreferenz wurde in einem anderen Fenster geändert. Lade die Ansicht neu, bevor du fortfährst.","The action reference changed in another window. Reload this view before continuing."));}}
  async function open(path=source,browseDirectory=false){await perform(async()=>{const observed=await client.workspaceStat(order,path);if(browseDirectory&&observed.node?.kind==="directory"){setDirectory(path);requestAnimationFrame(()=>{workingFiles.current?.scrollIntoView({block:"start"});workingFiles.current?.focus({preventScroll:true});});return;}if(!observed.node||observed.node.kind!=="file")throw new Error("document_not_file");const file=await client.downloadWorkspace(order,{path,expected_sha256:observed.node.sha256});setSource(path);setSelected({path,file});});}
  function read(operation:string,extra:Record<string,unknown>={}){if(!folder||!source)return;void prepare({id:"ui",name:"document_read",arguments:{operation,folder_id:folder,path:source,...(selected?.path===source?{expected_sha256:selected.file.sha256}:{}),...extra}});}
  function inputRef(path:string){return {folder_id:folder!,path,...(selected?.path===path?{expected_sha256:selected.file.sha256}:{})};}
  async function create(event:FormEvent){event.preventDefault();try{if(format!=="tex"){await prepare(createDocumentTool(format,body,title,output));return;}
    if(!onImport)throw new Error("document_import_unavailable");await perform(async()=>{const blob=new Blob([body],{type:"text/x-tex"}),file=await uploadMetadata(blob,output.split("/").at(-1)!);const upload=await client.reserveUpload(order,{idempotency_key:crypto.randomUUID(),file});onImport(await client.uploadContent(order,upload,blob));});
    }catch(e){setError(e instanceof Error?e.message:String(e));}
  }
  async function saveDraft(){await perform(async()=>{
    imageDraft(image);
    latexProject(latex);
    localStorage.setItem(storageKey+":draft",draftId);
    draftSave.current??={id:draftId,base_revision:draftRevision,idempotency_key:crypto.randomUUID(),content:{format,title,body,output,source,assets,image,latex}};
    const result=await client.saveDocumentDraft(order,draftSave.current);draftSave.current=null;
    await draftHistory.refetch();
    if(!result.saved){setNotice(tr("Konfliktkopie gespeichert. Lade den aktuellen Entwurf oder speichere deine Eingaben unter einer neuen Entwurfskennung.","Conflict copy saved. Load the current draft or save your input under a new draft identity."));return;}
    setDraftRevision(result.draft.revision);setNotice(tr("Entwurf gespeichert","Draft saved")+` · ${result.draft.revision}`);
  });}
  function loadDraftContent(value:unknown){const content=record(value),restored=imageDraft(content.image),project=latexProject(content.latex);setFormat(String(content.format??"docx"));setTitle(String(content.title??""));setBody(String(content.body??""));setOutput(String(content.output??"report.docx"));setSource(String(content.source??""));setAssets(String(content.assets??""));setImage(restored);setLatex(project);setLatexSource(null);setSelected(null);}
  async function openLatexSource(path:string,line=1){if(!latex||latex.folder!==folder)throw new Error("latex_project_workspace_changed");setLatexSource(await downloadLatexSource(latex,path,request=>client.downloadWorkspace(order,request),line));}
  function loadHistoricalDraft(value:unknown){try{loadDraftContent(value);setNotice(tr("Historische Kopie geladen. Speichern erzeugt eine neue Version; Konflikte bleiben als Kopie erhalten.","Historical copy loaded. Saving creates a new version; conflicts are retained as copies."));}catch(reason){setError(String(reason));}}
  async function loadDraft(){await perform(async()=>{const value=await client.documentDraft(order,draftId);setDraftRevision(value.revision);loadDraftContent(value.content);draftSave.current=null;setNotice(tr("Gespeicherten Entwurf geladen","Loaded saved draft"));});}
  function copyDraft(){if(busy||draftSave.current)return;const id=crypto.randomUUID();try{localStorage.setItem(storageKey+":draft",id);setDraftId(id);setDraftRevision(0);setNotice(tr("Neue Entwurfskopie; die nächste Speicherung erzeugt eine eigene Historie.","New draft copy; the next save creates its own history."));}catch(e){setError(String(e));}}

  return <section className="document-workbench">
    <header><div><h2>{tr("Dokumente und Bilder","Documents and images")}</h2><p>{tr("Arbeitsdateien dieses Chats lesen, erstellen und als neue Version bearbeiten.","Read, create and edit this chat's working files as new versions.")}</p></div></header>
    {availability.data?.documents_available!==true&&<p role={availability.data?.document_status==="data_path_too_long"?"alert":"status"}>{availability.data?.document_status==="data_path_too_long"?tr("Der interne Dokument-Arbeitsordner ist für Windows zu lang. Verwende einen kürzeren privaten V2-Datenpfad und starte den Arbeitsbereich neu. Bei einer Serververbindung muss der Betreiber den Datenpfad anpassen.","The internal document working directory is too long for Windows. Use a shorter private V2 data path and restart the workspace. For a server connection, the operator must adjust the data path."):availability.isPending||availability.data?.document_status==="probing"?tr("Dokument-Laufzeit wird geprüft…","Checking document runtime…"):tr("Dokument-Laufzeit ist noch nicht bereit. Einrichtung unter Einstellungen → Werkzeuge prüfen.","Document runtime is not ready. Check setup in Settings → Tools.")} <button disabled={availability.isFetching} onClick={()=>void availability.refetch()}>{tr("Bereitschaft erneut prüfen","Check readiness again")}</button></p>}
    {error&&<p role="alert" className="error">{error}</p>}{notice&&<p role="status">{notice}</p>}
    {journalBroken&&<button disabled={busy} onClick={()=>{if(window.confirm(tr("Die lokale Aktionsreferenz ist beschädigt. Prüfe bestehende Chatläufe vor einer neuen Aktion. Nur diese Referenz verwerfen?","The local action reference is invalid. Check existing chat runs before starting another action. Discard only this reference?"))){try{localStorage.removeItem(storageKey);setJournalBroken(false);setError("");}catch(e){setError(String(e));}}}}>{tr("Beschädigte Referenz verwerfen","Discard invalid reference")}</button>}
    {workspace.isError&&<p role="alert">{tr("Arbeitsordner konnte nicht geladen werden.","Could not load the working folder.")}</p>}
    {onImport&&<FileUploads client={client} order={order} t={t} prepareImport={onImport}/>}
    <section className="settings-card"><h3 ref={workingFiles} tabIndex={-1}>{tr("Arbeitsdateien","Working files")}</h3><div className="actions"><button disabled={busy||directory==="."} onClick={()=>setDirectory(directory.includes("/")?directory.slice(0,directory.lastIndexOf("/")):".")}>{tr("Übergeordneter Ordner","Parent folder")}</button><button disabled={busy} onClick={()=>void listing.refetch()}>{tr("Aktualisieren","Refresh")}</button><code>{directory}</code></div>
      {listing.isError&&<p role="alert">{tr("Dateiliste konnte nicht geladen werden.","Could not load files.")}</p>}
      <ul>{listing.data?.entries.map(entry=><li key={entry.path}><button disabled={busy} onClick={()=>entry.kind==="directory"?setDirectory(entry.path):void open(entry.path)}>{entry.name}{entry.kind==="directory"?"/":""}</button></li>)}</ul>
      <label>{tr("Quelldatei","Source file")}<input value={source} disabled={busy} onChange={e=>{setSource(e.target.value);setSelected(null);}}/></label>
      <div className="actions"><button disabled={busy||!source} onClick={()=>void open()}>{tr("Öffnen","Open")}</button>{["text","inspect","metadata","preview"].map(operation=><button disabled={locked||!source||!folder} key={operation} onClick={()=>read(operation)}>{({text:tr("Text lesen","Read text"),inspect:tr("Inhalt bearbeiten","Edit content"),metadata:tr("Metadaten","Metadata"),preview:tr("Vorschau rendern","Render preview")} as Record<string,string>)[operation]}</button>)}</div>
      {selected&&<><button disabled={busy} onClick={()=>void perform(async()=>{await deliverDownload(selected.file.blob,selected.file.file_name,language,selected.file);})}>{tr("Herunterladen","Download")}</button>{/\.tex$/i.test(selected.path)&&<button disabled={locked||mode==="read_only"} onClick={()=>void perform(async()=>{setBody(new TextDecoder("utf-8",{fatal:true}).decode(await selected.file.blob.arrayBuffer()));setFormat("tex");setOutput(selected.file.file_name);setNotice(tr("Quelltext im Entwurf geöffnet. Veröffentlichung erfolgt über den Dateiimport.","Source opened in the draft. Publish through file import."));})}>{tr("Quelltext bearbeiten","Edit source")}</button>}<DocumentPreview file={selected.file}/></>}
    </section>
    {inspection&&<OfficeBinaryEditor key={JSON.stringify([inspection.folder,inspection.model.input,inspection.model.source_sha256,inspection.model.offset])} model={inspection.model} locked={locked||mode==="read_only"} draftKey={officeEditorDraftKey(client.base,order,inspection.model)} prepare={async action=>{
      if(String(action.operation).startsWith("inspect_")){await prepare({id:"ui",name:"document_read",arguments:{operation:"inspect",folder_id:inspection.folder,path:inspection.model.input,expected_sha256:inspection.model.source_sha256,...JSON.parse(String(action.text))}});return;}
      const spec=JSON.parse(String(action.text));
      await prepare({id:"ui",name:"document_write",arguments:{operation:"edit",inputs:[{folder_id:inspection.folder,path:inspection.model.input,expected_sha256:inspection.model.source_sha256}],output:{path:String(action.output).split("/").at(-1)!},spec,preview:true}});
    }}/>}
    {pdfInspection&&<PdfFormEditor key={JSON.stringify([pdfInspection.folder,pdfInspection.path,pdfInspection.sha256])} {...pdfInspection} locked={locked||mode==="read_only"} prepare={prepare}/>}
    <form className="settings-card" onSubmit={e=>void create(e)}><fieldset disabled={locked||mode==="read_only"}><legend>{tr("Dokument erstellen","Create document")}</legend>
      <label>{tr("Format","Format")}<select value={format} onChange={e=>{setFormat(e.target.value);setOutput("report."+e.target.value);}}>{["docx","xlsx","pptx","pdf","tex"].map(f=><option key={f} value={f}>{f.toUpperCase()}</option>)}</select></label>
      <label>{tr("Titel","Title")}<input value={title} maxLength={300} onChange={e=>setTitle(e.target.value)}/></label>
      <label>{format==="xlsx"?tr("Tabelle als CSV; Formeln bleiben Text","CSV table; formulas remain text"):format==="pptx"?tr("Eine Folie pro Absatz; erste Zeile ist der Titel","One slide per paragraph; first line is its title"):format==="tex"?tr("LaTeX-Quelltext","LaTeX source"):tr("Inhalt als Markdown","Markdown content")}<textarea value={body} rows={12} maxLength={400000} required onChange={e=>setBody(e.target.value)}/></label>
      <label>{tr("Neue Ausgabedatei","New output file")}<input value={output} required onChange={e=>setOutput(e.target.value)}/></label><button type="submit">{tr("Erstellung prüfen","Review creation")}</button>
      </fieldset><div className="actions"><button type="button" disabled={busy} onClick={()=>void saveDraft()}>{tr("Entwurf speichern","Save draft")}</button><button type="button" disabled={busy} onClick={()=>void loadDraft()}>{tr("Entwurf laden","Load draft")}</button><button type="button" disabled={busy||!!draftSave.current} onClick={copyDraft}>{tr("Eigene Entwurfskopie","Independent draft copy")}</button><button type="button" disabled={busy} aria-expanded={showDraftHistory} onClick={()=>setShowDraftHistory(!showDraftHistory)}>{tr("Entwurfsverlauf","Draft history")}</button></div></form>
    {showDraftHistory&&<section className="settings-card"><h3>{tr("Gespeicherte Entwurfsversionen","Saved draft versions")}</h3>{draftHistory.isError&&<p role="alert">{tr("Noch kein gespeicherter Entwurf oder Verlauf nicht erreichbar.","No saved draft yet, or history is unavailable.")}</p>}<ul>{draftHistory.data?.map(snapshot=><li key={snapshot.id}><button disabled={busy||!!draftSave.current} onClick={()=>loadHistoricalDraft(snapshot.content)}>{new Date(snapshot.created_at).toLocaleString(language)} · {snapshot.conflict?tr("Konfliktkopie","Conflict copy"):tr("Version","Version")} {snapshot.base_revision+1} · {snapshot.sha256.slice(0,12)}</button></li>)}</ul></section>}
    <details className="settings-card"><summary>{tr("Office-Datei konvertieren / Excel neu berechnen","Convert Office file / recalculate Excel")}</summary><fieldset disabled={locked||mode==="read_only"||!folder||availability.data?.office_convert_available!==true}><p>{tr("Konvertierung mit LibreOffice. XLSX → XLSX berechnet unterstützte Formeln neu; die Formatierung kann sich ändern. Quelle, Ergebnis und Berechnungsbericht anschließend vergleichen.","Convert using LibreOffice. XLSX → XLSX recalculates supported formulas; formatting can change. Compare source, result and calculation report afterwards.")}</p><label>{tr("Zielformat","Target format")}<select value={convertFormat} onChange={e=>{setConvertFormat(e.target.value);setOutput("converted."+e.target.value);}}>{["pdf","docx","xlsx","pptx","odt","ods","odp"].map(value=><option key={value}>{value}</option>)}</select></label><label>{tr("Neue Ausgabedatei","New output file")}<input value={output} onChange={e=>setOutput(e.target.value)}/></label><button disabled={!source||!output} onClick={()=>void prepare({id:"ui",name:"document_write",arguments:{operation:"convert",inputs:[{folder_id:folder!,path:source,...(selected?.path===source?{expected_sha256:selected.file.sha256}:{})}],output:{path:output},spec:convertFormat==="xlsx"&&/\.xlsx$/i.test(source)?{recalculate:true}:{},preview:true}})}>{tr("Konvertierung prüfen","Review conversion")}</button></fieldset>{availability.data?.office_convert_available!==true&&<p>{availability.data?.office_convert_status==="data_path_too_long"?tr("LibreOffice benötigt einen kürzeren privaten Datenordner für diesen Arbeitsplatz.","LibreOffice needs a shorter private data directory for this workspace."):tr("Office-Konvertierung ist auf diesem Arbeitsplatz noch nicht bereit.","Office conversion is not ready on this workspace.")}</p>}</details>
    <details className="settings-card"><summary>{tr("PDF-Seiten bearbeiten","Edit PDF pages")}</summary><fieldset disabled={locked||mode==="read_only"||!folder}><label>{tr("Vorgang","Operation")}<select value={pdfAction} onChange={e=>setPdfAction(e.target.value)}>{["extract_pages","rotate","delete_pages","reorder","merge","split","watermark"].map(a=><option key={a}>{a}</option>)}</select></label><label>{tr("Seiten, Gradzahl, Wasserzeichentext oder weitere PDF-Pfade (je Zeile)","Pages, degrees, watermark text or additional PDF paths (one per line)")}<textarea value={pdfValue} onChange={e=>setPdfValue(e.target.value)}/></label><label>{tr("Neue Ausgabedatei oder Ordner bei split","New output file, or directory for split")}<input value={output} onChange={e=>setOutput(e.target.value)}/></label><button disabled={!source||!output} onClick={()=>{
      const spec:Record<string,unknown>={action:pdfAction};if(pdfAction==="rotate")spec.degrees=Number(pdfValue);else if(pdfAction==="reorder")spec.order=pdfValue.split(",").map(Number);else if(pdfAction==="watermark")spec.text=pdfValue;else if(!["merge","split"].includes(pdfAction))spec.pages=pdfValue;
      const paths=[source,...(pdfAction==="merge"?pdfValue.split("\n").map(p=>p.trim()).filter(Boolean):[])];void prepare({id:"ui",name:"document_write",arguments:{operation:"pdf",inputs:paths.map(inputRef),output:{path:output},spec,preview:true}});
    }}>{tr("PDF-Änderung prüfen","Review PDF change")}</button></fieldset></details>
    {selected&&folder&&/\.(png|jpe?g|webp|gif|bmp|tiff?)$/i.test(selected.path)?<ImagePipelineEditor key={JSON.stringify([folder,selected.path,selected.file.sha256])} source={{folder,path:selected.path,sha256:selected.file.sha256}} value={image} onChange={setImage} locked={locked||mode==="read_only"} prepare={prepare}/>:<p>{tr("Öffne eine Bilddatei, um ihre Bearbeitungsschritte zusammenzustellen.","Open an image file to assemble its editing steps.")}</p>}
    <LatexProjectEditor client={client} order={order} folder={folder} value={latex} onChange={value=>{setLatex(value);setLatexSource(null);}} seed={[source,...assets.split("\n")].filter(Boolean).join("\n")} locked={locked||mode==="read_only"} ready={availability.data?.latex_available===true} prepare={prepare} onOpen={openLatexSource} source={latexSource} onBusyChange={setBusy}/>
    {pending&&<section className="settings-card"><h3>{tr("Dokumentaktion","Document action")}</h3>{prepared?<><DocumentToolSummary name={prepared.tool.name} args={prepared.tool.arguments} folders={workspace.data?[workspace.data.folder]:[]} t={t}/><code>{prepared.fingerprint}</code><p>{t(prepared.access_mode)}</p>{!runId&&<button disabled={busy} onClick={()=>void commit()}>{pending.phase==="commit"?tr("Ursprünglichen Start abgleichen","Reconcile original start"):tr("Ausführung starten","Start execution")}</button>}</>:<p>{tr("Die ursprüngliche Vorbereitung ist noch nicht bestätigt.","The original preparation is not confirmed yet.")}</p>}<button disabled={busy} onClick={()=>void recover()}>{tr("Ursprüngliche Aktion abrufen","Retrieve original action")}</button><button disabled={busy||pending.phase==="commit"&&!terminal(detail.data?.run.state)} onClick={release}>{pending.phase==="prepare"?tr("Vorbereitung schließen","Close preparation"):tr("Nächste Aktion","Next action")}</button></section>}
    {detail.data&&<section className="settings-card"><h3>{t(detail.data.run.state as Key)}</h3>{detail.data.run.error_code&&<p role="alert">{detail.data.run.error_code}</p>}{terminal(detail.data.run.state)&&detail.data.approvals.some(a=>a.decision===null)&&<p role="status">{tr("Diese Freigabeanfrage ist abgelaufen. Bereite eine neue Dokumentaktion vor.","This approval request has expired. Prepare a new document action.")}</p>}{detail.data.approvals.filter(a=>a.decision===null&&!terminal(detail.data.run.state)).map(a=><div key={a.id}><DocumentToolSummary name={a.tool.name} args={a.tool.arguments} folders={detail.data!.run.spec.folders??[]} t={t}/>{[true,false].map(allow=><button key={String(allow)} disabled={busy} onClick={()=>void perform(async()=>{await client.approve(runId!,a.id,{fingerprint:a.fingerprint,allow});await detail.refetch();})}>{allow?tr("Erlauben","Allow"):tr("Ablehnen","Deny")}</button>)}</div>)}{!terminal(detail.data.run.state)&&<button disabled={busy} onClick={()=>void perform(async()=>{await client.cancel(runId!);await detail.refetch();})}>{tr("Abbrechen","Cancel")}</button>}
      {typeof data.text==="string"&&<pre className="document-source">{data.text}</pre>}
      <DocumentDiagnostics engine={engine} openSource={diagnosticProject?((file,line)=>{const member=latexDiagnosticFile(diagnosticProject,file);if(member)void perform(()=>openLatexSource(member.path,line));}):undefined} canOpenSource={file=>!!diagnosticProject&&!!latexDiagnosticFile(diagnosticProject,file)}/>
      {(detail.data.file_operations??[]).map(operation=><p key={operation.tool_call_id}><button disabled={busy} onClick={()=>void open(operation.path,true)}>{tr("Ergebnis öffnen","Open result")}: {operation.path}</button></p>)}
      {!!events.data?.length&&<ToolPreviews client={client} run={runId!} events={events.data} t={t}/>}
      <details><summary>{tr("Ergebnis und Diagnosen","Result and diagnostics")}</summary><pre className="document-source">{JSON.stringify(engine,null,2)}</pre></details>
    </section>}
  </section>;
}
