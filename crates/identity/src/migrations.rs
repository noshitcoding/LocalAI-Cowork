//! Separate tenant schema. The application explicitly registers this migration;
//! creating these tables never adopts or modifies personal single-owner data.
use crate::{IdentityError, Result};
use sqlx::{Any, Row, Transaction};

pub const SCHEMA_VERSION: i64 = 1;
pub const STATEMENTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS identity_schema_guard (version BIGINT PRIMARY KEY CHECK(version = 1))",
    "CREATE TABLE IF NOT EXISTS identity_installation (id BIGINT PRIMARY KEY CHECK(id = 1), bootstrap_sha256 TEXT NOT NULL, settlement_sha256 TEXT NOT NULL, bootstrapped BIGINT NOT NULL CHECK(bootstrapped IN (0,1)), utc_highwater BIGINT NOT NULL, revision BIGINT NOT NULL CHECK(revision > 0))",
    "CREATE TABLE IF NOT EXISTS identity_subjects (id TEXT PRIMARY KEY, username TEXT NOT NULL UNIQUE, display_name TEXT NOT NULL, active BIGINT NOT NULL CHECK(active IN (0,1)), platform_admin BIGINT NOT NULL CHECK(platform_admin IN (0,1)), revision BIGINT NOT NULL CHECK(revision > 0), password_salt TEXT NOT NULL, password_hash TEXT NOT NULL)",
    "CREATE TABLE IF NOT EXISTS identity_sessions (id TEXT PRIMARY KEY, identity_id TEXT NOT NULL REFERENCES identity_subjects(id), token_sha256 TEXT NOT NULL UNIQUE, expires_at BIGINT NOT NULL, revoked BIGINT NOT NULL CHECK(revoked IN (0,1)), revision BIGINT NOT NULL CHECK(revision > 0))",
    "CREATE TABLE IF NOT EXISTS identity_organizations (id TEXT PRIMARY KEY, name TEXT NOT NULL, state TEXT NOT NULL CHECK(state IN ('active','suspended','closed')), revision BIGINT NOT NULL CHECK(revision > 0), authorization_epoch BIGINT NOT NULL CHECK(authorization_epoch > 0))",
    "CREATE TABLE IF NOT EXISTS identity_memberships (id TEXT PRIMARY KEY, organization_id TEXT NOT NULL REFERENCES identity_organizations(id), identity_id TEXT NOT NULL REFERENCES identity_subjects(id), role TEXT NOT NULL CHECK(role IN ('owner','admin','member','viewer')), active BIGINT NOT NULL CHECK(active IN (0,1)), revision BIGINT NOT NULL CHECK(revision > 0))",
    "CREATE UNIQUE INDEX IF NOT EXISTS identity_live_membership ON identity_memberships(organization_id,identity_id) WHERE active = 1",
    "CREATE TABLE IF NOT EXISTS identity_resources (id TEXT PRIMARY KEY, organization_id TEXT NOT NULL REFERENCES identity_organizations(id), kind TEXT NOT NULL CHECK(kind IN ('project','order','artifact','workflow','knowledge')), revision BIGINT NOT NULL CHECK(revision > 0))",
    "CREATE TABLE IF NOT EXISTS identity_content_grants (resource_id TEXT NOT NULL REFERENCES identity_resources(id), membership_id TEXT NOT NULL REFERENCES identity_memberships(id), permissions BIGINT NOT NULL CHECK(permissions > 0 AND permissions <= 15), active BIGINT NOT NULL CHECK(active IN (0,1)), revision BIGINT NOT NULL CHECK(revision > 0), PRIMARY KEY(resource_id,membership_id))",
    "CREATE TABLE IF NOT EXISTS identity_quotas (scope TEXT PRIMARY KEY, revision BIGINT NOT NULL CHECK(revision > 0), limit_payload TEXT NOT NULL, held_payload TEXT NOT NULL, consumed_payload TEXT NOT NULL)",
    "CREATE TABLE IF NOT EXISTS identity_reservations (id TEXT PRIMARY KEY, organization_id TEXT NOT NULL REFERENCES identity_organizations(id), identity_id TEXT NOT NULL REFERENCES identity_subjects(id), intent_key TEXT NOT NULL, request_sha256 TEXT NOT NULL, payload TEXT NOT NULL, initial_receipt TEXT NOT NULL, content_permit TEXT NOT NULL, UNIQUE(organization_id,identity_id,intent_key))",
    "CREATE TABLE IF NOT EXISTS identity_operations (actor_id TEXT NOT NULL REFERENCES identity_subjects(id), organization_id TEXT NOT NULL, intent_key TEXT NOT NULL, request_sha256 TEXT NOT NULL, receipt TEXT NOT NULL, PRIMARY KEY(actor_id,organization_id,intent_key))",
    "CREATE TABLE IF NOT EXISTS identity_audit (id TEXT PRIMARY KEY, sequence BIGINT NOT NULL UNIQUE CHECK(sequence > 0), organization_id TEXT, actor_id TEXT NOT NULL REFERENCES identity_subjects(id), action TEXT NOT NULL, target_id TEXT NOT NULL, observed_at BIGINT NOT NULL)",
    crate::store::IMPORT_STORAGE_SCHEMA,
    crate::store::IMPORT_STORAGE_RELEASE_SCHEMA,
    crate::store::SESSION_SCOPE_SCHEMA,
];

pub async fn apply(tx: &mut Transaction<'_, Any>) -> Result<()> {
    sqlx::query(STATEMENTS[0]).execute(&mut **tx).await?;
    let versions = sqlx::query("SELECT version FROM identity_schema_guard")
        .fetch_all(&mut **tx)
        .await?;
    if versions.is_empty() {
        sqlx::query("INSERT INTO identity_schema_guard(version) VALUES(1)")
            .execute(&mut **tx)
            .await?;
    } else if versions.len() != 1 || versions[0].try_get::<i64, _>("version")? != SCHEMA_VERSION {
        return Err(IdentityError::Conflict);
    }
    for statement in &STATEMENTS[1..] {
        sqlx::query(statement).execute(&mut **tx).await?;
    }
    for statement in include_str!("audit_schema.sql")
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        sqlx::query(statement).execute(&mut **tx).await?;
    }
    for statement in include_str!("audit_insights_schema.sql")
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        sqlx::query(statement).execute(&mut **tx).await?;
    }
    for statement in include_str!("audit_recovery_schema.sql")
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        sqlx::query(statement).execute(&mut **tx).await?;
    }
    for statement in include_str!("audit_checkpoint_schema.sql")
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        sqlx::query(statement).execute(&mut **tx).await?;
    }
    for statement in include_str!("audit_trust_schema.sql")
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        sqlx::query(statement).execute(&mut **tx).await?;
    }
    for statement in include_str!("audit_origin_schema.sql")
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        sqlx::query(statement).execute(&mut **tx).await?;
    }
    Ok(())
}
