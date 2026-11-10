import {afterEach,describe,expect,it} from "vitest";
import {readFileSync} from "node:fs";
import {renderToStaticMarkup} from "react-dom/server";
import type {WorkflowDefinition,WorkflowDocumentOperation} from "@cowork/client";
import {addDocumentStep,setDocumentOperation,validWorkflowDocuments,nativeDocumentWorkflow} from "./workflowDocuments";
import {documentOutputType} from "./workflowDocumentTypes";
import {workflowDataError,workflowDataAncestors,workflowDataNodes,reconcileWorkflowDataflow,validWorkflowDataflow,validDataValue} from "./workflowDataflow";
import {validWorkflowEditor} from "./workflowUi";
import {eligibleSubflowSource,subflowNodes,workflowTemplateLimits} from "./workflowSubflows";
import {removeWorkflowNode} from "./workflowControls";
import {WorkflowDocumentsEditor} from "./WorkflowDocumentsEditor";
import {WorkflowDataflowEditor} from "./WorkflowDataflowEditor";
import {useView} from "./view";

const originalLanguage=useView.getState().language;
afterEach(()=>useView.setState({language:originalLanguage}));
const fixture=():WorkflowDefinition=>JSON.parse(readFileSync(new URL("../../../tests/fixtures/workflow-native-documents.json",import.meta.url),"utf8"));
const ocr=():Extract<WorkflowDocumentOperation,{kind:"ocr"}>=>({kind:"ocr",path_input:"path",sha256_input:"sha",languages:["deu","eng"],pages:[],dpi:200,rotation:0,offset:0,max_words:100});

describe("native document workflow product editing",()=>{
  it("retains OCR source bindings and exposes the actual typed engine output",()=>{
    const before=fixture(),value=setDocumentOperation(before,"read-source",ocr());
    expect(validWorkflowDocuments(value)).toBe(true);
    expect(value.dataflow!.steps["read-source"].inputs).toEqual(before.dataflow!.steps["read-source"].inputs);
    expect(reconcileWorkflowDataflow(value,value)).toEqual(value);
    const output=JSON.parse(readFileSync(new URL("../../../tests/fixtures/workflow-native-ocr-output.json",import.meta.url),"utf8"));
    expect(validDataValue(documentOutputType(ocr()),output)).toBe(true);
    expect(output.text).toContain("119,00");
    const damaged=structuredClone(output);delete damaged.words[0].score;
    expect(validDataValue(documentOutputType(ocr()),damaged)).toBe(false);
    expect(before.documents!["read-source"].operation.kind).toBe("read_text");
  });
  it.each([
    {languages:[]},{languages:["fra"]},{languages:["deu","deu"]},{dpi:99},{dpi:401},{dpi:200.5},
    {rotation:45},{pages:[1,1]},{pages:[0]},{pages:Array.from({length:17},(_,i)=>i+1)},
    {offset:-1},{offset:NaN},{max_words:0},{max_words:101},
  ])("rejects unsupported or unbounded OCR settings: %j",changes=>{
    expect(validWorkflowDocuments(setDocumentOperation(fixture(),"read-source",{...ocr(),...changes}))).toBe(false);
  });
  it("renders editable OCR settings and explains scores, page limits and explicit continuation",()=>{
    useView.setState({language:"en"});const value=setDocumentOperation(fixture(),"read-source",ocr());
    const markup=renderToStaticMarkup(<WorkflowDocumentsEditor value={value} onChange={()=>{}}/>);
    expect(markup).toContain("Scans erkennen (OCR)");expect(markup).toContain("keine Wahrscheinlichkeit");
    expect(markup).toContain("acht Seiten");expect(markup).toContain("Lesen ab Wortposition");
    expect(markup).toContain("Deutsch + English");expect(markup).toContain('max="400"');
    const message=workflowDataError(new Error("workflow_document_ocr_unavailable"),"de");
    expect(message).not.toContain("workflow_document_ocr_unavailable");expect(message).toContain("OCR");
  });
  it("keeps a selected folder visible regardless of imported JSON property order",()=>{
    const value=fixture();value.rights.folders=["chosen-folder"];
    value.documents!["read-source"].folder=JSON.parse('{"folder_id":"chosen-folder","kind":"selected_folder"}');
    value.documents!["read-source"].rights={tools:["document_read"],folders:["chosen-folder"]};
    expect(validWorkflowEditor(value)).toBe(true);
    const markup=renderToStaticMarkup(<WorkflowDocumentsEditor value={value} onChange={()=>{}}/>);
    expect(markup).toContain('value="{&quot;kind&quot;:&quot;selected_folder&quot;,&quot;folder_id&quot;:&quot;chosen-folder&quot;}" selected=""');
  });
  it("explains document validation and unconfirmed outcomes without raw internal codes",()=>{
    for(const code of ["workflow_document_invalid","workflow_native_document_result_invalid"]){
      for(const language of ["de","en"] as const){const message=workflowDataError(new Error(code),language);expect(message).not.toContain(code);expect(message.length).toBeGreaterThan(30);}
    }
  });
  it("offers an editable document workflow without a provider or fingerprint input",()=>{
    const value=nativeDocumentWorkflow("Document workflow","Create report","# My report",fixture().budgets);
    expect(value.roles).toEqual([]);expect(value.steps).toEqual([]);expect(value.dataflow!.input).toEqual({type:"object",fields:{}});
    expect(validWorkflowEditor(value)).toBe(true);expect(value.dataflow!.steps["create-document"].inputs.content.value).toEqual({source:"literal",value:"# My report"});
    expect(value.rights.tools).toEqual(["document_write"]);
  });
  it("accepts the real Rust fixture without any model role or account",()=>{
    const value=fixture();expect(value.roles).toEqual([]);
    expect(validWorkflowDocuments(value)).toBe(true);expect(validWorkflowEditor(value)).toBe(true);
    expect(workflowDataNodes(value).map(node=>node.id)).toEqual(["read-source","write-report"]);
    expect(workflowDataAncestors(value,"write-report").has("read-source")).toBe(true);
    expect(subflowNodes(value).map(node=>node.id)).toEqual(["read-source","write-report"]);
    expect(eligibleSubflowSource(value)).toBe(true);
  });
  it("keeps every native action and its fixed data contract through editor reconciliation",()=>{
    const value=fixture(),before=structuredClone(value);
    expect(reconcileWorkflowDataflow(value,before)).toEqual(before);
    for(const [id,step] of Object.entries(value.documents!))expect(documentOutputType(step.operation)).toEqual(value.dataflow!.steps[id].output);
    expect(value).toEqual(before);
  });
  it("changes format while preserving explicit input bindings and existing parent rights",()=>{
    const value=fixture(),before=structuredClone(value);
    const changed=setDocumentOperation(value,"write-report",{kind:"create",format:"pdf",path_input:"path",content_input:"content"});
    expect(changed.rights).toEqual(value.rights);expect(changed.roles).toEqual([]);
    expect(changed.dataflow!.steps["write-report"].inputs.content).toEqual(value.dataflow!.steps["write-report"].inputs.content);
    expect(validWorkflowDocuments(changed)).toBe(false);
    changed.dataflow!.steps["write-report"].inputs.path.value={source:"literal",value:"report.pdf"};
    expect(validWorkflowEditor(changed)).toBe(true);expect(value).toEqual(before);
  });
  it("retains dangling data references for review after removing a source action",()=>{
    const value=fixture(),removed=removeWorkflowNode(value,"read-source"),next=reconcileWorkflowDataflow(removed,value);
    expect(next.documents!["read-source"]).toBeUndefined();expect(next.documents!["write-report"].dependencies).toEqual([]);
    expect(next.dataflow!.steps["read-source"]).toBeUndefined();
    expect(next.dataflow!.steps["write-report"].inputs.content).toEqual(value.dataflow!.steps["write-report"].inputs.content);
    expect(validWorkflowDataflow(next)).toBe(false);
  });
  it("counts native nodes against the frozen template and editor limits",()=>{
    const value=fixture(),step=value.documents!["read-source"],config=value.dataflow!.steps["read-source"];
    value.documents=Object.fromEntries(Array.from({length:64},(_,i)=>["doc-"+i,structuredClone(step)]));
    value.dataflow!.steps=Object.fromEntries(Object.keys(value.documents).map(id=>[id,structuredClone(config)]));
    expect(workflowTemplateLimits(value)).toBe(true);expect(eligibleSubflowSource(value)).toBe(true);
    expect(()=>addDocumentStep(value,"overflow",step.operation,"Extra")).toThrow();
    value.documents.extra=structuredClone(step);value.dataflow!.steps.extra=structuredClone(config);
    expect(workflowTemplateLimits(value)).toBe(false);
  });
  it("preserves implicit sequential ordering when adding an explicit document node",()=>{
    const value=fixture();value.documents={};value.format_version=2;value.mode={mode:"sequential"};
    value.steps=["first","second"].map(id=>({id,role_id:"writer",goal:id,dependencies:[],rights:{tools:[],folders:[]},budgets:structuredClone(value.budgets)}));
    value.dataflow!.steps=Object.fromEntries(value.steps.map(step=>[step.id,{inputs:{},output:{type:"object",fields:{result:{type:"text"}}}}]));
    const next=addDocumentStep(value,"read-file",{kind:"read_text",path_input:"path",sha256_input:"sha",offset:0},"Read file");
    expect(next.mode).toEqual({mode:"parallel",max_parallel:1});expect(next.steps[1].dependencies).toEqual(["first"]);
    expect(next.documents!["read-file"].dependencies).toEqual(["first","second"]);
    expect(value.steps[1].dependencies).toEqual([]);
  });
  it.each<WorkflowDocumentOperation>([
    {kind:"read_text",path_input:"path",sha256_input:"sha",offset:-1},
    {kind:"preview",path_input:"path",sha256_input:"sha",pages:[1,1]},
    {kind:"preview",path_input:"path",sha256_input:"sha",pages:[0]},
  ])("rejects an invalid native operation before saving: %j",operation=>{
    const value=setDocumentOperation(fixture(),"read-source",operation);expect(validWorkflowDocuments(value)).toBe(false);
  });
  it("requires declared source fingerprints, exact result fields and narrowed folder rights",()=>{
    const value=fixture();value.dataflow!.steps["read-source"].inputs.sha.value={source:"literal",value:"A".repeat(64)};
    expect(validWorkflowDocuments(value)).toBe(false);
    const rights=fixture();rights.rights.current_workspace=false;expect(validWorkflowDocuments(rights)).toBe(false);
    const output=fixture();output.dataflow!.steps["read-source"].output={type:"object",fields:{text:{type:"text"}}};expect(validWorkflowDocuments(output)).toBe(false);
    const selected=fixture();selected.documents!["read-source"].folder={kind:"selected_folder",folder_id:"foreign"};expect(validWorkflowDocuments(selected)).toBe(false);
  });
  it("renders native editing and fixed bindings without exposing a model setup requirement",()=>{
    useView.setState({language:"en"});const value=fixture();
    const markup=renderToStaticMarkup(<><WorkflowDocumentsEditor value={value} onChange={()=>{}}/><WorkflowDataflowEditor value={value} onChange={()=>{}}/></>);
    expect(markup).toContain("Dokumentaktionen ohne Modell");expect(markup).toContain("Benutzer- und Netzwerkrechte");
    expect(markup).toContain("Read selected source");expect(markup).toContain("Create a report");
    expect(markup).toContain("diagnostics_truncated");expect(markup).not.toContain("Ergebnisfelder dieses Schritts");
    expect(markup).not.toContain("Select model");
  });
});
