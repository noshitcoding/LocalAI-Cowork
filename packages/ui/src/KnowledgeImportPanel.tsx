import {useRef,useState,type FormEvent} from "react";
import {decode,type Client} from "@cowork/client";
import {productRequest,type Knowledge} from "./productApi";
import {useView} from "./view";
import type {MemoryMetadata} from "./memoryMetadata";
import {binaryKnowledgeFormat,extractKnowledgeSource,knowledgeImportLatch,knowledgeImportLimits,knowledgeImportSource,knowledgeImportTags,previewKnowledgeImport,type KnowledgeImportSource,type KnowledgeImportChunk,type KnowledgeImportResult} from "./knowledgeImport";

type ImportProps={client:Client,project:string,onImported:()=>Promise<unknown>,
  /** Fixed memory metadata, e.g. the personal (user-scoped) knowledge base. */
  memory?:MemoryMetadata,
  /** Separates the retained import journal of different views of the same project. */
  journalScope?:string,
  folders?:{id:string,path:string}[]};
export function KnowledgeImportPanel(props:ImportProps){
  return <KnowledgeImportContext key={JSON.stringify([props.client.base,props.client.token,props.project,props.journalScope??""])} {...props}/>;
}
function KnowledgeImportContext({client,project,onImported,memory,journalScope,folders}:ImportProps){
  const language=useView(value=>value.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const personal=memory?.scope.kind==="user";
  const journal=useRef(knowledgeImportLatch(client,journalScope&&journalScope!=="project"?project+"#"+journalScope:project));
  const [original]=useState(()=>journal.current.pending&&!journal.current.invalid?journal.current.prepare(()=>{throw Error("request_journal_invalid");}):null);
  const [sources,setSources]=useState<KnowledgeImportSource[]>([]),[preview,setPreview]=useState<KnowledgeImportChunk[]>(()=>original?.documents.map((document,index)=>({source:document.title,index:index+1,total:original.documents.length,start:0,end:Array.from(document.content).length,firstLine:1,lastLine:document.content.split("\n").length,document:document as KnowledgeImportChunk["document"]}))??[]),[kind,setKind]=useState<Knowledge["kind"]>("knowledge"),[tags,setTags]=useState(""),[chunkSize,setChunkSize]=useState(3000);
  const [error,setError]=useState(""),[busy,setBusy]=useState(false),[pending,setPending]=useState(journal.current.pending),[result,setResult]=useState(0),[paste,setPaste]=useState(""),[title,setTitle]=useState(""),[reading,setReading]=useState("");
  const [folder,setFolder]=useState(()=>original?.documents[0]?.location?.folder_id??"");
  const inFlight=useRef(false),files=useRef<HTMLInputElement>(null);
  const locked=busy||pending;
  const problem=(error:unknown)=>{
    const code=error instanceof Error?error.message:String(error);
    const known:Record<string,[string,string]>={knowledge_import_size_limit:["Maximal 1 MiB Text pro Import.","Import up to 1 MiB of text at a time."],knowledge_import_source_limit:["Wähle 1 bis 20 Quellen.","Select 1 to 20 sources."],knowledge_import_chunk_limit:["Der Import darf höchstens 200 Abschnitte enthalten. Wähle weniger Dateien oder größere Abschnitte.","An import can contain up to 200 chunks. Select fewer files or larger chunks."],knowledge_import_text_required:["Die Quelle muss nicht leeren UTF-8-Text ohne Nullzeichen enthalten.","The source must contain nonempty UTF-8 text without null characters."],knowledge_import_blank_chunk:["Die Quelle enthält einen zu langen leeren Abschnitt. Entferne überflüssige Leerzeilen.","The source contains an oversized blank section. Remove excess blank lines."],invalid_knowledge_chunk_size:["Wähle 500 bis 4000 Zeichen pro Abschnitt.","Choose 500 to 4000 characters per chunk."],invalid_knowledge_tags:["Maximal 32 unterschiedliche Tags mit jeweils 80 UTF-8-Bytes.","Use up to 32 unique tags, each up to 80 UTF-8 bytes."]};
    if(code.startsWith("knowledge_extraction_"))return tr("Dokumentextraktion nicht bestätigt. PDF/DOCX/XLSX/PPTX: maximal 16 MiB je Datei, 200 Seiten/Folien und 400.000 UTF-8-Bytes extrahierter Text. Python mit den Dokumentbibliotheken muss eingerichtet sein. Verschlüsselte PDFs und reine Scanbilder benötigen vorher einen eigenen Textexport.","Document extraction was not confirmed. PDF/DOCX/XLSX/PPTX: at most 16 MiB per file, 200 pages/slides, and 400,000 extracted UTF-8 bytes. Configure Python with document libraries. Encrypted PDFs and scanned images require a separate text export.");
    return known[code]?.[language==="de"?0:1]??(error instanceof TypeError?tr("Die Datei konnte nicht als UTF-8-Text gelesen werden.","The file could not be read as UTF-8 text."):code);
  };
  function changed(){setPreview([]);setResult(0);setError("");}
  function append(next:KnowledgeImportSource[]){
    const combined=[...sources,...next];
    if(combined.length>knowledgeImportLimits.sources)throw new Error("knowledge_import_source_limit");
    if(combined.reduce((sum,source)=>sum+new TextEncoder().encode(source.text).byteLength,0)>knowledgeImportLimits.bytes)throw new Error("knowledge_import_size_limit");
    setSources(combined);changed();
  }
  async function selectFiles(list:FileList|null){
    if(!list?.length||inFlight.current||locked)return;inFlight.current=true;setBusy(true);setError("");
    try{
      const chosen=Array.from(list);if(chosen.length+sources.length>knowledgeImportLimits.sources)throw new Error("knowledge_import_source_limit");
      if(chosen.reduce((sum,file)=>sum+file.size,0)>64*1024*1024)throw new Error("knowledge_extraction_file_limit");
      const decoded:KnowledgeImportSource[]=[];
      for(const [index,file] of chosen.entries()){setReading(`${index+1}/${chosen.length} · ${file.name}`);if(file.size>(binaryKnowledgeFormat(file.name)?16*1024*1024:knowledgeImportLimits.bytes))throw Error("knowledge_extraction_file_limit");const bytes=new Uint8Array(await file.arrayBuffer());decoded.push(binaryKnowledgeFormat(file.name)?await extractKnowledgeSource(client,project,file.name,bytes,crypto.randomUUID()):await knowledgeImportSource(file.name,bytes));}append(decoded);
    }catch(error){setError(problem(error));}finally{inFlight.current=false;setReading("");setBusy(false);if(files.current)files.current.value="";}
  }
  async function addText(event:FormEvent){event.preventDefault();if(inFlight.current||locked)return;inFlight.current=true;setBusy(true);try{append([await knowledgeImportSource(title||tr("Eingefügter Text","Pasted text"),new TextEncoder().encode(paste))]);setPaste("");setTitle("");}catch(error){setError(problem(error));}finally{inFlight.current=false;setBusy(false);}}
  function prepare(){try{setPreview(previewKnowledgeImport(sources,{kind,tags:knowledgeImportTags(tags),chunkCharacters:chunkSize}).map(chunk=>folders?{...chunk,document:{...chunk.document,location:{folder_id:folder||null}}}:chunk));setError("");}catch(error){setError(problem(error));}}
  async function commit(){
    if(inFlight.current||journal.current.invalid)return;inFlight.current=true;
    setBusy(true);setPending(true);setError("");
    try{
      const operation=journal.current.prepare(()=>({idempotency_key:crypto.randomUUID(),documents:preview.map(chunk=>memory?{...chunk.document,kind:"memory" as const,memory}:chunk.document)}));
      const saved:KnowledgeImportResult=decode("KnowledgeImportResult",await productRequest<unknown>(client,`/projects/${project}/knowledge/import`,"POST",operation));
      if(saved.project_id!==project||saved.documents.length!==operation.documents.length||saved.documents.some((document,index)=>document.content!==operation.documents[index].content||document.title!==operation.documents[index].title||operation.documents[index].location!=null&&(document.folder_id??null)!==(operation.documents[index].location?.folder_id??null)))throw Error("knowledge_extraction_integrity");
      journal.current.confirm();setResult(saved.documents.length);setPending(false);setPreview([]);setSources([]);
      // A refresh failure must never make a committed batch look like a failed import.
      await onImported().catch(()=>undefined);
    }catch(error){
      setError(problem(error));
      if(error instanceof Error&&/\(4\d\d\)$/.test(error.message)){journal.current.confirm();setPending(false);}
      else setPending(journal.current.pending);
    }finally{inFlight.current=false;setBusy(false);}
  }
  return <details className="knowledge-import settings-card"><summary>{tr("Dateien oder Text importieren","Import files or text")}</summary>
    {folders&&<label>{tr("Zielordner für den Import","Import destination folder")}<select value={folder} disabled={locked} onChange={e=>{setFolder(e.target.value);changed();}}><option value="">{tr("Ohne Ordner","Unfiled")}</option>{folders.map(f=><option key={f.id} value={f.id}>{f.path}</option>)}</select></label>}
    <p className="muted">{tr(`Importiere UTF-8-Dateien sowie PDF, DOCX, XLSX und PPTX. Dokumenttext wird mit Seiten-, Absatz-, Tabellen- oder Folienbezeichnungen tatsächlich extrahiert. Die Vorschau zeigt genau die Abschnitte, die ${personal?"in deiner persönlichen Wissensbasis":"im aktuellen Projekt"} gespeichert werden. Neue Läufe wählen passende Einträge als Kontext aus.`,`Import UTF-8 files plus PDF, DOCX, XLSX and PPTX. Document text is actually extracted with page, paragraph, sheet or slide labels. The preview shows the exact chunks saved ${personal?"to your personal knowledge base":"to the current project"}. New runs select relevant entries as context.`)}</p>
    <fieldset disabled={locked}><legend>{tr("Quellen","Sources")}</legend><label>{tr("Dateien auswählen","Choose files")}<input ref={files} type="file" multiple accept=".txt,.md,.markdown,.csv,.json,.jsonl,.yaml,.yml,.xml,.html,.log,.pdf,.docx,.xlsx,.pptx,text/*,application/json,application/pdf" onChange={event=>void selectFiles(event.target.files)}/></label><small>{tr("Bis zu 20 Quellen, 1 MiB Text und 200 Abschnitte pro Import. Dokumente: 16 MiB je Datei, 64 MiB pro Auswahl und 400.000 extrahierte UTF-8-Bytes je Datei. Formeln werden als Text gelesen; Scan-PDFs benötigen OCR außerhalb dieses Imports.","Up to 20 sources, 1 MiB of text and 200 chunks per import. Documents: 16 MiB each, 64 MiB per selection and 400,000 extracted UTF-8 bytes per file. Formulas are read as text; scanned PDFs require OCR outside this import.")}</small>
      <form onSubmit={event=>void addText(event)}><label>{tr("Titel für eingefügten Text","Pasted text title")}<input value={title} maxLength={200} onChange={event=>setTitle(event.target.value)}/></label><label>{tr("Text einfügen","Paste text")}<textarea rows={5} value={paste} onChange={event=>setPaste(event.target.value)}/></label><button type="submit" disabled={!paste.trim()}>{tr("Text zu den Quellen hinzufügen","Add text to sources")}</button></form>
      {sources.length>0&&<ul className="knowledge-import-sources">{sources.map((source,index)=><li key={index}><span>{source.name} <small>· {Array.from(source.text).length.toLocaleString(language)} {tr("Zeichen","characters")}</small></span><button type="button" onClick={()=>{setSources(previous=>previous.filter((_,position)=>position!==index));changed();}} aria-label={tr("Quelle entfernen: ","Remove source: ")+source.name}>{tr("Entfernen","Remove")}</button></li>)}</ul>}
    </fieldset>
    <fieldset disabled={locked}><legend>{tr("Abschnitte vorbereiten","Prepare chunks")}</legend><div className="settings-field-grid">{!memory&&<label>{tr("Typ","Type")}<select value={kind} onChange={event=>{setKind(event.target.value as Knowledge["kind"]);changed();}}><option value="knowledge">{tr("Wissen","Knowledge")}</option><option value="memory">Memory</option></select></label>}<label>{tr("Maximale Zeichen pro Abschnitt","Maximum characters per chunk")}<input type="number" min={500} max={4000} step={100} value={chunkSize} onChange={event=>{setChunkSize(Number(event.target.value));changed();}}/></label><label>{tr("Tags, durch Kommas getrennt","Comma-separated tags")}<input value={tags} onChange={event=>{setTags(event.target.value);changed();}}/></label></div><button type="button" disabled={!sources.length} onClick={prepare}>{tr("Vorschau erstellen","Create preview")}</button></fieldset>
    {reading&&<p role="status">{tr("Quelle wird gelesen: ","Reading source: ")}{reading}</p>}
    {journal.current.invalid&&<aside role="alert"><p>{tr("Die gespeicherte Importreferenz ist ungültig. Neue Importe bleiben gesperrt.","The saved import reference is invalid. New imports remain blocked.")}</p><button type="button" disabled={busy} onClick={()=>{journal.current.discard();setPending(false);}}>{tr("Ungültige lokale Referenz ausdrücklich verwerfen","Explicitly discard invalid local reference")}</button></aside>}
    {error&&<p className="error" role="alert">{error}</p>}{result>0&&<p role="status">{tr(`${result} Abschnitte ${personal?"in deiner persönlichen Wissensbasis":"im Projekt"} gespeichert.`,`${result} chunks saved ${personal?"to your personal knowledge base":"to the project"}.`)}</p>}
    {preview.length>0&&<section aria-label={tr("Importvorschau","Import preview")}><h3>{tr("Vorschau","Preview")} · {preview.length} {tr("Abschnitte","chunks")}</h3><p className="muted">{tr("Originaltexte werden aufgeteilt und mit SHA256 und Fundstelle gespeichert. Die Quellenangabe ist deklariert; sie ersetzt keinen gespeicherten Dateisnapshot.","Original text is split and saved with its SHA256 and location. The source citation is declared; it does not establish a stored file snapshot.")}</p><div className="knowledge-import-preview">{preview.map((chunk,index)=><details key={index}><summary>{chunk.document.title} · {chunk.document.provenance.sources[0]?.locator}</summary><pre>{chunk.document.content}</pre><small>{chunk.document.provenance.sources.map(source=>source.sha256).join(" · ")}</small></details>)}</div><button className="primary" type="button" disabled={busy} onClick={()=>void commit()}>{busy?tr("Wird gespeichert…","Saving…"):pending?tr("Diesen Import erneut bestätigen","Retry this import"):tr("Alle Abschnitte importieren","Import all chunks")}</button>{pending&&!busy&&<p role="status">{tr("Die Antwort ist unklar. Derselbe Importbeleg bleibt auch nach Neustart erhalten; der erneute Versuch erzeugt keine doppelten Einträge.","The response is uncertain. The same import receipt is retained across restarts; retrying creates no duplicate entries.")}</p>}</section>}
  </details>;
}
