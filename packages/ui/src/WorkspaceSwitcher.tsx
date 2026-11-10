import {useEffect, useId, useRef, useState} from "react";
import {Check, ChevronDown, Ellipsis, Monitor, Plus, Trash2} from "lucide-react";
import type {Key} from "./i18n";
import type {WorkspaceProfile} from "./workspaceProfiles";

export type WorkspaceChanges = {name:string;endpoint:string;token:string};

export function WorkspaceSwitcher({profiles,selected,busy,t,onSelect,onAdd,onEdit}:{
  profiles:WorkspaceProfile[];selected:string;busy:boolean;t:(key:Key)=>string;
  onSelect:(profile:WorkspaceProfile)=>void;onAdd:()=>void;onEdit:(profile:WorkspaceProfile)=>void;
}){
  const label=(profile:WorkspaceProfile)=>profile.name||(profile.kind==="native"?t("local"):profile.base?new URL(profile.base).host:t("currentWorkspace"));
  const detail=(profile:WorkspaceProfile)=>profile.kind==="native"?t("local"):profile.base?new URL(profile.base).host:location.host;
  const active=profiles.find(profile=>profile.id===selected);
  const menuId=useId(),menu=useRef<HTMLDivElement>(null),button=useRef<HTMLButtonElement>(null);
  const [open,setOpen]=useState(false);
  function place(){const trigger=button.current,panel=menu.current;if(!trigger||!panel)return;const rect=trigger.getBoundingClientRect(),width=Math.min(320,innerWidth-16);panel.style.width=width+"px";panel.style.left=Math.max(8,Math.min(rect.right-width,innerWidth-width-8))+"px";panel.style.top=rect.bottom+6+"px";panel.style.maxHeight=Math.max(160,innerHeight-rect.bottom-20)+"px";}
  function hide(){try{menu.current?.hidePopover();}catch{/* already closed */}}
  useEffect(()=>{if(!open)return;addEventListener("resize",hide);return()=>removeEventListener("resize",hide);},[open]);
  return <div className="workspace-switcher" aria-busy={busy}>
    <button type="button" ref={button} className="workspace-select" popoverTarget={menuId} disabled={busy} aria-expanded={open}
      aria-label={t("chooseWorkspace")+": "+(active?label(active):t("currentWorkspace"))} title={active?.base||t("currentWorkspace")} onClick={place}>
      <Monitor size={16} aria-hidden="true"/><span className="workspace-select-label">{active?label(active):t("currentWorkspace")}</span><ChevronDown size={14} aria-hidden="true"/>
    </button>
    <div id={menuId} ref={menu} popover="auto" className="workspace-menu" role="dialog" aria-label={t("chooseWorkspace")} onToggle={event=>setOpen(event.newState==="open")}>
      <ul>{profiles.map(profile=><li key={profile.id} className={profile.id===selected?"selected":undefined}>
        <button type="button" className="icon workspace-menu-more" aria-label={t("editWorkspaceConnection")+": "+label(profile)} title={t("editWorkspaceConnection")}
          onClick={()=>{hide();onEdit(profile);}}><Ellipsis size={16}/></button>
        <button type="button" className="workspace-menu-item" aria-current={profile.id===selected?"true":undefined}
          onClick={()=>{hide();if(profile.id!==selected)onSelect(profile);}}>
          <span><strong>{label(profile)}</strong>{!!profile.name&&<small>{detail(profile)}</small>}</span>{profile.id===selected&&<Check size={15} aria-hidden="true"/>}
        </button>
      </li>)}</ul>
    </div>
    <button className="icon add-workspace" disabled={busy} aria-label={t("addWorkspace")} title={t("addWorkspace")} onClick={onAdd}><Plus size={18}/></button>
    {busy&&<span className="sr-only" role="status">{t("connecting")}</span>}
  </div>;
}

export function WorkspaceConnectionEditor({profile,active,busy,t,onSave,onRemove,onCancel}:{
  profile:WorkspaceProfile;active:boolean;busy:boolean;t:(key:Key)=>string;
  onSave:(changes:WorkspaceChanges)=>void;onRemove:()=>void;onCancel:()=>void;
}){
  const [confirm,setConfirm]=useState(false);
  const native=profile.kind==="native";
  return <form className="workspace-connection-form workspace-connection-editor" onSubmit={event=>{event.preventDefault();const data=new FormData(event.currentTarget);
    onSave({name:String(data.get("name")??"").trim(),endpoint:String(data.get("endpoint")??profile.base),token:String(data.get("token")??"").trim()});}}>
    <label>{t("workspaceName")}<input name="name" defaultValue={profile.name} placeholder={native?t("local"):t("workspaceNamePlaceholder")} maxLength={80} disabled={busy}/></label>
    {native?<p className="muted">{t("localConnectionFixed")}</p>:<>
      <label>{t("serverAddress")}<input name="endpoint" type="url" autoComplete="url" defaultValue={profile.base} placeholder="https://cowork.example" disabled={busy}/></label>
      <label>{t("token")}<input name="token" type="password" autoComplete="off" minLength={48} placeholder={t("workspaceTokenKeep")} disabled={busy}/></label>
      <p className="muted">{t("workspaceTokenEditHint")}</p>
    </>}
    <div className="actions"><button type="button" disabled={busy} onClick={onCancel}>{t("cancel")}</button><button className="primary" disabled={busy}>{busy?t("connecting"):t("save")}</button></div>
    {!native&&<div className="workspace-connection-remove">
      {confirm?<><p>{t(active?"removeActiveWorkspaceHint":"removeWorkspaceHint")}</p>
        <div className="actions"><button type="button" disabled={busy} onClick={()=>setConfirm(false)}>{t("cancel")}</button><button type="button" className="danger" disabled={busy} onClick={onRemove}><Trash2 size={16}/>{t("removeWorkspaceConfirm")}</button></div></>
      :<button type="button" className="quiet danger" disabled={busy} onClick={()=>setConfirm(true)}><Trash2 size={16}/>{t("removeWorkspace")}</button>}
    </div>}
  </form>;
}
