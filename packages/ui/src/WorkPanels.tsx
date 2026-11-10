import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Activity, ArrowRight, Search } from "lucide-react";
import { Client } from "@cowork/client";
import type { Key } from "./i18n";
import {independentChats} from "./chatScope";
type T = (key: Key) => string;
export type OpenOrder = (project: string, order: string, run?: string) => void;

function Pagination({offset,next,change,t}:{offset:number,next?:number|null,change:(n:number)=>void,t:T}) {
  return <div className="actions"><button disabled={!offset} onClick={()=>change(Math.max(0,offset-30))}>{t("previousPage")}</button>
    <button disabled={next==null} onClick={()=>{if(next!=null)change(next);}}>{t("nextPage")}</button></div>;
}

export function ActivityPanel({client,t,open}:{client:Client,t:T,open:OpenOrder}) {
  const [attention,setAttention]=useState(true); const [offset,setOffset]=useState(0);
  const query=useQuery({queryKey:["activity",attention,offset],queryFn:()=>client.activity(attention,offset),refetchInterval:1500});
  return <div className="work-panel"><p className="muted">{t("activityHint")}</p>
    <div className="panel-tabs"><button aria-pressed={attention} onClick={()=>{setAttention(true);setOffset(0);}}>{t("inbox")} · {query.data?.attention??"—"}</button>
      <button aria-pressed={!attention} onClick={()=>{setAttention(false);setOffset(0);}}>{t("allActive")} · {query.data?.total??"—"}</button></div>
    {query.isPending&&<p role="status">{t("loading")}</p>}
    {query.isError&&<p role="alert">{t("error")} <button onClick={()=>query.refetch()}>{t("retry")}</button></p>}
    {query.data&&!query.data.items.length&&<div className="panel-empty"><Activity size={28}/><p>{t(attention?"inboxEmpty":"activityEmpty")}</p></div>}
    <div className="work-list">{query.data?.items.map(({run,project_name,order_title})=><button key={run.id} className="work-item" onClick={()=>open(run.spec.project_id,run.spec.order_id,run.id)}>
      <span><small>{project_name}</small><strong>{order_title}</strong><span className={"badge "+run.state}>{t(run.state)}</span></span><ArrowRight size={18}/>
    </button>)}</div>
    <Pagination offset={offset} next={query.data?.next_offset} change={setOffset} t={t}/>
  </div>;
}

export function SearchPanel({client,t,project,open,initialQuery=""}:{client:Client,t:T,project:string,open:OpenOrder,initialQuery?:string}) {
  const [text,setText]=useState(initialQuery); const [term,setTerm]=useState(initialQuery); const [scope,setScope]=useState(""); const [offset,setOffset]=useState(0);
  useEffect(()=>{const timer=setTimeout(()=>{setTerm(text.trim());setOffset(0);},300);return()=>clearTimeout(timer);},[text]);
  const query=useQuery({queryKey:["search",term,scope,offset],queryFn:()=>client.search(term,scope,offset),enabled:!!term});
  return <div className="work-panel"><p className="muted">{t("searchHistoryHint")}</p>
    <label className="history-search"><span>{t("searchHistory")}</span><input autoFocus type="search" value={text} maxLength={200} onChange={e=>setText(e.target.value)}/></label>
    <label>{t("searchScope")}<select value={scope} onChange={e=>{setScope(e.target.value);setOffset(0);}}><option value="">{t("allProjects")}</option>{project&&<option value={project}>{t(project===independentChats?"orders":"currentProject")}</option>}</select></label>
    {!term&&<div className="panel-empty"><Search size={28}/><p>{t("searchHistoryHint")}</p></div>}
    {!!term&&query.isPending&&<p role="status">{t("loading")}</p>}
    {query.isError&&<p role="alert">{t("error")} <button onClick={()=>query.refetch()}>{t("retry")}</button></p>}
    {query.data&&!query.data.items.length&&<p className="panel-empty">{t("noSearchResults")}</p>}
    <div className="work-list">{query.data?.items.map(hit=><button key={hit.id} className="work-item" onClick={()=>open(hit.project_id,hit.order_id)}>
      <span><small>{hit.project_name} · {t(hit.kind==="order"?"orders":hit.kind==="artifact"?"results":"conversation")}</small>
      <strong>{hit.order_title}</strong><span className="search-excerpt">{hit.excerpt}</span></span><ArrowRight size={18}/>
    </button>)}</div>
    {!!term&&<Pagination offset={offset} next={query.data?.next_offset} change={setOffset} t={t}/>}
  </div>;
}
