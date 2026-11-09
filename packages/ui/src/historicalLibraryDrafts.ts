import {decode,defaultRunBudgets,type ProviderProfile,type WorkflowDefinition} from "@cowork/client";
import type {ImportedConfiguration,MigrationWorkflow} from "./v1Migration";
import {emptyAgentTemplate,emptyPersonality,validateAgentTemplate,validatePersonality,type LibraryDefinition,type LibraryKind} from "./libraryApi";
import {validWorkflowEditor} from "./workflowUi";
import {convertV1Pipeline,readV1Pipelines} from "./workflowStarters";
const text=(value:string,max:number)=>{if(typeof value!=="string"||!value.trim()||value.includes("\0")||new TextEncoder().encode(value).length>max)throw Error("historical_draft_bounds");return value;};
function currentModel(provider:ProviderProfile){if(!provider.id||provider.revision<1||!provider.credential_ready)throw Error("historical_current_model_required");return {kind:"provider" as const,provider_id:provider.id,provider_revision:provider.revision};}
export function historicalLibraryDraft(configuration:ImportedConfiguration,kind:LibraryKind,provider:ProviderProfile,agentId?:string):LibraryDefinition{
 const source=configuration.source,description=`${source.description}\nV1 ${source.source_id} · r${source.source_revision}`;
 if(kind==="personalities")return validatePersonality({...emptyPersonality(),name:text(source.name,200),description,system_prompt:text(source.instructions,64000),model:currentModel(provider),is_default:false});
 const agent=source.workflow?.agents.find(agent=>agent.id===agentId);if(!agent)throw Error("historical_agent_selection_required");
 return validateAgentTemplate({...emptyAgentTemplate(),name:text(`${source.name} · ${agent.id}`,200),description,configuration:{...emptyAgentTemplate().configuration,instructions:text(agent.instructions,64000),model:currentModel(provider),access_mode:"confirm_mutations",native_tools:[],mcp_tools:[],skill_ids:[],memory_ids:[],allow_delegation:false}});
}
export function historicalWorkflowDraft(configuration:ImportedConfiguration,workflow:MigrationWorkflow,provider:ProviderProfile):WorkflowDefinition{
 if(configuration.source.source_id.startsWith("rpc:")&&configuration.source.instructions.trim().startsWith("{"))return convertV1Pipeline(readV1Pipelines(configuration.source.instructions)[0],{id:provider.id,revision:provider.revision},[provider]);
 currentModel(provider);const rights=()=>({tools:[],folders:[]});const budgets={...defaultRunBudgets,max_output_tokens:Math.min(defaultRunBudgets.max_output_tokens,Math.floor(provider.context_tokens/4)),max_total_tokens:Math.max(defaultRunBudgets.max_total_tokens,provider.context_tokens)};
 if(!workflow||!Array.isArray(workflow.agents)||!Array.isArray(workflow.steps)||!workflow.agents.length||workflow.agents.length>32||!workflow.steps.length||workflow.steps.length>63)throw Error("historical_draft_bounds");
 const roles=workflow.agents.map(agent=>({id:agent.id,instructions:text(agent.instructions,256000),provider_id:provider.id,provider_revision:provider.revision,rights:rights(),budgets:{...budgets},delegate_to:[]}));
 const steps=workflow.steps.map(step=>({id:step.id,role_id:step.agent_id,goal:text(step.prompt,16000),dependencies:[...step.depends_on],rights:rights(),budgets:{...budgets}}));let mode:WorkflowDefinition["mode"];
 if(workflow.mode==="hierarchical"){
  if(!roles.some(role=>role.id===workflow.manager_agent_id))throw Error("historical_manager_required");let review="manager-review";while(steps.some(step=>step.id===review))review+="-final";
  steps.push({id:review,role_id:workflow.manager_agent_id!,goal:"Review the completed worker results against the original objective and return the final verified synthesis.",dependencies:steps.map(step=>step.id),rights:rights(),budgets:{...budgets}});mode={mode:"hierarchical",max_parallel:4,max_depth:1,max_delegations:1,manager_step_id:review};
 }else mode=workflow.mode==="parallel"?{mode:"parallel",max_parallel:4}:{mode:"sequential"};
 const result=decode("WorkflowDefinition",{format_version:1,name:text(configuration.source.name,200),mode,failure:"stop",rights:rights(),budgets,roles,steps});if(!validWorkflowEditor(result))throw Error("historical_draft_bounds");return result;
}
