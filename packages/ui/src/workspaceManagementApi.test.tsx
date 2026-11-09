import {afterEach,describe,expect,it,vi} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {QueryClient,QueryClientProvider} from "@tanstack/react-query";
import {Client,type ProjectSettings,type StartWorkspaceTask} from "@cowork/client";
import {WorkspaceRequestLatch,manageChat,archiveChat,projectInstructionSnapshot,startWorkspaceTask,writeProjectSettings,workspaceJournalKey,confirmedWorkspaceRejection,type ManageChat} from "./workspaceManagementApi";
import {ChatManagementDialog} from "./ChatManagementDialog";
import {restoreTaskMutation} from "./TaskWorkspace";
afterEach(()=>vi.unstubAllGlobals());
const order={id:"chat",project_id:"project",title:"Original",draft:"Keep draft",revision:4,created_at:10};
describe("native workspace management",()=>{
  it("retains an already committed chat move across a lost ACK, expired session, reload and token rotation",async()=>{
    const values=new Map<string,string>(),storage={getItem:(key:string)=>values.get(key)??null,setItem:(key:string,value:string)=>{values.set(key,value);},removeItem:(key:string)=>{values.delete(key);}};
    const oldClient=new Client("EXPIRED-OWNER","https://host"),newClient=new Client("ROTATED-OWNER","https://host"),key=workspaceJournalKey(oldClient,"chat-move");
    const first=new WorkspaceRequestLatch<ManageChat>(key,undefined,storage),original=first.prepare(()=>({expected_revision:4,idempotency_key:"committed-once",title:"Renamed",project_id:"target"}));
    const fetch=vi.fn().mockRejectedValueOnce(new TypeError("committed but acknowledgement lost")).mockResolvedValueOnce(new Response(JSON.stringify({code:"unauthorized",message:"expired",retryable:false,correlation_id:"fixture"}),{status:401})).mockResolvedValueOnce(new Response(JSON.stringify({order:{...order,title:"Renamed",project_id:"target",revision:5},archived:false})));vi.stubGlobal("fetch",fetch);
    async function attempt(client:Client,latch:WorkspaceRequestLatch<ManageChat>){try{const request=latch.prepare(()=>{throw new Error("must recover original move");});const saved=await manageChat(client,"chat",request);latch.confirm();return saved;}catch(error){if(confirmedWorkspaceRejection(error))latch.confirm();throw error;}}
    await expect(attempt(oldClient,first)).rejects.toThrow("acknowledgement lost");
    const afterReload=new WorkspaceRequestLatch<ManageChat>(key,undefined,storage);await expect(attempt(oldClient,afterReload)).rejects.toMatchObject({status:401});expect(afterReload.pending).toBe(true);
    expect(workspaceJournalKey(newClient,"chat-move")).toBe(key);const rotated=new WorkspaceRequestLatch<ManageChat>(key,undefined,storage);expect(rotated.prepare(()=>{throw new Error();})).toEqual(original);
    expect((await attempt(newClient,rotated)).order.revision).toBe(5);expect(values.size).toBe(0);
    expect(fetch.mock.calls.map(call=>call[1].body)).toEqual([JSON.stringify(original),JSON.stringify(original),JSON.stringify(original)]);
    expect(fetch.mock.calls.map(call=>call[1].headers.Authorization)).toEqual(["Bearer EXPIRED-OWNER","Bearer EXPIRED-OWNER","Bearer ROTATED-OWNER"]);
  });
  it("restores a saved task execution request after restart and rejects an altered journal shape",()=>{
    const values=new Map<string,string>(),storage={getItem:(key:string)=>values.get(key)??null,setItem:(key:string,value:string)=>{values.set(key,value);},removeItem:(key:string)=>{values.delete(key);}};
    const original={kind:"run" as const,id:"task",body:{idempotency_key:"original",expected_revision:3,order_revision:4}},first=new WorkspaceRequestLatch("native-task-journal",restoreTaskMutation,storage);first.prepare(()=>original);
    const restored=new WorkspaceRequestLatch("native-task-journal",restoreTaskMutation,storage);expect(restored.prepare(()=>{throw new Error();})).toEqual(original);restored.confirm();expect(values.size).toBe(0);
    expect(()=>restoreTaskMutation({...original,token:"forbidden"})).toThrow("task_journal_invalid");
  });
  it("preserves the exact task run request after a lost response",async()=>{
    const latch=new WorkspaceRequestLatch<StartWorkspaceTask>(),fetch=vi.fn().mockRejectedValue(new TypeError("lost acknowledgement"));vi.stubGlobal("fetch",fetch);
    const original=latch.prepare(()=>({idempotency_key:"original",expected_revision:3,order_revision:4}));await expect(startWorkspaceTask(new Client("owner"),"task",original)).rejects.toThrow();
    const retry=latch.prepare(()=>({idempotency_key:"new",expected_revision:8,order_revision:9}));await expect(startWorkspaceTask(new Client("owner"),"task",retry)).rejects.toThrow();
    expect(fetch.mock.calls[1][0]).toBe("/api/v2/workspace-tasks/task/run");expect(fetch.mock.calls[1][1].body).toBe(fetch.mock.calls[0][1].body);expect(latch.pending).toBe(true);
  });
  it("validates chat move and archive receipts against original identity and CAS revision",async()=>{
    const fetch=vi.fn().mockResolvedValueOnce(new Response(JSON.stringify({order:{...order,revision:5,project_id:"target"},archived:false}))).mockResolvedValueOnce(new Response(JSON.stringify({order:{...order,revision:5},archived:true}))).mockResolvedValueOnce(new Response(JSON.stringify({order:{...order,id:"another",revision:5},archived:false})));vi.stubGlobal("fetch",fetch);
    const request={expected_revision:4,idempotency_key:"move",title:"Renamed",project_id:"target"};expect((await manageChat(new Client("owner"),"chat",request)).order.project_id).toBe("target");
    expect((await archiveChat(new Client("owner"),"chat",{expected_revision:4,idempotency_key:"archive",archived:true})).archived).toBe(true);
    await expect(manageChat(new Client("owner"),"chat",request)).rejects.toThrow("workspace_receipt_mismatch");
  });
  it("preserves enabled source metadata without treating it as a filesystem grant",()=>{
    const settings:ProjectSettings={project_id:"project",revision:2,name:"Research",instructions:"Cite evidence",working_folder:null,sources:[{id:"public",label:"Public paper",kind:"link",reference:"https://example.com",access:"read_only",enabled:true},{id:"disabled",label:"Do not include",kind:"file",reference:"secret.txt",access:"read_only",enabled:false}]};
    const text=projectInstructionSnapshot(settings);expect(text).toContain("Cite evidence");expect(text).toContain("https://example.com");expect(text).toContain("metadata grants no file access");expect(text).not.toContain("secret.txt");
  });
  it("rejects a project settings receipt from another project",async()=>{
    vi.stubGlobal("fetch",vi.fn().mockResolvedValue(new Response(JSON.stringify({project_id:"other",revision:2,name:"Name",instructions:"",sources:[],working_folder:null}))));
    await expect(writeProjectSettings(new Client("owner"),"project",{expected_revision:1,idempotency_key:"key",name:"Name",instructions:"",sources:[],working_folder:null})).rejects.toThrow("workspace_receipt_mismatch");
  });
  it("embeds the chat editor without creating a nested dialog and explains retained history",()=>{
    const markup=renderToStaticMarkup(<QueryClientProvider client={new QueryClient()}><ChatManagementDialog client={new Client("owner")} order={order} projects={[{id:"project",name:"Project",revision:1,created_at:0}]} onChanged={()=>{}} onClose={()=>{}}/></QueryClientProvider>);
    expect(markup).not.toContain('role="dialog"');expect(markup).toContain("Chatname");expect(markup).toContain("Archivieren");expect(markup).toContain("Original");
  });
});
