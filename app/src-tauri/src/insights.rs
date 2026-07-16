use crate::db::Database;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ── Request / Response types ────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InsightsEventRequest {
    pub event_type: String,
    pub category: String,
    pub value_num: Option<f64>,
    pub value_text: Option<String>,
    pub session_id: Option<String>,
    pub metadata_json: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct InsightsQueryRequest {
    pub category: Option<String>,
    pub since: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InsightsSummary {
    pub total_events: i64,
    pub total_sessions: i64,
    pub total_messages_sent: i64,
    pub total_tokens_est: i64,
    pub avg_session_duration_min: f64,
    pub top_categories: Vec<CategoryCount>,
    pub recent_events: Vec<EventSummary>,
    pub skill_usage_count: i64,
    pub memory_entry_count: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryCount {
    pub category: String,
    pub count: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventSummary {
    pub event_type: String,
    pub category: String,
    pub value_text: Option<String>,
    pub created_at: String,
}

// ── Core logic ──────────────────────────────────────────────────────────────

/// Record an insights event
pub fn record_event(db: &Arc<Database>, req: &InsightsEventRequest) -> Result<String, String> {
    let id = uuid::Uuid::new_v4().to_string();
    db.insert_insights_event(
        &id,
        &req.event_type,
        &req.category,
        req.value_num,
        req.value_text.as_deref(),
        req.session_id.as_deref(),
        req.metadata_json.as_deref(),
    )
    .map_err(|e| e.to_string())?;
    Ok(id)
}

/// Build an aggregated summary dashboard
pub fn build_summary(db: &Arc<Database>) -> Result<InsightsSummary, String> {
    // Get raw counts from insights_events
    let events = db
        .query_insights(None, None, 500)
        .map_err(|e| e.to_string())?;

    let total_events = events.len() as i64;

    // Category counts
    let mut cat_map = std::collections::HashMap::<String, i64>::new();
    for ev in &events {
        *cat_map.entry(ev.category.clone()).or_insert(0) += 1;
    }
    let mut top_categories: Vec<CategoryCount> = cat_map
        .into_iter()
        .map(|(category, count)| CategoryCount { category, count })
        .collect();
    top_categories.sort_by(|a, b| b.count.cmp(&a.count));
    top_categories.truncate(10);

    // Recent events
    let recent_events: Vec<EventSummary> = events
        .iter()
        .take(20)
        .map(|ev| EventSummary {
            event_type: ev.event_type.clone(),
            category: ev.category.clone(),
            value_text: ev.value_text.clone(),
            created_at: ev.created_at.clone(),
        })
        .collect();

    // Session stats
    let sessions = db.list_sessions(1000).map_err(|e| e.to_string())?;
    let total_sessions = sessions.len() as i64;
    let total_messages_sent: i64 = sessions.iter().map(|s| s.total_messages as i64).sum();
    let total_tokens_est: i64 = sessions.iter().map(|s| s.total_tokens_est).sum();

    // Average session duration
    let avg_session_duration_min = if !sessions.is_empty() {
        let durations: Vec<f64> = sessions
            .iter()
            .filter_map(|s| {
                let start = chrono::DateTime::parse_from_rfc3339(&s.started_at).ok()?;
                let end = chrono::DateTime::parse_from_rfc3339(s.ended_at.as_deref()?).ok()?;
                Some((end - start).num_seconds() as f64 / 60.0)
            })
            .collect();
        if durations.is_empty() {
            0.0
        } else {
            durations.iter().sum::<f64>() / durations.len() as f64
        }
    } else {
        0.0
    };

    // Skill & memory counts
    let skills = db.list_skills(10000).map_err(|e| e.to_string())?;
    let skill_usage_count: i64 = skills.iter().map(|s| s.usage_count as i64).sum();

    let memory_entries = db
        .list_memory_entries("agent", None, 10000)
        .map_err(|e| e.to_string())?;
    let memory_entry_count = memory_entries.len() as i64;

    Ok(InsightsSummary {
        total_events,
        total_sessions,
        total_messages_sent,
        total_tokens_est,
        avg_session_duration_min,
        top_categories,
        recent_events,
        skill_usage_count,
        memory_entry_count,
    })
}
