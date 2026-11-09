import type {AutomationFileWatch,FolderGrant,WorkspaceTask} from "@cowork/client";

export function sameWatchFolder(a:FolderGrant,b:FolderGrant):boolean {
  return !a.order_id&&!b.order_id&&a.id===b.id&&a.project_id===b.project_id&&a.revision===b.revision&&a.resource_id===b.resource_id&&a.name===b.name;
}
export function eligibleWatchFolders(task:WorkspaceTask|undefined,current:FolderGrant[]):FolderGrant[]{
  return current.filter(folder=>!folder.order_id&&task?.value.folders?.some(selected=>sameWatchFolder(selected,folder))).map(folder=>({...folder,writable:false}));
}
export function defaultFileWatch(folder:FolderGrant):AutomationFileWatch {
  return {folder:{...folder,writable:false},path:".",recursive:false,changes:["created"],extensions:[],exclude_paths:[],include_existing:false,debounce_ms:2000,poll_interval_ms:10000,max_files:2000,max_file_bytes:64*1024*1024,max_scan_bytes:256*1024*1024};
}
export const watchExtensions=(value:string)=>value.split(",").map(value=>value.trim().toLowerCase()).filter(Boolean).map(value=>value.startsWith(".")?value:"."+value);
export function fileWatchMessage(reason:string|null|undefined,language:"de"|"en"):string {
  const messages:Record<string,[string,string]>={
    automation_file_watch_invalid:["Prüfe Unterordner, Dateitypen, mindestens einen Ereignistyp und die Dateigrenzen.","Review the subfolder, file types, at least one event type and the file limits."],
    automation_watch_state_limit:["Der gespeicherte Dateistand wird zu groß. Verkleinere den überwachten Bereich.","The saved file state is too large. Narrow the observed scope."],
    automation_watch_entries_unavailable:["Einige Pfade sind Links oder keine zugelassenen regulären Dateien. Ihr bisheriger Stand bleibt erhalten; sie gelten dadurch nicht als gelöscht.","Some paths are links or unsupported entries. Their previous state is retained; they are not treated as deleted."],
    trigger_paused:["Der Auslöser ist pausiert.","The trigger is paused."],
    task_changed:["Die Aufgabe wurde geändert oder archiviert. Binde die aktuelle Aufgabenrevision erneut.","The task changed or was archived. Bind the current revision again."],
    chat_unavailable:["Der zugehörige Chat ist nicht verfügbar.","The associated chat is unavailable."],
    automation_watch_folder_unavailable:["Die Ordnerfreigabe ist nicht mehr verfügbar. Prüfe den Zugriff und binde den Ordner erneut.","Folder access is unavailable. Review access and bind the folder again."],
    automation_watch_tools_unavailable:["Am verbundenen Daemon ist kein Dateizugriff eingerichtet.","File access is not configured on the connected daemon."],
    automation_watch_folder_busy:["Der Ordner wird gerade verwendet. Die Prüfung wird erneut versucht.","The folder is busy. Observation will be retried."],
    automation_watch_file_changed_during_scan:["Während der Prüfung wurden Dateien geändert. Die Beobachtung wird wiederholt.","Files changed during observation. The scan will be repeated."],
    automation_watch_file_too_large:["Eine ausgewählte Datei überschreitet die Dateigröße. Passe den Bereich, die Dateitypen oder die Grenze an.","A selected file exceeds the size limit. Adjust the scope, file types or limit."],
    automation_watch_byte_limit:["Der Bereich überschreitet die Datenmenge pro Prüfung. Verkleinere den Bereich oder passe die Grenze an.","The scan exceeds its byte budget. Narrow the scope or adjust the limit."],
    automation_watch_file_limit:["Der Bereich enthält zu viele ausgewählte Dateien. Verkleinere den Bereich oder passe die Dateigrenze an.","The scope contains too many selected files. Narrow the scope or adjust the file limit."],
    automation_watch_entry_limit:["Der Bereich enthält zu viele Einträge. Wähle einen kleineren Unterordner.","The scope contains too many entries. Choose a smaller subfolder."],
    automation_watch_directory_limit:["Der Bereich enthält zu viele oder zu tief verschachtelte Ordner.","The scope contains too many directories or exceeds the depth limit."],
    automation_watch_scan_timeout:["Die vollständige Prüfung dauert zu lange. Verkleinere den Bereich.","The full scan takes too long. Narrow its scope."],
    automation_watch_clock_rollback:["Die Systemzeit liegt vor der letzten gespeicherten Beobachtung. Neue Ereignisse warten auf eine gültige Zeit.","The system clock is behind the last saved observation. New events wait for a valid time."],
    automation_event_retention_limit:["Das Ereignisarchiv hat seine Grenze erreicht. Die Dateibeobachtung bleibt erhalten.","The event archive reached its limit. The file observation is retained."],
    automation_watch_configuration_changed:["Die Konfiguration wurde während der Prüfung geändert. Die aktuelle Version wird erneut geprüft.","The configuration changed during observation. The current version will be scanned again."],
    automation_watch_worker_failed:["Die Dateiprüfung wurde unterbrochen. Die gespeicherte Beobachtung bleibt erhalten.","The file scan was interrupted. The saved observation is retained."],
    automation_watch_io_failed:["Der Ordner konnte nicht vollständig gelesen werden. Es wurden keine Löschereignisse daraus abgeleitet.","The folder could not be fully read. No removal events were inferred."],
  };
  return reason?(messages[reason]?.[language==="de"?0:1]??(language==="de"?"Die Dateiprüfung benötigt Aufmerksamkeit.":"File observation requires attention.")):"";
}
