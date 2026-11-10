import {useEffect,useId,useRef,useState} from "react";
import type {WorkflowDataType,WorkflowReviewField,WorkflowReviewForm,WorkflowReviewRequest} from "@cowork/client";
import {useView} from "./view";
import {WorkflowDataTypeEditor,WorkflowDataValueEditor,WorkflowDataValuePreview} from "./WorkflowDataflowEditor";
import {defaultDataValue} from "./workflowDataflow";
import {formatMoneyMinor,parseMoneyMinor,reviewErrors,reviewPrefillType,validReviewForm} from "./workflowReviews";

const base=(kind:WorkflowDataType)=>kind.type==="nullable"?kind.value:kind;
function fieldFor(name:string,kind:WorkflowDataType,previous?:WorkflowReviewField):WorkflowReviewField {
  const type=base(kind).type;
  return {label:previous?.label??name,help:previous?.help??"",value_type:kind,
    format:type==="text"&&previous?.format==="date"?"date":type==="integer"&&previous?.format==="money_minor"?"money_minor":"value",
    required:previous?.required??true,minimum:type==="integer"?previous?.minimum??null:null,maximum:type==="integer"?previous?.maximum??null:null};
}
export function WorkflowReviewFormEditor({form,onChange,sources=[]}:{form:WorkflowReviewForm;onChange:(value:WorkflowReviewForm)=>void;sources?:{id:string;name:string}[]}){
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const patch=(name:string,changes:Partial<WorkflowReviewField>)=>onChange({...form,fields:{...form.fields,[name]:{...form.fields[name],...changes}}});
  const integers=Object.entries(form.fields).filter(([,field])=>(field.required?base(field.value_type):field.value_type).type==="integer").map(([name])=>name);
  return <section className="product-stack">
    <label>{tr("Frage an die prüfende Person","Question for the reviewer")}<textarea required rows={3} maxLength={4000} value={form.question} onChange={e=>onChange({...form,question:e.target.value})}/></label>
    <fieldset><legend>{tr("Quellen zum Vergleichen","Sources to compare")}</legend>
      <p className="muted">{tr("Wähle bis zu vier vorherige Text- oder OCR-Schritte. Die gespeicherten Originalergebnisse erscheinen schreibgeschützt neben den Angaben.","Choose up to four preceding text or OCR steps. Their saved original results appear read-only beside the values.")}</p>
      {!sources.length&&!(form.source_steps??[]).length&&<p>{tr("Verbinde zuerst einen Text- oder OCR-Schritt als Vorgänger.","Connect a text or OCR step as a predecessor first.")}</p>}
      {[...sources,...(form.source_steps??[]).filter(id=>!sources.some(source=>source.id===id)).map(id=>({id,name:tr("Quelle nicht mehr verbunden: ","Source no longer connected: ")+id}))].map(source=><label className="product-check" key={source.id}><input type="checkbox" checked={(form.source_steps??[]).includes(source.id)} disabled={!(form.source_steps??[]).includes(source.id)&&(form.source_steps??[]).length>=4} onChange={e=>onChange({...form,source_steps:e.target.checked?[...(form.source_steps??[]),source.id]:(form.source_steps??[]).filter(id=>id!==source.id)})}/>{source.name}</label>)}
      {(form.source_steps??[]).some(id=>!sources.some(source=>source.id===id))&&<p role="alert">{tr("Diese Quelle ist kein vorheriger Text- oder OCR-Schritt mehr. Verbinde sie erneut oder entferne die Auswahl.","This source is no longer a preceding text or OCR step. Reconnect it or remove the selection.")}</p>}
    </fieldset>
    <WorkflowDataTypeEditor label={tr("Felder des Prüfformulars","Review form fields")} objectOnly value={reviewPrefillType(form)} onChange={kind=>{if(kind.type==="object")onChange({...form,fields:Object.fromEntries(Object.entries(kind.fields).map(([name,type])=>[name,fieldFor(name,type,form.fields[name])]))});}}/>
    {Object.entries(form.fields).map(([name,field])=><fieldset className="product-item" key={name}><legend>{name}</legend>
      <label>{tr("Beschriftung","Label")}<input required maxLength={200} value={field.label} onChange={e=>patch(name,{label:e.target.value})}/></label>
      <label>{tr("Hinweis zur Prüfung","Review hint")}<textarea rows={2} maxLength={800} value={field.help} onChange={e=>patch(name,{help:e.target.value})}/></label>
      <label>{tr("Darstellung","Display")}<select value={field.format} onChange={e=>patch(name,{format:e.target.value as WorkflowReviewField["format"]})}><option value="value">{tr("Passend zum Datentyp","Match data type")}</option>{base(field.value_type).type==="text"&&<option value="date">{tr("Kalenderdatum","Calendar date")}</option>}{base(field.value_type).type==="integer"&&<option value="money_minor">{tr("Geldbetrag mit zwei Nachkommastellen","Money with two decimal places")}</option>}</select></label>
      <label className="product-check"><input type="checkbox" checked={field.required} onChange={e=>patch(name,{required:e.target.checked})}/>{tr("Angabe erforderlich","Value required")}</label>
      {base(field.value_type).type==="integer"&&<><label>{tr("Kleinster Wert; Geldbeträge in Cent","Minimum; money in cents")}<input type="number" step={1} min={Number.MIN_SAFE_INTEGER} max={Number.MAX_SAFE_INTEGER} value={field.minimum??""} onChange={e=>patch(name,{minimum:e.target.value===""?null:e.target.valueAsNumber})}/></label><label>{tr("Größter Wert; Geldbeträge in Cent","Maximum; money in cents")}<input type="number" step={1} min={Number.MIN_SAFE_INTEGER} max={Number.MAX_SAFE_INTEGER} value={field.maximum??""} onChange={e=>patch(name,{maximum:e.target.value===""?null:e.target.valueAsNumber})}/></label></>}
    </fieldset>)}
    <h4>{tr("Summen prüfen","Check totals")}</h4>
    {form.checks.map((check,index)=><fieldset className="product-item" key={index}><legend>{tr("Summenregel","Total rule")} {index+1}</legend>
      <label>{tr("Summenfeld","Total field")}<select value={check.total} onChange={e=>onChange({...form,checks:form.checks.map((rule,i)=>i===index?{...rule,total:e.target.value}:rule)})}>{!integers.includes(check.total)&&<option value={check.total}>{tr("Feld erneut auswählen","Select the field again")}</option>}{integers.map(name=><option key={name} value={name}>{form.fields[name].label}</option>)}</select></label>
      <p>{tr("Diese Felder werden addiert:","Add these fields:")}</p>{integers.map(name=><label className="product-check" key={name}><input type="checkbox" checked={check.terms.includes(name)} disabled={!check.terms.includes(name)&&(name===check.total||check.terms.length>=16)} onChange={e=>onChange({...form,checks:form.checks.map((rule,i)=>i===index?{...rule,terms:e.target.checked?[...rule.terms,name]:rule.terms.filter(term=>term!==name)}:rule)})}/>{form.fields[name].label}</label>)}
      <button type="button" onClick={()=>onChange({...form,checks:form.checks.filter((_,i)=>i!==index)})}>{tr("Summenregel entfernen","Remove total rule")}</button>
    </fieldset>)}
    <button type="button" disabled={integers.length<3||form.checks.length>=16} onClick={()=>onChange({...form,checks:[...form.checks,{kind:"sum",terms:integers.slice(0,2),total:integers[2]}]})}>{tr("Summenregel ergänzen","Add total rule")}</button>
    {!validReviewForm(form)&&<p role="alert">{tr("Prüfe die Feldtypen, Grenzen und Summenregeln. Summen benötigen mindestens zwei verschiedene Summanden und ein anderes Summenfeld.","Check field types, limits and total rules. A total needs at least two distinct terms and a different total field.")}</p>}
  </section>;
}

function MoneyInput({label,value,required,onChange}:{label:string;value:unknown;required:boolean;onChange:(value:unknown)=>void}){
  const [text,setText]=useState(()=>formatMoneyMinor(value));const emitted=useRef(value);
  useEffect(()=>{if(value!==emitted.current){setText(formatMoneyMinor(value));emitted.current=value;}},[value]);
  return <label>{label}<input type="text" inputMode="decimal" required={required} maxLength={128} value={text} onChange={e=>{const text=e.target.value;setText(text);const next=parseMoneyMinor(text);emitted.current=next;onChange(next);}}/></label>;
}
export function WorkflowReviewAnswerEditor({review,answer,disabled,onChange}:{review:WorkflowReviewRequest;answer:string;disabled:boolean;onChange:(answer:string)=>void}){
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en,id=useId();
  let values:Record<string,unknown>|null=null;try{const parsed=JSON.parse(answer);if(parsed&&typeof parsed==="object"&&!Array.isArray(parsed))values=parsed;}catch{}
  if(!values)return <section role="alert"><p>{tr("Der gespeicherte Entwurf lässt sich nicht als Formular öffnen. Er bleibt im Verlauf erhalten.","The saved draft cannot be opened as a form. It remains in the history.")}</p><pre>{answer}</pre><button type="button" disabled={disabled} onClick={()=>onChange(JSON.stringify(review.values))}>{tr("Mit den ursprünglichen Vorschlägen weiterarbeiten","Continue with the original suggestions")}</button></section>;
  const current=values,errors=reviewErrors(review.form,current);
  const patch=(name:string,value:unknown)=>onChange(JSON.stringify({...current,[name]:value}));
  const message=(code:string)=>code==="required"?tr("Bitte ergänze diese Angabe.","Complete this field."):code==="date"?tr("Bitte wähle ein gültiges Kalenderdatum.","Choose a valid calendar date."):code==="range"?tr("Der Wert liegt außerhalb der erlaubten Grenzen.","The value is outside the allowed range."):code==="sum"?tr("Die Summe stimmt nicht mit den Einzelbeträgen überein.","The total does not match the component amounts."):tr("Prüfe den Datentyp dieser Angabe.","Check the type of this value.");
  return <fieldset disabled={disabled} className="workflow-review-form"><legend>{tr("Angaben prüfen und korrigieren","Review and correct values")}</legend>
    {Object.entries(review.form.fields).map(([name,field],index)=>{
      const value=current[name],special=field.format!=="value",nullable=field.value_type.type==="nullable";
      return <div className="product-item" key={name} aria-describedby={`${id}-${index}-help`}>
        {nullable&&special&&!field.required&&<label className="product-check"><input type="checkbox" checked={value===null} onChange={e=>patch(name,e.target.checked?null:defaultDataValue(base(field.value_type)))}/>{field.label}: {tr("Kein Wert","No value")}</label>}
        {(field.required||!special||value!==null)&& (field.format==="money_minor"?<MoneyInput label={field.label} value={value} required={field.required} onChange={next=>patch(name,next)}/>:field.format==="date"?<label>{field.label}<input type="date" min="0001-01-01" max="9999-12-31" required={field.required} value={typeof value==="string"?value:""} onChange={e=>patch(name,e.target.value)}/></label>:<WorkflowDataValueEditor label={field.label} kind={field.required?base(field.value_type):field.value_type} value={value} onChange={next=>patch(name,next)}/>)}
        <p id={`${id}-${index}-help`} className="muted">{field.help}{field.required?` ${tr("Erforderlich.","Required.")}`:""}</p>
        {errors.filter(error=>error.field===name).map((error,i)=><p role="alert" key={i}>{field.label}: {message(error.code)}</p>)}
      </div>;
    })}
    {errors.filter(error=>error.field===null).map((error,i)=><p role="alert" key={i}>{message(error.code)}</p>)}
  </fieldset>;
}
export function WorkflowReviewAnswerPreview({form,answer}:{form:WorkflowReviewForm;answer:string}){
  let values:Record<string,unknown>|null=null;try{const parsed=JSON.parse(answer);if(parsed&&typeof parsed==="object"&&!Array.isArray(parsed))values=parsed;}catch{}
  return values?<dl>{Object.entries(form.fields).map(([name,field])=><div key={name}><dt>{field.label}</dt><dd>{field.format==="money_minor"?formatMoneyMinor(values![name]):<WorkflowDataValuePreview value={values![name]}/>}</dd></div>)}</dl>:<pre>{answer}</pre>;
}
