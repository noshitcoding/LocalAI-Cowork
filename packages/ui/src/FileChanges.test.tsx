import {describe,expect,it,vi} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {Client,decode,type RunDetail} from "@cowork/client";
import fixture from "../../client/src/fixtures/execution-plan-v2.json";
import {en} from "./i18n";
import {FileChanges} from "./FileChanges";

function detail(snapshot:boolean):RunDetail{
  const run=decode("RunRecord",structuredClone(fixture.run));
  run.state="completed";
  if(!snapshot)delete run.spec.input_workspace;
  return {run,approvals:[],artifacts:[],input_request:null,file_operations:[],uncertain_file_operation:null,
    token_budget:{limit:1000,charged_tokens:0,unconfirmed_tokens:0,remaining_tokens:1000,limit_exceeded:false},
    file_changes:[{run_id:run.id,tool_call_id:"write",folder_id:"synthetic-folder",path:"proof.txt",before_sha256:"a".repeat(64),after_sha256:"b".repeat(64),size:3}]};
}
function render(value:RunDetail,copyRestore=false){return renderToStaticMarkup(<FileChanges detail={value} events={[]} client={new Client("synthetic-test")} t={key=>en[key]} busy={false} action={async fn=>{await fn();}} restore={vi.fn()} copyRestore={copyRestore}/>);}
describe("file receipt restoration availability",()=>{
  it("keeps unsupported working-copy restoration out of the product action flow",()=>{
    const markup=render(detail(true));
    expect(markup).not.toContain(en.restorePrevious);
    expect(markup).not.toContain(en.restoreWritten);
    expect(markup).toContain(en.compareVersions);
  });
  it("preserves restoration for ordinary native-folder receipts",()=>{
    const markup=render(detail(false));
    expect(markup).toContain(en.restorePrevious);
    expect(markup).toContain(en.restoreWritten);
  });
  it("offers working-copy restoration only with the confirmed-copy planner and explains the new independent result",()=>{
    const markup=render(detail(true),true);
    expect(markup).toContain(en.restorePrevious);expect(markup).toContain(en.restoreWritten);
    expect(markup).toContain(en.runCopyRestoreHint);expect(markup).not.toContain(en.runCopyRestoreUnavailable);
  });
});
