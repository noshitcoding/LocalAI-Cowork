import {afterEach,beforeEach,describe,expect,it,vi} from "vitest";
import {Client} from "@cowork/client";
import {chatDeletionLatch,submitChatDeletion,validateChatDeletion} from "./chatDeletionApi";
const selected={id:"chat-a",title:"Retained history",revision:7};
const json=(value:unknown,status=200)=>new Response(JSON.stringify(value),{status,headers:{"Content-Type":"application/json"}});
beforeEach(()=>{const values=new Map<string,string>();vi.stubGlobal("localStorage",{getItem:(key:string)=>values.get(key)??null,setItem:(key:string,value:string)=>values.set(key,value),removeItem:(key:string)=>values.delete(key)});});
afterEach(()=>vi.unstubAllGlobals());
describe("durable chat deletion",()=>{
  it("reconciles the original deletion after a lost commit acknowledgement, reload and expired session",async()=>{
    const requests:unknown[]=[];let attempts=0;
    vi.stubGlobal("fetch",vi.fn(async(_url:string,options:RequestInit)=>{requests.push(JSON.parse(String(options.body)));if(attempts++===0)throw new Error("lost_ack");if(attempts===2)return json({code:"session_expired",message:"Sign in",retryable:false,correlation_id:"fixture"},401);return json({id:selected.id,revision:8,deleted:true});}));
    const client=new Client("expired","https://one.test"),first=chatDeletionLatch(client);
    await expect(submitChatDeletion(client,first,selected)).rejects.toThrow();expect(first.pending).toBe(true);
    const reloaded=chatDeletionLatch(client);await expect(submitChatDeletion(client,reloaded)).rejects.toThrow();expect(reloaded.pending).toBe(true);
    const authenticated=new Client("new-session","https://one.test"),reconnected=chatDeletionLatch(authenticated);
    expect(await submitChatDeletion(authenticated,reconnected)).toEqual({id:selected.id,revision:8,deleted:true});
    expect(requests).toHaveLength(3);expect(requests[1]).toEqual(requests[0]);expect(requests[2]).toEqual(requests[0]);expect(reconnected.pending).toBe(false);
  });
  it("retains a mismatched acknowledgement and does not substitute another selected chat",async()=>{
    const sent:unknown[]=[];vi.stubGlobal("fetch",vi.fn(async(_url:string,options:RequestInit)=>{sent.push(JSON.parse(String(options.body)));return json({id:"different-chat",revision:8,deleted:true});}));
    const client=new Client("owner","https://two.test"),latch=chatDeletionLatch(client);
    await expect(submitChatDeletion(client,latch,selected)).rejects.toThrow("receipt_mismatch");
    await expect(submitChatDeletion(client,latch,{id:"other",title:"Other",revision:88})).rejects.toThrow("receipt_mismatch");
    expect(sent[0]).toEqual(sent[1]);expect(latch.pending).toBe(true);
  });
  it("separates endpoints and rejects damaged request bodies before sending",async()=>{
    const fetch=vi.fn();vi.stubGlobal("fetch",fetch);
    expect(()=>validateChatDeletion({id:"a",title:"a",body:{expected_revision:1,idempotency_key:"x",extra:"injected"}})).toThrow();
    const one=chatDeletionLatch(new Client("owner","https://one.test"));one.prepare(()=>({id:selected.id,title:selected.title,body:{expected_revision:7,idempotency_key:"original"}}));
    expect(chatDeletionLatch(new Client("other-token","https://one.test")).pending).toBe(true);
    expect(chatDeletionLatch(new Client("owner","https://other.test")).pending).toBe(false);expect(fetch).not.toHaveBeenCalled();
  });
});
