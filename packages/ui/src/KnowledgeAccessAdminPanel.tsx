import {useEffect,useRef,useState} from "react";
import type {Client} from "@cowork/client";
import {useView} from "./view";
import {knowledgeAccessDirectory,toggleException,writeKnowledgeAccessPolicy,type KnowledgeAccessDirectory,type WriteKnowledgeAccessPolicy} from "./knowledgeAccessApi";
import "./knowledgeBase.css";

/** Platform administrators decide whether agents may use knowledge bases on this server. */
export function KnowledgeAccessAdminPanel({client,session}:{client:Client;session:string}){
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const [directory,setDirectory]=useState<KnowledgeAccessDirectory>(),[draft,setDraft]=useState<WriteKnowledgeAccessPolicy>(),[userQuery,setUserQuery]=useState(""),[projectQuery,setProjectQuery]=useState(""),[personalQuery,setPersonalQuery]=useState("");
  const [busy,setBusy]=useState(false),[error,setError]=useState(""),[notice,setNotice]=useState("");const generation=useRef(0);
  const message=(e:unknown)=>{const text=e instanceof Error?e.message:String(e);return /request_conflict|\(409\)/.test(text)?tr("Die Richtlinie wurde zwischenzeitlich geändert. Der aktuelle Stand wurde geladen; prüfe deine Auswahl erneut.","The policy changed elsewhere. The current values were loaded; review your selection again."):/access_denied|\(404\)|\(403\)/.test(text)?tr("Nur Plattformadministratoren mit einer globalen Sitzung dürfen diese Richtlinie sehen und ändern.","Only platform administrators with a global session may view and change this policy."):text;};
  async function load(){const current=++generation.current;setBusy(true);setError("");try{const value=await knowledgeAccessDirectory(client,session);if(current!==generation.current)return;setDirectory(value);setDraft({expected_revision:value.policy.revision,private_projects:value.policy.private_projects,team_projects:value.policy.team_projects,enabled_users:[...value.policy.enabled_users],enabled_projects:[...value.policy.enabled_projects],...(value.policy.personal_team_runs===undefined?{}:{personal_team_runs:value.policy.personal_team_runs,enabled_personal_team_users:[...value.policy.enabled_personal_team_users??[]]})});}catch(e){if(current===generation.current)setError(message(e));}finally{if(current===generation.current)setBusy(false);}}
  useEffect(()=>{void load();return()=>{generation.current++;};},[client.base,session]);
  async function save(){if(!draft||busy)return;setBusy(true);setError("");setNotice("");let reload=true;try{await writeKnowledgeAccessPolicy(client,session,draft);setNotice(tr("Richtlinie gespeichert. Sie gilt für neue Läufe.","Policy saved. It applies to new runs."));}catch(e){setError(message(e));reload=/request_conflict|\(409\)/.test(e instanceof Error?e.message:String(e));}finally{setBusy(false);}if(reload)await load();}
  const policy=directory?.policy;
  const changed=!!draft&&!!policy&&(draft.private_projects!==policy.private_projects||draft.team_projects!==policy.team_projects||JSON.stringify(draft.enabled_users)!==JSON.stringify([...policy.enabled_users].sort())||JSON.stringify(draft.enabled_projects)!==JSON.stringify([...policy.enabled_projects].sort())||draft.personal_team_runs!==policy.personal_team_runs||JSON.stringify(draft.enabled_personal_team_users)!==JSON.stringify(policy.enabled_personal_team_users&&[...policy.enabled_personal_team_users].sort()));
  const users=directory?.users.filter(u=>(u.username+" "+u.display_name).toLocaleLowerCase().includes(userQuery.trim().toLocaleLowerCase()))??[];
  const personalUsers=directory?.users.filter(u=>(u.username+" "+u.display_name).toLocaleLowerCase().includes(personalQuery.trim().toLocaleLowerCase()))??[];
  const projects=directory?.projects.filter(p=>(p.name+" "+p.team_name).toLocaleLowerCase().includes(projectQuery.trim().toLocaleLowerCase()))??[];
  return <details className="product-item knowledge-admin"><summary>{tr("Wissensbasen für Agenten","Knowledge bases for agents")}</summary>
    <p className="muted">{tr("Lege fest, ob Agenten auf diesem Server Projektwissen und persönliches Wissen verwenden dürfen. Standard ist ausgeschaltet. Ist ein globaler Schalter aus, kannst du einzelne Nutzer (für ihre privaten Projekte) oder einzelne Teamprojekte trotzdem freischalten. Die Freigabe verleiht keine Lese- oder Dateirechte und ändert keine bereits gestarteten Läufe.","Decide whether agents on this server may use project and personal knowledge. Off by default. While a global switch is off, you can still enable individual users (for their private projects) or individual team projects. Enabling grants no read or file rights and does not change runs already started.")}</p>
    {error&&<p role="alert" className="error">{error}</p>}{notice&&<p role="status">{notice}</p>}
    {!draft?<p role="status">{busy?tr("Wird geladen …","Loading …"):""}</p>:<fieldset disabled={busy}>
      <section><h4>{tr("Private Projekte der Nutzer","Users' private projects")}</h4>
        <label className="product-check"><input type="checkbox" checked={draft.private_projects} onChange={e=>setDraft({...draft,private_projects:e.target.checked})}/>{tr("Für alle Nutzer freischalten (Projekt- und persönliche Wissensbasis)","Enable for all users (project and personal knowledge base)")}</label>
        {!draft.private_projects&&<><label>{tr("Nutzer suchen","Search users")}<input value={userQuery} maxLength={200} onChange={e=>setUserQuery(e.target.value)}/></label>
          <ul className="project-folder-list">{users.map(u=><li key={u.id}><label className="product-check"><input type="checkbox" checked={draft.enabled_users.includes(u.id)} onChange={e=>setDraft({...draft,enabled_users:toggleException(draft.enabled_users,u.id,e.target.checked)})}/>{u.display_name} · {u.username}</label><small>{u.active?"":tr("Deaktiviert","Disabled")}</small></li>)}</ul>
          {!users.length&&<p className="muted">{tr("Keine passenden Nutzer.","No matching users.")}</p>}</>}
      </section>
      <section><h4>{tr("Erstellte Teamprojekte","Created team projects")}</h4>
        <label className="product-check"><input type="checkbox" checked={draft.team_projects} onChange={e=>setDraft({...draft,team_projects:e.target.checked})}/>{tr("Für alle Teamprojekte freischalten","Enable for all team projects")}</label>
        {!draft.team_projects&&<><label>{tr("Projekte suchen","Search projects")}<input value={projectQuery} maxLength={200} onChange={e=>setProjectQuery(e.target.value)}/></label>
          <ul className="project-folder-list">{projects.map(p=><li key={p.id}><label className="product-check"><input type="checkbox" checked={draft.enabled_projects.includes(p.id)} onChange={e=>setDraft({...draft,enabled_projects:toggleException(draft.enabled_projects,p.id,e.target.checked)})}/>{p.name||p.id}</label><small>{p.team_name}{p.archived?" · "+tr("archiviert","archived"):""}</small></li>)}</ul>
          {!projects.length&&<p className="muted">{tr("Keine passenden Teamprojekte.","No matching team projects.")}</p>}</>}
      </section>
      {draft.personal_team_runs!==undefined&&<section><h4>{tr("Persönliche Wissensbasis in Team-Läufen","Personal knowledge base in team runs")}</h4>
        <p className="muted">{tr("Erlaubt Personen, beim Start eines Team-Laufs ihre persönliche Wissensbasis mitzugeben. Ergebnisse sieht das Team, private Inhalte können darin erscheinen. Unabhängig von den Schaltern oben.","Lets people add their personal knowledge base when they start a team run. The team sees the results, which may contain private content. Independent of the switches above.")}</p>
        <label className="product-check"><input type="checkbox" checked={draft.personal_team_runs} onChange={e=>setDraft({...draft,personal_team_runs:e.target.checked})}/>{tr("Für alle Nutzer freischalten","Enable for all users")}</label>
        {!draft.personal_team_runs&&<><label>{tr("Nutzer suchen","Search users")}<input value={personalQuery} maxLength={200} onChange={e=>setPersonalQuery(e.target.value)}/></label>
          <ul className="project-folder-list">{personalUsers.map(u=><li key={u.id}><label className="product-check"><input type="checkbox" checked={!!draft.enabled_personal_team_users?.includes(u.id)} onChange={e=>setDraft({...draft,enabled_personal_team_users:toggleException(draft.enabled_personal_team_users??[],u.id,e.target.checked)})}/>{u.display_name} · {u.username}</label><small>{u.active?"":tr("Deaktiviert","Disabled")}</small></li>)}</ul>
          {!personalUsers.length&&<p className="muted">{tr("Keine passenden Nutzer.","No matching users.")}</p>}</>}
      </section>}
      {directory?.truncated&&<p className="muted">{tr("Es werden höchstens 500 Nutzer und 500 Projekte angezeigt.","At most 500 users and 500 projects are shown.")}</p>}
      <div className="actions"><button type="button" onClick={()=>void load()}>{tr("Neu laden","Reload")}</button><button type="button" className="primary" disabled={!changed} onClick={()=>void save()}>{tr("Richtlinie speichern","Save policy")}</button></div>
      {policy?.updated_at&&<small>{tr("Zuletzt geändert","Last changed")}: {new Date(policy.updated_at).toLocaleString(language)}</small>}
    </fieldset>}
  </details>;
}
