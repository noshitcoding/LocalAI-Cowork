import {readFileSync} from "node:fs";
import {afterEach,expect,it,vi} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {Client,decode,type CreateRun} from "@cowork/client";
import {EnvironmentSnapshotExecutionReview,ManagedEnvironmentConsent,environmentExecutionSelection} from "./EnvironmentSnapshotExecutionReview";
import type {EnvironmentRecord} from "./EnvironmentPanel";
import {PersistentRequestLatch} from "./requestJournal";
import {workspaceJournalKey} from "./workspaceManagementApi";
import {ExecutionPlanReview} from "./ExecutionPlanReview";

const state=vi.hoisted(()=>({language:"en"}));
vi.mock("./view",()=>({useView:(fn:(value:typeof state)=>unknown)=>fn(state)}));
const fixture=JSON.parse(readFileSync(new URL("../../client/src/fixtures/execution-managed-input.json",import.meta.url),"utf8"));
function request(){return decode("CreateRun",structuredClone(fixture.prepare.request));}
function records(input:CreateRun):EnvironmentRecord[]{
  const ref=input.environment!;
  return [{id:ref.environment_id,name:"Own runtime",project_id:fixture.prepared.plan.project_id,revision:4,revoked:false,active_version_id:ref.version_id,active_reference:structuredClone(ref),versions:[{
    id:ref.version_id,sha256:ref.definition_sha256,created_at:1,revoked:false,
    definition:{format_version:3,artifact:{kind:"managed_linux_package",package_sha256:"a".repeat(64),rootfs_manifest_sha256:"b".repeat(64),gvisor_manifest_sha256:"c".repeat(64),maximum_files:100,output_bytes:1024},network:{kind:"denied"},programs:{probe:"bin/probe"},packages:[],limits:{cpu_millis:1000,memory_bytes:268435456,processes:16,scratch_bytes:1048576}},
    state:{state:"qualified",qualification:{runner_id:"runner",runner_generation:1,adapter_build_sha256:"d".repeat(64),artifact_sha256:"a".repeat(64),definition_sha256:ref.definition_sha256,report_sha256:"e".repeat(64),capabilities:[],verified_at:1}}
  }]}];
}
afterEach(()=>{vi.unstubAllGlobals();state.language="en";});

it("selects the exact managed version without constructing runner authority in the client",()=>{
  const input=request(),catalog=records(input),before=structuredClone(catalog);
  expect(environmentExecutionSelection(input,catalog[0].project_id,catalog)).toEqual({kind:"managed",name:"Own runtime"});
  expect(catalog).toEqual(before);
});
it("keeps an explicitly chosen existing environment on its existing path",()=>{
  const input=request(),catalog=records(input),version=catalog[0].versions[0];
  version.definition.format_version=2;version.definition.artifact={kind:"linux_kata_image",reference:"example.invalid/pinned@sha256:"+"f".repeat(64)};
  expect(environmentExecutionSelection(input,catalog[0].project_id,catalog).kind).toBe("existing");
});
for(const mutation of ["environment_revoked","version_revoked","different_project","different_version","changed_qualification","unqualified","duplicate"] as const){
  it(`rejects ${mutation} without selecting another backend`,()=>{
    const input=request(),catalog=records(input),project=catalog[0].project_id;
    if(mutation==="environment_revoked")catalog[0].revoked=true;
    if(mutation==="version_revoked")catalog[0].versions[0].revoked=true;
    if(mutation==="different_project")catalog[0].project_id="foreign";
    if(mutation==="different_version")catalog[0].active_version_id="replacement";
    if(mutation==="changed_qualification")catalog[0].active_reference!.qualification_sha256="0".repeat(64);
    if(mutation==="unqualified")catalog[0].versions[0].state={state:"staged"};
    if(mutation==="duplicate")catalog.push(structuredClone(catalog[0]));
    expect(()=>environmentExecutionSelection(input,project,catalog)).toThrow("environment_execution_selection_changed");
  });
}
it("does not turn managed networking or mixed connector rights into legacy execution",()=>{
  const input=request(),catalog=records(input),project=catalog[0].project_id;
  catalog[0].versions[0].definition.network={kind:"web_proxy",policy_id:"proxy"};
  expect(()=>environmentExecutionSelection(input,project,catalog)).toThrow("environment_execution_selection_unsupported");
  catalog[0].versions[0].definition.network={kind:"denied"};input.native_tools=["execute_command"];
  expect(()=>environmentExecutionSelection(input,project,catalog)).toThrow("environment_execution_selection_unsupported");
});
it("explains the separate visible selection and later confirmation in both languages",()=>{
  for(const language of ["en","de"]){state.language=language;
    const html=renderToStaticMarkup(<ManagedEnvironmentConsent name="Own runtime" busy={false} choose={()=>{}} retire={()=>{}}/>);
    expect(html).toContain("Own runtime");expect(html).toContain(language==="en"?"registered runtime":"registrierte Runtime");
    expect(html).toContain(language==="en"?"review and confirm":"prüfst und bestätigst");
    expect(html).toContain(language==="en"?"process networking remains denied":"Prozessnetzwerk bleibt gesperrt");
    expect(html).not.toContain("start working copy");
  }
});
function storage(){const values=new Map<string,string>();const result={getItem:(key:string)=>values.get(key)??null,setItem:(key:string,value:string)=>{values.set(key,value);},removeItem:(key:string)=>{values.delete(key);}};vi.stubGlobal("localStorage",result);return result;}
it("recovers the stored original managed intent before asking for a new selection",()=>{
  const input=request(),client=new Client("fixture","https://fixture.invalid"),saved=storage();
  const key=workspaceJournalKey(client,JSON.stringify(["snapshot-execution-intent",input.order_id,input.idempotency_key]));
  new PersistentRequestLatch(key,value=>decode("PrepareOwnerExecutionPlanV2",value),saved).prepare(()=>fixture.prepare);
  const html=renderToStaticMarkup(<EnvironmentSnapshotExecutionReview client={client} request={input} fallback={<p>legacy-path</p>} onStarted={async()=>{}} onRetired={()=>{}}/>);
  expect(html).toContain('aria-label="Review execution"');expect(html).not.toContain("legacy-path");expect(html).not.toContain('aria-label="Choose runtime"');
});
it("keeps an original accepted legacy confirmation ahead of a new environment selection",()=>{
  const input=request(),client=new Client("fixture","https://fixture.invalid"),saved=storage();
  const source=fixture.prepared.plan;
  const legacy=decode("PreparedExecutionPlan",{plan:{format_version:1,id:"legacy-plan",actor:"installation-owner",project_id:source.project_id,order_id:input.order_id,idempotency_key:input.idempotency_key,
    request_sha256:source.request_sha256,spec_sha256:source.spec_sha256,created_at:source.created_at,expires_at:source.expires_at,preflight:source.preflight.review,boundary:source.boundary},
    sha256:"a".repeat(64),committed_run_id:"original-run",retired:false});
  const key=workspaceJournalKey(client,JSON.stringify(["execution-plan-confirmation",input.order_id,input.idempotency_key]));
  new PersistentRequestLatch(key,value=>decode("PreparedExecutionPlan",value),saved).prepare(()=>legacy);
  const html=renderToStaticMarkup(<ExecutionPlanReview client={client} request={input} onStarted={async()=>{}} onRetired={()=>{}}/>);
  expect(html).not.toContain('aria-label="Choose runtime"');
});
