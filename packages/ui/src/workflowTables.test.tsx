import {describe,expect,it} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {decode,defaultRunBudgets} from "@cowork/client";
import {documentInputTypes,documentOutputType} from "./workflowDocumentTypes";
import {nativeDocumentWorkflow,setDocumentOperation,validWorkflowDocuments} from "./workflowDocuments";
import {freshTableOperation,validTableRows,validTableSchema} from "./workflowTables";
import {validWorkflowEditor} from "./workflowUi";
import {WorkflowDocumentsEditor} from "./WorkflowDocumentsEditor";

const rows=()=>[{invoice:"INV-002",gross_minor:11900,issued:"2026-11-01"}];
function definition(){
  const value=setDocumentOperation(nativeDocumentWorkflow("Invoices","Book invoice","",defaultRunBudgets),"create-document",freshTableOperation());
  const inputs=value.dataflow!.steps["create-document"].inputs;
  for(const [name,item] of Object.entries({path:"invoices.xlsx",sha:"a".repeat(64),output:"updated.xlsx",rows:rows()}))inputs[name].value={source:"literal",value:item};
  return value;
}
describe("typed Excel table workflow editing",()=>{
  it("publishes four typed bindings in the generated Rust contract without model roles",()=>{
    const value=definition();expect(validWorkflowDocuments(value)).toBe(true);expect(validWorkflowEditor(value)).toBe(true);
    expect(decode("WorkflowDefinition",value)).toEqual(value);expect(value.roles).toEqual([]);
    const inputs=value.dataflow!.steps["create-document"].inputs;
    expect(Object.keys(inputs)).toEqual(["path","sha","output","rows"]);
    expect(inputs.rows.value_type).toEqual(documentInputTypes(freshTableOperation()).rows);
    expect(inputs.rows.value).toEqual({source:"literal",value:rows()});
    expect(documentOutputType(freshTableOperation()).type).toBe("object");
  });
  it("retains row bindings when only Excel headers change and resets an incompatible field type",()=>{
    const before=definition(),operation=freshTableOperation();operation.columns[0].header="Invoice number";
    const next=setDocumentOperation(before,"create-document",operation);
    expect(next.dataflow!.steps["create-document"].inputs.rows).toEqual(before.dataflow!.steps["create-document"].inputs.rows);
    operation.columns[1].kind="text";
    const changed=setDocumentOperation(next,"create-document",operation);
    expect(changed.dataflow!.steps["create-document"].inputs.rows.value).toEqual({source:"literal",value:[]});
    expect(changed.dataflow!.steps["create-document"].inputs.sha).toEqual(before.dataflow!.steps["create-document"].inputs.sha);
    expect(validWorkflowDocuments(changed)).toBe(false);
  });
  it.each([{keys:[]},{keys:["invoice","invoice"]},{keys:["unknown"]}])("rejects missing, repeated or unknown duplicate fields: %j",({keys})=>{
    const operation=freshTableOperation();operation.key_fields=keys;expect(validTableSchema(operation)).toBe(false);
  });
  it.each(["2026-02-30","1900-02-29","1899-12-31","2026-+1-01"])("rejects an invalid invoice date: %s",issued=>{
    expect(validTableRows(freshTableOperation(),[{...rows()[0],issued}])).toBe(false);
  });
  it("rejects duplicate, mistyped, empty and oversized row batches",()=>{
    const operation=freshTableOperation();
    for(const data of [[],[...rows(),...rows()],JSON.stringify(rows()),[{...rows()[0],gross_minor:"11900"}],
      [{...rows()[0],gross_minor:119.5}],[{...rows()[0],invoice:" "}],Array.from({length:101},(_,i)=>({...rows()[0],invoice:String(i)}))]){
      expect(validTableRows(operation,data)).toBe(false);
    }
    expect(validTableRows(operation,[{...rows()[0],gross_minor:-1,issued:"2024-02-29"}])).toBe(true);
  });
  it("requires a distinct XLSX output and exact typed row configuration",()=>{
    for(const output of ["invoices.xlsx","updated.docx"]){
      const value=definition();value.dataflow!.steps["create-document"].inputs.output.value={source:"literal",value:output};
      expect(validWorkflowDocuments(value)).toBe(false);
    }
    const value=definition();value.dataflow!.steps["create-document"].inputs.rows.value_type={type:"text"};
    expect(validWorkflowDocuments(value)).toBe(false);
  });
  it("shows named-table mapping, integer cents, duplicate keys and explicit result adoption",()=>{
    const markup=renderToStaticMarkup(<WorkflowDocumentsEditor value={definition()} onChange={()=>{}}/>);
    for(const label of ["Excel-Tabelle ergänzen","Name der Excel-Tabelle","Exakte Excel-Überschrift","Geldbetrag in Cent","Dublettenschlüssels","11900","übernimm"]){
      expect(markup).toContain(label);
    }
  });
});
