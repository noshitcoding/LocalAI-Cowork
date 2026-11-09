import {describe,expect,it} from "vitest";
import {githubArguments} from "./DeveloperWorkspace";
import {officeReadOperation} from "./OfficeWorkbench";
import {newToolboxRequest,successfulToolboxResult} from "./toolboxWorkbench";
import type {ToolJob} from "./productApi";
function job(overrides:Record<string,unknown>={}):ToolJob{return {id:"job",order_id:"chat",state:"completed",error:null,outcome:{termination:"exited",exit_code:0,output_complete:true,stdout:{bytes:Array.from(new TextEncoder().encode('{"output":"previews/report.pdf"}')),truncated:false,invalid_utf8:false},stderr:{bytes:[],truncated:false},...overrides}};}
describe("real tool workbench actions",()=>{
  it("builds structured GitHub arguments without interpreting issue text as flags or file references",()=>{
    const body='First line\n$(not-a-shell) `literal` @file --hostname example.com';
    const args=githubArguments({operation:"create_issue",repository:"owner/project",title:"A title",body});
    expect(args).toEqual(["api","repos/owner/project/issues","--method","POST","--raw-field","title=A title","--raw-field","body="+body]);
    expect(()=>githubArguments({operation:"repository",repository:"../secrets"})).toThrow("invalid_repository");
    expect(()=>githubArguments({operation:"pull",repository:"owner/project",number:"--web"})).toThrow("invalid_issue_or_pull_number");
    expect(()=>githubArguments({operation:"create_pull",repository:"owner/project",title:"T",body:"B",head:"",base:"main"})).toThrow("invalid_pull_branches");
    expect(officeReadOperation("report.DOCX")).toBe("read_docx");expect(officeReadOperation("unknown.binary")).toBeNull();
  });
  it("accepts generated file metadata only from complete successful physical tool output",()=>{
    expect(successfulToolboxResult(job())).toEqual({output:"previews/report.pdf"});
    expect(successfulToolboxResult(job({exit_code:1}))).toBeUndefined();
    expect(successfulToolboxResult(job({output_complete:false}))).toBeUndefined();
    expect(successfulToolboxResult(job({stdout:{bytes:[],truncated:true}}))).toBeUndefined();
    const action={kind:"office",operation:"convert",input:"input.docx",output:"preview.pdf"};
    const first=newToolboxRequest(action),second=newToolboxRequest(action);expect(first.idempotency_key).not.toBe(second.idempotency_key);expect(first.action).toEqual(action);expect(first.approval_sha256).toBeUndefined();
  });
});
