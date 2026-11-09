import {afterEach,describe,expect,it,vi} from "vitest";
import {createHash,webcrypto} from "node:crypto";
import {Client,decode,RequestError} from "./index";
const fp="a".repeat(64), answer='  ö😀\r\n"quoted"\\path\n  ';
const sha=(value:string)=>createHash("sha256").update(value,"utf8").digest("hex");
const save={fingerprint:fp,base_revision:0,idempotency_key:"save",answer};
const draft={run_id:"run",project_id:"project",request_id:"request",fingerprint:fp,owner:"single_owner_preview",revision:1,answer,answer_sha256:sha(answer),snapshot_id:"snapshot",updated_at:10};
const snapshot={id:"snapshot",run_id:"run",project_id:"project",request_id:"request",fingerprint:fp,owner:"single_owner_preview",base_revision:0,revision:1,answer,answer_sha256:sha(answer),conflict:false,created_at:10};
const prepare={fingerprint:fp,base_draft_revision:1,idempotency_key:"prepare"};
const failure={code:"revision_conflict",message:"Review",retryable:false,correlation_id:"corr"};
describe("workflow human answer durability transport",()=>{
 afterEach(()=>vi.unstubAllGlobals());
 it("sends exact UTF8 and verifies a frozen save even when caller later edits",async()=>{
  vi.stubGlobal("crypto",webcrypto);let finish!:(value:Response)=>void;
  const fetch=vi.fn().mockReturnValue(new Promise<Response>(r=>{finish=r;}));vi.stubGlobal("fetch",fetch);
  const mutable={...save},pending=new Client("owner").saveWorkflowInputDraft("run","request",mutable);mutable.answer="changed";
  finish(new Response(JSON.stringify({outcome:"saved",draft,snapshot}),{status:200}));
  await expect(pending).resolves.toMatchObject({outcome:"saved"});expect(JSON.parse(fetch.mock.calls[0][1].body)).toEqual(save);
 });
 it("decodes committed CAS conflicts and never treats an ordinary409 as a saved copy",async()=>{
  vi.stubGlobal("crypto",webcrypto);const conflict={outcome:"conflict",draft,snapshot:{...snapshot,id:"conflict",conflict:true}};
  const fetch=vi.fn().mockResolvedValueOnce(new Response(JSON.stringify(conflict),{status:409})).mockResolvedValueOnce(new Response(JSON.stringify(failure),{status:409}));vi.stubGlobal("fetch",fetch);
  await expect(new Client("owner").saveWorkflowInputDraft("run","request",save)).resolves.toEqual(conflict);
  await expect(new Client("owner").saveWorkflowInputDraft("run","request",save)).rejects.toBeInstanceOf(RequestError);
 });
 it("rejects receipt substitution, wrong scope and wrong conflict headrevision",async()=>{
  vi.stubGlobal("crypto",webcrypto);
  for(const forged of [{outcome:"saved",draft,snapshot:{...snapshot,answer:"other"}},{outcome:"saved",draft:{...draft,request_id:"other"},snapshot},{outcome:"conflict",draft,snapshot:{...snapshot,revision:7,conflict:true}}]){
   vi.stubGlobal("fetch",vi.fn().mockResolvedValue(new Response(JSON.stringify(forged),{status:forged.outcome==="conflict"?409:200})));
   await expect(new Client("owner").saveWorkflowInputDraft("run","request",save)).rejects.toThrow("workflow_input_draft_integrity");
  }
 });
 it("keeps lostACK unknown and retries solely the stored intent ID without any answerbody",async()=>{
  const fetch=vi.fn().mockRejectedValue(new TypeError("lostACK"));vi.stubGlobal("fetch",fetch);const client=new Client("owner");
  await expect(client.prepareWorkflowInputAnswer("run","request",prepare)).rejects.toThrow("lostACK");expect(fetch).toHaveBeenCalledTimes(1);
  await expect(client.commitWorkflowInputAnswerIntent("run","intent/old")).rejects.toThrow("lostACK");
  await expect(client.commitWorkflowInputAnswerIntent("run","intent/old")).rejects.toThrow("lostACK");
  for(const call of fetch.mock.calls.slice(1)){expect(call[0]).toBe("/api/v2/runs/run/workflow/input-answer-intents/intent%2Fold/commit");expect(call[1].body).toBeUndefined();}
 });
 it("strictly validates commands and reads the scoped immutable conflict snapshot",async()=>{
  for(const field of ["owner","run_id","grant","receipt","answer"]){expect(()=>decode("PrepareWorkflowInputAnswer",{...prepare,[field]:"forged"})).toThrow("contract_mismatch");}
  const fetch=vi.fn().mockResolvedValue(new Response(JSON.stringify(snapshot),{status:200}));vi.stubGlobal("fetch",fetch);
  await new Client("owner").workflowInputDraftSnapshot("r/u","q/u","s/u");expect(fetch.mock.calls[0][0]).toBe("/api/v2/runs/r%2Fu/workflow/inputs/q%2Fu/draft/snapshots/s%2Fu");
 });
});
