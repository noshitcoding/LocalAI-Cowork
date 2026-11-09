import {describe,it,expect} from "vitest";
import type {RunRecord} from "@cowork/client";
import {finishedTransitions} from "./desktopBehavior";
const run=(id:string,state:RunRecord["state"])=>({id,state} as RunRecord);
describe("completion notifications",()=>{it("announces each observed completion once and skips imported history",()=>{const states=new Map<string,RunRecord["state"]>();expect(finishedTransitions(states,[run("old","completed"),run("new","running")])).toEqual([]);expect(finishedTransitions(states,[run("old","completed"),run("new","completed")]).map(r=>r.id)).toEqual(["new"]);expect(finishedTransitions(states,[run("old","completed"),run("new","completed")])).toEqual([]);});});
