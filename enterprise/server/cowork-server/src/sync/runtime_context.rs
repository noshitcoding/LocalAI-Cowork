//! Freezes permitted sync metadata and crew provider references into a run context.

use crate::{error::ApiError, sync::validation::MAX_SYNC_PAYLOAD_BYTES};
use serde_json::Value;
use sqlx::{PgPool, Row};
use uuid::Uuid;

pub(crate) async fn freeze_runtime_context_for_run(
    pool: &PgPool,
    user_id: Uuid,
    input: Value,
) -> Result<Value, ApiError> {
    const CONTEXT_TYPES: &[(&str, &str, &[&str])] = &[
        (
            "skill_ids",
            "skill",
            &[
                "name",
                "description",
                "prompt_template",
                "trigger_pattern",
                "run_mode",
                "auto_generated",
                "parent_skill_id",
                "source_task_ids",
            ],
        ),
        (
            "memory_ids",
            "memory",
            &[
                "scope",
                "scope_ref",
                "category",
                "key",
                "content",
                "target",
                "source_run_id",
                "confidence",
                "source",
            ],
        ),
        (
            "mcp_metadata_ids",
            "mcp_metadata",
            &[
                "name",
                "transport",
                "executable_hint",
                "environment_keys",
                "device_binding_required",
                "source",
            ],
        ),
    ];
    let mut input = strip_client_frozen_fields(input);
    input = freeze_crew_definition_for_run(pool, user_id, input).await?;
    let mut frozen = serde_json::Map::new();
    for (reference_key, entity_type, allowed_fields) in CONTEXT_TYPES {
        let references = runtime_metadata_references(&input, reference_key)?;
        if references.is_empty() {
            continue;
        }
        let values =
            freeze_metadata_entities(pool, user_id, entity_type, &references, allowed_fields)
                .await?;
        frozen.insert((*entity_type).to_owned(), Value::Array(values));
    }
    if frozen.is_empty() {
        return Ok(input);
    }
    let encoded = serde_json::to_vec(&frozen)?;
    if encoded.len() > 256 * 1024 {
        return Err(ApiError::Unprocessable(
            "selected synchronized runtime context exceeds 256 KiB".to_owned(),
        ));
    }
    input
        .as_object_mut()
        .expect("metadata references can only originate from object run input")
        .insert("frozen_runtime_context".to_owned(), Value::Object(frozen));
    Ok(input)
}

fn strip_client_frozen_fields(input: Value) -> Value {
    match input {
        Value::Object(mut object) => {
            object.remove("frozen_runtime_context");
            object.remove("crew_definition");
            object.remove("crew_entity_id");
            object.remove("crew_revision");
            Value::Object(object)
        }
        other => other,
    }
}

fn runtime_metadata_references(input: &Value, key: &str) -> Result<Vec<String>, ApiError> {
    let value = input.get(key).or_else(|| {
        input.get("task_config").and_then(|config| {
            config.get(key).or_else(|| {
                config
                    .get("sync_metadata")
                    .and_then(|metadata| metadata.get(key))
            })
        })
    });
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let values = value.as_array().ok_or_else(|| {
        ApiError::Unprocessable(format!("{key} must be an array of synchronized entity IDs"))
    })?;
    if values.len() > 32 {
        return Err(ApiError::Unprocessable(format!(
            "{key} cannot contain more than 32 entries"
        )));
    }
    let mut unique = std::collections::HashSet::new();
    values
        .iter()
        .map(|value| {
            let reference = value
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty() && value.len() <= 200)
                .ok_or_else(|| {
                    ApiError::Unprocessable(format!(
                        "{key} entries must contain 1 to 200 characters"
                    ))
                })?;
            if !unique.insert(reference.to_owned()) {
                return Ok(None);
            }
            Ok(Some(reference.to_owned()))
        })
        .filter_map(|result| match result {
            Ok(Some(value)) => Some(Ok(value)),
            Ok(None) => None,
            Err(error) => Some(Err(error)),
        })
        .collect()
}

async fn freeze_metadata_entities(
    pool: &PgPool,
    user_id: Uuid,
    entity_type: &str,
    references: &[String],
    allowed_fields: &[&str],
) -> Result<Vec<Value>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT entity_id, revision, payload
        FROM sync_entities
        WHERE user_id = $1 AND entity_type = $2 AND NOT tombstone
          AND (
            entity_id::text = ANY($3)
            OR payload ->> '_cowork_local_entity_id' = ANY($3)
          )
        "#,
    )
    .bind(user_id)
    .bind(entity_type)
    .bind(references)
    .fetch_all(pool)
    .await?;
    let mut by_reference = std::collections::HashMap::new();
    for row in rows {
        let entity_id: Uuid = row.try_get("entity_id")?;
        let revision: i64 = row.try_get("revision")?;
        let payload: Value = row.try_get("payload")?;
        let local_id = payload
            .get("_cowork_local_entity_id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let definition = sanitized_runtime_metadata(payload, entity_type, allowed_fields)?;
        let frozen = serde_json::json!({
            "entity_id": entity_id,
            "revision": revision,
            "definition": definition,
        });
        by_reference.insert(entity_id.to_string(), frozen.clone());
        if let Some(local_id) = local_id {
            by_reference.insert(local_id, frozen);
        }
    }
    references
        .iter()
        .map(|reference| {
            by_reference.get(reference).cloned().ok_or_else(|| {
                ApiError::Unprocessable(format!(
                    "{entity_type} metadata {reference} is unavailable or deleted"
                ))
            })
        })
        .collect()
}

fn sanitized_runtime_metadata(
    payload: Value,
    entity_type: &str,
    allowed_fields: &[&str],
) -> Result<Value, ApiError> {
    let object = payload.as_object().ok_or_else(|| {
        ApiError::Unprocessable(format!("{entity_type} metadata must be an object"))
    })?;
    let sanitized = allowed_fields
        .iter()
        .filter_map(|key| {
            object
                .get(*key)
                .map(|value| ((*key).to_owned(), value.clone()))
        })
        .collect::<serde_json::Map<_, _>>();
    let required = match entity_type {
        "skill" => &["name", "prompt_template"][..],
        "memory" => &["key", "content"][..],
        "mcp_metadata" => &["name"][..],
        _ => &[][..],
    };
    for key in required {
        if sanitized
            .get(*key)
            .and_then(Value::as_str)
            .is_none_or(|value| value.trim().is_empty())
        {
            return Err(ApiError::Unprocessable(format!(
                "{entity_type} metadata requires {key}"
            )));
        }
    }
    Ok(Value::Object(sanitized))
}

async fn freeze_crew_definition_for_run(
    pool: &PgPool,
    user_id: Uuid,
    input: Value,
) -> Result<Value, ApiError> {
    if input.get("task_runner").and_then(Value::as_str) != Some("crew") {
        return Ok(input);
    }
    let crew_id = input
        .get("crew_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::Unprocessable("crew run is missing crew_id".to_owned()))?;
    let embedded = input
        .get("task_config")
        .and_then(|config| {
            config
                .get("crew_definition")
                .or_else(|| config.get("crew_request"))
        })
        .cloned();
    let (entity_id, revision, definition) = if let Some(definition) = embedded {
        (None, None, definition)
    } else {
        let row = sqlx::query(
            r#"
            SELECT entity_id, revision, payload
            FROM sync_entities
            WHERE user_id = $1 AND entity_type = 'crew' AND NOT tombstone
              AND (
                entity_id::text = $2
                OR payload ->> '_cowork_local_entity_id' = $2
                OR payload -> 'definition' ->> 'id' = $2
              )
            ORDER BY updated_at DESC, revision DESC
            LIMIT 1
            "#,
        )
        .bind(user_id)
        .bind(crew_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| {
            ApiError::Unprocessable(format!(
                "crew {crew_id} is unavailable in the user's synchronized metadata"
            ))
        })?;
        let payload: Value = row.try_get("payload")?;
        let definition = payload
            .get("definition")
            .cloned()
            .ok_or_else(|| ApiError::Unprocessable(format!("crew {crew_id} has no definition")))?;
        (
            Some(row.try_get::<Uuid, _>("entity_id")?),
            Some(row.try_get::<i64, _>("revision")?),
            definition,
        )
    };
    let mut definition = sanitized_crew_definition(definition, crew_id)?;
    canonicalize_crew_provider_profile_references(pool, user_id, &mut definition).await?;
    let mut object = input
        .as_object()
        .cloned()
        .expect("frozen task input is always an object");
    object.insert("crew_definition".to_owned(), definition);
    if let Some(entity_id) = entity_id {
        object.insert(
            "crew_entity_id".to_owned(),
            Value::String(entity_id.to_string()),
        );
    }
    if let Some(revision) = revision {
        object.insert("crew_revision".to_owned(), Value::Number(revision.into()));
    }
    Ok(Value::Object(object))
}

fn sanitized_crew_definition(definition: Value, expected_id: &str) -> Result<Value, ApiError> {
    const ALLOWED_FIELDS: &[&str] = &[
        "id",
        "name",
        "description",
        "executionSubject",
        "executionGuidelines",
        "knowledgeFocus",
        "governanceMode",
        "outputMode",
        "stopOnFailure",
        "retryCount",
        "managerReviewEnabled",
        "managerReviewGuidelines",
        "shareAllTaskOutputs",
        "sharedOutputCharLimit",
        "tasks",
        "process",
        "managerAgentId",
        "verbose",
        "maxRpm",
        "maxParallelTasks",
    ];
    let object = definition
        .as_object()
        .ok_or_else(|| ApiError::Unprocessable("crew definition must be an object".to_owned()))?;
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::Unprocessable("crew definition is missing id".to_owned()))?;
    if id != expected_id {
        return Err(ApiError::Unprocessable(format!(
            "crew definition id {id} does not match task crew_id {expected_id}"
        )));
    }
    let mut sanitized = ALLOWED_FIELDS
        .iter()
        .filter_map(|key| {
            object
                .get(*key)
                .map(|value| ((*key).to_owned(), value.clone()))
        })
        .collect::<serde_json::Map<_, _>>();
    if let Some(selection) = object.get("defaultBackendSelection") {
        sanitized.insert(
            "defaultBackendSelection".to_owned(),
            sanitized_crew_backend_selection(selection, "defaultBackendSelection")?,
        );
    }
    let mut agents = sanitized_crew_array(
        object.get("agents"),
        "agents",
        &[
            "id",
            "name",
            "role",
            "goal",
            "backstory",
            "skillsMarkdown",
            "personalityId",
            "modelOverride",
            "backendSelection",
            "tools",
            "mcpServerNames",
            "enabled",
            "allowDelegation",
            "verbose",
            "maxIterations",
            "maxRpm",
        ],
    )?;
    for agent in agents
        .as_array_mut()
        .expect("sanitized Crew agents are always an array")
    {
        let agent = agent
            .as_object_mut()
            .expect("sanitized Crew agents are always objects");
        if let Some(selection) = agent.get("backendSelection").cloned() {
            agent.insert(
                "backendSelection".to_owned(),
                sanitized_crew_backend_selection(&selection, "agent backendSelection")?,
            );
        }
        if agent
            .get("modelOverride")
            .is_some_and(|value| value.as_str().is_none_or(|value| value.len() > 4_096))
        {
            return Err(ApiError::Unprocessable(
                "Crew agent modelOverride must be a string of at most 4096 characters".to_owned(),
            ));
        }
    }
    sanitized.insert("agents".to_owned(), agents);
    sanitized.insert(
        "tasks".to_owned(),
        sanitized_crew_array(
            object.get("tasks"),
            "tasks",
            &[
                "id",
                "title",
                "name",
                "description",
                "expectedOutput",
                "agentId",
                "context",
                "dependencies",
                "asyncExecution",
            ],
        )?,
    );
    let encoded = serde_json::to_vec(&sanitized)?;
    if encoded.len() > MAX_SYNC_PAYLOAD_BYTES {
        return Err(ApiError::Unprocessable(format!(
            "crew definition exceeds {MAX_SYNC_PAYLOAD_BYTES} bytes"
        )));
    }
    Ok(Value::Object(sanitized))
}

fn sanitized_crew_backend_selection(value: &Value, label: &str) -> Result<Value, ApiError> {
    let object = value
        .as_object()
        .ok_or_else(|| ApiError::Unprocessable(format!("Crew {label} must be an object")))?;
    let backend = object
        .get("backend")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("openai-compatible");
    let mut sanitized = serde_json::Map::new();
    sanitized.insert("backend".to_owned(), Value::String(backend.to_owned()));
    match backend {
        "openai-compatible" => {
            let profile_id = object
                .get("profileId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty() && value.len() <= 200)
                .ok_or_else(|| {
                    ApiError::Unprocessable(format!(
                        "Crew {label} requires an OpenAI-compatible profileId of at most 200 characters"
                    ))
                })?;
            sanitized.insert("profileId".to_owned(), Value::String(profile_id.to_owned()));
            copy_optional_crew_selection_string(object, &mut sanitized, "model", 4_096, label)?;
        }
        "codex" => {
            copy_optional_crew_selection_string(
                object,
                &mut sanitized,
                "authProfileId",
                200,
                label,
            )?;
            copy_optional_crew_selection_string(object, &mut sanitized, "model", 4_096, label)?;
            copy_optional_crew_selection_string(
                object,
                &mut sanitized,
                "reasoningEffort",
                100,
                label,
            )?;
        }
        _ => {
            return Err(ApiError::Unprocessable(format!(
                "Crew {label} backend must be openai-compatible or codex"
            )))
        }
    }
    Ok(Value::Object(sanitized))
}

fn copy_optional_crew_selection_string(
    source: &serde_json::Map<String, Value>,
    target: &mut serde_json::Map<String, Value>,
    key: &str,
    max_len: usize,
    label: &str,
) -> Result<(), ApiError> {
    let Some(value) = source.get(key) else {
        return Ok(());
    };
    let value = value
        .as_str()
        .ok_or_else(|| ApiError::Unprocessable(format!("Crew {label} {key} must be a string")))?;
    let value = value.trim();
    if value.len() > max_len {
        return Err(ApiError::Unprocessable(format!(
            "Crew {label} {key} exceeds {max_len} characters"
        )));
    }
    if !value.is_empty() {
        target.insert(key.to_owned(), Value::String(value.to_owned()));
    }
    Ok(())
}

async fn canonicalize_crew_provider_profile_references(
    pool: &PgPool,
    user_id: Uuid,
    definition: &mut Value,
) -> Result<(), ApiError> {
    let references = crew_provider_profile_references(definition)?;
    if references.is_empty() {
        return Ok(());
    }
    if references.len() > 64 {
        return Err(ApiError::Unprocessable(
            "a Crew may select at most 64 provider profiles".to_owned(),
        ));
    }
    let rows = sqlx::query(
        r#"
        SELECT entity_id, payload
        FROM sync_entities
        WHERE user_id = $1 AND entity_type = 'provider_profile' AND NOT tombstone
          AND (
            entity_id::text = ANY($2)
            OR payload ->> '_cowork_local_entity_id' = ANY($2)
          )
        "#,
    )
    .bind(user_id)
    .bind(&references)
    .fetch_all(pool)
    .await?;
    let mut canonical = std::collections::HashMap::new();
    for row in rows {
        let entity_id: Uuid = row.try_get("entity_id")?;
        let payload: Value = row.try_get("payload")?;
        canonical.insert(entity_id.to_string(), entity_id.to_string());
        if let Some(local_id) = payload
            .get("_cowork_local_entity_id")
            .and_then(Value::as_str)
        {
            canonical.insert(local_id.to_owned(), entity_id.to_string());
        }
    }
    for reference in &references {
        if !canonical.contains_key(reference) {
            if let Ok(profile_id) = reference.parse::<Uuid>() {
                canonical.insert(reference.clone(), profile_id.to_string());
            } else {
                return Err(ApiError::Unprocessable(format!(
                    "Crew provider profile {reference} is unavailable or deleted"
                )));
            }
        }
    }
    rewrite_crew_profile_references(definition, &canonical);
    Ok(())
}

fn crew_provider_profile_references(definition: &Value) -> Result<Vec<String>, ApiError> {
    let mut references = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let default = definition.get("defaultBackendSelection");
    let agents = definition
        .get("agents")
        .and_then(Value::as_array)
        .ok_or_else(|| ApiError::Unprocessable("Crew definition requires agents".to_owned()))?;
    for selection in default.into_iter().chain(
        agents
            .iter()
            .filter_map(|agent| agent.get("backendSelection")),
    ) {
        if selection.get("backend").and_then(Value::as_str) != Some("openai-compatible") {
            continue;
        }
        let profile_id = selection
            .get("profileId")
            .and_then(Value::as_str)
            .expect("sanitized OpenAI-compatible Crew selections have profileId");
        if seen.insert(profile_id.to_owned()) {
            references.push(profile_id.to_owned());
        }
    }
    Ok(references)
}

fn rewrite_crew_profile_references(
    definition: &mut Value,
    canonical: &std::collections::HashMap<String, String>,
) {
    let Some(object) = definition.as_object_mut() else {
        return;
    };
    if let Some(selection) = object
        .get_mut("defaultBackendSelection")
        .and_then(Value::as_object_mut)
    {
        rewrite_crew_profile_reference(selection, canonical);
    }
    if let Some(agents) = object.get_mut("agents").and_then(Value::as_array_mut) {
        for agent in agents {
            if let Some(selection) = agent
                .get_mut("backendSelection")
                .and_then(Value::as_object_mut)
            {
                rewrite_crew_profile_reference(selection, canonical);
            }
        }
    }
}

fn rewrite_crew_profile_reference(
    selection: &mut serde_json::Map<String, Value>,
    canonical: &std::collections::HashMap<String, String>,
) {
    let Some(profile_id) = selection.get("profileId").and_then(Value::as_str) else {
        return;
    };
    if let Some(canonical_id) = canonical.get(profile_id) {
        selection.insert("profileId".to_owned(), Value::String(canonical_id.clone()));
    }
}

fn sanitized_crew_array(
    value: Option<&Value>,
    name: &str,
    allowed_fields: &[&str],
) -> Result<Value, ApiError> {
    let values = value
        .and_then(Value::as_array)
        .ok_or_else(|| ApiError::Unprocessable(format!("crew definition requires {name}")))?;
    if values.len() > 1_000 {
        return Err(ApiError::Unprocessable(format!(
            "crew definition {name} exceeds 1000 entries"
        )));
    }
    values
        .iter()
        .map(|value| {
            let object = value.as_object().ok_or_else(|| {
                ApiError::Unprocessable(format!("crew definition {name} must contain objects"))
            })?;
            Ok(Value::Object(
                allowed_fields
                    .iter()
                    .filter_map(|key| {
                        object
                            .get(*key)
                            .map(|value| ((*key).to_owned(), value.clone()))
                    })
                    .collect(),
            ))
        })
        .collect::<Result<Vec<_>, ApiError>>()
        .map(Value::Array)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frozen_crew_definitions_keep_secret_free_provider_routing_only() {
        let default_profile = Uuid::new_v4().to_string();
        let agent_profile = Uuid::new_v4().to_string();
        let sanitized = sanitized_crew_definition(
            serde_json::json!({
                "id":"researchers",
                "name":"Research crew",
                "defaultBackendSelection":{
                    "backend":"openai-compatible",
                    "profileId":default_profile,
                    "model":"default-model",
                    "apiKey":"nested-secret"
                },
                "agents":[{"id":"agent","name":"Agent","modelOverride":"agent-model","backendSelection":{
                    "backend":"openai-compatible",
                    "profileId":agent_profile,
                    "model":"agent-profile-model",
                    "headers":{"Authorization":"nested-secret"}
                },"apiKey":"nested-secret"}],
                "tasks":[{"id":"task","agentId":"agent","credential":"nested-secret"}],
                "providerConfigs":{"openAICompatible":{"apiKey":"never-persist"}},
                "config":{"apiKey":"never-persist-either"}
            }),
            "researchers",
        )
        .unwrap();
        assert_eq!(sanitized["id"], "researchers");
        assert!(sanitized.get("providerConfigs").is_none());
        assert!(sanitized.get("config").is_none());
        assert_eq!(
            sanitized["defaultBackendSelection"]["profileId"],
            default_profile
        );
        assert_eq!(
            sanitized["agents"][0]["backendSelection"]["profileId"],
            agent_profile
        );
        assert_eq!(sanitized["agents"][0]["modelOverride"], "agent-model");
        assert!(!sanitized.to_string().contains("never-persist"));
        assert!(!sanitized.to_string().contains("nested-secret"));
    }

    #[test]
    fn crew_provider_references_are_deduplicated_and_rewritten() {
        let first = "local-fast".to_owned();
        let second = "local-deep".to_owned();
        let first_uuid = Uuid::new_v4().to_string();
        let second_uuid = Uuid::new_v4().to_string();
        let mut definition = serde_json::json!({
            "defaultBackendSelection":{"backend":"openai-compatible","profileId":first},
            "agents":[
                {"id":"inherit"},
                {"id":"same","backendSelection":{"backend":"openai-compatible","profileId":"local-fast"}},
                {"id":"deep","backendSelection":{"backend":"openai-compatible","profileId":second}}
            ]
        });
        assert_eq!(
            crew_provider_profile_references(&definition).unwrap(),
            vec!["local-fast", "local-deep"]
        );
        rewrite_crew_profile_references(
            &mut definition,
            &std::collections::HashMap::from([
                ("local-fast".to_owned(), first_uuid.clone()),
                ("local-deep".to_owned(), second_uuid.clone()),
            ]),
        );
        assert_eq!(
            definition["defaultBackendSelection"]["profileId"],
            first_uuid
        );
        assert_eq!(
            definition["agents"][1]["backendSelection"]["profileId"],
            first_uuid
        );
        assert_eq!(
            definition["agents"][2]["backendSelection"]["profileId"],
            second_uuid
        );
    }

    #[test]
    fn runtime_metadata_references_are_nested_deduplicated_and_bounded() {
        let one = Uuid::new_v4().to_string();
        let two = Uuid::new_v4().to_string();
        let input = serde_json::json!({
            "task_config": {
                "sync_metadata": {"skill_ids": [one.clone(), two.clone(), one.clone()]}
            }
        });
        assert_eq!(
            runtime_metadata_references(&input, "skill_ids").unwrap(),
            vec![one, two]
        );
        assert!(runtime_metadata_references(
            &serde_json::json!({"memory_ids": "not-an-array"}),
            "memory_ids"
        )
        .is_err());
        assert!(runtime_metadata_references(
            &serde_json::json!({"memory_ids": (0..33).map(|_| Uuid::new_v4()).collect::<Vec<_>>() }),
            "memory_ids"
        )
        .is_err());
    }

    #[test]
    fn runtime_metadata_allowlist_drops_device_bindings_and_unknown_fields() {
        let sanitized = sanitized_runtime_metadata(
            serde_json::json!({
                "name":"Filesystem",
                "transport":"stdio",
                "executable_hint":"npx",
                "environment_keys":["TOKEN"],
                "command":"npx --unsafe",
                "api_key":"must-not-freeze"
            }),
            "mcp_metadata",
            &["name", "transport", "executable_hint", "environment_keys"],
        )
        .unwrap();
        assert_eq!(sanitized["name"], "Filesystem");
        assert!(sanitized.get("command").is_none());
        assert!(sanitized.get("api_key").is_none());
    }

    #[test]
    fn client_cannot_supply_reserved_frozen_runtime_fields() {
        let stripped = strip_client_frozen_fields(serde_json::json!({
            "prompt":"legitimate",
            "frozen_runtime_context":{"memory":[{"definition":{"content":"untrusted"}}]},
            "crew_definition":{"id":"injected"},
            "crew_entity_id":Uuid::new_v4(),
            "crew_revision":99
        }));
        assert_eq!(stripped["prompt"], "legitimate");
        assert!(stripped.get("frozen_runtime_context").is_none());
        assert!(stripped.get("crew_definition").is_none());
        assert!(stripped.get("crew_entity_id").is_none());
        assert!(stripped.get("crew_revision").is_none());
    }
}
