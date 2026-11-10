//! Pinned common V2 source contracts. These bytes are not execution authority.
use crate::{error::ApiError, AppState};
use axum::{http::header, response::IntoResponse, routing::get, Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, sync::OnceLock};

#[path = "v2_runtime_contract_pin.rs"]
mod pin;

const ROOTS: [&str; 16] = ["ExecutionPlan", "PreparedExecutionPlan", "CommitExecutionPlan", "RunRequest", "RetiredExecutionRequest", "RunRecord", "ApiError",
    "PrepareOwnerExecutionPlanV2", "ExecutionPlanV2", "PreparedExecutionPlanV2",
    "PrepareExecutionInput", "PreparedExecutionInput", "RetireExecutionInput", "PrepareRestoreExecutionPlanV2", "PrepareImportExecutionPlanV2", "WorkflowInputSnapshot"];
const QUALIFICATION: &str = "source_contract_only; no actor, snapshot, installed-runtime or execution attestation";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    sha256: String,
    bytes: usize,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format_version: u32,
    kind: String,
    producer_version: String,
    exporter_sha256: String,
    roots: Vec<String>,
    definition_count: usize,
    files: BTreeMap<String, File>,
    sources: BTreeMap<String, String>,
    qualification: String,
}

fn digest(bytes: &[u8]) -> String { hex::encode(Sha256::digest(bytes)) }
fn hash(value: &str) -> bool { value.len() == 64 && value.bytes().all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v)) }
fn references(value: &Value, prefix: &str, definitions: &serde_json::Map<String, Value>) -> Result<(), String> {
    match value {
        Value::Object(fields) => for (name, value) in fields {
            if name == "$ref" {
                let reference = value.as_str().and_then(|v| v.strip_prefix(prefix));
                if reference.is_none_or(|v| v.is_empty() || v.contains(['/', '~']) || !definitions.contains_key(v)) {
                    return Err("external or unresolved contract reference".into());
                }
            } else { references(value, prefix, definitions)?; }
        },
        Value::Array(items) => for value in items { references(value, prefix, definitions)?; },
        _ => {},
    }
    Ok(())
}
fn normalize(value: &mut Value) {
    match value {
        Value::Object(fields) => for (key, value) in fields {
            if key == "$ref" {
                if let Some(name) = value.as_str().and_then(|v| v.strip_prefix("#/components/schemas/")) {
                    *value = json!(format!("#/$defs/{name}"));
                }
            } else { normalize(value); }
        },
        Value::Array(values) => for value in values { normalize(value); },
        _ => {},
    }
}
fn validate(bundle: &str, manifest: &[u8], schema: &[u8], api: &[u8]) -> Result<Value, String> {
    if !hash(bundle) || manifest.len() > 1024 * 1024 || schema.len() > 2 * 1024 * 1024 || api.len() > 2 * 1024 * 1024 || digest(manifest) != bundle {
        return Err("contract bundle pin mismatch or bound exceeded".into());
    }
    let manifest: Manifest = serde_json::from_slice(manifest).map_err(|_| "invalid contract manifest")?;
    if manifest.format_version != 1 || manifest.kind != "localai-cowork-v2-execution-contracts"
        || manifest.qualification != QUALIFICATION || !hash(&manifest.exporter_sha256)
        || manifest.producer_version.is_empty() || manifest.producer_version.len() > 80
        || !manifest.producer_version.bytes().all(|v| v.is_ascii_alphanumeric() || b".+-".contains(&v))
        || manifest.roots.iter().map(String::as_str).collect::<Vec<_>>() != ROOTS
        || manifest.definition_count > 1000 || manifest.files.len() != 2
        || manifest.sources.is_empty() || manifest.sources.len() > 2048
        || manifest.sources.iter().any(|(name, value)| !hash(value) || name.contains(['\\', ':']) || name.split('/').any(|s| s.is_empty() || s == "." || s == "..")) {
        return Err("unsupported contract manifest or provenance".into());
    }
    for (name, bytes) in [("execution.schema.json", schema), ("openapi.json", api)] {
        let expected = manifest.files.get(name).ok_or("missing contract file")?;
        if expected.bytes != bytes.len() || expected.sha256 != digest(bytes) { return Err("contract file digest mismatch".into()); }
    }
    let schema: Value = serde_json::from_slice(schema).map_err(|_| "invalid contract schema")?;
    let api: Value = serde_json::from_slice(api).map_err(|_| "invalid contract OpenAPI")?;
    let definitions = schema["$defs"].as_object().ok_or("missing contract definitions")?;
    if definitions.len() != manifest.definition_count || ROOTS.iter().any(|name| !definitions.contains_key(*name))
        || schema["$schema"] != "https://json-schema.org/draft/2020-12/schema" || api["openapi"] != "3.1.0" {
        return Err("incomplete or unsupported common V2 contract".into());
    }
    references(&schema, "#/$defs/", definitions)?;
    references(&api, "#/components/schemas/", definitions)?;
    let mut api_definitions = api["components"]["schemas"].clone(); normalize(&mut api_definitions);
    if api_definitions != schema["$defs"] { return Err("common V2 schema and OpenAPI disagree".into()); }
    Ok(json!({"status":"source_contracts_only","bundle_id":bundle,"producer_version":manifest.producer_version,
        "roots":manifest.roots,"definition_count":manifest.definition_count,
        "files":manifest.files.into_iter().map(|(name,file)|(name,json!({"sha256":file.sha256,"bytes":file.bytes}))).collect::<BTreeMap<_,_>>(),
        "execution_authority":false,"qualification":QUALIFICATION}))
}

fn embedded() -> Result<&'static Value, ApiError> {
    static VALUE: OnceLock<Result<Value, String>> = OnceLock::new();
    VALUE.get_or_init(|| validate(pin::BUNDLE_ID, pin::MANIFEST, pin::SCHEMA, pin::OPENAPI))
        .as_ref().map_err(|_| ApiError::Unavailable("embedded common V2 contracts failed validation".into()))
}
pub(crate) fn validate_embedded() -> Result<(), ApiError> { embedded().map(|_| ()) }
pub(crate) fn descriptor() -> Result<&'static Value, ApiError> { embedded() }
pub(crate) fn router() -> Router<AppState> {
    Router::new().route("/runtime-contracts/v2", get(metadata))
        .route("/runtime-contracts/v2/schema", get(schema))
        .route("/runtime-contracts/v2/openapi", get(openapi))
}
async fn metadata() -> Result<impl IntoResponse, ApiError> {
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(embedded()?.clone())))
}
fn document(bytes: &'static [u8]) -> Result<impl IntoResponse, ApiError> {
    embedded()?;
    Ok(([(header::CONTENT_TYPE, "application/json"), (header::CACHE_CONTROL, "no-store")], bytes))
}
async fn schema() -> Result<impl IntoResponse, ApiError> { document(pin::SCHEMA) }
async fn openapi() -> Result<impl IntoResponse, ApiError> { document(pin::OPENAPI) }

#[cfg(test)]
mod tests {
    use super::*;
    fn repack(mut manifest: Value, schema: &Value, api: &Value) -> (String, Vec<u8>, Vec<u8>, Vec<u8>) {
        let schema = serde_json::to_vec(schema).unwrap(); let api = serde_json::to_vec(api).unwrap();
        for (name, bytes) in [("execution.schema.json", &schema), ("openapi.json", &api)] {
            manifest["files"][name] = json!({"sha256":digest(bytes),"bytes":bytes.len()});
        }
        let manifest = serde_json::to_vec(&manifest).unwrap(); (digest(&manifest), manifest, schema, api)
    }
    fn originals() -> (Value, Value, Value) {
        (serde_json::from_slice(pin::MANIFEST).unwrap(), serde_json::from_slice(pin::SCHEMA).unwrap(), serde_json::from_slice(pin::OPENAPI).unwrap())
    }
    #[test]
    fn embedded_common_contract_has_closed_refs_and_grants_no_execution() {
        let value = embedded().unwrap(); assert_eq!(value["bundle_id"], pin::BUNDLE_ID);
        assert_eq!(value["execution_authority"], false); assert!(value["definition_count"].as_u64().unwrap() >= ROOTS.len() as u64);
    }
    #[test]
    fn changed_bytes_and_forged_pins_are_rejected() {
        assert!(validate(&"0".repeat(64), pin::MANIFEST, pin::SCHEMA, pin::OPENAPI).is_err());
        let mut changed = pin::SCHEMA.to_vec(); changed.push(b' ');
        assert!(validate(pin::BUNDLE_ID, pin::MANIFEST, &changed, pin::OPENAPI).is_err());
    }
    #[test]
    fn future_format_unknown_fields_and_unsafe_provenance_are_rejected() {
        for change in [json!({"format_version":2}), json!({"runtime_qualified":true}), json!({"sources":{"../outside":"0".repeat(64)}})] {
            let (mut manifest, schema, api) = originals();
            for (key,value) in change.as_object().unwrap() { manifest[key] = value.clone(); }
            let (id,m,s,a) = repack(manifest,&schema,&api); assert!(validate(&id,&m,&s,&a).is_err());
        }
    }
    #[test]
    fn a_rehashed_bundle_cannot_introduce_remote_or_dangling_refs() {
        for reference in ["https://example.invalid/schema", "#/$defs/Missing"] {
            let (manifest, mut schema, api) = originals();
            schema["$defs"]["ExecutionPlan"]["properties"]["proof"] = json!({"$ref":reference});
            let (id,m,s,a) = repack(manifest,&schema,&api);
            assert_eq!(validate(&id,&m,&s,&a).unwrap_err(), "external or unresolved contract reference");
        }
    }
    #[test]
    fn missing_roots_and_divergent_api_semantics_are_rejected() {
        let (manifest, mut schema, api) = originals(); schema["$defs"].as_object_mut().unwrap().remove("ExecutionPlan");
        let (id,m,s,a) = repack(manifest,&schema,&api); assert!(validate(&id,&m,&s,&a).is_err());
        let (manifest, schema, mut api) = originals(); api["components"]["schemas"]["ExecutionPlan"]["additionalProperties"] = json!(true);
        let (id,m,s,a) = repack(manifest,&schema,&api); assert_eq!(validate(&id,&m,&s,&a).unwrap_err(), "common V2 schema and OpenAPI disagree");
    }
}
