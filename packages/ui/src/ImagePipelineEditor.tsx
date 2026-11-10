import {useEffect,useRef,useState} from "react";
import type {ToolCall} from "@cowork/client";
import {useView} from "./view";
import {imageDraftTool,imageOperations,imageStep,initialImageFields,newImageDraft,type ImageDraft} from "./imagePipeline";

export function ImagePipelineEditor({source,value,onChange,locked,prepare}:{source:ImageDraft["source"];value:ImageDraft|null;onChange:(draft:ImageDraft)=>void;locked:boolean;prepare:(tool:ToolCall)=>Promise<void>}){
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const [op,setOp]=useState("resize"),[fields,setFields]=useState(initialImageFields("resize")),[error,setError]=useState("");
  function describeError(reason:unknown){
    const code=reason instanceof Error?reason.message:String(reason);
    const messages:Record<string,[string,string]>={
      crop_box_invalid:["Rechts und unten müssen hinter links und oben liegen.","Right and bottom must follow left and top."],
      image_size_required:["Gib mindestens eine Bildkante an.","Enter at least one image dimension."],
      image_padding_required:["Vergrößere mindestens eine Seite der Bildfläche.","Add space on at least one side."],
      image_arrow_length_required:["Start und Ende des Pfeils müssen sich unterscheiden.","The arrow start and end must differ."],
      image_output_invalid:["Wähle einen neuen Bilddateinamen mit unterstützter Endung.","Choose a new image filename with a supported extension."],
      image_overlay_missing:["Für diesen Schritt fehlt das nummerierte Begleitbild.","The numbered overlay image is missing for this step."],
      image_draft_invalid:["Der Bildentwurf enthält ungültige Werte oder überschreitet die Grenzen.","The image draft contains invalid values or exceeds its limits."],
    };
    return messages[code]?tr(...messages[code]):code;
  }
  const [past,setPast]=useState<ImageDraft[]>([]),[future,setFuture]=useState<ImageDraft[]>([]),last=useRef(value);
  useEffect(()=>{if(value!==last.current){setPast([]);setFuture([]);last.current=value;}},[value]);
  const draft=value??newImageDraft(source),matches=source.folder===draft.source.folder&&source.path===draft.source.path&&source.sha256===draft.source.sha256;
  function emit(next:ImageDraft){last.current=next;onChange(next);setError("");}
  function change(next:ImageDraft){if(locked||!matches)return;setPast([...past.slice(-49),draft]);setFuture([]);emit(next);}
  function undo(){if(locked||!past.length)return;const previous=past.at(-1)!;setFuture([draft,...future]);setPast(past.slice(0,-1));emit(previous);}
  function redo(){if(locked||!future.length)return;setPast([...past,draft]);setFuture(future.slice(1));emit(future[0]);}
  function move(index:number,by:number){const steps=[...draft.steps];[steps[index],steps[index+by]]=[steps[index+by],steps[index]];change({...draft,steps});}
  return <section className="settings-card image-pipeline"><h3>{tr("Bildbearbeitung","Image editing")}</h3>
    <p><code>{source.path}</code> · <code>{source.sha256.slice(0,12)}</code></p>
    {!matches?<><p role="status">{tr("Der Bildentwurf gehört zu einer anderen Quelldatei oder Version. Öffne die ursprüngliche Quelle oder beginne einen neuen Bildentwurf.","This image draft belongs to a different source file or version. Open the original source or start a new image draft.")}</p><button disabled={locked} onClick={()=>{setPast([]);setFuture([]);emit(newImageDraft(source));}}>{tr("Neuen Bildentwurf beginnen","Start new image draft")}</button></>:<>
      <p>{tr("Schritte wirken in der angegebenen Reihenfolge. Koordinaten beziehen sich jeweils auf das Ergebnis des vorherigen Schritts. Die gerenderte Ergebnisvorschau erscheint nach der Ausführung.","Steps run in the listed order. Coordinates apply to the result of the previous step. The rendered result preview appears after execution.")}</p>
      <fieldset disabled={locked}><legend>{tr("Bearbeitungsschritte","Editing steps")}</legend>
        <div className="actions"><button type="button" disabled={!past.length} onClick={undo}>{tr("Rückgängig","Undo")}</button><button type="button" disabled={!future.length} onClick={redo}>{tr("Wiederholen","Redo")}</button></div>
        <ol>{draft.steps.map((step,index)=><li key={index}><strong>{tr(imageOperations[step.op].de,imageOperations[step.op].en)}</strong><dl className="image-step-values">{imageOperations[step.op].fields.map(field=>{const [key,part]=field.key.split("."),val=part===undefined?step[key]:(step[key] as unknown[]|undefined)?.[Number(part)];return val===undefined?null:<div key={field.key}><dt>{tr(field.label,field.en)}</dt><dd>{typeof val==="boolean"?val?tr("Ja","Yes"):tr("Nein","No"):String(val)}</dd></div>;})}</dl><div className="actions">
          <button type="button" disabled={index===0} aria-label={tr("Schritt nach oben","Move step up")+` ${index+1}`} onClick={()=>move(index,-1)}>↑</button>
          <button type="button" disabled={index===draft.steps.length-1} aria-label={tr("Schritt nach unten","Move step down")+` ${index+1}`} onClick={()=>move(index,1)}>↓</button>
          <button type="button" onClick={()=>change({...draft,steps:draft.steps.filter((_,i)=>i!==index)})}>{tr("Entfernen","Remove")}</button></div></li>)}</ol>
        <form onSubmit={event=>{event.preventDefault();try{if(draft.steps.length>=50)throw new Error(tr("Maximal 50 Schritte","Maximum 50 steps"));change({...draft,steps:[...draft.steps,imageStep(op,fields)]});}catch(reason){setError(describeError(reason));}}}>
          <label>{tr("Schritt hinzufügen","Add step")}<select value={op} onChange={event=>{setOp(event.target.value);setFields(initialImageFields(event.target.value));setError("");}}>{Object.entries(imageOperations).map(([key,entry])=><option key={key} value={key}>{tr(entry.de,entry.en)}</option>)}</select></label>
          <div className="image-step-fields">{imageOperations[op].fields.map(field=><label key={field.key}>{tr(field.label,field.en)}{field.type==="choice"?<select value={String(fields[field.key])} onChange={event=>setFields({...fields,[field.key]:event.target.value})}>{field.choices.map(choice=><option key={choice}>{choice}</option>)}</select>:field.type==="flag"?<input type="checkbox" checked={Boolean(fields[field.key])} onChange={event=>setFields({...fields,[field.key]:event.target.checked})}/>:<input type={field.type==="number"?"number":"text"} value={String(fields[field.key])} required={!field.optional} min={field.type==="number"?field.min:undefined} max={field.type==="number"?field.max:undefined} step={field.type==="number"?(field.integer?1:"any"):undefined} maxLength={field.type==="text"?field.maxLength:undefined} onChange={event=>setFields({...fields,[field.key]:event.target.value})}/>}</label>)}</div>
          <button type="submit" disabled={draft.steps.length>=50}>{tr("Schritt übernehmen","Add to steps")}</button>
        </form>
        <label>{tr("Begleitbilder (ein relativer Pfad pro Zeile, Nummerierung ab 1)","Overlay images (one relative path per line, numbered from 1)")}<textarea value={draft.overlays.join("\n")} maxLength={60000} onChange={event=>change({...draft,overlays:event.target.value.split("\n").map(path=>path.trim())})}/></label>
        <label>{tr("Neue Bilddatei","New image file")}<input value={draft.output} maxLength={4096} onChange={event=>change({...draft,output:event.target.value})}/></label>
        <label>{tr("JPEG-/WebP-Qualität","JPEG/WebP quality")}<input type="number" value={draft.quality} min={1} max={100} onChange={event=>change({...draft,quality:Number(event.target.value)})}/></label>
        <label><input type="checkbox" checked={draft.keepExif} onChange={event=>change({...draft,keepExif:event.target.checked})}/>{tr("EXIF-Metadaten einschließlich möglicher GPS-Daten erhalten","Keep EXIF metadata including possible GPS data")}</label>
        <button disabled={!draft.steps.length} onClick={()=>{try{const tool=imageDraftTool({...draft,overlays:draft.overlays.filter(Boolean)});void prepare(tool).catch(reason=>setError(describeError(reason)));}catch(reason){setError(describeError(reason));}}}>{tr("Bildänderungen prüfen","Review image changes")}</button>
      </fieldset>
    </>}{error&&<p role="alert">{error}</p>}
  </section>;
}
