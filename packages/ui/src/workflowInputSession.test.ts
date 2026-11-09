import {afterEach,describe,expect,it,vi} from "vitest";
import {Client,type WorkflowInputDraft,type WorkflowInputDraftLookup,type WorkflowInputDraftSnapshot,type WorkflowInputDraftSnapshotSummary,type WorkflowInputAnswerIntent,type SaveWorkflowInputDraft,type PrepareWorkflowInputAnswer} from "@cowork/client";
import {WorkflowInputSession,workflowAnswerBytes,workflowAnswerValid} from "./workflowInputSession";

const scope={run_id:"run",project_id:"project",request_id:"question",fingerprint:"f".repeat(64),owner:"single_owner_preview"};
const hash=async(value:string)=>Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256",new TextEncoder().encode(value)))).map(value=>value.toString(16).padStart(2,"0")).join("");
const response=(value:unknown,status=200)=>new Response(JSON.stringify(value),{status,headers:{"Content-Type":"application/json"}});
const apiError=(code:string,status=409)=>response({code,message:"Synthetic service error",retryable:false,correlation_id:"synthetic"},status);
async function head(answer="",revision=0):Promise<WorkflowInputDraft>{return {...scope,revision,answer,answer_sha256:await hash(answer),snapshot_id:revision?"snapshot-"+revision:null,updated_at:revision};}
async function snapshot(answer:string,revision=1,base_revision=revision-1,conflict=false):Promise<WorkflowInputDraftSnapshot>{return {...scope,id:conflict?"conflict-copy":"snapshot-"+revision,base_revision,revision,answer,answer_sha256:await hash(answer),conflict,created_at:revision};}
const summary=(value:WorkflowInputDraftSnapshot):WorkflowInputDraftSnapshotSummary=>({id:value.id,revision:value.revision,base_revision:value.base_revision,answer_sha256:value.answer_sha256,conflict:value.conflict,created_at:value.created_at});
async function intention(answer="  Original \"ü\"\\path\r\nline\n終 🟡  ",revision=1):Promise<WorkflowInputAnswerIntent>{return {...scope,id:"intent",request_sha256:"a".repeat(64),submission:{input:{request_id:scope.request_id,answer},fingerprint:scope.fingerprint,idempotency_key:"original-key"},draft_snapshot:{snapshot_id:"snapshot-"+revision,revision,answer_sha256:await hash(answer)},state:"pending",receipt:null,rejection:null,created_at:1,updated_at:1};}
const intentSummary=(value:WorkflowInputAnswerIntent)=>({id:value.id,state:value.state,answer_sha256:value.draft_snapshot.answer_sha256,draft_revision:value.draft_snapshot.revision,created_at:value.created_at});
const session=()=>new WorkflowInputSession(new Client("synthetic","http://fixture"),scope.project_id,scope.run_id,scope.request_id,scope.fingerprint);
function serve(handler:(path:string,init:RequestInit)=>Promise<Response>){const calls:{path:string;init:RequestInit}[]=[];vi.stubGlobal("fetch",vi.fn(async(path:unknown,init:RequestInit={})=>{calls.push({path:String(path),init});return handler(String(path),init);}));return calls;}
afterEach(()=>vi.unstubAllGlobals());

describe("workflow question UI service boundary",()=>{
  it("restores the receipt-bound accepted form answer on reopen without using a newer draft or sending",async()=>{
    const original=await intention('{"invoice":"CONFIRMED","gross_minor":11900}');
    const accepted:WorkflowInputAnswerIntent={...original,state:"accepted",receipt:{request_id:scope.request_id,fingerprint:scope.fingerprint,receipt_id:"receipt",answer_sha256:original.draft_snapshot.answer_sha256,confirmed_sequence:8}};
    const latest=await head('{"invoice":"UNSENT","gross_minor":1}',2),receipt=structuredClone(accepted.receipt!);
    const calls=serve(async(path)=>response(path.endsWith("/draft")?{draft:latest,snapshots:[],intents:[intentSummary(accepted)]}:accepted));
    const current=session(),reading=current.read(receipt);receipt.receipt_id="mutated-after-read";await reading;
    expect(current.intent).toEqual(accepted);expect(current.intent?.submission.input.answer).toContain("CONFIRMED");expect(current.intent?.submission.input.answer).not.toContain("UNSENT");expect(calls.every(call=>call.init.method==="GET")).toBe(true);
  });

  it.each(["receipt_id","confirmed_sequence","answer_sha256","missing_accepted","duplicate_accepted"] as const)("does not restore an accepted form from a mismatched %s",async(kind)=>{
    const original=await intention('{"invoice":"CONFIRMED","gross_minor":11900}');
    const accepted:WorkflowInputAnswerIntent={...original,state:"accepted",receipt:{request_id:scope.request_id,fingerprint:scope.fingerprint,receipt_id:"receipt",answer_sha256:original.draft_snapshot.answer_sha256,confirmed_sequence:8}};
    const receipt=structuredClone(accepted.receipt!),latest=await head(original.submission.input.answer,1);
    if(kind==="receipt_id")receipt.receipt_id="other-receipt";
    if(kind==="confirmed_sequence")receipt.confirmed_sequence++;
    if(kind==="answer_sha256")receipt.answer_sha256="b".repeat(64);
    const summaries=kind==="missing_accepted"?[]:kind==="duplicate_accepted"?[intentSummary(accepted),{...intentSummary(accepted),id:"other-accepted"}]:[intentSummary(accepted)];
    const calls=serve(async(path)=>response(path.endsWith("/draft")?{draft:latest,snapshots:[],intents:summaries}:accepted));
    const current=session();await expect(current.read(receipt)).rejects.toThrow("workflowInputBinding");expect(current.intent).toBeNull();expect(calls.every(call=>call.init.method==="GET")).toBe(true);
  });
  it("loads only the bound original native review proposal at revision zero without changing the legacy empty-draft rule",async()=>{
    const proposal:{invoice:string|null;gross_minor:number|null}={invoice:null,gross_minor:null},expected=structuredClone(proposal),initial=await head(JSON.stringify(proposal));
    const current=new WorkflowInputSession(new Client("synthetic","http://fixture"),scope.project_id,scope.run_id,scope.request_id,scope.fingerprint,proposal);
    proposal.invoice="mutated";
    const calls=serve(async()=>response({draft:initial,snapshots:[],intents:[]}));
    expect((await current.read())?.draft).toEqual(initial);expect(JSON.parse(initial.answer)).toEqual(expected);
    await expect(session().read()).rejects.toThrow("workflowInputBinding");expect(calls.every(call=>call.init.method==="GET")).toBe(true);
  });

  it("rejects a rebound, malformed or snapshotted native revision-zero proposal before any write",async()=>{
    const proposal={invoice:null,gross_minor:1},initial=await head('{"gross_minor":1.0,"invoice":null}');
    const make=()=>new WorkflowInputSession(new Client("synthetic","http://fixture"),scope.project_id,scope.run_id,scope.request_id,scope.fingerprint,proposal);
    let draft=initial;const calls=serve(async()=>response({draft,snapshots:[],intents:[]}));
    expect((await make().read())?.draft).toEqual(initial);
    for(const invalid of [await head('{"gross_minor":2,"invoice":null}'),await head("{unfinished"),{...initial,snapshot_id:"unexpected"},{...initial,answer_sha256:"0".repeat(64)}]){
      draft=invalid;await expect(make().read()).rejects.toThrow("workflowInputBinding");
    }
    expect(calls.every(call=>call.init.method==="GET")).toBe(true);
  });
  it("counts exact UTF-8 bytes without trimming or normalizing Unicode, CRLF, whitespace or escapes",async()=>{
    const value=await intention();expect(workflowAnswerBytes(value.submission.input.answer)).toBe(new TextEncoder().encode(value.submission.input.answer).length);
    expect(workflowAnswerValid("🟡".repeat(8000))).toBe(true);expect(workflowAnswerValid("🟡".repeat(8001))).toBe(false);
    expect(workflowAnswerValid(" \r\n\t\u0085")).toBe(false);expect(workflowAnswerValid("\uFEFF")).toBe(true);expect(workflowAnswerValid("x\0y")).toBe(false);
    const stored=await head();let saved:SaveWorkflowInputDraft|null=null;
    serve(async(_path,init)=>{saved=JSON.parse(String(init.body));const copy=await snapshot(saved!.answer);return response({outcome:"saved",draft:{...stored,revision:1,answer:copy.answer,answer_sha256:copy.answer_sha256,snapshot_id:copy.id,updated_at:1},snapshot:copy});});
    await session().save(value.submission.input.answer,0);expect(saved!.answer).toBe(value.submission.input.answer);
  });

  it("retains a frozen CAS0 save after lost acknowledgement and retries exactly that body despite newer local text",async()=>{
    const original="  Exact\r\nü 🟡 ";const copy=await snapshot(original);const savedHead=await head(original,1);const received:string[]=[];
    serve(async(_path,init)=>{received.push(String(init.body));if(received.length===1)throw new TypeError("lost save ACK");return response({outcome:"saved",draft:savedHead,snapshot:copy});});
    const current=session();await expect(current.save(original,0)).rejects.toThrow("lost save ACK");expect(current.pendingSave).toBe(true);expect(current.unconfirmedSave?.answer).toBe(original);
    await current.save("Edited after ACK loss",7);expect(received[1]).toBe(received[0]);expect(JSON.parse(received[0]).base_revision).toBe(0);expect(current.pendingSave).toBe(false);
  });

  it("restores the incoming conflict snapshot after a lost 409 ACK and a new UI session without changing the current head",async()=>{
    const server=await head("Server text",2),incoming=await snapshot("  My retained\r\ntext  ",2,0,true);
    const lookup:WorkflowInputDraftLookup={draft:server,snapshots:[summary(incoming)],intents:[]};let conflictSent=false;
    const calls=serve(async(path,init)=>{
      if(init.method==="PUT"){conflictSent=true;throw new TypeError("lost conflict ACK");}
      if(path.endsWith("/snapshots/conflict-copy"))return response(incoming);
      return response(lookup);
    });
    await expect(session().save(incoming.answer,0)).rejects.toThrow("lost conflict ACK");expect(conflictSent).toBe(true);
    const reopened=session();const saved=await reopened.read();expect(saved?.draft.answer).toBe("Server text");
    const copy=await reopened.snapshot(saved!.snapshots[0]);expect(copy.answer).toBe(incoming.answer);expect(copy.conflict).toBe(true);
    expect((await reopened.currentDraft()).draft).toEqual(server);expect(calls.filter(call=>call.init.method!=="GET")).toHaveLength(1);
  });

  it("reconciles the original prepared intention on reopen, retaining its old text after a newer draft and never auto-committing",async()=>{
    const original=await intention(),older=await head(original.submission.input.answer,1);let latest=older;let stored:WorkflowInputAnswerIntent|null=null;const prepared:string[]=[];
    const calls=serve(async(path,init)=>{
      if(path.endsWith("/draft"))return response({draft:latest,snapshots:[],intents:stored?[intentSummary(stored)]:[]});
      if(init.method==="POST"){prepared.push(String(init.body));const body=JSON.parse(String(init.body)) as PrepareWorkflowInputAnswer;stored={...original,submission:{...original.submission,idempotency_key:body.idempotency_key}};throw new TypeError("lost Prepare ACK");}
      return response(stored);
    });
    const first=session();await expect(first.prepare(1,older.answer)).rejects.toThrow("lost Prepare ACK");expect(first.uncertainPrepare).toBe(true);
    latest=await head("Newer draft must not replace frozen answer",2);
    const reopened=session();await reopened.read();expect(reopened.intent?.submission.input.answer).toBe(older.answer);expect(reopened.intent?.id).toBe(original.id);
    expect(prepared).toHaveLength(1);expect(calls.some(call=>call.path.endsWith("/commit"))).toBe(false);
  });

  it("retries only the original Prepare body when storage never confirmed it, even after the draft head changes",async()=>{
    const original=await head("Original",1);let latest=original;const bodies:string[]=[];
    serve(async(path,init)=>{if(path.endsWith("/draft"))return response({draft:latest,snapshots:[],intents:[]});bodies.push(String(init.body));if(bodies.length===1)throw new TypeError("lost unknown Prepare ACK");return apiError("revision_conflict");});
    const current=session();await expect(current.prepare(1,original.answer)).rejects.toThrow("lost unknown Prepare ACK");latest=await head("Changed",2);
    await expect(current.prepare(2,latest.answer)).rejects.toMatchObject({details:{code:"revision_conflict"}});expect(bodies[1]).toBe(bodies[0]);expect(current.uncertainPrepare).toBe(true);
  });

  it("recovers an older pending answer instead of preparing or sending the caller’s newer text",async()=>{
    const frozen=await intention("Old pending answer"),latest=await head("Current editor answer",2);
    const calls=serve(async(path)=>response(path.endsWith("/draft")?{draft:latest,snapshots:[],intents:[intentSummary(frozen)]}:frozen));
    const recovered=await session().prepare(2,latest.answer);expect(recovered.submission.input.answer).toBe("Old pending answer");expect(calls.every(call=>call.init.method==="GET")).toBe(true);
  });

  it("reconciles a lost commit ACK through a GET and repeats only the same intention ID without a mutable answer body",async()=>{
    const value=await intention();let stored=structuredClone(value);let commits=0;
    const calls=serve(async(path,init)=>{
      if(path.endsWith("/commit")){expect(init.body).toBeUndefined();commits++;stored={...value,state:"accepted",receipt:{request_id:scope.request_id,fingerprint:scope.fingerprint,receipt_id:"receipt",answer_sha256:value.draft_snapshot.answer_sha256,confirmed_sequence:8}};if(commits===1)throw new TypeError("lost Commit ACK");}
      return response(stored);
    });
    const current=session();await current.inspect(value.id);await expect(current.commit()).resolves.toMatchObject({state:"accepted",receipt:{receipt_id:"receipt"}});expect(current.intent?.state).toBe("accepted");expect(commits).toBe(1);
    const replay=await current.commit();expect(replay.receipt?.receipt_id).toBe("receipt");expect(calls.filter(call=>call.init.method==="POST").every(call=>call.path.endsWith("/input-answer-intents/intent/commit")&&call.init.body===undefined)).toBe(true);
  });

  it("returns a matching terminal rejection after lost commit ACK without submitting again",async()=>{
    const original=await intention();let stored=original;
    const calls=serve(async(path)=>{if(path.endsWith("/commit")){stored={...original,state:"rejected",rejection:{code:"forbidden",message:"Revoked before acceptance"},updated_at:2};throw new TypeError("lost Commit ACK");}return response(stored);});
    const current=session();await current.inspect(original.id);const result=await current.commit();expect(result.state).toBe("rejected");expect(result.rejection?.code).toBe("forbidden");expect(result.receipt).toBeNull();expect(current.intent).toEqual(result);expect(calls.filter(call=>call.init.method==="POST")).toHaveLength(1);
  });

  it("keeps an unresolved pending intent unconfirmed after commit transport loss",async()=>{
    const original=await intention();const calls=serve(async(path)=>{if(path.endsWith("/commit"))throw new TypeError("lost Commit ACK");return response(original);});
    const current=session();await current.inspect(original.id);await expect(current.commit()).rejects.toThrow("lost Commit ACK");expect(current.intent).toEqual(original);expect(calls.filter(call=>call.init.method==="POST")).toHaveLength(1);
  });

  it("retains original unconfirmed intent when its post-commit reconciliation GET fails",async()=>{
    const original=await intention();let committing=false;const calls=serve(async(path)=>{if(path.endsWith("/commit")){committing=true;throw new TypeError("lost Commit ACK");}if(committing)throw new TypeError("reconciliation unavailable");return response(original);});
    const current=session();await current.inspect(original.id);await expect(current.commit()).rejects.toThrow("lost Commit ACK");expect(current.intent).toEqual(original);expect(calls.filter(call=>call.init.method==="POST")).toHaveLength(1);
  });

  it.each(["foreign_project","other_id","changed_frozen_answer","invalid_receipt"] as const)("does not confirm a lost commit from %s reconciliation",async(kind)=>{
    const original=await intention(),changed=await intention("Changed frozen answer");let committing=false;
    const accepted:WorkflowInputAnswerIntent={...original,state:"accepted",receipt:{request_id:scope.request_id,fingerprint:scope.fingerprint,receipt_id:"receipt",answer_sha256:original.draft_snapshot.answer_sha256,confirmed_sequence:8}};
    if(kind==="foreign_project")accepted.project_id="other-project";
    if(kind==="other_id")accepted.id="other-intent";
    if(kind==="changed_frozen_answer"){accepted.submission=changed.submission;accepted.draft_snapshot=changed.draft_snapshot;accepted.receipt!.answer_sha256=changed.draft_snapshot.answer_sha256;}
    if(kind==="invalid_receipt")accepted.receipt!.confirmed_sequence=0;
    const calls=serve(async(path)=>{if(path.endsWith("/commit")){committing=true;throw new TypeError("lost Commit ACK");}return response(committing?accepted:original);});
    const current=session();await current.inspect(original.id);await expect(current.commit()).rejects.toThrow("lost Commit ACK");expect(current.intent).toEqual(original);expect(calls.filter(call=>call.init.method==="POST")).toHaveLength(1);
  });

  it("does not acknowledge a terminal reconciliation completed after context dispose and StrictMode reactivation",async()=>{
    const original=await intention();let committing=false;let release!:(value:Response)=>void;
    const accepted:WorkflowInputAnswerIntent={...original,state:"accepted",receipt:{request_id:scope.request_id,fingerprint:scope.fingerprint,receipt_id:"receipt",answer_sha256:original.draft_snapshot.answer_sha256,confirmed_sequence:8}};
    const calls=serve(async(path)=>{if(path.endsWith("/commit")){committing=true;throw new TypeError("lost Commit ACK");}if(committing)return new Promise<Response>(resolve=>{release=resolve;});return response(original);});
    const current=session();await current.inspect(original.id);const pending=current.commit();const failure=expect(pending).rejects.toThrow("lost Commit ACK");await vi.waitFor(()=>expect(release).toBeTypeOf("function"));current.dispose();current.activate();release(response(accepted));await failure;expect(current.intent).toEqual(original);expect(calls.filter(call=>call.init.method==="POST")).toHaveLength(1);
  });

  it("blocks multiple pending intentions before selecting one or creating another key",async()=>{
    const draft=await head("Draft",1),one=await intention(),two={...await intention(),id:"other"};const calls=serve(async()=>response({draft,snapshots:[],intents:[intentSummary(one),intentSummary(two)]}));
    const current=session();await expect(current.read()).rejects.toThrow("workflowInputMultiplePending");await expect(current.prepare(1,draft.answer)).rejects.toThrow("workflowInputMultiplePending");expect(calls).toHaveLength(2);expect(calls.every(call=>call.init.method==="GET")).toBe(true);
  });

  it.each(["project_id","run_id","request_id","fingerprint","owner"] as const)("rejects a snapshot with foreign %s without copying or writing it",async(field)=>{
    const original=await snapshot("Confirmed old snapshot"),bad={...original,[field]:"foreign"};const draft=await head("Current",2);
    const calls=serve(async(path)=>response(path.includes("/snapshots/")?bad:{draft,snapshots:[summary(original)],intents:[]}));
    const current=session();await current.read();await expect(current.snapshot(summary(original))).rejects.toThrow("workflowInputBinding");expect(calls.every(call=>call.init.method==="GET")).toBe(true);
  });

  it.each(["id","answer_sha256","revision","base_revision"] as const)("rejects a snapshot whose %s differs from the saved summary",async(field)=>{
    const original=await snapshot("Exact original"),bad={...original,[field]:typeof original[field]==="number"?4:"different"};serve(async()=>response(bad));await expect(session().snapshot(summary(original))).rejects.toThrow();
  });

  it("rejects same-revision draft rebinding to another snapshot or answer hash while preserving prior confirmation",async()=>{
    const original=await head("Original",1);let current=original;serve(async()=>response({draft:current,snapshots:[],intents:[]}));const reader=session();await reader.read();
    current={...original,snapshot_id:"changed-snapshot"};await expect(reader.read()).rejects.toThrow("workflowInputBinding");
    current=await head("Other text",1);await expect(reader.read()).rejects.toThrow("workflowInputBinding");
  });

  it.each(["accepted_without_receipt","pending_with_receipt","rejected_without_reason","accepted_zero_sequence","accepted_wrong_hash"] as const)("rejects semantic intention violation %s",async(kind)=>{
    const value=await intention();const receipt={request_id:scope.request_id,fingerprint:scope.fingerprint,receipt_id:"receipt",answer_sha256:value.draft_snapshot.answer_sha256,confirmed_sequence:1};
    if(kind==="accepted_without_receipt")value.state="accepted";
    if(kind==="pending_with_receipt")value.receipt=receipt;
    if(kind==="rejected_without_reason")value.state="rejected";
    if(kind==="accepted_zero_sequence"){value.state="accepted";value.receipt={...receipt,confirmed_sequence:0};}
    if(kind==="accepted_wrong_hash"){value.state="accepted";value.receipt={...receipt,answer_sha256:"b".repeat(64)};}
    serve(async()=>response(value));await expect(session().inspect(value.id)).rejects.toThrow("workflowInputBinding");
  });

  it("preserves an unknown original Prepare key while inspecting a different rejected historical intention",async()=>{
    const draft=await head("Current",1),history=await intention("Old rejection");history.id="history";history.state="rejected";history.rejection={code:"forbidden",message:"Historical"};const bodies:string[]=[];
    serve(async(path,init)=>{if(path.endsWith("/draft"))return response({draft,snapshots:[],intents:[intentSummary(history)]});if(init.method==="GET")return response(history);bodies.push(String(init.body));throw new TypeError("lost Prepare ACK");});
    const current=session();await expect(current.prepare(1,draft.answer)).rejects.toThrow();await current.inspect(history.id);expect(current.canContinueRejected).toBe(false);expect(()=>current.continueRejected()).toThrow();
    await expect(current.prepare(1,draft.answer)).rejects.toThrow();expect(bodies[1]).toBe(bodies[0]);
  });

  it("does not dispatch an old frozen Prepare retry after its lookup completes in a disposed context",async()=>{
    const draft=await head("Original",1);let posts=0,block=false;let release!:(value:Response)=>void;
    serve(async(path)=>{if(path.endsWith("/draft")){if(block)return new Promise<Response>(resolve=>{release=resolve;});return response({draft,snapshots:[],intents:[]});}posts++;throw new TypeError("lost Prepare ACK");});
    const reader=session();await expect(reader.prepare(1,draft.answer)).rejects.toThrow("lost Prepare ACK");block=true;
    const oldRetry=reader.prepare(1,draft.answer);reader.dispose();reader.activate();release(response({draft,snapshots:[],intents:[]}));
    await expect(oldRetry).rejects.toThrow("workflowInputDraftUnavailable");expect(posts).toBe(1);expect(reader.uncertainPrepare).toBe(true);
  });

  it("fences old reads and snapshots across StrictMode cleanup/setup and context disposal",async()=>{
    const draft=await head("Current",1),copy=await snapshot("Saved",1);let release!:(value:Response)=>void;let delayed=true;
    serve(async(path)=>{if(delayed){delayed=false;return new Promise<Response>(resolve=>{release=resolve;});}return response(path.includes("/snapshots/")?copy:{draft,snapshots:[],intents:[]});});
    const reader=session();reader.activate();const old=reader.read();reader.dispose();reader.activate();expect((await reader.read())?.draft.answer).toBe("Current");release(response({draft,snapshots:[],intents:[]}));expect(await old).toBeNull();
    delayed=true;const stale=reader.snapshot(summary(copy));reader.dispose();reader.activate();release(response(copy));await expect(stale).rejects.toThrow("workflowInputDraftUnavailable");
  });
});
