import {afterEach,expect,it,vi} from "vitest";
import {Client,type OwnerManagedResultDeliveryView} from "./index";

const id="b7f5fe42-28ed-4bb7-bf65-e9870a5987f6",version="f793719f-4f4d-4fe3-a137-30a0bd2c8cda";
const prepared:OwnerManagedResultDeliveryView={order_id:"order",run_id:"run",process_attempt_id:"attempt",delivery:{
  id,result_version_id:version,request_sha256:"a".repeat(64),manifest_sha256:"b".repeat(64),state:"prepared",
  created_at:1793510000000,accept_before:1793510030000,revision:1,accepted_at:null,delivery_receipt_sha256:null,
  retirement_revision:null,retirement_recorded_at:null,retirement_receipt_sha256:null,
}};
const confirmation={id,request_sha256:"a".repeat(64)};
function accepted():OwnerManagedResultDeliveryView {
  const value=structuredClone(prepared);Object.assign(value.delivery!,{state:"accepted",revision:3,
    accepted_at:1793510001000,delivery_receipt_sha256:"c".repeat(64)});return value;
}
const json=(value:unknown)=>new Response(JSON.stringify(value),{headers:{"content-type":"application/json"}});
afterEach(()=>{vi.unstubAllGlobals();vi.restoreAllMocks();});

it("reads local history and prepares separately without sending a confirmation",async()=>{
  const fetch=vi.fn().mockResolvedValueOnce(json({...prepared,delivery:null})).mockResolvedValueOnce(json(prepared));vi.stubGlobal("fetch",fetch);
  const client=new Client("owner-test-token","https://owner.invalid");
  expect((await client.ownerManagedResultDelivery("order","run","attempt")).delivery).toBeNull();
  expect(await client.prepareOwnerManagedResultDelivery("order","run","attempt")).toEqual(prepared);
  expect(fetch.mock.calls.map(call=>[call[0],call[1].method])).toEqual([
    ["https://owner.invalid/api/v2/orders/order/runs/run/managed-results/attempt/delivery","GET"],
    ["https://owner.invalid/api/v2/orders/order/runs/run/managed-results/attempt/delivery/prepare","POST"],
  ]);
  for(const [,options] of fetch.mock.calls)expect(options).toMatchObject({cache:"no-store",redirect:"error",headers:{Authorization:"Bearer owner-test-token"}});
});
it("confirms exactly the displayed intent once and keeps status refresh separate",async()=>{
  const fetch=vi.fn().mockResolvedValueOnce(json(accepted())).mockResolvedValueOnce(json(accepted()));vi.stubGlobal("fetch",fetch);
  const client=new Client("owner");
  await client.confirmOwnerManagedResultDelivery("order","run","attempt",confirmation);
  await client.refreshOwnerManagedResultRetirement("order","run","attempt");
  expect(fetch).toHaveBeenCalledTimes(2);
  expect(JSON.parse(fetch.mock.calls[0][1].body)).toEqual(confirmation);
  expect(fetch.mock.calls[1][0]).toContain("/delivery/retirement/refresh");
  expect(fetch.mock.calls[1][1].body).toBeUndefined();
});
it("retains uncertainty after a lost mutation response without retrying or preparing another intent",async()=>{
  const fetch=vi.fn().mockRejectedValue(new TypeError("lost reply"));vi.stubGlobal("fetch",fetch);
  await expect(new Client("owner").confirmOwnerManagedResultDelivery("order","run","attempt",confirmation)).rejects.toThrow("lost reply");
  expect(fetch).toHaveBeenCalledTimes(1);expect(JSON.parse(fetch.mock.calls[0][1].body)).toEqual(confirmation);
});
it("can refresh committed history without submitting the confirmation payload again",async()=>{
  const value=structuredClone(prepared);value.delivery!.state="committed";
  const fetch=vi.fn().mockResolvedValue(json(value));vi.stubGlobal("fetch",fetch);
  expect(await new Client("owner").refreshOwnerManagedResultRetirement("order","run","attempt")).toEqual(value);
  expect(fetch).toHaveBeenCalledTimes(1);expect(fetch.mock.calls[0][0]).toContain("/delivery/retirement/refresh");
  expect(fetch.mock.calls[0][1].body).toBeUndefined();
});
it("rejects renderer-authored grants and malformed intent identifiers before transport",async()=>{
  const fetch=vi.fn();vi.stubGlobal("fetch",fetch);const client=new Client("owner");
  for(const bad of [{...confirmation,id:".."},{...confirmation,request_sha256:"invalid"},
    {...confirmation,native_custody:{}},{...confirmation,accepted_at:1}]) {
    await expect(client.confirmOwnerManagedResultDelivery("order","run","attempt",bad)).rejects.toThrow();
  }
  expect(fetch).not.toHaveBeenCalled();
});
it.each(["order","run","attempt","id","request","acceptance","retirement","timestamp"])("rejects a mismatched or inconsistent %s in a successful confirmation",async changed=>{
  const value=accepted();
  if(changed==="order")value.order_id="foreign";
  if(changed==="run")value.run_id="foreign";
  if(changed==="attempt")value.process_attempt_id="foreign";
  if(changed==="id")value.delivery!.id="1eaaf6f3-1bb2-4d93-b5c7-ec7a457fbf72";
  if(changed==="request")value.delivery!.request_sha256="0".repeat(64);
  if(changed==="acceptance")value.delivery!.accepted_at=null;
  if(changed==="retirement")value.delivery!.state="retired";
  if(changed==="timestamp"){value.delivery!.created_at=8_640_000_000_000_001;value.delivery!.accept_before=8_640_000_000_030_001;}
  vi.stubGlobal("fetch",vi.fn().mockResolvedValue(json(value)));
  await expect(new Client("owner").confirmOwnerManagedResultDelivery("order","run","attempt",confirmation)).rejects.toThrow("managed_result_delivery_integrity");
});
