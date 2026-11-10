import {decode,RequestError,type Client,type ContractTypes} from "./index";

type View=ContractTypes["OwnerManagedResultDeliveryView"];
type Confirmation=ContractTypes["ConfirmOwnerManagedResultDelivery"];
const sha=/^[a-f0-9]{64}$/;
const uuid=/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/;
const integer=(value:number)=>Number.isSafeInteger(value)&&value>0;
const invalid=()=>new Error("managed_result_delivery_integrity");
const scope=(order:string,run:string,attempt:string)=>`/orders/${encodeURIComponent(order)}/runs/${encodeURIComponent(run)}/managed-results/${encodeURIComponent(attempt)}/delivery`;

async function call(client:Client,path:string,method="GET",body?:Confirmation):Promise<unknown> {
  const response=await fetch(client.base+"/api/v2"+path,{method,cache:"no-store",redirect:"error",
    headers:{Authorization:"Bearer "+client.token,...(body?{"Content-Type":"application/json"}:{})},
    body:body?JSON.stringify(body):undefined});
  const value:unknown=await response.json();
  if(!response.ok)throw new RequestError(decode("ApiError",value),response.status);
  return value;
}

function checked(value:unknown,order:string,run:string,attempt:string):View {
  const view=decode("OwnerManagedResultDeliveryView",value),d=view.delivery;
  if(view.order_id!==order||view.run_id!==run||view.process_attempt_id!==attempt)throw invalid();
  if(!d)return view;
  if(!uuid.test(d.id)||!uuid.test(d.result_version_id)||!sha.test(d.request_sha256)||!sha.test(d.manifest_sha256)
    ||!integer(d.revision)||!integer(d.created_at)||!integer(d.accept_before)||d.accept_before>8_640_000_000_000_000
    ||d.accept_before<=d.created_at||d.accept_before-d.created_at>30000)throw invalid();
  const accepted=["accepted","retired","unconfirmed"].includes(d.state);
  if(accepted!==!!d.delivery_receipt_sha256||accepted!==(d.accepted_at!=null))throw invalid();
  if(d.delivery_receipt_sha256&&(!sha.test(d.delivery_receipt_sha256)||!integer(d.accepted_at!)
    ||d.accepted_at!<d.created_at||d.accepted_at!>=d.accept_before))throw invalid();
  const retirement=d.retirement_revision!=null;
  if(retirement!==(d.retirement_recorded_at!=null)||retirement!==!!d.retirement_receipt_sha256)throw invalid();
  if(retirement&&(!accepted||!integer(d.retirement_revision!)||!integer(d.retirement_recorded_at!)
    ||d.retirement_recorded_at!<d.accepted_at!||!sha.test(d.retirement_receipt_sha256!)))throw invalid();
  if(["retired","unconfirmed"].includes(d.state)&&(!retirement||d.retirement_revision!<=1))throw invalid();
  if(d.state==="accepted"&&retirement&&d.retirement_revision!==1)throw invalid();
  return view;
}
export async function ownerManagedResultDelivery(client:Client,order:string,run:string,attempt:string):Promise<View> {
  return checked(await call(client,scope(order,run,attempt)),order,run,attempt);
}
export async function prepareOwnerManagedResultDelivery(client:Client,order:string,run:string,attempt:string):Promise<View> {
  const view=checked(await call(client,scope(order,run,attempt)+"/prepare","POST"),order,run,attempt);
  if(!view.delivery)throw invalid();
  return view;
}
export async function confirmOwnerManagedResultDelivery(client:Client,order:string,run:string,attempt:string,input:Confirmation):Promise<View> {
  decode("ConfirmOwnerManagedResultDelivery",input);
  if(!uuid.test(input.id)||!sha.test(input.request_sha256))throw invalid();
  // One POST, with exactly the displayed original ID/hash. Failures are left
  // to the caller for an explicit history check and user-chosen same-intent retry.
  const view=checked(await call(client,scope(order,run,attempt)+"/confirm","POST",input),order,run,attempt);
  if(view.delivery?.id!==input.id||view.delivery.request_sha256!==input.request_sha256)throw invalid();
  if(["prepared","committed"].includes(view.delivery.state))throw new Error("managed_result_delivery_unconfirmed");
  return view;
}
export async function refreshOwnerManagedResultRetirement(client:Client,order:string,run:string,attempt:string):Promise<View> {
  return checked(await call(client,scope(order,run,attempt)+"/retirement/refresh","POST"),order,run,attempt);
}
