import {describe,expect,it} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {decode,type WorkflowDefinition} from "@cowork/client";
import fixture from "../../../tests/fixtures/workflow-dataflow.json";
import reviewFixture from "../../../tests/fixtures/workflow-native-review.json";
import {dataTypeAt,defaultDataValue,enableWorkflowDataflow,reconcileWorkflowDataflow,validDataType,validDataValue,validWorkflowDataflow,workflowInputForStart,workflowDataError} from "./workflowDataflow";
import {WorkflowDataflowEditor,WorkflowDataflowSummary,WorkflowDataValueEditor,WorkflowDataValuePreview} from "./WorkflowDataflowEditor";
import {restoreWorkflowEditor,workflowEditorFormat} from "./workflowJournal";
import {validWorkflowEditor} from "./workflowUi";
import {chatWorkflowSelection} from "./chatConfiguration";

const definition=()=>decode("WorkflowDefinition",structuredClone(fixture));
describe("typed workflow definitions and user input",()=>{
  it("preserves an explicit model context boundary in editable and published definitions",()=>{
    const value=definition();value.dataflow!.steps.extract.context_policy="bound_inputs_only";
    expect(validWorkflowDataflow(value)).toBe(true);
    const restored=restoreWorkflowEditor({editor_format:workflowEditorFormat,data:value,origin:null,archived:false});
    expect(restored?.dataflow?.steps.extract.context_policy).toBe("bound_inputs_only");
    const edited=reconcileWorkflowDataflow({...value,name:"Renamed draft"},value);
    expect(edited.dataflow!.steps.extract.context_policy).toBe("bound_inputs_only");
    const html=renderToStaticMarkup(<WorkflowDataflowEditor value={edited} onChange={()=>{}}/>);
    expect(html).toContain('<option value="bound_inputs_only" selected');
    expect(renderToStaticMarkup(<WorkflowDataflowSummary definition={edited}/>)).toContain("Nur verbundene Eingaben");
    expect(definition().dataflow!.steps.extract.context_policy).toBeUndefined();
  });
  it("rejects unknown context policies and a model context policy on a native review",()=>{
    const value=definition();(value.dataflow!.steps.extract as unknown as {context_policy:string}).context_policy="unknown";
    expect(validWorkflowDataflow(value)).toBe(false);expect(()=>decode("WorkflowDefinition",value)).toThrow();
    const review=decode("WorkflowDefinition",structuredClone(reviewFixture));review.dataflow!.steps.review.context_policy="bound_inputs_only";
    expect(validWorkflowDataflow(review)).toBe(false);
  });
  it("accepts real published fixture types and binds exact numbers without text coercion",()=>{
    const value=definition();expect(validWorkflowDataflow(value)).toBe(true);expect(validWorkflowEditor(value)).toBe(true);
    const input={path:"invoice.pdf",sha256:"a".repeat(64),currency:"EUR",private_note:"not bound"};
    const frozen=workflowInputForStart(value,input);expect(frozen).toEqual(input);input.currency="USD";expect((frozen as typeof input).currency).toBe("EUR");
    expect(()=>workflowInputForStart(value,{...input,path:42})).toThrow("workflow_data_type_mismatch");
    expect(()=>workflowInputForStart(value,{...input,extra:true})).toThrow();
  });
  it("rejects wrong references, changed source types and dependencies removed by the editor",()=>{
    const value=definition();value.mode={mode:"parallel",max_parallel:2};value.steps[1].dependencies=[];expect(validWorkflowDataflow(value)).toBe(false);
    const typed=definition();typed.dataflow!.steps.extract.output={type:"object",fields:{invoice_id:{type:"text"},total:{type:"text"}}};expect(validWorkflowDataflow(typed)).toBe(false);
    expect(dataTypeAt({type:"list",item:{type:"text"},max_items:2},"/01")).toBeUndefined();
    expect(dataTypeAt({type:"list",item:{type:"text"},max_items:2},"/2")).toBeUndefined();
    expect(dataTypeAt({type:"object",fields:{}},"/toString")).toBeUndefined();
  });
  it("preserves negative fractional constants exactly across draft restore",()=>{
    const value=definition();value.dataflow!.steps.extract.inputs.amount={value_type:{type:"number"},value:{source:"literal",value:-42.5}};
    const restored=restoreWorkflowEditor({editor_format:workflowEditorFormat,data:value,origin:null,archived:false});expect(restored).toEqual(value);
    expect(restored?.dataflow?.steps.extract.inputs.amount.value).toEqual({source:"literal",value:-42.5});
  });
  it("keeps declarations when adding, removing and changing step execution",()=>{
    const value=definition();const next:WorkflowDefinition={...value,steps:[...value.steps,{...value.steps[0],id:"third"}]};
    const reconciled=reconcileWorkflowDataflow(next,value);expect(Object.keys(reconciled.dataflow!.steps)).toEqual(["extract","report","third"]);
    expect(reconciled.dataflow!.steps.third.output.type).toBe("object");
    const web=reconcileWorkflowDataflow({...value,steps:value.steps.map((step,index)=>index===0?{...step,execution:{kind:"web_fetch",url:"https://example.invalid"}}:step)},value);
    expect(web.dataflow!.steps.extract).toEqual({inputs:{},output:{type:"text"}});
    const removed=reconcileWorkflowDataflow({...value,steps:[value.steps[0]]},value);expect(Object.keys(removed.dataflow!.steps)).toEqual(["extract"]);
  });
  it("enables typed editing only in a new draft without rewriting the source definition",()=>{
    const value=definition();value.format_version=1;delete value.dataflow;value.roles[0].output={name:"old",schema:{type:"object"}};
    const before=JSON.stringify(value),typed=enableWorkflowDataflow(value);expect(JSON.stringify(value)).toBe(before);expect(typed.format_version).toBe(2);expect(typed.roles[0].output).toBeUndefined();expect(validWorkflowDataflow(typed)).toBe(true);
    expect(workflowInputForStart(value,undefined)).toBeUndefined();
  });
  it("bounds lists, nesting, Unicode text and safe integers consistently",()=>{
    const kind=decode("WorkflowDataType",{type:"object",fields:{count:{type:"integer"},values:{type:"list",item:{type:"nullable",value:{type:"text"}},max_items:2}}});
    expect(validDataValue(kind,{count:1,values:["text",null]})).toBe(true);
    expect(validDataValue(kind,{count:Number.MAX_SAFE_INTEGER+1,values:[]})).toBe(false);
    expect(validDataValue(kind,{count:1.5,values:[]})).toBe(false);
    expect(validDataValue(kind,{count:1,values:[null,null,null]})).toBe(false);
    expect(validDataValue({type:"text"},"😀".repeat(16384))).toBe(true);
    expect(validDataType({type:"list",item:{type:"text"},max_items:0})).toBe(false);
  });
  it("retains copied typed inputs in a saved chat selection",()=>{
    const input={amount:-42.5,nullable:null},selection=chatWorkflowSelection(JSON.stringify(["workflow","version"]),input);
    input.amount=0;expect(selection?.inputs).toEqual({amount:-42.5,nullable:null});expect(decode("ChatWorkflowSelection",selection)).toEqual(selection);
  });
  it("shows field controls, mapped inputs and a readable frozen value preview",()=>{
    const value=definition(),editor=renderToStaticMarkup(<WorkflowDataflowEditor value={value} onChange={()=>{}}/>);
    expect(editor).toContain("/total");expect(editor).toContain("Workflow-Eingabefelder");expect(editor).not.toContain("&quot;format_version&quot;");
    const fields=renderToStaticMarkup(<WorkflowDataValueEditor label="Inputs" kind={value.dataflow!.input} value={defaultDataValue(value.dataflow!.input)} onChange={()=>{}}/>);expect(fields).toContain("path");expect(fields).toContain("currency");
    const preview=renderToStaticMarkup(<WorkflowDataValuePreview value={{amount:42.5,items:["invoice.pdf",null]}}/>);expect(preview).toContain("42.5");expect(preview).toContain("invoice.pdf");expect(preview).not.toContain("[object Object]");
    expect(workflowDataError(new Error("workflow_data_type_mismatch"),"de")).toContain("keine automatische Umwandlung");
  });
});
