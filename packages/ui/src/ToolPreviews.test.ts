import {describe,expect,it} from "vitest";
import {readFileSync} from "node:fs";
import type {RunEvent} from "@cowork/client";
import {previewGroups} from "./ToolPreviews";

const event=(kind:string,payload:unknown)=>({kind,payload,sequence:1,run_id:"run",created_at:0}) as unknown as RunEvent;
describe("document preview groups",()=>{
  it("shows the original read-only native OCR preview after a completed workflow",()=>{
    const original=JSON.parse(readFileSync(new URL("../../../tests/fixtures/workflow-native-ocr-event.json",import.meta.url),"utf8")) as RunEvent;
    const receipt=(original.payload as any).receipt;
    expect(previewGroups([original])).toEqual([{tool_call_id:"workflow:"+original.id,name:"document_read",images:[{sha256:receipt.result.cowork_images[0].sha256,page:1,width:1131,height:1600}]}]);
    const model=structuredClone(original);model.kind="workflow_tool_completed";
    expect(previewGroups([model])).toEqual(previewGroups([original]));
  });
  it("ignores failed, malformed and oversized workflow preview references",()=>{
    const original=JSON.parse(readFileSync(new URL("../../../tests/fixtures/workflow-native-ocr-event.json",import.meta.url),"utf8")) as RunEvent;
    const variants=[
      (p:any)=>{p.receipt.succeeded=false;},(p:any)=>{p.receipt=null;},
      (p:any)=>{p.receipt.tool.name="execute_program";},(p:any)=>{p.receipt.result.cowork_images=[null,{sha256:"../foreign"}];},
      (p:any)=>{p.receipt.result.cowork_images=Array(17).fill(p.receipt.result.cowork_images[0]);},
    ];
    for(const change of variants){const damaged=structuredClone(original);change(damaged.payload);expect(previewGroups([damaged])).toEqual([]);}
    expect(previewGroups([event("tool_completed",null),event("tool_completed",{tool_call_id:"1",images:[null]})])).toEqual([]);
  });
  it("keeps only valid preview references of completed tools",()=>{
    const sha="a".repeat(64);
    const groups=previewGroups([
      event("tool_completed",{tool_call_id:"1",name:"document_read",images:[{sha256:sha,page:2,width:10,height:20},{sha256:"../x"}]}),
      event("tool_completed",{tool_call_id:"2",name:"write_text_file",result:{}}),
      event("approval_requested",{images:[{sha256:sha}]}),
    ]);
    expect(groups).toEqual([{tool_call_id:"1",name:"document_read",images:[{sha256:sha,page:2,width:10,height:20}]}]);
  });
});
