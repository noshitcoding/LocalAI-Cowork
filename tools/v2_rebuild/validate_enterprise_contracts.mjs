// Offline schema compatibility only; a matching document is never execution authority.
import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import assert from "node:assert/strict";
import Ajv2020 from "ajv/dist/2020.js";
import addFormats from "ajv-formats";

const [directory,expectedId,sampleFile,outputFile]=process.argv.slice(2);
assert(directory&&/^[a-f0-9]{64}$/.test(expectedId??""),"provide the bundle directory and its independently selected pin");
const root=fs.realpathSync(directory);
const digest=bytes=>crypto.createHash("sha256").update(bytes).digest("hex");
function read(name,maximum){
  const target=path.join(root,name),stat=fs.lstatSync(target);
  assert(stat.isFile()&&!stat.isSymbolicLink()&&stat.size<=maximum,"invalid artifact file");
  assert.equal(path.dirname(fs.realpathSync(target)),root,"artifact file escapes bundle");
  return fs.readFileSync(target);
}
assert.deepEqual(fs.readdirSync(root).sort(),["execution.schema.json","manifest.json","openapi.json"]);
const manifestBytes=read("manifest.json",1024*1024),manifest=JSON.parse(manifestBytes);
assert.equal(digest(manifestBytes),expectedId,"bundle differs from the selected pin");
assert.equal(manifest.format_version,1);assert.equal(manifest.kind,"localai-cowork-v2-execution-contracts");
assert.equal(manifest.qualification,"source_contract_only; no actor, snapshot, installed-runtime or execution attestation");
const documents={};
for(const name of ["execution.schema.json","openapi.json"]){
  const bytes=read(name,2*1024*1024);assert.equal(digest(bytes),manifest.files[name].sha256);assert.equal(bytes.length,manifest.files[name].bytes);
  documents[name]=JSON.parse(bytes);
}
const schema=documents["execution.schema.json"],api=documents["openapi.json"];
assert.equal(Object.keys(schema.$defs).length,manifest.definition_count);
const ajv=new Ajv2020({strict:false,allErrors:true,logger:false,loadSchema:undefined});addFormats(ajv);
ajv.addSchema(schema,"common-v2");
for(const name of manifest.roots){assert(ajv.getSchema(`common-v2#/$defs/${name}`));}
// Compile the OpenAPI references independently as schema entry points.
const apiAjv=new Ajv2020({strict:false,allErrors:true,logger:false});addFormats(apiAjv);
apiAjv.addSchema(api,"common-v2-openapi");
for(const name of manifest.roots){assert(apiAjv.getSchema(`common-v2-openapi#/components/schemas/${name}`));}
const proof={passed:true,bundle_id:expectedId,producer_version:manifest.producer_version,definition_count:manifest.definition_count,
  compiled_roots:manifest.roots.length,source_contract_only:true,execution_qualified:false,sample_verified:false};
if(sampleFile){
  const bytes=fs.readFileSync(sampleFile),sample=JSON.parse(bytes);
  const validate=ajv.getSchema("common-v2#/$defs/PreparedExecutionPlan"),validateApi=apiAjv.getSchema("common-v2-openapi#/components/schemas/PreparedExecutionPlan");
  assert(validate(sample),JSON.stringify(validate.errors));assert(validateApi(sample),JSON.stringify(validateApi.errors));
  for(const mutate of [v=>{v.plan.boundary.input_snapshot_bound="true";},v=>{delete v.plan.actor;},v=>{v.plan.grant_all_effects=true;}]){
    const invalid=structuredClone(sample);mutate(invalid);assert(!validate(invalid));assert(!validateApi(invalid));
  }
  proof.sample_verified=true;proof.sample_sha256=digest(bytes);proof.negative_sample_cases=3;
}
if(outputFile)fs.writeFileSync(outputFile,JSON.stringify(proof,null,2)+"\n");
console.log(JSON.stringify(proof));
