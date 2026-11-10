import {execFileSync} from "node:child_process";
import fs from "node:fs";
const files=["contracts/defaults.json","contracts/api.schema.json","contracts/openapi.json","packages/client/src/generated.ts","packages/client/src/api.schema.json","packages/client/src/validators.js","packages/client/src/validators.d.ts"];
const before=new Map(files.map(p=>[p,fs.readFileSync(p,"utf8")]));
execFileSync("cargo",["run","--locked","-p","cowork-contract-generator"],{stdio:"inherit"});
execFileSync(process.execPath,["tools/contracts/typescript.mjs"],{stdio:"inherit"});
const changed=files.filter(p=>fs.readFileSync(p,"utf8")!==before.get(p));
if(changed.length){console.error("Generated contracts were stale: "+changed.join(", "));process.exitCode=1;}
else console.log("Rust schemas, OpenAPI, TypeScript and CSP-safe validators agree.");

if(!changed.length)execFileSync(process.execPath,["tools/contracts/validator-parity.mjs"],{stdio:"inherit"});
