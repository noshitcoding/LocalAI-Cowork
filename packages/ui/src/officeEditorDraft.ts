import type {OfficeModel} from "./officeDocuments";
import {toolboxEndpoint,type PendingStorage} from "./toolboxPending";

export type OfficeEditorFields=Record<string,{text:string,type:string}>;
export function officeEditorDraftKey(base:string,order:string,model:OfficeModel){
  return "cowork-v2:office-editor-draft:v1:"+encodeURIComponent(JSON.stringify([toolboxEndpoint(base),order,model.input,model.source_sha256,model.format,model.offset]));
}
function validate(value:unknown,model:OfficeModel):OfficeEditorFields{
  if(!value||typeof value!=="object"||Array.isArray(value))throw new Error("invalid_office_editor_draft");
  const fields=value as OfficeEditorFields;
  for(const [key,field] of Object.entries(fields)){
    const index=Number(key),item=model.items[index];
    if(!/^(0|[1-9][0-9]*)$/.test(key)||!Number.isInteger(index)||!item||item.truncated||!field||typeof field!=="object"||Object.keys(field).some(key=>key!=="text"&&key!=="type")||typeof field.text!=="string"||field.text.length>(model.format==="xlsx"?64000:128000)||typeof field.type!=="string"||!(model.format==="xlsx"?["text","number","boolean","formula","empty"]:["text"]).includes(field.type))throw new Error("invalid_office_editor_draft");
  }
  return fields;
}
export function readOfficeEditorDraft(storage:PendingStorage,key:string,model:OfficeModel):OfficeEditorFields{
  const raw=storage.getItem(key);if(raw===null)return {};
  if(raw.length>2_621_440)throw new Error("invalid_office_editor_draft");
  const value=JSON.parse(raw);if(value?.format!==1||value?.source_sha256!==model.source_sha256||value?.input!==model.input||value?.offset!==model.offset)throw new Error("invalid_office_editor_draft");
  return validate(value.fields,model);
}
export function saveOfficeEditorDraft(storage:PendingStorage,key:string,model:OfficeModel,fields:OfficeEditorFields){
  validate(fields,model);
  if(!Object.keys(fields).length){storage.removeItem(key);return;}
  const value=JSON.stringify({format:1,source_sha256:model.source_sha256,input:model.input,offset:model.offset,fields});
  if(value.length>2_621_440)throw new Error("office_editor_draft_too_large");
  storage.setItem(key,value);
}
