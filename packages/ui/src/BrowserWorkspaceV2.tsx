import {useCallback,useEffect,useMemo,useRef,useState,type FormEvent,type KeyboardEvent} from "react";
import {useQuery} from "@tanstack/react-query";
import type {Client,BrowserCommand,BrowserEngine,BrowserMode,BrowserSessionView,BrowserExtensionConnection,BrowserTab} from "@cowork/client";
import {productRequest,ProductRequestError} from "./productApi";
import {useView} from "./view";
import {ToolboxActionReview,useToolboxAction} from "./ToolboxActionReview";
import {successfulToolboxResult,toolboxAvailability} from "./toolboxWorkbench";
import {browserSetupRequired,browserSetupMessage} from "./browserReadiness";
import {ToolRuntimeSetup} from "./ToolRuntimeSetup";
import "./browserWorkspace.css";
import {annotationPrompt,browserAnnotationsKey,mergeBrowserAnnotations,type BrowserAnnotation} from "./browserAnnotations";

type Snapshot={tabs:BrowserTab[],tab_id?:string,document_id?:string,url?:string,frame?:{image:string,width:number,height:number}};
export function browserPointer(event:{clientX:number,clientY:number},bounds:{left:number,top:number,width:number,height:number},viewport:{width:number,height:number}){
  if(bounds.width<=0||bounds.height<=0)throw Error("browser_viewport_unavailable");
  return {x:Math.max(0,Math.min(viewport.width-1,(event.clientX-bounds.left)*viewport.width/bounds.width)),y:Math.max(0,Math.min(viewport.height-1,(event.clientY-bounds.top)*viewport.height/bounds.height))};
}
export function BrowserWorkspaceV2({client,order,active}:{client:Client,order:string,active:boolean}){
  const language=useView(v=>v.language),tr=useCallback((de:string,en:string)=>language==="de"?de:en,[language]);
  const [engine,setEngine]=useState<BrowserEngine>("chrome"),[mode,setMode]=useState<BrowserMode>("managed");
  const readiness=useQuery({queryKey:["toolbox",client.base,"browser"],queryFn:()=>toolboxAvailability(client),retry:false});
  const setupRequired=browserSetupRequired(readiness.data?.programs,engine,mode);
  const [sessions,setSessions]=useState<BrowserSessionView[]>([]),[session,setSession]=useState<BrowserSessionView|null>(null);
  const [connections,setConnections]=useState<BrowserExtensionConnection[]>([]),[connection,setConnection]=useState("");
  const [snapshot,setSnapshot]=useState<Snapshot|null>(null),[url,setUrl]=useState(""),[error,setError]=useState(""),[busy,setBusy]=useState(false),[uncertain,setUncertain]=useState(false),[visible,setVisible]=useState(!document.hidden);
  const addressEditing=useRef(false),addressDirty=useRef(false),element=useRef<HTMLElement>(null),[onScreen,setOnScreen]=useState(true);
  const current=useRef(session),snapshotRef=useRef(snapshot),queue=useRef(Promise.resolve()),generation=useRef(0);
  const liveElement=useRef<HTMLDivElement>(null),manualRef=useRef<(command:BrowserCommand)=>void>(()=>undefined),uncertainRef=useRef(uncertain);uncertainRef.current=uncertain;
  const action=useToolboxAction(client,order,"browser");current.current=session;snapshotRef.current=snapshot;
  const base=`/orders/${encodeURIComponent(order)}/toolbox/browser-v2`;
  async function refresh(){try{const all=await productRequest<BrowserSessionView[]>(client,base+"/sessions");setSessions(all);setSession(previous=>previous?all.find(s=>s.id===previous.id)??null:all[0]??null);}catch{setError(tr("Browsersitzungen konnten nicht geladen werden.","Browser sessions could not be loaded."));}}
  useEffect(()=>{void refresh();return()=>{generation.current++;};},[client,order]);
  useEffect(()=>{if(mode==="extension"&&active)void productRequest<BrowserExtensionConnection[]>(client,"/toolbox/browser-connections").then(setConnections).catch(()=>undefined);},[client,mode,active]);
  useEffect(()=>{const change=()=>setVisible(!document.hidden);document.addEventListener("visibilitychange",change);return()=>document.removeEventListener("visibilitychange",change);},[]);
  useEffect(()=>{if(typeof IntersectionObserver!=="undefined"&&element.current){const observer=new IntersectionObserver(entries=>setOnScreen(entries.some(e=>e.isIntersecting)));observer.observe(element.current);return()=>observer.disconnect();}},[]);
  useEffect(()=>{if(!session||!active||!visible||!onScreen)return;let stopped=false;let timer:ReturnType<typeof setTimeout>|undefined;
    const id=session.id,epoch=session.epoch;
    const tick=async()=>{try{
      const value=await productRequest<Snapshot>(client,base+`/sessions/${encodeURIComponent(id)}/${session.mode==="managed"?"frame":"tabs"}`,session.mode==="managed"?"POST":"GET");
      if(!stopped&&current.current?.id===id&&current.current?.epoch===epoch){setSnapshot(value);if(value.url&&!addressEditing.current&&!addressDirty.current)setUrl(value.url);}
    }catch{if(!stopped)setError(tr("Browserverbindung unterbrochen. Sitzung prüfen oder schließen; Aktionen werden nicht wiederholt.","Browser connection interrupted. Inspect or close the session; actions are not replayed."));}
    finally{if(!stopped)timer=setTimeout(()=>void tick(),session.mode==="managed"?200:1000);}};
    void tick();return()=>{stopped=true;if(timer)clearTimeout(timer);};
  },[client,order,session?.id,session?.epoch,active,visible,onScreen]);
  async function work(task:()=>Promise<void>){if(busy)return;setBusy(true);setError("");try{await task();}catch(e){setError(browserSetupMessage(e instanceof ProductRequestError?e.reason:e instanceof Error?e.message:String(e),language)??tr("Die Browseraktion konnte nicht ausgeführt werden. Prüfe die Werkzeugkonfiguration und offene Sitzungen. Technischer Fehler: ","The browser action could not be completed. Check tool configuration and open sessions. Technical error: ")+(e instanceof Error?e.message:String(e)));}finally{setBusy(false);}}
  async function create(){await work(async()=>{
    if(setupRequired)throw new Error("browser_install_runtime_first");
    const value=await productRequest<BrowserSessionView>(client,base+"/sessions","POST",{engine,mode,connection_id:mode==="extension"?connection:null});
    addressDirty.current=false;setUrl("");setSession(value);setSnapshot(null);setUncertain(false);setSessions(old=>[...old,value]);
  });}
  async function takeover(manual:boolean){if(!session)return;await work(async()=>{
    const value=await productRequest<BrowserSessionView>(client,base+`/sessions/${session.id}/takeover`,"POST",{manual});setSession(value);setSessions(old=>old.map(s=>s.id===value.id?value:s));
  });}
  function manual(command:BrowserCommand){
    const selected=current.current;if(!selected?.manual||uncertain)return;
    const sequence=generation.current;
    queue.current=queue.current.then(async()=>{
      if(sequence!==generation.current||current.current?.id!==selected.id||!current.current.manual)return;
      const view=snapshotRef.current;
      const selectedTab=command.tab_id?view?.tabs.find(t=>t.id===command.tab_id):view?.tabs.find(t=>t.id===view.tab_id)??view?.tabs.find(t=>t.active);
      const body={idempotency_key:crypto.randomUUID(),epoch:selected.epoch,command:{...command,...(command.operation!=="new_tab"&&selectedTab?{tab_id:selectedTab.id,document_id:selectedTab.document_id}:{})}};
      try{await productRequest(client,base+`/sessions/${selected.id}/input`,"POST",body);}catch{
        generation.current++;setUncertain(true);setError(tr("Ergebnis der Eingabe ist unklar. Prüfe die Seite und schließe die Sitzung vor weiteren Eingaben. Es erfolgt keine Wiederholung.","Input outcome is uncertain. Inspect the page and close the session before further input. It will not be replayed."));
      }
    });
  }
  function navigate(event:FormEvent){event.preventDefault();const target=url;addressDirty.current=false;manual({operation:"navigate",url:target});}
  function key(event:KeyboardEvent){
    if(!session?.manual||event.nativeEvent.isComposing||event.altKey)return;
    if(["Shift","Control","Alt","Meta"].includes(event.key))return;
    event.preventDefault();event.stopPropagation();manual({operation:"key",text:event.key.length===1&&!event.ctrlKey&&!event.metaKey?event.key:(event.ctrlKey?"Control+":"")+(event.metaKey?"Meta+":"")+(event.shiftKey?"Shift+":"")+event.key,selector:event.key.length===1&&!event.ctrlKey&&!event.metaKey?"insert":undefined});
  }
  manualRef.current=manual;
  useEffect(()=>{const target=liveElement.current;if(!target)return;let timer:ReturnType<typeof setTimeout>|undefined,x=0,y=0;
    const wheel=(event:WheelEvent)=>{if(!current.current?.manual||uncertainRef.current)return;event.preventDefault();event.stopPropagation();const scale=event.deltaMode===1?16:event.deltaMode===2?(snapshotRef.current?.frame?.height??960):1;x+=event.deltaX*scale;y+=event.deltaY*scale;if(!timer)timer=setTimeout(()=>{timer=undefined;manualRef.current({operation:"scroll",x,y});x=0;y=0;},50);};
    target.addEventListener("wheel",wheel,{passive:false});return()=>{target.removeEventListener("wheel",wheel);if(timer)clearTimeout(timer);};
  },[session?.id,!!snapshot?.frame]);
  const selected=snapshot?.tabs.find(t=>t.id===snapshot.tab_id)??snapshot?.tabs.find(t=>t.active);
  const result=useMemo(()=>successfulToolboxResult(action.job),[action.job]);
  const notesKey=browserAnnotationsKey(client.base,order),[notes,setNotes]=useState<BrowserAnnotation[]>(()=>{try{return mergeBrowserAnnotations([],JSON.parse(localStorage.getItem(notesKey)??"[]"));}catch{return [];}});
  const resultData=result&&typeof result==="object"?result as Record<string,unknown>:null;
  useEffect(()=>{if(resultData?.annotations){try{setNotes(old=>{const next=mergeBrowserAnnotations(old,resultData.annotations);localStorage.setItem(notesKey,JSON.stringify(next));return next;});}catch{setError(tr("Annotationen konnten nicht gespeichert werden.","Annotations could not be saved."));}}},[result,notesKey]);
  const externalImage=typeof resultData?.image==="string"&&/^data:image\/(jpeg|png);base64,/.test(resultData.image)?resultData.image:undefined;
  return <section ref={element} className="browser-workspace" aria-label={tr("Chrome und Firefox","Chrome and Firefox")}>
    <header><h3>{tr("Browser","Browser")}</h3><p>{tr("Eigene Chrome-/Firefox-Profile bleiben je Projekt erhalten. Installierte Browser teilen nur ausgewählte Tabs.","Dedicated Chrome/Firefox profiles persist per project. Installed browsers share only selected tabs.")}</p></header>
    <div className="browser-controls"><label>{tr("Browser","Browser")}<select value={engine} onChange={e=>setEngine(e.target.value as BrowserEngine)}><option value="chrome">Chrome</option><option value="firefox">Firefox</option></select></label><label>{tr("Ausführung","Mode")}<select value={mode} onChange={e=>setMode(e.target.value as BrowserMode)}><option value="managed">{tr("In der App","In the app")}</option><option value="extension">{tr("Installierter Browser","Installed browser")}</option></select></label>
    {mode==="extension"&&<><label>{tr("Verbindung","Connection")}<select value={connection} onChange={e=>setConnection(e.target.value)}><option value="">{tr("Verbindung wählen","Select connection")}</option>{connections.filter(c=>c.engine===engine).map(c=><option key={c.id} value={c.id}>{c.label} · {c.tabs.length} Tabs</option>)}</select></label><button disabled={busy} onClick={()=>void work(async()=>setConnections(await productRequest(client,"/toolbox/browser-connections")))}>{tr("Erweiterungen suchen","Find extensions")}</button></>}
    <button disabled={busy||!order||readiness.isPending||readiness.isError||setupRequired||(mode==="extension"&&!connection)} onClick={()=>void create()}>{tr("Sitzung öffnen","Open session")}</button>
    <button disabled={busy||sessions.some(s=>s.engine===engine&&s.mode==="managed")} onClick={()=>{if(window.confirm(tr("Projektprofil und gespeicherte Browseranmeldungen löschen?","Delete the project profile and saved browser sign-ins?")))void work(async()=>{await productRequest(client,base+"/profile/delete","POST",{engine});});}}>{tr("Projektprofil löschen","Delete project profile")}</button></div>
    {setupRequired&&!readiness.isPending&&<p role="status">{browserSetupMessage("browser_install_runtime_first",language)}</p>}
    {readiness.isError&&<p role="alert">{tr("Die Browserbereitschaft konnte nicht geprüft werden.","Browser readiness could not be checked.")}</p>}
    <button type="button" disabled={readiness.isFetching} onClick={()=>void readiness.refetch()}>{tr("Browserbereitschaft erneut prüfen","Check browser readiness again")}</button>
    <details><summary>{tr("Werkzeuge einrichten","Configure tool runtimes")}</summary><ToolRuntimeSetup/></details>
    {mode==="extension"&&<p>{tr("Erweiterung in Chrome oder Firefox installieren, im Popup diese Cowork-App wählen und Tabs freigeben. Unter Einstellungen → Werkzeuge lässt sich der lokale Host einrichten oder reparieren.","Install the Chrome or Firefox extension, choose this Cowork app in its popup and share tabs. Set up or repair the local host in Settings → Tools.")}</p>}
    {!!sessions.length&&<label>{tr("Offene Sitzung","Open session")}<select value={session?.id??""} onChange={e=>{addressDirty.current=false;generation.current++;setSession(sessions.find(s=>s.id===e.target.value)??null);setSnapshot(null);setUncertain(false);}}>{sessions.map((s,index)=><option key={s.id} value={s.id}>{s.engine==="chrome"?"Chrome":"Firefox"} · {s.mode==="managed"?tr("In der App","In the app"):tr("Installierter Browser","Installed browser")} · {index+1}</option>)}</select></label>}
    {error&&<p role="alert" className="error">{error}</p>}
    {session&&<><div className="browser-controls"><strong>{session.engine==="chrome"?(session.mode==="managed"?"Chrome for Testing":"Chrome"):session.mode==="managed"?"Firefox (Playwright)":"Firefox"} · {session.manual?tr("Manuelle Steuerung","Manual control"):tr("KI-Steuerung","AI control")}</strong><button disabled={busy||session.manual} onClick={()=>void takeover(true)}>{tr("Übernehmen","Take over")}</button><button disabled={busy||!session.manual||uncertain} onClick={()=>void takeover(false)}>{tr("KI fortsetzen","Resume AI")}</button><button disabled={busy} onClick={()=>void work(async()=>{generation.current++;await productRequest(client,`/orders/${encodeURIComponent(order)}/toolbox/browser-sessions/${session.id}/close`,"POST",{});setSessions(old=>old.filter(s=>s.id!==session.id));setSession(null);setSnapshot(null);setUncertain(false);})}>{tr("Sitzung schließen","Close session")}</button></div>
      <fieldset disabled={!session.manual||uncertain||busy}><legend>{tr("Browser bedienen","Browser controls")}</legend><form className="browser-address" onSubmit={navigate}><button type="button" onClick={()=>manual({operation:"back"})} aria-label={tr("Zurück","Back")}>←</button><button type="button" onClick={()=>manual({operation:"forward"})} aria-label={tr("Vorwärts","Forward")}>→</button><button type="button" onClick={()=>manual({operation:"reload"})} aria-label={tr("Neu laden","Reload")}>↻</button><input type="url" value={url} onFocus={()=>{addressEditing.current=true;}} onBlur={()=>{addressEditing.current=false;}} onChange={e=>{addressDirty.current=true;setUrl(e.target.value);}} placeholder="https://example.com" aria-label="URL" required/><button type="submit">{tr("Öffnen","Go")}</button><button type="button" onClick={()=>{addressDirty.current=false;manual({operation:"new_tab",url:url||"https://example.com/"});}}>+ Tab</button></form>
      <div className="browser-tab-list" role="group" aria-label="Tabs">{snapshot?.tabs.map(tab=><button type="button" key={tab.id} aria-pressed={tab.id===selected?.id} onClick={()=>{addressDirty.current=false;manual({operation:"activate_tab",tab_id:tab.id});}}>{tab.title||tab.url||tr("Neuer Tab","New tab")}</button>)}<button type="button" disabled={!selected} onClick={()=>manual({operation:"close_tab"})}>{tr("Tab schließen","Close tab")}</button></div>
      {session.mode==="managed"&&<label>{tr("Ansichtsgröße","Viewport")}<select defaultValue="1440x960" onChange={e=>{const [width,height]=e.target.value.split("x").map(Number);manual({operation:"resize",width,height});}}><option>1440x960</option><option>1280x800</option><option>960x720</option><option>390x844</option></select></label>}
      <div className="browser-controls"><button type="button" onClick={()=>manual({operation:"scroll",y:-600})}>↑ {tr("Scrollen","Scroll")}</button><button type="button" onClick={()=>manual({operation:"scroll",y:600})}>↓ {tr("Scrollen","Scroll")}</button></div></fieldset>
      {snapshot?.frame&&<div ref={liveElement} className="browser-live" tabIndex={session.manual?0:-1} role="group" aria-label={tr("Browser-Liveansicht. Klicken und tippen nach Übernahme.","Live browser view. Click and type after taking over.")} onKeyDown={key}><img src={snapshot.frame.image} draggable={false} alt={selected?.title||tr("Browser-Liveansicht","Live browser view")} onClick={e=>{if(snapshot.frame&&session.manual&&!uncertain){e.currentTarget.parentElement?.focus();manual({operation:"pointer",...browserPointer(e,e.currentTarget.getBoundingClientRect(),snapshot.frame)});}}}/></div>}
      {session.mode==="extension"&&<p>{tr("Die Webseite bleibt im installierten Browser sichtbar. Für komplexe Eingaben dort manuell übernehmen.","The page remains visible in your installed browser. Take over there for complex inputs.")}</p>}
      <form onSubmit={e=>{e.preventDefault();const f=new FormData(e.currentTarget);void action.prepare({kind:"browser",session_id:session.id,operation:"control",text:JSON.stringify({operation:String(f.get("operation")),selector:String(f.get("selector")||""),text:String(f.get("text")||""),tab_id:selected?.id,document_id:selected?.document_id})});}}><fieldset disabled={session.manual||action.locked}><legend>{tr("KI-Werkzeugaktion prüfen","Review AI tool action")}</legend><label>{tr("Aktion","Action")}<select name="operation"><option>dom</option><option>fetch_text</option><option>screenshot</option><option>click</option><option>fill</option><option>select</option>{session.capabilities.developer_tools&&<><option>console</option><option>network</option><option>annotations</option><option>annotate</option><option>clear_annotations</option><option>network_details</option></>}</select></label><label>{tr("Selektor","Selector")}<input name="selector" maxLength={1000}/></label><label>Text<textarea name="text" rows={2} maxLength={32000}/></label><button type="submit">{tr("Aktion prüfen","Review action")}</button></fieldset></form>
      <ToolboxActionReview action={action} hideOutput={!!externalImage}/>{externalImage&&<img className="browser-screenshot" src={externalImage} alt={tr("Screenshot des freigegebenen Tabs","Screenshot of the shared tab")}/>}
      {!!notes.length&&<details><summary>{tr("Gespeicherte Annotationen","Saved annotations")} ({notes.length})</summary>{notes.map(note=><article key={note.id}><code>{note.selector}</code><p>{note.text}</p></article>)}<button type="button" onClick={()=>void navigator.clipboard.writeText(annotationPrompt(notes)).catch(()=>setError(tr("Kopieren fehlgeschlagen.","Copy failed.")))}>{tr("Anweisungen kopieren","Copy instructions")}</button></details>}
    </>}
  </section>;
}
