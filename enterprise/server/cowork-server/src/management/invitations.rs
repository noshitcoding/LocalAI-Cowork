//! Organisation-bound, one-use invitations. Existing identities must sign in;
//! an invitation never replaces credentials or links an external identity.
use super::*;
use axum::http::{header::CACHE_CONTROL, HeaderName, StatusCode};
use chrono::{DateTime, Duration, Utc};
use cowork_contracts::AuthTokens;
use sqlx::postgres::PgRow;

type NoStore = [(HeaderName, &'static str); 1];
fn no_store() -> NoStore {
    [(CACHE_CONTROL, "no-store")]
}
fn unusable() -> ApiError {
    ApiError::Unauthorized("invitation is unavailable, expired or already used".into())
}

pub fn public_router() -> Router<AppState> {
    Router::new()
        .route(
            "/auth/organisation-invitations/inspect",
            post(inspect_invitation),
        )
        .route(
            "/auth/organisation-invitations/register",
            post(register_invitation),
        )
}

fn digest(token: &str) -> Result<[u8; 32], ApiError> {
    if !(32..=256).contains(&token.len()) {
        return Err(unusable());
    }
    Ok(auth::opaque_token_hash(token))
}

// Match the shared schema's supported address syntax, so every issued invitation
// can be consumed by both clients. The general legacy identity API is unchanged.
fn validate_invitation_email(email: &str) -> Result<(), ApiError> {
    auth::validate_email_for_external_identity(email)?;
    let (local, domain) = email
        .split_once('@')
        .ok_or_else(|| invalid("email is invalid"))?;
    let labels: Vec<&str> = domain.split('.').collect();
    let valid_local = !local.starts_with('.')
        && !local.contains("..")
        && local
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_'+-.".contains(&b))
        && local
            .bytes()
            .last()
            .is_some_and(|b| b.is_ascii_alphanumeric() || b"_+-".contains(&b));
    let valid_domain = labels.len() >= 2
        && labels.iter().all(|s| {
            s.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric())
                && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        && labels
            .last()
            .is_some_and(|s| s.len() >= 2 && s.bytes().all(|b| b.is_ascii_alphabetic()));
    if !valid_local || !valid_domain {
        return Err(invalid("email is invalid"));
    }
    Ok(())
}

async fn lock_org(tx: &mut Transaction<'_, Postgres>, org: Uuid) -> Result<(), ApiError> {
    let exists = sqlx::query(
        "SELECT id FROM management_organisations WHERE id=$1 AND archived_at IS NULL FOR UPDATE",
    )
    .bind(org)
    .fetch_optional(&mut **tx)
    .await?;
    if exists.is_none() {
        return Err(denied());
    }
    Ok(())
}
async fn administrator(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
    user: Uuid,
) -> Result<bool, ApiError> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users u LEFT JOIN management_members m ON m.user_id=u.id AND m.organisation_id=$1 WHERE u.id=$2 AND u.deleted_at IS NULL AND (u.platform_admin OR m.role='admin'))")
        .bind(org).bind(user).fetch_one(&mut **tx).await?)
}
async fn authorize(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
    principal: &Principal,
) -> Result<(), ApiError> {
    lock_org(tx, org).await?;
    if principal.bootstrap || !administrator(tx, org, principal.user_id).await? {
        return Err(denied());
    }
    Ok(())
}

pub(super) async fn create_invitation(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(org): Path<Uuid>,
    Json(request): Json<CreateOrganisationInvitation>,
) -> Result<(StatusCode, NoStore, Json<OrganisationInvitationSecret>), ApiError> {
    let email = request.email.trim().to_ascii_lowercase();
    validate_invitation_email(&email)?;
    let now = Utc::now();
    let expires = request.expires_at.unwrap_or(now + Duration::days(7));
    if expires <= now + Duration::hours(1) || expires > now + Duration::days(30) {
        return Err(invalid(
            "invitations must expire between one hour and 30 days from now",
        ));
    }
    let mut tx = state.pool.begin().await?;
    authorize(&mut tx, org, &principal).await?;
    let pending:i64=sqlx::query_scalar("SELECT count(*) FROM management_invitations WHERE organisation_id=$1 AND accepted_at IS NULL AND revoked_at IS NULL AND expires_at>now()")
        .bind(org).fetch_one(&mut *tx).await?;
    if pending >= 1000 {
        return Err(ApiError::Quota(
            "too many pending organisation invitations".into(),
        ));
    }
    let token = auth::random_token()?;
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO management_invitations(id,organisation_id,email,role,token_hash,invited_by,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7)")
        .bind(id).bind(org).bind(&email).bind(tag(request.role)?).bind(auth::opaque_token_hash(&token).as_slice()).bind(principal.user_id).bind(expires).execute(&mut *tx).await?;
    append_audit(
        &mut tx,
        org,
        principal.user_id,
        "invitation.create",
        id,
        json!({"role":request.role,"expires_at":expires}),
    )
    .await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        no_store(),
        Json(OrganisationInvitationSecret {
            id,
            organisation_id: org,
            email,
            role: request.role,
            token,
            expires_at: expires,
        }),
    ))
}

pub(super) async fn list_invitations(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(org): Path<Uuid>,
) -> Result<Json<Vec<OrganisationInvitation>>, ApiError> {
    membership(&state, &principal, org, true).await?;
    let rows:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',i.id,'organisation_id',i.organisation_id,'email',i.email,'role',i.role,'invited_by',i.invited_by,'created_at',i.created_at,'expires_at',i.expires_at,'state',CASE WHEN i.accepted_at IS NOT NULL THEN 'accepted' WHEN i.revoked_at IS NOT NULL THEN 'revoked' WHEN i.expires_at<=now() THEN 'expired' WHEN u.deleted_at IS NOT NULL OR NOT (u.platform_admin OR coalesce(m.role='admin',false)) THEN 'authority_revoked' ELSE 'pending' END) FROM management_invitations i JOIN users u ON u.id=i.invited_by LEFT JOIN management_members m ON m.user_id=u.id AND m.organisation_id=i.organisation_id WHERE i.organisation_id=$1 ORDER BY (i.accepted_at IS NULL AND i.revoked_at IS NULL AND i.expires_at>now()) DESC,i.created_at DESC LIMIT 1000")
        .bind(org).fetch_all(&state.pool).await?;
    Ok(Json(
        rows.into_iter()
            .map(serde_json::from_value)
            .collect::<Result<_, _>>()?,
    ))
}

pub(super) async fn revoke_invitation(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Value>, ApiError> {
    let mut tx = state.pool.begin().await?;
    authorize(&mut tx, org, &principal).await?;
    let row=sqlx::query("SELECT accepted_at,revoked_at FROM management_invitations WHERE organisation_id=$1 AND id=$2 FOR UPDATE")
        .bind(org).bind(id).fetch_optional(&mut *tx).await?.ok_or_else(denied)?;
    if row.get::<Option<DateTime<Utc>>, _>("accepted_at").is_some() {
        return Err(ApiError::Conflict(
            "an accepted invitation cannot be revoked; manage the membership instead".into(),
        ));
    }
    if row.get::<Option<DateTime<Utc>>, _>("revoked_at").is_none() {
        sqlx::query("UPDATE management_invitations SET revoked_at=now() WHERE id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        append_audit(
            &mut tx,
            org,
            principal.user_id,
            "invitation.revoke",
            id,
            json!({}),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(Json(json!({"id":id,"state":"revoked"})))
}

// Read scope first, then take the organisation lock before the invitation lock.
// Creation, consumption and revocation use one lock order and recheck authority.
async fn pending(tx: &mut Transaction<'_, Postgres>, token: &str) -> Result<PgRow, ApiError> {
    let hash = digest(token)?;
    let org: Uuid = sqlx::query_scalar(
        "SELECT organisation_id FROM management_invitations WHERE token_hash=$1",
    )
    .bind(hash.as_slice())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(unusable)?;
    lock_org(tx, org).await?;
    let row=sqlx::query("SELECT i.*,o.name AS organisation_name FROM management_invitations i JOIN management_organisations o ON o.id=i.organisation_id WHERE i.token_hash=$1 AND i.accepted_at IS NULL AND i.revoked_at IS NULL AND i.expires_at>now() FOR UPDATE OF i")
        .bind(hash.as_slice()).fetch_optional(&mut **tx).await?.ok_or_else(unusable)?;
    if !administrator(tx, org, row.get("invited_by")).await? {
        return Err(unusable());
    }
    Ok(row)
}

async fn inspect_invitation(
    State(state): State<AppState>,
    Json(request): Json<OrganisationInvitationToken>,
) -> Result<(NoStore, Json<OrganisationInvitationPreview>), ApiError> {
    let mut tx = state.pool.begin().await?;
    let row = pending(&mut tx, &request.token).await?;
    let email: String = row.get("email");
    let account_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE lower(email)=$1)")
            .bind(&email)
            .fetch_one(&mut *tx)
            .await?;
    let preview = OrganisationInvitationPreview {
        id: row.get("id"),
        organisation_id: row.get("organisation_id"),
        organisation_name: row.get("organisation_name"),
        email,
        role: serde_json::from_value(json!(row.get::<String, _>("role")))?,
        expires_at: row.get("expires_at"),
        account_exists,
    };
    tx.commit().await?;
    Ok((no_store(), Json(preview)))
}

async fn consume(
    tx: &mut Transaction<'_, Postgres>,
    row: &PgRow,
    user: Uuid,
) -> Result<OrganisationInvitationAccepted, ApiError> {
    let org: Uuid = row.get("organisation_id");
    let invitation: Uuid = row.get("id");
    let inserted=sqlx::query("INSERT INTO management_members(organisation_id,user_id,role,content_admin) VALUES($1,$2,$3,FALSE) ON CONFLICT(organisation_id,user_id) DO NOTHING")
        .bind(org).bind(user).bind(row.get::<String,_>("role")).execute(&mut **tx).await?.rows_affected()>0;
    let role: String = sqlx::query_scalar(
        "SELECT role FROM management_members WHERE organisation_id=$1 AND user_id=$2",
    )
    .bind(org)
    .bind(user)
    .fetch_one(&mut **tx)
    .await?;
    if role == "suspended" {
        return Err(ApiError::Conflict(
            "use organisation SSO or request an explicit administrator membership grant".into(),
        ));
    }
    sqlx::query("UPDATE management_invitations SET accepted_by=$2,accepted_at=now() WHERE id=$1")
        .bind(invitation)
        .bind(user)
        .execute(&mut **tx)
        .await?;
    append_audit(
        tx,
        org,
        user,
        "invitation.accept",
        invitation,
        json!({"user_id":user,"role":role,"membership_created":inserted}),
    )
    .await?;
    Ok(OrganisationInvitationAccepted {
        organisation_id: org,
        user_id: user,
        role: serde_json::from_value(json!(role))?,
        membership_created: inserted,
    })
}

pub(super) async fn accept_invitation(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<OrganisationInvitationToken>,
) -> Result<Json<OrganisationInvitationAccepted>, ApiError> {
    if principal.bootstrap {
        return Err(denied());
    }
    let mut tx = state.pool.begin().await?;
    let row = pending(&mut tx, &request.token).await?;
    let matches: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM users WHERE id=$1 AND lower(email)=$2 AND deleted_at IS NULL)",
    )
    .bind(principal.user_id)
    .bind(row.get::<String, _>("email"))
    .fetch_one(&mut *tx)
    .await?;
    if !matches {
        return Err(ApiError::Unauthorized(
            "sign in with the email address named in this invitation".into(),
        ));
    }
    let result = consume(&mut tx, &row, principal.user_id).await?;
    tx.commit().await?;
    Ok(Json(result))
}

async fn register_invitation(
    State(state): State<AppState>,
    Json(request): Json<RegisterOrganisationInvitation>,
) -> Result<(StatusCode, NoStore, Json<AuthTokens>), ApiError> {
    if request.display_name.trim().is_empty() || request.display_name.len() > 200 {
        return Err(invalid("display_name must contain 1 to 200 characters"));
    }
    // Reject an invalid token before performing password hashing. Revalidate all
    // state after hashing so expiry/revocation during that work remains binding.
    let mut check = state.pool.begin().await?;
    let preview = pending(&mut check, &request.token).await?;
    let email: String = preview.get("email");
    auth::validate_identity(&email, &request.password)?;
    let existing: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE lower(email)=$1)")
            .bind(&email)
            .fetch_one(&mut *check)
            .await?;
    if existing {
        return Err(ApiError::Conflict(
            "this account already exists; sign in to accept the invitation".into(),
        ));
    }
    check.commit().await?;
    let password = auth::hash_password(request.password).await?;
    let mut tx = state.pool.begin().await?;
    let row = pending(&mut tx, &request.token).await?;
    let user = Uuid::new_v4();
    let inserted=sqlx::query("INSERT INTO users(id,etag,email,display_name,password_hash,platform_admin) VALUES($1,$2,$3,$4,$5,FALSE) ON CONFLICT DO NOTHING")
        .bind(user).bind(format!("W/\"{user}:1\"")).bind(&email).bind(request.display_name.trim()).bind(password).execute(&mut *tx).await?.rows_affected()>0;
    if !inserted {
        return Err(ApiError::Conflict(
            "this account already exists; sign in to accept the invitation".into(),
        ));
    }
    consume(&mut tx, &row, user).await?;
    let tokens = auth::create_session_tx(&mut tx, user, request.device_id).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, no_store(), Json(tokens)))
}
