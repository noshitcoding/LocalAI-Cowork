import {afterEach,describe,expect,it,vi} from "vitest";
import {Client,defaultRunBudgets} from "@cowork/client";
import {NativeScheduleLatch,mutateNativeSchedule,nativeScheduleHistory,validateNativeScheduleWrite,nativeScheduleModel,exactNativeScheduleModel,type WriteNativeSchedule} from "./nativeSchedulesApi";
const write=():WriteNativeSchedule=>({id:null,expected_revision:0,name:"Review",timezone:"Europe/Berlin",recurrence:{kind:"daily",hour:9,minute:0},template:{order_id:"chat",prompt:"Review evidence",provider_id:"profile",provider_revision:1,budgets:defaultRunBudgets},paused:true});
afterEach(()=>vi.unstubAllGlobals());
describe("native schedule original requests",()=>{
 it("preserves the exact save and Idempotency-Key after a lost acknowledgement and reload",async()=>{
  const values=new Map<string,string>(),storage={getItem:(k:string)=>values.get(k)??null,setItem:(k:string,v:string)=>{values.set(k,v);},removeItem:(k:string)=>{values.delete(k);}};
  const original=new NativeScheduleLatch("journal",storage).prepare(()=>({kind:"save",key:"original-key",body:write()}));
  const record={...write(),id:"schedule",project_id:"project",revision:1,next_run:1,active_occurrence:null,last_run_id:null,last_error:null,created_at:1,updated_at:1};delete (record as {expected_revision?:number}).expected_revision;
  const fetch=vi.fn().mockRejectedValueOnce(new TypeError("lost ack")).mockResolvedValueOnce(new Response(JSON.stringify(record)));vi.stubGlobal("fetch",fetch);const client=new Client("SECRET","https://host");
  await expect(mutateNativeSchedule(client,"project",original)).rejects.toThrow("lost ack");
  const reloaded=new NativeScheduleLatch("journal",storage);expect(reloaded.pending).toBe(true);await mutateNativeSchedule(client,"project",reloaded.prepare(()=>{throw new Error("never rebuild");}));
  expect(fetch.mock.calls[0][1].body).toBe(fetch.mock.calls[1][1].body);expect(fetch.mock.calls[1][1].headers["Idempotency-Key"]).toBe("original-key");expect([...values.values()].join()).not.toContain("SECRET");reloaded.confirm();expect(values.size).toBe(0);
 });
 it("rejects credential-shaped or unknown bindings and invalid recurrence rather than storing them",()=>{
  const original=write();expect(validateNativeScheduleWrite(original)).toEqual(original);
  expect(()=>validateNativeScheduleWrite({...original,template:{...original.template,api_key:"secret"}})).toThrow();
  expect(()=>validateNativeScheduleWrite({...original,recurrence:{kind:"weekly",hour:9,minute:0,weekday:8}})).toThrow();
  expect(()=>validateNativeScheduleWrite({...original,recurrence:{kind:"daily",hour:24,minute:0}})).toThrow();
 });
 it("loads history with a read-only request and paginated original cursor",async()=>{
  const fetch=vi.fn().mockResolvedValue(new Response(JSON.stringify({items:[],next_cursor:null})));vi.stubGlobal("fetch",fetch);await nativeScheduleHistory(new Client("OWNER","https://host"),"project","schedule",12);
  expect(fetch.mock.calls[0][0]).toBe("https://host/api/v2/projects/project/schedules/schedule/history?limit=50&before=12");expect(fetch.mock.calls[0][1].method).toBe("GET");expect(fetch.mock.calls[0][1].body).toBeUndefined();
 });
 it("roundtrips copied parent agent snapshots through workflow schedules and the durable exact request",()=>{
  const original=write();const config={instructions:"Frozen reviewer",generation:{temperature:0,timeout_ms:4000},agent_source:{id:"agent",revision:3},tool_filter:[],skill_ids:["skill"],memory_ids:[]};
  original.template.agent_config=structuredClone(config);
  original.template.workflow={kind:"workflow",order_id:"chat",base_revision:1,idempotency_key:"template",prompt:"Review",workflow_id:"workflow",version_id:"version",access_mode:"read_only",budgets:defaultRunBudgets,agent_config:structuredClone(config)};
  expect(validateNativeScheduleWrite(original).template.agent_config).toEqual(config);
  const values=new Map<string,string>(),storage={getItem:(key:string)=>values.get(key)??null,setItem:(key:string,value:string)=>{values.set(key,value);},removeItem:(key:string)=>{values.delete(key);}};
  new NativeScheduleLatch("agent-schedule",storage).prepare(()=>({kind:"save",key:"copy-once",body:original}));
  original.template.agent_config!.instructions="Later editor change";
  const restored=new NativeScheduleLatch("agent-schedule",storage).prepare(()=>{throw new Error("Do not rebuild");});
  expect(restored.kind).toBe("save");if(restored.kind==="save"){expect(restored.body.template.agent_config).toEqual(config);expect(restored.body.template.workflow?.agent_config).toEqual(config);}
  expect(()=>validateNativeScheduleWrite({...original,template:{...original.template,agent_config:{...config,generation:{temperature:9}}}})).toThrow();
 });
 it("matches exact provider revisions and never upgrades a saved schedule implicitly",()=>{
  const provider={id:"provider",revision:4,name:"Test",kind:"test" as const,endpoint:"",model:"test",context_tokens:32768,credential_ready:true,credential_required:false};
  expect(exactNativeScheduleModel(nativeScheduleModel(provider.id,3),[provider])).toBeUndefined();
  expect(exactNativeScheduleModel(nativeScheduleModel(provider.id,4),[provider])).toBe(provider);
 });
});
