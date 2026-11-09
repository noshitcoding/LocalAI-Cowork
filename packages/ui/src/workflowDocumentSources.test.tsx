import {afterEach,describe,expect,it,vi} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {Client,type DownloadedFile,type FileEntry,type WorkflowDefinition} from "@cowork/client";
import reference from "../../../tests/fixtures/workflow-document-extraction.json";
import {DocumentSourceSelection,bindDocumentSource,documentSourceBindings,documentSourceByteLimit,relativeDocumentPath,selectableDocumentFile} from "./documentSourceSelection";
import {WorkflowDocumentSources} from "./WorkflowDocumentSources";

const workflow=()=>structuredClone(reference) as WorkflowDefinition;
const binding=()=>documentSourceBindings(workflow())[0];
const entry=(path="invoices/Rechnung 1.pdf"):FileEntry=>({name:path.split("/").at(-1)!,path,kind:"file",size:3});
const file=():DownloadedFile=>({file_name:"Rechnung 1.pdf",size:3,sha256:"a".repeat(64),blob:new Blob(["pdf"])});
afterEach(()=>vi.unstubAllGlobals());

describe("guided document source selection",()=>{
  it("discovers declared workspace source pairs and preserves the published definition",()=>{
    const value=workflow(),before=structuredClone(value);
    expect(documentSourceBindings(value)).toEqual([{step:"document-source",name:"Read original source",pathField:"source_path",hashField:"source_sha256"}]);
    expect(value).toEqual(before);
    value.documents!["document-source"].folder={kind:"selected_folder",folder_id:"foreign"};expect(documentSourceBindings(value)).toEqual([]);
    value.documents!["document-source"].folder={kind:"current_workspace"};value.rights.current_workspace=false;expect(documentSourceBindings(value)).toEqual([]);
  });
  it("rejects nested, untyped, constant and overlapping source bindings",()=>{
    for(const source of [{source:"input",pointer:"/nested/source_path"},{source:"literal",value:"fixed"},{source:"input",pointer:"/__proto__"}] as const){
      const value=workflow();value.dataflow!.steps["document-source"].inputs.path.value=structuredClone(source);expect(documentSourceBindings(value)).toEqual([]);
    }
    const value=workflow();value.dataflow!.steps["document-source"].inputs.sha.value={source:"input",pointer:"/source_path"};expect(documentSourceBindings(value)).toEqual([]);
    value.dataflow!.steps["document-source"].inputs.sha.value={source:"input",pointer:"/source_sha256"};value.dataflow!.steps["document-source"].inputs.path.value_type={type:"integer"};expect(documentSourceBindings(value)).toEqual([]);
  });
  it("allows an exact shared pair but hides pairs that share only one input",()=>{
    const value=workflow();value.documents!.second=structuredClone(value.documents!["document-source"]);value.dataflow!.steps.second=structuredClone(value.dataflow!.steps["document-source"]);
    expect(documentSourceBindings(value)).toHaveLength(2);
    if(value.dataflow!.input.type!=="object")throw new Error();value.dataflow!.input.fields.second_sha={type:"text"};value.dataflow!.steps.second.inputs.sha.value={source:"input",pointer:"/second_sha"};
    expect(documentSourceBindings(value)).toEqual([]);
  });
  it("atomically replaces both source fields and preserves unrelated values",()=>{
    const input={source_path:"old.pdf",source_sha256:"old",recipient:"draft",rows:[1,2]},before=structuredClone(input);
    expect(bindDocumentSource(input,binding(),entry().path,file())).toEqual({...input,source_path:entry().path,source_sha256:file().sha256});expect(input).toEqual(before);
  });
  it("blocks links, traversal, oversized files and mismatched download metadata",()=>{
    for(const path of ["/absolute.pdf","../up.pdf","a/../up.pdf","a\\b.pdf","C:/file.pdf","a//b","a/./b","bad\0.pdf"]){expect(relativeDocumentPath(path)).toBe(false);}
    for(const changed of [{kind:"symlink"},{size:documentSourceByteLimit+1},{size:-1},{size:1.5}])expect(selectableDocumentFile({...entry(),...changed})).toBe(false);
    for(const changed of [{file_name:"other.pdf"},{sha256:"bad"},{size:4},{size:documentSourceByteLimit+1}])expect(()=>bindDocumentSource({},binding(),entry().path,{...file(),...changed})).toThrow("document_source_invalid");
  });
  it("ignores a late completion after the active workflow, connection or plan changes",async()=>{
    let resolve!:(file:DownloadedFile)=>void;const apply=vi.fn(),session=new DocumentSourceSelection();let current=true;
    const result=session.select({entry:entry(),binding:binding(),download:()=>new Promise(done=>resolve=done),current:()=>({}),isCurrent:()=>current,apply});
    current=false;resolve(file());expect(await result).toBe(false);expect(apply).not.toHaveBeenCalled();
  });
  it("admits one download and discards its result after unmount",async()=>{
    let resolve!:(file:DownloadedFile)=>void;const apply=vi.fn(),download=vi.fn(()=>new Promise<DownloadedFile>(done=>resolve=done)),session=new DocumentSourceSelection();
    const request={entry:entry(),binding:binding(),download,current:()=>({}),isCurrent:()=>true,apply};const first=session.select(request);
    expect(await session.select(request)).toBe(false);expect(download).toHaveBeenCalledTimes(1);session.close();resolve(file());expect(await first).toBe(false);expect(apply).not.toHaveBeenCalled();
  });
  it("reads current unrelated inputs at completion instead of overwriting an old draft",async()=>{
    let resolve!:(file:DownloadedFile)=>void;const apply=vi.fn();let current={notes:"first"};const result=new DocumentSourceSelection().select({entry:entry(),binding:binding(),download:()=>new Promise(done=>resolve=done),current:()=>current,isCurrent:()=>true,apply});
    current={notes:"newer"};resolve(file());expect(await result).toBe(true);expect(apply).toHaveBeenCalledExactlyOnceWith({notes:"newer",source_path:entry().path,source_sha256:file().sha256});
  });
  it("uses the real authenticated read-only client and verifies content before binding",async()=>{
    const blob=new Blob(["pdf"]),hash=Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256",await blob.arrayBuffer()))).map(v=>v.toString(16).padStart(2,"0")).join("");
    const fetch=vi.fn(async()=>new Response(blob,{headers:{"content-type":"application/octet-stream","content-length":"3","x-cowork-file-name":encodeURIComponent("Rechnung 1.pdf"),"x-cowork-sha256":hash}}));vi.stubGlobal("fetch",fetch);
    const client=new Client("fixture-token","http://127.0.0.1:1"),apply=vi.fn();
    expect(await new DocumentSourceSelection().select({entry:entry(),binding:binding(),download:path=>client.downloadWorkspace("order",{path}),current:()=>({keep:"value"}),isCurrent:()=>true,apply})).toBe(true);
    expect(apply).toHaveBeenCalledExactlyOnceWith({keep:"value",source_path:entry().path,source_sha256:hash});expect(fetch).toHaveBeenCalledTimes(1);
    const [url,init]=fetch.mock.calls[0] as unknown as [string,RequestInit];expect(url).toContain("/orders/order/workspace/download?path=invoices%2FRechnung+1.pdf");expect(init.method).toBeUndefined();expect(init.headers).toEqual({Authorization:"Bearer fixture-token"});
  });
  it("retains both old inputs when the real client rejects a corrupt response",async()=>{
    vi.stubGlobal("fetch",vi.fn(async()=>new Response("bad",{headers:{"content-type":"application/octet-stream","content-length":"3","x-cowork-file-name":encodeURIComponent("Rechnung 1.pdf"),"x-cowork-sha256":"a".repeat(64)}})));
    const client=new Client("fixture-token","http://127.0.0.1:1"),apply=vi.fn();
    await expect(new DocumentSourceSelection().select({entry:entry(),binding:binding(),download:path=>client.downloadWorkspace("order",{path}),current:()=>({source_path:"old",source_sha256:"old"}),isCurrent:()=>true,apply})).rejects.toThrow("download_integrity");expect(apply).not.toHaveBeenCalled();
  });
  it("renders a closed, translated chooser without any request or automatic selection",()=>{
    const client=new Client("synthetic"),read=vi.spyOn(client,"workspace"),apply=vi.fn(),onChecking=vi.fn();
    for(const language of ["de","en"] as const){const html=renderToStaticMarkup(<WorkflowDocumentSources client={client} order="order" selection="version" definition={workflow()} value={{}} language={language} disabled={false} onChange={apply} onChecking={onChecking}/>);expect(html).toContain(language==="de"?"Dokument aus Arbeitsdateien wählen":"Choose a document from working files");expect(html).not.toMatch(/<details[^>]*open/);expect(html).not.toContain("synthetic");}
    expect(read).not.toHaveBeenCalled();expect(apply).not.toHaveBeenCalled();expect(onChecking).not.toHaveBeenCalled();
  });
});
