import {Client,decode,type ProcessAttempt,type ReconcileOwnerManagedProcess,type RunRecord} from "@cowork/client";
import {PersistentRequestLatch} from "./requestJournal";

export function decodeManagedProcessIntent(value:unknown):ReconcileOwnerManagedProcess {
  const intent=decode("ReconcileOwnerManagedProcess",value);
  if(!Number.isSafeInteger(intent.expected_run_revision)||intent.expected_run_revision<=0
    ||intent.expected_run_revision>=Number.MAX_SAFE_INTEGER||!/^[a-f0-9]{64}$/.test(intent.expected_request_sha256))throw new Error("managed_process_intent_invalid");
  return intent;
}
export function originalManagedProcessRecheckAvailable(
  journal:PersistentRequestLatch<ReconcileOwnerManagedProcess>,run:RunRecord,attempt:ProcessAttempt
):boolean {
  if(!journal.pending||journal.invalid)return false;
  const intent=journal.prepare(()=>{throw new Error("original_request_missing");});
  return attempt.run_id===run.id&&attempt.project_id===run.spec.project_id
    &&attempt.request_sha256===intent.expected_request_sha256
    &&(run.revision===intent.expected_run_revision&&run.state==="interrupted"&&attempt.state.state==="launch_committed"
      ||run.revision===intent.expected_run_revision+1&&run.state===(intent.finish_cancellation?"canceled":"interrupted")
        &&attempt.state.state==="reconciled_completed");
}
export async function submitManagedProcessReconciliation(
  client:Client,order:string,run:string,attempt:string,
  journal:PersistentRequestLatch<ReconcileOwnerManagedProcess>,build:()=>ReconcileOwnerManagedProcess
) {
  const intent=journal.prepare(build);
  const response=await client.reconcileOwnerManagedProcess(order,run,attempt,intent);
  journal.confirm();
  return response;
}
