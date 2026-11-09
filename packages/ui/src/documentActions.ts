import type {Client,ToolCall,DocumentPreparation} from "@cowork/client";

export type PendingDocumentAction={key:string;phase:"prepare"|"commit"};
export const documentActionKey=(endpoint:string,order:string)=>"cowork-v2:document-action:2:"+encodeURIComponent(JSON.stringify([endpoint.replace(/\/+$/,""),order]));
export function readDocumentAction(storage:Pick<Storage,"getItem">,key:string):PendingDocumentAction|null{
  const raw=storage.getItem(key);if(raw===null)return null;
  const value=JSON.parse(raw) as PendingDocumentAction;
  if(!value||Object.keys(value).some(k=>k!=="key"&&k!=="phase")||typeof value.key!=="string"||!/^[a-zA-Z0-9-]{1,128}$/.test(value.key)||!["prepare","commit"].includes(value.phase))throw new Error("document_action_journal_invalid");
  return value;
}
export function retainDocumentAction(storage:Pick<Storage,"setItem"|"getItem">,key:string,value:PendingDocumentAction){
  const previous=readDocumentAction(storage,key);
  if(previous&&previous.key!==value.key)throw new Error("document_action_pending");
  if(previous?.phase==="commit"&&value.phase!=="commit")throw new Error("document_action_commit_pending");
  storage.setItem(key,JSON.stringify(value));
}
export function discardDocumentAction(storage:Pick<Storage,"getItem"|"removeItem">,key:string,expected:PendingDocumentAction){
  const current=readDocumentAction(storage,key);
  if(current&&(current.key!==expected.key||current.phase!==expected.phase))throw new Error("document_action_changed");
  storage.removeItem(key);
}
export async function recoverDocumentAction(client:Client,order:string,pending:PendingDocumentAction):Promise<DocumentPreparation>{
  return client.documentPreparationByKey(order,pending.key);
}
/** A CSV import never activates formulas. Formula editing requires an explicit cell type. */
export function csvRows(text:string):string[][]{
  const rows:string[][]=[],row:string[]=[];let cell="",quoted=false,closed=false;
  for(let i=0;i<text.length;i++){
    const c=text[i];
    if(c==='"'){if(quoted&&text[i+1]==='"'){cell+='"';i++;}else if(quoted){quoted=false;closed=true;}else if(cell===""&&!closed)quoted=true;else throw new Error("invalid_csv_quote");}
    else if(c===","&&!quoted){row.push(cell);cell="";closed=false;}
    else if((c==="\n"||c==="\r")&&!quoted){if(c==="\r"&&text[i+1]==="\n")i++;row.push(cell);rows.push(row.splice(0));cell="";closed=false;}
    else {if(closed&&!quoted)throw new Error("invalid_csv_quote");cell+=c;}
  }
  if(quoted)throw new Error("invalid_csv_quote");
  if(cell!==""||row.length||rows.length===0){row.push(cell);rows.push(row);}
  if(rows.length>10000||rows.some(r=>r.length>256))throw new Error("document_table_limit");
  return rows;
}
export function createDocumentTool(format:string,text:string,title:string,path:string):ToolCall{
  const spec:Record<string,unknown>={format,properties:{title}};
  if(format==="xlsx")spec.sheets=[{name:"Data",rows:csvRows(text),header:true}];
  else if(format==="pptx")spec.slides=text.split(/\n\s*\n/).filter(s=>s.trim()).map((s,i)=>{const [heading,...lines]=s.split("\n");return {title:heading||title||String(i+1),bullets:lines.filter(Boolean)};});
  else if(format==="docx")spec.markdown=text;
  else if(format==="pdf"){delete spec.format;delete spec.properties;spec.title=title;spec.markdown=text;}
  else throw new Error("document_format_unsupported");
  return {id:"ui",name:"document_write",arguments:{operation:format==="pdf"?"create_pdf":"create",output:{path},spec,preview:true}};
}
