import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import Ajv2020 from "ajv/dist/2020.js";
import addFormats from "ajv-formats";
import standalone from "ajv/dist/standalone/index.js";
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),"../..");
const schema=JSON.parse(fs.readFileSync(path.join(root,"contracts/api.schema.json"),"utf8"));
const defaults=JSON.parse(fs.readFileSync(path.join(root,"contracts/defaults.json"),"utf8"));
function type(s) {
  if(s===true)return "unknown";
  if(s===false)return "never";
  if(s.$ref)return s.$ref.split("/").at(-1);
  if(s.const!==undefined)return JSON.stringify(s.const);
  if(s.enum)return s.enum.map(v=>JSON.stringify(v)).join(" | ");
  if(s.anyOf||s.oneOf)return (s.anyOf||s.oneOf).map(type).join(" | ");
  if(Array.isArray(s.type))return s.type.map(t=>type({...s,type:t})).join(" | ");
  if(s.type==="array")return "Array<"+type(s.items||{})+">";
  if(s.type==="object"||s.properties) {
    const p=Object.entries(s.properties||{}).map(([k,v])=>"  "+JSON.stringify(k)+(s.required?.includes(k)?"":"?")+": "+type(v)+";");
    if(s.additionalProperties && s.additionalProperties!==false)p.push("  [key: string]: "+(s.additionalProperties===true?"unknown":type(s.additionalProperties))+";");
    return p.length ? "{\n"+p.join("\n")+"\n}" : "Record<string, unknown>";
  }
  return ({string:"string",integer:"number",number:"number",boolean:"boolean",null:"null"})[s.type]||"unknown";
}
const names=Object.keys(schema.$defs).sort();
const output="// Generated from Rust contract crates. Do not edit.\n\n"+
  names.map(n=>"export type "+n+" = "+type(schema.$defs[n])+";\n").join("\n")+
  "\nexport interface ContractTypes {\n"+names.map(n=>"  "+n+": "+n+";").join("\n")+"\n}\n"+
  "\nexport const defaultRunBudgets: Required<Budgets> = "+JSON.stringify(defaults.run_budgets)+";\n";
fs.writeFileSync(path.join(root,"packages/client/src/generated.ts"),output);
fs.copyFileSync(path.join(root,"contracts/api.schema.json"),path.join(root,"packages/client/src/api.schema.json"));
// Share referenced validators instead of duplicating their bodies in every contract.
// This preserves synchronous, CSP-safe validation while reducing the initial bundle.
const ajv=new Ajv2020({strict:false,inlineRefs:false,code:{source:true,esm:true}});
addFormats(ajv);
// These Rust numeric formats are annotations; JSON Schema type/minimum/maximum carry validation.
for(const format of ["int32","int64","uint","uint64","uint32","uint16","uint8","double"])ajv.addFormat(format,true);
ajv.addSchema(schema);
const refs=Object.fromEntries(names.map(n=>[n,schema.$id+"#/$defs/"+n]));
let validation=standalone(ajv,refs);
// AJV emits this CommonJS helper for Unicode-aware string lengths even with esm:true.
// Keep its vetted implementation, but resolve it as an explicit CSP-safe ESM import.
const unicodeHelper='require("ajv/dist/runtime/ucs2length").default';
if(validation.includes(unicodeHelper))validation='import unicodeLengthModule from "ajv/dist/runtime/ucs2length.js";\nconst coworkUnicodeLength=unicodeLengthModule.default ?? unicodeLengthModule;\n'+validation.replaceAll(unicodeHelper,"coworkUnicodeLength");
if(validation.includes("require("))throw new Error("Generated validators need an explicit ESM runtime import");
fs.writeFileSync(path.join(root,"packages/client/src/validators.js"),"// Generated at build time: CSP-safe, no runtime eval.\n"+validation+"\n");
fs.writeFileSync(path.join(root,"packages/client/src/validators.d.ts"),names.map(n=>"export declare const "+n+": (value: unknown) => boolean;").join("\n")+"\n");
console.log("Generated "+names.length+" contracts from Rust JSON Schema.");
