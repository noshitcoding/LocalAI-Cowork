import {describe,expect,it} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {QueryClient,QueryClientProvider} from "@tanstack/react-query";
import {Client,decode,defaultRunBudgets,type WorkflowRunView} from "@cowork/client";
import {WorkflowRunPanel,workflowCanResume} from "./WorkflowRunPanel";
import {de,en,type Key} from "./i18n";
function view():WorkflowRunView {
  const binding={workflow_id:"technical-workflow",version_id:"technical-version",definition_sha256:"a".repeat(64),plan_sha256:"b".repeat(64)};
  const provider={id:"technical-provider",revision:1,name:"Local model",kind:"ollama",endpoint:"http://127.0.0.1:11434",model:"local-qwen",context_tokens:8192,credential_ready:true,credential_required:false};
  const run={id:"technical-run",revision:5,last_sequence:8,state:"running",created_at:1,updated_at:2,steps:3,usage:{},spec:{order_id:"order",project_id:"project",prompt:"Research",provider:null,workflow:binding,access_mode:"confirm_mutations",executor:"local-v2",policy_revision:1,allowed_tools:["create_artifact"],budgets:{...defaultRunBudgets},folders:[]}};
  return decode("WorkflowRunView",{run,binding,name:"Research and review",version_revision:2,mode:{mode:"parallel",max_parallel:2},max_parallel:2,state:"running",roles:[{role_id:"Research",provider},{role_id:"Review",provider}],nodes:[{id:"technical-node-a",role_id:"Research",goal:"Create a cited report",dependencies:[],state:"waiting_approval",error_code:null,steps:2,charged_tokens:42},{id:"technical-node-b",role_id:"Review",goal:"Check conclusions",dependencies:[],state:"running",error_code:null,steps:1,charged_tokens:23}],approvals:[{node_id:"technical-node-a",role_id:"Research",approval:{id:"approval-a",run_id:run.id,policy_revision:1,target:"local-v2",fingerprint:"c".repeat(64),decision:null,tool:{id:"raw-call",name:"create_artifact",arguments:{name:"Report.txt",content:"Exact proposed content"}}}}],inputs:[],input_resume_available:false,input_resume_blocked_reason:null,completion_resume_available:false,charged_tokens:65,steps_used:3});
}
describe("workflow run presentation",()=>{
  it("identifies nested approvals by their call and step while retaining technical role IDs in diagnostics",()=>{
    const value=view(),technical="composed-"+"a".repeat(40);value.nodes[0].role_id=technical;value.nodes[0].scope_id="invoice-call";value.roles[0].role_id=technical;value.approvals[0].role_id=technical;
    value.nodes.push({id:"invoice-call",kind:"subflow",goal:"Invoice checks",dependencies:[value.nodes[0].id],state:"waiting_approval",steps:0,charged_tokens:0});
    const markup=renderToStaticMarkup(<WorkflowRunPanel client={new Client("fixture")} changed={async()=>{}} view={value} t={key=>en[key]} busy={false} artifacts={[]} approve={()=>{}}/>);
    expect(markup.replace(/<details>[\s\S]*?<\/details>/g,"")).not.toContain(technical);expect(markup).toContain("Invoice checks");expect(markup).toContain("Exact proposed content");expect(markup).toContain(technical);
  });
  it("renders exact approval and independent role state while the parent remains running",()=>{
    const value=view();expect(value.run.state).toBe("running");
    const markup=renderToStaticMarkup(<WorkflowRunPanel client={new Client("fixture")} changed={async()=>{}} view={value} t={key=>en[key]} busy={false} artifacts={[]} approve={()=>{}}/>);
    expect(markup).toContain("Approval for role");expect(markup).toContain("Research");expect(markup).toContain("Review");expect(markup).toContain("Running");expect(markup).toContain("Report.txt");expect(markup).toContain("Exact proposed content");expect(markup).toContain("text/plain; charset=utf-8");
    expect(markup.replace(/<details>[\s\S]*?<\/details>/g,"")).not.toContain("technical-node-a");
    expect(markup).not.toContain("File action without model");
  });
  it("resolves a read approval’s result name from the actual id argument",()=>{
    const value=view();value.approvals[0].approval.tool={id:"raw-read",name:"read_artifact",arguments:{id:"artifact-original"}};
    const artifact={id:"artifact-original",order_id:"order",run_id:"older-run",name:"Confirmed report.txt",content:"Confirmed result",media_type:"text/plain; charset=utf-8",size:16,sha256:"d".repeat(64),created_at:1};
    const markup=renderToStaticMarkup(<WorkflowRunPanel client={new Client("fixture")} changed={async()=>{}} view={value} t={key=>en[key]} busy={false} artifacts={[artifact]} approve={()=>{}}/>);
    expect(markup.replace(/<details>[\s\S]*?<\/details>/g,"")).toContain("Confirmed report.txt");
  });
  it("renders the shared default token limit when Rust omits its default field",()=>{
    const value=view();const explicit=renderToStaticMarkup(<WorkflowRunPanel client={new Client("fixture")} changed={async()=>{}} view={value} t={key=>en[key]} busy={false} artifacts={[]} approve={()=>{}}/>);
    delete value.run.spec.budgets.max_total_tokens;
    const omitted=renderToStaticMarkup(<WorkflowRunPanel client={new Client("fixture")} changed={async()=>{}} view={value} t={key=>en[key]} busy={false} artifacts={[]} approve={()=>{}}/>);
    expect(omitted).toBe(explicit);
  });
  it("does not enable generic re-execution of an interrupted graph",()=>{
    const value=view();value.run.state="interrupted";value.state="interrupted";value.approvals=[];
    expect(workflowCanResume(value)).toBe(false);
    expect(renderToStaticMarkup(<WorkflowRunPanel client={new Client("fixture")} changed={async()=>{}} view={value} t={key=>de[key]} busy={false} artifacts={[]} approve={()=>{}}/>)).toContain(de.workflowResumeUnavailable);
    value.completion_resume_available=true;expect(workflowCanResume(value)).toBe(true);
    expect(workflowCanResume(undefined)).toBe(false);
  });
  it("uses complete shared German and English keys for every displayed node status",()=>{
    for(const state of ["pending","ready","running","waiting_approval","waiting_input","completed","failed","blocked","interrupted"] as const){
      const value=view();value.nodes[0].state=state;
      for(const language of [de,en]){const translated:Key[]=[];const markup=renderToStaticMarkup(<WorkflowRunPanel client={new Client("fixture")} changed={async()=>{}} view={value} t={key=>{translated.push(key);return language[key];}} busy={true} artifacts={[]} approve={()=>{}}/>);expect(markup).not.toContain("undefined");for(const key of translated)expect(language[key].trim()).not.toBe("");}
    }
  });
});


describe("native document approval presentation",()=>{
  it("accepts an approval without a model role and shows its exact action",()=>{
    const value=view();value.name="Native document report";value.roles=[];value.nodes=[{...value.nodes[0],role_id:undefined,goal:"Create invoice PDF",state:"waiting_approval"}];
    delete (value.approvals[0] as Partial<WorkflowRunView["approvals"][number]>).role_id;
    value.approvals[0].approval.tool={id:"native",name:"document_write",arguments:{operation:"create_pdf",output:{path:"invoice.pdf"},spec:{markdown:"Bound report"}}};
    expect(decode("WorkflowRunView",value)).toEqual(value);
    const markup=renderToStaticMarkup(<WorkflowRunPanel client={new Client("fixture")} changed={async()=>{}} view={value} t={key=>en[key]} busy={false} artifacts={[]} approve={()=>{}}/>);
    const visible=markup.replace(/<details>[\s\S]*?<\/details>/g,"");
    expect(visible).toContain("Create invoice PDF");expect(visible).toContain("invoice.pdf");expect(visible).not.toContain("Approval for role");
    expect(visible).not.toContain("Research");expect(visible).not.toContain("undefined");
  });
});

describe("workflow clock status presentation",()=>{
  const codes=["workflow_clock_unverified","workflow_clock_rollback"] as const;
  function render(value:WorkflowRunView,language:typeof de|typeof en){return renderToStaticMarkup(<QueryClientProvider client={new QueryClient({defaultOptions:{queries:{retry:false}}})}><WorkflowRunPanel client={new Client("fixture")} changed={async()=>{}} view={value} t={key=>language[key]} busy={false} artifacts={[]} approve={()=>{}}/></QueryClientProvider>);}
  function closedQuestion(code:string):NonNullable<WorkflowRunView["inputs"]>[number]{return {request_id:"question",fingerprint:"e".repeat(64),node_id:"technical-node-a",role_id:"Research",question:"Which source should be preserved?",state:"closed",expires_at:3,actionable:false,receipt:null,closed_reason:code};}
  for(const code of codes)for(const [locale,language] of [["de",de],["en",en]] as const){
    it(`explains a sticky queued ${code} in ${locale} without replacing the historical nodes`,()=>{
      const value=view();value.run.state="queued";value.state="queued";value.run.error_code=code;value.approvals=[];
      const markup=render(value,language);
      expect(markup).toContain(`<p class="banner" role="status">${language[code]}</p>`);
      expect(markup).toContain("Create a cited report");expect(markup).toContain("Check conclusions");
      expect(markup.replace(/<details>[\s\S]*?<\/details>/g,"")).not.toContain(code);
      expect(markup).not.toContain("<button");expect(workflowCanResume(value)).toBe(false);
    });
  }
  for(const code of codes)it(`explains server-closed input ${code} even when the parent has no error code`,()=>{
    const value=view();value.approvals=[];value.inputs=[closedQuestion(code)];
    expect(value.run.error_code).toBeUndefined();
    for(const language of [de,en]){const markup=render(value,language);expect(markup).toContain(language[code]);expect(markup).toContain("Which source should be preserved?");expect(markup).toContain(language.workflowInputClosed);}
  });
  it("leaves unrelated errors and inputs without a confirmed clock closure unaffected",()=>{
    const value=view();value.run.error_code="workflow_clock_rollback_other";value.approvals=[];
    const input=closedQuestion("interrupted");value.inputs=[input];
    for(const language of [de,en]){const markup=render(value,language);for(const code of codes)expect(markup).not.toContain(language[code]);expect(markup).toContain(language.workflowInputInterruptedDraft);}
    input.state="waiting_answer";input.closed_reason="workflow_clock_rollback";
    const markup=render(value,en);for(const code of codes)expect(markup).not.toContain(en[code]);
    delete value.inputs;const withoutInputs=render(value,en);for(const code of codes)expect(withoutInputs).not.toContain(en[code]);expect(withoutInputs).toContain("Create a cited report");
  });
});


describe("workflow document approvals",()=>{
  it("shows the actual document source, SHA and new output instead of the artifact explanation",()=>{
    const value=view();value.run.spec.folders=[{id:"source",project_id:"project",name:"Reviewed files",resource_id:"native-resource",revision:"1",writable:true}];
    value.approvals[0].approval.tool={id:"document-call",name:"document_write",arguments:{operation:"edit",inputs:[{folder_id:"source",path:"invoice.docx",expected_sha256:"f".repeat(64)}],output:{folder_id:"source",path:"checked-invoice.docx"},spec:{blocks:[]}}};
    for(const language of [de,en]){
      const html=renderToStaticMarkup(<WorkflowRunPanel client={new Client("fixture")} changed={async()=>{}} view={value} t={key=>language[key]} busy={false} artifacts={[]} approve={()=>{}}/>);
      const visible=html.replace(/<details>[\s\S]*?<\/details>/g,"");
      expect(visible).toContain(language.document_write);expect(visible).toContain("invoice.docx");expect(visible).toContain("checked-invoice.docx");expect(visible).toContain("Reviewed files");expect(visible).toContain("ffffffff");expect(visible).toContain(language.documentNoOverwrite);expect(visible).not.toContain(language.workflowArtifactScope);
    }
  });
  it("explains image steps on the same approval card without another execution route",()=>{
    const value=view();value.approvals[0].approval.tool={id:"image-call",name:"image_edit",arguments:{input:{folder_id:"source",path:"scan.png"},output:{folder_id:"source",path:"scan-gray.png"},steps:[{op:"grayscale"},{op:"rotate",degrees:90}]}};
    const html=renderToStaticMarkup(<WorkflowRunPanel client={new Client("fixture")} changed={async()=>{}} view={value} t={key=>en[key]} busy={false} artifacts={[]} approve={()=>{}}/>);
    expect(html).toContain(en.image_edit);expect(html).toContain("scan-gray.png");expect(html).toContain("grayscale");expect(html).toContain("rotate");
  });
});
