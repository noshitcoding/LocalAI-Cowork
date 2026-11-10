import {afterEach,describe,expect,it,vi} from "vitest";
import {Client,type AutomationEvent} from "@cowork/client";
import {PersistentRequestLatch} from "./requestJournal";
import {eventFieldData,performAutomationMutation,restoreAutomationMutation,webhookGrant,mergeAutomationEvents,type AutomationMutation} from "./automationApi";
afterEach(()=>vi.unstubAllGlobals());
describe("automation event handling",()=>{
  it("keeps the current event state when polling overlaps a previously loaded history page",()=>{
    const old:AutomationEvent={id:"event",project_id:"project",trigger_id:"trigger",trigger_revision:1,task_id:"task",task_revision:1,event_id:"original-id",source:"manual",data:{},state:"queued",run_id:null,reason:null,received_at:1,updated_at:1};
    const current:AutomationEvent={...old,state:"started",run_id:"actual-run",updated_at:10};
    const earlier={...old,id:"older"};
    expect(mergeAutomationEvents([current],[old,earlier,earlier])).toEqual([current,earlier]);
    expect(old.state).toBe("queued");
  });
  it("preserves the exact event across lost acknowledgement and browser reload",async()=>{
    const memory=new Map<string,string>(),storage={getItem:(key:string)=>memory.get(key)??null,setItem:(key:string,value:string)=>{memory.set(key,value);},removeItem:(key:string)=>{memory.delete(key);}};
    const original:AutomationMutation={kind:"submit",trigger:"trigger",body:{event_id:"original-id",data:{amount:42}}};
    const first=new PersistentRequestLatch<AutomationMutation>("scope",restoreAutomationMutation,storage);
    const request=first.prepare(()=>original);
    const fetch=vi.fn().mockRejectedValueOnce(new Error("lost acknowledgement"));vi.stubGlobal("fetch",fetch);
    const client=new Client("owner","https://cowork.example");
    await expect(performAutomationMutation(client,"project",request)).rejects.toThrow("lost acknowledgement");
    const second=new PersistentRequestLatch<AutomationMutation>("scope",restoreAutomationMutation,storage);
    const restored=second.prepare(()=>{throw new Error("must not create another event");});
    const receipt:AutomationEvent={id:"event",project_id:"project",trigger_id:"trigger",trigger_revision:1,task_id:"task",task_revision:1,event_id:"original-id",source:"manual",data:{amount:42},state:"queued",run_id:null,reason:null,received_at:1,updated_at:1};
    fetch.mockResolvedValueOnce(new Response(JSON.stringify(receipt)));
    await expect(performAutomationMutation(client,"project",restored)).resolves.toEqual(receipt);
    expect(fetch.mock.calls[0][1].body).toBe(fetch.mock.calls[1][1].body);
    expect(fetch.mock.calls[1][0]).toBe("https://cowork.example/api/v2/projects/project/automation-triggers/trigger/events");
    second.confirm();expect(memory.size).toBe(0);
  });
  it("rejects injected journal fields and never accepts webhook credentials into it",()=>{
    expect(()=>restoreAutomationMutation({kind:"submit",trigger:"t",token:"secret",body:{event_id:"e",data:{}}})).toThrow();
    expect(()=>restoreAutomationMutation({kind:"submit",trigger:"",body:{event_id:"e",data:{}}})).toThrow();
    expect(()=>restoreAutomationMutation({kind:"save",body:{id:null,expected_revision:0,idempotency_key:"key",name:"n",task_id:"t",task_revision:1,source:"unknown",predicates:[],enabled:true}})).toThrow();
  });
  it("preserves field types, rejects duplicate names and handles prototype-shaped data safely",()=>{
    const data=eventFieldData([{name:"amount",kind:"number",value:"42.5"},{name:"approved",kind:"boolean",value:"false"},{name:"__proto__",kind:"text",value:"ordinary content"}]);
    expect(data.amount).toBe(42.5);expect(data.approved).toBe(false);expect(Object.getPrototypeOf(data)).toBe(Object.prototype);
    expect(Object.hasOwn(data,"__proto__")).toBe(true);
    expect(()=>eventFieldData([{name:"x",kind:"number",value:""}])).toThrow();
    expect(()=>eventFieldData([{name:"x",kind:"boolean",value:"yes"}])).toThrow();
    expect(()=>eventFieldData([{name:"x",kind:"text",value:"a"},{name:" x ",kind:"text",value:"b"}])).toThrow();
  });
  it("loads only secret-free webhook metadata with owner authentication",async()=>{
    const fetch=vi.fn().mockResolvedValue(new Response(JSON.stringify({trigger_id:"trigger",revision:2,enabled:true,expires_at:50})));vi.stubGlobal("fetch",fetch);
    await expect(webhookGrant(new Client("owner"),"project","trigger")).resolves.toMatchObject({revision:2});
    expect(fetch.mock.calls[0][1].headers.Authorization).toBe("Bearer owner");
    expect(fetch.mock.calls[0][0]).not.toContain("owner");
  });
});
