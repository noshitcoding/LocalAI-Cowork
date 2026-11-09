import {DocumentToolSummary,isDocumentTool} from "./DocumentToolSummary";
import type {Client,Artifact, ResolveApproval, WorkflowRunView} from "@cowork/client";
import {ShieldCheck, Layers} from "lucide-react";
import {useState} from "react";
import {de,type Key} from "./i18n";
import {useView} from "./view";
import {WorkflowNodeList} from "./WorkflowNodeList";
import {completeWorkflowBudgets} from "./workflowUi";
import {WorkflowInputsPanel} from "./WorkflowInputsPanel";
import {WorkflowInputResumePanel,workflowInputCanResume} from "./WorkflowInputResumePanel";
type T=(key:Key)=>string;
function args(value:unknown):Record<string,unknown>{return value&&typeof value==="object"&&!Array.isArray(value)?value as Record<string,unknown>:{};}
function text(value:unknown):string{return typeof value==="string"?value:"";}
export function workflowCanResume(view:WorkflowRunView|undefined):boolean{return !!view&&(!!view.completion_resume_available||!!view.settled_resume_available||workflowInputCanResume(view));}
export function WorkflowRunPanel({client,view,t,busy,artifacts,approve,changed}:{client:Client;view:WorkflowRunView;t:T;busy:boolean;artifacts:Artifact[];approve:(id:string,input:ResolveApproval)=>void;changed:()=>Promise<void>}) {
  const language=useView(state=>state.language);
  const [resolving,setResolving]=useState(false),[resolution,setResolution]=useState<string|null>(null);
  const reconcile=async()=>{setResolving(true);setResolution(null);try{
    const report=await client.one(`/runs/${encodeURIComponent(view.run.id)}/workflow/reconcile-effects`,"WorkflowEffectResolutionView","POST");
    setResolution(language==="de"?`${report.settled_calls.length} Aktionen anhand vorhandener Belege geklärt. ${report.unresolved_calls.length} Aktionen bleiben ungeklärt.`:`${report.settled_calls.length} actions settled from retained evidence. ${report.unresolved_calls.length} actions remain unresolved.`);await changed();
  }catch(error){setResolution(String(error));}finally{setResolving(false);}};
  const budgets=completeWorkflowBudgets(view.run.spec.budgets);
  const number=(value:number)=>value.toLocaleString(language);
  const clockNotices=(["workflow_clock_unverified","workflow_clock_rollback"] as const).filter(code=>view.run.error_code===code||view.inputs?.some(input=>input.state==="closed"&&input.closed_reason===code));
  return <section className="workflow-progress"><header><Layers size={17}/><h2>{view.roles.length===0?(language==="de"?"Ablauffortschritt":"Workflow progress"):t("workflowProgress")}</h2></header><p><strong>{view.name}</strong> · {t("workflowVersion")} {view.version_revision}</p>{view.roles.length>0&&<p className="muted">{t("workflowActiveHint")}</p>}
    {view.run.state==="waiting_for_timer"&&<p className="banner" role="status">{language==="de"?"Der Ablauf wartet bis zum angezeigten Termin. Andere Aufträge können währenddessen weiterlaufen.":"The workflow is waiting until the displayed time. Other tasks can continue in the meantime."}{view.run.spec.access_mode!=="read_only"&&(view.run.spec.folders?.length??0)>0&&<>{" "}{language==="de"?"Die Arbeitsordner dieses Laufs bleiben bis zum Abschluss oder Abbruch geschützt.":"This run's working folders remain protected until completion or cancellation."}</>}</p>}
    {clockNotices.map(code=><p className="banner" role="status" key={code}>{t(code)}</p>)}
    <p className="workflow-accounting">{view.roles.length===0?(language==="de"?"Ablaufschritte":"Workflow steps"):t("steps")}: {number(view.steps_used)} / {number(budgets.max_steps)} · {t("workflowChargedTokens")}: {number(view.charged_tokens)} / {number(budgets.max_total_tokens)}</p>
    <WorkflowNodeList view={view} t={t} language={language}/>
    {view.approvals.map(item=>{const approval=item.approval;const arguments_=args(approval.tool.arguments);const toolName=approval.tool.name;const source=artifacts.find(artifact=>artifact.id===arguments_.id);const node=view.nodes.find(candidate=>candidate.id===item.node_id);
      const roleLabel=node?.scope_id?view.nodes.find(scope=>scope.id===node.scope_id)?.goal??(language==="de"?"Unterablauf":"Subflow"):item.role_id??node?.goal??item.node_id;
      return <section className="approval workflow-approval" key={approval.id}><div className="approval-title"><ShieldCheck size={21}/><h2>{t("approvalTitle")}</h2></div><p><strong>{item.role_id?t("workflowApprovalAgent"):(language==="de"?"Freigabe für Schritt":"Approval for step")}: {roleLabel}</strong>{node&&roleLabel!==node.goal?" · "+node.goal:""}</p><p>{t("approvalDescription")}</p>
        <h3>{isDocumentTool(toolName)?t(toolName as "document_read"|"document_write"|"image_edit"):t(toolName==="create_artifact"?"workflowCreateArtifact":toolName==="list_artifacts"?"workflowListArtifacts":toolName==="read_artifact"?"workflowReadArtifact":toolName==="request_input"?"workflowRequestInput":"details")}</h3>
        {isDocumentTool(toolName)?<DocumentToolSummary name={toolName} args={arguments_} folders={view.run.spec.folders??[]} t={t}/>:<p>{t(toolName==="request_input"?"workflowInputApprovalHint":"workflowArtifactScope")}</p>}
        {toolName==="create_artifact"&&<><dl><dt>{t("workflowArtifactName")}</dt><dd>{text(arguments_.name)}</dd><dt>{t("workflowMediaType")}</dt><dd>text/plain; charset=utf-8</dd></dl><h4>{t("workflowArtifactContent")}</h4><pre>{text(arguments_.content)}</pre></>}
        {toolName==="read_artifact"&&<p>{source?.name??t("workflowReadArtifact")}</p>}{toolName==="request_input"&&<p className="workflow-input-question">{text(arguments_.question)}</p>}
        <details><summary>{t("diagnostics")}</summary><pre>{JSON.stringify({node_id:item.node_id,approval_id:approval.id,fingerprint:approval.fingerprint,tool:approval.tool},null,2)}</pre></details>
        <div className="actions"><button type="button" disabled={busy} onClick={()=>approve(approval.id,{fingerprint:approval.fingerprint,allow:false})}>{t("deny")}</button><button type="button" className="primary" disabled={busy} onClick={()=>approve(approval.id,{fingerprint:approval.fingerprint,allow:true})}>{t("approve")}</button></div></section>;
    })}
    {view.run.state==="interrupted"&&view.input_resume_blocked_reason&&<details><summary>{t("diagnostics")}</summary><pre>{JSON.stringify({input_resume_blocked_reason:view.input_resume_blocked_reason},null,2)}</pre></details>}
    {view.run.state==="interrupted"&&view.settled_resume_available&&<p>Die gespeicherten Modellantworten und Werkzeugbelege sind vollständig. Fortsetzen führt nur die nächsten noch nicht gestarteten Aktionen aus.</p>}
    {view.run.state==="interrupted"&&view.settled_resume_blocked_reason&&<p>Eine externe Aktion oder Modellantwort ist noch ungeklärt. Ihr tatsächliches Ergebnis muss zuerst anhand der vorhandenen Ausführungsbelege geklärt werden.</p>}
    {view.run.state==="interrupted"&&<div><button type="button" disabled={busy||resolving} onClick={()=>void reconcile()}>{language==="de"?"Ausführungsbelege prüfen":"Check execution evidence"}</button>{resolution&&<p role="status">{resolution}</p>}</div>}
    {(!!view.inputs?.length||view.input_resume_available)&&<WorkflowInputResumePanel client={client} view={view} t={t} busy={busy} changed={changed}/>}
    <WorkflowInputsPanel client={client} view={view} t={t} busy={busy} changed={changed}/>
    {view.run.state==="interrupted"&&!workflowCanResume(view)&&<p className="banner">{t(view.inputs?.length?"workflowInputResumeUnavailable":"workflowResumeUnavailable")}</p>}
  </section>;
}
