import {describe,it,expect,vi} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {Client,decode,type WorkflowDefinition,type WorkflowReviewEvidence,type WorkflowReviewForm,type WorkflowReviewRequest} from "@cowork/client";
import reviewFixture from "../../../tests/fixtures/workflow-native-review.json";
import documentFixture from "../../../tests/fixtures/workflow-native-documents.json";
import ocrFixture from "../../../tests/fixtures/workflow-native-ocr-output.json";
import {reviewSourceOutput,validReviewEvidence,sourceWordMatches} from "./workflowReviewEvidence";
import {reviewAnswerValid,eligibleReviewSources} from "./workflowReviews";
import {validWorkflowControls,removeWorkflowNode} from "./workflowControls";
import {WorkflowReviewSources} from "./WorkflowReviewSources";
import {WorkflowReviewFormEditor} from "./WorkflowReviewForm";
import {verifiedPreviewUrl} from "./ToolPreviews";

const source={folder_id:"copy",path:"invoice.pdf",sha256:"a".repeat(64)};
const values={invoice:"INV-42",issued:"2026-11-01",net_minor:10000,tax_minor:1900,gross_minor:11900};
function form():WorkflowReviewForm {const op=decode("WorkflowDefinition",structuredClone(reviewFixture)).controls!.review.operation;if(op.kind!=="review")throw new Error();return {...op.form,source_steps:["source"]};}
function textEvidence():WorkflowReviewEvidence {return {step_id:"source",name:"Original invoice",kind:"text",output:{source,text:"Invoice INV-42. Total 119.00 EUR.",offset:0,next_offset:null,truncated:false,diagnostics:[],diagnostics_truncated:false}};}
function ocrEvidence():WorkflowReviewEvidence {return {step_id:"source",name:"Original scan",kind:"ocr",output:{...structuredClone(ocrFixture),source,diagnostics:[],diagnostics_truncated:false}};}
function request(evidence=ocrEvidence()):WorkflowReviewRequest {return {form:form(),values,evidence:[evidence]};}
function definition():WorkflowDefinition {
  const d=decode("WorkflowDefinition",structuredClone(reviewFixture)),docs=decode("WorkflowDefinition",structuredClone(documentFixture));d.rights={...d.rights,current_workspace:true,tools:["request_input","document_read"]};d.documents={source:{...docs.documents!["read-source"],rights:{current_workspace:true,tools:["document_read"],folders:[]},budgets:d.budgets}};
  d.controls!.review.dependencies=["source"];const op=d.controls!.review.operation;if(op.kind!=="review")throw new Error();op.form.source_steps=["source"];
  const data=structuredClone(docs.dataflow!.steps["read-source"]);data.inputs.path.value={source:"literal",value:"invoice.pdf"};data.inputs.sha.value={source:"literal",value:"a".repeat(64)};d.dataflow!.steps.source=data;return d;
}

describe("explicit original review evidence",()=>{
  it("allows only the selected native document ancestors and retains dangling selections for explicit repair",()=>{
    const d=definition();expect(eligibleReviewSources(d,"review")).toEqual([{id:"source",name:"Read selected source"}]);expect(validWorkflowControls(d)).toBe(true);
    const disconnected=structuredClone(d);disconnected.controls!.review.dependencies=[];expect(eligibleReviewSources(disconnected,"review")).toEqual([]);expect(validWorkflowControls(disconnected)).toBe(false);
    const removed=removeWorkflowNode(d,"source"),op=removed.controls!.review.operation;if(op.kind!=="review")throw new Error();expect(op.form.source_steps).toEqual(["source"]);expect(validWorkflowControls(removed)).toBe(false);
    const change=vi.fn(),markup=renderToStaticMarkup(<WorkflowReviewFormEditor form={op.form} sources={[]} onChange={change}/>);expect(markup).toMatch(/nicht mehr verbunden|no longer connected/);expect(markup).toContain('role="alert"');expect(change).not.toHaveBeenCalled();
  });
  it("binds the displayed source order and identity to the form and blocks preparing from missing or substituted evidence",()=>{
    const review=request();expect(validReviewEvidence(review)).toBe(true);expect(reviewAnswerValid(review,JSON.stringify(values))).toBe(true);
    for(const invalid of [{...review,evidence:[]},{...review,evidence:[{...review.evidence![0],step_id:"foreign"}]},{...review,form:{...review.form,source_steps:[]}},{...review,evidence:[{...textEvidence(),output:{...reviewSourceOutput(textEvidence()),new_right:"write_text_file"}}]}]){
      expect(validReviewEvidence(invalid)).toBe(false);expect(reviewAnswerValid(invalid,JSON.stringify(values))).toBe(false);
    }
    const second={...textEvidence(),step_id:"second"},ordered={...review,form:{...review.form,source_steps:["source","second"]},evidence:[review.evidence![0],second]};expect(validReviewEvidence(ordered)).toBe(true);expect(validReviewEvidence({...ordered,evidence:[second,review.evidence![0]]})).toBe(false);
  });
  it("keeps legacy forms with no selected sources compatible",()=>{
    const review=request();review.form.source_steps=[];review.evidence=[];expect(validReviewEvidence(review)).toBe(true);expect(reviewAnswerValid(review,JSON.stringify(values))).toBe(true);expect(renderToStaticMarkup(<WorkflowReviewSources client={new Client("synthetic")} run="run" review={review}/>)).toBe("");
  });
  it("keeps original text as escaped readonly data without mutating the proposal or initiating a download during rendering",()=>{
    const evidence=textEvidence();(evidence.output as Record<string,unknown>).text='<script>untrusted</script><img src="https://outside.invalid">';const review=request(evidence),before=JSON.stringify(review),fetch=vi.spyOn(globalThis,"fetch");
    try {const markup=renderToStaticMarkup(<WorkflowReviewSources client={new Client("synthetic")} run="run" review={review}/>);expect(markup).toContain("&lt;script&gt;");expect(markup).not.toContain("<script>");expect(markup).not.toContain('<img src="https://outside.invalid">');expect(markup).not.toContain("<textarea");expect(markup).toContain(source.path);expect(markup).toContain(source.sha256);expect(JSON.stringify(review)).toBe(before);expect(fetch).not.toHaveBeenCalled();}finally{fetch.mockRestore();}
  });
  it("validates original word boxes, hashes, score bounds and unique page/word identities before offering locations",()=>{
    const evidence=ocrEvidence(),parsed=reviewSourceOutput(evidence)!;expect(parsed.words!.length).toBeGreaterThan(0);expect(parsed.pages![0].page).toBe(1);
    for(const mutate of [(v:typeof parsed)=>v.words![0].x=v.pages![0].width,(v:typeof parsed)=>v.words![0].score=101,(v:typeof parsed)=>v.pages!.push({...v.pages![0]}),(v:typeof parsed)=>v.words!.push({...v.words![0]}),(v:typeof parsed)=>v.pages![0].preview_sha256='https://outside.invalid',(v:typeof parsed)=>v.source.sha256='wrong']){const value=structuredClone(parsed);mutate(value);expect(reviewSourceOutput({...evidence,output:value})).toBeNull();}
    expect(sourceWordMatches(parsed.words!,parsed.words![0].text.toUpperCase())).toContainEqual(parsed.words![0]);expect(sourceWordMatches(parsed.words!,"not-in-this-scan-999")).toEqual([]);
  });
  it("makes recognition excerpts explicit and retains original coordinates without inventing confidence probabilities",()=>{
    const evidence=ocrEvidence(),output=reviewSourceOutput(evidence)!;output.truncated=true;const markup=renderToStaticMarkup(<WorkflowReviewSources client={new Client("synthetic")} run="run" review={request({...evidence,output})}/>);
    expect(markup).toMatch(/Ausschnitt|excerpt/);expect(markup).toMatch(/keine Wahrscheinlichkeit|not a probability/);expect(markup).toContain(output.words![0].text);expect(markup).toMatch(/Quellnachweis|Source evidence/);
  });
  it("rejects aggregate source data beyond the backend bound instead of silently dropping source text",()=>{
    const large=reviewSourceOutput(textEvidence())!;large.text="x".repeat(8192);large.diagnostics=Array.from({length:64},()=>({severity:"info",message:"x".repeat(512),file:null,line:null}));
    const ids=["a","b","c","d"],review={...request(),form:{...form(),source_steps:ids},evidence:ids.map(id=>({...textEvidence(),step_id:id,output:structuredClone(large)}))};expect(validReviewEvidence(review)).toBe(false);expect(reviewAnswerValid(review,JSON.stringify(values))).toBe(false);
  });
  it("never creates a preview URL for bytes that differ from the saved preview digest",async()=>{
    const url=vi.spyOn(URL,"createObjectURL").mockReturnValue("blob:synthetic-preview");
    try {await expect(verifiedPreviewUrl(new Blob(["changed preview"]),"0".repeat(64))).rejects.toThrow("preview_integrity");expect(url).not.toHaveBeenCalled();}finally{url.mockRestore();}
  });
});
