import {useRef,useState} from "react";
import {browserSession,securityRequest,securityOperation,securityOperationConfirmed,type SecurityTokens} from "./androidSecurityApi";
import {useView} from "./view";
export function BrowserSessionResume({endpoint,onSession}:{endpoint:string,onSession:(value:SecurityTokens)=>void}){
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en,[busy,setBusy]=useState(false),[error,setError]=useState("");
  const latest=useRef(endpoint);latest.current=endpoint;
  if(!browserSession(endpoint))return null;
  const purpose="browser-refresh",binding="same-origin-cookie";
  async function resume(){setBusy(true);setError("");try{const operation=securityOperation(endpoint,purpose,binding),value=await securityRequest<SecurityTokens>(endpoint,"","/auth/browser/refresh","POST",undefined,operation);if(latest.current!==endpoint)return;securityOperationConfirmed(endpoint,purpose,binding);onSession(value);}catch(e){if(latest.current===endpoint)setError(e instanceof Error?e.message:String(e));}finally{if(latest.current===endpoint)setBusy(false);}}
  return <section className="product-item"><h3>{tr("Gespeicherte Browsersitzung","Saved browser session")}</h3><p>{tr("Die Sitzung wird über das geschützte Cookie dieses Servers fortgesetzt. Der ursprüngliche Vorgang bleibt bei einer unterbrochenen Antwort erhalten.","Continue using this server's protected cookie. The original operation is retained if its response is interrupted.")}</p>{error&&<p role="alert">{error}</p>}<button disabled={busy} onClick={()=>void resume()}>{tr("Sitzung fortsetzen","Continue session")}</button>{error&&<button disabled={busy} onClick={()=>{securityOperationConfirmed(endpoint,purpose,binding);setError("");}}>{tr("Lokalen Vorgangsbeleg entfernen","Remove local operation receipt")}</button>}</section>;
}
