import {useMemo,useState} from "react";
import type {OfficeItem,OfficeModel} from "./officeDocuments";
import {officePatchAction} from "./officeDocuments";
import {useView} from "./view";
import {readOfficeEditorDraft,saveOfficeEditorDraft,type OfficeEditorFields} from "./officeEditorDraft";
export function OfficeBinaryEditor({model,locked,prepare,draftKey}:{model:OfficeModel,locked:boolean,prepare:(action:Record<string,unknown>)=>Promise<void>,draftKey:string}){
  const language=useView(s=>s.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const [initial]=useState(()=>{try{return {fields:readOfficeEditorDraft(localStorage,draftKey,model),error:""};}catch{return {fields:{},error:tr("Der gespeicherte Dokumententwurf konnte nicht gelesen werden. Verwirf ihn ausdrücklich, um weiterzuarbeiten.","The saved document draft could not be read. Explicitly discard it to continue.")};}});
  const [fields,setFields]=useState<OfficeEditorFields>(initial.fields),[error,setError]=useState(""),[storageError,setStorageError]=useState(initial.error),[broken,setBroken]=useState(!!initial.error);
  function patchFor(item:OfficeItem,text:string,type:string){let patch:Record<string,unknown>;
      if(model.format==="docx")patch={paragraph:item.paragraph,text};
      else if(model.format==="pptx")patch={slide:item.slide,shape:item.shape,text};
      else {let value:string|number|boolean|null=text;if(type==="number"){value=Number(text);if(!text.trim()||!Number.isFinite(value))throw new Error(tr("Eine endliche Zahl eingeben.","Enter a finite number."));}if(type==="boolean"){if(!["true","false"].includes(text))throw new Error("true / false");value=text==="true";}if(type==="empty")value=null;if(type==="formula"&&!text.startsWith("="))throw new Error(tr("Formeln beginnen mit =.","Formulas start with =."));patch={sheet:item.sheet,cell:item.cell,value,type,number_format:item.number_format};}
      return patch;
  }
  const {patches,invalid}=useMemo(()=>{const patches:Record<string,unknown>[]=[],invalid:Record<string,string>={};for(const [index,field] of Object.entries(fields)){try{patches.push(patchFor(model.items[Number(index)],field.text,field.type));}catch(error){invalid[index]=String(error instanceof Error?error.message:error);}}return {patches,invalid};},[fields,model,language]);
  function edit(index:number,_item:OfficeItem,text:string,type="text"){
    const next={...fields,[index]:{text,type}};setFields(next);setError("");
    try{saveOfficeEditorDraft(localStorage,draftKey,model,next);setStorageError("");}catch{setStorageError(tr("Der Entwurf ist nur in diesem Fenster verfügbar: lokale Speicherung fehlgeschlagen.","This draft is available only in this window: local saving failed."));}
  }
  function discard(){try{localStorage.removeItem(draftKey);setFields({});setBroken(false);setStorageError("");setError("");}catch{setStorageError(tr("Der lokale Entwurf konnte nicht gelöscht werden.","The local draft could not be removed."));}}
  async function save(){try{await prepare(officePatchAction(model,patches,`documents/${crypto.randomUUID()}.${model.format}`));setError("");}catch(error){setError(String(error instanceof Error?error.message:error));}}
  return <section className="settings-card"><h3>{tr("Dokument bearbeiten","Edit document")}</h3><p>{tr("Änderungen erzeugen eine neue Datei und eine unveränderliche Version. Word-Absatztext behält den ersten Textlauf; Folientext wird im gewählten Textobjekt ersetzt. Prüfe die erzeugte Kopie, besonders bei komplexen Office-Objekten. Das Original bleibt erhalten.","Edits create a new file and immutable version. Word paragraphs retain the first text run; slide text is replaced within the selected text object. Review the generated copy, especially for complex Office objects. The original remains available.")}</p><p>{model.offset+1}–{model.offset+model.items.length} / {model.total} · SHA256 <code>{model.source_sha256}</code></p><fieldset disabled={locked}>
    <p>{tr("Entwürfe werden auf diesem Gerät pro Chat und Dateiversion gespeichert. Eine Ausführung erfolgt erst nach Prüfung. Nach erfolgreicher Erstellung kannst du den Entwurf verwerfen.","Drafts are saved on this device per chat and file version. Execution requires review. You can discard the draft after creating the copy.")}</p>{storageError&&<p role="alert">{storageError}</p>}
    {model.items.map((item,index)=>{const field=fields[String(index)],value=field?.text??item.text??item.value??"",type=String(field?.type??item.type??"text");return <div className="settings-card" key={index}><label>{model.format==="docx"?`${tr("Absatz","Paragraph")} ${Number(item.paragraph)+1}`:model.format==="pptx"?`${tr("Folie","Slide")} ${Number(item.slide)+1} · ${item.name}`:`${item.sheet}!${item.cell}`}
      {model.format==="xlsx"&&<select aria-label={tr("Zellentyp","Cell type")} value={type} disabled={broken||item.truncated} onChange={event=>edit(index,item,String(value),event.target.value)}>{["text","number","boolean","formula","empty"].map(type=><option key={type} value={type}>{type}</option>)}</select>}
      <textarea rows={model.format==="xlsx"?2:4} value={String(value)} disabled={item.truncated||broken} maxLength={model.format==="xlsx"?64000:128000} onChange={event=>edit(index,item,event.target.value,type)}/></label>{item.truncated&&<p role="status">{tr("Dieser Text ist für den Editor zu lang und kann hier nicht geändert werden.","This text exceeds the editor limit and cannot be changed here.")}</p>}</div>;})}
    <button type="button" disabled={broken||!patches.length||Object.keys(invalid).length>0} onClick={()=>void save()}>{tr("Neue Dokumentversion prüfen","Review new document version")}</button>
    {(broken||Object.keys(fields).length>0)&&<button type="button" onClick={discard}>{tr("Lokalen Entwurf verwerfen","Discard local draft")}</button>}
    {model.next_offset!==null&&<button type="button" disabled={broken} onClick={()=>void prepare({kind:"office",operation:"inspect_"+model.format,input:model.input,text:JSON.stringify({offset:model.next_offset,limit:20})})}>{tr("Nächste Seite laden","Load next page")}</button>}
    </fieldset>{error&&<p role="alert">{error}</p>}{Object.entries(invalid).map(([index,error])=><p role="alert" key={index}>{Number(index)+1}: {error}</p>)}</section>;
}
