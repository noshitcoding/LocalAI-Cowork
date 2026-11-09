import {afterEach,beforeEach,describe,expect,it,vi} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {Client,defaultRunBudgets,type WorkflowOperation} from "@cowork/client";
import {useWorkflowRecovery,workflowClientIdentity} from "./workflowJournal";
import {WorkflowLibrary,exportWorkflowDraft} from "./WorkflowLibrary";
import {en} from "./i18n";
const queries=vi.hoisted(()=>({operations:[] as WorkflowOperation[],fetching:false,success:true,keys:[] as Array<readonly unknown[]>}));
const {invoke}=vi.hoisted(()=>({invoke:vi.fn()}));
vi.mock("@tauri-apps/api/core",()=>({invoke}));
vi.mock("@tanstack/react-query",()=>({useQueryClient:()=>({setQueryData:vi.fn()}),useQuery:(options:{queryKey:readonly unknown[]})=>{
  queries.keys.push(options.queryKey);return {isSuccess:queries.success,isFetching:queries.fetching,data:{operations:queries.operations,drafts:[]},isError:!queries.success};
}}));
const match=(value:WorkflowOperation)=>value.request.kind!=="start";
function Harness({client,project}:{client:Client;project:string}){
  const state=useWorkflowRecovery(client,project,match);
  return <output>{state.ready?"ready":"blocked"}</output>;
}
const operation=():WorkflowOperation=>({id:"operation",project_id:"project",intent_id:"intent",request_sha256:"a".repeat(64),request:{kind:"revoke",workflow_id:"workflow",request:{base_revision:1,idempotency_key:"key"}},draft_snapshot:null,state:"pending",receipt:null,rejection:null,created_at:1,updated_at:1,acknowledged_at:null});
beforeEach(()=>{queries.operations=[];queries.keys=[];queries.fetching=false;queries.success=true;invoke.mockReset();});
afterEach(()=>{vi.unstubAllGlobals();});
describe("workflow recovery before effects run",()=>{
  it.each(["pending","accepted","rejected","discarded"] as const)("blocks new keys for an unacknowledged %s record in the first render",state=>{
    queries.operations=[{...operation(),state}];
    expect(renderToStaticMarkup(<Harness client={new Client("fixture")} project="project"/>)).toBe("<output>blocked</output>");
  });
  it("allows an acknowledged record but waits for recovery revalidation and failed reads",()=>{
    const client=new Client("fixture");queries.operations=[{...operation(),acknowledged_at:2}];
    expect(renderToStaticMarkup(<Harness client={client} project="project"/>)).toBe("<output>ready</output>");
    queries.fetching=true;expect(renderToStaticMarkup(<Harness client={client} project="project"/>)).toContain("blocked");
    queries.fetching=false;queries.success=false;expect(renderToStaticMarkup(<Harness client={client} project="project"/>)).toContain("blocked");
  });
  it("isolates cached recovery and editor state by client instance and project",()=>{
    const first=new Client("fixture","http://one"),second=new Client("fixture","http://two");
    renderToStaticMarkup(<Harness client={first} project="project"/>);renderToStaticMarkup(<Harness client={second} project="project"/>);
    renderToStaticMarkup(<Harness client={first} project="other"/>);
    expect(queries.keys[0]).not.toEqual(queries.keys[1]);expect(queries.keys[0]).not.toEqual(queries.keys[2]);
    const props={client:first,project:"project",providers:[],t:(key:keyof typeof en)=>en[key],blockClose:()=>{}};
    expect(WorkflowLibrary(props).key).not.toBe(WorkflowLibrary({...props,client:second}).key);
    expect(WorkflowLibrary(props).key).not.toBe(WorkflowLibrary({...props,project:"other"}).key);
    expect(workflowClientIdentity(first)).toBe(workflowClientIdentity(first));
  });
});
describe("workflow draft export through the shared native bridge",()=>{
  it("exports exact opaque draft data with verified UTF8 metadata",async()=>{
    vi.stubGlobal("window",{__TAURI_INTERNALS__:{}});
    const value={editor_format:"unknown",content:{name:"Prüfung",budgets:{...defaultRunBudgets,max_steps:-2.5}},unconfirmed_request:{idempotency_key:"exact-original"}};
    invoke.mockImplementation(async(command:string,body:Uint8Array,options:{headers:Record<string,string>})=>{
      expect(command).toBe("save_file");expect(JSON.parse(new TextDecoder().decode(body))).toEqual(value);
      const request=JSON.parse(decodeURIComponent(options.headers["x-cowork-save"]));
      expect(request.language).toBe("de");expect(request.file.file_name).toBe("workflow-draft.json");
      expect(request.file.size).toBe(body.byteLength);expect(request.file.sha256).toMatch(/^[0-9a-f]{64}$/);
      return {status:"saved",file:request.file};
    });
    expect(await exportWorkflowDraft(value,"de")).toBe("saved");expect(invoke).toHaveBeenCalledOnce();
  });
  it("retains native cancellation and uncertain completion without browser fallback",async()=>{
    vi.stubGlobal("window",{__TAURI_INTERNALS__:{}});vi.stubGlobal("document",{createElement:vi.fn(()=>{throw new Error("unexpected browser fallback");})});
    invoke.mockResolvedValue({status:"canceled"});expect(await exportWorkflowDraft({retained:"draft"},"en")).toBe("canceled");
    invoke.mockRejectedValue(new Error("native response lost"));
    await expect(exportWorkflowDraft({retained:"draft"},"en")).rejects.toMatchObject({details:{code:"save_uncertain"}});
    expect(invoke).toHaveBeenCalledTimes(2);
  });
});
