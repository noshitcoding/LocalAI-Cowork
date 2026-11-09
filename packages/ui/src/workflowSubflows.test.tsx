import {describe,it,expect} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {QueryClient,QueryClientProvider} from "@tanstack/react-query";
import {Client,decode,type WorkflowDefinition,type WorkflowNodeView} from "@cowork/client";
import fixture from "../../../tests/fixtures/workflow-subflow.json";
import {addWorkflowSubflow,changeSubflowResult,eligibleSubflowSource,removeWorkflowSubflow,subflowBudget} from "./workflowSubflows";
import {editableWorkflow,validWorkflowEditor} from "./workflowUi";
import {WorkflowDefinitionSummary} from "./WorkflowLibrary";
import {WorkflowSubflowsEditor} from "./WorkflowSubflowsEditor";
import {WorkflowDataflowEditor} from "./WorkflowDataflowEditor";
import {workflowNodeTree} from "./WorkflowNodeList";
import {restoreWorkflowEditor,workflowEditorFormat} from "./workflowJournal";
import {de} from "./i18n";

function definition():WorkflowDefinition{
  const value=decode("WorkflowDefinition",structuredClone(fixture));
  value.rights.tools=["read_text_file","write_text_file"];
  value.subflows!.invoke.rights.tools=["read_text_file"];
  value.subflows!.invoke.definition.rights.tools=["read_text_file","write_text_file"];
  return value;
}
const body=()=>structuredClone(definition().subflows!.invoke.definition);
describe("public frozen subflow editing",()=>{
  it("restores the full original tree, nested rights and exact arguments in the existing journal",()=>{
    const value=definition();expect(eligibleSubflowSource(value)).toBe(true);expect(validWorkflowEditor(value)).toBe(true);expect(editableWorkflow(value)).toBe(true);
    expect(restoreWorkflowEditor({editor_format:workflowEditorFormat,data:value,origin:null,archived:false})).toEqual(value);
  });
  it("copies a body and return independently and starts with explicit constant arguments",()=>{
    const parent=body(),source=body();parent.rights.tools=["read_text_file"];parent.rights.folders=["project"];parent.budgets.max_seconds=10;
    const result={step_id:"echo",pointer:"/amount"};const copy=addWorkflowSubflow(parent,source,result,"call");
    source.name="Changed later";source.dataflow!.steps.echo.output={type:"text"};result.pointer="/missing";
    expect(copy.subflows!.call.definition.name).toBe("Amount body");expect(copy.subflows!.call.result.pointer).toBe("/amount");
    expect(copy.subflows!.call.rights).toEqual({tools:["read_text_file"],folders:["project"]});expect(copy.subflows!.call.budgets.max_seconds).toBe(10);
    expect(copy.dataflow!.steps.call.inputs.amount.value).toEqual({source:"literal",value:0});expect(validWorkflowEditor(copy)).toBe(true);
  });
  it("materializes implicit sequential ordering before adding a call",()=>{
    const parent=body();parent.mode={mode:"sequential"};parent.controls={};parent.roles=decode("WorkflowDefinition",{...fixture.subflows.invoke.definition,roles:[{id:"reader",instructions:"Read selected input",provider_id:"model",provider_revision:1,rights:{tools:[],folders:[]},budgets:parent.budgets,delegate_to:[]}]}).roles;
    parent.steps=["first","second"].map(id=>({id,role_id:"reader",goal:id,dependencies:[],rights:{tools:[],folders:[]},budgets:parent.budgets}));
    parent.dataflow!.steps=Object.fromEntries(parent.steps.map(step=>[step.id,{inputs:{},output:{type:"object",fields:{}}}]));
    const copy=addWorkflowSubflow(parent,body(),{step_id:"echo",pointer:""},"call");
    expect(copy.steps[1].dependencies).toEqual(["first"]);expect(copy.subflows!.call.dependencies).toEqual(["first","second"]);expect(copy.mode).toEqual({mode:"parallel",max_parallel:1});expect(validWorkflowEditor(copy)).toBe(true);
  });
  it("retains broken data references for explicit repair when a call is removed or its result changes",()=>{
    const value=definition(),changed=changeSubflowResult(value,"invoke",{step_id:"echo",pointer:"/amount"});
    expect(changed.dataflow!.steps.invoke.output).toEqual({type:"integer"});expect(changed.dataflow!.steps.after.inputs.final).toEqual(value.dataflow!.steps.after.inputs.final);expect(validWorkflowEditor(changed)).toBe(false);
    const removed=removeWorkflowSubflow(value,"invoke");expect(removed.format_version).toBe(3);expect(removed.subflows).toBeUndefined();expect(removed.controls!.after.dependencies).toEqual([]);expect(removed.dataflow!.steps.after.inputs.final.value).toEqual({source:"step",step_id:"invoke",pointer:"/amount"});expect(validWorkflowEditor(removed)).toBe(false);
  });
  it("rejects cross-scope references, unsupported nested tools and inconsistent source versions",()=>{
    const value=definition();value.subflows!.invoke.definition.dataflow!.steps.echo.inputs.amount.value={source:"step",step_id:"after",pointer:"/final"};expect(validWorkflowEditor(value)).toBe(false);
    const unknown=definition();unknown.subflows!.invoke.definition.rights.tools.push("future_tool");expect(editableWorkflow(unknown)).toBe(false);
    const call=definition();call.subflows!.invoke.rights.tools.push("future_tool");expect(editableWorkflow(call)).toBe(false);
    const wrong=definition();wrong.format_version=3;expect(eligibleSubflowSource(wrong)).toBe(false);expect(()=>addWorkflowSubflow(body(),wrong,{step_id:"invoke",pointer:""},"call")).toThrow();
  });
  it("bounds nesting and expanded nodes and preserves omitted default token limits",()=>{
    let nested=body();for(let i=0;i<4;i++)nested=addWorkflowSubflow(body(),nested,{step_id:i?"call":"echo",pointer:""},"call");
    expect(eligibleSubflowSource(nested)).toBe(true);expect(()=>addWorkflowSubflow(body(),nested,{step_id:"call",pointer:""},"call")).toThrow();
    const large=body();large.controls=Object.fromEntries(Array.from({length:64},(_,i)=>["step"+i,structuredClone(large.controls!.echo)]));large.dataflow!.steps=Object.fromEntries(Object.keys(large.controls).map(id=>[id,structuredClone(body().dataflow!.steps.echo)]));
    expect(()=>addWorkflowSubflow(large,body(),{step_id:"echo",pointer:""},"call")).toThrow();
    const a=body().budgets,b={...a};delete b.max_total_tokens;expect(subflowBudget(a,b).max_total_tokens).toBe(a.max_total_tokens);
  });
  it("shows frozen nested content, explicit bindings and effective call limits without raw source JSON",()=>{
    const value=definition();const summary=renderToStaticMarkup(<WorkflowDefinitionSummary definition={value} providers={[]} t={key=>de[key]}/>);expect(summary).toContain("Unterablauf");expect(summary).toContain("Copy selected input");
    const editor=renderToStaticMarkup(<QueryClientProvider client={new QueryClient()}><WorkflowSubflowsEditor client={new Client("fixture")} project="project" value={value} onChange={()=>{}}/></QueryClientProvider>);
    expect(editor).toContain("Wirksame Obergrenzen");expect(editor).toContain("Rechte dieses Aufrufs");expect(editor).toContain("Spätere Änderungen");expect(editor).not.toContain("&quot;format_version&quot;");
    const bindings=renderToStaticMarkup(<WorkflowDataflowEditor value={value} onChange={()=>{}}/>);expect(bindings).toContain("Frozen amount subflow");
  });
  it("groups nested calls and keeps orphaned and cyclic scope responses visible",()=>{
    const node=(id:string,scope_id?:string,kind?:WorkflowNodeView["kind"]):WorkflowNodeView=>({id,scope_id,kind,goal:id,dependencies:[],state:"pending",steps:0,charged_tokens:0});
    const nodes=[node("leaf","inner"),node("outer",undefined,"subflow"),node("inner","outer","subflow"),node("orphan","missing"),node("cycle-a","cycle-b","subflow"),node("cycle-b","cycle-a","subflow")];
    const tree=workflowNodeTree(nodes);expect(tree.find(branch=>branch.node.id==="outer")!.children[0].children[0].node.id).toBe("leaf");
    const flatten=(branches:ReturnType<typeof workflowNodeTree>):string[]=>branches.flatMap(branch=>[branch.node.id,...flatten(branch.children)]);
    expect(flatten(tree).sort()).toEqual(nodes.map(node=>node.id).sort());
  });
  it("orders each scope by actual dependencies even when the response is sorted by generated IDs",()=>{
    const node=(id:string,dependencies:string[],scope_id?:string,kind?:WorkflowNodeView["kind"]):WorkflowNodeView=>({id,scope_id,kind,goal:id,dependencies,state:"pending",steps:0,charged_tokens:0});
    const tree=workflowNodeTree([node("after",["wait","entry"],"call"),node("call",["after","entry","wait"],undefined,"subflow"),node("entry",["root"],"call","arguments"),node("root",[]),node("wait",["entry"],"call")]);
    expect(tree.map(branch=>branch.node.id)).toEqual(["root","call"]);expect(tree[1].children.map(branch=>branch.node.id)).toEqual(["entry","wait","after"]);
  });
});
