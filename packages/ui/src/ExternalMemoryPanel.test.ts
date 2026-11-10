import {describe,expect,it} from "vitest";
import {validateExternalMemoryWrite} from "./ExternalMemoryPanel";
import {PersistentRequestLatch} from "./requestJournal";
const request=()=>({id:"10000000-0000-4000-8000-000000000001",method:"PUT" as const,secret_required:true,body:{request_id:"10000000-0000-4000-8000-000000000002",expected_revision:0,value:{name:"Memory",endpoint:"https://memory.example.test/retrieve",method:"POST",request_body:{query:"language"},items_pointer:"/results",content_field:"text",id_field:"id",auth_scheme:"Bearer"},remove_key:false}});
describe("external memory durable metadata",()=>{
 it("never journals credentials pasted into metadata or endpoint fields",()=>{
  const rows=new Map<string,string>(),storage={getItem:(key:string)=>rows.get(key)??null,setItem:(key:string,v:string)=>{rows.set(key,v);},removeItem:(key:string)=>{rows.delete(key);}};
  for(const endpoint of ["https://user:private-password@memory.example.test/retrieve","https://memory.example.test/retrieve?api_key=private-key","https://memory.example.test/retrieve?accessToken=private-key"]){const value=request();value.body.value.endpoint=endpoint;const latch=new PersistentRequestLatch("test",validateExternalMemoryWrite,storage);expect(()=>latch.prepare(()=>value)).toThrow("external_memory_original_invalid");expect(rows.size).toBe(0);}
  const value=request();value.body.value.request_body={query:"language",nested:{apiKey:"private-key"}} as never;expect(()=>new PersistentRequestLatch("test",validateExternalMemoryWrite,storage).prepare(()=>value)).toThrow("external_memory_original_invalid");expect(rows.size).toBe(0);
 });
 it("restores the exact original metadata without retaining the transported secret",()=>{
  const rows=new Map<string,string>(),storage={getItem:(key:string)=>rows.get(key)??null,setItem:(key:string,v:string)=>{rows.set(key,v);},removeItem:(key:string)=>{rows.delete(key);}};
  const original=request();new PersistentRequestLatch("test",validateExternalMemoryWrite,storage).prepare(()=>original);expect(rows.get("test")).not.toContain("api_key");const restored=new PersistentRequestLatch("test",validateExternalMemoryWrite,storage);expect(restored.prepare(()=>{throw new Error("never mint replacement");})).toEqual(original);restored.confirm();expect(rows.size).toBe(0);
 });
});
