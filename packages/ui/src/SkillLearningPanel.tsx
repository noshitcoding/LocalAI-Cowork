import {useQuery} from "@tanstack/react-query";
import type {Client,Skill} from "@cowork/client";
import {useView} from "./view";
export function SkillLearningPanel({client,project,skill,onDraft}:{client:Client;project:string;skill:Skill;onDraft:(value:Skill)=>void}){
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const query=useQuery({queryKey:["skill-learning",client.base,project,skill.id],queryFn:()=>client.one(`/projects/${encodeURIComponent(project)}/skills/${encodeURIComponent(skill.id)}/learning`,"SkillLearningHistory"),retry:false});
  return <details><summary>{tr("Lernhistorie","Learning history")} · {query.data?.items.length??"…"}</summary><p>{tr("Automatisch erfasste Laufergebnisse und abgeleitete Vorschläge. Fertigstellung ist keine Qualitätsbewertung. Eine Übernahme erzeugt einen bearbeitbaren Entwurf.","Automatically recorded run outcomes and derived suggestions. Completion is not a quality rating. Adopting creates an editable draft.")}</p>{query.error&&<p role="alert">{String(query.error)}</p>}{query.data?.items.map(item=><article key={item.run_id+":"+item.skill_revision}><p>v{item.skill_revision} · {item.state} · {new Date(item.recorded_at).toLocaleString()}</p><p>{item.reason}</p>{item.proposed_instructions&&<><pre>{item.proposed_instructions}</pre><button type="button" onClick={()=>onDraft({...skill,instructions:item.proposed_instructions!})}>{tr("Als neuen Entwurf übernehmen","Adopt as new draft")}</button></>}</article>)}</details>;
}
