export type ActorSkillLearningOutcome={run_id:string;project_id:string;skill_id:string;skill_revision:number;node_ids:string[];recorded_at:number;state:string;reason:string;observed_tools:string[];source_definition:Record<string,unknown>;proposed_prompt_template:string|null};
export function actorSkillLearningHistory(value:unknown,project:string,skill:string):ActorSkillLearningOutcome[]{
    const history=value as {items:ActorSkillLearningOutcome[]};
    if(!history||!Array.isArray(history.items)||history.items.length>20)throw new Error("actor_skill_learning_scope_changed");
    for(const item of history.items){if(!item||item.project_id!==project||item.skill_id!==skill||typeof item.run_id!=="string"||!Number.isSafeInteger(item.skill_revision)||item.skill_revision<1||!Number.isFinite(item.recorded_at)||!["completed","failed","interrupted"].includes(item.state)||typeof item.reason!=="string"||!Array.isArray(item.node_ids)||item.node_ids.some(id=>typeof id!=="string")||!Array.isArray(item.observed_tools)||item.observed_tools.some(name=>typeof name!=="string")||!item.source_definition||typeof item.source_definition.prompt_template!=="string"||item.proposed_prompt_template!==null&&(typeof item.proposed_prompt_template!=="string"||new TextEncoder().encode(item.proposed_prompt_template).length>64000))throw new Error("actor_skill_learning_scope_changed");}
    return history.items;
}
/** Copies content into an editor; never changes the current CAS revision. */
export function actorSkillLearningDraft(outcome:ActorSkillLearningOutcome,current:{id:string;revision:number;project_id:string|null}){
    if(outcome.skill_id!==current.id||current.project_id!==null&&outcome.project_id!==current.project_id||!outcome.proposed_prompt_template)throw new Error("actor_skill_learning_scope_changed");
    return {expected_revision:current.revision,prompt_template:outcome.proposed_prompt_template,source_revision:outcome.skill_revision};
}
