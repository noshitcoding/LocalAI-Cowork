import {afterEach, describe, expect, it, vi} from "vitest";
import {Client, defaultRunBudgets, type WorkflowDefinition} from "@cowork/client";
import {editableWorkflow, findWorkflowSelection, validWorkflowEditor, workflowRoleInstructionsValid, workflowSelection, prepareWorkflowStart, WorkflowRequestLatch} from "./workflowUi";
const definition=():WorkflowDefinition=>({format_version:1,name:"Research and review",mode:{mode:"parallel",max_parallel:2},failure:"continue_independent",rights:{tools:["create_artifact"],folders:[]},budgets:{...defaultRunBudgets},roles:[{id:"Research",instructions:"Use sources",provider_id:"local",provider_revision:2,rights:{tools:["create_artifact"],folders:[]},budgets:{...defaultRunBudgets},delegate_to:[]}],steps:[{id:"first",role_id:"Research",goal:"Research",dependencies:[],rights:{tools:["create_artifact"],folders:[]},budgets:{...defaultRunBudgets}},{id:"second",role_id:"Research",goal:"Review",dependencies:["first"],rights:{tools:[],folders:[]},budgets:{...defaultRunBudgets}}]});
afterEach(()=>vi.unstubAllGlobals());
describe("workflow editor decisions",()=>{
  it("uses the256000-byte role instruction bound and refuses NUL without truncation",()=>{const instructions="😀".repeat(64000);expect(workflowRoleInstructionsValid(instructions)).toBe(true);const value=definition();value.roles[0].instructions=instructions;expect(validWorkflowEditor(value)).toBe(true);value.roles[0].instructions+="x";expect(validWorkflowEditor(value)).toBe(false);expect(workflowRoleInstructionsValid("Valid\0Hidden")).toBe(false);});
  it("rejects cyclic dependencies including a backwards edge introduced by sequential order",()=>{
    const value=definition();expect(validWorkflowEditor(value)).toBe(true);
    value.steps[0].dependencies=["second"];expect(validWorkflowEditor(value)).toBe(false);
    value.steps[1].dependencies=[];expect(validWorkflowEditor(value)).toBe(true);
    value.mode={mode:"sequential"};expect(validWorkflowEditor(value)).toBe(false);
  });
  it("rejects missing, duplicate or invalid role references without assigning another model",()=>{
    const value=definition();value.steps[1].role_id="missing";expect(validWorkflowEditor(value)).toBe(false);
    value.steps[1].role_id="Research";value.roles.push(structuredClone(value.roles[0]));expect(validWorkflowEditor(value)).toBe(false);
    value.roles.pop();value.roles[0].provider_revision=0;expect(validWorkflowEditor(value)).toBe(false);
    value.roles[0].provider_revision=2;value.roles[0].id="Research team";expect(validWorkflowEditor(value)).toBe(false);
  });
  it("supports folder rights and hierarchical delegation while rejecting unknown or malformed tool capabilities",()=>{
    const value=definition();expect(editableWorkflow(value)).toBe(true);
    value.steps[1].rights.folders=["private-folder"];expect(editableWorkflow(value)).toBe(true);
    value.roles[0].rights.tools.push("read_text_file");expect(editableWorkflow(value)).toBe(true);
    value.mode={mode:"hierarchical",max_parallel:2,max_depth:2,max_delegations:4};expect(editableWorkflow(value)).toBe(true);
    value.roles[0].rights.tools.push("unsupported_future_tool");expect(editableWorkflow(value)).toBe(false);
    value.roles[0].rights.tools=["mcp_"+"a".repeat(40)];expect(editableWorkflow(value)).toBe(true);
    value.roles[0].rights.tools=["mcp_unverified_alias"];expect(editableWorkflow(value)).toBe(false);
  });
  it("selects a concrete immutable version without delimiter collisions or fallback to the latest",()=>{
    const items=[{id:"a/b",project_id:"project",name:"Workflow",revision:3,revoked:false,versions:[{id:"older",revision:1,name:"Earlier",sha256:"a".repeat(64),created_at:1},{id:"newer",revision:2,name:"Latest",sha256:"b".repeat(64),created_at:2}]}];
    expect(findWorkflowSelection(items,workflowSelection("a/b","older"))?.version.id).toBe("older");
    expect(findWorkflowSelection(items,workflowSelection("a","b/older"))).toBeUndefined();
    expect(findWorkflowSelection(items,workflowSelection("a/b","removed"))).toBeUndefined();
  });
  it("retries the actual client publication with the same key and frozen definition after lost acknowledgement",async()=>{
    const client=new Client("private-test-token","http://fixture");
    const latch=new WorkflowRequestLatch<Parameters<Client["createWorkflow"]>[1]>();const draft=definition();let builds=0;
    const received:unknown[]=[];vi.stubGlobal("fetch",vi.fn(async(_url:unknown,init?:RequestInit)=>{
      received.push(JSON.parse(String(init?.body)));if(received.length===1)throw new TypeError("lost acknowledgement");
      return new Response(JSON.stringify({id:"created",project_id:"project",name:"Research and review",revision:1,revoked:false,versions:[{id:"version",revision:1,name:"Research and review",sha256:"a".repeat(64),created_at:1}]}),{status:200});
    }));
    const build=()=>{builds++;return {idempotency_key:"original-key",definition:draft};};
    await expect(client.createWorkflow("project",latch.prepare(build))).rejects.toThrow("lost acknowledgement");
    draft.name="Edited after uncertain outcome";draft.roles[0].provider_id="different-provider";
    const retry=latch.prepare(build);retry.definition.steps[0].goal="External mutation";
    await client.createWorkflow("project",latch.prepare(build));
    expect(builds).toBe(1);expect(received[1]).toEqual(received[0]);expect(latch.pending).toBe(true);
    latch.clear();expect(latch.pending).toBe(false);expect(latch.prepare(build).definition.name).toBe("Edited after uncertain outcome");
  });
  it("binds a workflow start to the checked immutable version and refuses mismatched preflight",async()=>{
    const client=new Client("private-test-token","http://fixture");
    const input={kind:"workflow" as const,order_id:"order",base_revision:2,idempotency_key:"start-key",prompt:"Research",workflow_id:"workflow",version_id:"version",access_mode:"confirm_mutations" as const,budgets:{...defaultRunBudgets}};
    const workflow={workflow_id:"workflow",version_id:"version",definition_sha256:"a".repeat(64),plan_sha256:"b".repeat(64)};
    let response={ready:true,workflow,name:"Research",version_revision:1,executor:"local-v2",policy_revision:1,access_mode:"confirm_mutations",budgets:defaultRunBudgets,allowed_tools:[],roles:[],max_parallel:1};
    const received:unknown[]=[];vi.stubGlobal("fetch",vi.fn(async(_url:unknown,init?:RequestInit)=>{received.push(JSON.parse(String(init?.body)));return new Response(JSON.stringify(response),{status:200});}));
    const prepared=await prepareWorkflowStart(client,input);
    expect(received[0]).toEqual(input);expect(prepared.expected_binding).toEqual(workflow);expect(prepared.idempotency_key).toBe("start-key");
    response={...response,workflow:{...workflow,version_id:"other-version"}};
    await expect(prepareWorkflowStart(client,input)).rejects.toThrow("workflowUnavailable");
    expect(prepared.expected_binding?.version_id).toBe("version");
  });

});
