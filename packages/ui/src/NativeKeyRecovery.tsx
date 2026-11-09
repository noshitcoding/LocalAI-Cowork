import {useState,useRef} from "react";
import type {UiLanguage} from "@cowork/client";
export function NativeKeyRecovery({language,revision,disabled,configured=true,refresh}:{language:UiLanguage,revision:number,disabled:boolean,configured?:boolean,refresh:()=>Promise<unknown>}){
  const de=language==="de",tr=(a:string,b:string)=>de?a:b;
  const [password,setPassword]=useState(""),[confirmation,setConfirmation]=useState(""),[busy,setBusy]=useState(false),[error,setError]=useState(""),[notice,setNotice]=useState("");
  type Key={instance:string,key:string};type Candidate={key:Key,orphan_operation_id?:string|null,retirement:{request_key:string,expected_revision:number}|null};
  const [cleanup,setCleanup]=useState<{revision:number,required_key_count:number,candidates:Candidate[],remaining:number}|null>(null);
  const intents=useRef(new Map<string,{id:string,revision:number}>());
  async function inspect(){setBusy(true);setError("");try{const {invoke}=await import("@tauri-apps/api/core");setCleanup(await invoke("native_keys_cleanup"));}catch(e){setError(String(e));}finally{setBusy(false);}}
  async function retire(candidate:Candidate){if(!cleanup||busy)return;setBusy(true);setError("");let intent=candidate.retirement?{id:candidate.retirement.request_key,revision:candidate.retirement.expected_revision}:intents.current.get(candidate.key.key);
    if(!intent){intent={id:crypto.randomUUID(),revision:cleanup.revision};intents.current.set(candidate.key.key,intent);}
    try{const {invoke}=await import("@tauri-apps/api/core");const result=await invoke(candidate.orphan_operation_id?"native_keys_retire_orphan":"native_keys_retire",{...(candidate.orphan_operation_id?{operationId:candidate.orphan_operation_id}:{key:candidate.key}),expectedRevision:intent.revision,idempotencyKey:intent.id,language});
      if(result){intents.current.delete(candidate.key.key);setCleanup(await invoke("native_keys_cleanup"));setNotice(tr("Unbenutzter Schlüssel entfernt.","Unused key removed."));await refresh();}}
    catch(e){setError(tr("Entfernen nicht bestätigt. Den ursprünglichen Vorgang erneut prüfen.","Removal unconfirmed. Reconcile the original operation.")+" "+String(e));}finally{setBusy(false);}}
  async function execute(action:"backup"|"restore"|"reencrypt"){
    if(busy)return;
    if(action!=="reencrypt"&&(password.length<12||(action==="backup"&&password!==confirmation))){setError(tr("Mindestens 12 Zeichen verwenden und das Sicherungspasswort bestätigen.","Use at least 12 characters and confirm the backup password."));return;}
    const entered=password;setPassword("");setConfirmation("");setBusy(true);setError("");setNotice("");
    try{
      const {invoke}=await import("@tauri-apps/api/core");
      const result=await invoke<Record<string,unknown>|null>("native_keys_"+action,{expectedRevision:revision,language,...(action==="reencrypt"?{}:{password:entered})});
      if(result){setNotice(tr("Vorgang bestätigt.","Operation confirmed."));await refresh();}
      else setNotice(tr("Abgebrochen.","Canceled."));
    }catch(e){setError(tr("Vorgang nicht bestätigt. Status aktualisieren und die ursprüngliche Sicherung erhalten. ","Operation not confirmed. Refresh status and retain the original backup. ")+String((e as {code?:string})?.code??"secrets_setup_uncertain"));}
    finally{setBusy(false);}
  }
  return <details className="product-item"><summary>{tr("Schlüssel sichern und wiederherstellen","Back up and recover keys")}</summary>
    <p>{tr("Die Sicherung ist passwortverschlüsselt und gehört zur ursprünglichen Datenbank. Sie enthält auch frühere Schlüssel für bestehende Daten. Bewahre Datenbank und Sicherung getrennt vom Passwort auf.","The password-encrypted backup belongs to the original database and includes earlier keys needed by existing data. Store the database and key backup separately from the password.")}</p>
    <label>{tr("Sicherungs-/Wiederherstellungspasswort","Backup/recovery password")}<input type="password" minLength={12} autoComplete="new-password" value={password} onChange={e=>setPassword(e.target.value)} disabled={busy}/></label>
    <label>{tr("Passwort für neue Sicherung bestätigen","Confirm password for a new backup")}<input type="password" autoComplete="new-password" value={confirmation} onChange={e=>setConfirmation(e.target.value)} disabled={busy}/></label>
    <div className="actions"><button disabled={disabled||busy||!configured} onClick={()=>void execute("backup")}>{tr("Verschlüsselte Sicherung speichern","Save encrypted backup")}</button><button disabled={disabled||busy||!configured} onClick={()=>void execute("restore")}>{tr("Ursprüngliche Schlüssel wiederherstellen","Recover original keys")}</button><button disabled={disabled||busy||!configured} onClick={()=>void execute("reencrypt")}>{tr("Bestehende Daten umschlüsseln","Re-encrypt existing data")}</button></div>
    <button disabled={disabled||busy} onClick={()=>void inspect()}>{tr("Unbenutzte frühere Schlüssel prüfen","Inspect unused earlier keys")}</button>
    {cleanup&&<div><p>{tr("Benötigte Schlüssel:","Required keys:")} {cleanup.required_key_count} · {tr("Entfernbare Schlüssel:","Retirable keys:")} {cleanup.remaining}</p>
      {cleanup.candidates.map(candidate=><p key={candidate.key.key}><code>{candidate.key.key}</code> {candidate.orphan_operation_id&&<span>{tr("Aufgegebene Einrichtung ohne Aktivierungsbeleg","Abandoned setup without activation proof")}</span>} <button disabled={disabled||busy} onClick={()=>void retire(candidate)}>{candidate.retirement?tr("Entfernen klären","Reconcile removal"):tr("Diesen Schlüssel entfernen","Remove this key")}</button></p>)}</div>}
    {busy&&<p role="status">{tr("Vorgang läuft…","Operation in progress…")}</p>}{notice&&<p role="status">{notice}</p>}{error&&<p role="alert">{error}</p>}
  </details>;
}
