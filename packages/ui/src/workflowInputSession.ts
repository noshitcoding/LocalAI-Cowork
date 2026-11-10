import {workflowDraftKey} from "./workflowJournal";
import {Client,type WorkflowInputReceipt,type WorkflowInputDraftLookup,type WorkflowInputDraft,type WorkflowInputDraftSave,type WorkflowInputAnswerIntent,type WorkflowInputDraftSnapshot,type WorkflowInputDraftSnapshotSummary,type WorkflowInputAnswerIntentSummary,type SaveWorkflowInputDraft,type PrepareWorkflowInputAnswer} from "@cowork/client";

export const workflowAnswerBytes=(answer:string)=>new TextEncoder().encode(answer).length;
export const workflowAnswerValid=(answer:string)=>workflowAnswerBytes(answer)<=32_000&&!answer.includes("\0")&&/[^\p{White_Space}]/u.test(answer);
const fail=()=>new Error("workflowInputBinding");
const identity=(value:WorkflowInputAnswerIntent)=>workflowDraftKey({id:value.id,run_id:value.run_id,project_id:value.project_id,request_id:value.request_id,fingerprint:value.fingerprint,owner:value.owner,request_sha256:value.request_sha256,submission:value.submission,draft_snapshot:value.draft_snapshot});
const answerHash=async(answer:string)=>Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256",new TextEncoder().encode(answer)))).map(value=>value.toString(16).padStart(2,"0")).join("");
/** Holds identical unconfirmed commands only. The service owns every durable draft and intent. */
export class WorkflowInputSession {
  private owner:string|null=null;private active=true;private epoch=0;private readSequence=0;
  private heads=new Map<number,string>();
  private saving:Promise<WorkflowInputDraftSave>|null=null;
  private preparing:Promise<WorkflowInputAnswerIntent>|null=null;
  private saveRequest:SaveWorkflowInputDraft|null=null;
  private prepareRequest:PrepareWorkflowInputAnswer|null=null;
  private prepareAnswerHash:string|null=null;
  intent:WorkflowInputAnswerIntent|null=null;
  private readonly initialReviewValues:unknown;
  constructor(private client:Client,readonly project:string,readonly run:string,readonly request:string,readonly fingerprint:string,initialReviewValues?:unknown){
    this.initialReviewValues=structuredClone(initialReviewValues);
  }
  private initialAnswerMatches(answer:string):boolean {
    if(this.initialReviewValues===undefined)return answer==="";
    if(workflowAnswerBytes(answer)>32_000||answer.includes("\0"))return false;
    try{return workflowDraftKey(JSON.parse(answer))===workflowDraftKey(this.initialReviewValues);}catch{return false;}
  }
  activate(){this.active=true;this.epoch++;this.readSequence++;}
  dispose(){this.active=false;this.epoch++;this.readSequence++;}
  get generation(){return this.epoch;}
  isCurrent(generation:number){return this.active&&generation===this.epoch;}
  private current(generation:number){if(!this.isCurrent(generation))throw new Error("workflowInputDraftUnavailable");}
  get pendingSave(){return this.saveRequest!==null;}
  get unconfirmedSave(){return this.saveRequest?structuredClone(this.saveRequest):null;}
  get uncertainPrepare(){return this.prepareRequest!==null&&this.intent?.submission.idempotency_key!==this.prepareRequest.idempotency_key;}
  get canContinueRejected(){return this.intent?.state==="rejected"&&!this.uncertainPrepare;}
  private scope(value:{run_id:string;project_id:string;request_id:string;fingerprint:string;owner:string}){
    if(value.run_id!==this.run||value.project_id!==this.project||value.request_id!==this.request||value.fingerprint!==this.fingerprint||!value.owner||(this.owner!==null&&value.owner!==this.owner))throw fail();
    this.owner=value.owner;
  }
  private async draft(value:WorkflowInputDraft){
    this.scope(value);if(value.answer_sha256!==await answerHash(value.answer)||value.revision<0||(value.revision===0?(!this.initialAnswerMatches(value.answer)||value.snapshot_id!==null):!value.snapshot_id))throw fail();
    const key=workflowDraftKey({answer:value.answer,sha256:value.answer_sha256,snapshot_id:value.snapshot_id});
    if(this.heads.has(value.revision)&&this.heads.get(value.revision)!==key)throw fail();this.heads.set(value.revision,key);
  }
  private async intention(value:WorkflowInputAnswerIntent){
    this.scope(value);
    if(!workflowAnswerValid(value.submission.input.answer)||!value.id||!value.submission.idempotency_key||!value.draft_snapshot.snapshot_id||value.draft_snapshot.revision<1||value.submission.input.request_id!==this.request||value.submission.fingerprint!==this.fingerprint||value.draft_snapshot.answer_sha256!==await answerHash(value.submission.input.answer))throw fail();
    if(!/^[0-9a-f]{64}$/.test(value.request_sha256)||value.state==="pending"&&(value.receipt!==null||value.rejection!==null)||value.state==="accepted"&&(!value.receipt||value.rejection!==null)||value.state==="rejected"&&(!value.rejection||value.receipt!==null))throw fail();
    if(value.receipt&&(!value.receipt.receipt_id||value.receipt.confirmed_sequence<1||value.receipt.request_id!==this.request||value.receipt.fingerprint!==this.fingerprint||value.receipt.answer_sha256!==value.draft_snapshot.answer_sha256))throw fail();
    return value;
  }
  private async summarizedIntent(summary:WorkflowInputAnswerIntentSummary){
    const value=await this.intention(await this.client.workflowInputAnswerIntent(this.run,summary.id));
    if(value.id!==summary.id||value.draft_snapshot.answer_sha256!==summary.answer_sha256||value.draft_snapshot.revision!==summary.draft_revision||value.created_at!==summary.created_at||summary.state!=="pending"&&value.state!==summary.state)throw fail();return value;
  }
  private async lookup(){const result=await this.client.workflowInputDraft(this.run,this.request);await this.draft(result.draft);
    if(result.intents.length>16||result.snapshots.length>128||new Set(result.intents.map(item=>item.id)).size!==result.intents.length||new Set(result.snapshots.map(item=>item.id)).size!==result.snapshots.length)throw fail();
    if(result.intents.filter(item=>item.state==="pending").length>1)throw new Error("workflowInputMultiplePending");return result;}
  async read(confirmedReceipt?:WorkflowInputReceipt|null):Promise<WorkflowInputDraftLookup|null>{
    const sequence=++this.readSequence,generation=this.generation,receipt=structuredClone(confirmedReceipt);
    try{const result=await this.lookup();let intent:WorkflowInputAnswerIntent|null=null;
      if(this.uncertainPrepare){for(const summary of result.intents){const value=await this.summarizedIntent(summary);if(value.submission.idempotency_key===this.prepareRequest?.idempotency_key){if(value.draft_snapshot.answer_sha256!==this.prepareAnswerHash||value.draft_snapshot.revision!==this.prepareRequest.base_draft_revision)throw fail();intent=value;break;}}}
      if(!intent){const id=this.intent?.id??result.intents.find(item=>item.state==="pending")?.id;
        if(id){const summary=result.intents.find(item=>item.id===id);if(!summary)throw fail();intent=await this.summarizedIntent(summary);if(this.intent?.id===id&&identity(this.intent)!==identity(intent))throw fail();}}
      if(!intent&&receipt){const accepted=result.intents.filter(item=>item.state==="accepted");if(accepted.length!==1)throw fail();intent=await this.summarizedIntent(accepted[0]);}
      if(receipt&&intent?.state==="accepted"&&workflowDraftKey(intent.receipt)!==workflowDraftKey(receipt))throw fail();
      if(!this.isCurrent(generation)||sequence!==this.readSequence)return null;
      if(intent)this.intent=intent;return result;
    }catch(error){if(!this.isCurrent(generation)||sequence!==this.readSequence)return null;throw error;}
  }
  async currentDraft():Promise<WorkflowInputDraftLookup>{const generation=this.generation;const value=await this.lookup();this.current(generation);return value;}
  async snapshot(summary:WorkflowInputDraftSnapshotSummary):Promise<WorkflowInputDraftSnapshot>{
    const generation=this.generation;const value=await this.client.workflowInputDraftSnapshot(this.run,this.request,summary.id);this.scope(value);
    if(value.id!==summary.id||value.base_revision!==summary.base_revision||value.revision!==summary.revision||value.answer_sha256!==summary.answer_sha256||value.conflict!==summary.conflict||value.created_at!==summary.created_at||value.answer_sha256!==await answerHash(value.answer)||value.revision<1||value.base_revision<0||!value.conflict&&value.revision!==value.base_revision+1)throw fail();
    this.current(generation);return value;
  }
  save(answer:string,revision:number):Promise<WorkflowInputDraftSave>{
    if(this.saving)return this.saving;
    const generation=this.generation;this.current(generation);
    this.saveRequest??=structuredClone({fingerprint:this.fingerprint,base_revision:revision,idempotency_key:crypto.randomUUID(),answer});
    const input=structuredClone(this.saveRequest);
    const task=(async()=>{const result=await this.client.saveWorkflowInputDraft(this.run,this.request,input);await this.draft(result.draft);this.scope(result.snapshot);
      const hash=await answerHash(input.answer);
      if(result.snapshot.conflict!==(result.outcome==="conflict")||result.snapshot.answer!==input.answer||result.snapshot.answer_sha256!==hash||result.snapshot.base_revision!==input.base_revision||result.snapshot.revision!==(result.outcome==="saved"?input.base_revision+1:result.draft.revision)||result.outcome==="saved"&&(result.draft.answer!==input.answer||result.draft.answer_sha256!==hash||result.draft.revision!==result.snapshot.revision||result.draft.snapshot_id!==result.snapshot.id))throw fail();
      this.current(generation);this.saveRequest=null;return result;})();
    this.saving=task;void task.finally(()=>{if(this.saving===task)this.saving=null;}).catch(()=>{});return task;
  }
  prepare(revision:number,answer:string):Promise<WorkflowInputAnswerIntent>{
    if(this.preparing)return this.preparing;
    const task=this.prepareOnce(revision,answer);this.preparing=task;void task.finally(()=>{if(this.preparing===task)this.preparing=null;}).catch(()=>{});return task;
  }
  private async prepareOnce(revision:number,answer:string){
    const generation=this.generation;this.current(generation);
    if(this.pendingSave)throw new Error("workflowInputUnsaved");
    if(this.intent&&!this.uncertainPrepare)return this.intent;
    const lookup=await this.lookup();
    const found:WorkflowInputAnswerIntent[]=[];
    for(const summary of lookup.intents){
      const value=await this.summarizedIntent(summary);
      if(this.prepareRequest&&value.submission.idempotency_key===this.prepareRequest.idempotency_key){if(value.draft_snapshot.answer_sha256!==this.prepareAnswerHash||value.draft_snapshot.revision!==this.prepareRequest.base_draft_revision)throw fail();this.current(generation);this.intent=value;return value;}
      found.push(value);
    }
    const pending=found.filter(value=>value.state==="pending");if(pending.length>1)throw new Error("workflowInputMultiplePending");
    if(pending[0]){this.current(generation);this.intent=pending[0];return pending[0];}
    if(!this.prepareRequest){
      if(lookup.draft.revision!==revision||lookup.draft.answer!==answer)throw new Error("workflowInputConflict");
      this.current(generation);this.prepareAnswerHash=lookup.draft.answer_sha256;
      this.prepareRequest=structuredClone({fingerprint:this.fingerprint,base_draft_revision:revision,idempotency_key:crypto.randomUUID()});
    }
    const input=structuredClone(this.prepareRequest);this.current(generation);
    const value=await this.intention(await this.client.prepareWorkflowInputAnswer(this.run,this.request,input));
    if(value.submission.idempotency_key!==input.idempotency_key||value.draft_snapshot.revision!==input.base_draft_revision||value.draft_snapshot.answer_sha256!==this.prepareAnswerHash)throw fail();
    this.current(generation);this.intent=value;return value;
  }
  async inspect(id:string){const generation=this.generation;const value=await this.intention(await this.client.workflowInputAnswerIntent(this.run,id));if(value.id!==id||this.intent?.id===id&&identity(this.intent)!==identity(value))throw fail();this.current(generation);this.intent=value;return value;}
  async commit(){
    const generation=this.generation;const original=this.intent;if(!original)throw new Error("workflowInputDraftUnavailable");this.current(generation);
    try{const value=await this.intention(await this.client.commitWorkflowInputAnswerIntent(this.run,original.id));
      if(identity(value)!==identity(original))throw fail();
      this.current(generation);this.intent=value;return value;
    }catch(error){
      if(this.isCurrent(generation)){
        try{const value=await this.intention(await this.client.workflowInputAnswerIntent(this.run,original.id));
          if(value.id!==original.id||identity(value)!==identity(original))throw fail();
          this.current(generation);
          if(value.state!=="pending"){this.intent=value;return value;}
        }catch{/* Keep the original intention when a confirmed, matching receipt cannot be read. */}
      }
      throw error;
    }
  }
  continueRejected(){if(!this.canContinueRejected)throw new Error("workflowInputDraftUnavailable");this.intent=null;this.prepareRequest=null;this.prepareAnswerHash=null;}
}
