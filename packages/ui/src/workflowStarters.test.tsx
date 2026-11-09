import {describe,expect,it} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {decode,type ProviderProfile} from "@cowork/client";
import {validWorkflowEditor} from "./workflowUi";
import {convertV1Pipeline,createStarterWorkflow,readV1Pipelines,workflowImportLimit,workflowStarters,WorkflowImportError,type V1Pipeline} from "./workflowStarters";
import {WorkflowStarterImport,workflowImportMessage} from "./WorkflowStarterImport";

const provider:ProviderProfile={id:"current-provider",revision:7,name:"Local model",model:"fixture",kind:"ollama",endpoint:"http://127.0.0.1:11434",context_tokens:1024,credential_ready:true,credential_required:false};
const selection={id:provider.id,revision:provider.revision};
const pipeline=(steps:unknown[],extra:Partial<V1Pipeline>={}):V1Pipeline=>({id:"v1-id",name:"Old pipeline",description:"Description is not an execution instruction",steps_json:JSON.stringify(steps),zero_context:false,...extra});

describe("native starter workflow and explicit V1 conversion",()=>{
  it("creates each V1 starter as a separate Plan/Execute/Review graph using the explicitly selected current model",()=>{
    for(const language of ["de","en"] as const)for(const preset of workflowStarters){
      const definition=createStarterWorkflow(preset.id,selection,[provider],language,"Verify release evidence");
      expect(decode("WorkflowDefinition",definition)).toEqual(definition);expect(validWorkflowEditor(definition)).toBe(true);
      expect(definition.roles.map(role=>role.id)).toEqual(["Plan","Execute","Review"]);
      expect(definition.steps.map(step=>step.dependencies)).toEqual([[],["plan"],["plan","execute"]]);
      expect(definition.roles.every(role=>role.provider_id===provider.id&&role.provider_revision===7)).toBe(true);
      expect(definition.rights.folders).toEqual([]);expect(definition.rights.tools).not.toContain("browser");
      expect(definition.roles.flatMap(role=>role.delegate_to)).toEqual([]);
      expect(definition.budgets.max_output_tokens).toBe(256);
      expect(definition.steps.every(step=>step.goal.includes("Verify release evidence"))).toBe(true);
      definition.roles[0].rights.tools.push("locally-edited");
      expect(createStarterWorkflow(preset.id,selection,[provider],language,"Fresh").roles[0].rights.tools).not.toContain("locally-edited");
    }
  });
  it("retains V1 prompt precedence, full earlier output context, order, and stop-on-first-error behavior without granting tools",()=>{
    const input=pipeline([{tool:"ollama",prompt:"First",args:"Ignored argument"},{args:"Second"},{tool:null,prompt:null,args:"Third"}]);
    const before=JSON.stringify(input),converted=convertV1Pipeline(input,selection,[provider]);
    expect(converted.steps.map(step=>step.goal)).toEqual(["Tool: ollama\nTask:\nFirst","Tool: ollama\nTask:\nSecond","Tool: ollama\nTask:\nThird"]);
    expect(converted.steps.map(step=>step.dependencies)).toEqual([[],["v1-step-1"],["v1-step-1","v1-step-2"]]);
    expect(converted.mode).toEqual({mode:"sequential"});expect(converted.failure).toBe("stop");
    expect(converted.roles).toHaveLength(1);expect(converted.rights).toEqual({tools:[],folders:[]});
    expect(converted.steps.every(step=>!step.rights.tools.length&&!step.rights.folders.length)).toBe(true);
    expect(JSON.stringify(converted)).not.toContain(input.description);expect(JSON.stringify(converted)).not.toContain("v1-id");
    expect(JSON.stringify(input)).toBe(before);expect(validWorkflowEditor(converted)).toBe(true);
  });
  it("reads actual RpcPipeline records and lists while refusing app backups and secret-bearing configuration fields",()=>{
    const input={...pipeline([{prompt:"Do work"}]),created_at:"2026-10-17",updated_at:"2026-10-17"};
    expect(readV1Pipelines(JSON.stringify([input]))).toEqual([pipeline([{prompt:"Do work"}])]);
    expect(readV1Pipelines(JSON.stringify(input))).toHaveLength(1);
    for(const input of [{pipelines:[pipeline([])]},{...pipeline([]),apiKey:"synthetic-secret"},{...pipeline([]),config:{endpoint:"https://private.example"}},[{tool:"ollama",prompt:"bare steps are not records"}]])expect(()=>readV1Pipelines(JSON.stringify(input))).toThrow(WorkflowImportError);
    try{readV1Pipelines(JSON.stringify({...pipeline([]),apiKey:"synthetic-secret"}));}catch(error){expect(workflowImportMessage(error,"en")).not.toContain("synthetic-secret");}
  });
  it("maps actual web/planner/gateway semantics and isolates prior step output without granting imported MCP authority",()=>{
    const converted=convertV1Pipeline(pipeline([{tool:"web_fetch",prompt:"ignored",args:"https://example.invalid/page"},{tool:"web_search",args:"exact query"},{tool:"planner",prompt:"Make a plan"},{tool:"custom-gateway",args:{task:"Do work"}}],{zero_context:true}),selection,[provider]);
    expect(converted.steps[0].execution).toEqual({kind:"web_fetch",url:"https://example.invalid/page"});expect(converted.steps[1].execution).toEqual({kind:"web_search",query:"exact query"});
    expect(converted.steps.every(step=>step.zero_context)).toBe(true);expect(converted.steps[3].dependencies).toHaveLength(3);
    expect(converted.roles.find(role=>role.id==="Planner")?.agent_config?.execution_mode).toBe("plan");expect(converted.rights.tools).toEqual(["browser"]);
    expect(converted.steps[3].goal).toContain("custom-gateway");expect(converted.steps[3].rights.tools).toEqual([]);expect(converted.steps[3].execution).toBeUndefined();
    expect(()=>convertV1Pipeline(pipeline([{tool:"web_fetch",args:"file:///private"}]),selection,[provider])).toThrow("invalid_text");
    expect(()=>convertV1Pipeline(pipeline([{prompt:"Task",model:"unknown"}]),selection,[provider])).toThrow("unsupported_field");
    expect(()=>convertV1Pipeline(pipeline([{args:{api_key:"private"}}]),selection,[provider])).toThrow("unsupported_field");
    expect(()=>convertV1Pipeline(pipeline(["untyped step"]),selection,[provider])).toThrow("unsupported_shape");
    expect(()=>convertV1Pipeline(pipeline([{prompt:"\ud800"}]),selection,[provider])).toThrow("invalid_text");
  });
  it("requires an exact model revision and bounds UTF8 input and graph size",()=>{
    const input=pipeline([{prompt:"Task"}]);
    expect(()=>convertV1Pipeline(input,{id:"",revision:0},[provider])).toThrow("model_missing");
    expect(()=>convertV1Pipeline(input,{...selection,revision:6},[provider])).toThrow("model_changed");
    expect(()=>createStarterWorkflow("build",selection,[],"en","Task")).toThrow("model_changed");
    expect(()=>readV1Pipelines(JSON.stringify(pipeline([{prompt:"😀".repeat(workflowImportLimit/4)}])))).toThrow("limit");
    expect(()=>convertV1Pipeline(pipeline(Array.from({length:65},()=>({prompt:"Task"}))),selection,[provider])).toThrow("limit");
    const maximum=convertV1Pipeline(pipeline(Array.from({length:64},(_,index)=>({prompt:"Step "+index}))),selection,[provider]);
    expect(maximum.steps).toHaveLength(64);expect(maximum.roles).toHaveLength(1);
    expect(()=>convertV1Pipeline(pipeline(Array.from({length:63},()=>({prompt:"x".repeat(15600)}))),selection,[provider])).toThrow("draft_limit");
  });
  it("renders disabled draft entry points, explicit model choice and truthful conversion limitations",()=>{
    const markup=renderToStaticMarkup(<WorkflowStarterImport providers={[provider]} language="en" disabled onCreate={()=>{throw new Error("Rendering must not create drafts");}} summary={()=>null}/>);
    expect(markup).toContain("Explicitly choose a model");expect(markup).toContain("Review conversion");expect(markup).toContain("Open starter as draft");
    expect(markup).toContain("zero_context. Gateway/tool names remain inert");expect(markup).toContain("fieldset disabled");
    expect(workflowImportMessage(new WorkflowImportError("unsupported_tool",2),"de")).toContain("Schritt 2");
  });
});
