import type {WorkflowRights} from "@cowork/client";
import {useView} from "./view";

export function WorkflowWorkspaceRight({value,onChange,available=true,label}:{value:WorkflowRights;onChange:(value:WorkflowRights)=>void;available?:boolean;label?:string}){
  const language=useView(view=>view.language),tr=(de:string,en:string)=>language==="de"?de:en;
  return <div className="product-stack"><label className="product-check"><input type="checkbox" checked={!!value.current_workspace} disabled={!available&&!value.current_workspace} onChange={event=>{const next={...value};if(event.target.checked)next.current_workspace=true;else delete next.current_workspace;onChange(next);}}/>{label??tr("Arbeitsbereich des aktuellen Auftrags","Current task workspace")}</label>
    <small>{tr("Wird beim bestätigten Start an den Arbeitsbereich dieses Auftrags gebunden. Werkzeugrechte und übergeordnete Grenzen gelten weiterhin.","Binds to this task's workspace at confirmed start. Tool permissions and enclosing limits still apply.")}</small>
    {!available&&<small>{tr("Muss auch in den übergeordneten Rechten ausgewählt sein.","Must also be selected in the enclosing permissions.")}</small>}
  </div>;
}
export function WorkflowWorkspaceNotice({value}:{value:WorkflowRights}){
  const language=useView(view=>view.language);
  return value.current_workspace?<p className="muted">{language==="de"?"Ausgewählt: Arbeitsbereich des aktuellen Auftrags":"Selected: current task workspace"}</p>:null;
}
