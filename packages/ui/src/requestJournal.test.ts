import {describe,expect,it} from "vitest";
import {definitiveRequestRejection} from "./requestJournal";
describe("request acknowledgement classification",()=>{
  it("keeps auth and transient failures unresolved while preserving explicit CAS conflict handling",()=>{
    for(const status of [401,403,408,425,429,500,502,503,0,undefined,NaN,"409"])expect(definitiveRequestRejection(status),String(status)).toBe(false);
    for(const status of [400,404,409,413,422])expect(definitiveRequestRejection(status),String(status)).toBe(true);
  });
});
