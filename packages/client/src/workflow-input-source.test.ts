import {readFileSync} from "node:fs";
import {createHash} from "node:crypto";
import {afterEach,expect,it,vi} from "vitest";
import {Client,decode,canonicalJsonValue,type PreparedExecutionPlanV2} from "./index";

// Real native review/queue fixture. The source input intentionally exceeds JS precision.
const fixture=JSON.parse(readFileSync(new URL("./fixtures/execution-workflow-input-source.json",import.meta.url),"utf8"));
const sha=(value:string)=>createHash("sha256").update(value).digest("hex");
const response=(value:unknown)=>new Response(JSON.stringify(value),{headers:{"content-type":"application/json"}});
const prepared=()=>decode("PreparedExecutionPlanV2",structuredClone(fixture.prepared));
const request=()=>decode("PrepareOwnerExecutionPlanV2",structuredClone(fixture.prepare));
const client=new Client("TRANSPORT-ONLY","https://fixture.invalid");
afterEach(()=>vi.unstubAllGlobals());

it("keeps the exact native input text and sends only its immutable reference through review and commit",async()=>{
 const fetch=vi.fn().mockResolvedValueOnce(response(fixture.snapshot)).mockResolvedValueOnce(response(prepared())).mockResolvedValueOnce(response(fixture.run));vi.stubGlobal("fetch",fetch);
 const snapshot=await client.workflowInputSnapshot(fixture.snapshot.source.run_id);
 expect(snapshot.canonical_json).toBe('{"amount":9007199254740993}');
 expect(JSON.parse(snapshot.canonical_json).amount).toBe(9007199254740992);
 const plan=await client.prepareExecutionPlanV2(request());
 expect(canonicalJsonValue(plan.canonical_json,["preflight","review","workflow","inputs"])).toBe(snapshot.canonical_json);
 expect(await client.commitExecutionPlanV2(plan)).toEqual(fixture.run);
 const sent=JSON.parse(fetch.mock.calls[1][1].body);expect(sent.request.input_source).toEqual(snapshot.source);expect(sent.request.inputs).toBeUndefined();
 expect(fetch.mock.calls.every(c=>!String(c[0]).endsWith('/runs'))).toBe(true);
});

it.each(["run","hash","rounded"])("rejects a changed %s from the native input read",async field=>{
 const value=structuredClone(fixture.snapshot);
 if(field==="run")value.source.run_id="different";else if(field==="hash")value.source.sha256="f".repeat(64);else value.canonical_json=value.canonical_json.replace("9007199254740993","9007199254740992");
 vi.stubGlobal("fetch",vi.fn().mockResolvedValue(response(value)));
 await expect(client.workflowInputSnapshot(fixture.snapshot.source.run_id)).rejects.toThrow("workflow_input_source_mismatch");
});

it("rejects input rounding even when the parsed display looks identical and the whole response has a fresh valid hash",async()=>{
 const p=prepared();p.canonical_json=p.canonical_json.replace("9007199254740993","9007199254740992");p.sha256=sha(p.canonical_json);
 expect(JSON.parse(p.canonical_json)).toEqual(p.plan);
 vi.stubGlobal("fetch",vi.fn().mockResolvedValue(response(p)));
 await expect(client.prepareExecutionPlanV2(request())).rejects.toThrow("workflow_input_source_mismatch");
});

it.each(["source","ambiguous"])("rejects a mismatched %s request before confirmation",async field=>{
 const r=request();if(!("kind" in r.request))throw new Error("workflow");
 if(field==="source")r.request.input_source!.run_id="other-run";else r.request.inputs={amount:9007199254740992};
 const fetch=vi.fn().mockResolvedValue(response(prepared()));vi.stubGlobal("fetch",fetch);
 await expect(client.prepareExecutionPlanV2(r)).rejects.toThrow(field==="source"?"execution_plan_v2_response_mismatch":"workflow_input_source_mismatch");
 expect(fetch.mock.calls.every(c=>!String(c[0]).endsWith('/commit'))).toBe(true);
});
