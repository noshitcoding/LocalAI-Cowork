use crate::db::{Database, SkillRow};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ── Request / Response types ────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct SkillCreateRequest {
    pub name: String,
    pub description: String,
    pub prompt_template: String,
    pub trigger_pattern: Option<String>,
    pub run_mode: Option<String>,
    pub source_task_ids: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct SkillImproveRequest {
    pub skill_id: String,
    pub new_prompt_template: String,
    pub reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct SkillUsageReport {
    pub skill_id: String,
    pub success: bool,
    pub quality: Option<f64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillAutoGenResult {
    pub generated: bool,
    pub skill_name: Option<String>,
    pub reason: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct SkillImproveResult {
    pub skill_id: String,
    pub new_version: i32,
    pub reason: String,
}

// ── Auto-generation logic ───────────────────────────────────────────────────

/// Analyze completed tasks and check if a new skill should be auto-generated
pub fn analyze_for_skill_generation(
    db: &Arc<Database>,
    task_title: &str,
    _task_prompt: &str,
    task_steps_summary: &str,
    task_outcome: &str,
) -> SkillAutoGenResult {
    // Check if a similar skill already exists by looking at name patterns
    let candidate_name = derive_skill_name(task_title);
    if let Ok(Some(_)) = db.get_skill_by_name(&candidate_name) {
        return SkillAutoGenResult {
            generated: false,
            skill_name: None,
            reason: format!("Skill '{}' already exists", candidate_name),
        };
    }

    // Simple heuristic: if task had >3 steps and succeeded, it's worth capturing
    let step_count = task_steps_summary.matches('\n').count() + 1;
    if step_count < 3 || task_outcome != "completed" {
        return SkillAutoGenResult {
            generated: false,
            skill_name: None,
            reason: "Task too simple or not successful".to_string(),
        };
    }

    // Generate prompt template from task
    let prompt_template = format!(
        "Run the following task based on the pattern from '{}': {{{{input}}}}\n\nProven steps:\n{}",
        task_title, task_steps_summary
    );

    let id = uuid::Uuid::new_v4().to_string();
    let trigger = format!("*{}*", candidate_name.to_lowercase());

    match db.upsert_skill(
        &id,
        &candidate_name,
        &format!("Auto-generated from task: {}", task_title),
        &prompt_template,
        Some(&trigger),
        "execute",
        true,
        None,
        None,
    ) {
        Ok(_) => SkillAutoGenResult {
            generated: true,
            skill_name: Some(candidate_name),
            reason: format!("Skill auto-generiert aus {} Schritten", step_count),
        },
        Err(e) => SkillAutoGenResult {
            generated: false,
            skill_name: None,
            reason: format!("Fehler bei Skill-Erstellung: {}", e),
        },
    }
}

/// Analyze skill performance and suggest improvements
#[allow(dead_code)]
pub fn analyze_for_improvement(db: &Arc<Database>, skill_id: &str) -> Option<String> {
    let skills = db.list_skills(200).ok()?;
    let skill = skills.into_iter().find(|s| s.id == skill_id)?;

    if skill.usage_count < 5 {
        return None; // Not enough data
    }

    let success_rate = if skill.usage_count > 0 {
        skill.success_count as f64 / skill.usage_count as f64
    } else {
        0.0
    };

    if success_rate < 0.6 {
        Some(format!(
            "Skill '{}' hat nur {:.0}% Erfolgsrate bei {} Verwendungen. Verbesserung empfohlen.",
            skill.name,
            success_rate * 100.0,
            skill.usage_count
        ))
    } else if skill.avg_quality < 0.5 && skill.usage_count >= 10 {
        Some(format!(
            "Skill '{}' hat low average quality ({:.2}). prompt optimization recommended.",
            skill.name, skill.avg_quality
        ))
    } else {
        None
    }
}

/// Match user input to available skills by trigger pattern
pub fn match_skill_for_input(db: &Arc<Database>, user_input: &str) -> Option<SkillRow> {
    let skills = db.list_skills(200).ok()?;
    let input_lower = user_input.to_lowercase();

    for skill in skills {
        if let Some(ref pattern) = skill.trigger_pattern {
            if simple_pattern_match(pattern, &input_lower) {
                return Some(skill);
            }
        }
    }

    None
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn derive_skill_name(task_title: &str) -> String {
    let cleaned: String = task_title
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-')
        .collect();
    let words: Vec<&str> = cleaned.split_whitespace().take(4).collect();
    if words.is_empty() {
        "auto-skill".to_string()
    } else {
        words.join("-").to_lowercase()
    }
}

fn simple_pattern_match(pattern: &str, text: &str) -> bool {
    let pattern_lower = pattern.to_lowercase();
    if !pattern_lower.contains('*') {
        return pattern_lower == text;
    }

    let parts: Vec<&str> = pattern_lower.split('*').collect();
    let mut remainder = text.to_string();

    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 && !pattern_lower.starts_with('*') {
            if !remainder.starts_with(part) {
                return false;
            }
            remainder = remainder[part.len()..].to_string();
        } else if i == parts.len() - 1 && !pattern_lower.ends_with('*') {
            if !remainder.ends_with(part) {
                return false;
            }
        } else if let Some(pos) = remainder.find(part) {
            remainder = remainder[pos + part.len()..].to_string();
        } else {
            return false;
        }
    }

    true
}
