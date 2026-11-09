import {useTeamText} from "./teamStrings";
import {useEffect,useState} from "react";
import {Client} from "@cowork/client";
import {productRequest} from "./productApi";

export function TeamToolPicker({client,session,team,names,selected,onChange}:{client:Client,session:string,team:string,names:string[],selected:string[],onChange:(value:string[])=>void}){
  const tr=useTeamText();
  const [labels,setLabels]=useState<Record<string,{name:string,description:string|null}>>({});
  useEffect(()=>{let current=true;setLabels({});if(!team||!session||!names.some(name=>name.startsWith("mcp_")))return;
    void productRequest<{id:string,name:string,enabled:boolean,catalog_sha256:string|null}[]>(client,`/teams/${team}/mcp-servers`,"GET",undefined,session).then(async servers=>{
      const entries=await Promise.all(servers.filter(server=>server.enabled&&server.catalog_sha256).map(async server=>{const tools=await productRequest<{name:string,remote_name:string,description:string|null}[]>(client,`/teams/${team}/mcp-servers/${server.id}/tools`,"GET",undefined,session);return tools.map(tool=>[tool.name,{name:server.name+" · "+tool.remote_name,description:tool.description}] as const);}));
      if(current)setLabels(Object.fromEntries(entries.flat()));
    }).catch(()=>{});return()=>{current=false;};
  },[client.base,session,team,names]);
  const native:Record<string,string>={execute_program:"Terminal",browser:"Browser",office:"Office / PDF",github:"GitHub"};
  return <fieldset><legend>{tr("Werkzeuge für diesen Lauf")}</legend>{names.map(name=><label className="product-check" key={name} title={labels[name]?.description??undefined}><input type="checkbox" checked={selected.includes(name)} onChange={event=>onChange(event.target.checked?[...selected,name]:selected.filter(value=>value!==name))}/>{labels[name]?.name??native[name]??name}</label>)}<p className="muted">{tr("Jede konkrete Aktion erfordert eine Freigabe. Terminal, Browser, Office und GitHub laufen im getrennten Teamcontainer. MCP verwendet die konfigurierte Teamverbindung.")}</p></fieldset>;
}
