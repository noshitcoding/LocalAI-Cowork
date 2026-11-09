import {afterEach,expect,it,vi} from "vitest";
import {Client,type OwnerManagedResultManifest} from "./index";
import fixture from "./fixtures/owner-managed-result.json";

const valid=fixture.valid as OwnerManagedResultManifest;
const fileHash=valid.manifest.files[0].sha256;
const json=(value:unknown)=>new Response(JSON.stringify(value),{headers:{"content-type":"application/json"}});
const chunk=(body="abc",length="3")=>new Response(body,{headers:{"content-type":"application/octet-stream","content-length":length,"x-cowork-file-name":"chunk-0.bin","x-cowork-sha256":fileHash}});
afterEach(()=>{vi.unstubAllGlobals();vi.restoreAllMocks();});

it("sends no manifest request for an already canceled download",async()=>{
  const abort=new AbortController();abort.abort();const fetch=vi.fn();vi.stubGlobal("fetch",fetch);
  await expect(new Client("owner").downloadOwnerManagedResultFile("order","run",valid,"input-0001/proof.bin",abort.signal)).rejects.toMatchObject({name:"AbortError"});
  expect(fetch).not.toHaveBeenCalled();
});

it("aborts the original in-flight manifest request without reading chunks",async()=>{
  const abort=new AbortController();let started!:()=>void;
  const reached=new Promise<void>(resolve=>{started=resolve;});
  const fetch=vi.fn((_url:string,options:RequestInit)=>new Promise<Response>((_resolve,reject)=>{
    expect(options.signal).toBe(abort.signal);options.signal!.addEventListener("abort",()=>reject(new DOMException("Aborted","AbortError")),{once:true});started();
  }));vi.stubGlobal("fetch",fetch);
  const pending=new Client("owner").downloadOwnerManagedResultFile("order","run",valid,"input-0001/proof.bin",abort.signal);
  await reached;abort.abort();await expect(pending).rejects.toMatchObject({name:"AbortError"});expect(fetch).toHaveBeenCalledTimes(1);
});

it("does not return a successful file when canceled during its final hash",async()=>{
  const original=crypto.subtle.digest.bind(crypto.subtle);let finish!:(value:ArrayBuffer)=>void,started!:()=>void;
  const reached=new Promise<void>(resolve=>{started=resolve;});const abort=new AbortController();
  const empty=fixture.empty_file as OwnerManagedResultManifest;
  vi.spyOn(crypto.subtle,"digest").mockImplementation((algorithm,data)=>{
    // This fixture has no chunks; its zero-byte digest is uniquely the final file hash.
    if(data.byteLength===0)return new Promise(resolve=>{finish=resolve;started();});return original(algorithm,data);
  });
  vi.stubGlobal("fetch",vi.fn().mockResolvedValueOnce(json(empty)));
  const pending=new Client("owner").downloadOwnerManagedResultFile("order","run",empty,"input-0001/proof.bin",abort.signal);
  await reached;abort.abort();finish(Uint8Array.from(empty.manifest.files[0].sha256.match(/../g)!,part=>parseInt(part,16)).buffer);
  await expect(pending).rejects.toMatchObject({name:"AbortError"});
});

it("verifies a retained empty file without inventing a binary chunk read",async()=>{
  const empty=fixture.empty_file as OwnerManagedResultManifest;
  const fetch=vi.fn().mockResolvedValueOnce(json(empty));vi.stubGlobal("fetch",fetch);
  const result=await new Client("owner").downloadOwnerManagedResultFile("order","run",empty,"input-0001/proof.bin");
  expect(result.size).toBe(0);expect(await result.blob.text()).toBe("");expect(result.sha256).toBe(empty.manifest.files[0].sha256);
  expect(fetch).toHaveBeenCalledTimes(1);expect(fetch.mock.calls[0][0]).toMatch(/\/manifest$/);
});

it("scopes local metadata and a fully verified file without sending a receive POST",async()=>{
  const fetch=vi.fn().mockResolvedValueOnce(json({entries:[valid.entry]})).mockResolvedValueOnce(json(valid)).mockResolvedValueOnce(json(valid)).mockResolvedValueOnce(chunk());
  vi.stubGlobal("fetch",fetch);const client=new Client("private-owner","https://fixture.invalid");
  expect((await client.ownerManagedResults("order","run")).entries).toEqual([valid.entry]);
  const manifest=await client.ownerManagedResultManifest("order","run","version");
  const file=await client.downloadOwnerManagedResultFile("order","run",manifest,"input-0001/proof.bin");
  expect(await file.blob.text()).toBe("abc");expect(file.file_name).toBe("proof.bin");expect(file.sha256).toBe(fileHash);
  expect(fetch.mock.calls.every(call=>!call[0].includes("/receive"))).toBe(true);
  expect(fetch.mock.calls[3][0]).toContain("/orders/order/runs/run/managed-result-versions/version/chunk?path=input-0001%2Fproof.bin&offset=0&expected_sha256="+fileHash);
  expect(fetch.mock.calls[3][1].headers.Authorization).toBe("Bearer private-owner");
  expect(fetch.mock.calls[3][0]).not.toContain("private-owner");
});

it("never repeats a lost receive POST and allows a later read of its original partial state",async()=>{
  const receiving=structuredClone(valid.entry);receiving.version!.state="receiving";
  const fetch=vi.fn().mockRejectedValueOnce(new Error("lost reply")).mockResolvedValueOnce(json({entries:[receiving]}));vi.stubGlobal("fetch",fetch);
  const client=new Client("owner");
  await expect(client.receiveOwnerManagedResult("order","run","attempt")).rejects.toThrow("lost reply");
  expect(fetch).toHaveBeenCalledTimes(1);expect(fetch.mock.calls[0][1].method).toBe("POST");
  expect((await client.ownerManagedResults("order","run")).entries[0].version?.id).toBe("version");
  expect(fetch).toHaveBeenCalledTimes(2);expect(fetch.mock.calls[1][1].method).toBe("GET");
});

it("rejects foreign scope, duplicate attempts and mismatched version metadata",async()=>{
  const foreign=structuredClone(valid);foreign.entry.order_id="other";
  const changed=structuredClone(valid);changed.entry.version!.manifest_sha256="f".repeat(64);
  const fetch=vi.fn().mockResolvedValueOnce(json(foreign)).mockResolvedValueOnce(json({entries:[valid.entry,valid.entry]})).mockResolvedValueOnce(json(changed));vi.stubGlobal("fetch",fetch);
  const client=new Client("owner");
  await expect(client.ownerManagedResultManifest("order","run","version")).rejects.toThrow("managed_result_integrity");
  await expect(client.ownerManagedResults("order","run")).rejects.toThrow("managed_result_integrity");
  await expect(client.ownerManagedResultManifest("order","run","version")).rejects.toThrow("managed_result_integrity");
  expect(fetch).toHaveBeenCalledTimes(3);
});

it("refuses invalid offsets and missing parents before requesting any file bytes",async()=>{
  const offset=structuredClone(valid);offset.manifest.files[0].chunks[0].offset=1;
  const parent=structuredClone(valid);parent.manifest.directories=[];
  const fetch=vi.fn();vi.stubGlobal("fetch",fetch);const client=new Client("owner");
  await expect(client.downloadOwnerManagedResultFile("order","run",offset,"input-0001/proof.bin")).rejects.toThrow("managed_result_integrity");
  await expect(client.downloadOwnerManagedResultFile("order","run",parent,"input-0001/proof.bin")).rejects.toThrow("managed_result_integrity");
  expect(fetch).not.toHaveBeenCalled();
});

it("rechecks the original version revision before allocating or requesting chunks",async()=>{
  const changed=structuredClone(valid);changed.entry.version!.revision++;
  const fetch=vi.fn().mockResolvedValueOnce(json(changed));vi.stubGlobal("fetch",fetch);
  await expect(new Client("owner").downloadOwnerManagedResultFile("order","run",valid,"input-0001/proof.bin")).rejects.toThrow("managed_result_changed");
  expect(fetch).toHaveBeenCalledTimes(1);
});

it("rejects a declared length mismatch before reading the binary response body",async()=>{
  const response=chunk("abc","4"),read=vi.spyOn(response.body!,"getReader");
  const fetch=vi.fn().mockResolvedValueOnce(json(valid)).mockResolvedValueOnce(response);vi.stubGlobal("fetch",fetch);
  await expect(new Client("owner").downloadOwnerManagedResultFile("order","run",valid,"input-0001/proof.bin")).rejects.toThrow("download_integrity");
  expect(read).not.toHaveBeenCalled();expect(fetch).toHaveBeenCalledTimes(2);
});

it("refuses altered chunk data without retry and a valid chunk with an invalid complete-file hash",async()=>{
  const wrong=fixture.wrong_full_file_hash as OwnerManagedResultManifest;
  const fetch=vi.fn().mockResolvedValueOnce(json(valid)).mockResolvedValueOnce(chunk("xyz")).mockResolvedValueOnce(json(wrong)).mockResolvedValueOnce(chunk());vi.stubGlobal("fetch",fetch);
  const client=new Client("owner");
  await expect(client.downloadOwnerManagedResultFile("order","run",valid,"input-0001/proof.bin")).rejects.toThrow("download_integrity");
  expect(fetch).toHaveBeenCalledTimes(2);
  await expect(client.downloadOwnerManagedResultFile("order","run",wrong,"input-0001/proof.bin")).rejects.toThrow("download_integrity");
  expect(fetch).toHaveBeenCalledTimes(4);
});
