import {useState} from "react";
import {useQuery} from "@tanstack/react-query";
import type {AutomationFileWatch,Client,FileWatchChange,WorkspaceTask} from "@cowork/client";
import {useView} from "./view";
import {workspaceManagementKey} from "./workspaceManagementApi";
import {automationPath} from "./automationApi";
import {defaultFileWatch,eligibleWatchFolders,fileWatchMessage,watchExtensions} from "./fileWatchUi";

export function FileWatchEditor({client,project,task,value,onChange}:{client:Client;project:string;task:WorkspaceTask|undefined;value:AutomationFileWatch|null;onChange:(value:AutomationFileWatch)=>void}){
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const [extensionsText,setExtensionsText]=useState(value?.extensions.join(", ")??""),[excludedText,setExcludedText]=useState(value?.exclude_paths.join(", ")??"");
  const current=useQuery({queryKey:["automation-watch-folders",...workspaceManagementKey(client),project],queryFn:()=>client.folders(project),retry:false});
  const folders=eligibleWatchFolders(task,current.data??[]);
  const patch=(patch:Partial<AutomationFileWatch>)=>{if(value)onChange({...value,...patch});};
  return <fieldset className="product-stack"><legend>{tr("Dateiüberwachung","File observation")}</legend>
    <p>{tr("Wähle einen aktuellen Projektordner, auf den die Aufgabe bereits Zugriff hat. Änderungen werden anhand des Datei-Inhalts erkannt. Ausgaben kannst du in einem anderen Ordner oder unter einem ausgeschlossenen Pfad ablegen.","Choose a current project folder that the task can already access. Changes are detected from file contents. Store outputs in another folder or an excluded path.")}</p>
    {current.isError&&<p role="alert">{tr("Ordnerfreigaben konnten nicht geladen werden.","Could not load folder access.")}</p>}
    {!current.isPending&&!folders.length&&<p role="status">{tr("Füge der Aufgabe zuerst eine aktuelle Projektordner-Freigabe hinzu und speichere sie.","Add current project folder access to the task and save it first.")}</p>}
    <label>{tr("Überwachter Ordner","Observed folder")}<select aria-label={tr("Überwachter Ordner","Observed folder")} required value={value?.folder.id??""} onChange={event=>{const folder=folders.find(folder=>folder.id===event.target.value);if(folder){setExtensionsText("");setExcludedText("");onChange(defaultFileWatch(folder));}}}><option value="">{tr("Ordner wählen","Choose folder")}</option>{folders.map(folder=><option key={folder.id} value={folder.id}>{folder.name}</option>)}</select></label>
    {value&&<>
      <label>{tr("Unterordner, Punkt für den gesamten Ordner","Subfolder, dot for the whole folder")}<input required maxLength={1024} value={value.path} onChange={event=>patch({path:event.target.value})}/></label>
      <label className="product-check"><input type="checkbox" checked={value.recursive} onChange={event=>patch({recursive:event.target.checked})}/>{tr("Unterordner einbeziehen","Include subfolders")}</label>
      <label>{tr("Dateitypen, durch Kommas getrennt; leer für alle","File types, separated by commas; empty for all")}<input maxLength={640} placeholder=".pdf, .png, .docx" value={extensionsText} onChange={event=>{setExtensionsText(event.target.value);patch({extensions:watchExtensions(event.target.value)});}}/></label>
      <label>{tr("Ausgeschlossene Pfade ab Projektordner, durch Kommas getrennt","Excluded paths relative to the project folder, separated by commas")}<input maxLength={4096} placeholder="processed, reports" value={excludedText} onChange={event=>{setExcludedText(event.target.value);patch({exclude_paths:event.target.value.split(",").map(path=>path.trim()).filter(Boolean)});}}/></label>
      <fieldset><legend>{tr("Ereignisse","Events")}</legend>{([['created',tr("Neue Dateien","New files")],['modified',tr("Geänderter Datei-Inhalt","Changed file contents")],['removed',tr("Entfernte Dateien","Removed files")]] as [FileWatchChange,string][]).map(([change,label])=><label className="product-check" key={change}><input type="checkbox" checked={value.changes.includes(change)} onChange={event=>patch({changes:event.target.checked?[...value.changes,change]:value.changes.filter(item=>item!==change)})}/>{label}</label>)}</fieldset>
      <label className="product-check"><input type="checkbox" checked={value.include_existing} onChange={event=>patch({include_existing:event.target.checked})}/>{tr("Bereits vorhandene Dateien beim ersten vollständigen Scan verarbeiten","Process existing files on the first complete scan")}</label>
      <label>{tr("Datei muss unverändert bleiben, Sekunden","File must remain unchanged, seconds")}<input type="number" required min={1} max={300} value={value.debounce_ms/1000} onChange={event=>patch({debounce_ms:Number(event.target.value)*1000})}/></label>
      <label>{tr("Prüfabstand, Sekunden","Scan interval, seconds")}<input type="number" required min={5} max={3600} value={value.poll_interval_ms/1000} onChange={event=>patch({poll_interval_ms:Number(event.target.value)*1000})}/></label>
      <details><summary>{tr("Grenzen für die vollständige Prüfung","Limits for a complete scan")}</summary><p>{tr("Unvollständige Prüfungen ändern den gespeicherten Dateistand nicht. Links und interne Cowork-Dateien werden ausgelassen.","Incomplete scans do not change the saved file state. Links and internal Cowork files are excluded.")}</p>
        <label>{tr("Maximale Dateianzahl","Maximum file count")}<input type="number" required min={1} max={10000} value={value.max_files} onChange={event=>patch({max_files:Number(event.target.value)})}/></label>
        <label>{tr("Maximale Dateigröße, MiB","Maximum file size, MiB")}<input type="number" required min={1} max={256} value={value.max_file_bytes/(1024*1024)} onChange={event=>patch({max_file_bytes:Number(event.target.value)*1024*1024})}/></label>
        <label>{tr("Datenmenge pro Prüfung, MiB","Bytes per scan, MiB")}<input type="number" required min={1} max={1024} value={value.max_scan_bytes/(1024*1024)} onChange={event=>patch({max_scan_bytes:Number(event.target.value)*1024*1024})}/></label>
      </details>
    </>}
  </fieldset>;
}

export function FileWatchStatus({client,project,trigger}:{client:Client;project:string;trigger:string}){
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const status=useQuery({queryKey:["automation-file-watch",...workspaceManagementKey(client),project,trigger],queryFn:()=>client.one(`${automationPath(project)}/${encodeURIComponent(trigger)}/file-watch`,"AutomationFileWatchStatus"),retry:false,refetchInterval:3000});
  const value=status.data;
  return <article className="product-item"><h4>{tr("Dateibeobachtung","File observation")}</h4>{status.isError&&<p role="alert">{tr("Der Beobachtungsstand konnte nicht geladen werden.","Could not load observation status.")}</p>}{value&&<>
    <p role="status">{value.scanning?tr("Ordner wird geprüft …","Scanning folder …"):value.initialized?tr("Dateistand gespeichert.","File state saved."):tr("Wartet auf die erste vollständige Prüfung.","Waiting for the first complete scan.")}</p>
    {value.reason&&<p role="alert">{fileWatchMessage(value.reason,language)}</p>}
    <p>{tr("Beobachtete Dateien","Observed files")}: {value.tracked_files} · {tr("Warten auf stabilen Inhalt","Waiting for stable contents")}: {value.pending_changes}</p>
    {value.last_scan_at!=null&&<p>{tr("Letzte vollständige Prüfung","Last complete scan")}: {new Date(value.last_scan_at).toLocaleString(language)}</p>}
    {value.ignored_entries>0&&<p>{tr("Aus Sicherheits- oder Pfadgründen ausgelassene Einträge","Entries excluded for access or path reasons")}: {value.ignored_entries}</p>}
    <p className="muted">{tr("Die Überwachung vergleicht vollständige Dateistände. Zwischen zwei Prüfungen entstandene und wieder entfernte Dateien sind darin nicht enthalten. Nach einer Unterbrechung wird der zuletzt gespeicherte Stand weitergeführt.","Observation compares complete file states. Files created and removed between scans are not included. After an interruption, observation continues from the last saved state.")}</p>
  </>}</article>;
}
