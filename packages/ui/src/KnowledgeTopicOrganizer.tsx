import {useState,type FormEvent,type ReactNode} from "react";
import {useQuery,useQueryClient} from "@tanstack/react-query";
import {decode,type Client,type KnowledgeFolder,type KnowledgeFolders,type KnowledgeGraphResults} from "@cowork/client";
import {productRequest,type Knowledge} from "./productApi";
import {knowledgeFolderPaths,knowledgeFolderSelection,knowledgeTopicProblem} from "./knowledgeTopics";
import {useView} from "./view";

export function useKnowledgeFolders(client:Client,project:string,personal:boolean){
  const path=`/projects/${encodeURIComponent(project)}/knowledge-folders?personal=${personal}`;
  return useQuery({queryKey:["product",client.base,path],queryFn:async()=>decode("KnowledgeFolders",await productRequest(client,path)),retry:false});
}

type Props={client:Client,project:string,personal:boolean,library:KnowledgeFolders,entries:Knowledge[],selected:string,onSelect:(id:string)=>void,descendants:boolean,onDescendants:(value:boolean)=>void};
export function KnowledgeTopics({client,project,personal,library,entries,selected,onSelect,descendants,onDescendants}:Props){
  const language=useView(v=>v.language),de=language==="de",tr=(a:string,b:string)=>de?a:b,qc=useQueryClient();
  const folders=library.folders,paths=knowledgeFolderPaths(folders),current=folders.find(f=>f.id===selected);
  const [busy,setBusy]=useState(false),[error,setError]=useState(""),[name,setName]=useState(""),[editing,setEditing]=useState(false),[query,setQuery]=useState(""),[results,setResults]=useState<KnowledgeGraphResults|null>(null);
  const graphPath=`/projects/${encodeURIComponent(project)}/knowledge-graphs?personal=${personal}`;
  const graphs=useQuery({queryKey:["product",client.base,graphPath],queryFn:async()=>{const value=await productRequest<unknown[]>(client,graphPath);if(!Array.isArray(value))throw Error("invalid_knowledge_graphs");return value.map(g=>decode("KnowledgeTopicGraph",g));},retry:false});
  const graph=graphs.data?.find(g=>g.folder_id===selected&&g.include_descendants===descendants);
  const refresh=()=>qc.invalidateQueries({queryKey:["product",client.base]});
  const changeSelection=(id:string)=>{onSelect(id);setEditing(false);setResults(null);setError("");};
  async function saveFolders(next:KnowledgeFolder[],select=selected){
    if(busy)return;setBusy(true);setError("");
    try{await productRequest(client,`/projects/${encodeURIComponent(project)}/knowledge-folders?personal=${personal}`,"PUT",{expected_revision:library.revision,folders:next});changeSelection(select);setName("");}
    catch(e){setError(knowledgeTopicProblem(e,de));}finally{await refresh();setBusy(false);}
  }
  function create(event:FormEvent){event.preventDefault();const id=crypto.randomUUID();void saveFolders([...folders,{id,name:name.trim(),parent_id:current?.id??null}],id);}
  function rename(event:FormEvent<HTMLFormElement>){event.preventDefault();if(!current)return;const form=new FormData(event.currentTarget);void saveFolders(folders.map(f=>f.id===current.id?{...f,name:String(form.get("name")??"").trim(),parent_id:String(form.get("parent")??"")||null}:f));}
  async function build(){if(!current||busy)return;setBusy(true);setError("");setResults(null);try{decode("KnowledgeTopicGraph",await productRequest(client,graphPath,"POST",{folder_id:current.id,include_descendants:descendants}));await refresh();}catch(e){setError(knowledgeTopicProblem(e,de));}finally{setBusy(false);}}
  async function removeGraph(){if(!current||busy)return;setBusy(true);setError("");setResults(null);try{await productRequest(client,graphPath,"DELETE",{folder_id:current.id,include_descendants:descendants});await refresh();}catch(e){setError(knowledgeTopicProblem(e,de));}finally{setBusy(false);}}
  async function search(event:FormEvent){event.preventDefault();if(!current||!graph||graph.stale||busy)return;setBusy(true);setError("");setResults(null);try{setResults(decode("KnowledgeGraphResults",await productRequest(client,`/projects/${encodeURIComponent(project)}/knowledge-graphs/search?personal=${personal}`,"POST",{folder_id:current.id,include_descendants:descendants,query})));}catch(e){setError(knowledgeTopicProblem(e,de));await refresh();}finally{setBusy(false);}}
  const blockedParents=knowledgeFolderSelection(folders,selected,true);
  const occupied=entries.some(e=>e.folder_id===selected)||folders.some(f=>f.parent_id===selected);
  function tree(parent:string|null):ReactNode{return <ul>{paths.filter(f=>f.parent_id===parent).map(folder=><li key={folder.id}><button type="button" disabled={busy} aria-current={selected===folder.id?"page":undefined} onClick={()=>changeSelection(folder.id)}>{folder.name}<small> {entries.filter(e=>e.folder_id===folder.id).length}</small></button>{folders.some(f=>f.parent_id===folder.id)&&tree(folder.id)}</li>)}</ul>;}
  return <section className="knowledge-topics settings-card" aria-label={tr("Themenordner","Topic folders")}>
    <div className="knowledge-topic-navigation"><h3>{tr("Themenordner","Topic folders")}</h3><nav aria-label={tr("Wissensordner","Knowledge folders")}><div className="actions"><button type="button" disabled={busy} aria-current={selected==="*"?"page":undefined} onClick={()=>changeSelection("*")}>{tr("Alle Einträge","All entries")}</button><button type="button" disabled={busy} aria-current={selected===""?"page":undefined} onClick={()=>changeSelection("")}>{tr("Ohne Ordner","Unfiled")}</button></div>{tree(null)}</nav>
      <form onSubmit={create}><label>{current?tr("Unterordner anlegen","Create subfolder"):tr("Themenordner anlegen","Create topic folder")}<input required value={name} maxLength={160} disabled={busy} onChange={e=>setName(e.target.value)} placeholder={tr("z. B. Forschung","e.g. Research")}/></label><button type="submit" disabled={busy||!name.trim()}>{tr("Ordner anlegen","Create folder")}</button></form>
    </div>
    <div className="knowledge-topic-details"><h3>{paths.find(f=>f.id===selected)?.path??tr("Wissensbasis","Knowledge base")}</h3>
      <p className="muted">{tr("Organisiere Themen in Ordnern und Unterordnern. Wähle einen Ordner als Materialbasis für einen Themengraphen.","Organize topics in folders and subfolders. Select a folder as the source corpus for a topic graph.")}</p>
      {current&&<><label className="product-check"><input type="checkbox" checked={descendants} onChange={e=>{onDescendants(e.target.checked);setResults(null);}}/>{tr("Unterordner einbeziehen","Include subfolders")}</label><div className="actions"><button type="button" disabled={busy} onClick={()=>setEditing(!editing)}>{tr("Ordner bearbeiten","Edit folder")}</button><button type="button" disabled={busy||occupied} title={occupied?tr("Verschiebe zuerst die Einträge und Unterordner.","Move the entries and subfolders first."):undefined} onClick={()=>void saveFolders(folders.filter(f=>f.id!==current.id),current.parent_id??"*")}>{tr("Leeren Ordner löschen","Delete empty folder")}</button></div>
        {editing&&<form key={current.id} onSubmit={rename}><label>{tr("Ordnername","Folder name")}<input name="name" required maxLength={160} defaultValue={current.name}/></label><label>{tr("Übergeordneter Ordner","Parent folder")}<select name="parent" defaultValue={current.parent_id??""}><option value="">{tr("Oberste Ebene","Top level")}</option>{paths.filter(f=>!blockedParents.has(f.id)).map(f=><option key={f.id} value={f.id}>{f.path}</option>)}</select></label><button disabled={busy}>{tr("Ordner speichern","Save folder")}</button></form>}
        <section className="knowledge-graph"><h4>{tr("Graph-RAG für dieses Thema","Graph RAG for this topic")}</h4><p className="muted">{tr("Verbindet Einträge über gemeinsame Tags und zitierte Quellen. Aktuelle Graphen ergänzen die Kontextsuche neuer Agentenläufe. Beziehungen werden aus deinen Angaben gebildet; eine automatische Entitätsextraktion durch ein Modell ist nicht enthalten.","Connects entries through shared tags and cited sources. Current graphs extend context retrieval for new agent runs. Relationships come from your metadata; automatic model-based entity extraction is not included.")}</p>
          {graph&&<p role="status">{graph.stale?tr("Veraltet – vor der Nutzung neu aufbauen.","Stale — rebuild before use."):tr("Aktuell","Current")} · {graph.documents} {tr("Einträge","entries")} · {graph.nodes.length} {tr("Knoten","nodes")} · {graph.edges.length} {tr("Verbindungen","links")}</p>}
          <div className="actions"><button type="button" disabled={busy} onClick={()=>void build()}>{graph?tr("Graph neu aufbauen","Rebuild graph"):tr("Themengraph aufbauen","Build topic graph")}</button>{graph&&<button type="button" disabled={busy} onClick={()=>void removeGraph()}>{tr("Graph entfernen (Einträge behalten)","Remove graph (keep entries)")}</button>}</div>
          {graph&&<details><summary>{tr("Graphverbindungen ansehen","Inspect graph links")}</summary><ul className="knowledge-graph-links">{graph.edges.slice(0,100).map((edge,index)=><li key={index}>{graph.nodes.find(n=>n.id===edge.from)?.label} → {graph.nodes.find(n=>n.id===edge.to)?.label} <small>({edge.relation})</small></li>)}</ul>{graph.edges.length>100&&<small>{tr("Erste 100 Verbindungen.","First 100 links.")}</small>}</details>}
          {graph&&!graph.stale&&<form onSubmit={search}><label>{tr("Graphsuche ausprobieren","Try graph retrieval")}<input required maxLength={500} value={query} onChange={e=>{setQuery(e.target.value);setResults(null);}}/></label><button disabled={busy||!query.trim()}>{tr("Graph durchsuchen","Search graph")}</button></form>}
          {results&&graph&&!graph.stale&&results.fingerprint===graph.fingerprint&&<div aria-live="polite">{!results.hits.length?<p>{tr("Keine passenden Einträge.","No matching entries.")}</p>:<ul className="knowledge-entry-list">{results.hits.map(hit=><li key={hit.document.id}><strong>{hit.document.title}</strong><small>{hit.via==="direct"?tr("Direkter Texttreffer","Direct text match"):tr("Über Verbindung: ","Via link: ")+(graph.nodes.find(n=>n.id===hit.via)?.label??hit.via)} · v{hit.document.revision}</small><p className="product-preview">{hit.document.content}</p></li>)}</ul>}</div>}
        </section>
      </>}
      {(error||graphs.isError)&&<p role="alert">{error||knowledgeTopicProblem(graphs.error,de)}</p>}
    </div>
  </section>;
}
