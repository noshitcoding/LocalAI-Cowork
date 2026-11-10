import {afterEach,expect,it,vi} from "vitest";
import {Client,type PrepareExecutionInput,type PreparedExecutionInput} from "./index";

// Golden input encoded independently with Python sorted-key JSON/SHA-256.
const request:PrepareExecutionInput={idempotency_key:"freeze-once",base_revision:1,folder_ids:[]};
const prepared:PreparedExecutionInput={snapshot:{format_version:1,id:"11111111-1111-4111-8111-111111111111",order_id:"order",project_id:"project",order_revision:1,
  request_sha256:"71258ba3b89287d72cac0bd47c669c124f7cc25ae8defaa4d46351b2d0d43c93",created_at:1,sources:[],
  manifest:{format_version:1,directories:[],files:[],total_bytes:0,sha256:"005f084c54e7a3af50096ad93c91670457b375f1ab1e1b18922f5c0ba9bcd96c"}},
  sha256:"836aa6404b0aee675379d13ada60a1d77e9e52c8ef2b84d90b3385a854bf4a54",retired:false};
const json=(value:unknown)=>new Response(JSON.stringify(value),{headers:{"content-type":"application/json"}});
afterEach(()=>vi.unstubAllGlobals());

it("prepares, explicitly reconciles and retires the same local input without exporting bytes",async()=>{
  const retired={...prepared,retired:true};
  const fetch=vi.fn().mockResolvedValueOnce(json(prepared)).mockResolvedValueOnce(json(prepared)).mockResolvedValueOnce(json(retired));vi.stubGlobal("fetch",fetch);
  const client=new Client("private-owner","https://fixture.invalid");
  const captured=await client.prepareExecutionInput("order",request);
  expect(captured).toEqual(prepared);
  expect(await client.inspectExecutionInput(captured)).toEqual(prepared);
  expect(await client.retireExecutionInput(captured)).toEqual(retired);
  expect(JSON.parse(fetch.mock.calls[0][1].body)).toEqual(request);
  expect(JSON.parse(fetch.mock.calls[2][1].body)).toEqual({sha256:prepared.sha256});
  for(const [url,options] of fetch.mock.calls){expect(url).not.toContain("private-owner");expect(url).not.toContain("/chunk");expect(options.headers.Authorization).toBe("Bearer private-owner");}
});

it("rejects substituted content, stale selections and receipt reversals",async()=>{
  const fetch=vi.fn();vi.stubGlobal("fetch",fetch);
  const client=new Client("owner");
  const changed=structuredClone(prepared);changed.snapshot.project_id="other-project";
  fetch.mockResolvedValueOnce(json(changed));
  await expect(client.prepareExecutionInput("order",request)).rejects.toThrow("execution_input_integrity");
  fetch.mockResolvedValueOnce(json(prepared));
  await expect(client.prepareExecutionInput("order",{...request,idempotency_key:"different-request"})).rejects.toThrow("execution_input_response_mismatch");
  fetch.mockResolvedValueOnce(json(prepared));
  await expect(client.retireExecutionInput(prepared)).rejects.toThrow("execution_input_receipt_mismatch");
  fetch.mockResolvedValueOnce(json(prepared));
  await expect(client.inspectExecutionInput({...prepared,retired:true})).rejects.toThrow("execution_input_receipt_mismatch");
  fetch.mockClear();
  await expect(client.prepareExecutionInput("order",{...request,folder_ids:["b","a"]})).rejects.toThrow("execution_input_selection_invalid");
  await expect(client.inspectExecutionInput({...prepared,sha256:"0".repeat(64)})).rejects.toThrow("execution_input_integrity");
  expect(fetch).not.toHaveBeenCalled();
});

it("never automatically retries a lost preparation answer",async()=>{
  const fetch=vi.fn().mockRejectedValueOnce(new TypeError("lost successful reply")).mockResolvedValueOnce(json(prepared));vi.stubGlobal("fetch",fetch);
  const client=new Client("owner");
  await expect(client.prepareExecutionInput("order",request)).rejects.toThrow("lost successful reply");
  expect(fetch).toHaveBeenCalledTimes(1);
  expect(await client.prepareExecutionInput("order",request)).toEqual(prepared);
  expect(fetch.mock.calls[0]).toEqual(fetch.mock.calls[1]);
});
