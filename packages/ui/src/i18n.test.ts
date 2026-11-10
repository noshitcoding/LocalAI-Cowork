import {expect,it} from "vitest";
import {de,en} from "./i18n";
it("has the same translated keys in German and English",()=>{
  expect(Object.keys(en).sort()).toEqual(Object.keys(de).sort());
  for(const value of Object.values(en))expect(value.trim()).not.toBe("");
});

