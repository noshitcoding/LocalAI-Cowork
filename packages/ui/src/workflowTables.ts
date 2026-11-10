import type {WorkflowDocumentOperation} from "@cowork/client";
import {dataFieldName,validDataValue} from "./workflowDataflow";
import {documentInputTypes} from "./workflowDocumentTypes";
export type TableOperation=Extract<WorkflowDocumentOperation,{kind:"append_table_rows"}>;

export function freshTableOperation():TableOperation {
  return {kind:"append_table_rows",path_input:"path",sha256_input:"sha",output_path_input:"output",rows_input:"rows",
    sheet:"Invoices",table:"InvoiceTable",columns:[
      {field:"invoice",header:"Invoice",kind:"text",nullable:false},
      {field:"gross_minor",header:"Gross",kind:"money_minor",nullable:false},
      {field:"issued",header:"Issued",kind:"date",nullable:false},
    ],key_fields:["invoice"]};
}
export function validTableSchema(operation:TableOperation):boolean {
  const {columns,key_fields:keys,sheet,table}=operation;
  return !!sheet.trim()&&sheet.length<=31&&!sheet.includes("\0")&&!!table.trim()&&table.length<=255&&!table.includes("\0")
    &&columns.length>0&&columns.length<=32&&new Set(columns.map(c=>c.field)).size===columns.length
    &&new Set(columns.map(c=>c.header.toLowerCase())).size===columns.length
    &&columns.every(c=>dataFieldName(c.field)&&!!c.header.trim()&&c.header.length<=255&&!c.header.includes("\0")
      &&["text","number","integer","boolean","date","money_minor"].includes(c.kind)&&typeof c.nullable==="boolean")
    &&keys.length>0&&keys.length<=4&&new Set(keys).size===keys.length&&keys.every(key=>columns.some(c=>c.field===key&&!c.nullable));
}
export function validTableRows(operation:TableOperation,value:unknown):boolean {
  if(!validTableSchema(operation)||!validDataValue(documentInputTypes(operation)[operation.rows_input],value)||!Array.isArray(value)||!value.length)return false;
  const seen=new Set<string>();
  return value.every(row=>{
    for(const column of operation.columns){
      const item=row[column.field];
      if(item===null&&column.nullable)continue;
      if(column.kind==="text"&&(typeof item!=="string"||item.length===0))return false;
      if(column.kind==="date"){
        if(typeof item!=="string"||!/^\d{4}-\d{2}-\d{2}$/.test(item))return false;
        const date=new Date(item+"T00:00:00Z");
        if(!Number.isFinite(date.valueOf())||date.toISOString().slice(0,10)!==item||Number(item.slice(0,4))<1900)return false;
      }
      if((column.kind==="money_minor"||column.kind==="integer")&&(!Number.isSafeInteger(item)||Math.abs(item)>(column.kind==="money_minor"?9_999_999_999_999:999_999_999_999_999)))return false;
    }
    const key=operation.key_fields.map(field=>row[field]);
    if(key.some(item=>typeof item==="string"&&!item.trim()))return false;
    const identity=JSON.stringify(key);if(seen.has(identity))return false;seen.add(identity);return true;
  });
}
