import {describe,expect,it} from "vitest";
import {defaultRunBudgets} from "@cowork/client";
import {copiedActorSelection,validateActorRunConfiguration,type ActorModelProfile} from "./ActorAgentConfigurationPanel";
import {emptyAgentTemplate,emptyPersonality,mcpLibraryAlias,type AgentTemplate,type LibraryPreferences,type Personality} from "./libraryApi";
const provider:ActorModelProfile={id:"10000000-0000-4000-8000-000000000001",revision:2,name:"Private model",model_defaults:{model:"fixture",context_tokens:32768,auth_mode:"none"}};
const preferences:LibraryPreferences={revision:1,updated_at:1,value:{system_prompt:"Actor-specific defaults",default_access_mode:"confirm_mutations",default_budgets:{...defaultRunBudgets}}};
function records(){
  const personality:Personality={id:"10000000-0000-4000-8000-000000000002",revision:3,created_at:1,updated_at:1,value:{...emptyPersonality(),name:"Writer",system_prompt:"Use clear words.",generation:{temperature:0.2}}};
  const value=emptyAgentTemplate();value.name="Research";value.personality=personality;value.configuration.instructions="Inspect primary evidence.";value.configuration.model={kind:"provider",provider_id:provider.id,provider_revision:2};value.configuration.generation={temperature:0.4};value.configuration.budgets.max_steps=9;
  value.configuration.mcp_tools=[{server_id:"10000000-0000-4000-8000-000000000003",server_revision:4,tool_name:"search"}];
  value.configuration.skill_ids=["10000000-0000-4000-8000-000000000004"];
  const agent:AgentTemplate={id:"10000000-0000-4000-8000-000000000005",revision:7,created_at:1,updated_at:1,value};return {agent,personality};
}
describe("private actor agent copies",()=>{
  it("freezes private defaults, model revision, provenance and exact tool/context selections",async()=>{
    const {agent,personality}=records();const copied=await copiedActorSelection(preferences,agent,personality,[provider]);
    expect(copied.model).toEqual({kind:"provider",provider_id:provider.id,provider_revision:2});
    expect(copied.selection.run_configuration.agent_config.instructions).toContain("Actor-specific defaults");expect(copied.selection.run_configuration.agent_config.instructions).toContain("Use clear words.");
    expect(copied.selection.run_configuration.agent_config.generation.temperature).toBe(0.4);expect(copied.selection.run_configuration.budgets.max_steps).toBe(9);
    const tool=agent.value.configuration.mcp_tools[0];expect(copied.selection.expected_mcp_revisions).toEqual({[tool.server_id]:4});expect(copied.selection.run_configuration.agent_config.tool_filter).toContain(await mcpLibraryAlias(tool.server_id,"search"));
    expect(copied.selection.run_configuration.agent_config.tool_filter).not.toContain("execute_program");
    expect(copied.selection.run_configuration.agent_config.agent_source).toEqual({id:agent.id,revision:7});
    agent.value.configuration.instructions="Changed after selection";personality.value.system_prompt="Changed source";agent.value.configuration.skill_ids.length=0;
    expect(copied.selection.run_configuration.agent_config.instructions).toContain("Inspect primary evidence.");expect(copied.selection.run_configuration.agent_config.instructions).not.toContain("Changed");expect(copied.selection.skill_ids).toHaveLength(1);
  });
  it("blocks stale provider references and preserves Codex choices for actual scoped materialization",async()=>{
    const {agent,personality}=records();await expect(copiedActorSelection(preferences,agent,personality,[{...provider,revision:3}])).rejects.toThrow("private_agent_provider_changed");
    agent.value.configuration.model={kind:"codex",account_id:"10000000-0000-4000-8000-000000000006",model:"fixture-model",reasoning_effort:"low"};const copied=await copiedActorSelection(preferences,agent,personality,[provider]);expect(copied.model).toEqual(agent.value.configuration.model);expect(copied.selection.run_configuration.agent_config.agent_source).toEqual({id:agent.id,revision:agent.revision});
  });
  it("rejects mixed MCP revisions and injected configuration fields before journaling",async()=>{
    const {agent,personality}=records();const tool=agent.value.configuration.mcp_tools[0];agent.value.configuration.mcp_tools.push({...tool,server_revision:5,tool_name:"write"});await expect(copiedActorSelection(preferences,agent,personality,[provider])).rejects.toThrow("private_agent_mcp_revision_conflict");
    expect(()=>validateActorRunConfiguration({agent_config:{instructions:"",generation:{},api_key:"secret"},access_mode:"confirm_all",budgets:defaultRunBudgets})).toThrow("private_agent_configuration_invalid");
  });
});
