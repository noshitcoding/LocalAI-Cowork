import type {ProviderProfile,WorkflowVersion} from "@cowork/client";
import type {Key} from "./i18n";
import {completeWorkflowBudgets,editableWorkflow} from "./workflowUi";
import {FrozenOutputPreview} from "./StructuredOutputEditor";
import {WorkflowDataflowSummary} from "./WorkflowDataflowEditor";
type T=(key:Key)=>string;
export function WorkflowVersionPreview({version,providers,t}:{version:WorkflowVersion;providers:ProviderProfile[];t:T}) {
  const definition=version.definition;
  const budgets=completeWorkflowBudgets(definition.budgets);
  return <details className="workflow-version-preview"><summary>{t("workflowModels")} · {definition.roles.map(role=>role.id).join(", ")}</summary><p>{t("workflowRemoteNotice")}</p>
    {!editableWorkflow(definition)&&<p role="alert">{t("workflowCapabilityUnavailable")}</p>}
    <ul>{definition.roles.map(role=>{const profile=providers.find(provider=>provider.id===role.provider_id&&provider.revision===role.provider_revision);return <li key={role.id}><strong>{role.id}</strong>{profile?<><p>{profile.name} · {profile.model}</p><p>{profile.endpoint}</p>{profile.credential_required&&!profile.credential_ready&&<p role="alert">{t("provider_unavailable")}</p>}</>:<p role="alert">{t("workflowProviderChanged")}</p>}<FrozenOutputPreview value={role.output}/></li>;})}</ul>
    <p>{t("workflowRightsHint")}</p><ul>{definition.rights.tools.map(tool=><li key={tool}>{t(tool==="create_artifact"?"workflowCreateArtifact":tool==="list_artifacts"?"workflowListArtifacts":tool==="read_artifact"?"workflowReadArtifact":tool==="request_input"?"workflowRequestInput":"workflowCapabilityUnavailable")}</li>)}</ul>{!definition.rights.tools.length&&<p>{t("workflowNoTools")}</p>}
    <dl><dt>{t("maxRunSteps")}</dt><dd>{budgets.max_steps}</dd><dt>{t("maxRunSeconds")}</dt><dd>{budgets.max_seconds}</dd><dt>{t("maxReplyTokens")}</dt><dd>{budgets.max_output_tokens}</dd><dt>{t("maxRunTokens")}</dt><dd>{budgets.max_total_tokens}</dd></dl>
    <WorkflowDataflowSummary definition={definition}/>
  </details>;
}
