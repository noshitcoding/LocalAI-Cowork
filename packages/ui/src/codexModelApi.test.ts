import {afterEach,beforeEach,describe,expect,it,vi} from "vitest";
import {Client} from "@cowork/client";
import {materializeCodexModelProfile,pendingCodexModelProfile,recoverCodexModelProfile,discardCodexModelProfile} from "./codexModelApi";
const account="11111111-1111-4111-8111-111111111111",profileId="22222222-2222-4222-8222-222222222222";
const selection={kind:"codex" as const,account_id:account,model:"codex-fixture",reasoning_effort:null};
const record={id:account,revision:3,created_at:1,updated_at:1,value:{name:"My account",priority:0}};
const model={id:"codex-fixture",name:"Fixture",default_reasoning_effort:"medium",reasoning_efforts:["low","medium","high"]};
const profile=(revision=3)=>({id:profileId,revision:1,name:"Codex fixture",model:model.id,kind:"codex",endpoint:`codex://${account}/${revision}?reasoning_effort=medium`,context_tokens:32768,credential_required:false,credential_ready:true,generation:{}});
const json=(value:unknown,status=200)=>new Response(JSON.stringify(value),{status,headers:{"Content-Type":"application/json"}});
beforeEach(()=>{const values=new Map<string,string>();vi.stubGlobal("localStorage",{getItem:(key:string)=>values.get(key)??null,setItem:(key:string,value:string)=>values.set(key,value),removeItem:(key:string)=>values.delete(key)});});
afterEach(()=>vi.unstubAllGlobals());
describe("isolated Codex profile materialization",()=>{
  it("discovers actual model effort and freezes account revision in an ordinary model profile",async()=>{
    const requests:{url:string;headers:Record<string,string>;body?:unknown}[]=[];
    vi.stubGlobal("fetch",vi.fn(async(url:string,options:RequestInit)=>{requests.push({url,headers:options.headers as Record<string,string>,body:options.body?JSON.parse(String(options.body)):undefined});return json(url.endsWith("/accounts")?[record]:url.endsWith("/models")?[model]:profile());}));
    const saved=await materializeCodexModelProfile({client:new Client("owner-session","https://cowork.test")},selection);
    expect(saved.id).toBe(profileId);expect(requests).toHaveLength(3);expect(requests[2].body).toMatchObject({expected_account_revision:3,model:model.id,reasoning_effort:"medium",context_tokens:32768});expect(requests.every(r=>r.headers.Authorization==="Bearer owner-session"&&r.url.startsWith("https://cowork.test/api/v2/codex/"))).toBe(true);
  });
  it("retains exact profile creation across reload and never fetches a newer account on recovery",async()=>{
    const requests:unknown[]=[];let calls=0;
    vi.stubGlobal("fetch",vi.fn(async(url:string,options:RequestInit)=>{if(url.endsWith("/accounts"))return json([record]);if(url.endsWith("/models"))return json([model]);requests.push(JSON.parse(String(options.body)));if(calls++===0)throw new Error("lost_ack");return json(profile());}));
    const context={client:new Client("owner","https://one.test")};
    await expect(materializeCodexModelProfile(context,selection)).rejects.toThrow("lost_ack");expect(pendingCodexModelProfile(context,account)).toBe(true);
    await expect(materializeCodexModelProfile(context,{...selection,model:"different"})).rejects.toThrow("reconcile_original");
    const saved=await recoverCodexModelProfile({client:new Client("owner-reconnected","https://one.test")},account);
    expect(saved.id).toBe(profileId);expect(requests).toHaveLength(2);expect(requests[0]).toEqual(requests[1]);expect(pendingCodexModelProfile(context,account)).toBe(false);
  });
  it("uses only the explicit actor session and rejects unavailable model capabilities before mutation",async()=>{
    const calls:{url:string;auth:string}[]=[];
    vi.stubGlobal("fetch",vi.fn(async(url:string,options:RequestInit)=>{calls.push({url,auth:(options.headers as Record<string,string>).Authorization});return json(url.endsWith("/accounts")?[record]:[model]);}));
    const context={client:new Client("never-use-owner","https://cowork.test"),session:"actor-session",actorId:"actor-a"};
    await expect(materializeCodexModelProfile(context,{...selection,reasoning_effort:"unsupported"})).rejects.toThrow("reasoning_effort_unavailable");
    expect(calls).toHaveLength(2);expect(calls.every(c=>c.url.includes("/api/v1/codex/")&&c.auth==="Bearer actor-session")).toBe(true);
    await expect(materializeCodexModelProfile({...context,session:""},selection)).rejects.toThrow("session_required");expect(calls).toHaveLength(2);
  });
  it("keeps a committed request through an expired-session retry and reconciles after sign-in",async()=>{
    const requests:unknown[]=[],auth:string[]=[];let calls=0;
    vi.stubGlobal("fetch",vi.fn(async(url:string,options:RequestInit)=>{
      if(url.endsWith("/accounts"))return json([record]);if(url.endsWith("/models"))return json([model]);
      requests.push(JSON.parse(String(options.body)));auth.push((options.headers as Record<string,string>).Authorization);
      if(calls++===0)throw new Error("committed_but_lost_ack");
      if(calls===2)return json({code:"session_expired"},401);
      return json(profile());
    }));
    const context={client:new Client("expired","https://auth-recovery.test")};
    await expect(materializeCodexModelProfile(context,selection)).rejects.toThrow("committed_but_lost_ack");
    await expect(recoverCodexModelProfile(context,account)).rejects.toThrow("session_expired");
    expect(pendingCodexModelProfile(context,account)).toBe(true);
    await recoverCodexModelProfile({client:new Client("signed-in-again","https://auth-recovery.test")},account);
    expect(requests).toHaveLength(3);expect(requests[1]).toEqual(requests[0]);expect(requests[2]).toEqual(requests[0]);
    expect(auth).toEqual(["Bearer expired","Bearer expired","Bearer signed-in-again"]);
    expect(pendingCodexModelProfile(context,account)).toBe(false);
  });
  it("rejects a substituted account or reasoning receipt and retains the original for explicit reconciliation",async()=>{
    vi.stubGlobal("fetch",vi.fn(async(url:string)=>json(url.endsWith("/accounts")?[record]:url.endsWith("/models")?[model]:{...profile(),endpoint:`codex://${account}/3?reasoning_effort=high`})));
    const context={client:new Client("owner","https://two.test")};
    await expect(materializeCodexModelProfile(context,selection)).rejects.toThrow("receipt_mismatch");expect(pendingCodexModelProfile(context,account)).toBe(true);
    discardCodexModelProfile(context,account);expect(pendingCodexModelProfile(context,account)).toBe(false);
  });
});
