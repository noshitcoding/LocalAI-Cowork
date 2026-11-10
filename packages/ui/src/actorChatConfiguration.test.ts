import {describe,expect,it,vi,afterEach} from "vitest";
import {defaultRunBudgets} from "@cowork/client";
import {ActorChatConfigurationLatch,actorChatConfiguration,type ActorChatWrite} from "./actorChatConfiguration";
const thread="10000000-0000-4000-8000-000000000001",project="10000000-0000-4000-8000-000000000002",profile="10000000-0000-4000-8000-000000000003";
const write=():ActorChatWrite=>({expected_revision:0,idempotency_key:"10000000-0000-4000-8000-000000000004",value:{model_profile_id:profile,provider_revision:2,run_configuration:{agent_config:{instructions:"Original copied prompt",generation:{},execution_mode:"hybrid",behavior:{command_allow_groups:[["git status"]],command_blacklist:["rm *"],max_tool_calls_per_loop:2,fallback_to_human_on_repeated_failure:true}},access_mode:"confirm_all",budgets:{...defaultRunBudgets}},skill_ids:[],memory_ids:[],mcp_metadata_ids:[],expected_mcp_revisions:{},model_binding:null,output:null}});
afterEach(()=>vi.unstubAllGlobals());
describe("private chat configuration",()=>{
  it("restores the exact copied request across a lost response and reload",()=>{
    const values=new Map<string,string>(),storage={getItem:(k:string)=>values.get(k)??null,setItem:(k:string,v:string)=>{values.set(k,v);},removeItem:(k:string)=>{values.delete(k);}};
    const first=new ActorChatConfigurationLatch("private-chat",storage),request=first.prepare(write);
    request.value.run_configuration.agent_config.instructions="Changed consumer copy";
    const restored=new ActorChatConfigurationLatch("private-chat",storage);const next=restored.prepare(()=>({...write(),expected_revision:9}));
    expect(next.expected_revision).toBe(0);expect(next.value.run_configuration.agent_config.instructions).toBe("Original copied prompt");expect(next.value.run_configuration.agent_config.execution_mode).toBe("hybrid");restored.confirm();expect(values.size).toBe(0);
  });
  it("uses the actual actor session and rejects a response from another chat",async()=>{
    const fetcher=vi.fn(async(_input:RequestInfo|URL,_init?:RequestInit)=>({ok:true,json:async()=>({thread_id:thread,project_id:project,revision:1,updated_at:1,value:write().value})}));vi.stubGlobal("fetch",fetcher);
    await actorChatConfiguration("https://fixture.test","actor-session",project,thread,write());
    expect(fetcher.mock.calls[0]?.[0]).toBe(`https://fixture.test/api/v1/threads/${thread}/configuration`);
    const options=fetcher.mock.calls[0]?.[1] as RequestInit;expect(options.headers).toMatchObject({Authorization:"Bearer actor-session"});expect(options.credentials).toBe("omit");
    fetcher.mockImplementationOnce(async()=>({ok:true,json:async()=>({thread_id:project,project_id:project,revision:1,updated_at:1,value:write().value})}));
    await expect(actorChatConfiguration("https://fixture.test","actor-session",project,thread)).rejects.toThrow("private_chat_configuration_invalid");
  });
  it("rejects unknown settings and injected credentials before durable journaling",()=>{
    const latch=new ActorChatConfigurationLatch();const v=write();(v.value.run_configuration.agent_config as unknown as Record<string,unknown>).api_key="injected";
    expect(()=>latch.prepare(()=>v)).toThrow("private_agent_configuration_invalid");
  });
});
