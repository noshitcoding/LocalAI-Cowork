import {afterEach,describe,expect,it,vi} from "vitest";
import {Client} from "@cowork/client";
import {emptyAgentTemplate,emptyPersonality,type AgentTemplate,type Personality} from "./libraryApi";
import {copyNativeScheduleLibrary} from "./nativeScheduleLibrary";
afterEach(()=>vi.unstubAllGlobals());
const provider={id:"provider",revision:4,name:"Test",kind:"test" as const,endpoint:"",model:"test",context_tokens:32768,credential_ready:true,credential_required:false};
describe("explicit schedule library copies",()=>{
 it("copies a personality and embedded agent configuration independently, retaining exact source revisions",async()=>{
  const personality:Personality={id:"personality",revision:2,created_at:1,updated_at:1,value:{...emptyPersonality(),name:"Reviewer",system_prompt:"Original persona",model:{kind:"provider",provider_id:provider.id,provider_revision:provider.revision},generation:{temperature:0}}};
  const agent:AgentTemplate={id:"agent",revision:3,created_at:1,updated_at:1,value:{...emptyAgentTemplate(),personality,configuration:{...emptyAgentTemplate().configuration,instructions:"Original agent",native_tools:["browser"],generation:{timeout_ms:4000}}}};
  const copied=await copyNativeScheduleLibrary(new Client("OWNER"),"project","chat",agent,[provider]);
  expect(copied.provider).toBe(provider);expect(copied.config.generation).toEqual({temperature:0,timeout_ms:4000});expect(copied.config.agent_source).toEqual({id:"agent",revision:3});
  agent.value.configuration.instructions="Later source edit";personality.value.system_prompt="Later persona edit";agent.value.configuration.native_tools.push("office");
  expect(copied.config.instructions).toContain("Original persona");expect(copied.config.instructions).not.toContain("Later");expect(copied.configuration?.native_tools).toEqual(["browser"]);
  await expect(copyNativeScheduleLibrary(new Client("OWNER"),"project","chat",personality,[{...provider,revision:5}])).rejects.toThrow("schedule_provider_revision_unavailable");
 });
 it("rejects absent project skill references before copying an agent",async()=>{
  vi.stubGlobal("fetch",vi.fn().mockImplementation(async()=>new Response("[]")));
  const agent:AgentTemplate={id:"agent",revision:1,created_at:1,updated_at:1,value:{...emptyAgentTemplate(),configuration:{...emptyAgentTemplate().configuration,skill_ids:["missing"]}}};
  await expect(copyNativeScheduleLibrary(new Client("OWNER"),"project","chat",agent,[provider])).rejects.toThrow("schedule_skill_unavailable");
 });
});
