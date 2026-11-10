import {readFileSync} from "node:fs";
import {expect,it,vi} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {decode} from "@cowork/client";
import {ImportPlanSummary} from "./ImportExecutionPlanReview";
vi.mock("./view",()=>({useView:(selector:(view:{language:string})=>unknown)=>selector({language:"en"})}));
const fixture=JSON.parse(readFileSync(new URL("../../client/src/fixtures/execution-import-plan-v2.json",import.meta.url),"utf8"));

it("reviews the actual upload, independent destination and explicit result reuse without model transmission",()=>{
  const prepared=decode("PreparedExecutionPlanV2",structuredClone(fixture.prepared));
  const html=renderToStaticMarkup(<ImportPlanSummary prepared={prepared} target="https://fixture.invalid"/>);
  expect(html).toContain("Prüfung.bin");expect(html).toContain("new.bin");expect(html).toContain("new working copy");
  expect(html).toContain("import without a model");expect(html).toContain("Branch from here");expect(html).toContain(prepared.sha256);
  expect(html).not.toContain("canonical_json");expect(html).not.toContain("generation");
});

it("distinguishes observed replacement in the new copy from creating a new file",()=>{
  const prepared=decode("PreparedExecutionPlanV2",structuredClone(fixture.prepared));
  if(prepared.plan.preflight.kind!=="import")throw new Error("import fixture");
  prepared.plan.preflight.review.import.expected_sha256="a".repeat(64);
  const html=renderToStaticMarkup(<ImportPlanSummary prepared={prepared} target="https://fixture.invalid"/>);
  expect(html).toContain("Replace the observed version in this copy");expect(html).toContain("a".repeat(64));expect(html).toContain("earlier results and your chat draft are preserved");
});
