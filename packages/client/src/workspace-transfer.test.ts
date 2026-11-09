import {afterEach,expect,it,vi} from "vitest";
import {Client,type WorkspaceTransferManifest} from "./index";

// Independently encoded with Python's sorted-key canonical JSON and SHA-256.
const fileHash="ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
const manifest:WorkspaceTransferManifest={format_version:1,directories:[],files:[{path:"proof.bin",size:3,sha256:fileHash,chunks:[{offset:0,size:3,sha256:fileHash}]}],total_bytes:3,sha256:"0f39f1b1f56d70c5710e4306f1719e17ddfb2efe8a8a7a2e980b282ba17c7b1f"};
const json=(value:unknown)=>new Response(JSON.stringify(value),{headers:{"content-type":"application/json"}});
const chunk=(data="abc",length="3")=>new Response(data,{headers:{"content-type":"application/octet-stream","content-length":length,"x-cowork-file-name":"chunk-0.bin","x-cowork-sha256":fileHash}});
afterEach(()=>vi.unstubAllGlobals());

it("verifies the portable manifest and repeated source-scoped chunk reads",async()=>{
  const fetch=vi.fn().mockResolvedValueOnce(json(manifest)).mockResolvedValueOnce(chunk()).mockResolvedValueOnce(chunk());vi.stubGlobal("fetch",fetch);
  const client=new Client("private-owner","https://fixture.invalid");
  const selected=await client.workspaceTransferManifest("order","run");
  expect(selected).toEqual(manifest);
  const first=await client.downloadWorkspaceTransferChunk("order","run",selected,"proof.bin",0);
  const repeated=await client.downloadWorkspaceTransferChunk("order","run",selected,"proof.bin",0);
  expect(await first.blob.text()).toBe("abc");expect(await repeated.blob.text()).toBe("abc");
  expect(fetch.mock.calls[1][0]).toBe(fetch.mock.calls[2][0]);
  expect(fetch.mock.calls[1][0]).toContain("/orders/order/runs/run/workspace/transfer/chunk?path=proof.bin&offset=0&expected_sha256="+fileHash);
  expect(fetch.mock.calls[1][0]).not.toContain("private-owner");
  expect(fetch.mock.calls[1][1].headers.Authorization).toBe("Bearer private-owner");
});

it("refuses altered manifests and undeclared offsets before downloading data",async()=>{
  const fetch=vi.fn();vi.stubGlobal("fetch",fetch);
  const client=new Client("owner");
  const changed=structuredClone(manifest);changed.files[0].path="substituted.bin";
  fetch.mockResolvedValueOnce(json(changed));
  await expect(client.workspaceTransferManifest("order","run")).rejects.toThrow("workspace_transfer_manifest_integrity");
  fetch.mockClear();
  await expect(client.downloadWorkspaceTransferChunk("order","run",manifest,"proof.bin",1)).rejects.toThrow("workspace_transfer_chunk_invalid");
  await expect(client.downloadWorkspaceTransferChunk("order","run",manifest,"other.bin",0)).rejects.toThrow("workspace_transfer_chunk_invalid");
  expect(fetch).not.toHaveBeenCalled();
});

it("rejects truncated or substituted chunk bodies without replay",async()=>{
  const fetch=vi.fn().mockResolvedValueOnce(chunk("abc","4")).mockResolvedValueOnce(chunk("xyz"));vi.stubGlobal("fetch",fetch);
  const client=new Client("owner");
  await expect(client.downloadWorkspaceTransferChunk("order","run",manifest,"proof.bin",0)).rejects.toThrow("download_integrity");
  expect(fetch).toHaveBeenCalledTimes(1);
  await expect(client.downloadWorkspaceTransferChunk("order","run",manifest,"proof.bin",0)).rejects.toThrow("download_integrity");
  expect(fetch).toHaveBeenCalledTimes(2);
});
