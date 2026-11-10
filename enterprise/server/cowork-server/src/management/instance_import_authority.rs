//! Request authority is never persisted in an import binding or receipt.
use super::*;
use axum::http::header::AUTHORIZATION;

pub(super) struct ImportAuthority {
    user: Uuid,
    session: Uuid,
    device: Uuid,
    token_hash: [u8; 32],
}

fn unavailable() -> ApiError {
    ApiError::Unauthorized("current signed-in session is required for instance import".into())
}

impl ImportAuthority {
    /// Snapshot the middleware-authenticated device before taking transaction
    /// locks. No second pool transaction is opened under the import's locks.
    pub(super) async fn from_request(
        state: &AppState,
        principal: &Principal,
        headers: &HeaderMap,
    ) -> Result<Self, ApiError> {
        let session = principal
            .session_id
            .filter(|_| !principal.bootstrap)
            .ok_or_else(unavailable)?;
        let mut values = headers.get_all(AUTHORIZATION).iter();
        let supplied = values.next().ok_or_else(unavailable)?;
        if values.next().is_some() || supplied.as_bytes().len() > 4096 {
            return Err(unavailable());
        }
        let token = supplied
            .to_str()
            .ok()
            .and_then(|v| v.strip_prefix("Bearer "))
            .filter(|v| !v.is_empty())
            .ok_or_else(unavailable)?;
        let token_hash = auth::opaque_token_hash(token);
        let device: Option<Uuid> = sqlx::query_scalar("SELECT s.device_id FROM access_tokens a JOIN auth_sessions s ON s.id=a.session_id AND s.user_id=a.user_id JOIN users u ON u.id=a.user_id WHERE a.token_hash=$1 AND a.session_id=$2 AND a.user_id=$3 AND a.revoked_at IS NULL AND a.expires_at>clock_timestamp() AND s.revoked_at IS NULL AND s.expires_at>clock_timestamp() AND u.deleted_at IS NULL")
            .bind(token_hash.as_slice()).bind(session).bind(principal.user_id).fetch_optional(&state.pool).await?;
        Ok(Self {
            user: principal.user_id,
            session,
            device: device.ok_or_else(unavailable)?,
            token_hash,
        })
    }

    /// User -> session -> access token agrees with logout/refresh's session ->
    /// token order. Do not join-lock access first: that would invert logout.
    pub(super) async fn lock(&self, tx: &mut Transaction<'_, Postgres>) -> Result<(), ApiError> {
        sqlx::query("SELECT id FROM users WHERE id=$1 FOR SHARE")
            .bind(self.user)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(unavailable)?;
        sqlx::query("SELECT id FROM auth_sessions WHERE id=$1 FOR SHARE")
            .bind(self.session)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(unavailable)?;
        sqlx::query("SELECT token_hash FROM access_tokens WHERE token_hash=$1 FOR SHARE")
            .bind(self.token_hash.as_slice())
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(unavailable)?;
        self.validate(tx).await
    }

    /// Evaluate the database clock after all row/advisory waits, including audit
    /// serialization. Transaction-stable now() cannot prove current authority.
    pub(super) async fn validate(
        &self,
        tx: &mut Transaction<'_, Postgres>,
    ) -> Result<(), ApiError> {
        let current: Option<Uuid> = sqlx::query_scalar("SELECT s.id FROM access_tokens a JOIN auth_sessions s ON s.id=a.session_id AND s.user_id=a.user_id JOIN users u ON u.id=a.user_id WHERE a.token_hash=$1 AND a.session_id=$2 AND a.user_id=$3 AND s.device_id=$4 AND a.revoked_at IS NULL AND a.expires_at>clock_timestamp() AND s.revoked_at IS NULL AND s.expires_at>clock_timestamp() AND u.deleted_at IS NULL AND (NOT u.platform_admin OR (s.second_factor_verified_at>clock_timestamp()-interval '12 hours' AND s.second_factor_verified_at<=clock_timestamp() AND (EXISTS(SELECT 1 FROM user_totp t WHERE t.user_id=u.id AND t.enabled) OR EXISTS(SELECT 1 FROM passkeys p WHERE p.user_id=u.id))))")
            .bind(self.token_hash.as_slice()).bind(self.session).bind(self.user).bind(self.device)
            .fetch_optional(&mut **tx).await?;
        if current.is_none() {
            return Err(unavailable());
        }
        Ok(())
    }
}
