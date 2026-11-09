import {useTeamText} from "./teamStrings";
import {useEffect,useMemo,useRef,useState,type FormEvent} from "react";
import {Client} from "@cowork/client";
import {productRequest} from "./productApi";

export type GithubRepositoryRun={id:string,order_id:string,status:string,response:string|null,error:string|null,model:string,updated_at:number};
type GithubConfiguration={hostname:string,enabled:boolean,key_configured:boolean};
type Operation="clone"|"checkout"|"fetch"|"pull"|"sync"|"status"|"push";
type RepositoryAction={operation:Operation,repository:string,directory:string,reference?:string,depth?:number,source_commit?:string};

export function TeamGithubRepositoryPanel({client,session,team,order,enabled,onRun}:{client:Client,session:string,team:string,order:string,enabled:boolean,onRun:(run:GithubRepositoryRun)=>void}){
  const tr=useTeamText();
  const [configuration,setConfiguration]=useState<GithubConfiguration|null>(null);
  const [operation,setOperation]=useState<Operation>("clone");
  const [repository,setRepository]=useState("");const [directory,setDirectory]=useState("repository");
  const [sourceCommit,setSourceCommit]=useState("");const [reference,setReference]=useState("");const [depth,setDepth]=useState("");
  const [busy,setBusy]=useState(false);const [error,setError]=useState<unknown>();
  const intent=useRef<{binding:string,key:string}|null>(null);
  const binding=client.base+":"+session+":"+team+":"+order;
  const activeBinding=useRef(binding);activeBinding.current=binding;
  const alive=useRef(true);useEffect(()=>{alive.current=true;return()=>{alive.current=false;};},[]);
  useEffect(()=>{let current=true;setConfiguration(null);setRepository("");setReference("");setDepth("");setError(undefined);intent.current=null;if(session&&team)void productRequest<GithubConfiguration|null>(client,`/teams/${team}/github`,"GET",undefined,session).then(value=>{if(current)setConfiguration(value);}).catch(value=>{if(current)setError(value);});return()=>{current=false;};},[client.base,session,team]);
  const action=useMemo<RepositoryAction>(()=>({operation,repository,directory,...(operation!=="status"&&reference?{reference}:{}),...(operation==="clone"&&depth?{depth:Number(depth)}:{}),...(operation==="push"?{source_commit:sourceCommit}:{})}),[operation,repository,directory,reference,depth,sourceCommit]);
  async function submit(event:FormEvent<HTMLFormElement>){
    event.preventDefault();const captured=binding;setBusy(true);setError(undefined);
    try{
      const parsed=new URL(repository);
      if(parsed.protocol!=="https:"||parsed.hostname!==configuration?.hostname||parsed.username||parsed.password||parsed.search||parsed.hash||parsed.port)throw new Error(tr("Die Repository-URL muss den eingestellten HTTPS-GitHub-Host verwenden."));
      if(!directory||directory.startsWith("/")||/[\\:\u0000-\u001f\u007f]/.test(directory)||directory.split("/").some(part=>!part||[".","..",".git"].includes(part)))throw new Error(tr("Bitte einen relativen Repositoryordner im Auftragsworkspace angeben."));
      if(operation==="push"&&(!reference||!/^(?:[a-fA-F0-9]{40}|[a-fA-F0-9]{64})$/.test(sourceCommit)))throw new Error(tr("Push benötigt den exakten HEAD-Commit und Zielbranch."));if(operation==="checkout"&&!reference)throw new Error(tr("Checkout benötigt einen Branch, Tag oder Commit."));
      const prompt="Führe genau die folgende GitHub-Repositoryaktion mit dem Werkzeug github aus. Verwende die strukturierten Argumente unverändert. Fordere die konkrete Freigabe an und warte darauf. Führe keine weitere GitHub- oder Terminalaktion aus und wiederhole fehlgeschlagene oder unklare Aktionen nicht. Berichte den tatsächlichen Git-Exitcode und Commit. Aktion:\n"+JSON.stringify(action);
      const plan={workflow:null,tools:["github"],skills:[],knowledge:[]};
      const key=JSON.stringify({binding,action});if(intent.current?.binding!==key)intent.current={binding:key,key:crypto.randomUUID()};
      const value=await productRequest<GithubRepositoryRun>(client,`/teams/${team}/runs`,"POST",{order_id:order,prompt,idempotency_key:intent.current.key,plan},session);
      if(alive.current&&captured===activeBinding.current){intent.current=null;onRun(value);}
    }catch(value){if(alive.current&&captured===activeBinding.current)setError(value);}finally{if(alive.current&&captured===activeBinding.current)setBusy(false);}
  }
  return <details className="product-item"><summary>{tr("GitHub-Repository im Team bearbeiten")}</summary><p className="muted">{tr("Der Teamagent verwendet ausschließlich den verschlüsselten Zugang dieses Teams. Jede konkrete Git-Aktion erscheint vor der Ausführung als Freigabe im Verlauf dieses Auftrags.")}</p>{!!error&&<p className="error" role="alert">{error instanceof Error?error.message:String(error)}</p>}<form onSubmit={submit}><fieldset disabled={busy||!enabled||!configuration?.enabled||!configuration.key_configured||!order}><label>{tr("Aktion")}<select value={operation} onChange={event=>setOperation(event.target.value as Operation)}><option value="clone">{tr("Clone – Repository herunterladen")}</option><option value="checkout">{tr("Checkout – Revision auswählen")}</option><option value="fetch">{tr("Fetch – Remote-Stand laden")}</option><option value="sync">{tr("Sync – sicher vorwärts aktualisieren")}</option><option value="pull">{tr("Pull – nur Fast-forward")}</option><option value="push">{tr("Push – freigegebenen Commit hochladen")}</option><option value="status">{tr("Status – Arbeitsstand anzeigen")}</option></select></label><label>{tr("Repository-URL")}<input type="url" value={repository} onChange={event=>setRepository(event.target.value)} required placeholder={`https://${configuration?.hostname??"github.com"}/owner/repository.git`}/></label><label>{tr("Ordner in diesem Auftrag")}<input value={directory} onChange={event=>setDirectory(event.target.value)} required maxLength={4096}/></label>{operation!=="status"&&<label>{tr("Branch, Tag oder Commit")}<input value={reference} onChange={event=>setReference(event.target.value)} required={operation==="checkout"||operation==="push"} maxLength={240} placeholder={operation==="checkout"?tr("main oder Commit-ID"):tr("Leer: aktueller Branch")}/></label>}{operation==="push"&&<label>{tr("Exakter HEAD-Commit")}<input value={sourceCommit} onChange={event=>setSourceCommit(event.target.value)} required pattern="[a-fA-F0-9]{40}|[a-fA-F0-9]{64}" maxLength={64}/></label>}{operation==="clone"&&<label>{tr("Clone-Tiefe")}<input type="number" min={1} max={1000} value={depth} onChange={event=>setDepth(event.target.value)} placeholder={tr("Leer: vollständiger Verlauf")}/></label>}<pre>{JSON.stringify(action,null,2)}</pre><button className="primary" disabled={busy}>{tr("Aktion beim Teamagenten anfordern")}</button></fieldset></form><p className="muted">{tr("Push überträgt ausschließlich den angezeigten bereits committeten HEAD in den Zielbranch, ohne Force oder automatischen Retry. Clone legt einen neuen Ordner an. Weitere Aktionen verwenden ein mit diesem Werkzeug geklontes Repository. Checkout einer expliziten Revision verwendet einen detached HEAD; Pull und Sync überschreiben weder lokale Änderungen noch abweichenden Verlauf. Für einen detached HEAD bei Sync den gewünschten Branch ausdrücklich angeben.")}</p>{(!enabled||!configuration?.enabled||!configuration.key_configured)&&<p className="muted">{tr("Zuerst LiteLLM und den Team-GitHub-Zugang unter Einstellungen und Integrationen einrichten sowie das Teamwerkzeug GitHub verfügbar machen.")}</p>}</details>;
}
