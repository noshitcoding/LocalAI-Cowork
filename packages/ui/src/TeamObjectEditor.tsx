import {useTeamText} from "./teamStrings";
import {useState,type FormEvent} from "react";
import {Client} from "@cowork/client";
import {productRequest} from "./productApi";

type Project={id:string,name:string,revision:number,archived:boolean};
type Order={id:string,title:string,prompt:string,revision:number,archived:boolean};
export function TeamObjectEditor({client,session,team,project,order,onProject,onOrder}:{client:Client,session:string,team:string,project?:Project,order?:Order,onProject?:(value:Project)=>void,onOrder?:(value:Order)=>void}) {
  const tr=useTeamText();
  const [busy,setBusy]=useState(false);const [error,setError]=useState<unknown>();
  const value=order??project;if(!value)return null;
  async function save(event:FormEvent<HTMLFormElement>){
    event.preventDefault();const input=new FormData(event.currentTarget);setBusy(true);setError(undefined);
    try {
      if(order)onOrder?.(await productRequest<Order>(client,`/teams/${team}/orders/${order.id}`,"PUT",{title:String(input.get("name")),prompt:String(input.get("prompt")),expected_revision:order.revision,archived:input.get("archived")==="on"},session));
      else if(project)onProject?.(await productRequest<Project>(client,`/teams/${team}/projects/${project.id}`,"PUT",{name:String(input.get("name")),expected_revision:project.revision,archived:input.get("archived")==="on"},session));
    }catch(value){setError(value);}finally{setBusy(false);}
  }
  return <details><summary>{order?tr("Auftrag bearbeiten"):tr("Projekt bearbeiten")}</summary><form key={value.id+":"+value.revision} onSubmit={event=>void save(event)}><label>{tr("Name")}<input name="name" required defaultValue={order?.title??project?.name}/></label>{order&&<label>{tr("Aufgabe")}<textarea name="prompt" required rows={4} defaultValue={order.prompt}/></label>}<label className="product-check"><input name="archived" type="checkbox" defaultChecked={value.archived}/>{tr("Archiviert")}</label><button disabled={busy}>{tr("Speichern")}</button>{!!error&&<p className="error" role="alert">{error instanceof Error?error.message:String(error)}</p>}</form></details>;
}
