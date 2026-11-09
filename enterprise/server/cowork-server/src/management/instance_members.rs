//! Membership metadata stays central; it never grants access to every project.
use super::*;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceMember {
    user_id: Uuid,
    role: String,
}

// Organisation mutations use this same row lock. Recheck the acting identity
// after locking so a queued membership request cannot outlive its authority.
pub(super) async fn authorize(
    tx: &mut Transaction<'_, Postgres>,
    principal: &Principal,
    org: Uuid,
    instance: Uuid,
) -> Result<Uuid, ApiError> {
    authorize_inner(tx, principal, org, instance, false).await
}
pub(super) async fn authorize_deletion_recovery(
    tx: &mut Transaction<'_, Postgres>,
    principal: &Principal,
    org: Uuid,
    instance: Uuid,
) -> Result<Uuid, ApiError> {
    authorize_inner(tx, principal, org, instance, true).await
}
async fn authorize_inner(
    tx: &mut Transaction<'_, Postgres>,
    principal: &Principal,
    org: Uuid,
    instance: Uuid,
    deleting: bool,
) -> Result<Uuid, ApiError> {
    if principal.bootstrap {
        return Err(denied());
    }
    organisation_lifecycle::lock_active(tx, org).await?;
    let owner=sqlx::query_scalar::<_,Uuid>("SELECT i.owner_user_id FROM management_instances i JOIN users u ON u.id=$3 LEFT JOIN management_members m ON m.organisation_id=i.organisation_id AND m.user_id=u.id WHERE i.organisation_id=$1 AND i.id=$2 AND i.state<>'deleted' AND (i.state<>'deleting' OR $4) AND u.deleted_at IS NULL AND (u.platform_admin OR m.role='admin' OR (m.role='member' AND (i.owner_user_id=u.id OR EXISTS(SELECT 1 FROM management_instance_members im WHERE im.instance_id=i.id AND im.user_id=u.id AND im.role='manager'))))")
        .bind(org).bind(instance).bind(principal.user_id).bind(deleting).fetch_optional(&mut **tx).await?.ok_or_else(denied)?;
    Ok(owner)
}

pub(super) async fn set_instance_member(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, id)): Path<(Uuid, Uuid)>,
    Json(request): Json<InstanceMember>,
) -> Result<Json<Value>, ApiError> {
    if !["manager", "member", "viewer"].contains(&request.role.as_str()) {
        return Err(invalid("unknown instance role"));
    }
    let mut tx = state.pool.begin().await?;
    let owner = authorize(&mut tx, &principal, org, id).await?;
    if request.user_id == owner {
        return Err(invalid(
            "instance ownership is separate from assigned membership",
        ));
    }
    let eligible:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_members m JOIN users u ON u.id=m.user_id WHERE m.organisation_id=$1 AND m.user_id=$2 AND m.role IN ('admin','member') AND u.deleted_at IS NULL)")
        .bind(org).bind(request.user_id).fetch_one(&mut *tx).await?;
    if !eligible {
        return Err(invalid(
            "select an active non-auditor member of this organisation",
        ));
    }
    sqlx::query("INSERT INTO management_instance_members(organisation_id,instance_id,user_id,role) VALUES($1,$2,$3,$4) ON CONFLICT(instance_id,user_id) DO UPDATE SET role=EXCLUDED.role")
        .bind(org).bind(id).bind(request.user_id).bind(&request.role).execute(&mut *tx).await?;
    append_audit(
        &mut tx,
        org,
        principal.user_id,
        "instance.member.update",
        id,
        json!({"user_id":request.user_id,"role":request.role}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"user_id":request.user_id,"role":request.role})))
}

pub(super) async fn remove_instance_member(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, id, user)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Json<Value>, ApiError> {
    let mut tx = state.pool.begin().await?;
    let owner = authorize(&mut tx, &principal, org, id).await?;
    if owner == user {
        return Err(invalid(
            "instance ownership cannot be removed as an assignment",
        ));
    }
    let removed=sqlx::query("DELETE FROM management_instance_members WHERE organisation_id=$1 AND instance_id=$2 AND user_id=$3")
        .bind(org).bind(id).bind(user).execute(&mut *tx).await?.rows_affected()>0;
    if removed {
        append_audit(
            &mut tx,
            org,
            principal.user_id,
            "instance.member.remove",
            id,
            json!({"user_id":user}),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(Json(json!({"user_id":user,"removed":removed})))
}

pub(super) async fn instance_member_candidates(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<SupportAdministrator>>, ApiError> {
    instance_access(&state, &principal, org, id, true).await?;
    let rows=sqlx::query_as::<_,(Uuid,String,String)>("SELECT u.id,u.display_name,u.email FROM management_members m JOIN users u ON u.id=m.user_id JOIN management_instances i ON i.organisation_id=m.organisation_id AND i.id=$2 WHERE m.organisation_id=$1 AND m.role IN ('admin','member') AND u.deleted_at IS NULL AND u.id<>i.owner_user_id ORDER BY u.display_name,u.id")
        .bind(org).bind(id).fetch_all(&state.pool).await?;
    Ok(Json(
        rows.into_iter()
            .map(|(user_id, display_name, email)| SupportAdministrator {
                user_id,
                display_name,
                email,
            })
            .collect(),
    ))
}

pub(super) async fn instance_members(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((org, id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<InstanceMemberRecord>>, ApiError> {
    instance_access(&state, &principal, org, id, false).await?;
    let values:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('user_id',u.id,'email',u.email,'display_name',u.display_name,'role',CASE WHEN u.id=i.owner_user_id THEN 'owner' ELSE im.role END,'organisation_role',m.role) FROM management_instances i JOIN management_members m ON m.organisation_id=i.organisation_id JOIN users u ON u.id=m.user_id LEFT JOIN management_instance_members im ON im.instance_id=i.id AND im.user_id=u.id WHERE i.organisation_id=$1 AND i.id=$2 AND u.deleted_at IS NULL AND (u.id=i.owner_user_id OR im.user_id IS NOT NULL) ORDER BY (u.id=i.owner_user_id) DESC,u.display_name,u.id")
        .bind(org).bind(id).fetch_all(&state.pool).await?;
    Ok(Json(
        values
            .into_iter()
            .map(serde_json::from_value)
            .collect::<Result<_, _>>()?,
    ))
}
