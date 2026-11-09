//! Short-lived, instance-bound sign-in tickets. Passwords never leave management.
use crate::{auth, error::ApiError, AppState};
use axum::{extract::State, http::HeaderMap, routing::post, Json, Router};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::Utc;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;
use sqlx::{Executor, Postgres, Row, Transaction};
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchTicket {
    pub purpose: String,
    pub organisation_id: Uuid,
    pub instance_id: Uuid,
    pub user_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub nonce: Uuid,
    pub expires_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub management_session_id: Option<Uuid>,
}

fn denied() -> ApiError {
    ApiError::Unauthorized("instance access is not authorised".into())
}
pub(crate) fn secret(name: &str) -> Result<String, ApiError> {
    let path = std::env::var(name).map_err(|_| denied())?;
    let key = std::fs::read_to_string(path).map_err(|e| ApiError::Internal(e.into()))?;
    if key.trim().len() < 32 {
        return Err(denied());
    }
    Ok(key.trim().into())
}
pub fn enabled() -> bool {
    std::env::var_os("COWORK_FLEET_KEY_FILE").is_some()
}
pub fn instance_key(org: Uuid, instance: Uuid) -> Result<String, ApiError> {
    let key = secret("COWORK_PROVISIONER_TOKEN_FILE")?;
    derive_instance_key(&key, org, instance, None)
}
fn derive_instance_key(
    key: &str,
    org: Uuid,
    instance: Uuid,
    epoch: Option<Uuid>,
) -> Result<String, ApiError> {
    let mut mac = HmacSha256::new_from_slice(key.as_bytes()).map_err(|_| denied())?;
    let scope = match epoch {
        Some(epoch) if !epoch.is_nil() => {
            format!("cowork-federation-v2\0{org}\0{instance}\0{epoch}")
        }
        Some(_) => return Err(denied()),
        None => format!("cowork-federation-v1\0{org}\0{instance}"),
    };
    mac.update(scope.as_bytes());
    Ok(mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}
pub(crate) fn epoch_key(org: Uuid, instance: Uuid, epoch: Uuid) -> Result<String, ApiError> {
    derive_instance_key(
        &secret("COWORK_PROVISIONER_TOKEN_FILE")?,
        org,
        instance,
        Some(epoch),
    )
}
pub(crate) async fn current_instance_key(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
    instance: Uuid,
) -> Result<String, ApiError> {
    crate::management::organisation_lifecycle::share_active(tx, org).await?;
    // READ COMMITTED returns the updated row after a blocked promotion. Holding
    // SHARE through the caller's commit prevents promotion during authorization.
    let row = sqlx::query("/* fleet-current-authority */ SELECT execution_required,execution_epoch FROM management_instances WHERE organisation_id=$1 AND id=$2 AND state NOT IN ('deleted','deleting') FOR SHARE")
        .bind(org).bind(instance).fetch_optional(&mut **tx).await?.ok_or_else(denied)?;
    if row.try_get::<bool, _>("execution_required")? {
        epoch_key(org, instance, row.try_get("execution_epoch")?)
    } else {
        instance_key(org, instance)
    }
}
pub(crate) async fn verify_execution_receipt_key(
    tx: &mut Transaction<'_, Postgres>,
    headers: &HeaderMap,
    org: Uuid,
    instance: Uuid,
    epoch: Uuid,
    version: i16,
) -> Result<(), ApiError> {
    // A confirmed old reservation may be settled after promotion or deletion.
    // Its caller must still match the original reservation and full receipt.
    let _: Uuid = sqlx::query_scalar(
        "SELECT id FROM management_instances WHERE organisation_id=$1 AND id=$2 FOR SHARE",
    )
    .bind(org)
    .bind(instance)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(denied)?;
    let key = match version {
        1 => instance_key(org, instance)?,
        2 => epoch_key(org, instance, epoch)?,
        _ => return Err(denied()),
    };
    verify_key(headers, &key)
}
pub fn sign(ticket: &LaunchTicket, key: &str) -> Result<String, ApiError> {
    let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(ticket)?);
    let mut mac = HmacSha256::new_from_slice(key.as_bytes()).map_err(|_| denied())?;
    mac.update(payload.as_bytes());
    Ok(format!(
        "{payload}.{}",
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    ))
}
fn verify(token: &str, key: &str, org: Uuid, instance: Uuid) -> Result<LaunchTicket, ApiError> {
    if token.len() > 4096 {
        return Err(denied());
    }
    let (payload, signature) = token.split_once('.').ok_or_else(denied)?;
    let mut mac = HmacSha256::new_from_slice(key.as_bytes()).map_err(|_| denied())?;
    mac.update(payload.as_bytes());
    mac.verify_slice(&URL_SAFE_NO_PAD.decode(signature).map_err(|_| denied())?)
        .map_err(|_| denied())?;
    let ticket: LaunchTicket =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).map_err(|_| denied())?)
            .map_err(|_| denied())?;
    let now = Utc::now().timestamp();
    if !["instance_login", "instance_support", "instance_viewer"].contains(&ticket.purpose.as_str())
        || ticket.organisation_id != org
        || ticket.instance_id != instance
        || ticket.expires_at <= now
        || ticket.expires_at > now + 90
        || ticket.email.is_empty()
        || ticket.email.len() > 320
        || ticket.display_name.len() > 200
    {
        return Err(denied());
    }
    Ok(ticket)
}

fn issued_after_restore(ticket: &LaunchTicket, not_before: i64) -> bool {
    // Management issues tickets for exactly 60 seconds. Keep the existing wire
    // format so rolling management updates still support older instance APIs.
    ticket.expires_at.saturating_sub(60) > not_before
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AccessRequest {
    pub organisation_id: Uuid,
    pub instance_id: Uuid,
    pub user_id: Uuid,
}

pub async fn allowed(state: &AppState, request: &AccessRequest) -> Result<bool, ApiError> {
    allowed_on(&state.pool, request).await
}
pub(crate) async fn allowed_on<'e, E>(
    executor: E,
    request: &AccessRequest,
) -> Result<bool, ApiError>
where
    E: Executor<'e, Database = Postgres>,
{
    // Infrastructure administrators are not implicitly instance content readers.
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_instances i JOIN management_organisations o ON o.id=i.organisation_id AND o.archived_at IS NULL JOIN management_members m ON m.organisation_id=i.organisation_id AND m.user_id=$3 JOIN users u ON u.id=m.user_id WHERE i.organisation_id=$1 AND i.id=$2 AND i.state='ready' AND NOT EXISTS(SELECT 1 FROM management_instance_import_locks il WHERE il.instance_id=i.id) AND u.deleted_at IS NULL AND m.role IN ('admin','member') AND (i.owner_user_id=$3 OR EXISTS(SELECT 1 FROM management_instance_members im WHERE im.instance_id=i.id AND im.user_id=$3 AND im.role IN ('manager','member'))))")
        .bind(request.organisation_id).bind(request.instance_id).bind(request.user_id).fetch_one(executor).await?)
}

fn verify_key(headers: &HeaderMap, expected: &str) -> Result<(), ApiError> {
    use subtle::ConstantTimeEq;
    let supplied = headers
        .get("x-cowork-instance-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !bool::from(expected.as_bytes().ct_eq(supplied.as_bytes())) {
        return Err(denied());
    }
    Ok(())
}
pub(crate) async fn verify_instance_key(
    tx: &mut Transaction<'_, Postgres>,
    headers: &HeaderMap,
    org: Uuid,
    instance: Uuid,
) -> Result<(), ApiError> {
    verify_key(headers, &current_instance_key(tx, org, instance).await?)
}
pub async fn inspect_access(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<AccessRequest>,
) -> Result<Json<Value>, ApiError> {
    let mut tx = state.pool.begin().await?;
    verify_instance_key(
        &mut tx,
        &headers,
        request.organisation_id,
        request.instance_id,
    )
    .await?;
    let policy = crate::management::mfa::policy(&mut tx, request.organisation_id).await?;
    let permitted = !policy.required && allowed_on(&mut *tx, &request).await?;
    tx.commit().await?;
    Ok(Json(json!({"allowed": permitted})))
}

pub async fn validate_access(user_id: Uuid) -> Result<(), ApiError> {
    crate::fleet_content::context(user_id, None).await?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchRequest {
    ticket: String,
    device_id: Uuid,
}

pub fn router() -> Router<AppState> {
    Router::new().route("/auth/fleet", post(accept_launch))
}
async fn accept_launch(
    State(state): State<AppState>,
    Json(request): Json<LaunchRequest>,
) -> Result<Json<cowork_contracts::AuthTokens>, ApiError> {
    let org = std::env::var("COWORK_ORGANISATION_ID")
        .map_err(|_| denied())?
        .parse()
        .map_err(|_| denied())?;
    let instance = std::env::var("COWORK_INSTANCE_ID")
        .map_err(|_| denied())?
        .parse()
        .map_err(|_| denied())?;
    let ticket = verify(
        &request.ticket,
        &secret("COWORK_FLEET_KEY_FILE")?,
        org,
        instance,
    )?;
    if let Ok(value) = std::env::var("COWORK_FLEET_LAUNCH_NOT_BEFORE") {
        let not_before = value.parse::<i64>().map_err(|_| denied())?;
        if !issued_after_restore(&ticket, not_before) {
            return Err(denied());
        }
    }
    validate_access(ticket.user_id).await?;
    let mut tx = state.pool.begin().await?;
    sqlx::query("INSERT INTO users(id,etag,email,display_name,platform_admin) VALUES($1,$2,$3,$4,FALSE) ON CONFLICT(id) DO NOTHING")
        .bind(ticket.user_id).bind(format!("W/\"{}:1\"", ticket.user_id)).bind(ticket.email).bind(ticket.display_name).execute(&mut *tx).await?;
    let inserted = sqlx::query("INSERT INTO instance_launch_nonces(nonce,user_id,expires_at) VALUES($1,$2,to_timestamp($3)) ON CONFLICT DO NOTHING")
        .bind(ticket.nonce).bind(ticket.user_id).bind(ticket.expires_at as f64).execute(&mut *tx).await?.rows_affected();
    if inserted != 1 {
        return Err(denied());
    }
    sqlx::query("DELETE FROM instance_launch_nonces WHERE expires_at < now() - interval '1 day'")
        .execute(&mut *tx)
        .await?;
    let tokens = auth::create_session_tx(&mut tx, ticket.user_id, request.device_id).await?;
    sqlx::query("UPDATE auth_sessions SET auth_origin=$2 WHERE id=$1")
        .bind(tokens.session_id)
        .bind(json!({"version":1,"kind":"instance_launch","management_session_id":ticket.management_session_id,"organisation_id":org,"instance_id":instance}))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(tokens))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generation_keys_keep_legacy_explicit_and_never_accept_old_tickets() {
        let org = Uuid::new_v4();
        let instance = Uuid::new_v4();
        let old = Uuid::new_v4();
        let new = Uuid::new_v4();
        let root = "fixture-only-manager-root-key-0000000000000000";
        let legacy = derive_instance_key(root, org, instance, None).unwrap();
        let before = derive_instance_key(root, org, instance, Some(old)).unwrap();
        let after = derive_instance_key(root, org, instance, Some(new)).unwrap();
        assert_ne!(legacy, before);
        assert_ne!(before, after);
        assert_ne!(
            after,
            derive_instance_key(root, org, Uuid::new_v4(), Some(new)).unwrap()
        );
        assert_ne!(
            after,
            derive_instance_key(root, Uuid::new_v4(), instance, Some(new)).unwrap()
        );
        let ticket = LaunchTicket {
            purpose: "instance_login".into(),
            organisation_id: org,
            instance_id: instance,
            user_id: Uuid::new_v4(),
            email: "fixture@example.invalid".into(),
            display_name: "Fixture".into(),
            nonce: Uuid::new_v4(),
            expires_at: Utc::now().timestamp() + 60,
            management_session_id: None,
        };
        assert!(verify(&sign(&ticket, &before).unwrap(), &after, org, instance).is_err());
        assert!(verify(&sign(&ticket, &after).unwrap(), &after, org, instance).is_ok());
        assert!(derive_instance_key(root, org, instance, Some(Uuid::nil())).is_err());
    }

    #[tokio::test]
    #[ignore = "requires own isolated PostgreSQL fixture and scoped key file"]
    async fn postgres_epoch_keys_callbacks_lock_wait_and_release_scope() {
        use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
        let url = std::env::var("COWORK_TEST_DATABASE_URL").unwrap();
        let cleanup = PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        let schema = format!("cowork_fleet_epoch_{}", Uuid::new_v4().simple());
        sqlx::raw_sql(&format!("CREATE SCHEMA {schema}"))
            .execute(&cleanup)
            .await
            .unwrap();
        let options: PgConnectOptions = url.parse().unwrap();
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect_with(options.options([("search_path", schema.as_str())]))
            .await
            .unwrap();
        struct KeyGuard(std::path::PathBuf, Option<std::ffi::OsString>);
        impl Drop for KeyGuard {
            fn drop(&mut self) {
                match &self.1 {
                    Some(value) => std::env::set_var("COWORK_PROVISIONER_TOKEN_FILE", value),
                    None => std::env::remove_var("COWORK_PROVISIONER_TOKEN_FILE"),
                };
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let keyfile = std::env::temp_dir().join(format!("cowork-fleet-epoch-{}", Uuid::new_v4()));
        let _key_guard = KeyGuard(
            keyfile.clone(),
            std::env::var_os("COWORK_PROVISIONER_TOKEN_FILE"),
        );
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&keyfile)
            .unwrap();
        file.write_all(b"fixture-only-manager-root-key-0000000000000000")
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&keyfile, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        std::env::set_var("COWORK_PROVISIONER_TOKEN_FILE", &keyfile);
        let result=tokio::spawn(async move {
            sqlx::raw_sql("CREATE TABLE management_organisations(id UUID PRIMARY KEY,archived_at TIMESTAMPTZ)").execute(&pool).await.unwrap();
            sqlx::raw_sql("CREATE TABLE management_instances(id UUID PRIMARY KEY,organisation_id UUID NOT NULL,state TEXT NOT NULL,execution_required BOOLEAN NOT NULL,execution_epoch UUID NOT NULL)").execute(&pool).await.unwrap();
            let org=Uuid::new_v4();let instance=Uuid::new_v4();let legacy=Uuid::new_v4();let old=Uuid::new_v4();let new=Uuid::new_v4();
            sqlx::query("INSERT INTO management_organisations(id) VALUES($1)").bind(org).execute(&pool).await.unwrap();
            for (id,required) in [(instance,true),(legacy,false)] {
                sqlx::query("INSERT INTO management_instances VALUES($1,$2,'ready',$3,$4)").bind(id).bind(org).bind(required).bind(old).execute(&pool).await.unwrap();
            }
            let header=|key:String|{let mut value=HeaderMap::new();value.insert("x-cowork-instance-key",key.parse().unwrap());value};
            let before=header(epoch_key(org,instance,old).unwrap());
            let after=header(epoch_key(org,instance,new).unwrap());
            let legacy_header=header(instance_key(org,legacy).unwrap());
            let mut tx=pool.begin().await.unwrap();
            assert!(verify_instance_key(&mut tx,&before,org,instance).await.is_ok());
            assert!(verify_instance_key(&mut tx,&legacy_header,org,legacy).await.is_ok());
            assert!(verify_instance_key(&mut tx,&header(instance_key(org,instance).unwrap()),org,instance).await.is_err());
            assert!(verify_instance_key(&mut tx,&before,org,legacy).await.is_err());
            tx.commit().await.unwrap();
            let mut blocker=pool.begin().await.unwrap();
            sqlx::query("UPDATE management_instances SET execution_epoch=$2 WHERE id=$1").bind(instance).bind(new).execute(&mut *blocker).await.unwrap();
            let waiting_pool=pool.clone();let waiting_header=before.clone();
            let waiting=tokio::spawn(async move {let mut tx=waiting_pool.begin().await.unwrap();let denied=verify_instance_key(&mut tx,&waiting_header,org,instance).await.is_err();tx.rollback().await.unwrap();denied});
            tokio::time::timeout(std::time::Duration::from_secs(5),async {
                loop {let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid<>pg_backend_pid() AND datname=current_database() AND wait_event_type='Lock' AND query LIKE '%fleet-current-authority%')").fetch_one(&pool).await.unwrap();
                    if blocked {break;}tokio::time::sleep(std::time::Duration::from_millis(10)).await;}
            }).await.unwrap();
            assert!(!waiting.is_finished());blocker.commit().await.unwrap();
            assert!(tokio::time::timeout(std::time::Duration::from_secs(5),waiting).await.unwrap().unwrap());
            let mut tx=pool.begin().await.unwrap();
            assert!(verify_instance_key(&mut tx,&after,org,instance).await.is_ok());
            assert!(verify_instance_key(&mut tx,&before,org,instance).await.is_err());
            assert!(verify_execution_receipt_key(&mut tx,&before,org,instance,old,2).await.is_ok());
            assert!(verify_execution_receipt_key(&mut tx,&after,org,instance,old,2).await.is_err());
            assert!(verify_execution_receipt_key(&mut tx,&before,org,legacy,old,2).await.is_err());
            tx.commit().await.unwrap();
            println!("fleet-epoch-check: current and legacy scopes, old key denial after verified row-lock wait, exact old receipt epoch accepted, wrong receipt key denied");
            pool.close().await;
        }).await;
        sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&cleanup)
            .await
            .unwrap();
        cleanup.close().await;
        result.unwrap();
    }
    #[test]
    fn tickets_are_bound_to_purpose_instance_org_key_and_expiry() {
        let mut ticket = LaunchTicket {
            purpose: "instance_login".into(),
            organisation_id: Uuid::new_v4(),
            instance_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            email: "member@example.test".into(),
            display_name: "Member".into(),
            nonce: Uuid::new_v4(),
            expires_at: Utc::now().timestamp() + 60,
            management_session_id: None,
        };
        let token = sign(&ticket, "test-key").unwrap();
        let issued = ticket.expires_at - 60;
        assert!(!issued_after_restore(&ticket, issued));
        assert!(!issued_after_restore(&ticket, issued + 10));
        assert!(issued_after_restore(&ticket, issued - 1));
        assert!(verify(
            &token,
            "test-key",
            ticket.organisation_id,
            ticket.instance_id
        )
        .is_ok());
        assert!(verify(
            &token,
            "other-key",
            ticket.organisation_id,
            ticket.instance_id
        )
        .is_err());
        assert!(verify(&token, "test-key", Uuid::new_v4(), ticket.instance_id).is_err());
        assert!(verify(&token, "test-key", ticket.organisation_id, Uuid::new_v4()).is_err());
        ticket.expires_at = Utc::now().timestamp() - 1;
        assert!(verify(
            &sign(&ticket, "test-key").unwrap(),
            "test-key",
            ticket.organisation_id,
            ticket.instance_id
        )
        .is_err());
        ticket.expires_at = Utc::now().timestamp() + 60;
        ticket.purpose = "provision".into();
        assert!(verify(
            &sign(&ticket, "test-key").unwrap(),
            "test-key",
            ticket.organisation_id,
            ticket.instance_id
        )
        .is_err());
    }
}
