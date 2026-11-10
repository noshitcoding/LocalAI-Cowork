//! Projects secret-free provider profiles and reconciles dependent schedules.

use crate::{
    error::ApiError,
    providers,
    sync::{
        materialization::{
            is_materialized, remember_materialization, schedules::reconcile_schedules_for_reference,
        },
        publication::publish_canonical_schedule_tx,
    },
};
use chrono::{DateTime, Utc};
use cowork_contracts::SyncOperation;
use serde_json::Value;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

pub(in crate::sync) async fn materialize_provider_profile(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    profile_id: Uuid,
    operation: SyncOperation,
    payload: Option<&Value>,
) -> Result<(), ApiError> {
    let tracked = is_materialized(tx, user_id, "provider_profile", profile_id).await?;
    let current = sqlx::query("SELECT * FROM provider_profiles WHERE id = $1 FOR UPDATE")
        .bind(profile_id)
        .fetch_optional(&mut **tx)
        .await?;
    if current.is_some() && !tracked {
        return Err(ApiError::Conflict(
            "synced provider profile ID collides with an independent server profile".to_owned(),
        ));
    }
    if let Some(row) = &current {
        if row.try_get::<Option<Uuid>, _>("owner_user_id")? != Some(user_id)
            || row.try_get::<Option<Uuid>, _>("team_id")?.is_some()
        {
            return Err(ApiError::Conflict(
                "synced provider profiles may only modify their owner's personal profile"
                    .to_owned(),
            ));
        }
    }
    if operation == SyncOperation::Delete {
        if tracked {
            let schedule_ids = sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM schedules WHERE model_profile_id = $1 AND deleted_at IS NULL ORDER BY id FOR UPDATE",
            )
            .bind(profile_id)
            .fetch_all(&mut **tx)
            .await?;
            sqlx::query(
                r#"
                UPDATE provider_profiles
                SET revision = revision + 1,
                    etag = 'W/"' || id::text || ':' || (revision + 1)::text || '"',
                    encrypted_secret = NULL, encrypted_data_key = NULL,
                    secret_nonce = NULL, secret_wrap_nonce = NULL,
                    deleted_at = now(), updated_at = now()
                WHERE id = $1 AND owner_user_id = $2 AND deleted_at IS NULL
                "#,
            )
            .bind(profile_id)
            .bind(user_id)
            .execute(&mut **tx)
            .await?;
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
            .execute(&mut **tx)
            .await?;
            for schedule_id in schedule_ids {
                publish_canonical_schedule_tx(tx, schedule_id).await?;
            }
        }
        return Ok(());
    }
    let (name, provider_kind, mut model_defaults) =
        providers::normalized_synced_profile(payload.ok_or_else(|| {
            ApiError::Unprocessable("provider profile sync payload is required".to_owned())
        })?)?;
    if let Some(row) = &current {
        let existing_defaults: Value = row.try_get("model_defaults")?;
        if existing_defaults
            .get("endpoint_binding")
            .and_then(Value::as_str)
            == Some("server")
        {
            let mut merged = existing_defaults.as_object().cloned().unwrap_or_default();
            let incoming = model_defaults.as_object().cloned().unwrap_or_default();
            for key in [
                "model",
                "timeout_ms",
                "verify_tls_certificates",
                "context_window",
                "temperature",
                "preset",
            ] {
                if let Some(value) = incoming.get(key) {
                    merged.insert(key.to_owned(), value.clone());
                }
            }
            model_defaults = Value::Object(merged);
        }
        let unchanged = row.try_get::<String, _>("name")? == name
            && row.try_get::<String, _>("provider_kind")? == provider_kind
            && row.try_get::<Value, _>("model_defaults")? == model_defaults
            && row
                .try_get::<Option<DateTime<Utc>>, _>("deleted_at")?
                .is_none();
        if unchanged {
            remember_materialization(tx, user_id, "provider_profile", profile_id).await?;
            reconcile_schedules_for_reference(tx, user_id, "model_profile_id", profile_id).await?;
            return Ok(());
        }
    }
    let revision = current
        .as_ref()
        .map(|row| row.try_get::<i64, _>("revision"))
        .transpose()?
        .unwrap_or(0)
        + 1;
    if current.is_some() {
        sqlx::query(
            r#"
            UPDATE provider_profiles
            SET revision = $2, etag = $3, name = $4, provider_kind = $5,
                model_defaults = $6, deleted_at = NULL, updated_at = now()
            WHERE id = $1
            "#,
        )
        .bind(profile_id)
        .bind(revision)
        .bind(format!("W/\"{profile_id}:{revision}\""))
        .bind(name)
        .bind(provider_kind)
        .bind(model_defaults)
        .execute(&mut **tx)
        .await?;
    } else {
        sqlx::query(
            r#"
            INSERT INTO provider_profiles (
                id, revision, etag, owner_user_id, team_id, name,
                provider_kind, model_defaults
            ) VALUES ($1, $2, $3, $4, NULL, $5, $6, $7)
            "#,
        )
        .bind(profile_id)
        .bind(revision)
        .bind(format!("W/\"{profile_id}:{revision}\""))
        .bind(user_id)
        .bind(name)
        .bind(provider_kind)
        .bind(model_defaults)
        .execute(&mut **tx)
        .await?;
    }
    remember_materialization(tx, user_id, "provider_profile", profile_id).await?;
    reconcile_schedules_for_reference(tx, user_id, "model_profile_id", profile_id).await?;
    Ok(())
}
