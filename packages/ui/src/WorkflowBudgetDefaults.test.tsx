import {expect,it} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {decode,defaultRunBudgets} from "@cowork/client";
import {WorkflowVersionPreview} from "./WorkflowVersionPreview";
import {WorkflowDefinitionSummary,WorkflowEditor} from "./WorkflowLibrary";
import {en} from "./i18n";
it("renders identical library and selected-version limits for explicit and serialized default budgets",()=>{
  const definition=decode("WorkflowDefinition",{format_version:1,name:"Budgeted workflow",mode:{mode:"sequential"},failure:"stop",rights:{tools:[],folders:[]},budgets:{...defaultRunBudgets},roles:[],steps:[]});
  const version=decode("WorkflowVersion",{workflow_id:"workflow",project_id:"project",reference:{id:"version",revision:1,name:definition.name,sha256:"a".repeat(64),created_at:1},definition});
  const explicitPreview=renderToStaticMarkup(<WorkflowVersionPreview version={version} providers={[]} t={key=>en[key]}/>);
  const explicitSummary=renderToStaticMarkup(<WorkflowDefinitionSummary definition={definition} providers={[]} t={key=>en[key]}/>);
  delete definition.budgets.max_total_tokens;
  expect(renderToStaticMarkup(<WorkflowVersionPreview version={version} providers={[]} t={key=>en[key]}/>)).toBe(explicitPreview);
  expect(renderToStaticMarkup(<WorkflowDefinitionSummary definition={definition} providers={[]} t={key=>en[key]}/>)).toBe(explicitSummary);
  for(const markup of [explicitPreview,explicitSummary]){expect(markup).toContain(String(defaultRunBudgets.max_total_tokens));expect(markup).toContain(String(defaultRunBudgets.max_output_tokens));expect(markup).toContain(en.maxReplyTokens);}
});

it("emits a browser-valid v-flag role pattern with the unchanged identifier semantics",()=>{
  const definition=decode("WorkflowDefinition",{format_version:1,name:"Workflow",mode:{mode:"sequential"},failure:"stop",rights:{tools:[],folders:[]},budgets:{...defaultRunBudgets},roles:[{id:"Research",instructions:"Research sources",provider_id:"model",provider_revision:1,rights:{tools:[],folders:[]},budgets:{...defaultRunBudgets},delegate_to:[]}],steps:[]});
  const markup=renderToStaticMarkup(<WorkflowEditor value={definition} setValue={()=>{}} providers={[]} disabled={false} t={key=>en[key]} onSubmit={()=>{}}/>);
  const pattern=markup.match(/pattern="([^"]+)"/)?.[1];expect(pattern).toBeDefined();
  const regex=new RegExp("^(?:"+pattern+")$","v");
  for(const valid of ["Research","Review_2","Research-2"])expect(regex.test(valid)).toBe(true);
  for(const invalid of ["","Research team","Prüfung","Research.2","Research/2"])expect(regex.test(invalid)).toBe(false);
});
