//! Scoped instance callbacks. No project/chat contents are stored centrally.
use super::*;
use crate::fleet_content::{ContentAccessRequest, ContentAudit, ContentCapability};
use cowork_contracts::enterprise::{InstanceContentAccess, SupportAdministrator};

pub(crate) async fn context(
    tx: &mut Transaction<'_, Postgres>,
    scope: &crate::fleet::AccessRequest,
) -> Result<InstanceContentAccess, ApiError> {
    let row = sqlx::query("SELECT i.policy, o.policy AS org_policy, m.role, m.content_admin FROM management_instances i JOIN management_organisations o ON o.id=i.organisation_id JOIN management_members m ON m.organisation_id=i.organisation_id AND m.user_id=$3 JOIN users u ON u.id=m.user_id WHERE i.organisation_id=$1 AND i.id=$2 AND i.state='ready' AND o.archived_at IS NULL AND NOT EXISTS(SELECT 1 FROM management_instance_import_locks il WHERE il.instance_id=i.id) AND u.deleted_at IS NULL AND m.role IN ('admin','member')")
        .bind(scope.organisation_id).bind(scope.instance_id).bind(scope.user_id).fetch_optional(&mut **tx).await?.ok_or_else(denied)?;
    let policy: PolicyRevision = serde_json::from_value(row.get("policy"))?;
    let org_policy: PolicyRevision = serde_json::from_value(row.get("org_policy"))?;
    let effective = policy
        .restricted_by(&org_policy)
        .restricted_by(platform_ceiling());
    let administrator = row.get::<String, _>("role") == "admin";
    let member = crate::fleet::allowed_on(&mut **tx, scope).await?;
    let viewer=sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM management_instance_members WHERE organisation_id=$1 AND instance_id=$2 AND user_id=$3 AND role='viewer')")
        .bind(scope.organisation_id).bind(scope.instance_id).bind(scope.user_id).fetch_one(&mut **tx).await? && !member;
    if !administrator && !member && !viewer {
        return Err(denied());
    }
    let admins=sqlx::query_as::<_,(Uuid,String,String)>("SELECT u.id,u.display_name,u.email FROM management_members m JOIN users u ON u.id=m.user_id WHERE m.organisation_id=$1 AND m.role='admin' AND u.deleted_at IS NULL ORDER BY u.display_name,u.id")
        .bind(scope.organisation_id).fetch_all(&mut **tx).await?;
    Ok(InstanceContentAccess {
        managed: true,
        organisation_id: Some(scope.organisation_id),
        instance_id: Some(scope.instance_id),
        policy_revision: Some(org_policy.revision),
        content_access: Some(effective.content_access),
        read_only: !member,
        instance_viewer: viewer,
        support_administrator: administrator,
        automatic_content_access: administrator
            && row.get::<bool, _>("content_admin")
            && effective.content_access == ContentAccess::OrganisationAdmin,
        administrators: admins
            .into_iter()
            .map(|(user_id, display_name, email)| SupportAdministrator {
                user_id,
                display_name,
                email,
            })
            .collect(),
    })
}

pub(crate) async fn can_launch_viewer(
    state: &AppState,
    org: Uuid,
    instance: Uuid,
    user: Uuid,
) -> Result<bool, ApiError> {
    can_launch_viewer_on(&state.pool, org, instance, user).await
}
pub(crate) async fn can_launch_viewer_on<'e, E>(
    executor: E,
    org: Uuid,
    instance: Uuid,
    user: Uuid,
) -> Result<bool, ApiError>
where
    E: sqlx::Executor<'e, Database = Postgres>,
{
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_instances i JOIN management_organisations o ON o.id=i.organisation_id JOIN management_members m ON m.organisation_id=i.organisation_id AND m.user_id=$3 JOIN users u ON u.id=m.user_id JOIN management_instance_members im ON im.instance_id=i.id AND im.user_id=u.id JOIN management_instance_features f ON f.instance_id=i.id WHERE i.organisation_id=$1 AND i.id=$2 AND i.state='ready' AND o.archived_at IS NULL AND NOT EXISTS(SELECT 1 FROM management_instance_import_locks il WHERE il.instance_id=i.id) AND m.role IN ('admin','member') AND im.role='viewer' AND u.deleted_at IS NULL AND f.content_access_version>=2 AND f.refreshed_at>now()-interval '90 seconds')")
        .bind(org).bind(instance).bind(user).fetch_one(executor).await?)
}

pub(crate) async fn can_launch(
    state: &AppState,
    org: Uuid,
    instance: Uuid,
    user: Uuid,
) -> Result<bool, ApiError> {
    can_launch_on(&state.pool, org, instance, user).await
}
pub(crate) async fn can_launch_on<'e, E>(
    executor: E,
    org: Uuid,
    instance: Uuid,
    user: Uuid,
) -> Result<bool, ApiError>
where
    E: sqlx::Executor<'e, Database = Postgres>,
{
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_instances i JOIN management_organisations o ON o.id=i.organisation_id JOIN management_members m ON m.organisation_id=i.organisation_id AND m.user_id=$3 JOIN users u ON u.id=m.user_id JOIN management_instance_features f ON f.instance_id=i.id WHERE i.organisation_id=$1 AND i.id=$2 AND i.state='ready' AND o.archived_at IS NULL AND NOT EXISTS(SELECT 1 FROM management_instance_import_locks il WHERE il.instance_id=i.id) AND m.role='admin' AND u.deleted_at IS NULL AND f.refreshed_at>now()-interval '90 seconds')")
        .bind(org).bind(instance).bind(user).fetch_one(executor).await?)
}

pub(crate) async fn inspect(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ContentAccessRequest>,
) -> Result<Json<InstanceContentAccess>, ApiError> {
    let mut tx = state.pool.begin().await?;
    crate::fleet::verify_instance_key(
        &mut tx,
        &headers,
        request.scope.organisation_id,
        request.scope.instance_id,
    )
    .await?;
    let mfa = super::mfa::policy(&mut tx, request.scope.organisation_id).await?;
    if mfa.required && request.mfa_contract != Some(1) {
        return Err(ApiError::MfaRequired(
            "instance does not enforce the current organisation MFA policy".into(),
        ));
    }
    let info = context(&mut tx, &request.scope).await?;
    if let Some(ContentAudit {
        target_type,
        target_id,
        grant_id,
    }) = request.audit
    {
        if !info.support_administrator || (grant_id.is_none() && !info.automatic_content_access) {
            return Err(denied());
        }
        append_audit(&mut tx,request.scope.organisation_id,request.scope.user_id,"content.read_authorized",target_id,
            json!({"instance_id":request.scope.instance_id,"target_type":target_type,"support_grant_id":grant_id,"policy_revision":info.policy_revision})).await?;
    }
    tx.commit().await?;
    Ok(Json(info))
}

pub(crate) async fn announce(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ContentCapability>,
) -> Result<Json<Value>, ApiError> {
    let mut tx = state.pool.begin().await?;
    crate::fleet::verify_instance_key(
        &mut tx,
        &headers,
        request.organisation_id,
        request.instance_id,
    )
    .await?;
    if ![1, 2].contains(&request.version) || request.mfa_version.is_some_and(|v| v != 1) {
        return Err(invalid("unsupported content access version"));
    }
    let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_instances WHERE organisation_id=$1 AND id=$2 AND state NOT IN ('deleted','deleting'))")
        .bind(request.organisation_id).bind(request.instance_id).fetch_one(&mut *tx).await?;
    if !exists {
        return Err(denied());
    }
    sqlx::query("INSERT INTO management_instance_features(organisation_id,instance_id,content_access_version,mfa_version) VALUES($1,$2,$3,$4) ON CONFLICT(instance_id) DO UPDATE SET refreshed_at=now(),content_access_version=EXCLUDED.content_access_version,mfa_version=EXCLUDED.mfa_version")
        .bind(request.organisation_id).bind(request.instance_id).bind(i32::from(request.version)).bind(i16::from(request.mfa_version.unwrap_or(0))).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(json!({"version":request.version})))
}
