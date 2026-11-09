import {useEffect,useRef,useState} from "react";
import type {Client,DownloadedFile,ToolCall} from "@cowork/client";
import {captureLatexProject,latexProject,latexProjectPaths,latexProjectTool,retryProjectRead,type LatexProject} from "./latexProject";
import {useView} from "./view";

export type LatexSource={path:string;text:string;line:number};
export function LatexProjectEditor({client,order,folder,value,onChange,seed,locked,ready,prepare,onOpen,source,onBusyChange}:{client:Client;order:string;folder?:string;value:LatexProject|null;onChange:(value:LatexProject)=>void;seed:string;locked:boolean;ready:boolean;prepare:(tool:ToolCall)=>Promise<void>;onOpen:(path:string,line?:number)=>Promise<void>;source:LatexSource|null;onBusyChange?:(busy:boolean)=>void}){
  const language=useView(state=>state.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const [paths,setPaths]=useState(value?.files.map(file=>file.path).join("\n")??seed),[output,setOutput]=useState(value?.output??"compiled.pdf"),[busy,setBusy]=useState(false),[error,setError]=useState("");
  const viewer=useRef<HTMLTextAreaElement>(null);
  useEffect(()=>{if(value){setPaths(value.files.map(file=>file.path).join("\n"));setOutput(value.output);}else setPaths(seed);},[value]);
  useEffect(()=>{if(!value)setPaths(seed);},[seed]);
  useEffect(()=>{if(source&&viewer.current){const lines=source.text.split("\n"),start=lines.slice(0,source.line-1).join("\n").length+(source.line>1?1:0);viewer.current.focus();viewer.current.setSelectionRange(start,start+(lines[source.line-1]?.length??0));viewer.current.scrollTop=Math.max(0,(source.line-3)*20);}},[source]);
  async function perform(fn:()=>Promise<void>,blockWorkspace=true){if(busy)return;setBusy(true);if(blockWorkspace)onBusyChange?.(true);setError("");try{await fn();}catch(reason){setError(reason instanceof Error?reason.message:String(reason));}finally{setBusy(false);if(blockWorkspace)onBusyChange?.(false);}}
  const mismatch=!!value&&value.folder!==folder;
  return <section className="settings-card latex-project"><h3>{tr("LaTeX-Projekt","LaTeX project")}</h3>
    <p>{tr("Erfasse bis zu 16 Dateien; die erste Datei ist zunächst die Hauptdatei. Begleitdateien liegen unter deren Ordner. Das Projekt speichert feste Dateiversionen; geänderte Quellen müssen neu erfasst werden.","Capture up to 16 files; the first file is initially the main file. Assets belong beneath its directory. The project retains fixed file versions; changed sources require a new capture.")}</p>
    {error&&<p role="alert">{error}</p>}{mismatch&&<p role="alert">{tr("Das Projekt gehört zu einem anderen Arbeitsordner. Erfasse die Dateien im aktuellen Arbeitsordner neu.","This project belongs to another workspace. Capture the files in the current workspace again.")}</p>}
    <fieldset disabled={locked||busy||!folder}>
      <label>{tr("Projektdateien (ein Pfad pro Zeile)","Project files (one path per line)")}<textarea rows={5} value={paths} onChange={event=>setPaths(event.target.value)}/></label>
      <label>{tr("Neue Projekt-PDF","New project PDF")}<input value={output} onChange={event=>setOutput(event.target.value)}/></label>
      <button type="button" onClick={()=>void perform(async()=>{const files=latexProjectPaths(paths),main=value&&files.includes(value.main)?value.main:files[0];onChange(await captureLatexProject(folder!,files,main,output,path=>client.workspaceStat(order,path)));})}>{tr("Projektdateien erfassen","Capture project files")}</button>
      {value&&<><label>{tr("Hauptdatei","Main file")}<select value={value.main} disabled={paths!==value.files.map(file=>file.path).join("\n")||output!==value.output} onChange={event=>{try{onChange(latexProject({...value,main:event.target.value})!);setError("");}catch{setError(tr("Die Hauptdatei muss im gemeinsamen Projektordner liegen.","The main file must be in the common project directory."));}}}>{value.files.filter(file=>/\.tex$/i.test(file.path)).map(file=><option key={file.path}>{file.path}</option>)}</select></label>
        <p role="status">{tr("Dateiversionen erfasst","File versions captured")} · {value.files.length}/16</p>
        <ul>{value.files.map(file=><li key={file.path}><button type="button" disabled={mismatch||!/\.(tex|bib|sty|cls|txt)$/i.test(file.path)} onClick={()=>void perform(()=>onOpen(file.path))}>{file.path}</button> · {file.size} B · <code>{file.sha256.slice(0,12)}</code></li>)}</ul>
        <button type="button" disabled={!ready||mismatch||paths!==value.files.map(file=>file.path).join("\n")||output!==value.output} onClick={()=>void perform(()=>prepare(latexProjectTool(value,folder!)),false)}>{tr("Projektkompilierung prüfen","Review project compilation")}</button></>}
    </fieldset>
    {!ready&&<p role="status">{tr("LaTeX-Kompilierung ist hier noch nicht bereit. Die Projektdateien können bereits erfasst werden.","LaTeX compilation is not ready here. Project files can already be captured.")}</p>}
    {source&&<div className="latex-source"><p><code>{source.path}:{source.line}</code></p><label>{tr("Projektquelltext","Project source")}<textarea ref={viewer} readOnly rows={15} value={source.text} spellCheck={false}/></label></div>}
  </section>;
}

/** Download against the captured hash, without silently accepting a newer source. */
export async function downloadLatexSource(project:LatexProject,path:string,download:(request:{path:string;expected_sha256:string})=>Promise<DownloadedFile>,line=1):Promise<LatexSource>{
  const file=project.files.find(file=>file.path===path);
  if(!file||! /\.(tex|bib|sty|cls|txt)$/i.test(path)||file.size>512*1024)throw new Error("latex_source_unavailable");
  const response=await retryProjectRead(()=>download({path,expected_sha256:file.sha256}));
  if(response.sha256!==file.sha256||response.blob.size>512*1024)throw new Error("latex_source_integrity");
  const text=new TextDecoder("utf-8",{fatal:true}).decode(await response.blob.arrayBuffer()),lines=text.split("\n").length;
  return {path,text,line:Number.isSafeInteger(line)&&line>0?Math.min(line,lines):1};
}
