import {describe,expect,it} from "vitest";
import {csvRows,createDocumentTool,documentActionKey,readDocumentAction,retainDocumentAction,discardDocumentAction} from "./documentActions";
describe("document action journal and builders",()=>{
  it("keeps endpoint/order isolation and never replaces an unconfirmed operation",()=>{
    const entries=new Map<string,string>(),storage={getItem:(k:string)=>entries.get(k)??null,setItem:(k:string,v:string)=>{entries.set(k,v);},removeItem:(k:string)=>{entries.delete(k);}};
    const key=documentActionKey("https://a/","chat");
    expect(key).toBe(documentActionKey("https://a","chat"));expect(key).not.toBe(documentActionKey("https://b","chat"));
    retainDocumentAction(storage,key,{key:"request-1",phase:"prepare"});
    expect(()=>retainDocumentAction(storage,key,{key:"request-2",phase:"prepare"})).toThrow("document_action_pending");
    retainDocumentAction(storage,key,{key:"request-1",phase:"commit"});
    expect(readDocumentAction(storage,key)?.phase).toBe("commit");
    expect(()=>retainDocumentAction(storage,key,{key:"request-1",phase:"prepare"})).toThrow("document_action_commit_pending");
    // Another window may have advanced the phase after this view was rendered.
    expect(()=>discardDocumentAction(storage,key,{key:"request-1",phase:"prepare"})).toThrow("document_action_changed");
    expect(()=>discardDocumentAction(storage,key,{key:"request-2",phase:"commit"})).toThrow("document_action_changed");
    expect(readDocumentAction(storage,key)?.phase).toBe("commit");
    discardDocumentAction(storage,key,{key:"request-1",phase:"commit"});
    expect(readDocumentAction(storage,key)).toBeNull();
    entries.set(key,'{"key":"request-1","phase":"commit","token":"secret"}');
    expect(()=>readDocumentAction(storage,key)).toThrow("document_action_journal_invalid");
  });
  it("preserves CSV content, embedded newlines and literal formulas",()=>{
    expect(csvRows('Name,Value\r\n"a,b","line\nnext"\r\n"a""b",=1+1\r\n')).toEqual([["Name","Value"],["a,b","line\nnext"],['a"b',"=1+1"]]);
    expect(()=>csvRows('"unfinished')).toThrow("invalid_csv_quote");
    const tool=createDocumentTool("xlsx","Name,Value\nX,=1+1","Table","table.xlsx");
    expect((tool.arguments as any).spec.sheets[0].rows[1][1]).toBe("=1+1");
  });
});
