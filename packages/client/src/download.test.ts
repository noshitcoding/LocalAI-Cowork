import {afterEach,describe,it,expect,vi} from "vitest";
import {Client,decode,readDownload,RequestError} from "./index";
const bytes=new Uint8Array([0,255,128,42]);
async function digest(data:Uint8Array<ArrayBuffer>){return Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256",data))).map(b=>b.toString(16).padStart(2,"0")).join("");}
async function response(data=bytes,headers:Record<string,string>={}){
  return new Response(data,{headers:{"content-type":"application/octet-stream","content-length":String(data.length),"x-cowork-file-name":encodeURIComponent("Prüfung 📄.bin"),"x-cowork-sha256":await digest(data),...headers}});
}
afterEach(()=>vi.unstubAllGlobals());
describe("Authenticated verified file download",()=>{
  it("preserves non-text bytes and Unicode names and sends credentials only as a header",async()=>{
    const fetch=vi.fn().mockResolvedValue(await response());vi.stubGlobal("fetch",fetch);
    const file=await new Client("private-token").downloadWorkspace("order",{path:"nested/Prüfung 📄.bin",expected_sha256:await digest(bytes)},"historical-run");
    expect(file.file_name).toBe("Prüfung 📄.bin");
    expect(new Uint8Array(await file.blob.arrayBuffer())).toEqual(bytes);
    expect(fetch.mock.calls[0][0]).toContain("/orders/order/runs/historical-run/workspace/download?");
    expect(fetch.mock.calls[0][0]).not.toContain("private-token");
    expect(fetch.mock.calls[0][1].headers.Authorization).toBe("Bearer private-token");
    const empty=await readDownload(await response(new Uint8Array()));expect(empty.blob.size).toBe(0);
  });
  it("rejects altered, truncated, oversized or unsafe responses before producing a blob",async()=>{
    const invalidHeaders:Record<string,string>[]=[
      {"x-cowork-sha256":"0".repeat(64)}, {"content-length":"3"}, {"content-length":"5"},
      {"content-length":"67108865"}, {"content-length":"invalid"}, {"content-type":"text/html"},
      {"x-cowork-file-name":encodeURIComponent("../outside")}, {"x-cowork-file-name":"%0d%0aInjected"},
      {"x-cowork-file-name":"%"}, {"x-cowork-sha256":"unknown"},
    ];
    for(const headers of invalidHeaders)await expect(readDownload(await response(bytes,headers))).rejects.toThrow();
    await expect(readDownload(await response(),"1".repeat(64))).rejects.toThrow("download_integrity");
    expect(()=>decode("DownloadMetadata",{file_name:"📄".repeat(1024),size:0,sha256:"0".repeat(64)})).not.toThrow();
    expect(()=>decode("DownloadMetadata",{file_name:"📄".repeat(1025),size:0,sha256:"0".repeat(64)})).toThrow();
  });
  it("preserves structured denial and conflict without interpreting the body as a file",async()=>{
    const failure=new Response(JSON.stringify({code:"file_conflict",message:"Changed",retryable:false,correlation_id:"test"}),{status:409,headers:{"content-type":"application/json"}});
    const error=await readDownload(failure).catch(e=>e);expect(error).toBeInstanceOf(RequestError);expect(error.details.code).toBe("file_conflict");
  });
});
