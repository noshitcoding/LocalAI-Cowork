import {useMemo,useState,type FormEvent} from "react";
import {useQuery} from "@tanstack/react-query";
import type {Client} from "@cowork/client";
import {ToolboxActionReview,useToolboxAction} from "./ToolboxActionReview";
import {toolboxAvailability} from "./toolboxWorkbench";
import {useView} from "./view";
import {BrowserWorkspaceV2} from "./BrowserWorkspaceV2";
import {GitWorkbench} from "./GitWorkbench";
import {PullRequestWorkbench} from "./PullRequestWorkbench";

export type GithubOperation="repository"|"pulls"|"pull"|"issues"|"issue"|"create_issue"|"create_pull";
export function githubArguments(input:{operation:GithubOperation,repository:string,number?:string,title?:string,body?:string,head?:string,base?:string}):string[]{
  const repository=input.repository.trim();
  if(!/^[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,38})\/[a-zA-Z0-9_.-]{1,100}$/.test(repository)||[".",".."].includes(repository.split("/")[1]))throw new Error("invalid_repository");
  const number=()=>{if(!/^[1-9][0-9]{0,8}$/.test(input.number??""))throw new Error("invalid_issue_or_pull_number");return input.number!;};
  if(input.operation==="repository")return ["repo","view",repository,"--json","nameWithOwner,description,url,defaultBranchRef"];
  if(input.operation==="pulls")return ["pr","list","--repo",repository,"--state","all","--json","number,title,state,url,headRefName,baseRefName"];
  if(input.operation==="issues")return ["issue","list","--repo",repository,"--state","all","--json","number,title,state,url"];
  if(input.operation==="pull")return ["pr","view",number(),"--repo",repository,"--json","number,title,body,state,url,headRefName,baseRefName,reviewDecision"];
  if(input.operation==="issue")return ["issue","view",number(),"--repo",repository,"--json","number,title,body,state,url,labels"];
  if(!input.title?.trim()||input.title.length>250||!input.body?.trim()||input.body.length>40_000)throw new Error("invalid_github_content");
  const arguments_=["api",`repos/${repository}/${input.operation==="create_issue"?"issues":"pulls"}`,"--method","POST","--raw-field","title="+input.title,"--raw-field","body="+input.body];
  if(input.operation==="create_pull"){
    if(!input.head?.trim()||!input.base?.trim()||input.head.length>200||input.base.length>200||/[\r\n\0]/.test(input.head+input.base))throw new Error("invalid_pull_branches");
    arguments_.push("--raw-field","head="+input.head,"--raw-field","base="+input.base);
  }
  return arguments_;
}
export function DeveloperWorkspace({client,order}:{client:Client,order:string}){
  const language=useView(value=>value.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const [tab,setTab]=useState<"browser"|"github"|"git">("browser");
  const scope=useMemo(()=>crypto.randomUUID(),[client]);
  return <section><header className="settings-section-heading"><h2>{tr("Entwicklungswerkzeuge","Developer tools")}</h2><p>{tr("Verwende die eingerichteten Werkzeuge dieses Arbeitsbereichs. Jede Aktion zeigt vor dem Start ihre genauen Parameter.","Use this workspace's configured tools. Review each action's exact parameters before execution.")}</p></header><div className="document-tabs"><button type="button" aria-pressed={tab==="browser"} onClick={()=>setTab("browser")}>Browser</button><button type="button" aria-pressed={tab==="git"} onClick={()=>setTab("git")}>Git</button><button type="button" aria-pressed={tab==="github"} onClick={()=>setTab("github")}>GitHub</button></div><div hidden={tab!=="browser"} inert={tab!=="browser"}><BrowserWorkspaceV2 key={scope+order+":browser"} client={client} order={order} active={tab==="browser"}/></div><div hidden={tab!=="git"} inert={tab!=="git"}><GitWorkbench key={scope+order+":git"} client={client} order={order}/></div><div hidden={tab!=="github"} inert={tab!=="github"}><GithubWorkspace key={scope+order+":github"} client={client} order={order}/><PullRequestWorkbench key={scope+order+":pulls"} client={client} order={order}/></div></section>;
}
function GithubWorkspace({client,order}:{client:Client,order:string}){
  const language=useView(value=>value.language),tr=(de:string,en:string)=>language==="de"?de:en;
  const availability=useQuery({queryKey:["toolbox-workbench",client.base],queryFn:()=>toolboxAvailability(client),retry:false}),action=useToolboxAction(client,order,"github"),[operation,setOperation]=useState<GithubOperation>("repository"),[error,setError]=useState("");
  async function prepare(event:FormEvent<HTMLFormElement>){event.preventDefault();setError("");try{const form=new FormData(event.currentTarget),value=(name:string)=>String(form.get(name)??"");await action.prepare({kind:"github",arguments:githubArguments({operation,repository:value("repository"),number:value("number"),title:value("title"),body:value("body"),head:value("head"),base:value("base")})});}catch(error){setError(error instanceof Error?error.message:String(error));}}
  const options:[GithubOperation,string][]=[["repository",tr("Repository anzeigen","View repository")],["pulls",tr("Pull Requests auflisten","List pull requests")],["pull",tr("Pull Request anzeigen","View pull request")],["issues",tr("Issues auflisten","List issues")],["issue",tr("Issue anzeigen","View issue")],["create_issue",tr("Issue erstellen","Create issue")],["create_pull",tr("Pull Request erstellen","Create pull request")]];
  return <section><p className="muted">{tr("GitHub verwendet den eingerichteten Zugang des verbundenen Arbeitsbereichs.","GitHub uses the connected workspace's configured credentials.")}</p>{!availability.isPending&&!availability.data?.github_available&&<p>{tr("Richte zuerst GitHub CLI und den GitHub-Zugang unter MCP & Werkzeuge ein.","Configure GitHub CLI and credentials under MCP & Tools first.")}</p>}<form onChange={()=>action.invalidatePreview()} onSubmit={event=>void prepare(event)}><fieldset disabled={action.locked||!availability.data?.github_available}><label>Repository<input name="repository" placeholder="owner/repository" required maxLength={140}/></label><label>{tr("Aktion","Action")}<select value={operation} onChange={event=>setOperation(event.target.value as GithubOperation)}>{options.map(([id,label])=><option key={id} value={id}>{label}</option>)}</select></label>{["pull","issue"].includes(operation)&&<label>{tr("Nummer","Number")}<input name="number" type="number" min={1} max={999999999} required/></label>}{operation.startsWith("create_")&&<><label>{tr("Titel","Title")}<input name="title" maxLength={250} required/></label><label>{tr("Beschreibung","Description")}<textarea name="body" rows={6} maxLength={40000} required/></label></>}{operation==="create_pull"&&<div className="settings-field-grid"><label>{tr("Quellbranch","Head branch")}<input name="head" placeholder="feature-branch" required maxLength={200}/></label><label>{tr("Zielbranch","Base branch")}<input name="base" placeholder="main" required maxLength={200}/></label></div>}<button type="submit">{tr("Aktion prüfen","Review action")}</button></fieldset></form>{error&&<p role="alert" className="error">{error}</p>}<ToolboxActionReview action={action}/></section>;
}
