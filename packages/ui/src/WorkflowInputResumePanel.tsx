import {useMemo} from "react";
import type {Client,ResumeWorkflowInput,WorkflowOperation,WorkflowRunView} from "@cowork/client";
import type {Key} from "./i18n";
import {useWorkflowRecovery,WorkflowOperationPanel} from "./workflowJournal";

type T=(key:Key)=>string;
const hash=(value:unknown):value is string=>typeof value==="string"&&/^[a-f0-9]{64}$/.test(value);
export function workflowInputCanResume(view:WorkflowRunView|undefined):boolean {
  const preview=view?.input_resume_preview;
  return !!view&&view.run.state==="interrupted"&&view.state==="interrupted"&&view.input_resume_available===true
    &&preview?.available===true&&preview.blocked_reason==null
    &&preview.base_run_revision===view.run.revision&&Number.isSafeInteger(preview.base_run_revision)&&preview.base_run_revision>0&&preview.base_run_revision<Number.MAX_SAFE_INTEGER
    &&Number.isSafeInteger(preview.base_workflow_revision)&&preview.base_workflow_revision>0&&preview.base_workflow_revision<Number.MAX_SAFE_INTEGER
    &&hash(preview.expected_checkpoint_sha256)&&hash(preview.expected_cursor_sha256);
}
/** The service rechecks these expectations atomically. A preview never grants authority. */
export function workflowInputResumeRequest(view:WorkflowRunView):ResumeWorkflowInput {
  if(!workflowInputCanResume(view))throw new Error("workflowInputResumeUnavailable");
  const preview=view.input_resume_preview!;
  return {run_id:view.run.id,base_run_revision:preview.base_run_revision,
    base_workflow_revision:preview.base_workflow_revision,expected_checkpoint_sha256:preview.expected_checkpoint_sha256,
    expected_cursor_sha256:preview.expected_cursor_sha256!,idempotency_key:crypto.randomUUID()};
}
export function WorkflowInputResumePanel({client,view,t,busy,changed}:{client:Client;view:WorkflowRunView;t:T;busy:boolean;changed:()=>Promise<void>}) {
  const run=view.run.id;
  const matches=useMemo(()=>(operation:WorkflowOperation)=>operation.request.kind==="resume_input"&&operation.request.request.run_id===run,[run]);
  const journal=useWorkflowRecovery(client,view.run.spec.project_id,matches);
  const available=workflowInputCanResume(view);
  async function begin(){await journal.begin(()=>({kind:"resume_input",request:workflowInputResumeRequest(view)}));await changed();}
  async function action(kind:"commit"|"discard"|"acknowledge"){await journal.action(kind);await changed();}
  if(!available&&!journal.operation&&!journal.uncertainPrepare)return null;
  return <section className="workflow-input-resume" aria-label={t("workflowInputResume")}>
    <p>{t("workflowInputResumeHint")}</p>
    {available&&!journal.operation&&!journal.uncertainPrepare&&<button type="button" disabled={busy||journal.busy||!journal.ready} onClick={()=>void begin()}>{t("workflowInputResume")}</button>}
    <WorkflowOperationPanel operation={journal.operation} busy={busy||journal.busy} error={journal.error} t={t}
      commit={()=>void action("commit")} discard={()=>void action("discard")} acknowledge={()=>void action("acknowledge")}/>
    {journal.operation?.receipt?.kind==="input_resume_queued"&&<p role="status">{t("workflowInputResumeQueued")}</p>}
    {journal.uncertainPrepare&&<p role="status">{t("workflowJournalPreparePending")} <button type="button" disabled={busy||journal.busy} onClick={()=>void begin()}>{t("workflowRetrySame")}</button></p>}
    {!journal.query.isSuccess&&<p role={journal.query.isError?"alert":"status"}>{t(journal.query.isError?"workflowJournalUnavailable":"workflowJournalLoading")}</p>}
  </section>;
}
