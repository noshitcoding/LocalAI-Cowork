import {useEffect,useState} from "react";
import type {DownloadedFile} from "@cowork/client";
import {ChatMessageBody} from "./ChatMessageBody";
import {documentKind,isolatedHtml,previewMediaType,verifyDocument} from "./documentFiles";
import {useView} from "./view";
import "./documentWorkbench.css";

export function DocumentPreview({file,source=false}:{file:DownloadedFile,source?:boolean}){
  const language=useView(value=>value.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const [content,setContent]=useState<{digest:string,text:string,url:string}|null>(null),[error,setError]=useState("");
  const kind=documentKind(file.file_name,file.blob.type);
  useEffect(()=>{
    let live=true,url="";setContent(null);setError("");
    void verifyDocument(file).then(async()=>{
      const text=["html","markdown","text"].includes(kind)?new TextDecoder("utf-8",{fatal:true}).decode(await file.blob.arrayBuffer()):"";
      if(!live)return;
      if(kind==="image"||kind==="pdf")url=URL.createObjectURL(new Blob([await file.blob.arrayBuffer()],{type:previewMediaType(file.file_name,file.blob.type)}));
      if(!live){if(url)URL.revokeObjectURL(url);return;}
      setContent({digest:file.sha256,text,url});
    }).catch(error=>{if(live)setError(error instanceof Error?error.message:String(error));});
    return()=>{live=false;if(url)URL.revokeObjectURL(url);};
  },[file,kind]);
  if(error&&error!=="image_decode")return <p role="alert" className="error">{tr("Die Dokumentvorschau konnte nicht geprüft oder gelesen werden.","The document preview could not be verified or read.")} <code>{error}</code></p>;
  if(!content||content.digest!==file.sha256)return <p role="status">{tr("Dokument wird geprüft…","Verifying document…")}</p>;
  if(source&&["html","markdown","text"].includes(kind))return <pre className="document-source">{content.text}</pre>;
  if(kind==="markdown")return <div className="document-rendered"><ChatMessageBody content={content.text}/></div>;
  if(kind==="html")return <iframe className="document-frame" title={file.file_name} sandbox="" referrerPolicy="no-referrer" srcDoc={isolatedHtml(content.text)}/>;
  if(kind==="image")return <>{error==="image_decode"?<p role="status">{tr("Dieser Browser kann das Bildformat nicht anzeigen. Nutze „Vorschau rendern“ für eine PNG-Vorschau oder lade das Original herunter.","This browser cannot display this image format. Use Render preview for a PNG preview, or download the original.")}</p>:<img className="document-image" alt={file.file_name} src={content.url} onError={()=>setError("image_decode")}/>}</>;
  if(kind==="pdf")return <><iframe className="document-frame document-pdf" title={file.file_name} sandbox="" src={content.url} referrerPolicy="no-referrer"/><p className="muted">{tr("Wenn die PDF-Anzeige in diesem Browser nicht verfügbar ist, lade die geprüfte Datei herunter.","If this browser does not support the PDF viewer, download the verified file.")}</p></>;
  if(kind==="text")return <pre className="document-source">{content.text}</pre>;
  return <p className="muted">{kind==="office"?tr("Lade die Originaldatei herunter oder erstelle im Dokument-Workbench eine PDF-Vorschau mit der eingerichteten Office-Konvertierung.","Download the original file or use the configured Office converter to create a PDF preview in the document workbench."):tr("Die Originaldatei steht zum Herunterladen bereit.","The original file is available for download.")}</p>;
}
