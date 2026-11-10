import {describe,expect,it} from "vitest";
import {decode,type WorkflowDefinition} from "@cowork/client";
import fixture from "../../../tests/fixtures/workflow-iteration.json";
import {addWorkflowIteration,changeIterationResult,defaultIterationArguments,iterationOutputType,removeWorkflowIteration,validWorkflowIterations} from "./workflowIterations";
import {addWorkflowSubflow,eligibleSubflowSource,removeWorkflowSubflow} from "./workflowSubflows";
import {validWorkflowEditor,editableWorkflow} from "./workflowUi";
import {validWorkflowDataflow,workflowDataAncestors,workflowDataNodes} from "./workflowDataflow";
import {removeWorkflowNode} from "./workflowControls";

const definition=()=>decode("WorkflowDefinition",structuredClone(fixture));
describe("public typed list definitions",()=>{
  it("accepts the native source fixture and includes the list in typed dependencies",()=>{
    const value=definition();
    expect(validWorkflowIterations(value)).toBe(true);expect(validWorkflowDataflow(value)).toBe(true);
    expect(validWorkflowEditor(value)).toBe(true);expect(editableWorkflow(value)).toBe(true);
    expect(workflowDataNodes(value).map(node=>node.id)).toEqual(["after","loop"]);
    expect(workflowDataAncestors(value,"after")).toEqual(new Set(["loop"]));
    expect(eligibleSubflowSource(value)).toBe(false);
  });
  it("copies the body, chosen result, source and explicit item binding without aliasing",()=>{
    const value=definition(),body=value.iterations!.loop.definition,parent:WorkflowDefinition={...value,format_version:3,iterations:undefined,controls:{},dataflow:{...value.dataflow!,steps:{}}};
    const source=structuredClone(value.dataflow!.steps.loop.inputs.items),args=defaultIterationArguments(body);
    expect(args.amount).toEqual({source:"literal",value:0});args.amount={source:"item"};
    const next=addWorkflowIteration(parent,body,{step_id:"echo",pointer:"/amount"},"each",source,args);
    source.value={source:"input",pointer:"/missing"};args.amount={source:"literal",value:999};body.name="Changed later";
    expect(next.iterations!.each.definition.name).toBe("Amount body");expect(next.iterations!.each.arguments.amount).toEqual({source:"item"});
    expect(next.dataflow!.steps.each.inputs.items.value).toEqual({source:"input",pointer:"/values"});
    expect(validWorkflowEditor(next)).toBe(true);
  });
  it("keeps root subflows and lists together when either kind is added or removed",()=>{
    const value=definition(),body=value.iterations!.loop.definition;
    const both=addWorkflowSubflow(value,body,{step_id:"echo",pointer:"/amount"},"call");
    expect(both.format_version).toBe(5);expect(validWorkflowEditor(both)).toBe(true);
    const withoutCall=removeWorkflowSubflow(both,"call");expect(withoutCall.format_version).toBe(5);expect(validWorkflowEditor(withoutCall)).toBe(true);
    const withoutLoop=removeWorkflowIteration(both,"loop");expect(withoutLoop.format_version).toBe(4);expect(withoutLoop.subflows!.call.dependencies).not.toContain("loop");
    expect(withoutLoop.dataflow!.steps.after.inputs.results.value).toEqual(value.dataflow!.steps.after.inputs.results.value);
    expect(validWorkflowEditor(withoutLoop)).toBe(false);
  });
  it("rejects implicit context, wrong item paths, incorrect outputs and nested lists",()=>{
    for(const mutate of [
      (value:WorkflowDefinition)=>{value.iterations!.loop.arguments.amount={source:"input",pointer:"/values/0"};},
      (value:WorkflowDefinition)=>{value.iterations!.loop.arguments.amount={source:"item",pointer:"/unknown"};},
      (value:WorkflowDefinition)=>{value.dataflow!.steps.loop.output={type:"boolean"};},
      (value:WorkflowDefinition)=>{value.iterations!.loop.definition=definition();},
      (value:WorkflowDefinition)=>{value.iterations!.loop.arguments={};},
    ]) {const value=definition();mutate(value);expect(validWorkflowEditor(value)).toBe(false);}
  });
  it("rejects cycles and source or role collisions without losing the source definition",()=>{
    const cycle=definition();cycle.iterations!.loop.dependencies=["after"];expect(validWorkflowEditor(cycle)).toBe(false);
    const bounds=definition();bounds.iterations!.loop.max_parallel=33;expect(validWorkflowEditor(bounds)).toBe(false);
    const mismatch=definition();mismatch.format_version=3;expect(validWorkflowEditor(mismatch)).toBe(false);
    const removed=removeWorkflowNode(cycle,"after");expect(removed.iterations!.loop.dependencies).toEqual([]);
    expect(removed.iterations!.loop.definition).toEqual(cycle.iterations!.loop.definition);
  });
  it("updates selected result types and retains downstream bindings for explicit repair",()=>{
    const value=definition(),next=changeIterationResult(value,"loop",{step_id:"echo",pointer:""});
    expect(iterationOutputType(next,"loop")).toEqual({type:"list",item:{type:"object",fields:{amount:{type:"integer"}}},max_items:1000});
    expect(next.dataflow!.steps.after.inputs.results).toEqual(value.dataflow!.steps.after.inputs.results);
    expect(validWorkflowEditor(next)).toBe(false);
  });
  it("checks nested capabilities and active item bounds independently of the parent budget",()=>{
    const value=definition();value.iterations!.loop.definition.rights.tools=["unsupported_tool"];
    expect(editableWorkflow(value)).toBe(false);
    const zero=definition();zero.iterations!.loop.max_parallel=0;expect(validWorkflowEditor(zero)).toBe(false);
    const whole=definition();whole.iterations!.loop.budgets.max_steps=whole.budgets.max_steps+1;
    expect(validWorkflowEditor(whole)).toBe(true);
  });
});
