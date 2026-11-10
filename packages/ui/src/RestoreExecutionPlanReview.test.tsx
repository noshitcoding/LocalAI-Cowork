import {readFileSync} from "node:fs";
import {beforeEach,expect,it,vi} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {decode} from "@cowork/client";
import {RestorePlanSummary} from "./RestoreExecutionPlanReview";

const state=vi.hoisted(()=>({language:"en"}));
vi.mock("./view",()=>({useView:(selector:(view:{language:string})=>unknown)=>selector({language:state.language})}));
const fixture=JSON.parse(readFileSync(new URL("../../client/src/fixtures/execution-restore-plan-v2.json",import.meta.url),"utf8"));
beforeEach(()=>{state.language="en";});
it("reviews the exact version and destination with no model and a new independent copy",()=>{
  const p=decode("PreparedExecutionPlanV2",fixture.prepared);
  const html=renderToStaticMarkup(<RestorePlanSummary prepared={p} target="https://fixture.invalid"/>);
  expect(html).toContain("new copy");expect(html).toContain("remain unchanged");
  expect(html).toContain("restore without a model");expect(html).toContain("restored.txt");
  expect(html).toContain("Written version");expect(html).toContain(p.sha256);
  expect(html).not.toContain("canonical_json");expect(html).not.toContain("generation");
  state.language="de";expect(renderToStaticMarkup(<RestorePlanSummary prepared={p} target="https://fixture.invalid"/>)).toContain("neue Kopie");
});
it("displays a directory restoration as a folder instead of claiming it is a text file",()=>{
  const p=decode("PreparedExecutionPlanV2",structuredClone(fixture.prepared));
  if(p.plan.preflight.kind!=="restore")throw new Error("restore fixture");
  p.plan.preflight.review.content.kind="directory";
  expect(renderToStaticMarkup(<RestorePlanSummary prepared={p} target="https://fixture.invalid"/>)).toContain("Written version · Folder");
});
