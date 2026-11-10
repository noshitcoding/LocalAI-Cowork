import {useEffect,useRef,useState} from "react";
import RFB from "@novnc/novnc";
import {androidRequest} from "./androidDeviceApi";
import {useView} from "./view";
export function RemoteDeviceDesktop({endpoint,token,runId,onClose}:{endpoint:string,token:string,runId:string,onClose:()=>void}){
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const host=useRef<HTMLDivElement>(null),rfb=useRef<RFB|null>(null),session=useRef<string|null>(null),generation=useRef(0);const[error,setError]=useState(""),[status,setStatus]=useState(""),[password,setPassword]=useState(""),[busy,setBusy]=useState(false),[controlled,setControlled]=useState(false);
  const api=<T,>(path:string,method="GET",body?:unknown)=>androidRequest<T>(endpoint,token,path,method,body);
  async function connect(control:boolean){
    const current=generation.current;setBusy(true);setError("");
    try{
      rfb.current?.disconnect();rfb.current=null;setControlled(false);
      const sessions=await api<{id:string,state:string}[]>("/runs/"+runId+"/desktop-sessions");
      if(current!==generation.current)return;
      const id=sessions.find(v=>["starting","agent_controlled","user_controlled"].includes(v.state))?.id??(await api<{id:string}>("/runs/"+runId+"/desktop-sessions","POST",{width:1440,height:900})).id;
      if(current!==generation.current){await api("/runs/"+runId+"/desktop-sessions/"+id,"DELETE");return;}
      session.current=id;
      let grant:null|string=null;
      if(control){grant=(await api<{token:string}>("/auth/reauthenticate","POST",{password,purpose:"desktop_control"})).token;setPassword("");}
      if(current!==generation.current)return;
      const ticket=await api<{token:string}>("/runs/"+runId+"/desktop-sessions/"+id+"/tickets","POST",{control,reauthentication_token:grant});
      if(current!==generation.current)return;
      const url=new URL(endpoint+"/api/v1/desktop-sessions/"+id+"/stream",location.href);url.protocol=url.protocol==="https:"?"wss:":"ws:";url.searchParams.set("ticket",ticket.token);
      if(!host.current)throw new Error(tr("Desktopfläche fehlt.","Desktop surface unavailable."));
      const viewer=new RFB(host.current,url.toString());viewer.viewOnly=!control;viewer.scaleViewport=true;viewer.resizeSession=false;
      viewer.addEventListener("connect",()=>{if(current===generation.current){setStatus(control?tr("Steuerung verbunden","Control connected"):tr("Ansicht verbunden","View connected"));setControlled(control);}});
      viewer.addEventListener("disconnect",()=>{if(current===generation.current){setStatus(tr("Verbindung beendet","Connection ended"));setControlled(false);}});
      viewer.addEventListener("securityfailure",()=>{if(current===generation.current)setError(tr("Desktop-Verbindung abgewiesen.","Desktop connection refused."));});
      rfb.current=viewer;setStatus(tr("Warte auf lokale Gerätefreigabe und Rückverbindung …","Waiting for local device consent and reverse connection …"));
    }catch(e){if(current===generation.current)setError(e instanceof Error?e.message:tr("Desktop-Verbindung fehlgeschlagen.","Desktop connection failed."));}
    finally{if(current===generation.current)setBusy(false);}
  }
  useEffect(()=>{generation.current++;setStatus(tr("Bereit","Ready"));setPassword("");setBusy(false);return()=>{generation.current++;rfb.current?.disconnect();rfb.current=null;const id=session.current;session.current=null;if(id)void androidRequest(endpoint,token,"/runs/"+runId+"/desktop-sessions/"+id,"DELETE").catch(()=>{});};},[endpoint,token,runId]);
  async function close(){setBusy(true);setError("");rfb.current?.disconnect();rfb.current=null;setControlled(false);try{if(session.current)await api("/runs/"+runId+"/desktop-sessions/"+session.current,"DELETE");session.current=null;onClose();}catch(e){setError(e instanceof Error?e.message:tr("Sitzung konnte nicht geschlossen werden.","Could not close the session."));}finally{setBusy(false);}}
  return <section className="product-item" aria-label={tr("Geräte-Desktop","Device desktop")}><h4>{tr("Geräte-Desktop","Device desktop")}</h4><p role="status">{status}</p><p className="muted">{tr("Nur während einer gültigen Gerätelease. Ansicht und Steuerung werden getrennt lokal bestätigt. Mit „Desktop fokussieren“ gehen Tastatureingaben in den Desktop; Tab navigiert vorher durch diese Bedienelemente.","Only during a valid device lease. View and control receive separate local consent. Focus desktop to direct keyboard input to it; Tab navigates these controls first.")}</p><button disabled={busy} onClick={()=>void connect(false)}>{tr("Nur ansehen","View only")}</button><label>{tr("Passwort für frische Steuerungsfreigabe","Password for fresh control authorization")}<input type="password" autoComplete="current-password" value={password} onChange={e=>setPassword(e.target.value)}/></label><button disabled={busy||!password} onClick={()=>void connect(true)}>{tr("Steuerung ausdrücklich übernehmen","Explicitly take control")}</button><button disabled={!controlled} onClick={()=>rfb.current?.focus()}>{tr("Desktop fokussieren","Focus desktop")}</button><button disabled={busy} onClick={()=>void close()}>{tr("Sitzung schließen","Close session")}</button>{error&&<p role="alert">{error}</p>}<div ref={host} role="region" aria-label={tr("Entfernter Desktopbildschirm","Remote desktop screen")} style={{height:"65vh",minHeight:320,background:"#111"}}/></section>;
}
