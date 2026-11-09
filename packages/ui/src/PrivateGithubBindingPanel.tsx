import {useEffect,useRef,useState,type FormEvent} from "react";
import type {Client} from "@cowork/client";
import {githubIntent,githubUuid,privateGithubBindings,privateGithubMutate,privateGithubProjects,type PrivateGithubBinding,type PrivateGithubFields,type PrivateGithubIntent,type PrivateGithubProject} from "./privateGithubApi";
import {deliverDownload} from "./download";
import {useView} from "./view";

const empty:PrivateGithubFields={expected_revision:null,name:"",git_origin:"https://github.com",api_root:"https://api.github.com/",owner:"",repository:"",author_name:"",author_email:""};
type Props={client:Client;session:string;actorId:string;projectId?:string;selectedBindingId?:string;selectionDisabled?:boolean;onSelect?:(projectId:string,bindingId:string)=>void};
type Pending={intent:PrivateGithubIntent;token?:string};

export function PrivateGithubBindingPanel({client,session,actorId,projectId,selectedBindingId,selectionDisabled=false,onSelect}:Props){
  const language=useView(v=>v.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const [projects,setProjects]=useState<PrivateGithubProject[]>([]),[project,setProject]=useState(projectId??""),[bindings,setBindings]=useState<PrivateGithubBinding[]>([]),[editing,setEditing]=useState<string>(),[fields,setFields]=useState<PrivateGithubFields>({...empty}),[saved,setSaved]=useState<PrivateGithubIntent>(),[receiptInvalid,setReceiptInvalid]=useState(false),[busy,setBusy]=useState(false),[error,setError]=useState(""),[notice,setNotice]=useState("");
  const generation=useRef(0),pending=useRef<Pending|undefined>(undefined),form=useRef<HTMLFormElement>(null);
  const storageKey="cowork.private_github.original.v1:"+JSON.stringify([client.base,actorId]);
  const message=(value:unknown)=>{
    const code=value instanceof Error?value.message:String(value);
    const messages:Record<string,[string,string]>={
      github_original_unconfirmed:["Die Bestätigung fehlt. Rufe denselben Originalvorgang erneut ab; beginne keinen neuen Speichervorgang.","Acknowledgement is missing. Retrieve the same original operation; do not begin another save."],
      github_original_invalid:["Der lokal gespeicherte Originalvorgang ist ungültig.","The recorded original operation is invalid."],
      github_response_invalid:["Der Serverbeleg passt nicht zu diesem Konto, Projekt oder Originalauftrag.","The server receipt does not match this account, project or original request."],
      github_original_pending:["Bestätige zuerst den gespeicherten Originalvorgang oder verwerfe bewusst seine lokale Referenz.","Confirm the recorded original operation first, or explicitly discard its local reference."],
      github_connection_failed:["Die Serververbindung ist nicht verfügbar.","The server connection is unavailable."],
      github_session_required:["Melde dich zuerst mit deinem eigenen Konto an.","Sign in with your own account first."],
      access_denied:["Diese Sitzung darf den privaten Zugang nicht verwalten.","This session may not manage the private credential."],
      forbidden:["Diese Sitzung darf den privaten Zugang nicht verwalten.","This session may not manage the private credential."],
      request_conflict:["Profilrevision oder Originaleingaben stimmen nicht überein. Prüfe den gespeicherten Vorgang.","Profile revision or original input differs. Review the recorded operation."],
      conflict:["Profilrevision oder Originaleingaben stimmen nicht überein. Prüfe den gespeicherten Vorgang.","Profile revision or original input differs. Review the recorded operation."],
    };
    return messages[code]?.[language==="de"?0:1]??tr("Vorgang fehlgeschlagen: ","Operation failed: ")+code;
  };
  async function attempt(action:(current:number)=>Promise<void>){const current=generation.current;setBusy(true);setError("");setNotice("");try{await action(current);}catch(e){if(current===generation.current)setError(message(e));}finally{if(current===generation.current)setBusy(false);}}
  async function reload(current=generation.current,p=project){if(!p)return;const values=await privateGithubBindings(client,session,actorId,p);if(current===generation.current&&p===project)setBindings(values);}
  useEffect(()=>{
    const current=++generation.current;pending.current=undefined;form.current?.reset();setProjects([]);setBindings([]);setProject(projectId??"");setEditing(undefined);setFields({...empty});setSaved(undefined);setReceiptInvalid(false);setError("");setNotice("");setBusy(false);
    if(!session||!githubUuid(actorId))return;
    try{const raw=localStorage.getItem(storageKey);if(raw){const original=githubIntent(JSON.parse(raw));setSaved(original);if(!projectId||projectId===original.project_id){setProject(original.project_id);if(!original.remove){setFields(original.fields as PrivateGithubFields);setEditing(original.fields.expected_revision===null?undefined:original.binding_id);}}}}
    catch(e){setReceiptInvalid(true);setError(message(e));}
    void privateGithubProjects(client,session).then(values=>{if(current===generation.current)setProjects(values);}).catch(e=>{if(current===generation.current)setError(message(e));});
    return()=>{generation.current++;pending.current=undefined;form.current?.reset();};
  },[client.base,session,actorId,projectId]);
  useEffect(()=>{let canceled=false;const current=generation.current;setBindings([]);if(!project||!session)return;void privateGithubBindings(client,session,actorId,project).then(values=>{if(!canceled&&current===generation.current)setBindings(values);}).catch(e=>{if(!canceled&&current===generation.current)setError(message(e));});return()=>{canceled=true;};},[client.base,session,actorId,project]);
  function edit(row:PrivateGithubBinding){if(busy||saved)return;form.current?.reset();setEditing(row.id);setFields({expected_revision:row.revision,name:row.name,git_origin:row.git_origin,api_root:row.api_root,owner:row.owner,repository:row.repository,author_name:row.author_name,author_email:row.author_email});setError("");}
  async function execute(intent:PrivateGithubIntent,token:string|undefined,current:number){
    if(receiptInvalid)throw new Error("github_original_invalid");
    if(saved&&JSON.stringify(saved)!==JSON.stringify(intent))throw new Error("github_original_pending");
    if(pending.current&&(JSON.stringify(pending.current.intent)!==JSON.stringify(intent)||token!==pending.current.token))throw new Error("github_original_pending");
    // Only the immutable nonsecret request reference is stored across reloads. PAT remains in memory.
    localStorage.setItem(storageKey,JSON.stringify(intent));pending.current={intent,token};setSaved(intent);
    await privateGithubMutate(client,session,actorId,intent,token);
    if(current!==generation.current)return;
    localStorage.removeItem(storageKey);pending.current=undefined;setSaved(undefined);setEditing(undefined);setFields({...empty});form.current?.reset();
    setNotice(tr("Originalvorgang bestätigt. Wähle den Zugang anschließend ausdrücklich für einen neuen Lauf.","Original operation confirmed. Explicitly select the credential for a new run afterwards."));
    await reload(current,intent.project_id);
  }
  function submit(e:FormEvent<HTMLFormElement>){
    e.preventDefault();const token=String(new FormData(e.currentTarget).get("token")??"");e.currentTarget.reset();
    if(!project||busy)return;
    const intent=saved??{format:1 as const,operation_id:crypto.randomUUID(),project_id:project,binding_id:editing??crypto.randomUUID(),remove:false,fields:{...fields}};
    if(intent.remove||intent.project_id!==project||JSON.stringify(intent.fields)!==JSON.stringify(fields)){setError(message(new Error("github_original_pending")));return;}
    void attempt(current=>execute(intent,token,current));
  }
  async function reference(row:PrivateGithubBinding,current:number){
    const value={format:1,actor_id:actorId,project_id:project,github_binding_id:row.id,github_binding_revision:row.revision,binding_sha256:row.binding_sha256,repository:`${row.git_origin}/${row.owner}/${row.repository}.git`};
    const result=await deliverDownload(new Blob([JSON.stringify(value,null,2)+"\n"],{type:"application/json"}),`private-github-reference-${row.id}.json`,language);
    if(current===generation.current)setNotice(result==="canceled"?tr("Speichern abgebrochen.","Save canceled."):tr("Referenz gespeichert. Sie enthält keinen Token.","Reference saved. It contains no token."));
  }
  function resetOriginal(){localStorage.removeItem(storageKey);pending.current=undefined;setSaved(undefined);setReceiptInvalid(false);setEditing(undefined);setFields({...empty});form.current?.reset();setError("");setNotice(tr("Die lokale Referenz wurde verworfen. Bereits gespeicherte Zugänge und Originalbelege bleiben auf dem Server; prüfe vor einem neuen Vorgang den aktuellen Stand.","The local reference was discarded. Saved credentials and original receipts remain on the server; review current state before a new operation."));}
  return <details className="product-item"><summary>{tr("Privater GitHub-Zugang","Private GitHub credential")}</summary>
    <p>{tr("Verbinde ein Repository mit deinem privaten Projekt. Der Server verwahrt deinen eigenen Token verschlüsselt. Neue Läufe verwenden den Zugang erst nach deiner ausdrücklichen Auswahl; Git-, Pull-Request- und Issue-Aktionen werden einzeln freigegeben.","Connect one repository to your private project. The server encrypts your own token. New runs use the credential after your explicit selection; Git, pull request and issue actions receive individual approvals.")}</p>
    {error&&<p role="alert" className="error">{error}</p>}{notice&&<p role="status">{notice}</p>}
    <label>{tr("Privates Projekt","Private project")}<select value={project} disabled={busy||!!saved||!!projectId} onChange={e=>{setProject(e.target.value);setEditing(undefined);setFields({...empty});form.current?.reset();}}><option value="">{tr("Auswählen","Select")}</option>{projects.map(p=><option key={p.id} value={p.id}>{p.name}</option>)}{project&&!projects.some(p=>p.id===project)&&<option value={project}>{project}</option>}</select></label>
    <button disabled={busy||!project} onClick={()=>void attempt(reload)}>{tr("Aktuellen Stand laden","Refresh current state")}</button>
    {(saved||receiptInvalid)&&<aside className="product-item"><p>{tr("Ein Originalvorgang wartet auf Bestätigung.","An original operation is awaiting acknowledgement.")}{saved&&<> {saved.operation_id}</>}</p>
      {pending.current&&<button disabled={busy} onClick={()=>void attempt(current=>{const p=pending.current!;return execute(p.intent,p.token,current);})}>{tr("Denselben Originalvorgang erneut abrufen","Retrieve the same original operation")}</button>}
      {saved?.remove&&!pending.current&&<button disabled={busy} onClick={()=>void attempt(current=>execute(saved,undefined,current))}>{tr("Originalen Widerruf erneut abrufen","Retrieve the original revocation")}</button>}
      {saved&&!saved.remove&&!pending.current&&<p>{tr("Gib denselben ursprünglichen Token erneut ein. Er wird im Browser nicht dauerhaft gespeichert.","Enter the same original token again. It is not stored persistently in the browser.")}</p>}
      <button disabled={busy} onClick={resetOriginal}>{tr("Lokale Vorgangsreferenz ausdrücklich verwerfen","Explicitly discard local operation reference")}</button>
    </aside>}
    {!saved?.remove&&<form ref={form} onSubmit={submit} autoComplete="off"><h4>{editing?tr("Zugang aktualisieren","Update credential"):tr("Neuen Zugang einrichten","Configure a new credential")}</h4>
      {(["name","git_origin","api_root","owner","repository","author_name","author_email"] as const).map(key=><label key={key}>{({name:tr("Name","Name"),git_origin:tr("Git-Adresse ohne Pfad","Git origin without path"),api_root:tr("GitHub-API-Adresse","GitHub API root"),owner:tr("GitHub-Konto oder Organisation","GitHub account or organization"),repository:tr("Repository ohne .git","Repository without .git"),author_name:tr("Commit-Autor","Commit author"),author_email:tr("Commit-E-Mail","Commit email")})[key]}<input required type={key==="author_email"?"email":"text"} value={fields[key]} maxLength={key==="git_origin"||key==="api_root"?2048:key==="author_email"?254:key==="owner"||key==="repository"?100:200} disabled={busy||!!saved||receiptInvalid} onChange={e=>setFields(v=>({...v,[key]:e.target.value}))}/></label>)}
      <label>{tr("Eigener Repository-Token","Own repository token")}<input name="token" type="password" required maxLength={4096} autoComplete="off" spellCheck={false} disabled={busy||!!pending.current||receiptInvalid}/></label>
      <p>{tr("Erteile nur die benötigten Repository-Rechte. Für GitHub Enterprise gehören Git und API zum selben HTTPS-Server; die API endet auf /api/v3/.","Grant the repository permissions you need. GitHub Enterprise Git and API use the same HTTPS server; the API ends in /api/v3/.")}</p>
      <button disabled={busy||!project||!!pending.current||receiptInvalid}>{saved?tr("Originalen Speichervorgang abrufen","Retrieve original save"):tr("Zugang verschlüsselt speichern","Save encrypted credential")}</button>
      {!saved&&editing&&<button type="button" disabled={busy} onClick={()=>{setEditing(undefined);setFields({...empty});form.current?.reset();}}>{tr("Neuen Zugang vorbereiten","Prepare a new credential")}</button>}
    </form>}
    {bindings.map(row=><article key={row.id} className="product-item"><h4>{row.name}{selectedBindingId===row.id&&tr(" · Ausgewählt"," · Selected")}</h4><p>{row.git_origin}/{row.owner}/{row.repository}.git · v{row.revision}</p><p>{row.author_name} &lt;{row.author_email}&gt;</p>
      {onSelect&&<button disabled={busy||!!saved||selectionDisabled} onClick={()=>onSelect(project,row.id)}>{tr("Für neue private Läufe auswählen","Select for new private runs")}</button>}
      <button disabled={busy} onClick={()=>void attempt(current=>reference(row,current))}>{tr("Laufreferenz exportieren","Export run reference")}</button>
      <button disabled={busy||!!saved} onClick={()=>edit(row)}>{tr("Aktualisieren","Update")}</button>
      <button disabled={busy||!!saved} onClick={()=>void attempt(current=>execute({format:1,operation_id:crypto.randomUUID(),project_id:project,binding_id:row.id,remove:true,fields:{expected_revision:row.revision}},undefined,current))}>{tr("Diesen Zugang widerrufen","Revoke this credential")}</button>
    </article>)}
    {onSelect&&selectedBindingId&&<button disabled={busy||selectionDisabled} onClick={()=>onSelect(project,"")}>{tr("Auswahl für neue Läufe entfernen","Clear selection for new runs")}</button>}
  </details>;
}
