import {useView} from "./view";

const record=(value:unknown):Record<string,unknown>=>value!==null&&typeof value==="object"&&!Array.isArray(value)?value as Record<string,unknown>:{};
export function DocumentDiagnostics({engine,openSource,canOpenSource}:{engine:unknown;openSource?:(file:string,line:number)=>void;canOpenSource?:(file:string)=>boolean}){
  const language=useView(value=>value.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const receipt=record(engine),calculation=record(record(receipt.qa).calculation??record(receipt.result).calculation);
  const failure=receipt.error&&(typeof receipt.message==="string"?receipt.message:record(receipt.error).message);
  const diagnostics=(Array.isArray(receipt.diagnostics)?receipt.diagnostics:[]).map(record).filter(item=>typeof item.message==="string").slice(0,200);
  const count=(key:string)=>typeof calculation[key]==="number"&&Number.isSafeInteger(calculation[key])&&Number(calculation[key])>=0?Number(calculation[key]):null;
  const descriptions:Record<string,[string,string]>={
    unverified:["Vorhandene Formelwerte sind ungeprüft. Änderungen und gespeicherte Caches belegen keine aktuelle Berechnung.","Existing formula values are unverified. Edits and stored caches do not establish a current calculation."],
    recalculated:["Formeln wurden mit LibreOffice neu berechnet; Ergebniswerte sind vorhanden.","Formulas were recalculated with LibreOffice; cached result values are present."],
    incomplete:["Für einige Formeln fehlen Ergebniswerte. Prüfe die Arbeitsmappe vor der Nutzung.","Some formulas have no cached result values. Review the workbook before use."],
    errors:["Die Arbeitsmappe enthält Formel-Fehlerwerte. Prüfe die Ergebnisse vor der Nutzung.","The workbook contains formula error values. Review the results before use."],
    no_formulas:["Die Arbeitsmappe enthält keine Formeln.","The workbook contains no formulas."],
  };
  const status=typeof calculation.status==="string"?calculation.status:"";
  if(!status&&!diagnostics.length&&typeof failure!=="string")return null;
  return <section className="document-diagnostics" aria-label={tr("Prüfhinweise","Validation notes")}>
    {typeof failure==="string"&&<p role="alert">{failure}</p>}
    {status&&<><p role={status==="errors"?"alert":"status"}>{Object.hasOwn(descriptions,status)?tr(...descriptions[status]):tr("Unbekannter Berechnungsstatus. Prüfe die Details.","Unknown calculation status. Review the details.")}</p>
      <p>{count("formula_cells")??"?"} {tr("Formelzellen","formula cells")} · {count("missing_values")??"?"} {tr("fehlende Werte","missing values")} · {count("error_values")??"?"} {tr("Fehlerwerte","error values")}</p></>}
    {!!diagnostics.length&&<ul>{diagnostics.map((item,index)=><li key={index} role={item.severity==="error"?"alert":undefined}>{typeof item.file==="string"&&(openSource&&canOpenSource?.(item.file)?<button type="button" onClick={()=>openSource(item.file as string,typeof item.line==="number"?item.line:1)}>{item.file}{typeof item.line==="number"?`:${item.line}`:""}</button>:<code>{item.file}{typeof item.line==="number"?`:${item.line}`:""}: </code>)}{String(item.message)}</li>)}</ul>}
  </section>;
}
