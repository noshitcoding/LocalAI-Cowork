import {afterEach,beforeEach,describe,expect,it,vi} from "vitest";
import {allowedUrl,grantedTab,validateCommand} from "../extensions/browser/src/policy";
describe("extension tab boundary",()=>{
  it("accepts only HTTP(S) URLs without embedded credentials",()=>{
    for(const value of ["file:///tmp/private","chrome://settings","about:config","https://user:password@example.com",null])expect(allowedUrl(value)).toBe(false);
    expect(allowedUrl("https://example.com/path")).toBe(true);
  });
  it("requires an explicit grant even when website permissions exist",()=>{
    const grants=new Set([1]);expect(grantedTab({id:2,url:"https://example.com"},grants)).toBe(false);
    expect(grantedTab({id:1,url:"https://example.com",incognito:true},grants)).toBe(false);
    expect(grantedTab({id:1,url:"https://example.com"},grants)).toBe(true);grants.clear();expect(grantedTab({id:1,url:"https://example.com"},grants)).toBe(false);
  });
  it("rejects arbitrary script execution and oversized field values",()=>{
    expect(()=>validateCommand({operation:"eval",text:"1"})).toThrow();
    expect(()=>validateCommand({operation:"fill",text:"a".repeat(32001)})).toThrow();
    expect(()=>validateCommand({operation:"navigate",url:"https://example.com"})).not.toThrow();
  });
});
describe("actual background dispatch",()=>{
  let handler:(message:any,sender:any)=>Promise<any>,nativeMessage:(message:any)=>void;
  const sent:any[]=[];const execute=vi.fn();const tabs=[{id:1,url:"https://example.com/",title:"Shared",active:true,windowId:1},{id:2,url:"https://private.example/",title:"Private",active:false,windowId:1}];
  beforeEach(async()=>{
    vi.resetModules();sent.length=0;execute.mockReset();
    const event=()=>({addListener:vi.fn()});
    vi.stubGlobal("browser",{
      runtime:{id:"fixture-extension",getManifest:()=>({browser_specific_settings:{gecko:{id:"fixture"}}}),onMessage:{addListener:(h:any)=>handler=h},connectNative:()=>({postMessage:(v:any)=>sent.push(v),disconnect:vi.fn(),onDisconnect:event(),onMessage:{addListener:(h:any)=>nativeMessage=h}})},
      tabs:{query:async()=>tabs,get:async(id:number)=>tabs.find(t=>t.id===id),onUpdated:event(),onRemoved:event(),onActivated:event()},
      permissions:{onRemoved:event()},action:{setBadgeText:async()=>undefined,setBadgeBackgroundColor:async()=>undefined},scripting:{executeScript:execute},
    });
    await import("../extensions/browser/src/background");
  });
  afterEach(()=>vi.unstubAllGlobals());
  const settle=()=>new Promise(resolve=>setTimeout(resolve,5));
  it("publishes only selected tabs and refuses a command for another tab",async()=>{
    await handler({type:"connect",instance_id:"fixture",tabs:[1]},{id:"fixture-extension"});
    const connect=sent.find(v=>v.type==="connect");expect(connect.tabs.map((t:any)=>t.id)).toEqual(["1"]);expect(JSON.stringify(connect)).not.toContain("private.example");
    nativeMessage({type:"connected",version:1,connection_id:"connection"});await settle();
    nativeMessage({type:"command",version:1,request_id:"request",epoch:"epoch",command:{operation:"dom",tab_id:"2",document_id:"unknown"}});await settle();
    expect(sent.find(v=>v.type==="result")?.ok).toBe(false);expect(execute).not.toHaveBeenCalled();
    await handler({type:"disconnect"},{id:"fixture-extension"});expect((await handler({type:"status"},{id:"fixture-extension"})).grants).toEqual([]);
  });
  it("rejects messages from content scripts before connecting native",async()=>{
    await expect(handler({type:"connect",tabs:[1]},{id:"fixture-extension",tab:{id:1}})).rejects.toThrow("browser_popup_only");expect(sent).toEqual([]);
  });
  it("ignores a late native connection event after explicit revocation",async()=>{
    await handler({type:"connect",instance_id:"fixture",tabs:[1]},{id:"fixture-extension"});
    nativeMessage({type:"connected",version:1,connection_id:"old"});await settle();
    await handler({type:"disconnect"},{id:"fixture-extension"});
    nativeMessage({type:"connected",version:1,connection_id:"old"});await settle();
    const state=await handler({type:"status"},{id:"fixture-extension"});expect(state.connected).toBe(false);expect(state.grants).toEqual([]);
  });
});
