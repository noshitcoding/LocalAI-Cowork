// Run separately from parallel UI workers: this compiles every reference schema.
// The shipped validators remain static ESM and never compile code in the browser.
import assert from "node:assert/strict";
import fs from "node:fs";
import Ajv2020 from "ajv/dist/2020.js";
import addFormats from "ajv-formats";
import * as validators from "../../packages/client/src/validators.js";

const schema = JSON.parse(fs.readFileSync(new URL("../../contracts/api.schema.json", import.meta.url), "utf8"));
const reference = new Ajv2020({strict: false});
addFormats(reference);
for (const format of ["int32", "int64", "uint", "uint64", "uint32", "uint16", "uint8", "double"]) reference.addFormat(format, true);
reference.addSchema(schema);
const corpus = [null, undefined, true, false, "", "completed", "made_up", "2026-11-09T00:00:00Z", "urn:cowork:source", "a".repeat(257), 0, 1, -1, 0.5, Number.MAX_SAFE_INTEGER, Infinity, NaN, [], [null], ["a"], {}, {id: "r", state: "made_up"}, {input_tokens: 12, output_tokens: 3, cost: null, price_source_version: null}, {__proto__: null, id: "r"}];
const names = Object.keys(validators).sort();
assert.deepEqual(names, Object.keys(schema.$defs).sort());
for (const name of names) {
  const check = reference.getSchema(`${schema.$id}#/$defs/${name}`);
  assert.ok(check, `Missing reference contract ${name}`);
  for (const [index, value] of corpus.entries()) {
    assert.equal(validators[name](value), check(value), `${name}: boundary corpus case ${index}`);
  }
}
console.log(`Standalone/reference parity: ${names.length} contracts, ${names.length * corpus.length} boundary checks passed.`);
