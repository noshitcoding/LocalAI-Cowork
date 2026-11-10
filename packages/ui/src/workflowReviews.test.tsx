import {describe,it,expect,vi} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {decode,defaultRunBudgets,type WorkflowReviewForm} from "@cowork/client";
import fixture from "../../../tests/fixtures/workflow-native-review.json";
import {WorkflowReviewAnswerEditor,WorkflowReviewAnswerPreview,WorkflowReviewFormEditor} from "./WorkflowReviewForm";
import {WorkflowControlsEditor} from "./WorkflowControlsEditor";
import {validWorkflowEditor} from "./workflowUi";
import {validWorkflowControls} from "./workflowControls";
import {formatMoneyMinor,parseMoneyMinor,reviewErrors,reviewAnswerValid,reviewOutput,reviewPrefillType,validReviewForm,nativeReviewWorkflow,patchReviewForm} from "./workflowReviews";

const definition=()=>decode("WorkflowDefinition",structuredClone(fixture));
function form():WorkflowReviewForm {const op=definition().controls!.review.operation;if(op.kind!=="review")throw new Error("review fixture");return op.form;}
const values=()=>({invoice:"INV-42",issued:"2026-11-01",net_minor:10000,tax_minor:1900,gross_minor:11900});

describe("native typed review",()=>{
  it("publishes editable forms without a model role and binds the fixed typed response",()=>{
    const d=definition();expect(validWorkflowEditor(d)).toBe(true);expect(d.roles).toEqual([]);expect(d.steps).toEqual([]);
    const fresh=nativeReviewWorkflow("Review without a model","en",defaultRunBudgets);expect(validWorkflowEditor(fresh)).toBe(true);expect(fresh.roles).toEqual([]);
    const id=Object.keys(fresh.controls!)[0],updated=patchReviewForm(fresh,id,form());expect(updated.dataflow!.steps[id].output).toEqual(reviewOutput(form()));expect(validWorkflowEditor(updated)).toBe(true);expect(Object.keys(updated.dataflow!.steps[id].inputs)).toHaveLength(5);
    expect(fresh.dataflow!.steps[id].inputs).toHaveProperty("value");
  });
  it("rejects missing or expanded human-input rights before publication",()=>{
    const missing=definition();missing.rights.tools=[];expect(validWorkflowControls(missing)).toBe(false);
    const expanded=definition(),op=expanded.controls!.review.operation;if(op.kind!=="review")throw new Error();op.rights.tools.push("write_text_file");expect(validWorkflowControls(expanded)).toBe(false);
    const mismatch=definition();mismatch.dataflow!.steps.review.inputs.gross_minor.value_type={type:"text"};expect(validWorkflowControls(mismatch)).toBe(false);
  });
  it("requires actual correction rather than treating a well-formed JSON object as approval",()=>{
    const review={form:form(),values:values()};expect(reviewAnswerValid(review,JSON.stringify(values()))).toBe(true);
    for(const bad of [{...values(),invoice:"   "},{...values(),gross_minor:1},{...values(),gross_minor:119.00},{...values(),net_minor:-1},{...values(),issued:"2026-02-29"},{...values(),issued:"1900-02-29"},{...values(),issued:"0000-01-01"},{...values(),issued:"2026-1-01"},{...values(),new_rights:true}])expect(reviewAnswerValid(review,JSON.stringify(bad)),JSON.stringify(bad)).toBe(false);
    expect(reviewAnswerValid(review,JSON.stringify({...values(),issued:"2000-02-29"}))).toBe(true);expect(reviewAnswerValid(review,"{" )).toBe(false);
    expect(reviewErrors(form(),{...values(),gross_minor:11901})).toContainEqual({field:"gross_minor",code:"sum"});
  });
  it("converts decimal input to exact cents with no rounding, including safe-integer boundaries",()=>{
    for(const [text,cents] of [["119,01",11901],["119.01",11901],["119",11900],["-0.01",-1],["90071992547409.91",Number.MAX_SAFE_INTEGER],["-90071992547409.91",Number.MIN_SAFE_INTEGER]] as const){expect(parseMoneyMinor(text)).toBe(cents);expect(parseMoneyMinor(formatMoneyMinor(cents))).toBe(cents);}
    for(const text of ["119.001","119,","1.234,56","90071992547409.92","", "9".repeat(129)])expect(parseMoneyMinor(text)).toBe(text);
  });
  it("renders date and money controls, readable labels and per-field correction errors",()=>{
    const change=vi.fn(),review={form:form(),values:values()};const markup=renderToStaticMarkup(<WorkflowReviewAnswerEditor review={review} answer={JSON.stringify({...values(),gross_minor:11901})} disabled={false} onChange={change}/>);
    expect(markup).toContain('type="date"');expect(markup).toContain('inputMode="decimal"');expect(markup).toContain('value="119.01"');expect(markup).toContain("Invoice number");expect(markup).toMatch(/Summe|total/);expect(markup).not.toContain("<textarea>{");expect(change).not.toHaveBeenCalled();
    const frozen=renderToStaticMarkup(<WorkflowReviewAnswerPreview form={form()} answer={JSON.stringify({...values(),invoice:"<script>untrusted</script>"})}/>);expect(frozen).toContain("Gross amount");expect(frozen).toContain("119.00");expect(frozen).toContain("&lt;script&gt;");expect(frozen).not.toContain("<script>");
  });
  it("keeps unreadable saved drafts visible and offers explicit recovery without sending",()=>{
    const change=vi.fn(),markup=renderToStaticMarkup(<WorkflowReviewAnswerEditor review={{form:form(),values:values()}} answer="{unfinished" disabled={true} onChange={change}/>);
    expect(markup).toContain("{unfinished");expect(markup).toContain('disabled=""');expect(markup).toMatch(/ursprünglichen Vorschlägen|original suggestions/);expect(change).not.toHaveBeenCalled();
  });
  it("opens missing required values directly without a null switch or implicit defaults",()=>{
    const review={form:form(),values:{invoice:null,issued:null,net_minor:null,tax_minor:null,gross_minor:null}},change=vi.fn();
    const markup=renderToStaticMarkup(<WorkflowReviewAnswerEditor review={review} answer={JSON.stringify(review.values)} disabled={false} onChange={change}/>);
    expect(markup).toContain('type="date"');expect(markup.match(/inputMode="decimal"/g)).toHaveLength(3);expect(markup).toContain('<textarea');
    expect(markup).not.toContain('type="checkbox"');expect(markup).not.toContain('value="0.00"');expect(change).not.toHaveBeenCalled();
    expect(reviewErrors(review.form,review.values)).toHaveLength(5);
  });
  it("requires an explicit boolean choice while optional missing values keep their null switch",()=>{
    const review={form:{question:"Confirm",checks:[],fields:{confirmed:{label:"Confirmed",help:"",value_type:{type:"nullable",value:{type:"boolean"}},format:"value",required:true,minimum:null,maximum:null},optional:{label:"Optional date",help:"",value_type:{type:"nullable",value:{type:"text"}},format:"date",required:false,minimum:null,maximum:null}}} satisfies WorkflowReviewForm,values:{confirmed:null,optional:null}},change=vi.fn();
    const markup=renderToStaticMarkup(<WorkflowReviewAnswerEditor review={review} answer={JSON.stringify(review.values)} disabled={false} onChange={change}/>);
    expect(markup).toMatch(/<option value="" disabled="" selected="">/);expect(markup).not.toMatch(/<option value="false" selected/);expect(markup.match(/type="checkbox"/g)).toHaveLength(1);
    expect(reviewAnswerValid(review,JSON.stringify(review.values))).toBe(false);expect(reviewAnswerValid(review,JSON.stringify({confirmed:false,optional:null}))).toBe(true);expect(change).not.toHaveBeenCalled();
  });
  it("renders field definitions and bounded sum rules in the workflow editor",()=>{
    expect(validReviewForm(form())).toBe(true);const change=vi.fn(),markup=renderToStaticMarkup(<WorkflowReviewFormEditor form={form()} onChange={change}/>);
    expect(markup).toContain("Invoice number");expect(markup).toContain("gross_minor");expect(markup).toMatch(/Summenregel|Total rule/);expect(markup).toContain('value="money_minor" selected');expect(change).not.toHaveBeenCalled();
    expect(renderToStaticMarkup(<WorkflowControlsEditor value={definition()} onChange={change}/>)).toMatch(/Prüfformular hinzufügen|Add review form/);
  });
  it("decodes native questions without a fabricated role while preserving legacy plain questions",()=>{
    const plain={request_id:"request",fingerprint:"a".repeat(64),node_id:"review",question:"Review these fields",state:"waiting_answer",expires_at:2_000_000_000_000,actionable:true,receipt:null,closed_reason:null};
    expect(decode("WorkflowInputView",{...plain,review:{form:form(),values:values()}}).role_id).toBeUndefined();
    expect(decode("WorkflowInputView",{...plain,role_id:"Author"}).role_id).toBe("Author");
  });
  it("accepts missing extraction values as prefill and exposes required corrected outputs without null",()=>{
    const review={form:form(),values:{invoice:null,issued:null,net_minor:null,tax_minor:null,gross_minor:null}};
    expect(validReviewForm(review.form)).toBe(true);expect(reviewPrefillType(review.form)).toEqual(definition().dataflow!.input);
    const output=reviewOutput(review.form);expect(output.type).toBe("object");if(output.type!=="object")throw new Error();expect(output.fields.gross_minor).toEqual({type:"integer"});expect(output.fields.issued).toEqual({type:"text"});
    expect(reviewAnswerValid(review,JSON.stringify(review.values))).toBe(false);expect(reviewErrors(review.form,review.values)).toHaveLength(5);expect(reviewAnswerValid(review,JSON.stringify(values()))).toBe(true);
    const optional=structuredClone(review.form);optional.fields.gross_minor.required=false;expect(validReviewForm(optional)).toBe(false);
  });
});
