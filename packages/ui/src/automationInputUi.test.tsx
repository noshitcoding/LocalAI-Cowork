import {describe,expect,it} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {QueryClient,QueryClientProvider} from "@tanstack/react-query";
import {Client,decode} from "@cowork/client";
import fixture from "../../../tests/fixtures/workflow-dataflow.json";
import {automationInputTargets,automationTargetConflict,eventFieldPointer,automationInputMessage} from "./automationInputUi";
import {AutomationWorkflowInputsEditor} from "./AutomationWorkflowInputsEditor";
import {workspaceManagementKey} from "./workspaceManagementApi";

describe("event-to-workflow input selection",()=>{
  it("offers nested fields, entire values and existing list positions with a bound",()=>{
    const targets=automationInputTargets({type:"object",fields:{invoice:{type:"object",fields:{total:{type:"number"}}},items:{type:"list",max_items:1000,item:{type:"text"}}}},{items:Array.from({length:1000},()=>"value")});
    expect(targets.some(item=>item.pointer==="/invoice/total")).toBe(true);expect(targets.some(item=>item.pointer==="/items")).toBe(true);expect(targets.some(item=>item.pointer==="/items/0")).toBe(true);expect(targets.length).toBeLessThanOrEqual(512);
    expect(automationInputTargets({type:"nullable",value:{type:"object",fields:{name:{type:"text"}}}},null).map(item=>item.pointer)).toEqual([""]);
  });
  it("escapes event keys and rejects overlapping target choices",()=>{
    expect(eventFieldPointer("a/b~c")).toBe("/a~1b~0c");
    expect(automationTargetConflict("/a/b",{"/a":"/input"})).toBe(true);
    expect(automationTargetConflict("/a",{"/a/b":"/input"})).toBe(true);
    expect(automationTargetConflict("",{"/a":"/input"})).toBe(true);
    expect(automationTargetConflict("/ab",{"/a":"/input"})).toBe(false);
    expect(automationTargetConflict("/a",{"/a":"/input"},"/a")).toBe(false);
    expect(automationInputMessage("automation_inputs_value_missing","de")).toContain("fehlt");
  });
  it("renders explicit mapping, source choices and saved fixed values from the exact version",()=>{
    const client=new Client("fixture","http://fixture.invalid"),cache=new QueryClient();
    const definition=decode("WorkflowDefinition",structuredClone(fixture));
    const task=decode("WorkspaceTask",{id:"task",project_id:"project",revision:1,archived:false,created_at:1,updated_at:1,value:{name:"Task",description:"",order_id:"order",prompt:"prompt",expected_output:"",execution:{kind:"workflow",workflow_id:"wf",version_id:"version",inputs:{path:"fixed.pdf",sha256:"hash",currency:"EUR",private_note:"fixed"}},access_mode:"read_only",budgets:definition.budgets,folders:[],native_tools:[],mcp_tools:[],agent_config:null,environment:null}});
    cache.setQueryData(["automation-workflow-input-schema",...workspaceManagementKey(client),"project","wf","version"],{project_id:"project",workflow_id:"wf",reference:{id:"version"},definition});
    const html=renderToStaticMarkup(<QueryClientProvider client={cache}><AutomationWorkflowInputsEditor client={client} project="project" task={task} source="file" value={{format_version:1,bindings:{"/path":"/path"}}} onChange={()=>{}} eventFields={[]}/></QueryClientProvider>);
    expect(html).toContain("Workflow-Eingabefeld");expect(html).toContain("Dateipfad");expect(html).toContain("Inhaltsfingerprint");expect(html).toContain("fixed.pdf");expect(html).toContain("nicht zusätzlich");cache.clear();
  });
});
