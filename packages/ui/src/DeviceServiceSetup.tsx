import {useView} from "./view";
import {teamText} from "./teamStrings";
import {useEffect,useState,type FormEvent} from "react";
import {Client} from "@cowork/client";
import {productRequest} from "./productApi";
import {AuthManagementSetup} from "./AuthManagementSetup";

type Configuration={revision:number,origin:string|null,rp_id:string|null,passkeys_enabled:boolean,fcm_enabled:boolean,fcm_project:string|null,fleet_enabled:boolean,organisation_id:string|null,instance_id:string|null,configuration_source:string};
export function DeviceServiceSetup({client}:{client:Client}) {
  const language=useView(v=>v.language),tr=(value:string)=>teamText(value,language);
  const [config,setConfig]=useState<Configuration>();
  const [error,setError]=useState<unknown>();
  const [busy,setBusy]=useState(false);
  const [checked,setChecked]=useState<Record<string,string>>();
  const [origin,setOrigin]=useState("");const [rp,setRp]=useState("");
  async function reload(){try{const value=await productRequest<Configuration>(client,"/operator/devices");setConfig(value);setOrigin(value.origin??"");setRp(value.rp_id??"");setError(undefined);}catch(e){setError(e);}}
  useEffect(()=>{void reload();},[client.base,client.token]);
  async function save(e:FormEvent<HTMLFormElement>){
    e.preventDefault();if(!config)return;const form=e.currentTarget;const values=new FormData(form);setBusy(true);setChecked(undefined);
    try {
      const file=values.get("fcm") as File|null;
      if(file&&file.size>32768)throw new Error(tr("Die FCM-Datei darf höchstens 32 KB enthalten."));
      const account=file&&file.size?await file.text():null;
      const updated=await productRequest<Configuration>(client,"/operator/devices","PUT",{expected_revision:config.revision,origin:origin.trim()||null,rp_id:rp.trim()||null,fcm_account:account,clear_fcm:values.get("clear_fcm")==="on",fleet_key:String(values.get("fleet_key")??"").trim()||null,clear_fleet:values.get("clear_fleet")==="on",organisation_id:String(values.get("organisation_id")??"").trim()||null,instance_id:String(values.get("instance_id")??"").trim()||null});
      setConfig(updated);setError(undefined);(form.elements.namedItem("fleet_key") as HTMLInputElement).value="";(form.elements.namedItem("fcm") as HTMLInputElement).value="";
    }catch(e){setError(e);}finally{setBusy(false);}
  }
  async function check(){setBusy(true);try{setChecked(await productRequest(client,"/operator/devices/check","POST",{}));setError(undefined);}catch(e){setError(e);}finally{setBusy(false);}}
  const status=(enabled:boolean)=>enabled?tr("Eingerichtet"):tr("Noch nicht eingerichtet");
  return <section><h2>{tr("Gerätedienste einrichten")}</h2><p>{tr("Diese Einstellungen gelten für diese Serverinstallation. Speichern aktiviert sie sofort und verschlüsselt Zugangsdaten im geschützten Serverspeicher.")}</p>
    {!!error&&<p role="alert" className="error">{error instanceof Error?error.message:String(error)}</p>}
    {config?<><div className="product-columns"><form onSubmit={save} key={config.revision}>
      <h3>Passkeys</h3><p>{status(config.passkeys_enabled)}</p>
      <label>{tr("Öffentliche HTTPS-Adresse")}<input type="url" value={origin} placeholder="https://cowork.example.com" onChange={e=>{const next=e.target.value;setOrigin(next);try{setRp(new URL(next).hostname);}catch{}}}/></label>
      <label>{tr("RP-Hostname")}<input value={rp} onChange={e=>setRp(e.target.value)} placeholder="cowork.example.com"/></label><p className="muted">{tr("Die Adresse muss exakt mit der Adresse im Browser übereinstimmen. HTTP ist für localhost möglich. Anschließend im Team unter Geräte einen Passkey registrieren.")}</p>
      <h3>{tr("Android-Push über Firebase")}</h3><p>{status(config.fcm_enabled)}{config.fcm_project?" · "+config.fcm_project:""}</p>
      <label>{tr("Firebase-Servicekonto (JSON)")}<input name="fcm" type="file" accept="application/json,.json"/></label><p className="muted">{tr("Eine neue Datei ersetzt das Servicekonto. Ohne Datei bleibt das vorhandene Konto erhalten.")}</p>
      <label className="product-check"><input name="clear_fcm" type="checkbox"/>{tr("Push deaktivieren und Servicekonto entfernen")}</label>
      <h3>{tr("Fleet-Anmeldung")}</h3><p>{status(config.fleet_enabled)}</p>
      <label>{tr("Organisations-ID")}<input name="organisation_id" defaultValue={config.organisation_id??""} placeholder="UUID"/></label>
      <label>{tr("Instanz-ID")}<input name="instance_id" defaultValue={config.instance_id??""} placeholder="UUID"/></label>
      <label>{tr("Gemeinsamer Fleet-Schlüssel")}<input name="fleet_key" type="password" autoComplete="off" minLength={32} maxLength={4096} placeholder={config.fleet_enabled?tr("Zum Ersetzen neuen Schlüssel eingeben"):tr("Mindestens 32 Zeichen")}/></label>
      <label className="product-check"><input name="clear_fleet" type="checkbox"/>{tr("Fleet-Anmeldung deaktivieren und Schlüssel entfernen")}</label>
      <button className="primary" disabled={busy}>{tr("Speichern und aktivieren")}</button></form>
      <div><h3>{tr("Verbindung prüfen")}</h3><p>{tr("Prüft die gespeicherte Passkey-Konfiguration und die echte Firebase-OAuth-Anmeldung. Geräte werden im Team gekoppelt; dort lassen sich Aufträge und Fernsteuerung starten.")}</p><button disabled={busy} onClick={()=>void check()}>{tr("Konfiguration prüfen")}</button>{checked&&<dl>{Object.entries(checked).filter(([key])=>!['schema_version','revision'].includes(key)).map(([key,value])=><div key={key}><dt>{key}</dt><dd>{String(value)}</dd></div>)}</dl>}<p className="muted">{tr("Fernsteuerung erscheint bei einem verbundenen Geräteagenten mit der passenden Fähigkeit desktop.windows oder desktop.linux. Die Freigabe gilt jeweils für den gewählten Auftrag und Lauf.")}</p></div>
    </div></>:<button onClick={()=>void reload()}>{tr("Einstellungen laden")}</button>}
    <AuthManagementSetup client={client}/>
  </section>;
}
