import {decode,type Client,type AutomationEvent,type WriteAutomationTrigger,type SubmitAutomationEvent,type RetireAutomationEvent} from "@cowork/client";
import {RequestError} from "@cowork/client";

export const automationPath=(project:string)=>`/projects/${encodeURIComponent(project)}/automation-triggers`;
export const automationTriggers=(client:Client,project:string)=>client.many(automationPath(project),"AutomationTrigger");
export const saveAutomationTrigger=(client:Client,project:string,body:WriteAutomationTrigger)=>client.one(automationPath(project),"AutomationTrigger","POST",body);
export const automationEvents=(client:Client,project:string,trigger:string,before?:number)=>client.one(`${automationPath(project)}/${encodeURIComponent(trigger)}/events?limit=50${before?`&before=${before}`:""}`,"AutomationEventPage");
export const previewAutomationEvent=(client:Client,project:string,trigger:string,body:SubmitAutomationEvent)=>client.one(`${automationPath(project)}/${encodeURIComponent(trigger)}/preview`,"AutomationEventPreview","POST",body);
export const submitAutomationEvent=(client:Client,project:string,trigger:string,body:SubmitAutomationEvent)=>client.one(`${automationPath(project)}/${encodeURIComponent(trigger)}/events`,"AutomationEvent","POST",body);
export const retireAutomationEvent=(client:Client,project:string,event:string,body:RetireAutomationEvent)=>client.one(`/projects/${encodeURIComponent(project)}/automation-events/${encodeURIComponent(event)}/retire`,"AutomationEvent","POST",body);
export async function webhookGrant(client:Client,project:string,trigger:string){
  const response=await fetch(`${client.base}/api/v2${automationPath(project)}/${encodeURIComponent(trigger)}/webhook`,{headers:{Authorization:`Bearer ${client.token}`}});
  const value:unknown=await response.json();
  if(!response.ok)throw new RequestError(decode("ApiError",value),response.status);
  return value===null?null:decode("AutomationWebhookGrant",value);
}
export type AutomationMutation=
  |{kind:"save";body:WriteAutomationTrigger}
  |{kind:"submit";trigger:string;body:SubmitAutomationEvent}
  |{kind:"retire";event:string;body:RetireAutomationEvent};
export function restoreAutomationMutation(value:unknown):AutomationMutation{
  if(!value||typeof value!=="object"||Array.isArray(value))throw new Error("automation_journal_invalid");
  const input=value as Record<string,unknown>;
  if(Object.keys(input).some(key=>!["kind","body","trigger","event"].includes(key)))throw new Error("automation_journal_invalid");
  if(input.kind==="save")return {kind:"save",body:decode("WriteAutomationTrigger",input.body)};
  if(input.kind==="submit"&&typeof input.trigger==="string"&&input.trigger)return {kind:"submit",trigger:input.trigger,body:decode("SubmitAutomationEvent",input.body)};
  if(input.kind==="retire"&&typeof input.event==="string"&&input.event)return {kind:"retire",event:input.event,body:decode("RetireAutomationEvent",input.body)};
  throw new Error("automation_journal_invalid");
}
export async function performAutomationMutation(client:Client,project:string,input:AutomationMutation){
  const original=restoreAutomationMutation(input);
  if(original.kind==="save")return saveAutomationTrigger(client,project,original.body);
  if(original.kind==="submit")return submitAutomationEvent(client,project,original.trigger,original.body);
  return retireAutomationEvent(client,project,original.event,original.body);
}
export function eventCanRetire(event:AutomationEvent){return event.state==="queued"||event.state==="blocked";}
export function mergeAutomationEvents(current:AutomationEvent[],older:AutomationEvent[]):AutomationEvent[]{
  const seen=new Set<string>();
  return [...current,...older].filter(event=>{if(seen.has(event.id))return false;seen.add(event.id);return true;});
}

export type EventField={name:string;kind:"text"|"number"|"boolean";value:string};
export function eventFieldData(fields:EventField[]):Record<string,unknown>{
  if(fields.length>32)throw new Error("automation_fields_limit");
  const names=new Set<string>();
  return Object.fromEntries(fields.map(field=>{
    const name=field.name.trim();
    if(!name||name.length>128||names.has(name))throw new Error("automation_field_name_invalid");
    names.add(name);
    let value:unknown=field.value;
    if(field.kind==="number"){
      if(!field.value.trim()||!Number.isFinite(Number(field.value)))throw new Error("automation_field_number_invalid");
      value=Number(field.value);
    }else if(field.kind==="boolean"){
      if(!["true","false"].includes(field.value))throw new Error("automation_field_boolean_invalid");
      value=field.value==="true";
    }
    return [name,value];
  }));
}
