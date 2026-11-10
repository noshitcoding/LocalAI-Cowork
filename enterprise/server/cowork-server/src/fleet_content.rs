//! Instance-side administrative readers. Object authorization stays in this DB.
use crate::{auth::Principal, error::ApiError, fleet};
use axum::{extract::Extension, Json};
use cowork_contracts::enterprise::InstanceContentAccess;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentCapability {
    pub organisation_id: Uuid,
    pub instance_id: Uuid,
    pub version: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mfa_version: Option<u8>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentTarget {
    Project,
    Thread,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentAudit {
    pub target_type: ContentTarget,
    pub target_id: Uuid,
    pub grant_id: Option<Uuid>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentAccessRequest {
    pub scope: fleet::AccessRequest,
    pub audit: Option<ContentAudit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mfa_contract: Option<u8>,
}

fn scope(user_id: Uuid) -> Result<fleet::AccessRequest, ApiError> {
    let parse = |key| {
        std::env::var(key)
            .ok()
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| ApiError::Unauthorized("instance scope is unavailable".into()))
    };
    Ok(fleet::AccessRequest {
        organisation_id: parse("COWORK_ORGANISATION_ID")?,
        instance_id: parse("COWORK_INSTANCE_ID")?,
        user_id,
    })
}
async fn callback<T: Serialize>(path: &str, body: &T) -> Result<reqwest::Response, ApiError> {
    let base = std::env::var("COWORK_FLEET_MANAGEMENT_URL")
        .map_err(|_| ApiError::Unauthorized("management is unavailable".into()))?;
    let client = crate::management_transport::client(&base, std::time::Duration::from_secs(5))?;
    let response = client
        .post(format!(
            "{}/management/runner/v1/{path}",
            base.trim_end_matches('/')
        ))
        .header(
            "x-cowork-instance-key",
            fleet::secret("COWORK_FLEET_KEY_FILE")?,
        )
        .json(body)
        .send()
        .await
        .map_err(|e| ApiError::Internal(e.into()))?;
    if !response.status().is_success() {
        return Err(ApiError::Unauthorized(
            "administrative access is not authorised".into(),
        ));
    }
    Ok(response)
}
pub async fn context(
    user_id: Uuid,
    audit: Option<ContentAudit>,
) -> Result<InstanceContentAccess, ApiError> {
    if !fleet::enabled() {
        return Ok(InstanceContentAccess::default());
    }
    callback(
        "content-access",
        &ContentAccessRequest {
            scope: scope(user_id)?,
            audit,
            mfa_contract: crate::organisation_mfa::instance_contract(),
        },
    )
    .await?
    .json()
    .await
    .map_err(|e| ApiError::Internal(e.into()))
}
pub async fn status(
    Extension(principal): Extension<Principal>,
) -> Result<Json<InstanceContentAccess>, ApiError> {
    Ok(Json(context(principal.user_id, None).await?))
}
pub async fn announce(pool: PgPool) {
    if !fleet::enabled() {
        return;
    }
    loop {
        if let Ok(scope) = scope(Uuid::nil()) {
            // Only the version is advertised; no user or content information.
            let _ = callback(
                "content-capability",
                &ContentCapability {
                    organisation_id: scope.organisation_id,
                    instance_id: scope.instance_id,
                    version: 2,
                    mfa_version: crate::organisation_mfa::fleet_capability(&pool)
                        .await
                        .unwrap_or(None),
                },
            )
            .await;
        }
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
    }
}
pub async fn eligible(pool: &PgPool, user: Uuid) -> Result<bool, ApiError> {
    if !fleet::enabled() {
        return crate::db::user_is_platform_admin(pool, user).await;
    }
    Ok(context(user, None).await?.support_administrator)
}
pub async fn audit(
    user: Uuid,
    target_type: &str,
    target_id: Uuid,
    grant_id: Option<Uuid>,
) -> Result<(), ApiError> {
    if !fleet::enabled() {
        return Ok(());
    }
    let target_type = match target_type {
        "project" => ContentTarget::Project,
        "thread" => ContentTarget::Thread,
        _ => return Err(ApiError::Unauthorized("invalid content target".into())),
    };
    context(
        user,
        Some(ContentAudit {
            target_type,
            target_id,
            grant_id,
        }),
    )
    .await?;
    Ok(())
}
/// Support-only sessions cannot create personal objects, start tasks, pair devices,
/// change security settings or use a viewer WebSocket as a control channel.
pub fn permits_request(
    info: &InstanceContentAccess,
    method: &axum::http::Method,
    path: &str,
) -> bool {
    if !info.managed || !info.read_only {
        return true;
    }
    if crate::organisation_mfa::recovery_route(method, path) {
        return true;
    }
    if method == axum::http::Method::DELETE
        && path
            .strip_prefix("/api/v1/support-grants/")
            .is_some_and(|id| id.parse::<Uuid>().is_ok())
    {
        return true;
    }
    if method != axum::http::Method::GET {
        return false;
    }
    matches!(path, "/api/v1/content-access" | "/api/v1/support-grants")
        || [
            "/api/v1/projects",
            "/api/v1/threads",
            "/api/v1/runs",
            "/api/v1/artifacts",
        ]
        .iter()
        .any(|prefix| path == *prefix || path.starts_with(&format!("{prefix}/")))
}

pub async fn interactive_access(
    pool: &PgPool,
    user: Uuid,
    run: Uuid,
    required: cowork_contracts::ProjectRole,
) -> Result<(), ApiError> {
    crate::organisation_mfa::session_pool(
        pool,
        user,
        crate::organisation_mfa::request_authority(user).map(|a| a.session),
    )
    .await?;
    let active: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id=$1 AND deleted_at IS NULL)")
            .bind(user)
            .fetch_one(pool)
            .await?;
    if !active || context(user, None).await?.read_only {
        return Err(ApiError::Unauthorized(
            "interactive access is no longer authorised".into(),
        ));
    }
    let run = crate::db::get_run(pool, run).await?;
    crate::organization::ensure_thread_role(
        pool,
        user,
        run.spec.project_id,
        run.spec.thread_id,
        required,
    )
    .await?;
    Ok(())
}

/// A stream ticket is only a handshake credential, never a lasting permission.
pub async fn guard_interactive<T, F>(
    pool: &PgPool,
    user: Uuid,
    run: Uuid,
    required: cowork_contracts::ProjectRole,
    work: F,
) -> anyhow::Result<T>
where
    F: std::future::Future<Output = anyhow::Result<T>>,
{
    interactive_access(pool, user, run, required)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    tokio::pin!(work);
    let mut checks = tokio::time::interval_at(
        tokio::time::Instant::now() + std::time::Duration::from_secs(2),
        std::time::Duration::from_secs(2),
    );
    checks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            _=checks.tick()=>interactive_access(pool,user,run,required).await.map_err(|e|anyhow::anyhow!("{e}"))?,
            result=&mut work=>return result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Method;
    #[test]
    fn support_scope_cannot_become_an_execution_or_settings_channel() {
        let info = InstanceContentAccess {
            managed: true,
            read_only: true,
            ..Default::default()
        };
        assert!(permits_request(&info, &Method::GET, "/api/v1/projects"));
        assert!(permits_request(
            &info,
            &Method::GET,
            "/api/v1/runs/123/events"
        ));
        assert!(!permits_request(&info, &Method::POST, "/api/v1/projects"));
        assert!(!permits_request(
            &info,
            &Method::GET,
            "/api/v1/projects-secret"
        ));
        assert!(!permits_request(
            &info,
            &Method::GET,
            "/api/v1/provider-profiles"
        ));
        assert!(!permits_request(
            &info,
            &Method::POST,
            "/api/v1/auth/totp/disable"
        ));
        assert!(!permits_request(
            &info,
            &Method::POST,
            "/api/v1/desktop-sessions/123/ticket"
        ));
        assert!(!permits_request(
            &info,
            &Method::DELETE,
            "/api/v1/support-grants/all"
        ));
        assert!(permits_request(
            &info,
            &Method::DELETE,
            &format!("/api/v1/support-grants/{}", Uuid::new_v4())
        ));
        assert!(permits_request(&info, &Method::POST, "/api/v1/auth/logout"));
    }
}
