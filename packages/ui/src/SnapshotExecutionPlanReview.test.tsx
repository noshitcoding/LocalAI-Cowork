import {readFileSync} from "node:fs";
import {beforeEach,expect,it,vi} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {Client,decode,type PreparedExecutionPlanV2} from "@cowork/client";
import {SnapshotPlanSummary} from "./SnapshotExecutionPlanReview";
import {WorkingFiles} from "./WorkingFiles";
import {en} from "./i18n";

const fixture=JSON.parse(readFileSync(new URL("../../client/src/fixtures/execution-plan-v2.json",import.meta.url),"utf8"));
const queries=vi.hoisted(()=>({options:[] as Array<{queryKey:unknown[];enabled?:boolean}>,failed:false,language:"en"}));
vi.mock("./view",()=>({useView:(selector:(view:{language:string})=>unknown)=>selector({language:queries.language})}));
vi.mock("@tanstack/react-query",()=>({useQueryClient:()=>({invalidateQueries:vi.fn()}),useQuery:(options:{queryKey:unknown[];enabled?:boolean})=>{
  queries.options.push(options);
  if(options.queryKey[0]==="run-copy-files")return {isSuccess:!queries.failed,isError:queries.failed,isPending:false,isFetching:false,error:queries.failed?new Error("missing copy"):null,data:queries.failed?undefined:{entries:[{name:"changed.txt",path:"changed.txt",kind:"file",size:5}],truncated:false},refetch:vi.fn()};
  return {isSuccess:false,isError:false,isPending:false,isFetching:false,data:undefined,refetch:vi.fn()};
}}));
beforeEach(()=>{queries.language="en";queries.options=[];queries.failed=false;});

it("reviews frozen data, real model recipients and bounded copy permissions",()=>{
  const p:PreparedExecutionPlanV2=decode("PreparedExecutionPlanV2",structuredClone(fixture.prepared));
  const html=renderToStaticMarkup(<SnapshotPlanSummary prepared={p} target="https://cowork.example"/>);
  expect(html).toContain("https://cowork.example");expect(html).toContain("own file copy");expect(html).toContain("separate adoption");
  expect(html).toContain("Fixture model");expect(html).toContain("Empty working copy");expect(html).toContain(p.sha256);
  expect(html).not.toContain("canonical_json");expect(html).not.toContain("generation");
  queries.language="de";expect(renderToStaticMarkup(<SnapshotPlanSummary prepared={p} target="https://cowork.example"/>)).toContain("eigenen Dateikopie");
});

it("shows the frozen workflow name/version and typed input even when no model is required",()=>{
  const p=decode("PreparedExecutionPlanV2",structuredClone(fixture.prepared));
  if(p.plan.preflight.kind!=="owner")throw new Error("owner fixture");
  const source=p.plan.preflight.review;
  p.plan.preflight.review={ready:true,executor:source.executor,access_mode:source.access_mode,budgets:source.budgets,allowed_tools:[],policy_revision:1,
    name:"Invoice review",version_revision:2,max_parallel:4,roles:[],workflow:{workflow_id:"workflow",version_id:"frozen-version",definition_sha256:"a".repeat(64),plan_sha256:"b".repeat(64),inputs:{currency:"EUR"}}};
  const html=renderToStaticMarkup(<SnapshotPlanSummary prepared={p} target="https://fixture.invalid"/>);
  expect(html).toContain("Invoice review");expect(html).toContain("Version");expect(html).toContain("2");expect(html).toContain("None");expect(html).toContain("EUR");
});

it("shows the exact native input bytes instead of a rounded JavaScript number",()=>{
 const value=JSON.parse(readFileSync(new URL("../../client/src/fixtures/execution-workflow-input-source.json",import.meta.url),"utf8"));
 const p=decode("PreparedExecutionPlanV2",value.prepared);
 const html=renderToStaticMarkup(<SnapshotPlanSummary prepared={p} target="https://fixture.invalid"/>);
 expect(html).toContain("9007199254740993");expect(html).not.toContain("9007199254740992");
 expect(html).toContain("Bound workflow inputs");expect(html).toContain("None");
});

it("shows the native managed recipient, programs and initial execution limit",()=>{
 const value=JSON.parse(readFileSync(new URL("../../client/src/fixtures/execution-managed-input.json",import.meta.url),"utf8"));
 const p=decode("PreparedExecutionPlanV2",value.prepared);
 const html=renderToStaticMarkup(<SnapshotPlanSummary prepared={p} target="https://fixture.invalid"/>);
 expect(html).toContain("managed-input-fixture");expect(html).toContain("probe");expect(html).toContain("Denied");
 expect(html).toContain("At most 30 seconds");expect(html).not.toContain("File broker");
});

it("shows the selected run copy and its retained result instead of querying the shared project workspace",()=>{
  const run=decode("RunRecord",structuredClone(fixture.run));
  run.spec.folders=[{id:"copied-source",name:"Selected source",project_id:"project",resource_id:"opaque",revision:"opaque",writable:true}];
  const snapshot={run_id:run.id,order_id:"order",sha256:"a".repeat(64),entries:[{path:"input-0001/changed.txt",sha256:"b".repeat(64),size:5}]};
  const html=renderToStaticMarkup(<WorkingFiles order="order" client={new Client("owner")} t={k=>en[k]} run={run} snapshot={snapshot} prepareImport={()=>{throw new Error("copy view must not import into originals");}}/>);
  expect(html).toContain("This run&#x27;s working copy");expect(html).toContain("Selected source");expect(html).toContain("changed.txt");expect(html).toContain("Retained result version");
  expect(queries.options.some(o=>o.queryKey[0]==="workspace")).toBe(false);
  expect(queries.options.find(o=>o.queryKey[0]==="run-copy-files")?.queryKey).toEqual(["run-copy-files","order","run","copied-source","."]);
});

it("retains the immutable result when the mutable working directory is unavailable",()=>{
  queries.failed=true;const run=decode("RunRecord",structuredClone(fixture.run));
  run.spec.folders=[{id:"copied-source",name:"Selected source",project_id:"project",resource_id:"opaque",revision:"opaque",writable:true}];
  const snapshot={run_id:run.id,order_id:"order",sha256:"a".repeat(64),entries:[{path:"retained.txt",sha256:"b".repeat(64),size:5}]};
  const html=renderToStaticMarkup(<WorkingFiles order="order" client={new Client("owner")} t={k=>en[k]} run={run} snapshot={snapshot} prepareImport={()=>{}}/>);
  expect(html).toContain('role="alert"');expect(html).toContain("retained.txt");expect(html).toContain("Its retained result version is listed below.");
});

it("keeps uploads available beside a copied result only with the confirmed import capability",()=>{
  const run=decode("RunRecord",structuredClone(fixture.run));
  const props={order:"order",client:new Client("owner"),t:(k:keyof typeof en)=>en[k],run,prepareImport:()=>{}};
  expect(renderToStaticMarkup(<WorkingFiles {...props}/>)).not.toContain(en.addFile);
  const markup=renderToStaticMarkup(<WorkingFiles {...props} copyImport/>);
  expect(markup).toContain(en.addFile);expect(markup).toContain("new confirmed working copy");expect(markup).toContain("first branch from its answer");
  expect(queries.options.some(o=>o.queryKey[0]==="workspace")).toBe(false);
  expect(queries.options.some(o=>o.queryKey[0]==="uploads")).toBe(true);
});


it("shows document engine user permissions and never claims process-free isolation",()=>{
 const value=JSON.parse(readFileSync(new URL("../../client/src/fixtures/execution-document-input.json",import.meta.url),"utf8"));
 const p=decode("PreparedExecutionPlanV2",value.prepared);
 for(const language of ["en","de"]){queries.language=language;
  const html=renderToStaticMarkup(<SnapshotPlanSummary prepared={p} target="https://fixture.invalid"/>);
  expect(html).toContain(language==="de"?"Dokumentengine":"Document engine");
  expect(html).toContain(language==="de"?"Betriebssystemrechten":"operating system permissions");
  expect(html).toContain(language==="de"?"Netzwerkrechte deines Benutzerkontos":"Network permissions of your user account");
  expect(html).not.toContain(language==="de"?"Dateibroker":"File broker");expect(html).not.toContain("Denied");
 }
});
