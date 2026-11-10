import {decode,RequestError,type Client,type ReconcileOwnerManagedProcess,type OwnerManagedProcessReconciliation} from "./index";

export async function reconcileOwnerManagedProcess(client:Client,order:string,run:string,attempt:string,input:ReconcileOwnerManagedProcess):Promise<OwnerManagedProcessReconciliation> {
  const intent=decode("ReconcileOwnerManagedProcess",input);
  if(!Number.isSafeInteger(intent.expected_run_revision)||intent.expected_run_revision<=0
    ||intent.expected_run_revision>=Number.MAX_SAFE_INTEGER||!/^[a-f0-9]{64}$/.test(intent.expected_request_sha256))throw new Error("managed_process_intent_invalid");
  // One explicit mutation. Transport failure never triggers a launch or retry.
  const response=await fetch(client.base+`/api/v2/orders/${encodeURIComponent(order)}/runs/${encodeURIComponent(run)}/managed-processes/${encodeURIComponent(attempt)}/reconcile`,{
    method:"POST",headers:{Authorization:"Bearer "+client.token,"Content-Type":"application/json"},
    cache:"no-store",redirect:"error",body:JSON.stringify(intent)
  });
  const value:unknown=await response.json();
  if(!response.ok)throw new RequestError(decode("ApiError",value),response.status);
  const result=decode("OwnerManagedProcessReconciliation",value),process=result.process_attempt;
  if(result.run.id!==run||result.run.spec.order_id!==order||result.run.spec.workflow
    ||result.run.revision!==intent.expected_run_revision+1||result.run.state!==(intent.finish_cancellation?"canceled":"interrupted")
    ||process.id!==attempt||process.run_id!==run||process.project_id!==result.run.spec.project_id
    ||process.revision!==2||process.request_sha256!==intent.expected_request_sha256
    ||process.state.state!=="reconciled_completed"||!/^[a-f0-9]{64}$/.test(process.state.evidence_sha256))throw new Error("managed_process_reconciliation_integrity");
  return result;
}
