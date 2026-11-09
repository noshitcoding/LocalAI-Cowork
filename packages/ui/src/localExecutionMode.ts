import type {EnvironmentReference} from "./EnvironmentPanel";

export type LocalExecutionMode="selected_folders"|"user_rights"|"windows_test_vm";
export type ExecutionSelection={environment:EnvironmentReference|null,nativeTools:string[]};

// The submitted fields are the authority. This presentation preference never
// grants an executable or converts a bundle into a Windows virtual machine.
export function executionMode(selection:ExecutionSelection):LocalExecutionMode {
  return selection.nativeTools.length?"user_rights":"selected_folders";
}
export function changeExecutionMode(mode:LocalExecutionMode,selection:ExecutionSelection):ExecutionSelection {
  return {environment:mode==="selected_folders"?selection.environment:null,nativeTools:[]};
}
