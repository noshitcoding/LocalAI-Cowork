//! Session-only evidence from successful model-list responses, never persisted UI lists.
use crate::db::{ApiProfileRow, Database};
use serde_json::{json, Value};
use std::{collections::HashMap, sync::{Mutex, OnceLock}};

pub const MAX_AGE_MS: i64 = 5 * 60 * 1_000;

#[derive(Clone)]
struct Catalog { target: String, checked_at: i64, models: Vec<String> }
#[derive(Default)]
struct CatalogState { entries: HashMap<String, Catalog>, generations: HashMap<String, u64> }
static CATALOGS: OnceLock<Mutex<CatalogState>> = OnceLock::new();

fn target(profile: &ApiProfileRow) -> String {
    json!([profile.base_url.trim().trim_end_matches('/'), profile.auth_mode,
        profile.verify_tls_certificates]).to_string()
}

/// A stored credential may only accompany its saved endpoint/TLS configuration.
pub fn resolve_request_profile(database: &Database, profile_id: Option<&str>, base_url: &str, verify_tls: bool) -> Result<Option<ApiProfileRow>, String> {
    let Some(id) = profile_id.map(str::trim).filter(|id| !id.is_empty()) else { return Ok(None); };
    let profile = database.get_api_profile(id).map_err(|error| error.to_string())?
        .ok_or_else(|| format!("API profile '{id}' does not exist. Save the profile before fetching models."))?;
    if profile.base_url.trim().trim_end_matches('/') != base_url.trim().trim_end_matches('/')
        || profile.verify_tls_certificates != verify_tls {
        return Err("API profile changed. Save the profile and retry the model list.".to_string());
    }
    Ok(Some(profile))
}

pub fn is_fresh(checked_at: i64, now: i64) -> bool {
    checked_at > 0 && checked_at <= now && now - checked_at < MAX_AGE_MS
}

pub fn capture_generation(profile_id: &str) -> u64 {
    CATALOGS.get_or_init(Default::default).lock().ok()
        .and_then(|state| state.generations.get(profile_id).copied()).unwrap_or(0)
}

pub fn record(profile: &ApiProfileRow, models: Vec<String>, generation: u64) -> bool {
    if let Ok(mut state) = CATALOGS.get_or_init(Default::default).lock() {
        if state.generations.get(&profile.id).copied().unwrap_or(0) != generation { return false; }
        state.entries.insert(profile.id.clone(), Catalog { target: target(profile),
            checked_at: chrono::Utc::now().timestamp_millis(), models });
        return true;
    }
    false
}

pub fn invalidate(profile_id: &str) {
    if let Ok(mut state) = CATALOGS.get_or_init(Default::default).lock() {
        state.entries.remove(profile_id);
        let generation = state.generations.entry(profile_id.to_string()).or_default();
        *generation = generation.wrapping_add(1);
    }
}

pub fn attach(config: &mut Value, profile: Option<&ApiProfileRow>) {
    // A frontend timestamp or an old persisted list is not backend evidence.
    if let Some(object) = config.as_object_mut() {
        object.remove("models");
        object.remove("modelCatalogCheckedAt");
    }
    let Some(profile) = profile else { return; };
    let Some(entries) = CATALOGS.get().and_then(|entries| entries.lock().ok()) else { return; };
    if let Some(entry) = entries.entries.get(&profile.id).filter(|entry|
        entry.target == target(profile) && is_fresh(entry.checked_at, chrono::Utc::now().timestamp_millis())) {
        config["models"] = json!(entry.models);
        config["modelCatalogCheckedAt"] = json!(entry.checked_at);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timestamps_expire_and_future_or_unknown_times_are_not_evidence() {
        assert!(is_fresh(1_000, 1_001));
        assert!(!is_fresh(0, 1_001));
        assert!(!is_fresh(1_002, 1_001));
        assert!(!is_fresh(1_000, 1_000 + MAX_AGE_MS));
    }
    #[test]
    fn unknown_persisted_catalogs_are_removed() {
        let mut config = json!({"models":["old"], "modelCatalogCheckedAt":chrono::Utc::now().timestamp_millis()});
        attach(&mut config, None);
        assert!(config.get("models").is_none());
        assert!(config.get("modelCatalogCheckedAt").is_none());
    }
    #[test]
    fn saved_model_list_requests_reject_endpoint_or_tls_substitution() {
        let db = Database::open_in_memory().unwrap();
        assert!(resolve_request_profile(&db, Some("default-openrouter"), "https://unrelated.test/v1", true).is_err());
        assert!(resolve_request_profile(&db, Some("default-openrouter"), "https://openrouter.ai/api/v1", false).is_err());
        assert!(resolve_request_profile(&db, Some("missing"), "https://unrelated.test/v1", true).is_err());
        let profile = resolve_request_profile(&db, Some("default-ollama"), "http://localhost:11434/v1/", true).unwrap().unwrap();
        assert_eq!(profile.auth_mode, "none");
        assert!(resolve_request_profile(&db, None, "http://localhost:1234/v1", true).unwrap().is_none());
    }
    #[test]
    fn saving_profile_metadata_does_not_discard_current_catalog_evidence() {
        let db = Database::open_in_memory().unwrap();
        let mut profile = db.get_api_profile("default-ollama").unwrap().unwrap();
        profile.id = "catalog-metadata-fixture".to_string();
        assert!(record(&profile, vec!["current-model".to_string()], capture_generation(&profile.id)));
        profile.updated_at = "later-metadata-sync".to_string();
        profile.model = "selected-model".to_string();
        let mut config = json!({});
        attach(&mut config, Some(&profile));
        assert_eq!(config["models"], json!(["current-model"]));
        invalidate(&profile.id);
    }
    #[test]
    fn credential_change_rejects_an_inflight_catalog_response() {
        let db = Database::open_in_memory().unwrap();
        let mut profile = db.get_api_profile("default-openrouter").unwrap().unwrap();
        profile.id = "credential-race-fixture".to_string();
        let generation = capture_generation(&profile.id);
        invalidate(&profile.id);
        assert!(!record(&profile, vec!["old-credential-model".to_string()], generation));
        let mut config = json!({});
        attach(&mut config, Some(&profile));
        assert!(config.get("models").is_none());
        assert!(record(&profile, vec!["new-credential-model".to_string()], capture_generation(&profile.id)));
        attach(&mut config, Some(&profile));
        assert_eq!(config["models"], json!(["new-credential-model"]));
        invalidate(&profile.id);
    }
}
