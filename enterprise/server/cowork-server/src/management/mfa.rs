//! Current administrator CAS and once-only original policy-change receipts.
use super::*;
use cowork_contracts::organisation_mfa::*;

pub(super) async fn ensure_policy(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
) -> Result<(), ApiError> {
    sqlx::query("INSERT INTO organisation_mfa_policies(organisation_id,epoch) VALUES($1,$2) ON CONFLICT DO NOTHING")
        .bind(org).bind(Uuid::new_v4()).execute(&mut **tx).await?;
    Ok(())
}
pub(crate) async fn policy(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
) -> Result<OrganisationMfaPolicy, ApiError> {
    let row = sqlx::query("SELECT organisation_id,revision,epoch,required,max_age_seconds,allow_validated_idp FROM organisation_mfa_policies WHERE organisation_id=$1 FOR SHARE")
        .bind(org).fetch_optional(&mut **tx).await?.ok_or_else(|| ApiError::Unavailable("organisation MFA policy is not initialized".into()))?;
    Ok(OrganisationMfaPolicy {
        organisation_id: row.get("organisation_id"),
        revision: row.get("revision"),
        epoch: row.get("epoch"),
        required: row.get("required"),
        max_age_seconds: row.get("max_age_seconds"),
        allow_validated_idp: row.get("allow_validated_idp"),
    })
}
pub(super) async fn inspect(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(org): Path<Uuid>,
) -> Result<Json<OrganisationMfaPolicy>, ApiError> {
    membership(&state, &principal, org, false).await?;
    let mut tx = state.pool.begin().await?;
    crate::organisation_mfa::session_in_tx(
        &state,
        &mut tx,
        principal.user_id,
        principal.session_id.ok_or_else(denied)?,
        false,
    )
    .await?;
    let p = policy(&mut tx, org).await?;
    tx.commit().await?;
    Ok(Json(p))
}
pub(super) async fn update(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(org): Path<Uuid>,
    Json(request): Json<UpdateOrganisationMfaPolicy>,
) -> Result<Json<OrganisationMfaReceipt>, ApiError> {
    if request.request_id.is_nil()
        || request.expected_revision < 1
        || !(300..=43200).contains(&request.max_age_seconds)
        || request.confirmation != "CHANGE_ORGANISATION_MFA"
    {
        return Err(invalid("confirm this exact organisation MFA policy change"));
    }
    let request_sha = hex::encode(Sha256::digest(serde_json::to_vec(&request)?));
    let sid = principal.session_id.ok_or_else(denied)?;
    let mut tx = state.pool.begin().await?;
    // Same org-authority lock as current SSO and ordinary policy changes.
    sso::authorize(&mut tx, org, &principal).await?;
    let session =
        crate::organisation_mfa::session_in_tx(&state, &mut tx, principal.user_id, sid, false)
            .await?;
    let original=sqlx::query("SELECT request_sha256,receipt_bytes FROM organisation_mfa_operations WHERE organisation_id=$1 AND actor_user_id=$2 AND request_id=$3")
        .bind(org).bind(principal.user_id).bind(request.request_id).fetch_optional(&mut *tx).await?;
    if let Some(original) = original {
        if original.get::<String, _>("request_sha256") != request_sha {
            return Err(ApiError::Conflict(
                "original MFA operation has a different request".into(),
            ));
        }
        let bytes: Vec<u8> = original.get("receipt_bytes");
        let receipt: OrganisationMfaReceipt = serde_json::from_slice(&bytes)?;
        if receipt.request_id != request.request_id
            || receipt.actor_user_id != principal.user_id
            || receipt.policy.organisation_id != org
        {
            return Err(ApiError::Conflict(
                "original MFA receipt binding is invalid".into(),
            ));
        }
        tx.commit().await?;
        return Ok(Json(receipt));
    }
    let row = sqlx::query(
        "SELECT revision FROM organisation_mfa_policies WHERE organisation_id=$1 FOR UPDATE",
    )
    .bind(org)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(denied)?;
    let previous: i64 = row.get("revision");
    if previous != request.expected_revision {
        return Err(ApiError::Conflict(
            "reload the organisation MFA policy before saving".into(),
        ));
    }
    // Changing the requirement always needs fresh genuine MFA, even when the
    // old policy was optional. A password-only admin cannot switch it off.
    let mut proof_policy = policy(&mut tx, org).await?;
    proof_policy.required = true;
    proof_policy.max_age_seconds = 600;
    if !crate::organisation_mfa::assured_in_tx(
        &state,
        &mut tx,
        principal.user_id,
        &session,
        &proof_policy,
    )
    .await?
    .0
    {
        return Err(ApiError::MfaRequired(
            "verify this administrator session within ten minutes before changing MFA policy"
                .into(),
        ));
    }
    // Keep one independently recoverable local administrator. Enabling IdP
    // assurance does not turn provider availability into recovery authority.
    if request.required {
        let unsupported:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_instances i LEFT JOIN management_instance_features f ON f.instance_id=i.id AND f.organisation_id=i.organisation_id WHERE i.organisation_id=$1 AND i.state='ready' AND (coalesce(f.mfa_version,0)<>1 OR f.refreshed_at<=clock_timestamp()-interval '45 seconds' OR f.refreshed_at IS NULL))").bind(org).fetch_one(&mut *tx).await?;
        if unsupported {
            return Err(ApiError::Conflict("every ready instance must report current API and worker MFA enforcement before activation".into()));
        }
        let recovery:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_members m JOIN users u ON u.id=m.user_id WHERE m.organisation_id=$1 AND m.role='admin' AND m.sso_provider_id IS NULL AND NOT m.directory_managed AND u.deleted_at IS NULL AND u.password_hash IS NOT NULL AND (EXISTS(SELECT 1 FROM user_totp t WHERE t.user_id=u.id AND t.enabled) OR EXISTS(SELECT 1 FROM passkeys p WHERE p.user_id=u.id)))")
            .bind(org).fetch_one(&mut *tx).await?;
        if !recovery {
            return Err(ApiError::Conflict("retain a manually assigned administrator with a local sign-in and enrolled factor before requiring organisation MFA".into()));
        }
    }
    let next = previous
        .checked_add(1)
        .ok_or_else(|| invalid("MFA revision exhausted"))?;
    let epoch = Uuid::new_v4();
    sqlx::query("UPDATE organisation_mfa_policies SET revision=$2,epoch=$3,required=$4,max_age_seconds=$5,allow_validated_idp=$6,updated_by=$7,updated_at=clock_timestamp() WHERE organisation_id=$1 AND revision=$8")
        .bind(org).bind(next).bind(epoch).bind(request.required).bind(request.max_age_seconds).bind(request.allow_validated_idp).bind(principal.user_id).bind(previous).execute(&mut *tx).await?;
    let receipt = OrganisationMfaReceipt {
        request_id: request.request_id,
        actor_user_id: principal.user_id,
        policy: OrganisationMfaPolicy {
            organisation_id: org,
            revision: next,
            epoch,
            required: request.required,
            max_age_seconds: request.max_age_seconds,
            allow_validated_idp: request.allow_validated_idp,
        },
        previous_revision: previous,
        applied_at: chrono::Utc::now(),
    };
    let bytes = serde_json::to_vec(&receipt)?;
    sqlx::query("INSERT INTO organisation_mfa_operations(organisation_id,actor_user_id,request_id,expected_revision,request_sha256,receipt_bytes) VALUES($1,$2,$3,$4,$5,$6)")
        .bind(org).bind(principal.user_id).bind(request.request_id).bind(previous).bind(request_sha).bind(bytes).execute(&mut *tx).await?;
    append_audit(
        &mut tx,
        org,
        principal.user_id,
        "organisation.mfa.changed",
        org,
        serde_json::to_value(&receipt)?,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(receipt))
}
/// No content is returned. Authenticated instance asks for this exact current
/// org and optionally its original management sign-in session.
pub(super) async fn authority(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<crate::organisation_mfa::AuthorityRequest>,
) -> Result<Json<crate::organisation_mfa::AuthorityResponse>, ApiError> {
    let mut authentication = state.pool.begin().await?;
    crate::fleet::verify_instance_key(
        &mut authentication,
        &headers,
        request.organisation_id,
        request.instance_id,
    )
    .await?;
    authentication.commit().await?;
    let mut tx = state.pool.begin().await?;
    let session = if let Some(sid) = request.management_session_id {
        Some(
            crate::organisation_mfa::session_in_tx(&state, &mut tx, request.user_id, sid, false)
                .await?,
        )
    } else {
        None
    };
    crate::fleet::verify_instance_key(
        &mut tx,
        &headers,
        request.organisation_id,
        request.instance_id,
    )
    .await?;
    let ready:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_instances WHERE id=$1 AND organisation_id=$2 AND state='ready' AND NOT EXISTS(SELECT 1 FROM management_instance_import_locks WHERE instance_id=$1))")
        .bind(request.instance_id).bind(request.organisation_id).fetch_one(&mut *tx).await?;
    if !ready {
        return Err(denied());
    }
    let policy = policy(&mut tx, request.organisation_id).await?;
    let (verified, verified_until) = if let Some(session) = session {
        crate::organisation_mfa::assured_in_tx(&state, &mut tx, request.user_id, &session, &policy)
            .await?
    } else {
        (false, None)
    };
    let access = crate::fleet::allowed_on(
        &mut *tx,
        &crate::fleet::AccessRequest {
            organisation_id: request.organisation_id,
            instance_id: request.instance_id,
            user_id: request.user_id,
        },
    )
    .await?
        || content::can_launch_on(
            &mut *tx,
            request.organisation_id,
            request.instance_id,
            request.user_id,
        )
        .await?
        || content::can_launch_viewer_on(
            &mut *tx,
            request.organisation_id,
            request.instance_id,
            request.user_id,
        )
        .await?;
    if !access {
        return Err(denied());
    }
    tx.commit().await?;
    Ok(Json(crate::organisation_mfa::AuthorityResponse {
        policy,
        management_session_id: request.management_session_id,
        user_id: request.user_id,
        instance_id: request.instance_id,
        verified,
        verified_until,
    }))
}
pub(crate) async fn attach(
    tx: &mut Transaction<'_, Postgres>,
    instance: &mut Value,
) -> Result<(), ApiError> {
    let org = serde_json::from_value(instance["organisation_id"].clone())?;
    instance["mfa_policy"] = serde_json::to_value(policy(tx, org).await?)?;
    Ok(())
}
