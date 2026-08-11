use std::{collections::HashSet, time::Duration};

use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    Json,
};
use cowork_contracts::{
    CreateProviderProfileRequest, ExecutorTarget, ProviderProfile, SetProviderProfileSecretRequest,
    UpdateProviderProfileRequest, SCHEMA_VERSION,
};
use serde::Deserialize;
use serde_json::{Map, Value};
use sqlx::{postgres::PgRow, PgPool, Row};
use uuid::Uuid;

use crate::{auth::Principal, error::ApiError, organization, storage::SealedValue, sync, AppState};

const MAX_PROFILE_NAME: usize = 200;
const MAX_API_KEY_BYTES: usize = 32 * 1024;
const MAX_TIMEOUT_MS: u64 = 24 * 60 * 60 * 1_000;
const MAX_MODEL_STEPS: u64 = 256;

#[derive(Debug, Deserialize)]
pub struct DeleteProfileQuery {
    expected_revision: i64,
}

#[derive(Debug)]
pub(crate) struct ResolvedServerProvider {
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
    pub timeout: Duration,
    pub max_steps: usize,
    pub verify_tls_certificates: bool,
}

pub async fn create(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<CreateProviderProfileRequest>,
) -> Result<(StatusCode, Json<ProviderProfile>), ApiError> {
    if let Some(team_id) = request.team_id {
        organization::ensure_team_admin(&state.pool, principal.user_id, team_id).await?;
    }
    let name = validated_name(&request.name)?;
    let provider_kind = normalized_provider_kind(&request.provider_kind)?;
    let model_defaults = normalized_server_defaults(request.model_defaults)?;
    let sealed = seal_optional_secret(
        &state,
        principal.user_id,
        request.team_id,
        request.api_key.as_deref(),
    )?;
    let id = Uuid::new_v4();
    let etag = version_etag(id, 1);
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query(
        r#"
        INSERT INTO provider_profiles (
            id, etag, owner_user_id, team_id, name, provider_kind,
            model_defaults, encrypted_secret, encrypted_data_key,
            secret_nonce, secret_wrap_nonce
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        RETURNING *
        "#,
    )
    .bind(id)
    .bind(etag)
    .bind(request.team_id.is_none().then_some(principal.user_id))
    .bind(request.team_id)
    .bind(name)
    .bind(provider_kind)
    .bind(model_defaults)
    .bind(sealed.as_ref().map(|value| value.ciphertext.as_slice()))
    .bind(
        sealed
            .as_ref()
            .map(|value| value.encrypted_data_key.as_slice()),
    )
    .bind(sealed.as_ref().map(|value| value.nonce.as_slice()))
    .bind(sealed.as_ref().map(|value| value.wrap_nonce.as_slice()))
    .fetch_one(&mut *tx)
    .await?;
    sync::publish_canonical_provider_profile_tx(&mut tx, id).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(row_to_profile(&row)?)))
}

pub async fn list(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<ProviderProfile>>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT DISTINCT profile.*
        FROM provider_profiles profile
        LEFT JOIN team_members member
          ON member.team_id = profile.team_id AND member.user_id = $1
        WHERE profile.deleted_at IS NULL
          AND (profile.owner_user_id = $1 OR member.user_id IS NOT NULL)
        ORDER BY profile.name, profile.id
        "#,
    )
    .bind(principal.user_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.iter()
            .map(row_to_profile)
            .collect::<Result<Vec<_>, _>>()?,
    ))
}

pub async fn update(
    State(state): State<AppState>,
    Path(profile_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<UpdateProviderProfileRequest>,
) -> Result<Json<ProviderProfile>, ApiError> {
    let scope = profile_scope(&state.pool, profile_id).await?;
    ensure_profile_admin(&state.pool, principal.user_id, scope).await?;
    let name = validated_name(&request.name)?;
    let provider_kind = normalized_provider_kind(&request.provider_kind)?;
    let model_defaults = normalized_server_defaults(request.model_defaults)?;
    let next_revision = request.expected_revision + 1;
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query(
        r#"
        UPDATE provider_profiles
        SET revision = revision + 1, etag = $3, name = $4,
            provider_kind = $5, model_defaults = $6, updated_at = now()
        WHERE id = $1 AND revision = $2 AND deleted_at IS NULL
        RETURNING *
        "#,
    )
    .bind(profile_id)
    .bind(request.expected_revision)
    .bind(version_etag(profile_id, next_revision))
    .bind(name)
    .bind(provider_kind)
    .bind(model_defaults)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| {
        ApiError::Conflict("provider profile revision changed; reload before updating".to_owned())
    })?;
    sync::publish_canonical_provider_profile_tx(&mut tx, profile_id).await?;
    tx.commit().await?;
    Ok(Json(row_to_profile(&row)?))
}

pub async fn set_secret(
    State(state): State<AppState>,
    Path(profile_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<SetProviderProfileSecretRequest>,
) -> Result<Json<ProviderProfile>, ApiError> {
    let scope = profile_scope(&state.pool, profile_id).await?;
    ensure_profile_admin(&state.pool, principal.user_id, scope).await?;
    let sealed = seal_optional_secret(
        &state,
        scope.owner_user_id.unwrap_or(principal.user_id),
        scope.team_id,
        request.api_key.as_deref(),
    )?;
    let next_revision = request.expected_revision + 1;
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query(
        r#"
        UPDATE provider_profiles
        SET revision = revision + 1, etag = $3,
            encrypted_secret = $4, encrypted_data_key = $5,
            secret_nonce = $6, secret_wrap_nonce = $7, updated_at = now()
        WHERE id = $1 AND revision = $2 AND deleted_at IS NULL
        RETURNING *
        "#,
    )
    .bind(profile_id)
    .bind(request.expected_revision)
    .bind(version_etag(profile_id, next_revision))
    .bind(sealed.as_ref().map(|value| value.ciphertext.as_slice()))
    .bind(
        sealed
            .as_ref()
            .map(|value| value.encrypted_data_key.as_slice()),
    )
    .bind(sealed.as_ref().map(|value| value.nonce.as_slice()))
    .bind(sealed.as_ref().map(|value| value.wrap_nonce.as_slice()))
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| {
        ApiError::Conflict("provider profile revision changed; reload before updating".to_owned())
    })?;
    sync::publish_canonical_provider_profile_tx(&mut tx, profile_id).await?;
    tx.commit().await?;
    Ok(Json(row_to_profile(&row)?))
}

pub async fn delete(
    State(state): State<AppState>,
    Path(profile_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
    Query(query): Query<DeleteProfileQuery>,
) -> Result<StatusCode, ApiError> {
    let scope = profile_scope(&state.pool, profile_id).await?;
    ensure_profile_admin(&state.pool, principal.user_id, scope).await?;
    let mut tx = state.pool.begin().await?;
    let schedule_ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM schedules WHERE model_profile_id = $1 AND deleted_at IS NULL ORDER BY id FOR UPDATE",
    )
    .bind(profile_id)
    .fetch_all(&mut *tx)
    .await?;
    let row = sqlx::query(
        r#"
        UPDATE provider_profiles
        SET revision = revision + 1,
            etag = 'W/"' || id::text || ':' || (revision + 1)::text || '"',
            encrypted_secret = NULL, encrypted_data_key = NULL,
            secret_nonce = NULL, secret_wrap_nonce = NULL,
            deleted_at = now(), updated_at = now()
        WHERE id = $1 AND revision = $2 AND deleted_at IS NULL
        RETURNING owner_user_id, team_id
        "#,
    )
    .bind(profile_id)
    .bind(query.expected_revision)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| {
        ApiError::Conflict("provider profile revision changed; reload before deleting".to_owned())
    })?;
    sqlx::query(
        r#"
        UPDATE schedules
        SET revision = revision + 1,
            etag = 'W/"' || id::text || ':' || (revision + 1)::text || '"',
            enabled = FALSE, next_run_at = NULL,
            blocked_reason = 'model profile was deleted', updated_at = now()
        WHERE model_profile_id = $1 AND deleted_at IS NULL
        "#,
    )
    .bind(profile_id)
    .execute(&mut *tx)
    .await?;
    sync::publish_provider_profile_tombstones_tx(
        &mut tx,
        profile_id,
        row.try_get("owner_user_id")?,
        row.try_get("team_id")?,
    )
    .await?;
    for schedule_id in schedule_ids {
        sync::publish_canonical_schedule_tx(&mut tx, schedule_id).await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn ensure_profile_for_target(
    pool: &PgPool,
    user_id: Uuid,
    project_id: Uuid,
    profile_id: Option<Uuid>,
    target: &ExecutorTarget,
) -> Result<(), ApiError> {
    let Some(profile_id) = profile_id else {
        return Ok(());
    };
    let row = accessible_profile(pool, user_id, project_id, profile_id).await?;
    let defaults: Value = row.try_get("model_defaults")?;
    let binding = defaults
        .get("endpoint_binding")
        .and_then(Value::as_str)
        .unwrap_or("server");
    let expected = if matches!(target, ExecutorTarget::PersonalDevice { .. }) {
        "per_device"
    } else {
        "server"
    };
    if binding != expected {
        return Err(ApiError::Unprocessable(format!(
            "model profile {profile_id} is bound to {binding}, but the selected executor requires {expected}"
        )));
    }
    Ok(())
}

pub(crate) async fn ensure_crew_profiles_for_target(
    pool: &PgPool,
    user_id: Uuid,
    project_id: Uuid,
    input: &Value,
    target: &ExecutorTarget,
) -> Result<(), ApiError> {
    if !matches!(target, ExecutorTarget::ServerLinux { .. }) {
        return Ok(());
    }
    for profile_id in crew_profile_ids_for_server(input)? {
        ensure_profile_for_target(pool, user_id, project_id, Some(profile_id), target).await?;
    }
    Ok(())
}

pub(crate) fn crew_profile_ids_for_server(input: &Value) -> Result<Vec<Uuid>, ApiError> {
    let Some(definition) = input.get("crew_definition") else {
        return Ok(Vec::new());
    };
    let agents = definition
        .get("agents")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ApiError::Unprocessable("the frozen Crew definition requires agents".to_owned())
        })?;
    let default_selection = definition.get("defaultBackendSelection");
    let mut profile_ids = Vec::new();
    let mut seen = HashSet::new();
    for agent in agents {
        if agent.get("enabled").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        let Some(selection) = agent.get("backendSelection").or(default_selection) else {
            continue;
        };
        let backend = selection
            .get("backend")
            .and_then(Value::as_str)
            .unwrap_or("openai-compatible");
        if backend != "openai-compatible" {
            return Err(ApiError::Unprocessable(format!(
                "Crew backend {backend} is unavailable to Linux server executors"
            )));
        }
        let profile_id = selection
            .get("profileId")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ApiError::Unprocessable(
                    "an OpenAI-compatible Crew backend is missing profileId".to_owned(),
                )
            })?
            .parse::<Uuid>()
            .map_err(|_| {
                ApiError::Unprocessable(
                    "Crew provider profile references must be canonical UUIDs".to_owned(),
                )
            })?;
        if seen.insert(profile_id) {
            profile_ids.push(profile_id);
        }
    }
    Ok(profile_ids)
}

pub(crate) async fn resolve_server_provider(
    pool: &PgPool,
    object_store: Option<&crate::storage::ObjectStore>,
    user_id: Uuid,
    project_id: Uuid,
    profile_id: Uuid,
) -> anyhow::Result<ResolvedServerProvider> {
    let row = accessible_profile(pool, user_id, project_id, profile_id).await?;
    let defaults: Value = row.try_get("model_defaults")?;
    let object = defaults
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("provider profile model_defaults is not an object"))?;
    if object
        .get("endpoint_binding")
        .and_then(Value::as_str)
        .unwrap_or("server")
        != "server"
    {
        anyhow::bail!("the selected provider profile is bound to a personal device");
    }
    let base_url = required_string(object, "base_url")?;
    let model = required_string(object, "model")?;
    let timeout_ms = object
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .unwrap_or(20 * 60 * 1_000)
        .clamp(1_000, MAX_TIMEOUT_MS);
    let max_steps = object
        .get("max_steps")
        .and_then(Value::as_u64)
        .unwrap_or(64)
        .clamp(1, MAX_MODEL_STEPS);
    let api_key = if let Some(sealed) = sealed_from_profile_row(&row)? {
        let store = object_store
            .ok_or_else(|| anyhow::anyhow!("encrypted provider secret storage is unavailable"))?;
        let plaintext =
            if let Some(owner_user_id) = row.try_get::<Option<Uuid>, _>("owner_user_id")? {
                store.open_for_user(owner_user_id, &sealed)?
            } else {
                store.open_for_team(
                    row.try_get::<Option<Uuid>, _>("team_id")?
                        .ok_or_else(|| anyhow::anyhow!("provider profile has no key scope"))?,
                    &sealed,
                )?
            };
        Some(
            String::from_utf8(plaintext)
                .map_err(|_| anyhow::anyhow!("provider profile secret is not valid UTF-8"))?,
        )
    } else {
        None
    };
    if object.get("auth_mode").and_then(Value::as_str) == Some("bearer")
        && api_key.as_deref().is_none_or(str::is_empty)
    {
        anyhow::bail!("the selected provider profile requires an API key");
    }
    Ok(ResolvedServerProvider {
        base_url,
        api_key,
        model,
        timeout: Duration::from_millis(timeout_ms),
        max_steps: usize::try_from(max_steps).unwrap_or(64),
        verify_tls_certificates: object
            .get("verify_tls_certificates")
            .and_then(Value::as_bool)
            .unwrap_or(true),
    })
}

pub(crate) fn normalized_synced_profile(
    payload: &Value,
) -> Result<(String, String, Value), ApiError> {
    let object = payload.as_object().ok_or_else(|| {
        ApiError::Unprocessable("provider profile sync payload must be an object".to_owned())
    })?;
    let name = validated_name(
        object
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    )?;
    let provider_kind = normalized_provider_kind(
        object
            .get("provider")
            .or_else(|| object.get("provider_kind"))
            .and_then(Value::as_str)
            .unwrap_or("openai-compatible"),
    )?;
    let mut defaults = object.clone();
    defaults.remove("_cowork_local_entity_id");
    defaults.remove("name");
    defaults.remove("provider");
    defaults.remove("provider_kind");
    defaults.insert(
        "endpoint_binding".to_owned(),
        Value::String("per_device".to_owned()),
    );
    defaults.insert("source".to_owned(), Value::String("desktop".to_owned()));
    Ok((name, provider_kind, Value::Object(defaults)))
}

fn normalized_server_defaults(value: Value) -> Result<Value, ApiError> {
    let mut object = value.as_object().cloned().ok_or_else(|| {
        ApiError::Unprocessable("model_defaults must be a JSON object".to_owned())
    })?;
    if contains_secret_field(&Value::Object(object.clone())) {
        return Err(ApiError::Unprocessable(
            "model_defaults must not contain cleartext credentials".to_owned(),
        ));
    }
    let base_url = required_string_api(&object, "base_url")?;
    let url = reqwest::Url::parse(&base_url)
        .map_err(|_| ApiError::Unprocessable("base_url must be a valid URL".to_owned()))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(ApiError::Unprocessable(
            "base_url must be an HTTP(S) URL without embedded credentials".to_owned(),
        ));
    }
    let model = required_string_api(&object, "model")?;
    let timeout_ms = object
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .unwrap_or(20 * 60 * 1_000);
    if !(1_000..=MAX_TIMEOUT_MS).contains(&timeout_ms) {
        return Err(ApiError::Unprocessable(format!(
            "timeout_ms must be between 1000 and {MAX_TIMEOUT_MS}"
        )));
    }
    let max_steps = object
        .get("max_steps")
        .and_then(Value::as_u64)
        .unwrap_or(64);
    if !(1..=MAX_MODEL_STEPS).contains(&max_steps) {
        return Err(ApiError::Unprocessable(format!(
            "max_steps must be between 1 and {MAX_MODEL_STEPS}"
        )));
    }
    let auth_mode = object
        .get("auth_mode")
        .and_then(Value::as_str)
        .unwrap_or("bearer")
        .to_owned();
    if !matches!(auth_mode.as_str(), "none" | "bearer") {
        return Err(ApiError::Unprocessable(
            "auth_mode must be none or bearer".to_owned(),
        ));
    }
    object.insert("base_url".to_owned(), Value::String(base_url));
    object.insert("model".to_owned(), Value::String(model));
    object.insert("timeout_ms".to_owned(), Value::from(timeout_ms));
    object.insert("max_steps".to_owned(), Value::from(max_steps));
    object.insert("auth_mode".to_owned(), Value::String(auth_mode));
    object.insert(
        "endpoint_binding".to_owned(),
        Value::String("server".to_owned()),
    );
    object.insert("source".to_owned(), Value::String("server".to_owned()));
    object
        .entry("verify_tls_certificates".to_owned())
        .or_insert(Value::Bool(true));
    Ok(Value::Object(object))
}

#[derive(Clone, Copy)]
struct ProfileScope {
    owner_user_id: Option<Uuid>,
    team_id: Option<Uuid>,
}

async fn profile_scope(pool: &PgPool, profile_id: Uuid) -> Result<ProfileScope, ApiError> {
    let row = sqlx::query(
        "SELECT owner_user_id, team_id FROM provider_profiles WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(profile_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("provider profile {profile_id} was not found")))?;
    Ok(ProfileScope {
        owner_user_id: row.try_get("owner_user_id")?,
        team_id: row.try_get("team_id")?,
    })
}

async fn ensure_profile_admin(
    pool: &PgPool,
    user_id: Uuid,
    scope: ProfileScope,
) -> Result<(), ApiError> {
    if scope.owner_user_id == Some(user_id) {
        return Ok(());
    }
    if let Some(team_id) = scope.team_id {
        return organization::ensure_team_admin(pool, user_id, team_id).await;
    }
    Err(ApiError::Unauthorized(
        "the current user cannot manage this provider profile".to_owned(),
    ))
}

async fn accessible_profile(
    pool: &PgPool,
    user_id: Uuid,
    project_id: Uuid,
    profile_id: Uuid,
) -> Result<PgRow, ApiError> {
    sqlx::query(
        r#"
        SELECT profile.*
        FROM provider_profiles profile
        JOIN projects project ON project.id = $3 AND project.deleted_at IS NULL
        LEFT JOIN team_members member
          ON member.team_id = profile.team_id AND member.user_id = $2
        WHERE profile.id = $1 AND profile.deleted_at IS NULL
          AND (
            profile.owner_user_id = $2
            OR (
              profile.team_id = project.team_id
              AND member.user_id IS NOT NULL
            )
          )
        "#,
    )
    .bind(profile_id)
    .bind(user_id)
    .bind(project_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| {
        ApiError::Unprocessable(format!(
            "model profile {profile_id} is unavailable to the selected project"
        ))
    })
}

fn seal_optional_secret(
    state: &AppState,
    owner_user_id: Uuid,
    team_id: Option<Uuid>,
    api_key: Option<&str>,
) -> Result<Option<SealedValue>, ApiError> {
    let Some(api_key) = api_key else {
        return Ok(None);
    };
    let api_key = api_key.trim();
    if api_key.is_empty() || api_key.len() > MAX_API_KEY_BYTES {
        return Err(ApiError::Unprocessable(format!(
            "api_key must contain 1 to {MAX_API_KEY_BYTES} bytes"
        )));
    }
    let store = state.object_store.as_ref().ok_or_else(|| {
        ApiError::Conflict("encrypted secret storage is not configured".to_owned())
    })?;
    team_id
        .map(|team_id| store.seal_for_team(team_id, api_key.as_bytes()))
        .unwrap_or_else(|| store.seal_for_user(owner_user_id, api_key.as_bytes()))
        .map(Some)
}

fn sealed_from_profile_row(row: &PgRow) -> Result<Option<SealedValue>, ApiError> {
    let ciphertext = row.try_get::<Option<Vec<u8>>, _>("encrypted_secret")?;
    let Some(ciphertext) = ciphertext else {
        return Ok(None);
    };
    let nonce = fixed_nonce(row.try_get("secret_nonce")?, "provider secret nonce")?;
    let wrap_nonce = fixed_nonce(
        row.try_get("secret_wrap_nonce")?,
        "provider secret wrapping nonce",
    )?;
    Ok(Some(SealedValue {
        ciphertext,
        encrypted_data_key: row
            .try_get::<Option<Vec<u8>>, _>("encrypted_data_key")?
            .ok_or_else(|| {
                ApiError::Internal(anyhow::anyhow!("provider secret data key is missing"))
            })?,
        nonce,
        wrap_nonce,
    }))
}

fn fixed_nonce(value: Option<Vec<u8>>, label: &str) -> Result<[u8; 12], ApiError> {
    value
        .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("{label} is missing")))?
        .try_into()
        .map_err(|_| ApiError::Internal(anyhow::anyhow!("{label} has an invalid length")))
}

fn validated_name(value: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_PROFILE_NAME {
        return Err(ApiError::Unprocessable(format!(
            "provider profile name must contain 1 to {MAX_PROFILE_NAME} characters"
        )));
    }
    Ok(value.to_owned())
}

fn normalized_provider_kind(value: &str) -> Result<String, ApiError> {
    match value.trim() {
        "openai-compatible" | "openai_compatible" => Ok("openai_compatible".to_owned()),
        other => Err(ApiError::Unprocessable(format!(
            "unsupported provider_kind {other}; expected openai_compatible"
        ))),
    }
}

fn required_string_api(object: &Map<String, Value>, key: &str) -> Result<String, ApiError> {
    let value = object
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    if value.is_empty() || value.len() > 4_096 {
        return Err(ApiError::Unprocessable(format!(
            "model_defaults.{key} must contain 1 to 4096 characters"
        )));
    }
    Ok(value.to_owned())
}

fn required_string(object: &Map<String, Value>, key: &str) -> anyhow::Result<String> {
    let value = object
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    if value.is_empty() {
        anyhow::bail!("provider profile model_defaults.{key} is missing");
    }
    Ok(value.to_owned())
}

fn contains_secret_field(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, value)| {
            matches!(
                key.to_ascii_lowercase().as_str(),
                "api_key"
                    | "access_token"
                    | "refresh_token"
                    | "password"
                    | "secret"
                    | "client_secret"
                    | "authorization"
            ) || contains_secret_field(value)
        }),
        Value::Array(items) => items.iter().any(contains_secret_field),
        _ => false,
    }
}

fn row_to_profile(row: &PgRow) -> Result<ProviderProfile, ApiError> {
    Ok(ProviderProfile {
        schema_version: SCHEMA_VERSION,
        id: row.try_get("id")?,
        revision: row.try_get("revision")?,
        etag: row.try_get("etag")?,
        owner_user_id: row.try_get("owner_user_id")?,
        team_id: row.try_get("team_id")?,
        name: row.try_get("name")?,
        provider_kind: row.try_get("provider_kind")?,
        model_defaults: row.try_get("model_defaults")?,
        has_secret: row
            .try_get::<Option<Vec<u8>>, _>("encrypted_secret")?
            .is_some(),
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        deleted_at: row.try_get("deleted_at")?,
    })
}

fn version_etag(id: Uuid, revision: i64) -> String {
    format!("W/\"{id}:{revision}\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validates_server_provider_defaults_without_credentials() {
        let normalized = normalized_server_defaults(json!({
            "base_url": "https://models.example.test/v1",
            "model": "test-model",
            "auth_mode": "bearer"
        }))
        .unwrap();
        assert_eq!(normalized["endpoint_binding"], "server");
        assert_eq!(normalized["timeout_ms"], 1_200_000);
        assert!(normalized_server_defaults(json!({
            "base_url": "https://user:password@example.test/v1",
            "model": "test-model"
        }))
        .is_err());
        assert!(normalized_server_defaults(json!({
            "base_url": "https://models.example.test/v1",
            "model": "test-model",
            "api_key": "must-not-be-metadata"
        }))
        .is_err());
    }

    #[test]
    fn synced_profiles_are_forced_to_per_device_binding() {
        let (_, kind, defaults) = normalized_synced_profile(&json!({
            "name": "Local vLLM",
            "provider": "openai-compatible",
            "model": "local-model",
            "endpoint_binding": "server"
        }))
        .unwrap();
        assert_eq!(kind, "openai_compatible");
        assert_eq!(defaults["endpoint_binding"], "per_device");
    }

    #[test]
    fn extracts_effective_server_crew_profiles_without_duplicates() {
        let default_profile = Uuid::new_v4();
        let override_profile = Uuid::new_v4();
        let input = json!({
            "crew_definition":{
                "defaultBackendSelection":{
                    "backend":"openai-compatible",
                    "profileId":default_profile
                },
                "agents":[
                    {"id":"inherit"},
                    {"id":"same","backendSelection":{
                        "backend":"openai-compatible",
                        "profileId":default_profile
                    }},
                    {"id":"override","backendSelection":{
                        "backend":"openai-compatible",
                        "profileId":override_profile
                    }},
                    {"id":"disabled","enabled":false,"backendSelection":{
                        "backend":"codex"
                    }}
                ]
            }
        });
        assert_eq!(
            crew_profile_ids_for_server(&input).unwrap(),
            vec![default_profile, override_profile]
        );
        let unsupported = json!({
            "crew_definition":{
                "defaultBackendSelection":{"backend":"codex"},
                "agents":[{"id":"active"}]
            }
        });
        assert!(crew_profile_ids_for_server(&unsupported)
            .unwrap_err()
            .to_string()
            .contains("unavailable to Linux server"));
    }
}
