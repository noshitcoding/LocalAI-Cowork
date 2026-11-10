import {afterEach,expect,it,vi} from "vitest";
import {Client,type ReconcileOwnerManagedProcess} from "@cowork/client";
import fixture from "../../client/src/fixtures/owner-managed-reconciliation.json";
import {PersistentRequestLatch,type RequestJournalStorage} from "./requestJournal";
import {decodeManagedProcessIntent,submitManagedProcessReconciliation} from "./managedProcessReconciliation";

const intent=fixture.intent as ReconcileOwnerManagedProcess;
const response=fixture.response;
function storage():RequestJournalStorage {
  const values=new Map<string,string>();return {getItem:key=>values.get(key)??null,setItem:(key,value)=>{values.set(key,value);},removeItem:key=>{values.delete(key);}};
}
const submit=(client:Client,journal:PersistentRequestLatch<ReconcileOwnerManagedProcess>,build:()=>ReconcileOwnerManagedProcess)=>submitManagedProcessReconciliation(client,response.run.spec.order_id,response.run.id,response.process_attempt.id,journal,build);
afterEach(()=>{vi.unstubAllGlobals();vi.restoreAllMocks();});

it("retains the original exact cancellation intent across lost response and reload",async()=>{
  const retained=storage();let journal=new PersistentRequestLatch("original-scope",decodeManagedProcessIntent,retained);
  const fetch=vi.fn().mockRejectedValueOnce(new TypeError("lost response")).mockResolvedValueOnce(new Response(JSON.stringify(response)));
  vi.stubGlobal("fetch",fetch);const client=new Client("private-token");
  await expect(submit(client,journal,()=>intent)).rejects.toThrow("lost response");expect(journal.pending).toBe(true);
  expect(fetch).toHaveBeenCalledTimes(1);expect(retained.getItem("original-scope")).not.toContain("private-token");
  journal=new PersistentRequestLatch("original-scope",decodeManagedProcessIntent,retained);
  await submit(client,journal,()=>({...intent,finish_cancellation:!intent.finish_cancellation,expected_run_revision:99}));
  expect(fetch).toHaveBeenCalledTimes(2);
  expect(JSON.parse(fetch.mock.calls[1][1].body)).toEqual(intent);expect(journal.pending).toBe(false);expect(retained.getItem("original-scope")).toBeNull();
});
it("keeps a malformed saved intent blocked before any request",async()=>{
  const retained=storage();retained.setItem("scope",JSON.stringify({format:1,scope:"scope",request:{...intent,result:{}}}));
  const journal=new PersistentRequestLatch("scope",decodeManagedProcessIntent,retained);const fetch=vi.fn();vi.stubGlobal("fetch",fetch);
  expect(journal.invalid).toBe(true);await expect(submit(new Client("owner"),journal,()=>intent)).rejects.toThrow("request_journal_invalid");
  expect(fetch).not.toHaveBeenCalled();
});
it("does not dispatch when the original intent could not be durably saved",async()=>{
  const retained=storage();retained.setItem=()=>{throw new Error("storage unavailable");};
  const journal=new PersistentRequestLatch("scope",decodeManagedProcessIntent,retained);const fetch=vi.fn();vi.stubGlobal("fetch",fetch);
  await expect(submit(new Client("owner"),journal,()=>intent)).rejects.toThrow("storage unavailable");expect(fetch).not.toHaveBeenCalled();
});
