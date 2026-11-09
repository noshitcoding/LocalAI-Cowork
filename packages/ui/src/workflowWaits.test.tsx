import {describe,it,expect} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {Client,decode} from "@cowork/client";
import fixture from "../../../tests/fixtures/workflow-wait.json";
import {validWorkflowEditor} from "./workflowUi";
import {WorkflowControlsEditor} from "./WorkflowControlsEditor";
import {WorkflowRunPanel} from "./WorkflowRunPanel";
import {de} from "./i18n";
import {restoreWorkflowEditor,workflowEditorFormat} from "./workflowJournal";
const definition=()=>decode("WorkflowDefinition",structuredClone(fixture));
describe("durable timer editor and progress",()=>{
  it("accepts the published wait contract and preserves its draft verbatim",()=>{
    const value=definition();expect(validWorkflowEditor(value)).toBe(true);
    expect(restoreWorkflowEditor({editor_format:workflowEditorFormat,data:value,origin:null,archived:false})).toEqual(value);
  });
  it("rejects a wrong duration type, missing field and wrong scheduled output",()=>{
    for(const kind of ["type","field","output"]){const value=definition();if(kind==="type")value.dataflow!.steps.wait.inputs.seconds.value_type={type:"number"};if(kind==="field")value.controls!.wait.operation={kind:"wait",seconds_input:"missing"};if(kind==="output")value.dataflow!.steps.wait.output={type:"text"};expect(validWorkflowEditor(value)).toBe(false);}
  });
  it("requires a bounded integer for fixed durations and keeps the run time limit",()=>{
    for(const duration of [0,-1,1.5,86401,60,59,"2",null]){const value=definition();value.dataflow!.steps.wait.inputs.seconds.value={source:"literal",value:duration};expect(validWorkflowEditor(value)).toBe(false);}
    const value=definition();value.dataflow!.steps.wait.inputs.seconds.value={source:"literal",value:2};expect(validWorkflowEditor(value)).toBe(true);
  });
  it("renders scheduling and folder ownership explanations without a model selector",()=>{
    const markup=renderToStaticMarkup(<WorkflowControlsEditor value={definition()} onChange={()=>{}}/>);
    expect(markup).toContain("Warteschritt hinzufügen");expect(markup).toContain("Zeitbudget");expect(markup).toContain("Arbeitsordner");expect(markup).toContain("Eingabe für die Wartezeit");
  });
  it("shows a scheduled time, continuing work, and zero token consumption",()=>{
    const binding={workflow_id:"workflow",version_id:"version",definition_sha256:"a".repeat(64),plan_sha256:"b".repeat(64),inputs:{delay:2}};
    const value=decode("WorkflowRunView",{run:{id:"run",revision:3,last_sequence:4,state:"waiting_for_timer",created_at:1,updated_at:2,started_at:1,steps:1,usage:{},spec:{order_id:"order",project_id:"project",prompt:"Wait",provider:null,workflow:binding,access_mode:"read_only",executor:"local-v2",policy_revision:1,allowed_tools:[],budgets:definition().budgets,folders:[]}},binding,name:"Wait",version_revision:1,mode:{mode:"parallel",max_parallel:2},max_parallel:2,state:"waiting_timer",roles:[],nodes:[{id:"wait",goal:"Wait until scheduled time",dependencies:[],state:"waiting_timer",wake_at:1793300000,error_code:null,steps:1,charged_tokens:0}],approvals:[],inputs:[],input_resume_available:false,input_resume_blocked_reason:null,completion_resume_available:false,charged_tokens:0,steps_used:1});
    const markup=renderToStaticMarkup(<WorkflowRunPanel client={new Client("fixture")} changed={async()=>{}} view={value} t={key=>de[key]} busy={false} artifacts={[]} approve={()=>{}}/>);
    expect(markup).toContain("Andere Aufträge");expect(markup).toContain("Früheste Fortsetzung");expect(markup).toContain("Wartet bis zum Termin");expect(markup).not.toContain("Freigabe erforderlich");
    delete value.run.spec.folders;value.run.spec.access_mode="confirm_all";
    expect(()=>renderToStaticMarkup(<WorkflowRunPanel client={new Client("fixture")} changed={async()=>{}} view={value} t={key=>de[key]} busy={false} artifacts={[]} approve={()=>{}}/>)).not.toThrow();
  });
  it("rejects inherited property names in imported duration and gate references without throwing",()=>{
    const value=definition();value.controls!.wait.operation={kind:"wait",seconds_input:"constructor"};expect(()=>validWorkflowEditor(value)).not.toThrow();expect(validWorkflowEditor(value)).toBe(false);
    const gate=definition();gate.gates={after:{step_id:"constructor",when:true}};expect(()=>validWorkflowEditor(gate)).not.toThrow();expect(validWorkflowEditor(gate)).toBe(false);
  });

});
