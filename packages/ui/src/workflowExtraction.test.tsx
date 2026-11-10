import {describe,expect,it,vi} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {writeFileSync} from "node:fs";
import {decode,defaultRunBudgets,type WorkflowDataType,type WorkflowReviewForm} from "@cowork/client";
import {createExtractionWorkflow,ExtractionDraftError,extractionSourceId,invoiceExtractionForm} from "./workflowExtraction";
import {WorkflowExtractionStarter} from "./WorkflowExtractionStarter";
import {validDataValue} from "./workflowDataflow";
import {reviewErrors,reviewOutput,reviewPrefillType} from "./workflowReviews";
import {validWorkflowEditor} from "./workflowUi";
import reference from "../../../tests/fixtures/workflow-document-extraction.json";

const profile=()=>decode("ProviderProfile",{id:"extraction-fixture",name:"Explicit fixture profile",kind:"openai_compatible",endpoint:"http://127.0.0.1:1",model:"extraction-fixture-model",revision:1,context_tokens:16384,credential_ready:true,credential_required:false});
const options=()=>({name:"Source-backed extraction fixture",language:"en",source:"ocr" as const,form:invoiceExtractionForm("en"),model:{id:"extraction-fixture",revision:1},providers:[profile()],budgets:{...defaultRunBudgets}});
const formOf=(definition:ReturnType<typeof createExtractionWorkflow>)=>{const operation=definition.controls!["review-values"].operation;if(operation.kind!=="review")throw new Error();return operation.form;};

describe("source-backed document extraction drafts",()=>{
  it("builds an editable three-step workflow with a real selected profile and native source evidence",()=>{
    const definition=createExtractionWorkflow(options());
    expect(definition).toEqual(reference);
    expect(decode("WorkflowDefinition",definition)).toEqual(definition);expect(validWorkflowEditor(definition)).toBe(true);
    expect(definition.roles).toHaveLength(1);expect(definition.roles[0].provider_id).toBe("extraction-fixture");expect(definition.roles[0].provider_revision).toBe(1);
    expect(definition.steps[0].dependencies).toEqual([extractionSourceId]);expect(definition.controls!["review-values"].dependencies).toEqual(["extract-values"]);
    expect(formOf(definition).source_steps).toEqual([extractionSourceId]);expect(definition.roles[0].output).toBeUndefined();
    expect(definition.dataflow!.steps["extract-values"].output).toEqual(reviewPrefillType(formOf(definition)));
    expect(JSON.stringify(definition)).not.toContain(profile().endpoint);
    if(process.env.COWORK_EXTRACTION_FIXTURE_OUT)writeFileSync(process.env.COWORK_EXTRACTION_FIXTURE_OUT,JSON.stringify(definition,null,2)+"\n",{encoding:"utf-8",flag:"wx"});
  });
  it("gives the extractor no tools, folders or delegation and binds only explicit native text fields",()=>{
    const definition=createExtractionWorkflow(options()),role=definition.roles[0],step=definition.steps[0];
    expect(role.rights).toEqual({tools:[],folders:[]});expect(step.rights).toEqual({tools:[],folders:[]});expect(role.delegate_to).toEqual([]);
    expect(definition.rights.tools).toEqual(["document_read","request_input"]);
    expect(definition.dataflow!.steps["extract-values"].context_policy).toBe("bound_inputs_only");
    const inputs=definition.dataflow!.steps["extract-values"].inputs;
    expect(Object.keys(inputs).sort()).toEqual(["document_text","next_offset","next_page","pages_truncated","source_sha256","source_truncated"]);
    expect(Object.values(inputs).every(binding=>binding.value.source==="step"&&binding.value.step_id===extractionSourceId)).toBe(true);
    expect(inputs.document_text.value).toEqual({source:"step",step_id:extractionSourceId,pointer:"/text"});
    expect(Object.keys(inputs)).not.toContain("words");expect(role.instructions).toContain("untrusted source data");
  });
  it("preserves unknown proposals as null and narrows required fields only after human confirmation",()=>{
    const definition=createExtractionWorkflow(options()),form=formOf(definition),unknown=Object.fromEntries(Object.keys(form.fields).map(name=>[name,null]));
    expect(validDataValue(definition.dataflow!.steps["extract-values"].output,unknown)).toBe(true);
    expect(reviewErrors(form,unknown)).toHaveLength(5);
    const output=reviewOutput(form);if(output.type!=="object")throw new Error();
    expect(output.fields.gross_minor).toEqual({type:"integer"});expect(output.fields.issued).toEqual({type:"text"});
    expect(validDataValue(output,unknown)).toBe(false);
    expect(definition.dataflow!.steps["review-values"].inputs.gross_minor.value).toEqual({source:"step",step_id:"extract-values",pointer:"/gross_minor"});
    expect(reviewErrors(form,{invoice:"R-42",issued:"2026-10-31",net_minor:10000,tax_minor:1900,gross_minor:11800})).toContainEqual({field:"gross_minor",code:"sum"});
  });
  it("supports existing text without OCR-only continuation fields or geometry in the model context",()=>{
    const definition=createExtractionWorkflow({...options(),source:"read_text"});expect(validWorkflowEditor(definition)).toBe(true);
    expect(definition.documents![extractionSourceId].operation.kind).toBe("read_text");
    expect(Object.keys(definition.dataflow!.steps["extract-values"].inputs).sort()).toEqual(["document_text","next_offset","source_sha256","source_truncated"]);
    expect(formOf(definition).source_steps).toEqual([extractionSourceId]);
  });
  it("builds arbitrary editable field forms rather than hard-coding invoice outputs",()=>{
    const input=options(),kind:WorkflowDataType={type:"text"};
    input.form={question:"Check the purchase order",source_steps:[extractionSourceId],checks:[],fields:{purchase_order:{label:"Purchase order",help:"Do not substitute an invoice number",format:"value",required:true,value_type:kind,minimum:null,maximum:null}}} satisfies WorkflowReviewForm;
    const definition=createExtractionWorkflow(input),form=formOf(definition);
    expect(form.fields.purchase_order.value_type).toEqual({type:"nullable",value:{type:"text"}});expect(input.form.fields.purchase_order.value_type).toEqual({type:"text"});
    expect(definition.dataflow!.steps["extract-values"].output).toEqual({type:"object",fields:{purchase_order:{type:"nullable",value:{type:"text"}}}});
    expect(definition.dataflow!.steps["review-values"].output).toEqual({type:"object",fields:{purchase_order:{type:"text"}}});
    expect(definition.roles[0].instructions).toContain("Do not substitute an invoice number");
  });
  it("rejects missing, stale or ambiguous model selections and invalid context limits",()=>{
    for(const patch of [{model:{id:"",revision:1}},{model:{id:"extraction-fixture",revision:2}},{model:{id:"extraction-fixture",revision:1.5}},{providers:[]},{providers:[profile(),profile()]},{providers:[{...profile(),context_tokens:0}]},{providers:[{...profile(),context_tokens:2_000_001}]}]){
      expect(()=>createExtractionWorkflow({...options(),...patch})).toThrow(ExtractionDraftError);
    }
  });
  it("rejects invalid forms and source substitutions before opening an executable definition",()=>{
    const badSource=options();badSource.form.source_steps=["extract-values"];expect(()=>createExtractionWorkflow(badSource)).toThrow(ExtractionDraftError);
    const badSum=options();badSum.form.checks=[{kind:"sum",terms:["net_minor","gross_minor"],total:"gross_minor"}];expect(()=>createExtractionWorkflow(badSum)).toThrow(ExtractionDraftError);
    const noName=options();noName.name=" ";expect(()=>createExtractionWorkflow(noName)).toThrow(ExtractionDraftError);
  });
  it("does not mutate the selected model, original form or requested limits",()=>{
    const input=options(),saved=structuredClone(input);createExtractionWorkflow(input);expect(input).toEqual(saved);
    const definition=createExtractionWorkflow(input);expect(definition.roles[0].budgets.max_total_tokens).toBeGreaterThanOrEqual(profile().context_tokens);
    expect(definition.roles[0].budgets.max_output_tokens).toBeLessThanOrEqual(profile().context_tokens/4);
  });
  it("keeps model selection explicit and opening a starter has no publication or run side effect",()=>{
    const create=vi.fn();for(const language of ["en","de"] as const){
      const html=renderToStaticMarkup(<WorkflowExtractionStarter providers={[profile()]} language={language} disabled={false} onCreate={create}/>);
      expect(html).toMatch(/Explicitly select a model|Modell ausdrücklich auswählen/);expect(html).toContain('<option value="" selected');
      expect(html).toMatch(/Open document workflow as a draft|Dokumentablauf als Entwurf öffnen/);expect(html).toMatch(/<button type="button" disabled/);
    }
    const html=renderToStaticMarkup(<WorkflowExtractionStarter providers={[]} language="en" disabled={false} onCreate={create}/>);
    expect(html).toContain("Configure a model");expect(create).not.toHaveBeenCalled();
  });
  it("shows business values first and keeps advanced field types in an initially closed disclosure",()=>{
    const html=renderToStaticMarkup(<WorkflowExtractionStarter providers={[profile()]} language="en" disabled={false} onCreate={()=>{}}/>);
    expect(html).toContain("Values to review: ");expect(html).toContain("Invoice number · Invoice date · Net amount · Tax amount · Total amount");
    expect(html).toContain('<details class="extraction-field-settings"><summary>Customize fields and review rules</summary>');
    expect(html).not.toContain('<details class="extraction-field-settings" open');expect(html).toContain("gross_minor");
  });
});
