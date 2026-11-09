import {afterEach,expect,it,vi} from "vitest";
import {Client,uploadMetadata,type FileUpload,RequestError} from "./index";
const bytes=new Uint8Array([0,255,128,42]);
async function fixture():Promise<FileUpload>{return {id:"upload",order_id:"order",project_id:"project",file:await uploadMetadata(new Blob([bytes]),"Prüfung 📄.bin"),state:"reserved",revision:1,created_at:0};}
afterEach(()=>vi.unstubAllGlobals());
it("binds reservation and raw upload receipts to the order and bytes, with header-only authentication",async()=>{
  const upload=await fixture();const fetch=vi.fn().mockResolvedValueOnce(new Response(JSON.stringify(upload))).mockResolvedValueOnce(new Response(JSON.stringify({...upload,state:"ready",revision:2})));vi.stubGlobal("fetch",fetch);
  const client=new Client("private-token");const reserved=await client.reserveUpload("order",{idempotency_key:"stable-request",file:upload.file});
  const blob=new Blob([bytes]);const result=await client.uploadContent("order",reserved,blob);expect(result.state).toBe("ready");
  expect(JSON.parse(fetch.mock.calls[0][1].body).idempotency_key).toBe("stable-request");
  expect(fetch.mock.calls[1][1].body).toBe(blob);expect(fetch.mock.calls[1][1].headers["Content-Type"]).toBe("application/octet-stream");
  for(const [url,request] of fetch.mock.calls){expect(url).not.toContain("private-token");expect(request.headers.Authorization).toBe("Bearer private-token");}
  expect(await uploadMetadata(new Blob([]),"empty.bin")).toMatchObject({size:0,sha256:"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"});
});
it("rejects changed or oversized input before network transfer and rejects a mismatched completion",async()=>{
  const upload=await fixture();const fetch=vi.fn();vi.stubGlobal("fetch",fetch);const client=new Client("token");
  await expect(client.uploadContent("order",upload,new Blob(["bad"]))).rejects.toThrow("upload_integrity");
  await expect(client.uploadContent("foreign",upload,new Blob([bytes]))).rejects.toThrow("upload_integrity");
  await expect(uploadMetadata({size:67_108_865} as Blob,"large.bin")).rejects.toThrow("upload_too_large");expect(fetch).not.toHaveBeenCalled();
  fetch.mockResolvedValue(new Response(JSON.stringify({...upload,state:"ready",order_id:"foreign"})));
  await expect(client.uploadContent("order",upload,new Blob([bytes]))).rejects.toThrow("upload_integrity");
});
it("preserves transfer rejection without an automatic retry or a fabricated ready state",async()=>{
  const upload=await fixture();const failure={code:"storage_unavailable",message:"Not confirmed",retryable:true,correlation_id:"fixture"};
  const fetch=vi.fn().mockResolvedValue(new Response(JSON.stringify(failure),{status:503}));vi.stubGlobal("fetch",fetch);
  const error=await new Client("token").uploadContent("order",upload,new Blob([bytes])).catch(e=>e);
  expect(error).toBeInstanceOf(RequestError);expect(error.details).toEqual(failure);expect(fetch).toHaveBeenCalledTimes(1);
});
