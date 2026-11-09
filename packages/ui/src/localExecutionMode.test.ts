import {describe,expect,it} from "vitest";
import {changeExecutionMode,executionMode} from "./localExecutionMode";
const environment={environment_id:"env",version_id:"version",definition_sha256:"a".repeat(64),qualification_sha256:"b".repeat(64)};
describe("execution access selection",()=>{
  it("does not mistake a folder grant or environment for native user rights",()=>{
    expect(executionMode({environment,nativeTools:[]})).toBe("selected_folders");
    expect(executionMode({environment:null,nativeTools:["execute_program"]})).toBe("user_rights");
  });
  it("revokes hidden native authority whenever a different mode is selected",()=>{
    expect(changeExecutionMode("selected_folders",{environment,nativeTools:["execute_program","office"]})).toEqual({environment,nativeTools:[]});
    expect(changeExecutionMode("windows_test_vm",{environment,nativeTools:["execute_program"]})).toEqual({environment:null,nativeTools:[]});
    expect(changeExecutionMode("user_rights",{environment,nativeTools:[]})).toEqual({environment:null,nativeTools:[]});
  });
});
