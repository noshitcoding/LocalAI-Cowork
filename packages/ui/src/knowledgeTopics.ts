import type {KnowledgeFolder} from "@cowork/client";

/** Stable paths for breadcrumbs, selects and folder searches. */
export function knowledgeFolderPaths(folders:KnowledgeFolder[]){
  const byId=new Map(folders.map(folder=>[folder.id,folder]));
  const path=(id:string)=>{const names:string[]=[],seen=new Set<string>();let folder=byId.get(id);while(folder){if(seen.has(folder.id)||seen.size>=12)throw Error("knowledge_folder_cycle_or_depth");seen.add(folder.id);names.unshift(folder.name);folder=folder.parent_id?byId.get(folder.parent_id):undefined;}return names.join(" / ");};
  return folders.map(folder=>({...folder,path:path(folder.id)})).sort((a,b)=>a.path.localeCompare(b.path)||a.id.localeCompare(b.id));
}
export function knowledgeFolderSelection(folders:KnowledgeFolder[],selected:string,descendants:boolean){
  const ids=new Set([selected]);
  if(descendants){let count=0;while(count!==ids.size){count=ids.size;for(const folder of folders)if(folder.parent_id&&ids.has(folder.parent_id))ids.add(folder.id);}}
  return ids;
}
export function knowledgeTopicProblem(error:unknown,de:boolean){
  const text=error instanceof Error?error.message:String(error),reason=error&&typeof error==="object"&&"reason" in error?String(error.reason):text;
  const messages:Record<string,[string,string]>={
    knowledge_folder_not_empty:["Der Ordner enthält Einträge. Verschiebe diese zuerst.","Move the entries out of this folder first."],
    invalid_knowledge_folder:["Verwende einen eindeutigen Ordnernamen ohne Schrägstriche (maximal 160 UTF-8-Bytes).","Use a unique folder name without slashes (up to 160 UTF-8 bytes)."],
    knowledge_folder_cycle_or_depth:["Ein Ordner darf nicht in sich selbst liegen; maximal 12 Ebenen.","A folder cannot contain itself; use up to 12 levels."],
    knowledge_folder_missing:["Der Ordner ist nicht mehr vorhanden. Lade die Ansicht neu.","This folder no longer exists. Reload the view."],
    knowledge_graph_stale:["Der Graph ist veraltet. Baue ihn für den aktuellen Inhalt neu auf.","The graph is stale. Rebuild it for the current content."],
    knowledge_graph_limit:["Der Graph ist zu groß. Wähle einen kleineren Themenbereich.","The graph is too large. Select a smaller topic."],
    knowledge_graph_count_limit:["Maximal 32 Themengraphen pro Wissensbasis.","Use up to 32 topic graphs per knowledge base."],
  };
  for(const [code,message] of Object.entries(messages))if(reason.includes(code)||text.includes(code))return message[de?0:1];
  if(/409|revision_conflict/.test(text))return de?"Die Wissensbasis wurde zwischenzeitlich geändert. Der aktuelle Stand wird geladen.":"The knowledge base changed elsewhere. The current state is being loaded.";
  return reason||text;
}
