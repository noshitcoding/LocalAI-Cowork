import {beforeEach,expect,it,vi} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {Client,type OwnerManagedResultEntry} from "@cowork/client";
import fixture from "../../client/src/fixtures/owner-managed-result.json";
import {ManagedResultsPanel} from "./ManagedResultsPanel";
import {en} from "./i18n";

const state=vi.hoisted(()=>({language:"en",entries:[] as unknown[],listError:false,listFetching:false,manifestError:false,manifestFetching:false,manifest:undefined as unknown,processes:[] as unknown[],delivery:undefined as unknown,options:[] as Array<{queryKey:unknown[];retry:boolean;refetchOnWindowFocus:boolean}>}));
vi.mock("./view",()=>({useView:(select:(value:{language:string})=>unknown)=>select(state)}));
vi.mock("@tanstack/react-query",()=>({useQuery:(options:typeof state.options[number])=>{
  state.options.push(options);const list=options.queryKey[0]==="owner-managed-results";
  if(options.queryKey[0]==="owner-managed-result-process-history")return {isSuccess:true,isError:false,isFetching:false,data:state.processes,refetch:vi.fn()};
  if(options.queryKey[0]==="owner-managed-result-delivery")return {isSuccess:true,isError:false,isFetching:false,data:state.delivery,refetch:vi.fn()};
  const error=list?state.listError:state.manifestError,fetching=list?state.listFetching:state.manifestFetching;
  return {isPending:false,isSuccess:!error,isError:error,isFetching:fetching,error:error?new Error("forbidden"):null,data:list?{entries:state.entries}:state.manifest,refetch:vi.fn()};
}}));
const valid=fixture.valid;
const client=new Client("private-test-credential","https://owner.invalid");
function render(connection=client,order="order",run="run"){return renderToStaticMarkup(<ManagedResultsPanel client={connection} order={order} run={run} t={key=>en[key]}/>);}
beforeEach(()=>{state.entries=[];state.processes=[];state.delivery=undefined;state.listError=false;state.listFetching=false;state.manifestError=false;state.manifestFetching=false;state.manifest=structuredClone(valid);state.options=[];});

it("keeps unreceived and partial results explicit without creating remote work during render",()=>{
  const partial=structuredClone(valid.entry) as OwnerManagedResultEntry;partial.version!.state="receiving";
  state.entries=[{...valid.entry,process_attempt_id:"unreceived",version:null},partial];
  const receive=vi.spyOn(client,"receiveOwnerManagedResult");const html=render();
  expect(html).toContain("Not yet retained locally");expect(html).toContain("Retrieve result");
  expect(html).toContain("Transfer incomplete");expect(html).toContain("Continue retrieving missing blocks");
  expect(receive).not.toHaveBeenCalled();expect(state.options).toHaveLength(2);receive.mockRestore();
});
it("exposes downloads only after the local manifest matches the selected immutable version",()=>{
  state.entries=[valid.entry];let html=render();
  expect(html).toContain("Retained locally and fully verified");expect(html).toContain("Download result file: input-0001/proof.bin");
  state.manifest={...valid,entry:{...valid.entry,version:{...valid.entry.version,revision:99}}};html=render();
  expect(html).not.toContain("Download result file:");expect(html).not.toContain("Retained locally and fully verified");expect(html).toContain('role="alert"');
});
it("hides cached file actions when local access fails or is being revalidated",()=>{
  state.entries=[valid.entry];state.manifestError=true;let html=render();
  expect(html).not.toContain("Download result file:");expect(html).toContain("This version is currently unreadable");
  state.manifestError=false;state.manifestFetching=true;html=render();expect(html).not.toContain("Download result file:");
  state.listError=true;html=render();expect(html).not.toContain("input-0001/proof.bin");expect(html).toContain("The result overview could not be verified");
});
it("scopes queries to connection, order and run without storing credentials in cache keys",()=>{
  render();const original=state.options[0].queryKey;
  expect(JSON.stringify(original)).not.toContain(client.token);expect(original.slice(2)).toEqual(["order","run"]);
  state.options=[];render(client,"other","new-run");expect(state.options[0].queryKey.slice(2)).toEqual(["other","new-run"]);
  state.options=[];render(new Client("second-credential",client.base));expect(state.options[0].queryKey[1]).not.toBe(original[1]);
  expect(state.options.every(option=>option.retry===false&&option.refetchOnWindowFocus===false)).toBe(true);
});
it("keeps locally ready results readable without offering branch or adoption actions",()=>{
  state.entries=[valid.entry];const html=render();
  expect(html).toContain("Download result file:");expect(html).not.toContain("Retrieve result");
  expect(html).not.toMatch(/branch|adopt|promote|environment settings/i);
});
it("retains accepted delivery history and status refresh after content listing access is denied",()=>{
  state.entries=[valid.entry];state.listError=true;
  state.processes=[{id:valid.entry.process_attempt_id}];
  state.delivery={order_id:"order",run_id:"run",process_attempt_id:valid.entry.process_attempt_id,delivery:{
    id:"b7f5fe42-28ed-4bb7-bf65-e9870a5987f6",result_version_id:valid.entry.version!.id,
    request_sha256:"a".repeat(64),manifest_sha256:valid.entry.version!.manifest_sha256,state:"accepted",
    created_at:1793510000000,accept_before:1793510030000,revision:3,accepted_at:1793510001000,
    delivery_receipt_sha256:"c".repeat(64),
  }};
  const html=render();expect(html).toContain("The result overview could not be verified");
  expect(html).toContain("Receipt acknowledged by the runner");expect(html).toContain("Check release status");
  expect(html).not.toContain("Download result file:");expect(html).not.toContain("Prepare storage release");
  expect(html).not.toContain("Confirm receipt and release runner storage");
});
