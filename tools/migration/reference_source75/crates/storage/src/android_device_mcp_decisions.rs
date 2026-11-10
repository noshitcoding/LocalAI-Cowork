//! Original human-session provenance for explicitly versioned device MCP requests.
//! Reads verify that same persisted session; they never construct a Principal.
use super::*;
use cowork_identity::Principal;
use sqlx::{Any, Transaction, any::AnyRow};

pub(crate) fn applies(value: &Value) -> bool {
    let action = &value["requested_action"];
    let format = if action["format"] == "device-original-action-reference/1" {
        action["original_format"].as_str()
    } else {
        action["format"].as_str()
    }
    .or_else(|| value["prompt"]["format"].as_str());
    matches!(
        format,
        Some(
            "device-mcp-task-interaction/1"
                | "device-mcp-task-cancel/1"
                | "device-mcp-sampling/1"
                | "device-mcp-task-release/1"
        )
    )
}

pub(crate) async fn lock_request(
    tx: &mut Transaction<'_, Any>,
    run: Uuid,
    request: Uuid,
    kind: &str,
) -> Result<()> {
    let raw: Option<String> = sqlx::query_scalar(
        "SELECT payload FROM android_scoped_requests WHERE id=$1 AND run_id=$2 AND kind=$3",
    )
    .bind(request.to_string())
    .bind(run.to_string())
    .bind(kind)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage_error)?;
    if raw
        .map(|raw| parse::<Value>(&raw))
        .transpose()?
        .as_ref()
        .is_some_and(applies)
        && sqlx::query("UPDATE identity_installation SET revision=revision WHERE id=1")
            .execute(&mut **tx)
            .await
            .map_err(storage_error)?
            .rows_affected()
            != 1
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}

fn run_binding(run: &AnyRow) -> Result<Value> {
    let payload: Value = parse(&run.try_get::<String, _>("payload").map_err(storage_error)?)?;
    let lease = run
        .try_get::<Option<String>, _>("lease_token")
        .map_err(storage_error)?
        .filter(|lease| !lease.is_empty())
        .ok_or(Error::Forbidden)?;
    Ok(
        json!({"run_id":run.try_get::<String,_>("id").map_err(storage_error)?,
        "project_id":run.try_get::<String,_>("project_id").map_err(storage_error)?,
        "team_id":run.try_get::<Option<String>,_>("team_id").map_err(storage_error)?,
        "creator_id":run.try_get::<String,_>("creator_id").map_err(storage_error)?,
        "spec_sha256":canonical_hash(&payload["spec"]),"lease_sha256":hash(lease.as_bytes())}),
    )
}

async fn context(
    tx: &mut Transaction<'_, Any>,
    run: &AnyRow,
    identity: &str,
    session: &str,
    current: i64,
) -> Result<Value> {
    let project = run
        .try_get::<String, _>("project_id")
        .map_err(storage_error)?;
    let team = run
        .try_get::<Option<String>, _>("team_id")
        .map_err(storage_error)?;
    if let Some(team) = team {
        let row = sqlx::query("SELECT s.revision AS session_revision,i.revision AS identity_revision,o.authorization_epoch,m.id AS membership_id,m.revision AS membership_revision,m.role,g.revision AS grant_revision,r.revision AS resource_revision,(SELECT organization_id FROM identity_session_scopes WHERE session_id=s.id) AS session_scope_org,(SELECT permissions FROM identity_session_scopes WHERE session_id=s.id) AS session_scope_permissions FROM identity_sessions s JOIN identity_subjects i ON i.id=s.identity_id JOIN identity_organizations o ON o.id=$3 JOIN identity_memberships m ON m.identity_id=i.id AND m.organization_id=o.id JOIN identity_resources r ON r.id=$4 AND r.organization_id=o.id JOIN identity_content_grants g ON g.membership_id=m.id AND g.resource_id=r.id WHERE s.id=$1 AND i.id=$2 AND s.revoked=0 AND s.expires_at>$5 AND i.active=1 AND o.state='active' AND m.active=1 AND m.role<>'viewer' AND g.active=1 AND (g.permissions & 5)=5 AND NOT EXISTS(SELECT 1 FROM identity_session_scopes ss WHERE ss.session_id=s.id AND (ss.organization_id<>o.id OR ss.permissions<>15))")
            .bind(session).bind(identity).bind(&team).bind(&project).bind(current)
            .fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        Ok(json!({"identity_id":identity,"session_id":session,
            "session_revision":row.try_get::<i64,_>("session_revision").map_err(storage_error)?,
            "identity_revision":row.try_get::<i64,_>("identity_revision").map_err(storage_error)?,
            "organization_epoch":row.try_get::<i64,_>("authorization_epoch").map_err(storage_error)?,
            "session_scope_org":row.try_get::<Option<String>,_>("session_scope_org").map_err(storage_error)?,
            "session_scope_permissions":row.try_get::<Option<i64>,_>("session_scope_permissions").map_err(storage_error)?,
            "membership_id":row.try_get::<String,_>("membership_id").map_err(storage_error)?,
            "membership_revision":row.try_get::<i64,_>("membership_revision").map_err(storage_error)?,
            "role":row.try_get::<String,_>("role").map_err(storage_error)?,
            "grant_revision":row.try_get::<i64,_>("grant_revision").map_err(storage_error)?,
            "resource_revision":row.try_get::<i64,_>("resource_revision").map_err(storage_error)?}))
    } else {
        if run
            .try_get::<String, _>("creator_id")
            .map_err(storage_error)?
            != identity
        {
            return Err(Error::Forbidden);
        }
        let row = sqlx::query("SELECT s.revision AS session_revision,i.revision AS identity_revision,p.revision AS project_revision FROM identity_sessions s JOIN identity_subjects i ON i.id=s.identity_id JOIN android_personal_projects p ON p.owner_id=i.id WHERE s.id=$1 AND i.id=$2 AND s.revoked=0 AND s.expires_at>$3 AND i.active=1 AND p.id=$4 AND p.deleted=0 AND NOT EXISTS(SELECT 1 FROM identity_session_scopes ss WHERE ss.session_id=s.id)")
            .bind(session).bind(identity).bind(current).bind(&project)
            .fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        Ok(json!({"identity_id":identity,"session_id":session,
            "session_revision":row.try_get::<i64,_>("session_revision").map_err(storage_error)?,
            "identity_revision":row.try_get::<i64,_>("identity_revision").map_err(storage_error)?,
            "project_revision":row.try_get::<i64,_>("project_revision").map_err(storage_error)?}))
    }
}

fn decision(value: &Value) -> Value {
    json!({"id":value["id"],"run_id":value["run_id"],"revision":value["revision"],
        "state":value["state"],"requested_action":value.get("requested_action"),
        "prompt":value.get("prompt"),"response":value.get("response"),
        "resolved_by_user_id":value.get("resolved_by_user_id"),
        "responded_by_user_id":value.get("responded_by_user_id")})
}

pub(crate) async fn capture(
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    run: &AnyRow,
    value: &mut Value,
    current: i64,
) -> Result<()> {
    if !applies(value) {
        return Ok(());
    }
    let context = context(
        tx,
        run,
        &actor.identity_id().to_string(),
        &actor.session_id().to_string(),
        current,
    )
    .await?;
    value["mcp_decision_proof"] = json!({"format":"android-device-mcp-decision/1",
        "context":context,"run":run_binding(run)?,"decision_sha256":canonical_hash(&decision(value))});
    Ok(())
}

pub(crate) async fn validate(
    tx: &mut Transaction<'_, Any>,
    run: &AnyRow,
    value: &Value,
    current: i64,
) -> Result<()> {
    if !applies(value) || !matches!(value["state"].as_str(), Some("submitted" | "approved")) {
        return Ok(());
    }
    let proof = &value["mcp_decision_proof"];
    let identity = proof["context"]["identity_id"]
        .as_str()
        .ok_or(Error::Forbidden)?;
    let session = proof["context"]["session_id"]
        .as_str()
        .ok_or(Error::Forbidden)?;
    if proof["format"] != "android-device-mcp-decision/1"
        || proof["run"] != run_binding(run)?
        || proof["decision_sha256"] != canonical_hash(&decision(value))
        || proof["context"] != context(tx, run, identity, session, current).await?
        || value["responded_by_user_id"]
            .as_str()
            .or_else(|| value["resolved_by_user_id"].as_str())
            != Some(identity)
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}
