import type {ReactNode} from "react";
import {describe,expect,it,vi} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {QueryClient,QueryClientProvider} from "@tanstack/react-query";
import {Client,decode,defaultRunBudgets,type WorkflowInputView,type WorkflowInputDraft,type WorkflowInputDraftSnapshot} from "@cowork/client";
import {WorkflowInputsPanel,WorkflowInputAnswer,WorkflowInputSnapshotView} from "./WorkflowInputsPanel";
import {WorkflowRunPanel,workflowCanResume} from "./WorkflowRunPanel";
import {de,en} from "./i18n";
import {editableWorkflow,workflowTools} from "./workflowUi";

const question=(request_id="question",role_id="Research",node_id="node-a"):WorkflowInputView=>({request_id,fingerprint:"f".repeat(64),role_id,node_id,question:"  <script>Question</script>\r\nExact line  🟡",state:"waiting_answer",expires_at:2_000_000_000_000,actionable:true,receipt:null,closed_reason:null});
function view(){const binding={workflow_id:"workflow",version_id:"version",definition_sha256:"a".repeat(64),plan_sha256:"b".repeat(64)};return decode("WorkflowRunView",{run:{id:"run",revision:1,last_sequence:1,state:"running",created_at:1,updated_at:1,steps:2,usage:{},spec:{order_id:"order",project_id:"project",prompt:"Two questions",provider:null,workflow:binding,access_mode:"confirm_all",executor:"local-v2",policy_revision:1,allowed_tools:["request_input"],budgets:{...defaultRunBudgets},folders:[]}},binding,name:"Two agent questions",version_revision:1,mode:{mode:"parallel",max_parallel:2},max_parallel:2,state:"running",roles:[],nodes:[{id:"node-a",role_id:"Research",goal:"Research facts",dependencies:[],state:"waiting_input",error_code:null,steps:1,charged_tokens:2},{id:"node-b",role_id:"Review",goal:"Review facts",dependencies:[],state:"waiting_input",error_code:null,steps:1,charged_tokens:2}],approvals:[],inputs:[question(),question("other","Review","node-b")],input_resume_available:false,input_resume_blocked_reason:null,completion_resume_available:false,charged_tokens:4,steps_used:2});}
const wrapper=(children:ReactNode)=>renderToStaticMarkup(<QueryClientProvider client={new QueryClient({defaultOptions:{queries:{retry:false}}})}>{children}</QueryClientProvider>);

describe("workflow question presentation",()=>{
  it("shows multiple independent agent questions while the parent is running, without exposing technical request IDs",()=>{
    const value=view();expect(value.run.state).toBe("running");const changed=vi.fn(async()=>{});
    const markup=wrapper(<WorkflowInputsPanel client={new Client("synthetic")} view={value} t={key=>en[key]} busy={false} changed={changed}/>);
    expect(markup).toContain("Research · Research facts");expect(markup).toContain("Review · Review facts");expect(markup.match(/workflow-input-card/g)).toHaveLength(2);
    expect(markup).toContain("&lt;script&gt;Question&lt;/script&gt;");expect(markup).not.toContain("<script>");expect(markup).not.toContain(en.workflowInputSend);
    expect(markup.replace(/<details>[\s\S]*?<\/details>/g,"")).not.toContain('"request_id"');expect(changed).not.toHaveBeenCalled();
  });

  it("keeps interrupted answers readable with an explicit draft-only explanation and no unsupported resume action",()=>{
    const input={...question(),state:"closed" as const,actionable:false,closed_reason:"interrupted"};
    const markup=wrapper(<WorkflowInputAnswer client={new Client("synthetic")} project="project" run="run" input={input} goal="Research facts" t={key=>en[key]} busy={false} changed={async()=>{}}/>);
    expect(markup).toContain(en.workflowInputInterruptedDraft);expect(markup).not.toContain(en.workflowInputSend);
    const current=view();current.run.state="interrupted";current.state="interrupted";current.input_resume_available=false;
    expect(workflowCanResume(current)).toBe(false);const panel=wrapper(<WorkflowRunPanel client={new Client("synthetic")} view={current} t={key=>en[key]} busy={false} artifacts={[]} approve={()=>{}} changed={async()=>{}}/>);expect(panel).toContain(en.workflowInputResumeUnavailable);
    current.input_resume_available=true;expect(workflowCanResume(current)).toBe(false);
    current.input_resume_preview={available:true,blocked_reason:null,base_run_revision:current.run.revision,base_workflow_revision:3,expected_checkpoint_sha256:"c".repeat(64),expected_cursor_sha256:"d".repeat(64)};expect(workflowCanResume(current)).toBe(true);
  });

  it("renders both retained conflict texts as data and requires an explicit adoption action",()=>{
    const value:WorkflowInputDraftSnapshot={id:"copy",run_id:"run",project_id:"project",request_id:"question",fingerprint:"f".repeat(64),owner:"single_owner_preview",base_revision:0,revision:2,answer:"  Incoming <img src=x>\r\nCopy  ",answer_sha256:"a".repeat(64),conflict:true,created_at:2};
    const head:WorkflowInputDraft={...value,answer:"Confirmed <script>head</script>",snapshot_id:"head",updated_at:2};const copy=vi.fn();
    for(const language of [de,en]){const markup=renderToStaticMarkup(<WorkflowInputSnapshotView value={value} head={head} t={key=>language[key]} disabled={false} useCopy={copy}/>);expect(markup).toContain("&lt;img src=x&gt;");expect(markup).toContain("&lt;script&gt;head&lt;/script&gt;");expect(markup).toContain(language.workflowInputRestoreSnapshot);expect(markup).not.toContain("<img ");}
    expect(copy).not.toHaveBeenCalled();expect(renderToStaticMarkup(<WorkflowInputSnapshotView value={value} head={head} t={key=>en[key]} disabled={true} useCopy={copy}/>)).toContain('disabled=""');
  });

  it("renders exact request_input approval text alongside the supported file, delegation and native adapter tools",()=>{
    expect(workflowTools).toEqual(["create_artifact","list_artifacts","read_artifact","request_input","delegate_agents","list_files","read_text_file","stat_file","write_text_file","create_directory","rename_file","remove_file","restore_file","execute_program","browser","office","github","document_read","document_write","image_edit"]);
    const value=view();value.inputs=[];value.approvals=[{node_id:"node-a",role_id:"Research",approval:{id:"approval",run_id:"run",policy_revision:1,target:"local-v2",fingerprint:"b".repeat(64),decision:null,tool:{id:"call",name:"request_input",arguments:{question:"Exact <script>question</script>"}}}}];
    const markup=wrapper(<WorkflowRunPanel client={new Client("synthetic")} view={value} t={key=>en[key]} busy={false} artifacts={[]} approve={()=>{}} changed={async()=>{}}/>);expect(markup).toContain(en.workflowInputApprovalHint);expect(markup).toContain("&lt;script&gt;question&lt;/script&gt;");expect(markup).not.toContain(en.workflowArtifactScope);
    const definition={format_version:1 as const,name:"Question",mode:{mode:"sequential" as const},failure:"stop" as const,rights:{tools:["request_input"],folders:[]},budgets:{...defaultRunBudgets},roles:[],steps:[]};expect(editableWorkflow(definition)).toBe(true);definition.rights.tools=["read_text_file"];expect(editableWorkflow(definition)).toBe(true);for(const tool of ["document_read","document_write","image_edit"]){definition.rights.tools=[tool];expect(editableWorkflow(definition)).toBe(true);}definition.rights.tools=["unsupported_future_tool"];expect(editableWorkflow(definition)).toBe(false);
  });
});
