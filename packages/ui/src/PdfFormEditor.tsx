import {useState} from "react";
import type {ToolCall} from "@cowork/client";
import {useView} from "./view";

export type PdfField={name:string;type:string;value:string;states?:string[];options?:string[]};
export function pdfFormFields(value:unknown):PdfField[]{
  if(!Array.isArray(value))return [];
  return value.filter((field):field is PdfField=>field!==null&&typeof field==="object"&&typeof field.name==="string"&&typeof field.type==="string"&&typeof field.value==="string").slice(0,200).map(field=>({...field,states:Array.isArray(field.states)?field.states.filter(v=>typeof v==="string"):[],options:Array.isArray(field.options)?field.options.filter(v=>typeof v==="string"):[]}));
}

export function PdfFormEditor({folder,path,sha256,fields,locked,prepare}:{folder:string;path:string;sha256:string;fields:PdfField[];locked:boolean;prepare:(tool:ToolCall)=>Promise<void>}){
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const [values,setValues]=useState<Record<string,string|boolean>>({}),[flatten,setFlatten]=useState(false);
  const [output,setOutput]=useState(path.replace(/\.pdf$/i,"-filled.pdf")),[error,setError]=useState("");
  const supported=fields.filter(f=>["/Tx","/Ch","/Btn"].includes(f.type));
  function fieldValue(field:PdfField):string|boolean{return Object.hasOwn(values,field.name)?values[field.name]:field.type==="/Btn"&&!field.states?.some(s=>!["/Off","/Yes"].includes(s))?field.value!=="/Off"&&field.value!=="":field.value;}
  return <section className="settings-card"><h3>{tr("PDF-Formular ausfüllen","Fill PDF form")}</h3><p><code>{path}</code> · <code>{sha256.slice(0,12)}</code></p>
    {!supported.length?<p>{tr("Keine bearbeitbaren Formularfelder gefunden.","No editable form fields found.")}</p>:<form onSubmit={event=>{event.preventDefault();setError("");void prepare({id:"ui",name:"document_write",arguments:{operation:"pdf",inputs:[{folder_id:folder,path,expected_sha256:sha256}],output:{path:output},spec:{action:"fill_form",fields:values,flatten},preview:true}}).catch(reason=>setError(String(reason)));}}>
      <fieldset disabled={locked}>{supported.map(field=><label key={field.name}>{field.name}{field.type==="/Btn"?field.states?.filter(s=>s!=="/Off").some(s=>s!=="/Yes")?<select value={String(fieldValue(field))} onChange={event=>setValues({...values,[field.name]:event.target.value})}>{[...new Set(["/Off",field.value,...(field.states??[])])].map(state=><option key={state}>{state}</option>)}</select>:<input type="checkbox" checked={Boolean(fieldValue(field))} onChange={event=>setValues({...values,[field.name]:event.target.checked})}/>:field.options?.length?<select value={String(fieldValue(field))} onChange={event=>setValues({...values,[field.name]:event.target.value})}>{[...new Set([field.value,...field.options])].map(value=><option key={value}>{value}</option>)}</select>:<input value={String(fieldValue(field))} maxLength={10000} onChange={event=>setValues({...values,[field.name]:event.target.value})}/>}</label>)}
        <label><input type="checkbox" checked={flatten} onChange={event=>setFlatten(event.target.checked)}/>{tr("Werte zusätzlich auf der Seite darstellen","Draw values onto the page")}</label>
        <label>{tr("Neue PDF-Datei","New PDF file")}<input value={output} required pattern=".*\.[pP][dD][fF]" onChange={event=>setOutput(event.target.value)}/></label>
        <button disabled={!Object.keys(values).length} type="submit">{tr("Formularänderung prüfen","Review form changes")}</button>
      </fieldset>
    </form>}{fields.length!==supported.length&&<p>{tr("Signaturfelder und unbekannte Feldtypen sind hier nicht bearbeitbar.","Signature fields and unknown field types cannot be edited here.")}</p>}{error&&<p role="alert">{error}</p>}
  </section>;
}
