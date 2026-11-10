import {describe,expect,it,vi} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {decode,defaultRunBudgets,type WorkflowOperation} from "@cowork/client";
import {canCompactWorkflowComposer,ComposerDisclosure,confirmedWorkflowStart,confirmedSnapshotWorkflowStart} from "./composerLayout";
import {WorkflowOperationPanel} from "./workflowJournal";
import {WorkflowVersionPreview} from "./WorkflowVersionPreview";
import {en,de} from "./i18n";
const prompt="  Original <script>data</script>\r\n🟡 retained  ";
function start():WorkflowOperation {
  const binding={workflow_id:"workflow",version_id:"version",definition_sha256:"a".repeat(64),plan_sha256:"b".repeat(64)};
  return decode("WorkflowOperation",{id:"operation",project_id:"project",intent_id:"intent",request_sha256:"a".repeat(64),request:{kind:"start",request:{kind:"workflow",order_id:"order",base_revision:1,prompt,workflow_id:"workflow",version_id:"version",idempotency_key:"original-key",access_mode:"confirm_all",budgets:{...defaultRunBudgets}}},draft_snapshot:null,state:"accepted",receipt:{kind:"started",run_id:"first-run",created_at:2,spec:{order_id:"order",project_id:"project",prompt,provider:null,workflow:binding,access_mode:"confirm_all",executor:"local-v2",policy_revision:1,allowed_tools:["request_input"],budgets:{...defaultRunBudgets},folders:[]}},rejection:null,created_at:1,updated_at:2,acknowledged_at:null});
}
const safe=()=>{
  const operation=start();if(operation.receipt?.kind!=="started")throw new Error("fixture");
  const definition=decode("WorkflowDefinition",{format_version:1,name:"Workflow",mode:{mode:"sequential"},failure:"stop",rights:{tools:["request_input"],folders:[]},budgets:{...defaultRunBudgets},roles:[],steps:[]});
  const version=decode("WorkflowVersion",{project_id:"project",workflow_id:"workflow",reference:{id:"version",revision:1,name:"Workflow",sha256:"a".repeat(64),created_at:1},definition});
  const run=decode("RunRecord",{id:"first-run",revision:1,last_sequence:1,state:"running",created_at:2,updated_at:2,steps:0,usage:{},spec:structuredClone(operation.receipt.spec)});
  return {operation,context:{project:"project",order:"order",run,version},uncertainPrepare:false,pendingSingleRequest:false,draft:prompt,saved:prompt,saving:false,conflict:false,error:null as unknown,loading:false};
};
const snapshotStart=()=>{
  const value=safe(),selection={workflow_id:"workflow",version_id:"version",inputs:{source_path:"invoice.pdf",source_sha256:"c".repeat(64)},access_mode:"confirm_all" as const,budgets:{...defaultRunBudgets}};
  value.context.run.spec.workflow!.inputs=structuredClone(selection.inputs);
  value.context.run.spec.input_workspace={id:"copy",input:{snapshot_id:"snapshot",sha256:"d".repeat(64),manifest_sha256:"e".repeat(64)}};
  return {...value,operation:null,context:{...value.context,selection}};
};
describe("accepted snapshot workflow composer presentation",()=>{
  it("compacts the current saved snapshot run without manufacturing a legacy journal operation",()=>{
    const value=snapshotStart();expect(confirmedSnapshotWorkflowStart(value.context)).toBe(true);expect(canCompactWorkflowComposer(value)).toBe(true);expect(value.operation).toBeNull();
  });
  it("describes the editable next-run draft accurately without requiring a legacy journal action",()=>{
    for(const language of [de,en]){
      const markup=renderToStaticMarkup(<ComposerDisclosure compact snapshot t={key=>language[key]}><form><textarea value={prompt} readOnly/></form></ComposerDisclosure>);
      expect(markup).toContain(language.workflowSnapshotComposer);expect(markup).toContain(language.workflowSnapshotComposerHint);
      expect(markup).not.toContain(language.workflowLockedComposerHint);expect(markup).not.toContain("<details open");expect(markup).toContain("Original &lt;script&gt;data&lt;/script&gt;");
    }
  });
  it("keeps a new selection, changed inputs, access or limits expanded",()=>{
    for(const patch of [{workflow_id:"other"},{version_id:"other"},{inputs:{source_path:"new.pdf",source_sha256:"c".repeat(64)}},{access_mode:"autonomous" as const},{budgets:{...defaultRunBudgets,max_steps:1}}]){
      const value=snapshotStart();Object.assign(value.context.selection,patch);expect(canCompactWorkflowComposer(value)).toBe(false);
    }
  });
  it("requires the authenticated displayed run's project, order, version and frozen copy fields",()=>{
    for(const patch of [{project:"foreign"},{order:"foreign"},{run:undefined},{version:undefined},{selection:undefined}]){
      const value=snapshotStart();expect(canCompactWorkflowComposer({...value,context:{...value.context,...patch}})).toBe(false);
    }
    const missing=snapshotStart();missing.context.run.spec.input_workspace=null;expect(canCompactWorkflowComposer(missing)).toBe(false);
    const corrupt=snapshotStart();corrupt.context.run.spec.input_workspace!.input.sha256="bad";expect(canCompactWorkflowComposer(corrupt)).toBe(false);
    const changed=snapshotStart();changed.context.version.reference.sha256="different";expect(canCompactWorkflowComposer(changed)).toBe(false);
  });
  it("preserves every unsaved draft, conflict, error and in-flight start in expanded form",()=>{
    for(const patch of [{draft:"new request"},{saved:"server copy"},{uncertainPrepare:true},{pendingSingleRequest:true},{saving:true},{conflict:true},{loading:true},{error:new Error("read failed")}])expect(canCompactWorkflowComposer({...snapshotStart(),...patch})).toBe(false);
  });
  it("keeps an unresolved legacy operation visible alongside an earlier snapshot run",()=>{
    expect(canCompactWorkflowComposer({...snapshotStart(),operation:{...start(),state:"pending",receipt:null}})).toBe(false);
  });
});
describe("confirmed start presentation without changing commands or drafts",()=>{
  it("allows compact input only for a confirmed start with its original run receipt",()=>{
    expect(canCompactWorkflowComposer(safe())).toBe(true);
    for(const state of ["pending","rejected","discarded"] as const)expect(confirmedWorkflowStart({...start(),state})).toBe(false);
    expect(confirmedWorkflowStart({...start(),receipt:null})).toBe(false);
    expect(confirmedWorkflowStart(null)).toBe(false);
    expect(confirmedWorkflowStart({...start(),receipt:{kind:"created",publication:{project_id:"project",workflow_id:"workflow",version_id:"version",revision:1,revoked:false,created_at:2}}})).toBe(false);
  });
  it.each(["uncertainPrepare","pendingSingleRequest","saving","conflict","loading"] as const)("expands input whenever %s needs attention",field=>{
    expect(canCompactWorkflowComposer({...safe(),[field]:true})).toBe(false);
  });
  it("keeps errors and every exact unsaved draft difference expanded",()=>{
    expect(canCompactWorkflowComposer({...safe(),error:new Error("lost acknowledgement")})).toBe(false);
    for(const draft of [prompt+" ",prompt.replace("\r\n","\n"),"new draft 🟡",""])expect(canCompactWorkflowComposer({...safe(),draft})).toBe(false);
  });
  it("binds receipt scope, exact original prompt, access and immutable workflow version before compacting",()=>{
    const original=safe();
    for(const [field,value] of [["project_id","foreign"],["order_id","foreign"],["prompt",prompt+" "],["access_mode","autonomous"]] as const){
      const changed=structuredClone(original);if(changed.operation.receipt?.kind!=="started")throw new Error("fixture");
      Object.assign(changed.operation.receipt.spec,{[field]:value});expect(canCompactWorkflowComposer(changed)).toBe(false);
    }
    for(const field of ["workflow_id","version_id","definition_sha256"] as const){const changed=structuredClone(original);if(changed.operation.receipt?.kind!=="started"||!changed.operation.receipt.spec.workflow)throw new Error("fixture");changed.operation.receipt.spec.workflow[field]="foreign";expect(canCompactWorkflowComposer(changed)).toBe(false);}
  });
  it("requires the current project, order and displayed run plus the exact selected version",()=>{
    const original=safe();
    expect(canCompactWorkflowComposer({...original,context:{...original.context,project:"other"}})).toBe(false);
    expect(canCompactWorkflowComposer({...original,context:{...original.context,order:"other"}})).toBe(false);
    expect(canCompactWorkflowComposer({...original,context:{...original.context,run:{...original.context.run,id:"historical"}}})).toBe(false);
    expect(canCompactWorkflowComposer({...original,context:{...original.context,run:undefined}})).toBe(false);
    expect(canCompactWorkflowComposer({...original,context:{...original.context,version:undefined}})).toBe(false);
    const foreign=structuredClone(original);foreign.context.version.project_id="foreign";expect(canCompactWorkflowComposer(foreign)).toBe(false);
    const changed=structuredClone(original);changed.context.run.spec.prompt="different displayed spec";expect(canCompactWorkflowComposer(changed)).toBe(false);
  });
  it("uses exact effective parent limits rather than equating broader requested budgets with the receipt",()=>{
    const value=safe();if(value.operation.receipt?.kind!=="started"||value.operation.request.kind!=="start")throw new Error("fixture");
    for(const key of Object.keys(defaultRunBudgets) as Array<keyof typeof defaultRunBudgets>){
      value.operation.request.request.budgets={...defaultRunBudgets,...value.operation.request.request.budgets,[key]:defaultRunBudgets[key]*2};
    }
    expect(canCompactWorkflowComposer(value)).toBe(true);
    value.operation.receipt.spec.budgets.max_steps--;value.context.run.spec.budgets.max_steps--;
    expect(canCompactWorkflowComposer(value)).toBe(false);
  });
  it("accepts an unchanged confirmed empty server draft after start/reload without rewriting the original request",()=>{
    const value=safe();value.draft="";value.saved="";expect(canCompactWorkflowComposer(value)).toBe(true);
    expect(value.operation.request.kind==="start"&&value.operation.request.request.prompt).toBe(prompt);
  });
  it("keeps the unchanged locked draft mounted inside an explicitly labelled disclosure",()=>{
    const child=<form><textarea disabled value={prompt} readOnly/></form>;
    for(const language of [en,de]){
      const markup=renderToStaticMarkup(<ComposerDisclosure compact t={key=>language[key]}>{child}</ComposerDisclosure>);
      expect(markup).toContain(language.workflowLockedComposer);expect(markup).toContain("<details");expect(markup).not.toContain("<details open");
      expect(markup).toContain("Original &lt;script&gt;data&lt;/script&gt;");expect(markup).toContain("🟡 retained  ");expect(markup).toContain("<textarea");
    }
    const markup=renderToStaticMarkup(<ComposerDisclosure compact={false} t={key=>en[key]}>{child}</ComposerDisclosure>);
    expect(markup).toContain("<details");expect(markup).toContain(" open=");expect(markup).toContain("<summary hidden");expect(markup).toContain("<textarea");
  });
  it("shows an accepted start action immediately and preserves its full frozen request as data",()=>{
    const acknowledge=vi.fn(),commit=vi.fn(),discard=vi.fn();const original=start();
    const markup=renderToStaticMarkup(<WorkflowOperationPanel compactAcceptedStart operation={original} busy={false} error={null} t={key=>en[key]} commit={commit} discard={discard} acknowledge={acknowledge}/>);
    expect(markup).toContain(en.workflowStartAccepted);expect(markup).toContain(en.workflowStartReceipt);expect(markup).toContain(en.workflowJournalOpen);
    expect(markup).toContain("Original &lt;script&gt;data&lt;/script&gt;");expect(markup).toContain("first-run");expect(markup).not.toContain("<script>");
    expect(acknowledge).not.toHaveBeenCalled();expect(commit).not.toHaveBeenCalled();expect(discard).not.toHaveBeenCalled();expect(original).toEqual(start());
  });
  it.each(["pending","rejected","discarded"] as const)("keeps a %s operation in its full recovery view",state=>{
    const markup=renderToStaticMarkup(<WorkflowOperationPanel compactAcceptedStart operation={{...start(),state,receipt:null}} busy={false} error={null} t={key=>en[key]} commit={()=>{}} discard={()=>{}} acknowledge={()=>{}}/>);
    expect(markup).not.toContain(en.workflowStartReceipt);expect(markup).toContain("Original &lt;script&gt;data&lt;/script&gt;");
    expect(markup).toContain(state==="pending"?en.workflowRetrySame:en.workflowJournalAcknowledge);
  });
  it("keeps a lost-response error visible even alongside an accepted receipt",()=>{
    const markup=renderToStaticMarkup(<WorkflowOperationPanel compactAcceptedStart operation={start()} busy={false} error={new Error("response lost")} t={key=>en[key]} commit={()=>{}} discard={()=>{}} acknowledge={()=>{}}/>);
    expect(markup).toContain('role="alert"');expect(markup).toContain(en.workflowJournalUnavailable);expect(markup).not.toContain(en.workflowStartReceipt);
  });
  it("keeps the library's accepted publication presentation unchanged",()=>{
    const original=start();const operation={...original,request:{kind:"revoke" as const,workflow_id:"workflow",request:{base_revision:1,idempotency_key:"key"}}};
    const markup=renderToStaticMarkup(<WorkflowOperationPanel operation={operation} busy={false} error={null} t={key=>en[key]} commit={()=>{}} discard={()=>{}} acknowledge={()=>{}}/>);
    expect(markup).toContain(en.workflowJournalAccepted);expect(markup).not.toContain(en.workflowStartReceipt);
  });
  it("labels the selected version's question capability consistently with the four-tool catalog",()=>{
    const definition=decode("WorkflowDefinition",{format_version:1,name:"Questions",mode:{mode:"sequential"},failure:"stop",rights:{tools:["request_input"],folders:[]},budgets:{...defaultRunBudgets},roles:[],steps:[]});
    const version=decode("WorkflowVersion",{project_id:"project",workflow_id:"workflow",reference:{id:"version",revision:1,name:"Questions",sha256:"a".repeat(64),created_at:1},definition});
    const markup=renderToStaticMarkup(<WorkflowVersionPreview version={version} providers={[]} t={key=>en[key]}/>);
    expect(markup).toContain(en.workflowRequestInput);expect(markup).not.toContain(en.workflowCapabilityUnavailable);
  });
});
