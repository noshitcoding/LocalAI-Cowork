import {afterEach,beforeEach,expect,it,vi} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {Client,decode,type RunDetail} from "@cowork/client";
import fixture from "../../client/src/fixtures/owner-managed-reconciliation.json";
import {ManagedProcessRecoveryPanel} from "./ManagedProcessRecoveryPanel";
import {workspaceJournalKey} from "./workspaceManagementApi";
const view=vi.hoisted(()=>({language:"en"}));
vi.mock("./view",()=>({useView:(select:(value:{language:string})=>unknown)=>select(view)}));
const client=new Client("private-token");
function detail():RunDetail&{process_attempts:NonNullable<RunDetail["process_attempts"]>} {return {
  run:decode("RunRecord",structuredClone(fixture.response.run)),process_attempts:[decode("ProcessAttempt",structuredClone(fixture.response.process_attempt))],
  approvals:[],artifacts:[],token_budget:{limit:1000,charged_tokens:0,unconfirmed_tokens:0,remaining_tokens:1000,limit_exceeded:false}
};}
function render(value:RunDetail,fresh=true,order=value.run.spec.order_id) {return renderToStaticMarkup(<ManagedProcessRecoveryPanel client={client} order={order} detail={value} fresh={fresh} busy={false} changed={vi.fn()}/>);}
beforeEach(()=>{view.language="en";});
afterEach(()=>{vi.unstubAllGlobals();vi.restoreAllMocks();});
it("offers explicit recovery only for an uncertain interrupted managed attempt",()=>{
  const value=detail();value.run.state="interrupted";value.process_attempts[0].state={state:"launch_committed"};
  const call=vi.spyOn(client,"reconcileOwnerManagedProcess");const html=render(value);
  expect(html).toContain("Actual process termination is not confirmed yet");expect(html).toContain("Check process completion and cancel task");
  expect(html).toContain("Check process completion</button>");expect(call).not.toHaveBeenCalled();call.mockRestore();
  value.run.state="running";expect(render(value)).not.toContain("Check process completion</button>");
});
it("disables recovery while current run state is being revalidated",()=>{
  const value=detail();value.run.state="interrupted";value.process_attempts[0].state={state:"launch_committed"};
  expect(render(value,false)).toContain('disabled=""');
});
it("shows business cancellation and original physical exit independently",()=>{
  const html=render(detail());expect(html).toContain("Verified process termination");expect(html).toContain("exited");expect(html).toContain("Exit code: 0");
  expect(html).toContain("The task is closed as canceled");expect(html).not.toContain("Check process completion</button>");
  const unknown=detail(),state=unknown.process_attempts[0].state;
  if(state.state==="reconciled_completed"){state.result.termination="canceled";state.result.exit_code=null;}
  expect(render(unknown)).toContain("Exit code: unknown");expect(render(unknown)).not.toContain("Exit code: 0");
});
it("hides process controls for a foreign order or a run without an isolated input copy",()=>{
  const value=detail();expect(render(value,true,"foreign-order")).toBe("");
  delete value.run.spec.input_workspace;expect(render(value)).toBe("");
});
it("keeps an exact successful original replay usable after lost reply and reload",()=>{
  const value=detail(),attempt=value.process_attempts[0];
  const key=workspaceJournalKey(client,`managed-process:${JSON.stringify([value.run.spec.order_id,value.run.id,attempt.id])}`);
  vi.stubGlobal("localStorage",{getItem:(name:string)=>name===key?JSON.stringify({format:1,scope:key,request:fixture.intent}):null});
  const html=render(value);
  expect(html).toContain("Confirmation of the original reconciliation request is pending");
  expect(html).toContain("<button>Recheck original reconciliation</button>");
  expect(html).not.toContain("Check process completion and cancel task");
  value.run.revision++;expect(render(value)).toContain('<button disabled="">Recheck original reconciliation</button>');
});
