import type {WorkflowDataType,WorkflowDocumentOperation} from "@cowork/client";

const object=(fields:Record<string,WorkflowDataType>):WorkflowDataType=>({type:"object",fields});
const text=():WorkflowDataType=>({type:"text"});
const integer=():WorkflowDataType=>({type:"integer"});
const nullable=(value:WorkflowDataType):WorkflowDataType=>({type:"nullable",value});
const list=(item:WorkflowDataType,max_items:number):WorkflowDataType=>({type:"list",item,max_items});
function ocrFields(source:WorkflowDataType):Record<string,WorkflowDataType> {
  return {source,engine:object({name:text(),version:text(),binding_version:text(),languages:list(text(),2),models:list(object({language:text(),sha256:text()}),2)}),
    total_pages:integer(),pages:list(object({page:integer(),width:integer(),height:integer(),dpi:integer(),rotation:integer(),raster_sha256:text(),preview_sha256:nullable(text()),word_count:integer()}),16),
    pages_truncated:{type:"boolean"},next_page:nullable(integer()),total_words:integer(),offset:integer(),next_offset:nullable(integer()),
    words:list(object({page:integer(),index:integer(),line:integer(),text:text(),text_truncated:{type:"boolean"},x:integer(),y:integer(),width:integer(),height:integer(),score:{type:"number"}}),100),
    text:text(),truncated:{type:"boolean"}};
}
/** Product data only; the engine, original plan and file broker authorize effects. */
export function documentOutputType(operation:WorkflowDocumentOperation):WorkflowDataType {
  const source=object({folder_id:text(),path:text(),sha256:text()});
  const fields:Record<string,WorkflowDataType>=operation.kind==="ocr"?ocrFields(source):operation.kind==="read_text"
    ?{source,text:text(),offset:integer(),next_offset:nullable(integer()),truncated:{type:"boolean"}}
    :operation.kind==="preview"
    ?{source,total_pages:integer(),truncated:{type:"boolean"},images:{type:"list",max_items:16,item:object({sha256:text(),mime_type:text(),page:integer(),width:integer(),height:integer()})}}
    :{file:object({folder_id:text(),path:text(),sha256:text(),mime_type:text(),bytes:integer()})};
  if(operation.kind==="append_table_rows"){
    fields.source=source;
    fields.table=object({sheet:text(),table:text(),rows_added:integer(),first_row:integer(),last_row:integer(),previous_range:text(),range:text(),key_columns:list(integer(),4)});
  }
  return object({...fields,diagnostics:{type:"list",max_items:64,item:object({severity:text(),message:text(),file:nullable(text()),line:nullable(integer())})},diagnostics_truncated:{type:"boolean"}});
}

export function documentInputs(operation:WorkflowDocumentOperation):string[] {
  if(operation.kind==="append_table_rows")return [operation.path_input,operation.sha256_input,operation.output_path_input,operation.rows_input];
  return [operation.path_input,operation.kind==="create"?operation.content_input:operation.sha256_input];
}
export function documentInputTypes(operation:WorkflowDocumentOperation):Record<string,WorkflowDataType>{
  const fields:Record<string,WorkflowDataType>=Object.fromEntries(documentInputs(operation).map(name=>[name,text()]));
  if(operation.kind==="append_table_rows")fields[operation.rows_input]=list(object(Object.fromEntries(operation.columns.map(column=>{
    const value:WorkflowDataType=column.kind==="text"||column.kind==="date"?text():column.kind==="boolean"?{type:"boolean"}:column.kind==="number"?{type:"number"}:integer();
    return [column.field,column.nullable?nullable(value):value];
  }))),100);
  return fields;
}
export const documentTool=(operation:WorkflowDocumentOperation)=>operation.kind==="create"||operation.kind==="append_table_rows"?"document_write":"document_read";
