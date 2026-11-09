import {beforeEach,expect,it,vi} from "vitest";
import {offlineWorkspaceScope,rememberOfflineWorkspace,type OfflineWorkspace} from "./offlineProjects";

const saved=vi.hoisted(()=>new Map<string,OfflineWorkspace>());
// Only the metadata write is replaced. Endpoint and identity validation remain
// the actual product path, before any IndexedDB request is made.
vi.mock("./teamReplicaDatabase",()=>({replicaDatabase:async()=>({
 close(){},transaction(){
  const tx={oncomplete:null as null|(()=>void),objectStore:()=>({put(row:OfflineWorkspace){
   saved.set(row.scope,row);queueMicrotask(()=>tx.oncomplete?.());
  }})};return tx;
 }
})}));
const identity="72baa2a5-e13c-40d2-af6b-55710755129e";
const team="e7b00c13-df97-42f8-8f94-aa55d90ec34c";
const workspace=(endpoint:string)=>({endpoint,identity,team,name:"Synthetic workspace",can_create:true});
beforeEach(()=>saved.clear());

it("caches the current web origin without changing its relative endpoint or scope",async()=>{
 await expect(rememberOfflineWorkspace(workspace(""))).resolves.toBeUndefined();
 expect(saved.get(offlineWorkspaceScope("",identity,team))).toMatchObject(workspace(""));
});

it("keeps relative, explicit remote and separate identity scopes distinct",async()=>{
 for(const endpoint of ["","https://cowork.example","https://another.example"])
  await rememberOfflineWorkspace(workspace(endpoint));
 await rememberOfflineWorkspace({...workspace(""),identity:team});
 expect(saved.size).toBe(4);
 expect(new Set([...saved.values()].map(row=>row.scope)).size).toBe(4);
});

it("rejects invalid server authorities and identity IDs before caching anything",async()=>{
 for(const endpoint of ["/other","//other.example"," ","file:///private", 
  "https://user:secret@cowork.example","https://cowork.example/?key=x","https://cowork.example/#key"])
  await expect(rememberOfflineWorkspace(workspace(endpoint))).rejects.toThrow("offline_project_scope");
 for(const id of ["", "not-an-identity", "00000000-0000-0000-0000-000000000000"])
  await expect(rememberOfflineWorkspace({...workspace(""),identity:id})).rejects.toThrow("offline_project_scope");
 expect(saved.size).toBe(0);
});
