import {type Client,type ProviderProfile} from "@cowork/client";
import {effectiveAgent,effectivePersonality,resolveAgentModel,type AgentTemplate,type Personality} from "./libraryApi";
import {productRequest,type Skill,type Knowledge} from "./productApi";
/** Copies the library record once. Subsequent source changes cannot edit this snapshot. */
export async function copyNativeScheduleLibrary(client:Client,project:string,order:string,item:AgentTemplate|Personality,providers:ProviderProfile[]){
  const isAgent="configuration" in item.value,agent=isAgent?item as AgentTemplate:null,personality=isAgent?null:item as Personality;
  const model=agent?resolveAgentModel(agent):personality!.value.model;
  if(model.kind==="codex")throw new Error("schedule_codex_unavailable");
  const provider=model.kind==="provider"?providers.find(p=>p.id===model.provider_id&&p.revision===model.provider_revision):undefined;
  if(model.kind==="provider"&&!provider)throw new Error("schedule_provider_revision_unavailable");
  const config=structuredClone(agent?effectiveAgent(agent):effectivePersonality(personality!));
  const c=agent?.value.configuration;
  if(c&&(c.skill_ids.length||c.memory_ids.length)){
    const [skills,knowledge]=await Promise.all([
      productRequest<Skill[]>(client,`/projects/${encodeURIComponent(project)}/skills`),
      productRequest<{document:Knowledge}[]>(client,`/projects/${encodeURIComponent(project)}/knowledge?query=`)]);
    if(c.skill_ids.some(id=>!skills.some(skill=>skill.id===id&&skill.enabled)))throw new Error("schedule_skill_unavailable");
    for(const id of c.memory_ids){const document=knowledge.find(hit=>hit.document.id===id)?.document;if(!document)throw new Error("schedule_memory_unavailable");const memory=document.kind==="memory"?document.memory:undefined;if(memory&&(!memory.enabled||memory.scope.kind==="chat"&&memory.scope.order_id!==order||memory.scope.kind==="agent"&&memory.scope.agent_id!==agent!.id))throw new Error("schedule_memory_unavailable");}
  }
  if(c?.mcp_tools.length){const servers=await client.mcpServers(project);if(c.mcp_tools.some(tool=>!servers.some(server=>server.id===tool.server_id&&server.revision===tool.server_revision&&server.configuration.enabled&&server.configuration.selected_tools.includes(tool.tool_name))))throw new Error("schedule_mcp_revision_unavailable");}
  return {config,provider,configuration:c?structuredClone(c):undefined};
}
