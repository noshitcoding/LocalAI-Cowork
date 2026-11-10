import {useState,type FormEvent} from "react";
import {useQuery,useQueryClient} from "@tanstack/react-query";
import type {Client} from "@cowork/client";
import {productRequest,type Knowledge} from "./productApi";
import {KnowledgeImportPanel} from "./KnowledgeImportPanel";
import {KnowledgeTopics,useKnowledgeFolders} from "./KnowledgeTopicOrganizer";
import {knowledgeFolderPaths,knowledgeFolderSelection,knowledgeTopicProblem} from "./knowledgeTopics";
import {useView} from "./view";
import {isPersonalEntry,ownerKnowledgeAccess,personalKnowledgeProject,personalMemory,projectKnowledgeAccess,writeOwnerKnowledgeAccess,writeProjectKnowledgeAccess} from "./knowledgeAccessApi";
import "./knowledgeBase.css";

type Mode="project"|"personal";
const useTr=()=>{const language=useView(v=>v.language);return (de:string,en:string)=>language==="de"?de:en;};
function problem(error:unknown,tr:(de:string,en:string)=>string){
  const text=error instanceof Error?error.message:String(error);
  if(/revision_conflict|request_conflict|\(409\)/.test(text))return tr("Die Einstellung wurde zwischenzeitlich geändert. Der aktuelle Stand wurde geladen.","The setting changed elsewhere. The current values were loaded.");
  return text;
}

/** Project page: agent switches, notes and imported files of this project's knowledge base. */
export function ProjectKnowledgeBase({client,project}:{client:Client;project:string}){
  const tr=useTr();
  return <div className="knowledge-base"><ProjectKnowledgeSwitches client={client} project={project}/>
    <KnowledgeEntries client={client} project={project} mode="project" intro={tr("Infos, Notizen und importierte Dateien dieses Projekts. Agenten erhalten passende Einträge als Kontext, solange die Projektwissensbasis freigegeben ist.","Information, notes and imported files of this project. Agents receive relevant entries as context while the project knowledge base is enabled.")}/></div>;
}

/** Sidebar page: the personal knowledge base is shared by all of the owner's projects. */
export function PersonalKnowledgeBase({client}:{client:Client}){
  const tr=useTr();
  const space=useQuery({queryKey:["independent-chat-space"],queryFn:()=>client.one("/independent-chat-space","Project"),retry:false});
  return <div className="knowledge-base"><header className="knowledge-base-header"><h1>{tr("Meine Wissensbasis","My knowledge base")}</h1><p className="muted">{tr("Persönliches Wissen gilt projektübergreifend: Vorlieben, Fakten über dich und Material, das Agenten in jedem deiner Projekte nutzen dürfen.","Personal knowledge applies across projects: preferences, facts about you and material agents may use in all of your projects.")}</p></header>
    <OwnerKnowledgeSwitch client={client}/>
    {space.isSuccess?<KnowledgeEntries client={client} project={personalKnowledgeProject} mode="personal" intro={tr("Einträge hier stehen allen Projekten zur Verfügung, in denen die persönliche Wissensbasis freigegeben ist.","Entries here are available in every project where the personal knowledge base is enabled.")}/>:space.isError?<p role="alert">{problem(space.error,tr)}</p>:<p role="status">{tr("Wird geladen …","Loading …")}</p>}</div>;
}

function OwnerKnowledgeSwitch({client}:{client:Client}){
  const tr=useTr(),qc=useQueryClient(),key=["knowledge-access",client.base,"owner"];
  const query=useQuery({queryKey:key,queryFn:()=>ownerKnowledgeAccess(client),retry:false});
  const [busy,setBusy]=useState(false),[error,setError]=useState("");
  async function save(personal:boolean){if(!query.data||busy)return;setBusy(true);setError("");try{qc.setQueryData(key,await writeOwnerKnowledgeAccess(client,{expected_revision:query.data.revision,personal_knowledge:personal}));await qc.invalidateQueries({queryKey:["knowledge-access",client.base]});}catch(e){setError(problem(e,tr));await query.refetch();}finally{setBusy(false);}}
  return <section className="settings-card knowledge-switches"><h2>{tr("Zugriff für Agenten","Agent access")}</h2>
    {query.isError?<p role="alert">{problem(query.error,tr)}</p>:!query.data?<p role="status">{tr("Wird geladen …","Loading …")}</p>:<label className="product-check"><input type="checkbox" checked={query.data.personal_knowledge} disabled={busy} onChange={e=>void save(e.target.checked)}/>{tr("Agenten dürfen meine persönliche Wissensbasis verwenden","Agents may use my personal knowledge base")}</label>}
    <p className="muted">{tr("Ausgeschaltet erhält kein neuer Lauf persönliche Einträge – in keinem Projekt. Pro Projekt kannst du sie zusätzlich auf der Projektseite abschalten.","When off, no new run receives personal entries in any project. You can additionally switch them off per project on the project page.")}</p>{error&&<p role="alert">{error}</p>}</section>;
}

function ProjectKnowledgeSwitches({client,project}:{client:Client;project:string}){
  const tr=useTr(),qc=useQueryClient(),key=["knowledge-access",client.base,"project",project];
  const query=useQuery({queryKey:key,queryFn:()=>projectKnowledgeAccess(client,project),retry:false});
  const [busy,setBusy]=useState(false),[error,setError]=useState("");
  async function save(patch:{project_knowledge?:boolean;personal_knowledge?:boolean}){const current=query.data;if(!current||busy)return;setBusy(true);setError("");try{qc.setQueryData(key,await writeProjectKnowledgeAccess(client,project,{expected_revision:current.revision,project_knowledge:patch.project_knowledge??current.project_knowledge,personal_knowledge:patch.personal_knowledge??current.personal_knowledge}));}catch(e){setError(problem(e,tr));await query.refetch();}finally{setBusy(false);}}
  const value=query.data;
  return <section className="settings-card knowledge-switches"><h2>{tr("Zugriff für Agenten","Agent access")}</h2><p className="muted">{tr("Gilt für neue Läufe in diesem Projekt. Bereits gestartete Läufe behalten ihren Kontext.","Applies to new runs in this project. Runs already started keep their context.")}</p>
    {query.isError?<p role="alert">{problem(query.error,tr)}</p>:!value?<p role="status">{tr("Wird geladen …","Loading …")}</p>:<>
      <label className="product-check"><input type="checkbox" checked={value.project_knowledge} disabled={busy} onChange={e=>void save({project_knowledge:e.target.checked})}/>{tr("Projektwissensbasis verwenden","Use the project knowledge base")}</label>
      <label className="product-check"><input type="checkbox" checked={value.personal_knowledge&&value.personal_available} disabled={busy||!value.personal_available} onChange={e=>void save({personal_knowledge:e.target.checked})}/>{tr("Meine persönliche Wissensbasis in diesem Projekt verwenden","Use my personal knowledge base in this project")}</label>
      {!value.personal_available&&<p className="muted">{tr("Die persönliche Wissensbasis ist unter „Meine Wissensbasis“ für alle Projekte ausgeschaltet.","The personal knowledge base is switched off for all projects under “My knowledge base”.")}</p>}
    </>}{error&&<p role="alert">{error}</p>}</section>;
}

function KnowledgeEntries({client,project,mode,intro}:{client:Client;project:string;mode:Mode;intro:string}){
  const tr=useTr(),qc=useQueryClient();
  const [selected,setSelected]=useState("*"),[descendants,setDescendants]=useState(true);
  const [noteFolder,setNoteFolder]=useState("");
  const folders=useKnowledgeFolders(client,project,mode==="personal");
  const paths=knowledgeFolderPaths(folders.data?.folders??[]);
  const selectedFolders=knowledgeFolderSelection(folders.data?.folders??[],selected,descendants);
  const [search,setSearch]=useState(""),[edit,setEdit]=useState<Knowledge|null>(null),[epoch,setEpoch]=useState(0),[removing,setRemoving]=useState<Knowledge|null>(null),[busy,setBusy]=useState(false),[error,setError]=useState(""),[notice,setNotice]=useState("");
  const path=`/projects/${encodeURIComponent(project)}/knowledge`;
  const items=useQuery({queryKey:["product",client.base,path,"knowledge-base",mode],queryFn:()=>productRequest<{document:Knowledge}[]>(client,`${path}?limit=2000`)});
  const allEntries=(items.data??[]).map(hit=>hit.document).filter(entry=>mode==="personal"?isPersonalEntry(entry):entry.project_id===project&&!isPersonalEntry(entry));
  const terms=search.toLowerCase().split(/\s+/).filter(Boolean);
  const entries=allEntries.filter(entry=>(selected==="*"||(selected===""?!entry.folder_id:selectedFolders.has(entry.folder_id??"")))&&terms.every(term=>`${entry.title} ${entry.content} ${entry.tags.join(" ")} ${paths.find(f=>f.id===entry.folder_id)?.path??""}`.toLowerCase().includes(term)));
  const refresh=async()=>{await qc.invalidateQueries({queryKey:["product",client.base]});};
  async function save(event:FormEvent<HTMLFormElement>){
    event.preventDefault();if(busy)return;const form=new FormData(event.currentTarget);setBusy(true);setError("");setNotice("");
    try{
      const target=edit?.project_id??project;
      const body={location:{folder_id:String(form.get("folder")??"")||null},id:edit?.id??null,expected_revision:edit?.revision??0,kind:mode==="personal"?"memory":edit?.kind??"knowledge",memory:mode==="personal"?(edit?.memory??personalMemory()):edit?.kind==="memory"?edit.memory??null:null,title:String(form.get("title")??"").trim(),content:String(form.get("content")??""),tags:String(form.get("tags")??"").split(",").map(v=>v.trim()).filter(Boolean)};
      await productRequest(client,`/projects/${encodeURIComponent(target)}/knowledge`,"POST",body);
      setEdit(null);setEpoch(v=>v+1);setNotice(tr("Eintrag gespeichert.","Entry saved."));await refresh();
    }catch(e){setError(knowledgeTopicProblem(e,tr("de","en")==="de"));}finally{setBusy(false);}
  }
  async function remove(){if(!removing||busy)return;setBusy(true);setError("");try{await productRequest(client,`/projects/${encodeURIComponent(removing.project_id)}/knowledge/${encodeURIComponent(removing.id)}`,"DELETE",{expected_revision:removing.revision});if(edit?.id===removing.id){setEdit(null);setEpoch(v=>v+1);}setRemoving(null);await refresh();}catch(e){setError(problem(e,tr));}finally{setBusy(false);}}
  const imported=(entry:Knowledge)=>entry.provenance?.origin==="imported";
  return <section className="knowledge-entries">
    {folders.isError&&<p role="alert">{problem(folders.error,tr)}</p>}
    {folders.data&&<KnowledgeTopics client={client} project={project} personal={mode==="personal"} library={folders.data} entries={allEntries} selected={selected} onSelect={id=>{setSelected(id);if(!edit)setNoteFolder(id==="*"?"":id);}} descendants={descendants} onDescendants={setDescendants}/>}
    <div className="knowledge-entries-columns">
      <div>
        <h2>{mode==="personal"?tr("Persönliche Einträge","Personal entries"):tr("Infos & Dateien","Information & files")}</h2><p className="muted">{intro}</p>
        <label>{tr("Suchen","Search")}<input value={search} onChange={e=>setSearch(e.target.value)} maxLength={500}/></label>
        {items.isError&&<p role="alert">{problem(items.error,tr)}</p>}{items.isPending&&<p role="status">{tr("Wird geladen …","Loading …")}</p>}
        {items.isSuccess&&!entries.length&&<p className="muted">{search?tr("Keine passenden Einträge.","No matching entries."):tr("Noch keine Einträge. Lege eine Notiz an oder importiere Dateien.","No entries yet. Add a note or import files.")}</p>}
        <ul className="knowledge-entry-list">{entries.map(entry=><li key={entry.id} className="product-item"><strong>{entry.title}</strong>
          <small>{imported(entry)?tr("Aus Datei importiert","Imported from a file"):tr("Notiz","Note")}{entry.tags.length?" · "+entry.tags.join(", "):""} · v{entry.revision}{mode==="personal"&&entry.project_id!==personalKnowledgeProject?" · "+tr("aus einem Projekt","from a project"):""}</small>
          <small>{paths.find(f=>f.id===entry.folder_id)?.path??tr("Ohne Ordner","Unfiled")}</small>
          <p className="product-preview">{entry.content}</p>
          <div className="actions"><button type="button" disabled={busy} onClick={()=>{setEdit(entry);setNoteFolder(entry.folder_id??"");setEpoch(v=>v+1);setNotice("");}}>{tr("Bearbeiten","Edit")}</button><button type="button" disabled={busy} onClick={()=>setRemoving(entry)}>{tr("Löschen","Delete")}</button></div>
          {removing?.id===entry.id&&<div className="knowledge-confirm" role="alertdialog" aria-label={tr("Eintrag löschen","Delete entry")}><p>{tr("Diesen Eintrag löschen? Bereits gestartete Läufe behalten ihre Kopie.","Delete this entry? Runs already started keep their copy.")}</p><div className="actions"><button type="button" disabled={busy} onClick={()=>setRemoving(null)}>{tr("Abbrechen","Cancel")}</button><button type="button" className="primary" disabled={busy} onClick={()=>void remove()}>{tr("Endgültig löschen","Delete")}</button></div></div>}
        </li>)}</ul>
      </div>
      <form key={epoch} className="knowledge-entry-form" onSubmit={event=>void save(event)}><h3>{edit?tr("Eintrag bearbeiten","Edit entry"):tr("Notiz hinzufügen","Add note")}</h3>
        <label>{tr("Themenordner","Topic folder")}<select name="folder" value={noteFolder} onChange={e=>setNoteFolder(e.target.value)} disabled={!folders.data||busy}><option value="">{tr("Ohne Ordner","Unfiled")}</option>{paths.map(folder=><option key={folder.id} value={folder.id}>{folder.path}</option>)}</select></label>
        <label>{tr("Titel","Title")}<input name="title" required maxLength={200} defaultValue={edit?.title}/></label>
        <label>{tr("Inhalt","Content")}<textarea name="content" rows={10} required maxLength={128000} defaultValue={edit?.content}/></label>
        <label>{tr("Tags, durch Kommas getrennt","Comma-separated tags")}<input name="tags" defaultValue={edit?.tags.join(", ")}/></label>
        <div className="actions">{edit&&<button type="button" disabled={busy} onClick={()=>{setEdit(null);setEpoch(v=>v+1);}}>{tr("Abbrechen","Cancel")}</button>}<button className="primary" disabled={busy||!folders.data}>{busy?tr("Wird gespeichert …","Saving …"):tr("Speichern","Save")}</button></div>
        {error&&<p role="alert">{error}</p>}{notice&&<p role="status">{notice}</p>}
      </form>
    </div>
    {folders.data&&<KnowledgeImportPanel client={client} project={project} memory={mode==="personal"?personalMemory():undefined} journalScope={mode} onImported={refresh} folders={paths}/>}
  </section>;
}
