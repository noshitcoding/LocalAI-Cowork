import {beforeEach,expect,it,vi} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {Client,type OwnerManagedResultEntry,type OwnerManagedResultDeliveryView} from "@cowork/client";
import {ManagedResultDelivery} from "./ManagedResultDelivery";

const state=vi.hoisted(()=>({language:"en",value:undefined as unknown,error:false,fetching:false,
  options:[] as Array<{queryKey:unknown[];retry:boolean;refetchOnWindowFocus:boolean}>}));
vi.mock("./view",()=>({useView:(select:(value:{language:string})=>unknown)=>select(state)}));
vi.mock("@tanstack/react-query",()=>({useQuery:(options:typeof state.options[number])=>{
  state.options.push(options);return {isPending:false,isSuccess:!state.error,isError:state.error,
    isFetching:state.fetching,data:state.value,refetch:vi.fn()};
}}));
const entry:OwnerManagedResultEntry={order_id:"order",run_id:"run",process_attempt_id:"attempt",version:{
  id:"f793719f-4f4d-4fe3-a137-30a0bd2c8cda",revision:2,state:"ready",created_at:1793510000000,
  manifest_sha256:"b".repeat(64),file_count:1,total_bytes:41,
}};
const prepared:OwnerManagedResultDeliveryView={order_id:"order",run_id:"run",process_attempt_id:"attempt",delivery:{
  id:"b7f5fe42-28ed-4bb7-bf65-e9870a5987f6",result_version_id:entry.version!.id,request_sha256:"a".repeat(64),
  manifest_sha256:"b".repeat(64),state:"prepared",created_at:1793510000000,accept_before:1793510030000,revision:1,
}};
const client=new Client("private-ui-test-token");
const render=(available=true)=>renderToStaticMarkup(<ManagedResultDelivery client={client} connection={4} entry={entry} available={available}/>);
beforeEach(()=>{state.value={...prepared,delivery:null};state.error=false;state.fetching=false;state.options=[];});

it("offers explicit preparation without performing mutation while rendering",()=>{
  const prepare=vi.spyOn(client,"prepareOwnerManagedResultDelivery"),confirm=vi.spyOn(client,"confirmOwnerManagedResultDelivery"),refresh=vi.spyOn(client,"refreshOwnerManagedResultRetirement");
  expect(render()).toContain("Prepare storage release");
  expect(prepare).not.toHaveBeenCalled();expect(confirm).not.toHaveBeenCalled();expect(refresh).not.toHaveBeenCalled();
  expect(state.options[0]).toMatchObject({retry:false,refetchOnWindowFocus:false});
  expect(JSON.stringify(state.options[0].queryKey)).not.toContain(client.token);
  prepare.mockRestore();confirm.mockRestore();refresh.mockRestore();
});
it("displays the original prepared hash and offers only same-intent reconciliation after a sent confirmation",()=>{
  const value=structuredClone(prepared);state.value=value;
  let html=render();expect(html).toContain("Confirm receipt and release runner storage");expect(html).toContain(value.delivery!.request_sha256);
  value.delivery!.state="committed";html=render();expect(html).toContain("Reconcile original confirmation");
  expect(html).not.toContain("Prepare storage release");expect(html).not.toContain("Physical storage release verified");
});
it("keeps accepted delivery separate from verified retirement",()=>{
  const value=structuredClone(prepared);value.delivery!.state="accepted";value.delivery!.delivery_receipt_sha256="c".repeat(64);state.value=value;
  let html=render();expect(html).toContain("Physical storage release is pending");expect(html).toContain("Check release status");
  expect(html).not.toContain("Confirm receipt and release runner storage");
  value.delivery!.state="unconfirmed";expect(render()).toContain("Physical storage release is currently unconfirmed");
  value.delivery!.state="retired";html=render();expect(html).toContain("Physical storage release verified");expect(html).not.toContain("Check release status");
});
it("hides cached mutation actions during failed or pending verification and on version mismatch",()=>{
  state.value=structuredClone(prepared);state.fetching=true;expect(render()).not.toContain("Confirm receipt and release runner storage");
  state.fetching=false;state.error=true;expect(render()).not.toContain("Confirm receipt and release runner storage");
  state.error=false;(state.value as OwnerManagedResultDeliveryView).delivery!.result_version_id="foreign";
  const html=render();expect(html).toContain("Confirmation history could not be verified");expect(html).not.toContain("Confirm receipt and release runner storage");
});
it("requires currently readable local bytes before preparing or confirming",()=>{
  let html=render(false);expect(html).toContain('disabled=""');
  state.value=structuredClone(prepared);html=render(false);expect(html).toContain('disabled=""');
  expect(html).toContain("Your local result version is retained");
});
it("offers an independent original-ACK lookup for committed history without content rights",()=>{
  const value=structuredClone(prepared);value.delivery!.state="committed";state.value=value;
  const html=renderToStaticMarkup(<ManagedResultDelivery client={client} connection={4} entry={{...entry,version:null}} available={false} historyOnly/>);
  expect(html).toContain("Check original confirmation receipt");
  expect(html).toContain("Reconcile original confirmation");
  expect(html).toContain('disabled=""');expect(html).not.toContain("Prepare storage release");
});
