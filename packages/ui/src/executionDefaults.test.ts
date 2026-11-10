import {afterEach,describe,expect,it,vi} from "vitest";
import {defaultExecutionTarget,executionDefaultsScope,executionDefaultsStorageKey,readDefaultExecutionMode,saveDefaultExecutionMode} from "./executionDefaults";

function storage(){const values=new Map<string,string>();return {getItem:(key:string)=>values.get(key)??null,setItem:(key:string,value:string)=>{values.set(key,value);}};}
afterEach(()=>vi.unstubAllGlobals());
describe("execution defaults",()=>{
  it("persists the chosen mode per workspace and restores it for new requests",()=>{
    const saved=storage(),client={base:"https://cowork.example/team-a"},scope=executionDefaultsScope(client);
    vi.stubGlobal("localStorage",saved);
    expect(defaultExecutionTarget(client)).toEqual({kind:"broker_controller"});
    saveDefaultExecutionMode(saved,scope,"document_engine");
    expect(defaultExecutionTarget(client)).toEqual({kind:"document_engine"});
    expect(defaultExecutionTarget({base:"https://cowork.example/team-b"})).toEqual({kind:"broker_controller"});
    expect(defaultExecutionTarget({base:""})).toEqual({kind:"broker_controller"});
    expect(executionDefaultsScope({base:client.base+"/"})).toBe(scope);
    saveDefaultExecutionMode(saved,scope,"ask");
    expect(defaultExecutionTarget(client)).toBeNull();
  });
  it("falls back to file tools for missing, malformed or unsupported settings",()=>{
    const saved=storage();
    for(const value of ["{broken",'"managed_runner"',"null",'{"kind":"document_engine"}']){
      saved.setItem(executionDefaultsStorageKey("local"),value);
      expect(readDefaultExecutionMode(saved,"local")).toBe("broker_controller");
    }
    expect(readDefaultExecutionMode({getItem:()=>{throw new Error("storage unavailable");}},"local")).toBe("broker_controller");
  });
  it("surfaces a failed save and preserves the previously persisted default",()=>{
    const saved=storage();saveDefaultExecutionMode(saved,"local","ask");
    expect(()=>saveDefaultExecutionMode({...saved,setItem:()=>{throw new Error("quota");}},"local","document_engine")).toThrow("quota");
    expect(readDefaultExecutionMode(saved,"local")).toBe("ask");
  });
});
