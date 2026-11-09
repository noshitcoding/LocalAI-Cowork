import {readFileSync} from "node:fs";
import {afterEach,expect,it,vi} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {Client,decode,type PrepareOwnerExecutionPlanV2} from "@cowork/client";
import {SingleExecutionChoice,SingleSnapshotExecutionReview} from "./SingleSnapshotExecutionReview";
import {executionDefaultsScope,saveDefaultExecutionMode} from "./executionDefaults";
import {PersistentRequestLatch} from "./requestJournal";
import {workspaceJournalKey} from "./workspaceManagementApi";

const language=vi.hoisted(()=>({value:"de"}));
vi.mock("./view",()=>({useView:(selector:(view:{language:string})=>unknown)=>selector({language:language.value})}));
const review=vi.hoisted(()=>({targets:[] as unknown[]}));
vi.mock("./SnapshotExecutionPlanReview",()=>({SnapshotExecutionPlanReview:({executionTarget}:{executionTarget?:PrepareOwnerExecutionPlanV2["target"]})=>{review.targets.push(executionTarget);return <p>plan-review</p>;}}));
const fixture=JSON.parse(readFileSync(new URL("../../client/src/fixtures/execution-plan-v2.json",import.meta.url),"utf8"));
afterEach(()=>{vi.unstubAllGlobals();review.targets=[];language.value="de";});
function storage(){const values=new Map<string,string>();const saved={getItem:(key:string)=>values.get(key)??null,setItem:(key:string,value:string)=>{values.set(key,value);},removeItem:(key:string)=>{values.delete(key);}};vi.stubGlobal("localStorage",saved);return saved;}

it("separates bounded file access from explicit document process consent in both languages",()=>{
 for(const value of ["de","en"]){language.value=value;const choose=vi.fn(),retire=vi.fn();
  const html=renderToStaticMarkup(<SingleExecutionChoice busy={false} choose={choose} retire={retire}/>);
  expect(html).toContain(value==="de"?"Mit Dateibroker prüfen":"Review with file broker");
  expect(html).toContain(value==="de"?"Dokumentengine verwenden":"Use document engine");
  expect(html).toContain(value==="de"?"Betriebssystem- und Netzwerkrechten":"operating system and network permissions");
  expect(html).toContain(value==="de"?"keine Betriebssystemisolation":"not operating system isolation");
  expect(choose).not.toHaveBeenCalled();expect(retire).not.toHaveBeenCalled();
 }
});
it("disables all target changes and retirement while a request is being settled",()=>{
 const html=renderToStaticMarkup(<SingleExecutionChoice busy choose={()=>{throw new Error("must wait");}} retire={()=>{throw new Error("must wait");}}/>);
 expect(html.match(/disabled=""/g)).toHaveLength(3);
});

it("opens the plan review with the saved default without starting the task",()=>{
  const saved=storage(),client=new Client("fixture","https://fixture.invalid"),request=decode("CreateRun",fixture.prepare.request),onStarted=vi.fn();
  for(const mode of ["broker_controller","document_engine"] as const){
    saveDefaultExecutionMode(saved,executionDefaultsScope(client),mode);
    const html=renderToStaticMarkup(<SingleSnapshotExecutionReview client={client} request={request} onStarted={onStarted} onRetired={()=>{}}/>);
    expect(html).toContain("plan-review");expect(review.targets.at(-1)).toEqual({kind:mode});
  }
  expect(onStarted).not.toHaveBeenCalled();
});
it("retains per-task selection when the user explicitly chooses to ask each time",()=>{
  const saved=storage(),client=new Client("fixture","https://fixture.invalid");
  saveDefaultExecutionMode(saved,executionDefaultsScope(client),"ask");
  const html=renderToStaticMarkup(<SingleSnapshotExecutionReview client={client} request={decode("CreateRun",fixture.prepare.request)} onStarted={async()=>{}} onRetired={()=>{}}/>);
  expect(html).toContain("Ausführungsart auswählen");expect(review.targets).toHaveLength(0);
});
it("gives the saved original intent precedence over a changed default",()=>{
  const saved=storage(),client=new Client("fixture","https://fixture.invalid"),request=decode("CreateRun",fixture.prepare.request);
  const key=workspaceJournalKey(client,JSON.stringify(["snapshot-execution-intent",request.order_id,request.idempotency_key]));
  new PersistentRequestLatch(key,value=>decode("PrepareOwnerExecutionPlanV2",value),saved).prepare(()=>fixture.prepare);
  saveDefaultExecutionMode(saved,executionDefaultsScope(client),"document_engine");
  const before=saved.getItem(key);
  const html=renderToStaticMarkup(<SingleSnapshotExecutionReview client={client} request={request} onStarted={async()=>{}} onRetired={()=>{}}/>);
  expect(html).toContain("plan-review");expect(review.targets.at(-1)).toBeUndefined();expect(saved.getItem(key)).toBe(before);
});
