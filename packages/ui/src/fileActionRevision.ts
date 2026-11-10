import {RequestError,type Client,type Order} from "@cowork/client";

/** Native actions can advance a chat without changing its saved composer draft. */
export async function refreshSavedOrder(client:Pick<Client,"order">,known:Order):Promise<Order>{
  const current=await client.order(known.id);
  if(current.id!==known.id||current.project_id!==known.project_id||current.revision<known.revision||current.draft!==known.draft){
    throw new RequestError({code:"revision_conflict",message:"The saved chat draft changed; review it before retrying",correlation_id:crypto.randomUUID(),retryable:false},409);
  }
  return current;
}

/** The caller serializes this whole read/CAS operation with its other saves. */
export async function saveCurrentDraft(client:Pick<Client,"order"|"saveDraft">,known:Order,value:string):Promise<Order>{
  const current=await refreshSavedOrder(client,known);
  return value===known.draft?current:client.saveDraft(known.id,{base_revision:current.revision,draft:value});
}
