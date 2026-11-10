import {describe,it,expect} from "vitest";
import {officeEditorDraftKey,readOfficeEditorDraft,saveOfficeEditorDraft} from "./officeEditorDraft";
import type {OfficeModel} from "./officeDocuments";

const model:OfficeModel={format:"xlsx",input:"docs/financials.xlsx",source_sha256:"a".repeat(64),offset:0,total:1,next_offset:null,items:[{sheet:"Sheet",cell:"A1",value:1,type:"number"}]};
function memory(){const items=new Map<string,string>();return {getItem:(key:string)=>items.get(key)??null,setItem:(key:string,value:string)=>{items.set(key,value);},removeItem:(key:string)=>{items.delete(key);}};}
describe("version-bound Office editor drafts",()=>{
  it("survives a new editor instance including currently invalid numeric input",()=>{const storage=memory(),key=officeEditorDraftKey("https://user:secret@example.invalid/api","chat",model),fields={"0":{text:"not yet numeric",type:"number"}};saveOfficeEditorDraft(storage,key,model,fields);expect(readOfficeEditorDraft(storage,key,model)).toEqual(fields);expect(key).not.toContain("secret");expect(readOfficeEditorDraft(storage,officeEditorDraftKey("https://example.invalid/api","other-chat",model),model)).toEqual({});expect(readOfficeEditorDraft(storage,officeEditorDraftKey("https://example.invalid/api","chat",{...model,source_sha256:"b".repeat(64)}),model)).toEqual({});});
  it("rejects a copied draft for changed bytes or an invalid cell and permits explicit discard",()=>{const storage=memory(),key="draft";saveOfficeEditorDraft(storage,key,model,{"0":{text:"=2+2",type:"formula"}});expect(()=>readOfficeEditorDraft(storage,key,{...model,source_sha256:"b".repeat(64)})).toThrow();expect(()=>saveOfficeEditorDraft(storage,key,model,{"1":{text:"outside inspected cells",type:"text"}})).toThrow();expect(()=>saveOfficeEditorDraft(storage,key,model,{"0":{text:"shell",type:"command"}})).toThrow();saveOfficeEditorDraft(storage,key,model,{});expect(readOfficeEditorDraft(storage,key,model)).toEqual({});});
});
