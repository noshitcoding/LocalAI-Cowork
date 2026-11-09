import {expect,it} from "vitest";
import {canonicalJsonValue} from "./canonicalJson";

it("keeps native large integer, negative zero, decimal and escaped string bytes exact",()=>{
 const input='{"amount":9007199254740993,"decimal":1.2300,"negative":-0.0,"text":"escape \\\" } ] \\u0041"}';
 const json=`{"preflight":{"review":{"workflow":{"inputs":${input}}}}}`;
 expect(canonicalJsonValue(json,["preflight","review","workflow","inputs"])).toBe(input);
 expect(canonicalJsonValue(input,["amount"])).toBe("9007199254740993");
 expect(canonicalJsonValue(input,["decimal"])).toBe("1.2300");expect(canonicalJsonValue(input,["negative"])).toBe("-0.0");
});
it("selects decoded object keys and nested arrays without matching text inside strings",()=>{
 const raw=' {"other":"\\\"target\\\":999", "ta\\u0072get" : [1,{"end":" ] \\\" }"}]} ';
 expect(canonicalJsonValue(raw,["target"])).toBe('[1,{"end":" ] \\\" }"}]');
 expect(canonicalJsonValue('{"target":{"x":false},"z":null}',["target","x"])).toBe("false");
});
it("rejects missing paths, invalid JSON, duplicate decoded keys and excessive nesting",()=>{
 for(const [raw,path] of [['{}',["missing"]],['{"x":',[]],['{"x":1,"\\u0078":2}',["x"]],['{"x":'+"[".repeat(257)+'0'+"]".repeat(257)+'}',["x"]]] as const){
  expect(()=>canonicalJsonValue(raw,path)).toThrow();
 }
});
