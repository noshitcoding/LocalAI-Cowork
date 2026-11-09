import {afterEach,describe,expect,it,vi} from "vitest";
import {Client,decode,defaultRunBudgets,type CreateRun,type Order,type RunRequest} from "@cowork/client";
import {PersistentRequestLatch,type RequestJournalStorage} from "./requestJournal";
import {prepareRegenerationReview,singleStartJournalKey,type RegenerationIntent} from "./regenerationReview";

const client=new Client("PRIVATE-TRANSPORT-ONLY","http://127.0.0.1:43825/api/v2");
const parent={id:"parent",project_id:"project"};
const child:Order={id:"child",project_id:"project",draft:"original prompt",revision:1,title:"New answer",created_at:1};
const intent:RegenerationIntent={branch:{title:"New answer",through_message_id:"answer",idempotency_key:"branch-key"},template:{order_id:"parent",base_revision:8,prompt:"original prompt",provider_id:"provider",provider_revision:2,idempotency_key:"start-key",access_mode:"confirm_mutations",budgets:{...defaultRunBudgets,max_steps:5,max_seconds:120}}};
function memory(){const values=new Map<string,string>();const storage:RequestJournalStorage={getItem:k=>values.get(k)??null,setItem:(k,v)=>{values.set(k,v);},removeItem:k=>{values.delete(k);}};return {values,storage};}
function start(storage:RequestJournalStorage){return new PersistentRequestLatch<RunRequest>(singleStartJournalKey(client,child.project_id,child.id),v=>decode("RunRequest",v),storage);}
afterEach(()=>vi.unstubAllGlobals());
describe("regeneration hand-off into the existing confirmed plan review",()=>{
  it("branches without dispatch and persists the exact original request for a reloaded child",async()=>{
    const {values,storage}=memory(),fetch=vi.fn().mockResolvedValue(new Response(JSON.stringify(child)));vi.stubGlobal("fetch",fetch);
    const original=structuredClone(intent);expect(await prepareRegenerationReview(client,parent,original,storage)).toEqual(child);
    original.template.prompt="later edit";
    expect(start(storage).prepare(()=>{throw new Error("must recover");})).toEqual({...intent.template,order_id:child.id,base_revision:1});
    expect(fetch).toHaveBeenCalledTimes(1);expect(String(fetch.mock.calls[0][0])).toContain("/orders/parent/regenerate");
    expect(JSON.parse(fetch.mock.calls[0][1].body)).toEqual(intent.branch);
    expect([...values.values()].join("")).not.toContain("PRIVATE-TRANSPORT-ONLY");
  });
  it("recovers a lost branch ACK and repeats only that branch intent",async()=>{
    const {storage}=memory(),fetch=vi.fn().mockRejectedValueOnce(new TypeError("lost branch ACK")).mockResolvedValue(new Response(JSON.stringify(child)));vi.stubGlobal("fetch",fetch);
    const source=new PersistentRequestLatch<RegenerationIntent>("parent-intent",v=>v as RegenerationIntent,storage);source.prepare(()=>intent);
    await expect(prepareRegenerationReview(client,parent,source.prepare(()=>intent),storage)).rejects.toThrow("lost branch ACK");expect(source.pending).toBe(true);expect(start(storage).pending).toBe(false);
    const recovered=new PersistentRequestLatch<RegenerationIntent>("parent-intent",v=>v as RegenerationIntent,storage);
    await prepareRegenerationReview(client,parent,recovered.prepare(()=>{throw new Error("must recover");}),storage);recovered.confirm();
    expect(source.pending).toBe(true);expect(start(storage).pending).toBe(true);
    expect(fetch.mock.calls.map(c=>JSON.parse(c[1].body))).toEqual([intent.branch,intent.branch]);
  });
  it("keeps the parent recoverable when durable child storage fails",async()=>{
    const {storage}=memory(),source=new PersistentRequestLatch<RegenerationIntent>("parent-intent",v=>v as RegenerationIntent,storage);source.prepare(()=>intent);
    vi.stubGlobal("fetch",vi.fn().mockImplementation(()=>Promise.resolve(new Response(JSON.stringify(child)))));
    const broken={...storage,setItem:()=>{throw new Error("storage full");}};
    await expect(prepareRegenerationReview(client,parent,intent,broken)).rejects.toThrow("storage full");
    expect(source.pending).toBe(true);expect(start(storage).pending).toBe(false);
    await prepareRegenerationReview(client,parent,intent,storage);expect(start(storage).pending).toBe(true);
  });
  it("recovers after durable hand-off before parent cleanup without replacing the child intent",async()=>{
    const {storage,values}=memory();vi.stubGlobal("fetch",vi.fn().mockImplementation(()=>Promise.resolve(new Response(JSON.stringify({...child,revision:4,draft:"later draft"})))));
    await prepareRegenerationReview(client,parent,intent,storage);const bytes=[...values.values()];
    await prepareRegenerationReview(client,parent,structuredClone(intent),storage);expect([...values.values()]).toEqual(bytes);
    expect((start(storage).prepare(()=>intent.template) as CreateRun).base_revision).toBe(1);
    const conflicting=structuredClone(intent);conflicting.template.prompt="different";
    await expect(prepareRegenerationReview(client,parent,conflicting,storage)).rejects.toThrow("execution_plan_original_mismatch");expect([...values.values()]).toEqual(bytes);
  });
  it("does not overwrite a malformed child journal",async()=>{
    const {storage,values}=memory();values.set(singleStartJournalKey(client,child.project_id,child.id),"broken");vi.stubGlobal("fetch",vi.fn().mockResolvedValue(new Response(JSON.stringify(child))));
    await expect(prepareRegenerationReview(client,parent,intent,storage)).rejects.toThrow("request_journal_invalid");expect([...values.values()]).toEqual(["broken"]);
  });
  it.each([{...child,project_id:"foreign"},{...child,id:parent.id}])("rejects a mismatched branch receipt before saving a start",async receipt=>{
    const {values,storage}=memory();vi.stubGlobal("fetch",vi.fn().mockResolvedValue(new Response(JSON.stringify(receipt))));
    await expect(prepareRegenerationReview(client,parent,intent,storage)).rejects.toThrow("workspace_receipt_mismatch");expect(values.size).toBe(0);
  });
});
