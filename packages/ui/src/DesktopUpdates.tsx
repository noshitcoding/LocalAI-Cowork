import {useEffect,useState} from 'react';
import {useView} from './view';
import {version} from '../../../package.json';

type UpdateInfo={configured:boolean;current_version:string;update_id:string|null;version:string|null;notes:string|null};
type Attempt={attempt:string;from:string;to:string;state:string;can_reconcile:boolean};
type Status={blocked:boolean;current_version:string;attempts:Attempt[];journal_error:string|null};
export function DesktopUpdates(){
 const language=useView(v=>v.language),de=language==='de',native='__TAURI_INTERNALS__'in window;
 const [info,setInfo]=useState<UpdateInfo|null>(null),[status,setStatus]=useState<Status|null>(null),[busy,setBusy]=useState(false),[error,setError]=useState<string|null>(null);
 async function refresh(){const {invoke}=await import('@tauri-apps/api/core');setStatus(await invoke<Status>('desktop_update_status'));}
 useEffect(()=>{if(native)void refresh().catch(e=>setError(String(e)));},[native]);
 if(!native)return <section className="product-card"><h2>{de?'App aktualisieren':'Update application'}</h2><p>{de?'Version der Weboberfläche: ':'Web interface version: '}{version}</p><p>{de?'Updates dieses Arbeitsbereichs verwaltet der Betreiber auf dem Server. Öffne die installierte Desktop-App, um deren Updatekanal zu prüfen.':'The operator manages workspace updates on the server. Open the installed desktop application to inspect its update channel.'}</p></section>;
 async function action(work:()=>Promise<void>){if(busy)return;setBusy(true);setError(null);try{await work();}catch(e){setError(String(e));}finally{try{await refresh();}catch(e){setError(String(e));}setBusy(false);}}
 async function check(){const {invoke}=await import('@tauri-apps/api/core');setInfo(await invoke<UpdateInfo>('desktop_update_check'));}
 async function install(){if(!info?.update_id)return;const {invoke}=await import('@tauri-apps/api/core');await invoke('desktop_update_install',{updateId:info.update_id});}
 async function reconcile(attempt:string){const {invoke}=await import('@tauri-apps/api/core');setStatus(await invoke<Status>('desktop_update_reconcile',{attempt}));setInfo(null);}
 function stateText(state:string){const text:Record<string,[string,string]>={
  'original-daemon-exit-unconfirmed':['Das Ende des ursprünglichen Daemons ist noch nicht bestätigt.','The original daemon’s exit is not yet confirmed.'],
  'daemon-stopped-install-not-started':['Der ursprüngliche Daemon ist beendet. Die Installation wurde nicht gestartet.','The original daemon exited. Installation was not started.'],
  'installer-failed':['Die Installation ist fehlgeschlagen. Der ursprüngliche Vorgang bleibt erhalten.','Installation failed. The original attempt is retained.'],
  'installer-outcome-unconfirmed':['Der Abschluss des externen Installers ist noch nicht bestätigt.','The external installer’s outcome is not yet confirmed.'],
  'installed-target-version-not-running':['Der Installer ist beendet. Öffne die installierte Zielversion, um dieses Update abzuschließen.','The installer exited. Open the installed target version to complete this update.'],
 };return text[state]?.[de?0:1]??state;}
 return <section className="product-card"><h2>{de?'App aktualisieren':'Update application'}</h2>
 <button disabled={busy} onClick={()=>void action(refresh)}>{de?'Updatevorgänge prüfen':'Inspect update attempts'}</button>
 {status?.blocked&&<p role="status">{de?'Der lokale Daemonstart bleibt bis zur Klärung des gespeicherten Updates angehalten.':'Starting the local daemon remains paused until the recorded update is resolved.'}</p>}
 {status?.journal_error&&<p role="alert">{de?'Die Updatebelege können nicht sicher gelesen werden.':'The update receipts cannot be read safely.'} {status.journal_error}</p>}
 {status?.attempts.map(attempt=><article className="product-item" key={attempt.attempt}>
  <p>{attempt.from} → {attempt.to}</p><p>{stateText(attempt.state)}</p>
  {attempt.can_reconcile&&<><p>{de?'Der Abgleich startet keine Installation. Suche danach bei Bedarf erneut nach Updates oder öffne den lokalen Arbeitsbereich.':'Reconciliation does not start installation. Afterwards, check for updates again or open the local workspace.'}</p><button disabled={busy} onClick={()=>void action(()=>reconcile(attempt.attempt))}>{de?'Belegten Vorgang abschließen':'Resolve the confirmed attempt'}</button></>}
  <details><summary>{de?'Vorgangskennung':'Attempt ID'}</summary><code>{attempt.attempt}</code></details>
 </article>)}
 <button disabled={busy||status===null||status.blocked} onClick={()=>void action(check)}>{de?'Nach Updates suchen':'Check for updates'}</button>
 {info&&<p role="status">{info.current_version} · {!info.configured?(de?'Für dieses Paket ist kein Updatekanal eingerichtet.':'This package has no configured update channel.'):info.version?(de?'Verfügbar: ':'Available: ')+info.version:(de?'Aktuell':'Up to date')}</p>}
 {info?.update_id&&!status?.blocked&&<><p>{info.notes}</p><p>{de?'Beende laufende Läufe und Terminalsitzungen, bevor du die neue Version installierst. Die App schließt sich für die Installation; öffne sie anschließend erneut.':'Finish active runs and terminal sessions before installing the new version. The app closes for installation; reopen it afterwards.'}</p><button disabled={busy} onClick={()=>void action(install)}>{de?'Signiertes Update herunterladen und installieren':'Download and install signed update'}</button></>}
 {busy&&<p role="status">{de?'Aktualisierung läuft …':'Updating …'}</p>}{error&&<p role="alert">{error}</p>}
 </section>;
}
