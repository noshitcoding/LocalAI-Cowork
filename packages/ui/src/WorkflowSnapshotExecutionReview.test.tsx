import {readFileSync} from "node:fs";
import {expect,it} from "vitest";
import {decode} from "@cowork/client";
import {workflowRequiresDocumentEngine} from "./WorkflowSnapshotExecutionReview";

const original=JSON.parse(readFileSync(new URL("../../../tests/fixtures/workflow-documents.json",import.meta.url),"utf8"));
function withoutDocuments(){const value=decode("WorkflowDefinition",structuredClone(original));
  for(const rights of [value.rights,...value.roles.map(role=>role.rights),...value.steps.map(step=>step.rights)])rights.tools=[];
  return value;
}
it("requires explicit program selection for the published document source",()=>{
 expect(workflowRequiresDocumentEngine(decode("WorkflowDefinition",original))).toBe(true);
 expect(workflowRequiresDocumentEngine(withoutDocuments())).toBe(false);
});
it("also recognizes document tools retained only inside an immutable nested template",()=>{
 const parent=withoutDocuments(),child=decode("WorkflowDefinition",original);
 parent.subflows={nested:{name:"Frozen document source",dependencies:[],definition:child,result:{step_id:"document",pointer:""},rights:{tools:[],folders:[]},budgets:child.budgets}};
 expect(workflowRequiresDocumentEngine(parent)).toBe(true);
});
