import type {WorkflowNodeView,WorkflowRunView} from "@cowork/client";
import {de,type Key} from "./i18n";

const labels:Record<WorkflowNodeView["state"],Key>={waiting_timer:"workflowWaitingTimer",skipped:"workflowNodeSkipped",pending:"workflowNodePending",ready:"workflowNodeReady",running:"running",waiting_approval:"workflowWaitingApproval",waiting_input:"waiting_input",completed:"completed",failed:"failed",blocked:"workflowNodeBlocked",interrupted:"interrupted"};
type Branch={node:WorkflowNodeView;index:number;children:Branch[]};
const members=(branch:Branch):WorkflowNodeView[]=>[branch.node,...branch.children.flatMap(members)];

function order(branches:Branch[]):Branch[]{
  const owner=new Map(branches.flatMap(branch=>members(branch).map(node=>[node.id,branch.node.id] as const)));
  const pending=[...branches],done=new Set<string>(),sorted:Branch[]=[];
  while(pending.length){
    const index=pending.findIndex(branch=>members(branch).every(node=>node.dependencies.every(id=>!owner.has(id)||owner.get(id)===branch.node.id||done.has(owner.get(id)!))));
    const [branch]=pending.splice(index<0?0:index,1);done.add(branch.node.id);sorted.push({...branch,children:order(branch.children)});
  }
  return sorted;
}

/** Preserve every node even if an older or damaged response has orphaned scopes. */
export function workflowNodeTree(nodes:WorkflowNodeView[]):Branch[]{
  const seen=new Set<string>();
  const parent=(node:WorkflowNodeView)=>nodes.find(candidate=>candidate.id===node.scope_id&&candidate.kind==="subflow"&&candidate.id!==node.id);
  const branch=(node:WorkflowNodeView):Branch=>{
    seen.add(node.id);
    return {node,index:nodes.indexOf(node),children:nodes.filter(child=>parent(child)?.id===node.id&&!seen.has(child.id)).map(branch)};
  };
  const roots=nodes.filter(node=>!parent(node)).map(branch);
  for(const node of nodes)if(!seen.has(node.id))roots.push(branch(node));
  return order(roots);
}

export function WorkflowNodeList({view,t,language}:{view:WorkflowRunView;t:(key:Key)=>string;language:string}){
  const tr=(a:string,b:string)=>language==="de"?a:b,number=(value:number)=>value.toLocaleString(language);
  const tree=workflowNodeTree(view.nodes),positions=new Map<string,string>();
  const numberBranches=(branches:Branch[],prefix="")=>branches.forEach((branch,index)=>{const position=prefix+(index+1);positions.set(branch.node.id,position);numberBranches(branch.children,position+".");});
  numberBranches(tree);
  const counts=(branch:Branch):{steps:number;tokens:number}=>branch.children.reduce((sum,child)=>{const count=counts(child);return {steps:sum.steps+count.steps,tokens:sum.tokens+count.tokens};},{steps:branch.node.steps,tokens:branch.node.charged_tokens});
  function render(branch:Branch){
    const {node,children}=branch,role=view.roles.find(item=>item.role_id===node.role_id),total=counts(branch),contained=new Set(members(branch).map(node=>node.id));
    const dependencies=node.kind==="subflow"?[...new Set(members(branch).flatMap(member=>member.dependencies).filter(id=>!contained.has(id)))]:node.dependencies;
    const name=node.kind==="subflow"?tr("Unterablauf","Subflow"):node.kind==="arguments"?tr("Eingaben übernehmen","Bind inputs"):node.scope_id&&node.role_id?tr("Agent","Agent"):node.role_id??tr("Schritt","Step");
    return <li key={node.id} className={node.kind==="subflow"?"workflow-subflow":undefined}>
      <div className="workflow-node-heading"><strong>{positions.get(node.id)}. {name}</strong><span className={"badge "+node.state}>{t(labels[node.state])}</span></div>
      <p>{node.goal}</p>
      {node.wake_at!=null&&<p className="muted">{tr("Früheste Fortsetzung: ","Earliest continuation: ")}{new Date(node.wake_at*1000).toLocaleString(language,{timeZoneName:"short"})}</p>}
      {node.skip_reason&&<p className="muted">{node.skip_reason==="condition_not_met"?tr("Die Bedingung trifft für diesen Zweig nicht zu.","The condition does not select this branch."):node.skip_reason==="empty_selection"?tr("Kein Zweig hat ein Ergebnis geliefert.","No branch produced a result."):tr("Ein benötigter vorheriger Schritt wurde übersprungen.","A required preceding step was skipped.")}</p>}
      {role&&<p className="muted">{role.provider.name} · {role.provider.model}</p>}
      {!!dependencies.length&&<p className="muted">{t("workflowDependencies")}: {dependencies.map(id=>{const predecessor=view.nodes.find(item=>item.id===id);return predecessor?`${positions.get(id)}. ${predecessor.goal}`:t("workflowUnavailable");}).join("; ")}</p>}
      {node.kind==="subflow"&&<p className="workflow-accounting">{tr("Unterablauf insgesamt","Subflow total")}: {tr("Schritte","Steps")}: {number(total.steps)} · {t("workflowChargedTokens")}: {number(total.tokens)}</p>}
      <details><summary>{t("diagnostics")}</summary><dl><dt>{t("steps")}</dt><dd>{number(node.steps)}</dd><dt>{t("workflowChargedTokens")}</dt><dd>{number(node.charged_tokens)}</dd></dl><pre>{JSON.stringify({node_id:node.id,role_id:node.role_id,scope_id:node.scope_id,error_code:node.error_code},null,2)}</pre></details>
      {node.error_code&&<p>{t(node.error_code in de?node.error_code as Key:"error")}</p>}
      {!!children.length&&<ol className="workflow-node-list">{children.map(render)}</ol>}
    </li>;
  }
  return <ol className="workflow-node-list">{tree.map(render)}</ol>;
}
