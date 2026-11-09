import {useEffect,useMemo,useState,type FormEvent} from "react";
import {useQuery} from "@tanstack/react-query";
import {uploadMetadata,type Client,type DownloadedFile} from "@cowork/client";
import {DocumentPreview} from "./DocumentPreview";
import {documentKind} from "./documentFiles";
import {deliverDownload} from "./download";
import {useToolboxAction,ToolboxActionReview} from "./ToolboxActionReview";
import {downloadToolboxFile,successfulToolboxResult,toolboxAvailability} from "./toolboxWorkbench";
import {useView} from "./view";
import {OfficeBinaryEditor} from "./OfficeBinaryEditor";
import {documentVersions,downloadDocumentVersion,officeFormat,type DocumentVersion,type OfficeModel} from "./officeDocuments";
import {toolboxEndpoint} from "./toolboxPending";
import {officeEditorDraftKey} from "./officeEditorDraft";
import {DocumentWorkbench} from "./DocumentWorkbench";
import type {AccessMode,FileUpload} from "@cowork/client";
type SavedTab={key:string,path:string,sha256:string,file_name:string,historical:boolean};

export function officeReadOperation(path:string):string|null{
  const extension=path.toLowerCase().split(".").at(-1);
  return ({docx:"read_docx",xlsx:"read_xlsx",pptx:"read_pptx",pdf:"read_pdf",txt:"read_text",md:"read_text",csv:"read_text",json:"read_text",html:"read_text"} as Record<string,string>)[extension??""]??null;
}
export function OfficeWorkbench({client,order,mode="confirm_mutations",onImport}:{client:Client,order:string,mode?:AccessMode,onImport?:(upload:FileUpload)=>void}){
  const scope=useMemo(()=>crypto.randomUUID(),[client]);
  return <><DocumentWorkbench key={scope+order} client={client} order={order} mode={mode} onImport={onImport}/><details><summary>Office · Benutzerrechte / Owner tools</summary><OfficeWorkspace key={"legacy"+scope+order} client={client} order={order}/></details></>;
}
function OfficeWorkspace({client,order}:{client:Client,order:string}){
  const language=useView(value=>value.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const availability=useQuery({queryKey:["toolbox-workbench",client.base],queryFn:()=>toolboxAvailability(client),retry:false});
  const action=useToolboxAction(client,order,"office"),[path,setPath]=useState(""),[file,setFile]=useState<DownloadedFile|null>(null),[error,setError]=useState(""),[notice,setNotice]=useState(""),[busy,setBusy]=useState(false),[format,setFormat]=useState("docx");
  const [tabs,setTabs]=useState<{key:string,path:string,file:DownloadedFile,historical:boolean}[]>([]),[active,setActive]=useState("");
  const [models,setModels]=useState<Record<string,OfficeModel>>({});
  const tabsKey="cowork-v2:document-tabs:v1:"+encodeURIComponent(JSON.stringify([toolboxEndpoint(client.base),order]));
  const [recent,setRecent]=useState<SavedTab[]>(()=>{try{const items=JSON.parse(localStorage.getItem(tabsKey)??"[]");return Array.isArray(items)?items.filter(item=>item&&typeof item.key==="string"&&typeof item.path==="string"&&item.path.length<=4096&&typeof item.file_name==="string"&&/^[0-9a-f]{64}$/.test(item.sha256)&&typeof item.historical==="boolean").slice(-12):[];}catch{return [];}});
  const history=useQuery({queryKey:["document-versions",client.base,order,path,action.job?.id,action.job?.state],queryFn:()=>documentVersions(client,order,path),enabled:!!path,retry:false});
  const locked=busy||action.locked,readOperation=officeReadOperation(path),canOffice=availability.data?.office_available===true,canConvert=canOffice&&availability.data?.programs.includes("libreoffice");
  const result=useMemo(()=>successfulToolboxResult(action.job),[action.job]),data=result&&typeof result==="object"?result as {text?:unknown,output?:unknown,model?:OfficeModel}:null;
  const modelKey=JSON.stringify([path,file?.sha256]),model=data?.model?.input===path&&data.model.source_sha256===file?.sha256?data.model:models[modelKey]??null;
  useEffect(()=>{const next=data?.model;if(!next||!["docx","xlsx","pptx"].includes(next.format)||!Array.isArray(next.items)||!tabs.some(tab=>tab.path===next.input&&tab.file.sha256===next.source_sha256&&!tab.historical))return;const key=JSON.stringify([next.input,next.source_sha256]);setModels(old=>old[key]===next?old:{...old,[key]:next});},[data?.model,tabs]);
  function addTab(key:string,selected:string,saved:DownloadedFile,historical=false){setTabs(old=>[...old.filter(tab=>tab.key!==key),{key,path:selected,file:saved,historical}].slice(-12));setActive(key);setPath(selected);setFile(saved);setRecent(previous=>{const next=[...previous.filter(tab=>tab.key!==key),{key,path:selected,sha256:saved.sha256,file_name:saved.file_name,historical}].slice(-12);try{localStorage.setItem(tabsKey,JSON.stringify(next));}catch{setError(tr("Dokumenttabs konnten nicht für den Neustart gespeichert werden.","Document tabs could not be saved for restart."));}return next;});}
  async function reopen(tab:SavedTab){if(locked)return;setBusy(true);try{const saved=tab.historical?await downloadDocumentVersion(client,order,{id:tab.key,sha256:tab.sha256}):await downloadToolboxFile(client,order,tab.path,tab.sha256);addTab(tab.key,tab.path,saved,tab.historical);}catch(error){setError(String(error instanceof Error?error.message:error));}finally{setBusy(false);}}
  const output=typeof data?.output==="string"&&data.output===action.request?.action.output?data.output:null;
  async function open(selected=path,expected?:string){
    if(!selected||locked)return;setBusy(true);setError("");setNotice("");
    try{addTab(selected,selected,await downloadToolboxFile(client,order,selected,expected));}catch(error){setError(error instanceof Error?error.message:String(error));}finally{setBusy(false);}
  }
  async function upload(event:FormEvent<HTMLFormElement>){
    event.preventDefault();if(locked)return;const form=new FormData(event.currentTarget),source=form.get("file");if(!(source instanceof File)||!source.size)return;
    setBusy(true);setError("");setNotice("");
    try{
      if(source.size>16*1024*1024)throw new Error(tr("Dateien dürfen höchstens 16 MiB groß sein.","Files must not exceed 16 MiB."));
      const target=String(form.get("path")||source.name),metadata=await uploadMetadata(source,source.name);
      const response=await fetch(client.base+`/api/v2/orders/${encodeURIComponent(order)}/toolbox/files?`+new URLSearchParams({path:target}),{method:"PUT",headers:{Authorization:"Bearer "+client.token,"Content-Type":"application/octet-stream"},body:source});
      if(!response.ok)throw new Error(`upload_failed (${response.status})`);
      const saved=await downloadToolboxFile(client,order,target,metadata.sha256);addTab(target,target,saved);void history.refetch();setNotice(tr("Datei hochgeladen und geprüft.","File uploaded and verified."));
    }catch(error){setError(error instanceof Error?error.message:String(error));}finally{setBusy(false);}
  }
  async function save(){if(!file||locked)return;setBusy(true);try{const status=await deliverDownload(file.blob,file.file_name,language,file);setNotice(status==="saved"?tr("Datei gespeichert.","File saved."):status==="canceled"?tr("Speichern abgebrochen.","Save canceled."):tr("Download bereitgestellt.","Download prepared."));}catch(error){setError(error instanceof Error?error.message:String(error));}finally{setBusy(false);}}
  async function create(event:FormEvent<HTMLFormElement>){event.preventDefault();const form=new FormData(event.currentTarget);await action.prepare({kind:"office",operation:"create_"+format,title:String(form.get("title")??""),text:String(form.get("content")??""),output:`documents/${crypto.randomUUID()}.${format}`});}
  async function openVersion(version:DocumentVersion){if(locked)return;setBusy(true);try{addTab(version.id,version.path,await downloadDocumentVersion(client,order,version),true);}catch(error){setError(String(error instanceof Error?error.message:error));}finally{setBusy(false);}}
  function closeTab(key:string){if(locked)return;const remaining=tabs.filter(tab=>tab.key!==key);setTabs(remaining);if(active===key){const next=remaining.at(-1);setActive(next?.key??"");setPath(next?.path??"");setFile(next?.file??null);}}
  const historical=tabs.find(tab=>tab.key===active)?.historical===true;
  return <section className="office-workbench"><header className="settings-section-heading"><h2>{tr("Dokument-Workbench","Document workbench")}</h2><p>{tr("Öffne Dateien im verwalteten Werkzeug-Arbeitsverzeichnis dieses Chats. Dokumentaktionen werden vor ihrer Ausführung geprüft.","Open files in this chat's managed tool workspace. Review document actions before execution.")}</p></header>
    {availability.isError&&<p role="alert">{tr("Die verfügbaren Dokumentwerkzeuge konnten nicht geladen werden.","Available document tools could not be loaded.")}</p>}
    <details className="settings-card"><summary>{tr("Datei hochladen","Upload a file")}</summary><form onChange={()=>action.invalidatePreview()} onSubmit={event=>void upload(event)}><fieldset disabled={locked}><label>{tr("Datei","File")}<input name="file" type="file" required/></label><label>{tr("Relativer Zielpfad (optional)","Relative destination path (optional)")}<input name="path" placeholder="documents/report.docx"/></label><button type="submit">{tr("Hochladen und öffnen","Upload and open")}</button></fieldset></form></details>
    <form className="settings-card" onChange={()=>action.invalidatePreview()} onSubmit={event=>{event.preventDefault();void open();}}><fieldset disabled={locked}><label>{tr("Dateipfad im Werkzeug-Arbeitsverzeichnis","Path in the tool workspace")}<input value={path} onChange={event=>setPath(event.target.value)} placeholder="documents/report.docx" required/></label><div className="actions"><button type="submit">{tr("Öffnen","Open")}</button><button type="button" disabled={!canOffice||!readOperation||!path} onClick={()=>void action.prepare({kind:"office",operation:readOperation!,input:path})}>{tr("Text extrahieren","Extract text")}</button><button type="button" disabled={!canConvert||!file||historical||path!==tabs.find(tab=>tab.key===active)?.path||!["office","html"].includes(documentKind(path))} onClick={()=>void action.prepare({kind:"office",operation:"convert",input:path,text:JSON.stringify({source_sha256:file!.sha256}),output:`previews/${crypto.randomUUID()}.pdf`})}>{tr("PDF-Vorschau erstellen","Create PDF preview")}</button>{file&&<button type="button" onClick={()=>void save()}>{tr("Geprüfte Datei herunterladen","Download verified file")}</button>}</div></fieldset>{!canConvert&&<p className="muted">{tr("PDF-Konvertierung benötigt die eingerichtete LibreOffice-Laufzeit.","PDF conversion requires a configured LibreOffice runtime.")}</p>}</form>
    <details className="settings-card"><summary>{tr("Dokument erstellen","Create a document")}</summary><form onChange={()=>action.invalidatePreview()} onSubmit={event=>void create(event)}><fieldset disabled={locked||!canOffice}><label>{tr("Format","Format")}<select value={format} onChange={event=>setFormat(event.target.value)}><option value="docx">Word · DOCX</option><option value="pdf">PDF</option><option value="xlsx">Excel · XLSX</option><option value="pptx">PowerPoint · PPTX</option></select></label><label>{tr("Titel","Title")}<input name="title" maxLength={300}/></label><label>{format==="xlsx"?tr("Tabelleninhalt als CSV","Table content as CSV"):tr("Inhalt","Content")}<textarea name="content" rows={8} maxLength={1_000_000} required/></label><button type="submit">{tr("Erstellung prüfen","Review creation")}</button></fieldset></form></details>
    {error&&<p role="alert" className="error">{error}</p>}{notice&&<p role="status">{notice}</p>}
    <ToolboxActionReview action={action}/>{output&&<button type="button" className="primary" disabled={locked} onClick={()=>void open(output,typeof (data as {output_sha256?:unknown})?.output_sha256==="string"?(data as {output_sha256:string}).output_sha256:undefined)}>{tr("Erzeugte Datei öffnen","Open generated file")}: {output}</button>}
    {typeof data?.text==="string"&&data.text&&<details className="settings-card" open><summary>{tr("Extrahierter Dokumenttext","Extracted document text")}</summary><pre className="document-source">{data.text}</pre></details>}
    {recent.some(saved=>!tabs.some(tab=>tab.key===saved.key))&&<details className="settings-card"><summary>{tr("Dokumenttabs wieder öffnen","Reopen document tabs")}</summary>{recent.filter(saved=>!tabs.some(tab=>tab.key===saved.key)).map(saved=><button key={saved.key} type="button" disabled={locked} onClick={()=>void reopen(saved)}>{saved.file_name}{saved.historical?" · "+tr("Version","Version"):""}</button>)}</details>}
    <div className="document-tabs" role="tablist" aria-label={tr("Offene Dokumente","Open documents")}>{tabs.map(tab=><span key={tab.key} role="presentation"><button type="button" role="tab" aria-selected={active===tab.key} disabled={locked} onClick={()=>{setActive(tab.key);setPath(tab.path);setFile(tab.file);}}>{tab.file.file_name}{tab.historical?" · "+tr("Version","Version"):""}</button><button type="button" disabled={locked} aria-label={tr("Dokumenttab schließen: ","Close document tab: ")+tab.file.file_name} onClick={()=>closeTab(tab.key)}>×</button></span>)}</div>
    {file&&<section className="settings-card"><div className="actions"><button type="button" disabled={locked||!canOffice||historical||!officeFormat(path)} onClick={()=>void action.prepare({kind:"office",operation:"inspect_"+officeFormat(path),input:path,text:JSON.stringify({offset:0,limit:20})})}>{tr("Inhalt bearbeiten","Edit content")}</button><button type="button" disabled={locked||!canOffice||historical} onClick={()=>void action.prepare({kind:"office",operation:"snapshot",input:path,text:JSON.stringify({source_sha256:file.sha256})})}>{tr("Unveränderliche Version sichern","Save immutable version")}</button></div>{historical&&<p>{tr("Gesicherte Version: schreibgeschützt; Download enthält die gespeicherten Originalbytes.","Saved version: read only; download contains the retained original bytes.")}</p>}</section>}
    {model&&!historical&&<OfficeBinaryEditor key={officeEditorDraftKey(client.base,order,model)} draftKey={officeEditorDraftKey(client.base,order,model)} model={model} locked={locked} prepare={action.prepare}/>}
    <details className="settings-card"><summary>{tr("Dokumentversionen","Document versions")}</summary>{history.isError&&<p role="alert">{tr("Versionshistorie konnte nicht geladen werden.","Version history could not be loaded.")}</p>}{history.data?.map(version=><p key={version.id}><button type="button" disabled={locked} onClick={()=>void openVersion(version)}>v{version.revision} · {version.file_name} · {version.operation}</button> · {new Date(version.created_at).toLocaleString()} · <code>{version.sha256}</code></p>)}{history.data?.length===0&&<p>{tr("Noch keine gesicherten Versionen.","No retained versions yet.")}</p>}</details>
    {file&&<section className="document-workbench"><h3>{file.file_name}</h3><DocumentPreview file={file}/><details className="document-integrity"><summary>SHA256</summary><code>{file.sha256}</code></details></section>}
  </section>;
}
