use std::collections::{BTreeMap, BTreeSet};

use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    Json,
};
use cowork_contracts::{
    ProjectRole, ServerMcpBindingRecord, SetServerMcpBindingRequest, SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{postgres::PgRow, PgPool, Row};
use uuid::Uuid;

use crate::{
    auth::Principal,
    error::ApiError,
    organization,
    storage::{ObjectStore, SealedValue},
    AppState,
};

const MAX_NAME_BYTES: usize = 256;
const MAX_COMMAND_BYTES: usize = 32 * 1024;
const MAX_ARGUMENTS: usize = 256;
const MAX_ARGUMENT_BYTES: usize = 64 * 1024;
const MAX_ENVIRONMENT: usize = 64;
const MAX_ENVIRONMENT_KEY_BYTES: usize = 256;
const MAX_ENVIRONMENT_VALUE_BYTES: usize = 64 * 1024;
const MAX_ENCODED_BINDING_BYTES: usize = 512 * 1024;
const MAX_CREW_MCP_SERVERS: usize = 64;

#[derive(Debug, Deserialize)]
pub struct DeleteBindingQuery {
    expected_revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredServerMcpBinding {
    name: String,
    command: String,
    args: Vec<String>,
    environment: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedServerMcpBinding {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub environment: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FrozenMcpReference {
    entity_id: Uuid,
    name: String,
}

pub async fn list(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<ServerMcpBindingRecord>>, ApiError> {
    organization::ensure_project_role(
        &state.pool,
        principal.user_id,
        project_id,
        ProjectRole::Viewer,
    )
    .await?;
    let rows = sqlx::query(
        "SELECT * FROM server_mcp_bindings WHERE project_id = $1 ORDER BY name, mcp_entity_id",
    )
    .bind(project_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.iter()
            .map(row_to_record)
            .collect::<Result<Vec<_>, _>>()?,
    ))
}

pub async fn set(
    State(state): State<AppState>,
    Path((project_id, mcp_entity_id)): Path<(Uuid, Uuid)>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<SetServerMcpBindingRequest>,
) -> Result<Json<ServerMcpBindingRecord>, ApiError> {
    organization::ensure_project_role(
        &state.pool,
        principal.user_id,
        project_id,
        ProjectRole::Editor,
    )
    .await?;
    ensure_owned_mcp_metadata(&state.pool, principal.user_id, mcp_entity_id, &request.name).await?;
    let expected_revision = request.expected_revision;
    let binding = validated_binding(request)?;
    let encoded = serde_json::to_vec(&binding)?;
    if encoded.len() > MAX_ENCODED_BINDING_BYTES {
        return Err(ApiError::Unprocessable(format!(
            "MCP binding exceeds {MAX_ENCODED_BINDING_BYTES} encoded bytes"
        )));
    }
    let (owner_user_id, team_id) = project_key_scope(&state.pool, project_id).await?;
    let store = state.object_store.as_ref().ok_or_else(|| {
        ApiError::Conflict("encrypted MCP binding storage is not configured".to_owned())
    })?;
    let sealed = team_id
        .map(|team_id| store.seal_for_team(team_id, &encoded))
        .unwrap_or_else(|| store.seal_for_user(owner_user_id, &encoded))?;
    let executable_hint = executable_hint(&binding.command);
    let environment_keys = Value::Array(
        binding
            .environment
            .keys()
            .cloned()
            .map(Value::String)
            .collect(),
    );
    let argument_count =
        i32::try_from(binding.args.len()).map_err(|error| ApiError::Internal(error.into()))?;

    let mut tx = state.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("server-mcp-bindings:{project_id}"))
        .execute(&mut *tx)
        .await?;
    let duplicate_name = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM server_mcp_bindings WHERE project_id = $1 AND name = $2 AND mcp_entity_id <> $3)",
    )
    .bind(project_id)
    .bind(&binding.name)
    .bind(mcp_entity_id)
    .fetch_one(&mut *tx)
    .await?;
    if duplicate_name {
        return Err(ApiError::Conflict(format!(
            "another MCP binding in this project already uses the name {:?}",
            binding.name
        )));
    }
    let current_revision = sqlx::query_scalar::<_, i64>(
        "SELECT revision FROM server_mcp_bindings WHERE project_id = $1 AND mcp_entity_id = $2 FOR UPDATE",
    )
    .bind(project_id)
    .bind(mcp_entity_id)
    .fetch_optional(&mut *tx)
    .await?;
    let next_revision = match (current_revision, expected_revision) {
        (None, None) => 1,
        (None, Some(_)) => {
            return Err(ApiError::Conflict(
                "MCP binding does not exist; create it without expected_revision".to_owned(),
            ));
        }
        (Some(_), None) => {
            return Err(ApiError::Conflict(
                "MCP binding already exists; reload it before updating".to_owned(),
            ));
        }
        (Some(current), Some(expected)) if current == expected => current + 1,
        (Some(_), Some(_)) => {
            return Err(ApiError::Conflict(
                "MCP binding revision changed; reload before updating".to_owned(),
            ));
        }
    };
    let etag = binding_etag(project_id, mcp_entity_id, next_revision);
    let row = if current_revision.is_none() {
        sqlx::query(
            r#"
            INSERT INTO server_mcp_bindings (
                project_id, mcp_entity_id, revision, etag, owner_user_id, team_id,
                name, executable_hint, argument_count, environment_keys,
                encrypted_binding, encrypted_data_key, binding_nonce, binding_wrap_nonce
            ) VALUES ($1, $2, 1, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            RETURNING *
            "#,
        )
        .bind(project_id)
        .bind(mcp_entity_id)
        .bind(etag)
        .bind(owner_user_id)
        .bind(team_id)
        .bind(&binding.name)
        .bind(executable_hint)
        .bind(argument_count)
        .bind(environment_keys)
        .bind(&sealed.ciphertext)
        .bind(&sealed.encrypted_data_key)
        .bind(sealed.nonce.as_slice())
        .bind(sealed.wrap_nonce.as_slice())
        .fetch_one(&mut *tx)
        .await?
    } else {
        sqlx::query(
            r#"
            UPDATE server_mcp_bindings SET
                revision = $3, etag = $4, owner_user_id = $5, team_id = $6,
                name = $7, executable_hint = $8, argument_count = $9,
                environment_keys = $10, encrypted_binding = $11,
                encrypted_data_key = $12, binding_nonce = $13,
                binding_wrap_nonce = $14, updated_at = now()
            WHERE project_id = $1 AND mcp_entity_id = $2
            RETURNING *
            "#,
        )
        .bind(project_id)
        .bind(mcp_entity_id)
        .bind(next_revision)
        .bind(etag)
        .bind(owner_user_id)
        .bind(team_id)
        .bind(&binding.name)
        .bind(executable_hint)
        .bind(argument_count)
        .bind(environment_keys)
        .bind(&sealed.ciphertext)
        .bind(&sealed.encrypted_data_key)
        .bind(sealed.nonce.as_slice())
        .bind(sealed.wrap_nonce.as_slice())
        .fetch_one(&mut *tx)
        .await?
    };
    tx.commit().await?;
    Ok(Json(row_to_record(&row)?))
}

pub async fn delete(
    State(state): State<AppState>,
    Path((project_id, mcp_entity_id)): Path<(Uuid, Uuid)>,
    Extension(principal): Extension<Principal>,
    Query(query): Query<DeleteBindingQuery>,
) -> Result<StatusCode, ApiError> {
    organization::ensure_project_role(
        &state.pool,
        principal.user_id,
        project_id,
        ProjectRole::Editor,
    )
    .await?;
    let deleted = sqlx::query(
        "DELETE FROM server_mcp_bindings WHERE project_id = $1 AND mcp_entity_id = $2 AND revision = $3",
    )
    .bind(project_id)
    .bind(mcp_entity_id)
    .bind(query.expected_revision)
    .execute(&state.pool)
    .await?
    .rows_affected();
    if deleted == 0 {
        return Err(ApiError::Conflict(
            "MCP binding revision changed or the binding no longer exists".to_owned(),
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn ensure_server_bindings_for_run(
    pool: &PgPool,
    project_id: Uuid,
    input: &Value,
) -> Result<(), ApiError> {
    let references = frozen_mcp_references(input)?;
    if references.is_empty() {
        return Ok(());
    }
    let entity_ids = references
        .iter()
        .map(|reference| reference.entity_id)
        .collect::<Vec<_>>();
    let bound = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT mcp_entity_id, name FROM server_mcp_bindings WHERE project_id = $1 AND mcp_entity_id = ANY($2)",
    )
    .bind(project_id)
    .bind(&entity_ids)
    .fetch_all(pool)
    .await?;
    let bound = bound
        .into_iter()
        .collect::<std::collections::HashMap<_, _>>();
    for reference in references {
        match bound.get(&reference.entity_id) {
            None => {
                return Err(ApiError::Unprocessable(format!(
                    "MCP metadata {} has no encrypted Linux server binding for this project",
                    reference.entity_id
                )));
            }
            Some(name) if name != &reference.name => {
                return Err(ApiError::Unprocessable(format!(
                    "MCP metadata {} was renamed; replace its Linux server binding before running it",
                    reference.entity_id
                )));
            }
            Some(_) => {}
        }
    }
    Ok(())
}

pub(crate) fn run_selects_mcp(input: &Value) -> bool {
    input
        .get("frozen_runtime_context")
        .and_then(|context| context.get("mcp_metadata"))
        .and_then(Value::as_array)
        .is_some_and(|values| !values.is_empty())
}

pub(crate) fn ensure_crew_mcp_selection(input: &Value) -> Result<(), ApiError> {
    if input.get("task_runner").and_then(Value::as_str) != Some("crew") {
        return Ok(());
    }
    let requested = crew_requested_mcp_names(input)?
        .into_iter()
        .collect::<BTreeSet<_>>();
    let selected = frozen_mcp_references(input)?
        .into_iter()
        .map(|reference| reference.name)
        .collect::<BTreeSet<_>>();
    if requested == selected {
        return Ok(());
    }
    let missing = requested.difference(&selected).cloned().collect::<Vec<_>>();
    let unused = selected.difference(&requested).cloned().collect::<Vec<_>>();
    Err(ApiError::Unprocessable(format!(
        "Crew MCP selection must exactly match enabled agent mcpServerNames; missing selections: {}; selected but unused: {}",
        display_names(&missing),
        display_names(&unused),
    )))
}

pub(crate) fn crew_requested_mcp_names(input: &Value) -> Result<Vec<String>, ApiError> {
    if input.get("task_runner").and_then(Value::as_str) != Some("crew") {
        return Ok(Vec::new());
    }
    let agents = input
        .get("crew_definition")
        .and_then(|definition| definition.get("agents"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ApiError::Unprocessable(
                "the frozen Crew definition must contain an agents array".to_owned(),
            )
        })?;
    let mut names = BTreeSet::new();
    for agent in agents {
        if agent.get("enabled").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        let Some(values) = agent.get("mcpServerNames") else {
            continue;
        };
        let values = values.as_array().ok_or_else(|| {
            ApiError::Unprocessable("Crew agent mcpServerNames must be an array".to_owned())
        })?;
        for value in values {
            let name = value
                .as_str()
                .map(str::trim)
                .filter(|name| {
                    !name.is_empty()
                        && name.len() <= MAX_NAME_BYTES
                        && !name.chars().any(char::is_control)
                })
                .ok_or_else(|| {
                    ApiError::Unprocessable(
                        "Crew agent mcpServerNames contains an invalid name".to_owned(),
                    )
                })?;
            names.insert(name.to_owned());
            if names.len() > MAX_CREW_MCP_SERVERS {
                return Err(ApiError::Unprocessable(format!(
                    "Crew definitions may reference at most {MAX_CREW_MCP_SERVERS} MCP servers"
                )));
            }
        }
    }
    Ok(names.into_iter().collect())
}

fn display_names(names: &[String]) -> String {
    if names.is_empty() {
        "none".to_owned()
    } else {
        names
            .iter()
            .map(|name| format!("{name:?}"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

pub(crate) async fn resolve_server_bindings_for_run(
    pool: &PgPool,
    store: Option<&ObjectStore>,
    project_id: Uuid,
    input: &Value,
) -> anyhow::Result<Vec<ResolvedServerMcpBinding>> {
    let references = frozen_mcp_references(input)?;
    if references.is_empty() {
        return Ok(Vec::new());
    }
    let entity_ids = references
        .iter()
        .map(|reference| reference.entity_id)
        .collect::<Vec<_>>();
    let store =
        store.ok_or_else(|| anyhow::anyhow!("encrypted MCP binding storage is unavailable"))?;
    let rows = sqlx::query(
        "SELECT * FROM server_mcp_bindings WHERE project_id = $1 AND mcp_entity_id = ANY($2)",
    )
    .bind(project_id)
    .bind(&entity_ids)
    .fetch_all(pool)
    .await?;
    let mut resolved = std::collections::HashMap::new();
    for row in rows {
        let mcp_entity_id: Uuid = row.try_get("mcp_entity_id")?;
        let sealed = sealed_from_row(&row)?;
        let plaintext = if let Some(team_id) = row.try_get::<Option<Uuid>, _>("team_id")? {
            store.open_for_team(team_id, &sealed)?
        } else {
            store.open_for_user(row.try_get("owner_user_id")?, &sealed)?
        };
        let binding: StoredServerMcpBinding = serde_json::from_slice(&plaintext)?;
        validate_stored_binding(&binding).map_err(|error| anyhow::anyhow!(error.to_string()))?;
        let expected_name = references
            .iter()
            .find(|reference| reference.entity_id == mcp_entity_id)
            .map(|reference| reference.name.as_str())
            .ok_or_else(|| anyhow::anyhow!("unexpected MCP binding {mcp_entity_id}"))?;
        anyhow::ensure!(
            binding.name == expected_name,
            "MCP metadata {mcp_entity_id} was renamed after its Linux binding was created"
        );
        resolved.insert(
            mcp_entity_id,
            ResolvedServerMcpBinding {
                name: binding.name,
                command: binding.command,
                args: binding.args,
                environment: binding.environment,
            },
        );
    }
    references
        .iter()
        .map(|reference| {
            resolved.remove(&reference.entity_id).ok_or_else(|| {
                anyhow::anyhow!(
                    "MCP metadata {} has no encrypted Linux server binding",
                    reference.entity_id
                )
            })
        })
        .collect()
}

fn validated_binding(
    request: SetServerMcpBindingRequest,
) -> Result<StoredServerMcpBinding, ApiError> {
    let binding = StoredServerMcpBinding {
        name: request.name.trim().to_owned(),
        command: request.command.trim().to_owned(),
        args: request.args,
        environment: request.environment,
    };
    validate_stored_binding(&binding)?;
    Ok(binding)
}

fn validate_stored_binding(binding: &StoredServerMcpBinding) -> Result<(), ApiError> {
    if binding.name.is_empty() || binding.name.len() > MAX_NAME_BYTES {
        return Err(ApiError::Unprocessable(format!(
            "MCP binding name must contain 1 to {MAX_NAME_BYTES} bytes"
        )));
    }
    if binding.command.is_empty()
        || binding.command.len() > MAX_COMMAND_BYTES
        || binding.command.contains('\0')
    {
        return Err(ApiError::Unprocessable(
            "MCP command is missing or invalid".to_owned(),
        ));
    }
    if binding.args.len() > MAX_ARGUMENTS
        || binding
            .args
            .iter()
            .any(|argument| argument.len() > MAX_ARGUMENT_BYTES || argument.contains('\0'))
    {
        return Err(ApiError::Unprocessable(
            "MCP arguments exceed the server safety limits".to_owned(),
        ));
    }
    if binding.environment.len() > MAX_ENVIRONMENT
        || binding.environment.iter().any(|(key, value)| {
            let upper_key = key.to_ascii_uppercase();
            key.is_empty()
                || key.len() > MAX_ENVIRONMENT_KEY_BYTES
                || matches!(
                    upper_key.as_str(),
                    "HTTP_PROXY" | "HTTPS_PROXY" | "ALL_PROXY" | "NO_PROXY"
                )
                || key.chars().any(|character| {
                    character == '=' || character == '\0' || character.is_control()
                })
                || value.len() > MAX_ENVIRONMENT_VALUE_BYTES
                || value.contains('\0')
        })
    {
        return Err(ApiError::Unprocessable(
            "MCP environment exceeds the server safety limits".to_owned(),
        ));
    }
    Ok(())
}

async fn ensure_owned_mcp_metadata(
    pool: &PgPool,
    user_id: Uuid,
    entity_id: Uuid,
    requested_name: &str,
) -> Result<(), ApiError> {
    let payload = sqlx::query_scalar::<_, Value>(
        "SELECT payload FROM sync_entities WHERE user_id = $1 AND entity_type = 'mcp_metadata' AND entity_id = $2 AND NOT tombstone",
    )
    .bind(user_id)
    .bind(entity_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| {
        ApiError::Unprocessable(format!(
            "MCP metadata {entity_id} is unavailable or deleted for the current user"
        ))
    })?;
    let metadata_name = payload
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if metadata_name.is_empty() || metadata_name != requested_name.trim() {
        return Err(ApiError::Unprocessable(
            "MCP binding name must exactly match the synchronized metadata name".to_owned(),
        ));
    }
    Ok(())
}

async fn project_key_scope(
    pool: &PgPool,
    project_id: Uuid,
) -> Result<(Uuid, Option<Uuid>), ApiError> {
    sqlx::query_as::<_, (Uuid, Option<Uuid>)>(
        "SELECT owner_user_id, team_id FROM projects WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(project_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("project {project_id} was not found")))
}

fn frozen_mcp_references(input: &Value) -> Result<Vec<FrozenMcpReference>, ApiError> {
    let Some(values) = input
        .get("frozen_runtime_context")
        .and_then(|context| context.get("mcp_metadata"))
        .and_then(Value::as_array)
    else {
        return Ok(Vec::new());
    };
    values
        .iter()
        .map(|value| {
            let entity_id = value
                .get("entity_id")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    ApiError::Unprocessable(
                        "frozen MCP metadata is missing its entity_id".to_owned(),
                    )
                })?
                .parse::<Uuid>()
                .map_err(|_| {
                    ApiError::Unprocessable(
                        "frozen MCP metadata contains an invalid entity_id".to_owned(),
                    )
                })?;
            let name = value
                .get("definition")
                .and_then(|definition| definition.get("name"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .ok_or_else(|| {
                    ApiError::Unprocessable("frozen MCP metadata is missing its name".to_owned())
                })?;
            Ok(FrozenMcpReference {
                entity_id,
                name: name.to_owned(),
            })
        })
        .collect()
}

fn row_to_record(row: &PgRow) -> Result<ServerMcpBindingRecord, ApiError> {
    let environment_keys: Value = row.try_get("environment_keys")?;
    let environment_keys = environment_keys
        .as_array()
        .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("invalid MCP environment metadata")))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(ToOwned::to_owned)
                .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("invalid MCP environment key")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ServerMcpBindingRecord {
        schema_version: SCHEMA_VERSION,
        project_id: row.try_get("project_id")?,
        mcp_entity_id: row.try_get("mcp_entity_id")?,
        revision: row.try_get("revision")?,
        etag: row.try_get("etag")?,
        name: row.try_get("name")?,
        transport: row.try_get("transport")?,
        executable_hint: row.try_get("executable_hint")?,
        argument_count: u32::try_from(row.try_get::<i32, _>("argument_count")?)
            .map_err(|error| ApiError::Internal(error.into()))?,
        environment_keys,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn sealed_from_row(row: &PgRow) -> Result<SealedValue, ApiError> {
    Ok(SealedValue {
        ciphertext: row.try_get("encrypted_binding")?,
        encrypted_data_key: row.try_get("encrypted_data_key")?,
        nonce: fixed_nonce(row.try_get("binding_nonce")?, "MCP binding nonce")?,
        wrap_nonce: fixed_nonce(row.try_get("binding_wrap_nonce")?, "MCP binding wrap nonce")?,
    })
}

fn fixed_nonce(value: Vec<u8>, name: &str) -> Result<[u8; 12], ApiError> {
    value
        .try_into()
        .map_err(|_| ApiError::Internal(anyhow::anyhow!("{name} has an invalid persisted length")))
}

fn executable_hint(command: &str) -> String {
    command
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn binding_etag(project_id: Uuid, entity_id: Uuid, revision: i64) -> String {
    format!("W/\"{project_id}:{entity_id}:{revision}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_validation_rejects_shell_nuls_and_invalid_environment_names() {
        let valid = StoredServerMcpBinding {
            name: "Docs".to_owned(),
            command: "/opt/mcp/docs-server".to_owned(),
            args: vec!["--stdio".to_owned()],
            environment: BTreeMap::from([("MCP_TOKEN".to_owned(), "secret".to_owned())]),
        };
        assert!(validate_stored_binding(&valid).is_ok());
        let mut invalid = valid.clone();
        invalid.command.push('\0');
        assert!(validate_stored_binding(&invalid).is_err());
        let mut invalid = valid;
        invalid
            .environment
            .insert("BAD=NAME".to_owned(), "value".to_owned());
        assert!(validate_stored_binding(&invalid).is_err());
    }

    #[test]
    fn frozen_mcp_references_are_exact_and_ordered() {
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        assert_eq!(
            frozen_mcp_references(&serde_json::json!({
                "frozen_runtime_context":{"mcp_metadata":[
                    {"entity_id":first,"revision":1,"definition":{"name":"one"}},
                    {"entity_id":second,"revision":2,"definition":{"name":"two"}},
                ]}
            }))
            .unwrap(),
            vec![
                FrozenMcpReference {
                    entity_id: first,
                    name: "one".to_owned()
                },
                FrozenMcpReference {
                    entity_id: second,
                    name: "two".to_owned()
                },
            ]
        );
        assert!(frozen_mcp_references(&serde_json::json!({
            "frozen_runtime_context":{"mcp_metadata":[{"entity_id":"not-a-uuid"}]}
        }))
        .is_err());
        assert!(frozen_mcp_references(&serde_json::json!({
            "frozen_runtime_context":{"mcp_metadata":[{"entity_id":first,"definition":{}}]}
        }))
        .is_err());
    }

    #[test]
    fn crew_mcp_selection_is_exact_and_ignores_disabled_agents() {
        let selected = Uuid::new_v4();
        let base = serde_json::json!({
            "task_runner":"crew",
            "crew_definition":{"agents":[
                {"id":"active","mcpServerNames":["Docs"]},
                {"id":"disabled","enabled":false,"mcpServerNames":["Ignored"]}
            ]},
            "frozen_runtime_context":{"mcp_metadata":[
                {"entity_id":selected,"definition":{"name":"Docs"}}
            ]}
        });
        assert_eq!(crew_requested_mcp_names(&base).unwrap(), vec!["Docs"]);
        ensure_crew_mcp_selection(&base).unwrap();

        let mut missing = base.clone();
        missing["frozen_runtime_context"]["mcp_metadata"] = serde_json::json!([]);
        assert!(ensure_crew_mcp_selection(&missing)
            .unwrap_err()
            .to_string()
            .contains("missing selections"));

        let mut invalid = base;
        invalid["crew_definition"]["agents"][0]["mcpServerNames"] = serde_json::json!("Docs");
        assert!(crew_requested_mcp_names(&invalid).is_err());
    }
}
