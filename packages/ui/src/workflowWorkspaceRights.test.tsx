import {describe,it,expect} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {decode,type WorkflowDefinition} from "@cowork/client";
import fixture from "../../../tests/fixtures/workflow-iteration.json";
import {WorkflowWorkspaceRight} from "./WorkflowWorkspaceRight";
import {WorkflowDefinitionSummary} from "./WorkflowLibrary";
import {addWorkflowSubflow,intersectWorkflowRights} from "./workflowSubflows";
import {addWorkflowIteration} from "./workflowIterations";
import {restoreWorkflowEditor,workflowEditorFormat} from "./workflowJournal";
import {workflowDataError} from "./workflowDataflow";
import {validWorkflowEditor} from "./workflowUi";
import {de} from "./i18n";

const body=()=>structuredClone(decode("WorkflowDefinition",structuredClone(fixture)).iterations!.loop.definition);
describe("explicit current-workspace selections",()=>{
  it("copies only the intersection of explicit parent and frozen body selections",()=>{
    const parent=body(),source=body();parent.rights.current_workspace=true;source.rights.current_workspace=true;
    const subflow=addWorkflowSubflow(parent,source,{step_id:"echo",pointer:"/amount"},"call");
    expect(subflow.subflows!.call.rights.current_workspace).toBe(true);expect(validWorkflowEditor(subflow)).toBe(true);
    const list=addWorkflowIteration(parent,source,{step_id:"echo",pointer:"/amount"},"each",{value_type:{type:"list",item:{type:"integer"},max_items:4},value:{source:"literal",value:[1]}},{amount:{source:"item"}});
    expect(list.iterations!.each.rights.current_workspace).toBe(true);expect(validWorkflowEditor(list)).toBe(true);
    source.rights.current_workspace=false;expect(list.iterations!.each.definition.rights.current_workspace).toBe(true);
    expect(Object.hasOwn(intersectWorkflowRights(parent.rights,source.rights),"current_workspace")).toBe(false);
    expect(intersectWorkflowRights({tools:[],folders:["workspace:old"]},{current_workspace:true,tools:[],folders:[]})).toEqual({tools:[],folders:[]});
  });
  it("retains declared selectors through the existing draft journal and rejects a wrong selector type",()=>{
    const d=decode("WorkflowDefinition",structuredClone(fixture));d.rights.current_workspace=true;d.iterations!.loop.rights.current_workspace=true;d.iterations!.loop.definition.rights.current_workspace=true;
    expect(restoreWorkflowEditor({editor_format:workflowEditorFormat,data:d,origin:null,archived:false})).toEqual(d);
    expect(()=>decode("WorkflowDefinition",{...d,rights:{...d.rights,current_workspace:"true"}})).toThrow();
    expect(JSON.stringify(body())).not.toContain("current_workspace");
  });
  it("keeps unselected unavailable rights disabled and permits removal of an existing selection",()=>{
    const empty={tools:[],folders:[]};
    const blocked=renderToStaticMarkup(<WorkflowWorkspaceRight value={empty} available={false} onChange={()=>{}}/>);
    expect(blocked).toContain('disabled=""');expect(blocked).toContain("übergeordneten Rechten");
    const selected=renderToStaticMarkup(<WorkflowWorkspaceRight value={{...empty,current_workspace:true}} available={false} onChange={()=>{}}/>);
    expect(selected).toContain('checked=""');expect(selected).not.toContain('disabled=""');
  });
  it("shows the logical workspace selection in the frozen source review",()=>{
    const d:WorkflowDefinition=body();d.rights.current_workspace=true;
    const html=renderToStaticMarkup(<WorkflowDefinitionSummary definition={d} providers={[]} t={key=>de[key]}/>);
    expect(html).toContain("Ausgewählt: Arbeitsbereich des aktuellen Auftrags");expect(html).not.toContain("native-resource");
  });
  it("explains a missing or ambiguous current workspace without changing other errors",()=>{
    expect(workflowDataError(new Error("workflow_current_workspace_unavailable"),"de")).toContain("Planprüfung");
    expect(workflowDataError(new Error("workflow_current_workspace_ambiguous"),"en")).toContain("new plan");
    expect(workflowDataError(new Error("other-error"),"de")).toBe("other-error");
  });
});
