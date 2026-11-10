import {describe,it,expect} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {decode} from "@cowork/client";
import fixture from "../../../tests/fixtures/workflow-control.json";
import {validWorkflowEditor} from "./workflowUi";
import {validWorkflowDataflow,reconcileWorkflowDataflow,workflowDataAncestors} from "./workflowDataflow";
import {validWorkflowControls,removeWorkflowNode} from "./workflowControls";
import {restoreWorkflowEditor,workflowEditorFormat} from "./workflowJournal";
import {WorkflowControlsEditor} from "./WorkflowControlsEditor";
import {WorkflowDataflowEditor,WorkflowDataflowSummary} from "./WorkflowDataflowEditor";

const definition=()=>decode("WorkflowDefinition",structuredClone(fixture));
describe("native workflow controls",()=>{
  it("accepts a complete editor definition with no roles or model steps",()=>{
    const value=definition();expect(validWorkflowEditor(value)).toBe(true);expect(validWorkflowDataflow(value)).toBe(true);expect(validWorkflowControls(value)).toBe(true);
    expect(workflowDataAncestors(value,"choice")).toEqual(new Set(["automatic","review","large"]));
    expect(reconcileWorkflowDataflow(value,value)).toEqual(value);
  });
  it("rejects missing gates, duplicate identifiers, cycles, wrong comparison types and merge types",()=>{
    const value=definition();value.gates!.review.step_id="missing";expect(validWorkflowEditor(value)).toBe(false);
    const cycle=definition();cycle.controls!.large.dependencies=["results"];expect(validWorkflowEditor(cycle)).toBe(false);
    const mismatch=definition();mismatch.dataflow!.steps.review.output={type:"text"};expect(validWorkflowEditor(mismatch)).toBe(false);
    const bad=definition();bad.controls!.large.operation={kind:"condition",predicate:{operator:"greater",input:"amount",value:"100"}};expect(validWorkflowEditor(bad)).toBe(false);
    const legacy=definition();legacy.format_version=2;expect(validWorkflowEditor(legacy)).toBe(false);
  });
  it("restores negative fractional comparisons and immutable control declarations exactly",()=>{
    const value=definition();value.controls!.large.operation={kind:"condition",predicate:{operator:"less",input:"amount",value:-42.5}};
    expect(restoreWorkflowEditor({editor_format:workflowEditorFormat,data:value,origin:null,archived:false})).toEqual(value);
  });
  it("renders editable conditions, branch gates and ordered result sources",()=>{
    const value=definition();const markup=renderToStaticMarkup(<WorkflowControlsEditor value={value} onChange={()=>{}}/>);
    expect(markup).toContain("Amount above 100");expect(markup).toContain("Needs review");expect(markup).toContain("Automatic route");
    expect(markup).toContain('value="greater" selected');expect(markup).toContain('value="100"');
    expect(markup).toMatch(/Ergebnisquellen|Result sources/);expect(markup).toMatch(/Bedingungsergebnis|Condition result/);
  });
  it("renders data bindings for control nodes and preserves the required typed declarations",()=>{
    const value=definition();const markup=renderToStaticMarkup(<WorkflowDataflowEditor value={value} onChange={()=>{}}/>);
    expect(markup).toContain("Amount above 100");expect(markup).toContain("Selected results");expect(markup).not.toContain('role="alert"');
    const summary=renderToStaticMarkup(<WorkflowDataflowSummary definition={value}/>);expect(summary).toContain("Chosen result");
  });
  it("removes ordering edges but retains dangling data and condition bindings for explicit repair",()=>{
    const value=definition(),removed=reconcileWorkflowDataflow(removeWorkflowNode(value,"large"),value);
    expect(removed.controls?.large).toBeUndefined();expect(removed.controls?.review.dependencies).toEqual([]);
    expect(removed.gates?.review.step_id).toBe("large");expect(validWorkflowEditor(removed)).toBe(false);
    const branch=reconcileWorkflowDataflow(removeWorkflowNode(value,"review"),value);
    expect(branch.gates?.review).toBeUndefined();expect(branch.controls?.choice.dependencies).toEqual(["automatic"]);
    expect(validWorkflowEditor(branch)).toBe(false);
  });

});
