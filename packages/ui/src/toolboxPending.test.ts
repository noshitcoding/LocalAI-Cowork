import {afterEach,describe,expect,it,vi} from "vitest";
import {Client} from "@cowork/client";
import {clearPendingToolbox,readPendingToolbox,savePendingToolbox,submitPendingToolbox,toolboxPendingKey} from "./toolboxPending";
import {newToolboxRequest} from "./toolboxWorkbench";
import type {ToolPreview} from "./productApi";
const preview:ToolPreview={request_sha256:"a".repeat(64),mode:"owner_native",cwd:"managed/chat",timeout_seconds:120,output_bytes:1024,approval_required:true};
function storage(){const values=new Map<string,string>();return {values,getItem:(key:string)=>values.get(key)??null,setItem:(key:string,value:string)=>{values.set(key,value);},removeItem:(key:string)=>{values.delete(key);}};}
afterEach(()=>vi.unstubAllGlobals());
describe("durable pending tool operations",()=>{
  it("restores the exact original request and preview, isolates scopes, and refuses replacement",()=>{
    const disk=storage(),request=newToolboxRequest({kind:"github",arguments:["api","repos/owner/repository/issues","--method","POST","--raw-field","title=Original"]});
    savePendingToolbox(disk,"https://server.example","chat-a","github",request,preview);
    const loaded=readPendingToolbox(disk,"https://server.example","chat-a","github")!;
    expect(loaded.request).toEqual(request);expect(loaded.preview).toEqual(preview);
    expect(readPendingToolbox(disk,"https://other.example","chat-a","github")).toBeNull();
    expect(readPendingToolbox(disk,"https://server.example","chat-b","github")).toBeNull();
    expect(readPendingToolbox(disk,"https://server.example","chat-a","browser")).toBeNull();
    expect(()=>savePendingToolbox(disk,"https://server.example","chat-a","github",newToolboxRequest(request.action),preview)).toThrow("pending_toolbox_action_exists");
    expect(toolboxPendingKey("https://user:password@server.example?token=private","chat-a","github")).not.toMatch(/password|private|token/);
    const key=toolboxPendingKey("https://server.example","chat-a","github");
    disk.setItem(key,JSON.stringify({...loaded,request:newToolboxRequest(request.action)}));
    expect(()=>clearPendingToolbox(disk,loaded)).toThrow("pending_toolbox_action_exists");
    disk.setItem(key,JSON.stringify(loaded));
    clearPendingToolbox(disk,loaded);expect(readPendingToolbox(disk,"https://server.example","chat-a","github")).toBeNull();
  });
  it("reconciles a physically admitted action after a lost response and reload without a second POST",async()=>{
    const disk=storage(),client=new Client("private-owner-token","https://server.example"),request=newToolboxRequest({kind:"github",arguments:["api","repos/owner/repository/issues","--method","POST"]});
    let posts=0;const admitted:Record<string,unknown>[]=[];
    vi.stubGlobal("fetch",vi.fn(async(_url:string,input?:RequestInit)=>{
      if(input?.method==="POST"){
        posts++;const body=JSON.parse(String(input.body));admitted.push({id:"00000000-0000-4000-8000-000000000001",order_id:"chat",state:"running",error:null,outcome:null,idempotency_key:body.idempotency_key,request_sha256:body.approval_sha256});
        throw new Error("lost response");
      }
      return new Response(JSON.stringify(admitted),{status:200,headers:{"content-type":"application/json"}});
    }));
    const original=savePendingToolbox(disk,client.base,"chat","github",request,preview);
    await expect(submitPendingToolbox(client,original)).rejects.toThrow("lost response");expect(posts).toBe(1);
    // A reload only reads the journal. It neither executes nor generates a fresh key.
    const restored=readPendingToolbox(disk,client.base,"chat","github")!;expect(posts).toBe(1);
    const recovered=await submitPendingToolbox(client,restored);expect(recovered.id).toBe("00000000-0000-4000-8000-000000000001");expect(posts).toBe(1);
    expect(JSON.stringify([...disk.values])).not.toContain("private-owner-token");
    clearPendingToolbox(disk,restored);expect(readPendingToolbox(disk,client.base,"chat","github")).toBeNull();
  });
  it("fails closed on a damaged journal or mismatched receipt binding",async()=>{
    const disk=storage();disk.setItem(toolboxPendingKey("","chat","office"),"broken json");
    expect(()=>readPendingToolbox(disk,"","chat","office")).toThrow("pending_toolbox_journal_invalid");
    const record=savePendingToolbox(storage(),"","chat","github",newToolboxRequest({kind:"github",arguments:["repo","view","owner/repository"]}),preview);
    vi.stubGlobal("fetch",vi.fn(async()=>new Response(JSON.stringify([{idempotency_key:record.request.idempotency_key,request_sha256:"b".repeat(64)}]),{status:200})));
    await expect(submitPendingToolbox(new Client("token",""),record)).rejects.toThrow("pending_toolbox_binding_changed");
    expect(fetch).toHaveBeenCalledTimes(1);
  });
  it("does not acknowledge a successful HTTP response without an exact job receipt",async()=>{
    const disk=storage(),client=new Client("token"),record=savePendingToolbox(disk,"","chat","office",newToolboxRequest({kind:"office",operation:"create_pdf",output:"report.pdf",text:"Report"}),preview);
    vi.stubGlobal("fetch",vi.fn(async(_url:string,input?:RequestInit)=>new Response(JSON.stringify(input?.method==="POST"?{saved:true}:[]),{status:200})));
    await expect(submitPendingToolbox(client,record)).rejects.toThrow("pending_toolbox_acknowledgement_invalid");
    expect(readPendingToolbox(disk,"","chat","office")).toEqual(record);
  });
});
