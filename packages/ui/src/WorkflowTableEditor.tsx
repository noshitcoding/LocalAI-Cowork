import type {WorkflowTableColumn} from "@cowork/client";
import type {TableOperation} from "./workflowTables";
import {useView} from "./view";

export function WorkflowTableEditor({value,onChange}:{value:TableOperation;onChange:(value:TableOperation)=>void}){
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const kinds:Record<WorkflowTableColumn["kind"],string>={text:tr("Text","Text"),integer:tr("Ganze Zahl","Integer"),number:tr("Zahl","Number"),boolean:tr("Wahr/Falsch","True/false"),date:tr("Datum (JJJJ-MM-TT)","Date (YYYY-MM-DD)"),money_minor:tr("Geldbetrag in Cent","Amount in minor units")};
  const changeColumn=(index:number,changes:Partial<WorkflowTableColumn>)=>{
    const previous=value.columns[index],next={...previous,...changes};
    onChange({...value,columns:value.columns.map((column,i)=>i===index?next:column),
      key_fields:value.key_fields.map(key=>key===previous.field?next.field:key).filter(key=>!next.nullable||key!==next.field)});
  };
  return <fieldset><legend>{tr("Zeilen an Excel-Tabelle anhängen","Append rows to an Excel table")}</legend>
    <p>{tr("Wähle die benannte Tabelle und ihre Spalten in Excel-Reihenfolge. Quelle, Inhaltsfingerprint, neue Ausgabedatei und Zeilenliste werden unten verbunden. Bestehende Dateien werden erhalten. Schlüssel verhindern doppelte Zeilen beim Einlesen des zuletzt übernommenen Tabellenstands.","Select the named table and its columns in Excel order. Bind the source, content fingerprint, new output file and row list below. Existing files are preserved. Keys prevent duplicate rows when reading the latest adopted table version.")}</p>
    <label>{tr("Excel-Blatt","Excel sheet")}<input required maxLength={31} value={value.sheet} onChange={e=>onChange({...value,sheet:e.target.value})}/></label>
    <label>{tr("Name der Excel-Tabelle","Excel table name")}<input required maxLength={255} value={value.table} onChange={e=>onChange({...value,table:e.target.value})}/></label>
    <p>{tr("Beträge werden als ganze Centwerte gebunden: 11900 ergibt 119,00. Datumswerte benötigen JJJJ-MM-TT. Leere Werte sind nur mit ausdrücklich erlaubtem null möglich. Änderungen an Feldnamen oder Typen setzen unvereinbare Zeilenbindungen zurück; prüfe diese danach erneut.","Bind amounts as whole minor units: 11900 becomes 119.00. Dates require YYYY-MM-DD. Missing values require explicitly allowed null. Changing field names or types resets incompatible row bindings; review them afterwards.")}</p>
    {value.columns.map((column,index)=><fieldset key={index}><legend>{tr("Spalte","Column")} {index+1}</legend>
      <label>{tr("Datenfeld","Data field")}<input required maxLength={64} value={column.field} onChange={e=>changeColumn(index,{field:e.target.value})}/></label>
      <label>{tr("Exakte Excel-Überschrift","Exact Excel header")}<input required maxLength={255} value={column.header} onChange={e=>changeColumn(index,{header:e.target.value})}/></label>
      <label>{tr("Werttyp","Value type")}<select value={column.kind} onChange={e=>changeColumn(index,{kind:e.target.value as WorkflowTableColumn["kind"]})}>{Object.entries(kinds).map(([kind,label])=><option key={kind} value={kind}>{label}</option>)}</select></label>
      <label><input type="checkbox" checked={column.nullable} onChange={e=>changeColumn(index,{nullable:e.target.checked})}/>{tr("null als fehlenden Wert erlauben","Allow null for missing values")}</label>
      <label><input type="checkbox" checked={value.key_fields.includes(column.field)} disabled={column.nullable||(!value.key_fields.includes(column.field)&&value.key_fields.length>=4)} onChange={e=>onChange({...value,key_fields:e.target.checked?[...value.key_fields,column.field]:value.key_fields.filter(key=>key!==column.field)})}/>{tr("Teil des Dublettenschlüssels","Part of duplicate key")}</label>
      <button type="button" disabled={value.columns.length<=1} onClick={()=>onChange({...value,columns:value.columns.filter((_,i)=>i!==index),key_fields:value.key_fields.filter(key=>key!==column.field)})}>{tr("Spalte entfernen","Remove column")}</button>
    </fieldset>)}
    <button type="button" disabled={value.columns.length>=32} onClick={()=>{
      let index=value.columns.length+1;while(value.columns.some(c=>c.field===`column_${index}`))index++;
      onChange({...value,columns:[...value.columns,{field:`column_${index}`,header:`Column ${index}`,kind:"text",nullable:false}]});
    }}>{tr("Spalte ergänzen","Add column")}</button>
    <p>{tr("Höchstens 100 Zeilen pro Aktion und ein bis vier Schlüsselfelder. Prüfe die Ergebnisdatei und übernimm sie ausdrücklich als nächsten Tabellenstand, bevor ein weiterer Eingang darauf aufbaut.","At most 100 rows per action and one to four key fields. Review the result and explicitly adopt it as the next table version before another input builds on it.")}</p>
  </fieldset>;
}
