use crate::{model::PermitData, *};
use ring::pbkdf2;
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use sqlx::{Acquire, Any, AnyPool, Row, Transaction, any::AnyPoolOptions};
use std::{num::NonZeroU32, path::Path, time::Duration};
use uuid::Uuid;
mod account_security;
#[path = "android_auth_adapter.rs"]
mod android_auth_adapter;
#[path = "android_auth_transport.rs"]
mod android_auth_transport;
#[path = "android_management_assurance.rs"]
mod android_management_assurance;
mod audit;
mod import_storage;
mod migration_identity;
mod session_scopes;
pub use android_auth_adapter::VerifiedPassword;
pub(crate) use import_storage::{IMPORT_STORAGE_SCHEMA, IMPORT_STORAGE_RELEASE_SCHEMA};
pub use import_storage::StorageImportCharge;
pub use migration_identity::{
    MigrationIdentityBinding, MigrationIdentityMapping, MigrationIdentityTarget,
    MigrationProjectGrant,
};
pub(crate) use session_scopes::SESSION_SCOPE_SCHEMA;

#[derive(Clone)]
pub struct IdentityStore {
    pool: AnyPool,
    backend: BackendKind,
    android_auth_security: bool,
}

/// Exclusive access to the caller's transaction after the installation writer
/// lock and database-clock sample. This guard cannot be cloned, deserialized or
/// reconstructed from a receipt. Acquire it before accounting/product row locks.
///
/// Successful mutations release a savepoint, never commit the outer transaction.
/// Returned reservation/execution metadata remains provisional until the caller
/// commits. It does not authorize external IO by itself. Revalidate content and
/// execution after later lock waits and keep the outer transaction through the
/// associated durable product effect.
pub struct LockedIdentityQuota<'store, 'tx, 'connection> {
    store: &'store IdentityStore,
    tx: &'tx mut Transaction<'connection, Any>,
    poisoned: bool,
}

async fn finish_quota_operation<T>(
    operation: Transaction<'_, Any>,
    result: Result<T>,
    poisoned: &mut bool,
) -> Result<T> {
    match result {
        Ok(value) => {
            operation.commit().await?;
            *poisoned = false;
            Ok(value)
        }
        Err(error) => {
            operation.rollback().await?;
            *poisoned = false;
            Err(error)
        }
    }
}

impl<'connection> LockedIdentityQuota<'_, '_, 'connection> {
    fn usable(&self) -> Result<()> {
        if self.poisoned {
            return Err(IdentityError::TransactionPoisoned);
        }
        Ok(())
    }

    /// Product/accounting SQL uses this same connection while identity authority
    /// stays locked. On any unconfirmed savepoint release/rollback or canceled
    /// mutation future, all guard methods fail closed. Drop the guard and roll
    /// back the outer transaction; never commit a poisoned caller transaction.
    pub fn transaction(&mut self) -> Result<&mut Transaction<'connection, Any>> {
        self.usable()?;
        Ok(self.tx)
    }

    pub async fn organization_in_transaction(
        &mut self,
        actor: &Principal,
        org: Uuid,
    ) -> Result<Organization> {
        self.usable()?;
        let (organization, _) = self.store.org_member(self.tx, actor, org).await?;
        Ok(organization)
    }

    pub async fn authorize_content_in_transaction(
        &mut self,
        actor: &Principal,
        org: Uuid,
        resource: Uuid,
        action: ContentAction,
    ) -> Result<ContentPermit> {
        self.usable()?;
        self.store
            .authorize_in(self.tx, actor, org, resource, action)
            .await
    }

    /// Fresh DB time/session/membership/epoch/ACL validation under the original
    /// writer lock. Call again after product/accounting locks have actually waited.
    pub async fn validate_content_in_transaction(&mut self, permit: &ContentPermit) -> Result<()> {
        self.usable()?;
        self.store.validate_content_locked(self.tx, permit).await
    }

    pub async fn reserve_quota_in_transaction(
        &mut self,
        actor: &Principal,
        permit: &ContentPermit,
        key: &str,
        amounts: QuotaAmounts,
        expires_at: i64,
    ) -> Result<QuotaReservation> {
        // A failed second scope must not leave a first-scope charge which a
        // caller catching the error could accidentally commit with product SQL.
        self.usable()?;
        self.poisoned = true;
        let mut operation = self.tx.begin().await?;
        let result = self
            .store
            .reserve_quota_locked(&mut operation, actor, permit, key, amounts, expires_at)
            .await;
        finish_quota_operation(operation, result, &mut self.poisoned).await
    }

    pub async fn activate_quota_in_transaction(
        &mut self,
        actor: &Principal,
        original: &QuotaReservation,
    ) -> Result<QuotaExecutionPermit> {
        self.usable()?;
        self.poisoned = true;
        let mut operation = self.tx.begin().await?;
        let result = self
            .store
            .activate_quota_locked(&mut operation, actor, original)
            .await;
        finish_quota_operation(operation, result, &mut self.poisoned).await
    }

    pub async fn validate_execution_in_transaction(
        &mut self,
        execution: &QuotaExecutionPermit,
    ) -> Result<()> {
        self.usable()?;
        self.store
            .validate_execution_locked(self.tx, execution)
            .await
    }

    /// The secret is an executor authority, not a tenant/content credential.
    /// Revoked or expired user sessions cannot prevent verified physical closure;
    /// unknown closure continues to hold all four quota dimensions indefinitely.
    #[allow(clippy::too_many_arguments)]
    pub async fn finalize_quota_in_transaction(
        &mut self,
        settlement_secret: &str,
        id: Uuid,
        expected_request_sha256: &str,
        consumed: QuotaAmounts,
        evidence_sha256: &str,
        aborted_before_dispatch: bool,
    ) -> Result<QuotaReservation> {
        self.usable()?;
        self.poisoned = true;
        let mut operation = self.tx.begin().await?;
        let result = self
            .store
            .finalize_quota_locked(
                &mut operation,
                settlement_secret,
                id,
                expected_request_sha256,
                consumed,
                evidence_sha256,
                aborted_before_dispatch,
            )
            .await;
        finish_quota_operation(operation, result, &mut self.poisoned).await
    }
}

fn encoded<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|_| IdentityError::Corrupt)
}
fn decoded<T: DeserializeOwned>(value: &str) -> Result<T> {
    serde_json::from_str(value).map_err(|_| IdentityError::Corrupt)
}
fn word<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .ok_or(IdentityError::Corrupt)
}
fn parse_word<T: DeserializeOwned>(value: &str) -> Result<T> {
    decoded(&encoded(&value)?)
}
fn identifier(value: String) -> Result<Uuid> {
    let id = Uuid::parse_str(&value).map_err(|_| IdentityError::Corrupt)?;
    if id.is_nil() || id.to_string() != value {
        return Err(IdentityError::Corrupt);
    }
    Ok(id)
}
fn hash(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}
fn secret_hash(domain: &str, value: &str) -> String {
    hash(format!("{domain}\0{value}").as_bytes())
}
fn digest_request<T: Serialize>(value: &T) -> Result<String> {
    Ok(hash(encoded(value)?.as_bytes()))
}
fn valid_name(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}
fn valid_key(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}
fn username(value: &str) -> Result<String> {
    if !(3..=64).contains(&value.len())
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        return Err(IdentityError::Validation);
    }
    Ok(value.to_ascii_lowercase())
}
fn password(value: &str) -> Result<()> {
    if !(12..=1024).contains(&value.len()) {
        return Err(IdentityError::Validation);
    }
    Ok(())
}
fn password_hash(salt: &str, value: &str) -> String {
    let mut output = [0u8; 32];
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        NonZeroU32::new(100_000).expect("nonzero"),
        salt.as_bytes(),
        value.as_bytes(),
        &mut output,
    );
    output.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn verify_password(salt: &str, value: &str, expected: &str) -> bool {
    if expected.len() != 64 {
        return false;
    }
    let bytes: Option<Vec<_>> = expected
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            std::str::from_utf8(pair)
                .ok()
                .and_then(|v| u8::from_str_radix(v, 16).ok())
        })
        .collect();
    bytes.is_some_and(|expected| {
        pbkdf2::verify(
            pbkdf2::PBKDF2_HMAC_SHA256,
            NonZeroU32::new(100_000).expect("nonzero"),
            salt.as_bytes(),
            value.as_bytes(),
            &expected,
        )
        .is_ok()
    })
}
fn identity(row: &sqlx::any::AnyRow) -> Result<Identity> {
    Ok(Identity {
        id: identifier(row.try_get("id")?)?,
        username: row.try_get("username")?,
        display_name: row.try_get("display_name")?,
        active: row.try_get::<i64, _>("active")? == 1,
        platform_admin: row.try_get::<i64, _>("platform_admin")? == 1,
        revision: row.try_get("revision")?,
    })
}
fn organization(row: &sqlx::any::AnyRow) -> Result<Organization> {
    Ok(Organization {
        id: identifier(row.try_get("id")?)?,
        name: row.try_get("name")?,
        state: parse_word(&row.try_get::<String, _>("state")?)?,
        revision: row.try_get("revision")?,
        authorization_epoch: row.try_get("authorization_epoch")?,
    })
}
fn membership(row: &sqlx::any::AnyRow) -> Result<Membership> {
    Ok(Membership {
        id: identifier(row.try_get("id")?)?,
        organization_id: identifier(row.try_get("organization_id")?)?,
        identity_id: identifier(row.try_get("identity_id")?)?,
        role: parse_word(&row.try_get::<String, _>("role")?)?,
        active: row.try_get::<i64, _>("active")? == 1,
        revision: row.try_get("revision")?,
    })
}
fn resource(row: &sqlx::any::AnyRow) -> Result<ContentResource> {
    Ok(ContentResource {
        id: identifier(row.try_get("id")?)?,
        organization_id: identifier(row.try_get("organization_id")?)?,
        kind: parse_word(&row.try_get::<String, _>("kind")?)?,
        revision: row.try_get("revision")?,
    })
}

impl IdentityStore {
    /// Use the same database as product storage. Schema registration remains an
    /// explicit application integration action; no personal rows are adopted.
    pub fn from_pool(pool: AnyPool, backend: BackendKind) -> Self {
        Self {
            pool,
            backend,
            android_auth_security: false,
        }
    }
    /// Product storage has explicitly registered Android security migration 59.
    /// A standalone identity database does not acquire a foreign product schema.
    pub fn with_android_auth_security(mut self) -> Self {
        self.android_auth_security = true;
        self
    }
    pub async fn connect(url: &str) -> Result<Self> {
        let backend = if url.starts_with("sqlite:") {
            BackendKind::Sqlite
        } else if url.starts_with("postgres:") || url.starts_with("postgresql:") {
            BackendKind::Postgres
        } else {
            return Err(IdentityError::Validation);
        };
        sqlx::any::install_default_drivers();
        let pool = AnyPoolOptions::new()
            .max_connections(8)
            .acquire_timeout(Duration::from_secs(10))
            .after_connect(move |connection, _| {
                Box::pin(async move {
                    if backend == BackendKind::Sqlite {
                        for pragma in [
                            "PRAGMA foreign_keys = ON",
                            "PRAGMA busy_timeout = 10000",
                            "PRAGMA journal_mode = WAL",
                            "PRAGMA synchronous = FULL",
                        ] {
                            sqlx::query(pragma).execute(&mut *connection).await?;
                        }
                    }
                    Ok(())
                })
            })
            .connect(url)
            .await?;
        let mut tx = pool.begin().await?;
        if backend == BackendKind::Postgres {
            sqlx::query("SELECT pg_advisory_xact_lock(72831026)")
                .execute(&mut *tx)
                .await?;
        }
        migrations::apply(&mut tx).await?;
        tx.commit().await?;
        Ok(Self {
            pool,
            backend,
            android_auth_security: false,
        })
    }
    pub async fn sqlite(path: &Path) -> Result<Self> {
        let path = path.to_str().ok_or(IdentityError::Validation)?;
        let encoded =
            percent_encoding::utf8_percent_encode(path, percent_encoding::NON_ALPHANUMERIC);
        Self::connect(&format!("sqlite:{encoded}?mode=rwc")).await
    }
    pub async fn close(&self) {
        self.pool.close().await;
    }

    /// Trusted installation setup only. Existing hashes are immutable and the
    /// bootstrap flag cannot be reset. Neither secret is a tenant bearer token.
    pub async fn initialize_credentials(
        &self,
        bootstrap_secret: &str,
        settlement_secret: &str,
    ) -> Result<()> {
        if bootstrap_secret.len() < 32
            || settlement_secret.len() < 32
            || bootstrap_secret == settlement_secret
        {
            return Err(IdentityError::Validation);
        }
        let bootstrap = secret_hash("bootstrap-v1", bootstrap_secret);
        let settlement = secret_hash("settlement-v1", settlement_secret);
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE identity_schema_guard SET version = version WHERE version = 1")
            .execute(&mut *tx)
            .await?;
        if let Some(row) = sqlx::query(
            "SELECT bootstrap_sha256,settlement_sha256 FROM identity_installation WHERE id = 1",
        )
        .fetch_optional(&mut *tx)
        .await?
        {
            if row.try_get::<String, _>("bootstrap_sha256")? != bootstrap
                || row.try_get::<String, _>("settlement_sha256")? != settlement
            {
                return Err(IdentityError::Forbidden);
            }
        } else {
            sqlx::query("INSERT INTO identity_installation(id,bootstrap_sha256,settlement_sha256,bootstrapped,utc_highwater,revision) VALUES(1,$1,$2,0,0,1)")
                .bind(bootstrap).bind(settlement).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
    /// All identity mutations and authority checks use one durable lock order.
    /// The database clock is sampled after the real SQL lock wait.
    async fn begin(&self) -> Result<Transaction<'_, Any>> {
        let mut tx = self.pool.begin().await?;
        self.lock_installation(&mut tx).await?;
        Ok(tx)
    }

    async fn lock_installation(&self, tx: &mut Transaction<'_, Any>) -> Result<()> {
        let result =
            sqlx::query("UPDATE identity_installation SET revision = revision WHERE id = 1")
                .execute(&mut **tx)
                .await?;
        if result.rows_affected() != 1 {
            return Err(IdentityError::Unauthenticated);
        }
        self.sample(tx).await?;
        Ok(())
    }

    /// Acquire the existing global identity writer lock once, on the caller's
    /// transaction, before monetary ledger/clock/run/product locks. The DB clock
    /// is sampled after this real lock wait. No connection or transaction opens.
    pub async fn lock_quota_in_transaction<'store, 'tx, 'connection>(
        &'store self,
        tx: &'tx mut Transaction<'connection, Any>,
    ) -> Result<LockedIdentityQuota<'store, 'tx, 'connection>> {
        self.lock_installation(tx).await?;
        Ok(LockedIdentityQuota {
            store: self,
            tx,
            poisoned: false,
        })
    }
    async fn sample(&self, tx: &mut Transaction<'_, Any>) -> Result<i64> {
        let query = match self.backend {
            BackendKind::Sqlite => {
                "SELECT CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER) AS observed"
            }
            BackendKind::Postgres => {
                "SELECT CAST(FLOOR(EXTRACT(EPOCH FROM clock_timestamp()) * 1000) AS BIGINT) AS observed"
            }
        };
        let observed: i64 = sqlx::query(query)
            .fetch_one(&mut **tx)
            .await?
            .try_get("observed")?;
        let floor: i64 =
            sqlx::query("SELECT utc_highwater FROM identity_installation WHERE id = 1")
                .fetch_one(&mut **tx)
                .await?
                .try_get("utc_highwater")?;
        if observed <= 0 || observed < floor {
            return Err(IdentityError::ClockRollback);
        }
        sqlx::query("UPDATE identity_installation SET utc_highwater = $1 WHERE id = 1")
            .bind(observed)
            .execute(&mut **tx)
            .await?;
        Ok(observed)
    }
    async fn actor(&self, tx: &mut Transaction<'_, Any>, actor: &Principal) -> Result<Identity> {
        let observed = self.sample(tx).await?;
        let row = sqlx::query(
            "SELECT identity_id,revision,expires_at,revoked FROM identity_sessions WHERE id = $1",
        )
        .bind(actor.session_id.to_string())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(IdentityError::Unauthenticated)?;
        if row.try_get::<String, _>("identity_id")? != actor.identity_id.to_string()
            || row.try_get::<i64, _>("revision")? != actor.session_revision
            || row.try_get::<i64, _>("revoked")? != 0
            || row.try_get::<i64, _>("expires_at")? <= observed
        {
            return Err(IdentityError::Unauthenticated);
        }
        let row = sqlx::query("SELECT * FROM identity_subjects WHERE id = $1")
            .bind(actor.identity_id.to_string())
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(IdentityError::Unauthenticated)?;
        let mut value = identity(&row)?;
        if !value.active {
            return Err(IdentityError::Unauthenticated);
        }
        if let Some(org) = self.session_scope(tx, actor).await? {
            let active:i64=sqlx::query_scalar("SELECT COUNT(*) FROM identity_memberships m JOIN identity_organizations o ON o.id=m.organization_id WHERE m.identity_id=$1 AND m.organization_id=$2 AND m.active=1 AND o.state='active'")
                .bind(actor.identity_id.to_string()).bind(org.to_string()).fetch_one(&mut **tx).await?;
            if active != 1 {
                return Err(IdentityError::Forbidden);
            }
            value.platform_admin = false;
        }
        Ok(value)
    }
    async fn platform_admin(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
    ) -> Result<Identity> {
        let value = self.actor(tx, actor).await?;
        if !value.platform_admin {
            return Err(IdentityError::Forbidden);
        }
        self.require_android_management_in_transaction(tx, actor)
            .await?;
        Ok(value)
    }
    async fn session(
        &self,
        tx: &mut Transaction<'_, Any>,
        identity: Identity,
        ttl_millis: i64,
    ) -> Result<SessionIssued> {
        if !(1_000..=86_400_000).contains(&ttl_millis) {
            return Err(IdentityError::Validation);
        }
        let observed = self.sample(tx).await?;
        let expires_at = observed
            .checked_add(ttl_millis)
            .ok_or(IdentityError::Validation)?;
        let token = format!(
            "cw2s_{}{}",
            Uuid::new_v4().simple(),
            Uuid::new_v4().simple()
        );
        let session_id = Uuid::new_v4();
        sqlx::query("INSERT INTO identity_sessions(id,identity_id,token_sha256,expires_at,revoked,revision) VALUES($1,$2,$3,$4,0,1)")
            .bind(session_id.to_string()).bind(identity.id.to_string())
            .bind(secret_hash("session-v1", &token)).bind(expires_at).execute(&mut **tx).await?;
        if self.android_auth_security {
            sqlx::query("INSERT INTO android_auth_session_metadata(session_id,owner_id,device_id,created_at,last_used_at) VALUES($1,$2,'',$3,$3)")
                .bind(session_id.to_string()).bind(identity.id.to_string()).bind(observed).execute(&mut **tx).await?;
        }
        Ok(SessionIssued {
            token,
            identity,
            expires_at,
        })
    }
    /// Validate an existing authenticated principal using the caller's writer
    /// transaction. Product authentication never constructs principals from IDs.
    pub async fn validate_principal_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
    ) -> Result<Identity> {
        self.lock_installation(tx).await?;
        self.actor(tx, actor).await
    }

    /// Trusted authentication-adapter hook after cryptographic verification.
    /// The caller must consume its server-side challenge and update the verified
    /// credential in this SAME transaction before requesting this session.
    /// It must also pin the original subject revision while holding this writer.
    /// No HTTP route accepts a user ID as proof for calling this hook.
    pub async fn issue_verified_session_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        identity_id: Uuid,
        ttl_millis: i64,
    ) -> Result<SessionIssued> {
        self.lock_installation(tx).await?;
        let row = sqlx::query("SELECT * FROM identity_subjects WHERE id = $1")
            .bind(identity_id.to_string())
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(IdentityError::Unauthenticated)?;
        let value = identity(&row)?;
        if !value.active {
            return Err(IdentityError::Unauthenticated);
        }
        let issued = self.session(tx, value, ttl_millis).await?;
        self.audit_event(
            tx,
            None,
            identity_id,
            "verified_session_issued",
            identity_id,
        )
        .await?;
        Ok(issued)
    }
    /// Server-internal authority for one already-admitted durable schedule
    /// occurrence. The caller must verify its persisted execution grant first
    /// in this same transaction. No bearer token is returned or recoverable.
    /// HTTP/session issuance endpoints never call this method.
    pub async fn issue_scheduler_session_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        identity_id: Uuid,
        org: Uuid,
        resource: Uuid,
        ttl_millis: i64,
    ) -> Result<Principal> {
        if !(1_000..=28_800_000).contains(&ttl_millis) {
            return Err(IdentityError::Validation);
        }
        let lock = sqlx::query("UPDATE identity_installation SET revision = revision WHERE id = 1")
            .execute(&mut **tx)
            .await?;
        if lock.rows_affected() != 1 {
            return Err(IdentityError::Unauthenticated);
        }
        let mut operation = tx.begin().await?;
        let result = async {
            let row = sqlx::query("SELECT * FROM identity_subjects WHERE id = $1")
                .bind(identity_id.to_string()).fetch_optional(&mut *operation).await?
                .ok_or(IdentityError::Unauthenticated)?;
            if !identity(&row)?.active {
                return Err(IdentityError::Unauthenticated);
            }
            let observed = self.sample(&mut operation).await?;
            let expires_at = observed.checked_add(ttl_millis).ok_or(IdentityError::Validation)?;
            let actor = Principal { identity_id, session_id: Uuid::new_v4(), session_revision: 1 };
            // authenticate_session uses session-v1, which cannot authenticate
            // this domain-separated non-bearer record.
            let nonce = Uuid::new_v4().to_string();
            sqlx::query("INSERT INTO identity_sessions(id,identity_id,token_sha256,expires_at,revoked,revision) VALUES($1,$2,$3,$4,0,1)")
                .bind(actor.session_id.to_string()).bind(identity_id.to_string())
                .bind(secret_hash("scheduler-session-v1", &nonce)).bind(expires_at)
                .execute(&mut *operation).await?;
            self.authorize_in(&mut operation, &actor, org, resource, ContentAction::Read).await?;
            self.authorize_in(&mut operation, &actor, org, resource, ContentAction::Execute).await?;
            self.audit_event(&mut operation, Some(org), identity_id, "scheduler_session_issued", actor.session_id).await?;
            Ok(actor)
        }.await;
        match result {
            Ok(actor) => {
                operation.commit().await?;
                Ok(actor)
            }
            Err(error) => {
                operation.rollback().await?;
                Err(error)
            }
        }
    }
    async fn insert_identity(
        &self,
        tx: &mut Transaction<'_, Any>,
        name: &str,
        display: &str,
        pass: &str,
        admin: bool,
    ) -> Result<Identity> {
        let name = username(name)?;
        password(pass)?;
        if !valid_name(display) {
            return Err(IdentityError::Validation);
        }
        let value = Identity {
            id: Uuid::new_v4(),
            username: name,
            display_name: display.into(),
            active: true,
            platform_admin: admin,
            revision: 1,
        };
        let salt = Uuid::new_v4().simple().to_string();
        sqlx::query("INSERT INTO identity_subjects(id,username,display_name,active,platform_admin,revision,password_salt,password_hash) VALUES($1,$2,$3,1,$4,1,$5,$6)")
            .bind(value.id.to_string()).bind(&value.username).bind(display).bind(i64::from(admin))
            .bind(&salt).bind(password_hash(&salt, pass)).execute(&mut **tx).await?;
        Ok(value)
    }
    pub async fn bootstrap(
        &self,
        secret: &str,
        name: &str,
        display: &str,
        pass: &str,
        organization_name: &str,
    ) -> Result<(Organization, SessionIssued)> {
        if !valid_name(organization_name) {
            return Err(IdentityError::Validation);
        }
        let mut tx = self.begin().await?;
        let row = sqlx::query(
            "SELECT bootstrap_sha256,bootstrapped FROM identity_installation WHERE id = 1",
        )
        .fetch_one(&mut *tx)
        .await?;
        if row.try_get::<String, _>("bootstrap_sha256")? != secret_hash("bootstrap-v1", secret) {
            return Err(IdentityError::Forbidden);
        }
        if row.try_get::<i64, _>("bootstrapped")? != 0 {
            return Err(IdentityError::Conflict);
        }
        let identity = self
            .insert_identity(&mut tx, name, display, pass, true)
            .await?;
        let organization = self
            .insert_organization(&mut tx, identity.id, organization_name)
            .await?;
        sqlx::query("UPDATE identity_installation SET bootstrapped = 1,revision = revision + 1 WHERE id = 1")
            .execute(&mut *tx).await?;
        let session = self.session(&mut tx, identity, 3_600_000).await?;
        tx.commit().await?;
        Ok((organization, session))
    }
    /// Registration creates no platform privilege, membership or content grant.
    pub async fn register_local_identity(
        &self,
        name: &str,
        display: &str,
        pass: &str,
    ) -> Result<Identity> {
        let mut tx = self.begin().await?;
        let ready: i64 = sqlx::query("SELECT bootstrapped FROM identity_installation WHERE id = 1")
            .fetch_one(&mut *tx)
            .await?
            .try_get("bootstrapped")?;
        if ready != 1 {
            return Err(IdentityError::Forbidden);
        }
        let value = self
            .insert_identity(&mut tx, name, display, pass, false)
            .await?;
        self.audit_event(&mut tx, None, value.id, "identity_registered", value.id)
            .await?;
        tx.commit().await?;
        Ok(value)
    }
    pub async fn login(&self, name: &str, pass: &str, ttl_millis: i64) -> Result<SessionIssued> {
        let name = username(name).map_err(|_| IdentityError::Unauthenticated)?;
        if pass.len() > 1024 {
            return Err(IdentityError::Unauthenticated);
        }
        let mut tx = self.begin().await?;
        let row = sqlx::query("SELECT * FROM identity_subjects WHERE username = $1")
            .bind(name)
            .fetch_optional(&mut *tx)
            .await?;
        let Some(row) = row else {
            let _ = verify_password("00000000000000000000000000000000", pass, &"00".repeat(32));
            return Err(IdentityError::Unauthenticated);
        };
        let value = identity(&row)?;
        if !verify_password(
            &row.try_get::<String, _>("password_salt")?,
            pass,
            &row.try_get::<String, _>("password_hash")?,
        ) || !value.active
        {
            return Err(IdentityError::Unauthenticated);
        }
        if self.android_auth_security {
            let enabled: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM android_auth_totp WHERE owner_id=$1 AND enabled=1",
            )
            .bind(value.id.to_string())
            .fetch_one(&mut *tx)
            .await?;
            if enabled != 0 {
                return Err(IdentityError::Unauthenticated);
            }
        }
        let session = self.session(&mut tx, value, ttl_millis).await?;
        tx.commit().await?;
        Ok(session)
    }
    pub async fn authenticate_session(&self, token: &str) -> Result<Principal> {
        if token.len() != 69
            || !token.starts_with("cw2s_")
            || !token[5..].bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(IdentityError::Unauthenticated);
        }
        let mut tx = self.begin().await?;
        let row = sqlx::query(
            "SELECT id,identity_id,revision FROM identity_sessions WHERE token_sha256 = $1",
        )
        .bind(secret_hash("session-v1", token))
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(IdentityError::Unauthenticated)?;
        let actor = Principal {
            identity_id: identifier(row.try_get("identity_id")?)?,
            session_id: identifier(row.try_get("id")?)?,
            session_revision: row.try_get("revision")?,
        };
        self.actor(&mut tx, &actor).await?;
        if self.android_auth_security {
            let current = self.sample(&mut tx).await?;
            sqlx::query("UPDATE android_auth_session_metadata SET last_used_at=$1 WHERE session_id=$2 AND owner_id=$3 AND revoked_at=0")
                .bind(current).bind(actor.session_id.to_string()).bind(actor.identity_id.to_string()).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(actor)
    }
    pub async fn logout(&self, actor: &Principal) -> Result<()> {
        let mut tx = self.begin().await?;
        self.actor(&mut tx, actor).await?;
        sqlx::query(
            "UPDATE identity_sessions SET revoked = 1,revision = revision + 1 WHERE id = $1",
        )
        .bind(actor.session_id.to_string())
        .execute(&mut *tx)
        .await?;
        if self.android_auth_security {
            let current = self.sample(&mut tx).await?;
            sqlx::query("UPDATE android_auth_session_metadata SET revoked_at=$1,revoke_reason='user_logout' WHERE session_id=$2 AND owner_id=$3 AND revoked_at=0")
                .bind(current).bind(actor.session_id.to_string()).bind(actor.identity_id.to_string()).execute(&mut *tx).await?;
        }
        self.audit_event(
            &mut tx,
            None,
            actor.identity_id,
            "session_revoked",
            actor.session_id,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
    /// Atomic session rotation for the native compatibility client. A revoked,
    /// expired or stale session can never issue a replacement.
    pub async fn rotate_session(
        &self,
        actor: &Principal,
        ttl_millis: i64,
    ) -> Result<SessionIssued> {
        let mut tx = self.begin().await?;
        let issued = self
            .rotate_android_session_in_transaction(&mut tx, actor, ttl_millis)
            .await?;
        tx.commit().await?;
        Ok(issued)
    }
    pub async fn self_identity(&self, actor: &Principal) -> Result<Identity> {
        let mut tx = self.begin().await?;
        let value = self.actor(&mut tx, actor).await?;
        tx.commit().await?;
        Ok(value)
    }
    /// Session revocation remains available even for the last tenant owner or
    /// platform administrator. It never deletes membership or content grants.
    pub async fn revoke_identity_sessions(
        &self,
        actor: &Principal,
        target: Uuid,
        expected_revision: i64,
    ) -> Result<()> {
        let mut tx = self.begin().await?;
        self.platform_admin(&mut tx, actor).await?;
        let row = sqlx::query("SELECT revision FROM identity_subjects WHERE id = $1")
            .bind(target.to_string())
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(IdentityError::NotFound)?;
        if row.try_get::<i64, _>("revision")? != expected_revision {
            return Err(IdentityError::Conflict);
        }
        sqlx::query("UPDATE identity_sessions SET revoked = 1,revision = revision + 1 WHERE identity_id = $1 AND revoked = 0")
            .bind(target.to_string()).execute(&mut *tx).await?;
        self.revoke_android_metadata(&mut tx, target, "administrator_revoked")
            .await?;
        self.audit_event(
            &mut tx,
            None,
            actor.identity_id,
            "identity_sessions_revoked",
            target,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
    /// A credential change requires the current password in addition to the
    /// persisted session and invalidates every previous session atomically.
    pub async fn change_password(
        &self,
        actor: &Principal,
        old_password: &str,
        new_password: &str,
        ttl_millis: i64,
    ) -> Result<SessionIssued> {
        password(new_password)?;
        if old_password.len() > 1024 {
            return Err(IdentityError::Unauthenticated);
        }
        let mut tx = self.begin().await?;
        self.require_global_session_in_transaction(&mut tx, actor)
            .await?;
        let mut value = self.actor(&mut tx, actor).await?;
        self.require_android_management_in_transaction(&mut tx, actor)
            .await?;
        self.require_android_factor_verified(&mut tx, actor).await?;
        let row =
            sqlx::query("SELECT password_salt,password_hash FROM identity_subjects WHERE id = $1")
                .bind(value.id.to_string())
                .fetch_one(&mut *tx)
                .await?;
        if !verify_password(
            &row.try_get::<String, _>("password_salt")?,
            old_password,
            &row.try_get::<String, _>("password_hash")?,
        ) {
            return Err(IdentityError::Unauthenticated);
        }
        let salt = Uuid::new_v4().simple().to_string();
        value.revision += 1;
        sqlx::query("UPDATE identity_subjects SET password_salt = $1,password_hash = $2,revision = $3 WHERE id = $4")
            .bind(&salt).bind(password_hash(&salt, new_password)).bind(value.revision).bind(value.id.to_string())
            .execute(&mut *tx).await?;
        sqlx::query("UPDATE identity_sessions SET revoked = 1,revision = revision + 1 WHERE identity_id = $1 AND revoked = 0")
            .bind(value.id.to_string()).execute(&mut *tx).await?;
        self.audit_event(
            &mut tx,
            None,
            actor.identity_id,
            "identity_password_changed",
            value.id,
        )
        .await?;
        let session = self.session(&mut tx, value, ttl_millis).await?;
        if self.android_auth_security {
            self.inherit_android_assurance(&mut tx, actor, &session)
                .await?;
        }
        self.revoke_android_metadata(&mut tx, actor.identity_id, "password_changed")
            .await?;
        tx.commit().await?;
        Ok(session)
    }
    pub async fn disable_identity(
        &self,
        actor: &Principal,
        target: Uuid,
        expected_revision: i64,
    ) -> Result<Identity> {
        let mut tx = self.begin().await?;
        self.platform_admin(&mut tx, actor).await?;
        let row = sqlx::query("SELECT * FROM identity_subjects WHERE id = $1")
            .bind(target.to_string())
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(IdentityError::NotFound)?;
        let mut value = identity(&row)?;
        if value.revision != expected_revision {
            return Err(IdentityError::Conflict);
        }
        if value.platform_admin && value.active {
            let count: i64 = sqlx::query("SELECT COUNT(*) AS count FROM identity_subjects WHERE active = 1 AND platform_admin = 1")
                .fetch_one(&mut *tx).await?.try_get("count")?;
            if count <= 1 {
                return Err(IdentityError::Forbidden);
            }
        }
        if value.active {
            let owner_orgs = sqlx::query("SELECT organization_id FROM identity_memberships WHERE identity_id = $1 AND role = 'owner' AND active = 1")
                .bind(target.to_string()).fetch_all(&mut *tx).await?;
            for row in owner_orgs {
                let org: String = row.try_get("organization_id")?;
                let other_owners: i64 = sqlx::query("SELECT COUNT(*) AS count FROM identity_memberships m JOIN identity_subjects s ON s.id = m.identity_id WHERE m.organization_id = $1 AND m.role = 'owner' AND m.active = 1 AND s.active = 1 AND s.id <> $2")
                    .bind(org).bind(target.to_string()).fetch_one(&mut *tx).await?.try_get("count")?;
                if other_owners == 0 {
                    return Err(IdentityError::Forbidden);
                }
            }
        }
        value.active = false;
        value.revision += 1;
        sqlx::query("UPDATE identity_subjects SET active = 0,revision = $1 WHERE id = $2")
            .bind(value.revision)
            .bind(target.to_string())
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE identity_sessions SET revoked = 1,revision = revision + 1 WHERE identity_id = $1 AND revoked = 0")
            .bind(target.to_string()).execute(&mut *tx).await?;
        self.revoke_android_metadata(&mut tx, target, "identity_disabled")
            .await?;
        self.audit_event(
            &mut tx,
            None,
            actor.identity_id,
            "identity_disabled",
            target,
        )
        .await?;
        tx.commit().await?;
        Ok(value)
    }
    async fn insert_organization(
        &self,
        tx: &mut Transaction<'_, Any>,
        owner: Uuid,
        name: &str,
    ) -> Result<Organization> {
        let value = Organization {
            id: Uuid::new_v4(),
            name: name.into(),
            state: OrganizationState::Active,
            revision: 1,
            authorization_epoch: 1,
        };
        sqlx::query("INSERT INTO identity_organizations(id,name,state,revision,authorization_epoch) VALUES($1,$2,'active',1,1)")
            .bind(value.id.to_string()).bind(name).execute(&mut **tx).await?;
        self.insert_membership(tx, value.id, owner, OrganizationRole::Owner)
            .await?;
        self.audit_event(tx, Some(value.id), owner, "organization_created", value.id)
            .await?;
        Ok(value)
    }
    pub async fn create_organization(
        &self,
        actor: &Principal,
        name: &str,
        key: &str,
    ) -> Result<Organization> {
        if !valid_name(name) || !valid_key(key) {
            return Err(IdentityError::Validation);
        }
        let mut tx = self.begin().await?;
        self.platform_admin(&mut tx, actor).await?;
        let request = digest_request(&("organization_create", name))?;
        if let Some(value) = self
            .operation(&mut tx, actor.identity_id, Uuid::nil(), key, &request)
            .await?
        {
            tx.commit().await?;
            return Ok(value);
        }
        let value = self
            .insert_organization(&mut tx, actor.identity_id, name)
            .await?;
        self.save_operation(
            &mut tx,
            actor.identity_id,
            Uuid::nil(),
            key,
            &request,
            &value,
        )
        .await?;
        tx.commit().await?;
        Ok(value)
    }
    async fn org_member(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
        org: Uuid,
    ) -> Result<(Organization, Membership)> {
        self.actor(tx, actor).await?;
        self.require_android_management_in_transaction(tx, actor)
            .await?;
        if self
            .session_scope(tx, actor)
            .await?
            .is_some_and(|scope| scope != org)
        {
            return Err(IdentityError::Forbidden);
        }
        let row = sqlx::query("SELECT * FROM identity_memberships WHERE organization_id = $1 AND identity_id = $2 AND active = 1")
            .bind(org.to_string()).bind(actor.identity_id.to_string()).fetch_optional(&mut **tx).await?.ok_or(IdentityError::Forbidden)?;
        let mut member = membership(&row)?;
        if self.session_scope_permissions(tx, actor).await? == 1 {
            member.role = OrganizationRole::Viewer;
        }
        let row = sqlx::query("SELECT * FROM identity_organizations WHERE id = $1")
            .bind(org.to_string())
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(IdentityError::Forbidden)?;
        let organization = organization(&row)?;
        if organization.state != OrganizationState::Active {
            return Err(IdentityError::Forbidden);
        }
        Ok((organization, member))
    }
    pub async fn organizations(&self, actor: &Principal) -> Result<Vec<Organization>> {
        let mut tx = self.begin().await?;
        self.actor(&mut tx, actor).await?;
        let rows = sqlx::query("SELECT o.* FROM identity_organizations o JOIN identity_memberships m ON m.organization_id = o.id WHERE m.identity_id = $1 AND m.active = 1 ORDER BY o.id")
            .bind(actor.identity_id.to_string()).fetch_all(&mut *tx).await?;
        let scope = self.session_scope(&mut tx, actor).await?;
        let values = rows
            .iter()
            .map(organization)
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .filter(|o| scope.is_none_or(|org| o.id == org))
            .collect();
        tx.commit().await?;
        Ok(values)
    }
    pub async fn organization(&self, actor: &Principal, org: Uuid) -> Result<Organization> {
        let mut tx = self.begin().await?;
        let (value, _) = self.org_member(&mut tx, actor, org).await?;
        tx.commit().await?;
        Ok(value)
    }
    /// Authenticate a tenant scope in the caller's product transaction, even
    /// when it has no content rows. Acquire this authority lock before product
    /// row locks and recheck after a later lock wait before returning data.
    pub async fn organization_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
        org: Uuid,
    ) -> Result<Organization> {
        let lock = sqlx::query("UPDATE identity_installation SET revision = revision WHERE id = 1")
            .execute(&mut **tx)
            .await?;
        if lock.rows_affected() != 1 {
            return Err(IdentityError::Unauthenticated);
        }
        let (organization, _) = self.org_member(tx, actor, org).await?;
        Ok(organization)
    }
    async fn bump_epoch(&self, tx: &mut Transaction<'_, Any>, org: Uuid) -> Result<()> {
        sqlx::query("UPDATE identity_organizations SET authorization_epoch = authorization_epoch + 1,revision = revision + 1 WHERE id = $1")
            .bind(org.to_string()).execute(&mut **tx).await?;
        Ok(())
    }
    async fn insert_membership(
        &self,
        tx: &mut Transaction<'_, Any>,
        org: Uuid,
        target: Uuid,
        role: OrganizationRole,
    ) -> Result<Membership> {
        let row = sqlx::query("SELECT active FROM identity_subjects WHERE id = $1")
            .bind(target.to_string())
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(IdentityError::NotFound)?;
        if row.try_get::<i64, _>("active")? != 1 {
            return Err(IdentityError::Forbidden);
        }
        let value = Membership {
            id: Uuid::new_v4(),
            organization_id: org,
            identity_id: target,
            role,
            active: true,
            revision: 1,
        };
        sqlx::query("INSERT INTO identity_memberships(id,organization_id,identity_id,role,active,revision) VALUES($1,$2,$3,$4,1,1)")
            .bind(value.id.to_string()).bind(org.to_string()).bind(target.to_string()).bind(word(&role)?)
            .execute(&mut **tx).await?;
        Ok(value)
    }
    fn may_manage(actor: OrganizationRole, target: OrganizationRole) -> bool {
        actor == OrganizationRole::Owner
            || (actor == OrganizationRole::Admin
                && matches!(target, OrganizationRole::Member | OrganizationRole::Viewer))
    }
    pub async fn add_member(
        &self,
        actor: &Principal,
        org: Uuid,
        target: Uuid,
        role: OrganizationRole,
    ) -> Result<Membership> {
        let mut tx = self.begin().await?;
        let (_, current) = self.org_member(&mut tx, actor, org).await?;
        if !Self::may_manage(current.role, role) {
            return Err(IdentityError::Forbidden);
        }
        let value = self.insert_membership(&mut tx, org, target, role).await?;
        self.bump_epoch(&mut tx, org).await?;
        self.audit_event(
            &mut tx,
            Some(org),
            actor.identity_id,
            "member_added",
            value.id,
        )
        .await?;
        tx.commit().await?;
        Ok(value)
    }
    pub async fn members(&self, actor: &Principal, org: Uuid) -> Result<Vec<Membership>> {
        let mut tx = self.begin().await?;
        let (_, member) = self.org_member(&mut tx, actor, org).await?;
        if !member.role.manages_members() {
            return Err(IdentityError::Forbidden);
        }
        let rows = sqlx::query(
            "SELECT * FROM identity_memberships WHERE organization_id = $1 ORDER BY id",
        )
        .bind(org.to_string())
        .fetch_all(&mut *tx)
        .await?;
        let values = rows.iter().map(membership).collect::<Result<Vec<_>>>()?;
        tx.commit().await?;
        Ok(values)
    }
    pub async fn change_member(
        &self,
        actor: &Principal,
        org: Uuid,
        member_id: Uuid,
        expected_revision: i64,
        role: OrganizationRole,
        active: bool,
    ) -> Result<Membership> {
        let mut tx = self.begin().await?;
        let (_, current) = self.org_member(&mut tx, actor, org).await?;
        let row = sqlx::query(
            "SELECT * FROM identity_memberships WHERE id = $1 AND organization_id = $2",
        )
        .bind(member_id.to_string())
        .bind(org.to_string())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(IdentityError::NotFound)?;
        let mut target = membership(&row)?;
        if !target.active || target.revision != expected_revision {
            return Err(IdentityError::Conflict);
        }
        if !Self::may_manage(current.role, target.role) || !Self::may_manage(current.role, role) {
            return Err(IdentityError::Forbidden);
        }
        if target.role == OrganizationRole::Owner && (!active || role != OrganizationRole::Owner) {
            let count: i64 = sqlx::query("SELECT COUNT(*) AS count FROM identity_memberships m JOIN identity_subjects s ON s.id = m.identity_id WHERE m.organization_id = $1 AND m.role = 'owner' AND m.active = 1 AND s.active = 1")
                .bind(org.to_string()).fetch_one(&mut *tx).await?.try_get("count")?;
            if count <= 1 {
                return Err(IdentityError::Forbidden);
            }
        }
        target.role = role;
        target.active = active;
        target.revision += 1;
        sqlx::query(
            "UPDATE identity_memberships SET role = $1,active = $2,revision = $3 WHERE id = $4",
        )
        .bind(word(&role)?)
        .bind(i64::from(active))
        .bind(target.revision)
        .bind(member_id.to_string())
        .execute(&mut *tx)
        .await?;
        self.bump_epoch(&mut tx, org).await?;
        self.audit_event(
            &mut tx,
            Some(org),
            actor.identity_id,
            if active {
                "member_role_changed"
            } else {
                "member_revoked"
            },
            member_id,
        )
        .await?;
        tx.commit().await?;
        Ok(target)
    }
    /// Platform management may suspend/close a tenant but never gains its content
    /// ACL. Closing is terminal; suspension restoration increments the epoch.
    pub async fn set_organization_state(
        &self,
        actor: &Principal,
        org: Uuid,
        expected_revision: i64,
        state: OrganizationState,
    ) -> Result<Organization> {
        let mut tx = self.begin().await?;
        self.platform_admin(&mut tx, actor).await?;
        let row = sqlx::query("SELECT * FROM identity_organizations WHERE id = $1")
            .bind(org.to_string())
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(IdentityError::NotFound)?;
        let mut value = organization(&row)?;
        if value.revision != expected_revision || value.state == OrganizationState::Closed {
            return Err(IdentityError::Conflict);
        }
        value.state = state;
        value.revision += 1;
        value.authorization_epoch += 1;
        sqlx::query("UPDATE identity_organizations SET state = $1,revision = $2,authorization_epoch = $3 WHERE id = $4")
            .bind(word(&state)?).bind(value.revision).bind(value.authorization_epoch).bind(org.to_string()).execute(&mut *tx).await?;
        self.audit_event(
            &mut tx,
            Some(org),
            actor.identity_id,
            "organization_state_changed",
            org,
        )
        .await?;
        tx.commit().await?;
        Ok(value)
    }
    /// Mints a fresh tenant object ID. There is intentionally no adopt/bind API
    /// for caller-selected personal/foreign object IDs.
    pub async fn create_resource(
        &self,
        actor: &Principal,
        org: Uuid,
        kind: ContentKind,
        key: &str,
    ) -> Result<ContentResource> {
        let mut tx = self.begin().await?;
        let value = self
            .create_resource_in_transaction(&mut tx, actor, org, kind, key)
            .await?;
        tx.commit().await?;
        Ok(value)
    }
    /// Create the ownership row and the product object in one SQL transaction.
    /// The caller must use the freshly minted ID for its tenant product row.
    pub async fn create_resource_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
        org: Uuid,
        kind: ContentKind,
        key: &str,
    ) -> Result<ContentResource> {
        if !valid_key(key) {
            return Err(IdentityError::Validation);
        }
        let lock = sqlx::query("UPDATE identity_installation SET revision = revision WHERE id = 1")
            .execute(&mut **tx)
            .await?;
        if lock.rows_affected() != 1 {
            return Err(IdentityError::Unauthenticated);
        }
        let (_, member) = self.org_member(tx, actor, org).await?;
        if member.role == OrganizationRole::Viewer {
            return Err(IdentityError::Forbidden);
        }
        let request = digest_request(&("resource_create", kind))?;
        if let Some(value) = self
            .operation::<ContentResource>(tx, actor.identity_id, org, key, &request)
            .await?
        {
            self.authorize_in(tx, actor, org, value.id, ContentAction::Write)
                .await?;
            return Ok(value);
        }
        let value = ContentResource {
            id: Uuid::new_v4(),
            organization_id: org,
            kind,
            revision: 1,
        };
        sqlx::query(
            "INSERT INTO identity_resources(id,organization_id,kind,revision) VALUES($1,$2,$3,1)",
        )
        .bind(value.id.to_string())
        .bind(org.to_string())
        .bind(word(&kind)?)
        .execute(&mut **tx)
        .await?;
        sqlx::query("INSERT INTO identity_content_grants(resource_id,membership_id,permissions,active,revision) VALUES($1,$2,15,1,1)")
            .bind(value.id.to_string()).bind(member.id.to_string()).execute(&mut **tx).await?;
        self.bump_epoch(tx, org).await?;
        self.save_operation(tx, actor.identity_id, org, key, &request, &value)
            .await?;
        self.audit_event(
            tx,
            Some(org),
            actor.identity_id,
            "content_resource_created",
            value.id,
        )
        .await?;
        Ok(value)
    }
    pub async fn resources(&self, actor: &Principal, org: Uuid) -> Result<Vec<ContentResource>> {
        let mut tx = self.begin().await?;
        let (_, member) = self.org_member(&mut tx, actor, org).await?;
        let rows = sqlx::query("SELECT r.* FROM identity_resources r JOIN identity_content_grants g ON g.resource_id = r.id WHERE r.organization_id = $1 AND g.membership_id = $2 AND g.active = 1 AND (g.permissions & 1) = 1 ORDER BY r.id")
            .bind(org.to_string()).bind(member.id.to_string()).fetch_all(&mut *tx).await?;
        let values = rows.iter().map(resource).collect::<Result<Vec<_>>>()?;
        tx.commit().await?;
        Ok(values)
    }
    async fn authorize_in(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
        org: Uuid,
        content: Uuid,
        action: ContentAction,
    ) -> Result<ContentPermit> {
        let (organization, member) = self.org_member(tx, actor, org).await?;
        if !member.role.allows(action) {
            return Err(IdentityError::Forbidden);
        }
        let row =
            sqlx::query("SELECT * FROM identity_resources WHERE id = $1 AND organization_id = $2")
                .bind(content.to_string())
                .bind(org.to_string())
                .fetch_optional(&mut **tx)
                .await?
                .ok_or(IdentityError::NotFound)?;
        let resource = resource(&row)?;
        let row = sqlx::query("SELECT permissions,revision FROM identity_content_grants WHERE resource_id = $1 AND membership_id = $2 AND active = 1")
            .bind(content.to_string()).bind(member.id.to_string()).fetch_optional(&mut **tx).await?.ok_or(IdentityError::Forbidden)?;
        let permissions: Permissions = decoded(&row.try_get::<i64, _>("permissions")?.to_string())?;
        if !permissions.valid() || !permissions.permits(action) {
            return Err(IdentityError::Forbidden);
        }
        Ok(ContentPermit(PermitData {
            identity_id: actor.identity_id,
            session_id: actor.session_id,
            session_revision: actor.session_revision,
            organization_id: org,
            authorization_epoch: organization.authorization_epoch,
            membership_id: member.id,
            membership_revision: member.revision,
            resource_id: resource.id,
            resource_revision: resource.revision,
            grant_revision: row.try_get("revision")?,
            action,
        }))
    }
    pub async fn authorize_content(
        &self,
        actor: &Principal,
        org: Uuid,
        resource: Uuid,
        action: ContentAction,
    ) -> Result<ContentPermit> {
        let mut tx = self.begin().await?;
        let value = self
            .authorize_in(&mut tx, actor, org, resource, action)
            .await?;
        tx.commit().await?;
        Ok(value)
    }
    /// Acquire tenant authority before product row locks, then keep this SQL
    /// transaction through the data effect. Revalidate the returned permit
    /// after any later lock wait so session expiry uses the current DB clock.
    pub async fn authorize_content_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
        org: Uuid,
        resource: Uuid,
        action: ContentAction,
    ) -> Result<ContentPermit> {
        let lock = sqlx::query("UPDATE identity_installation SET revision = revision WHERE id = 1")
            .execute(&mut **tx)
            .await?;
        if lock.rows_affected() != 1 {
            return Err(IdentityError::Unauthenticated);
        }
        self.authorize_in(tx, actor, org, resource, action).await
    }
    /// Integration first acquires authority with authorize_content_in_transaction,
    /// then takes product row locks and revalidates here after the real wait.
    /// Keep that transaction until its scoped effect commits. Standalone permits
    /// do not bypass the product's tool policy or executor admission.
    pub async fn validate_content_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        permit: &ContentPermit,
    ) -> Result<()> {
        let lock = sqlx::query("UPDATE identity_installation SET revision = revision WHERE id = 1")
            .execute(&mut **tx)
            .await?;
        if lock.rows_affected() != 1 {
            return Err(IdentityError::Unauthenticated);
        }
        self.validate_content_locked(tx, permit).await
    }

    async fn validate_content_locked(
        &self,
        tx: &mut Transaction<'_, Any>,
        permit: &ContentPermit,
    ) -> Result<()> {
        let actor = Principal {
            identity_id: permit.0.identity_id,
            session_id: permit.0.session_id,
            session_revision: permit.0.session_revision,
        };
        let fresh = self
            .authorize_in(
                tx,
                &actor,
                permit.0.organization_id,
                permit.0.resource_id,
                permit.0.action,
            )
            .await?;
        if fresh.0 != permit.0 {
            return Err(IdentityError::Conflict);
        }
        Ok(())
    }
    pub async fn validate_content(&self, permit: &ContentPermit) -> Result<()> {
        let mut tx = self.begin().await?;
        self.validate_content_in_transaction(&mut tx, permit)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    #[allow(
        clippy::too_many_arguments,
        reason = "tenant, object and membership scopes stay explicit alongside the CAS grant update"
    )]
    pub async fn grant_content(
        &self,
        actor: &Principal,
        org: Uuid,
        resource: Uuid,
        target_member: Uuid,
        expected_revision: i64,
        permissions: Permissions,
        active: bool,
    ) -> Result<ContentGrant> {
        if !permissions.valid() {
            return Err(IdentityError::Validation);
        }
        let mut tx = self.begin().await?;
        self.authorize_in(&mut tx, actor, org, resource, ContentAction::Share)
            .await?;
        let row = sqlx::query("SELECT * FROM identity_memberships WHERE id = $1 AND organization_id = $2 AND active = 1")
            .bind(target_member.to_string()).bind(org.to_string()).fetch_optional(&mut *tx).await?.ok_or(IdentityError::NotFound)?;
        let target = membership(&row)?;
        if target.role == OrganizationRole::Viewer && permissions != Permissions::read_only() {
            return Err(IdentityError::Forbidden);
        }
        let prior = sqlx::query("SELECT revision FROM identity_content_grants WHERE resource_id = $1 AND membership_id = $2")
            .bind(resource.to_string()).bind(target_member.to_string()).fetch_optional(&mut *tx).await?;
        let revision = prior
            .as_ref()
            .map(|r| r.try_get::<i64, _>("revision"))
            .transpose()?
            .unwrap_or(0);
        if revision != expected_revision {
            return Err(IdentityError::Conflict);
        }
        let value = ContentGrant {
            resource_id: resource,
            membership_id: target_member,
            permissions,
            active,
            revision: revision + 1,
        };
        if revision == 0 {
            sqlx::query("INSERT INTO identity_content_grants(resource_id,membership_id,permissions,active,revision) VALUES($1,$2,$3,$4,1)")
                .bind(resource.to_string()).bind(target_member.to_string()).bind(permissions.bits()).bind(i64::from(active)).execute(&mut *tx).await?;
        } else {
            sqlx::query("UPDATE identity_content_grants SET permissions = $1,active = $2,revision = $3 WHERE resource_id = $4 AND membership_id = $5")
                .bind(permissions.bits()).bind(i64::from(active)).bind(value.revision).bind(resource.to_string()).bind(target_member.to_string()).execute(&mut *tx).await?;
        }
        self.bump_epoch(&mut tx, org).await?;
        self.audit_event(
            &mut tx,
            Some(org),
            actor.identity_id,
            if active {
                "content_grant_changed"
            } else {
                "content_grant_revoked"
            },
            resource,
        )
        .await?;
        tx.commit().await?;
        Ok(value)
    }
    /// Only an explicit Share ACL authorizes inspecting other content grants.
    pub async fn content_grants(
        &self,
        actor: &Principal,
        org: Uuid,
        resource: Uuid,
    ) -> Result<Vec<ContentGrant>> {
        let mut tx = self.begin().await?;
        self.authorize_in(&mut tx, actor, org, resource, ContentAction::Share)
            .await?;
        let rows = sqlx::query("SELECT g.* FROM identity_content_grants g JOIN identity_memberships m ON m.id = g.membership_id WHERE g.resource_id = $1 AND m.organization_id = $2 ORDER BY g.membership_id")
            .bind(resource.to_string()).bind(org.to_string()).fetch_all(&mut *tx).await?;
        let mut values = Vec::with_capacity(rows.len());
        for row in rows {
            let permissions: Permissions =
                decoded(&row.try_get::<i64, _>("permissions")?.to_string())?;
            if !permissions.valid() {
                return Err(IdentityError::Corrupt);
            }
            values.push(ContentGrant {
                resource_id: resource,
                membership_id: identifier(row.try_get("membership_id")?)?,
                permissions,
                active: row.try_get::<i64, _>("active")? == 1,
                revision: row.try_get("revision")?,
            });
        }
        tx.commit().await?;
        Ok(values)
    }
    async fn quota(&self, tx: &mut Transaction<'_, Any>, scope: QuotaScope) -> Result<QuotaStatus> {
        let row = sqlx::query("SELECT revision,limit_payload,held_payload,consumed_payload FROM identity_quotas WHERE scope = $1")
            .bind(scope.key()).fetch_optional(&mut **tx).await?.ok_or(IdentityError::QuotaExceeded)?;
        let value = QuotaStatus {
            scope,
            revision: row.try_get("revision")?,
            limit: decoded(&row.try_get::<String, _>("limit_payload")?)?,
            held: decoded(&row.try_get::<String, _>("held_payload")?)?,
            consumed: decoded(&row.try_get::<String, _>("consumed_payload")?)?,
        };
        if !value.limit.valid() || !value.held.valid() || !value.consumed.valid() {
            return Err(IdentityError::Corrupt);
        }
        Ok(value)
    }
    async fn save_quota(&self, tx: &mut Transaction<'_, Any>, quota: &QuotaStatus) -> Result<()> {
        sqlx::query("UPDATE identity_quotas SET revision = $1,limit_payload = $2,held_payload = $3,consumed_payload = $4 WHERE scope = $5")
            .bind(quota.revision).bind(encoded(&quota.limit)?).bind(encoded(&quota.held)?).bind(encoded(&quota.consumed)?)
            .bind(quota.scope.key()).execute(&mut **tx).await?;
        Ok(())
    }
    /// Lifetime totals: changing a plan never resets history or retained charges.
    /// Both platform and tenant limits must exist before admission is possible.
    pub async fn configure_quota(
        &self,
        actor: &Principal,
        scope: QuotaScope,
        expected_revision: i64,
        limit: QuotaAmounts,
    ) -> Result<QuotaStatus> {
        if !limit.valid() {
            return Err(IdentityError::Validation);
        }
        let mut tx = self.begin().await?;
        self.platform_admin(&mut tx, actor).await?;
        if let QuotaScope::Organization(org) = scope {
            let count: i64 =
                sqlx::query("SELECT COUNT(*) AS count FROM identity_organizations WHERE id = $1")
                    .bind(org.to_string())
                    .fetch_one(&mut *tx)
                    .await?
                    .try_get("count")?;
            if count != 1 {
                return Err(IdentityError::NotFound);
            }
        }
        let mut value = match self.quota(&mut tx, scope).await {
            Ok(value) => value,
            Err(IdentityError::QuotaExceeded) => QuotaStatus {
                scope,
                revision: 0,
                limit,
                held: QuotaAmounts::default(),
                consumed: QuotaAmounts::default(),
            },
            Err(error) => return Err(error),
        };
        if value.revision != expected_revision {
            return Err(IdentityError::Conflict);
        }
        if value.revision == 0 {
            value.consumed.storage_bytes = self
                .storage_import_consumed_in_transaction(&mut tx, scope)
                .await?;
        }
        let charged = value
            .held
            .add(value.consumed)
            .ok_or(IdentityError::QuotaExceeded)?;
        if !charged.fits(limit) {
            return Err(IdentityError::QuotaExceeded);
        }
        value.limit = limit;
        value.revision += 1;
        if expected_revision == 0 {
            sqlx::query("INSERT INTO identity_quotas(scope,revision,limit_payload,held_payload,consumed_payload) VALUES($1,1,$2,$3,$4)")
                .bind(scope.key()).bind(encoded(&limit)?).bind(encoded(&value.held)?).bind(encoded(&value.consumed)?)
                .execute(&mut *tx).await?;
        } else {
            self.save_quota(&mut tx, &value).await?;
        }
        self.audit_event(
            &mut tx,
            match scope {
                QuotaScope::Platform => None,
                QuotaScope::Organization(id) => Some(id),
            },
            actor.identity_id,
            "quota_plan_changed",
            match scope {
                QuotaScope::Platform => actor.identity_id,
                QuotaScope::Organization(id) => id,
            },
        )
        .await?;
        tx.commit().await?;
        Ok(value)
    }
    pub async fn quota_status(&self, actor: &Principal, scope: QuotaScope) -> Result<QuotaStatus> {
        let mut tx = self.begin().await?;
        match scope {
            QuotaScope::Platform => {
                self.platform_admin(&mut tx, actor).await?;
            }
            QuotaScope::Organization(org) => {
                let (_, member) = self.org_member(&mut tx, actor, org).await?;
                if !member.role.manages_members() {
                    return Err(IdentityError::Forbidden);
                }
            }
        }
        let value = self.quota(&mut tx, scope).await?;
        tx.commit().await?;
        Ok(value)
    }
    pub async fn reserve_quota(
        &self,
        actor: &Principal,
        permit: &ContentPermit,
        key: &str,
        amounts: QuotaAmounts,
        expires_at: i64,
    ) -> Result<QuotaReservation> {
        let mut tx = self.pool.begin().await?;
        let value = self
            .lock_quota_in_transaction(&mut tx)
            .await?
            .reserve_quota_in_transaction(actor, permit, key, amounts, expires_at)
            .await?;
        tx.commit().await?;
        Ok(value)
    }

    async fn reserve_quota_locked(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
        permit: &ContentPermit,
        key: &str,
        amounts: QuotaAmounts,
        expires_at: i64,
    ) -> Result<QuotaReservation> {
        if !valid_key(key)
            || !amounts.valid()
            || amounts.concurrent_runs != 1
            || permit.0.action != ContentAction::Execute
        {
            return Err(IdentityError::Validation);
        }
        if actor.identity_id != permit.0.identity_id || actor.session_id != permit.0.session_id {
            return Err(IdentityError::Forbidden);
        }
        self.validate_content_locked(tx, permit).await?;
        let request = digest_request(&("quota_reserve", &permit.0, amounts, expires_at))?;
        if let Some(row) = sqlx::query("SELECT id,request_sha256,initial_receipt FROM identity_reservations WHERE organization_id = $1 AND identity_id = $2 AND intent_key = $3")
            .bind(permit.0.organization_id.to_string()).bind(actor.identity_id.to_string()).bind(key).fetch_optional(&mut **tx).await? {
            if row.try_get::<String,_>("request_sha256")? != request { return Err(IdentityError::Conflict); }
            let (_, _) = self.reservation(tx, identifier(row.try_get("id")?)?).await?;
            let value: QuotaReservation = decoded(&row.try_get::<String,_>("initial_receipt")?)?;
            return Ok(value);
        }
        let observed = self.sample(tx).await?;
        let session_expiry: i64 =
            sqlx::query("SELECT expires_at FROM identity_sessions WHERE id = $1")
                .bind(actor.session_id.to_string())
                .fetch_one(&mut **tx)
                .await?
                .try_get("expires_at")?;
        if expires_at <= observed || expires_at > session_expiry {
            return Err(IdentityError::Validation);
        }
        for scope in [
            QuotaScope::Platform,
            QuotaScope::Organization(permit.0.organization_id),
        ] {
            let mut quota = self.quota(tx, scope).await?;
            quota.held = quota
                .held
                .add(amounts)
                .ok_or(IdentityError::QuotaExceeded)?;
            if !quota
                .held
                .add(quota.consumed)
                .ok_or(IdentityError::QuotaExceeded)?
                .fits(quota.limit)
            {
                return Err(IdentityError::QuotaExceeded);
            }
            quota.revision += 1;
            self.save_quota(tx, &quota).await?;
        }
        let value = QuotaReservation {
            id: Uuid::new_v4(),
            organization_id: permit.0.organization_id,
            resource_id: permit.0.resource_id,
            identity_id: actor.identity_id,
            request_sha256: request.clone(),
            held: amounts,
            consumed: None,
            state: ReservationState::Prepared,
            created_at: observed,
            expires_at,
            completion_evidence_sha256: None,
        };
        sqlx::query("INSERT INTO identity_reservations(id,organization_id,identity_id,intent_key,request_sha256,payload,initial_receipt,content_permit) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
            .bind(value.id.to_string()).bind(value.organization_id.to_string()).bind(actor.identity_id.to_string())
            .bind(key).bind(request).bind(encoded(&value)?).bind(encoded(&value)?).bind(encoded(&permit.0)?).execute(&mut **tx).await?;
        self.audit_event(
            tx,
            Some(value.organization_id),
            actor.identity_id,
            "quota_reserved",
            value.id,
        )
        .await?;
        Ok(value)
    }
    async fn reservation(
        &self,
        tx: &mut Transaction<'_, Any>,
        id: Uuid,
    ) -> Result<(QuotaReservation, ContentPermit)> {
        let row = sqlx::query("SELECT organization_id,identity_id,request_sha256,payload,initial_receipt,content_permit FROM identity_reservations WHERE id = $1")
            .bind(id.to_string()).fetch_optional(&mut **tx).await?.ok_or(IdentityError::NotFound)?;
        let value: QuotaReservation = decoded(&row.try_get::<String, _>("payload")?)?;
        let original: QuotaReservation = decoded(&row.try_get::<String, _>("initial_receipt")?)?;
        let permit: PermitData = decoded(&row.try_get::<String, _>("content_permit")?)?;
        if value.id != id
            || value.organization_id.to_string() != row.try_get::<String, _>("organization_id")?
            || value.identity_id.to_string() != row.try_get::<String, _>("identity_id")?
            || value.request_sha256 != row.try_get::<String, _>("request_sha256")?
            || permit.organization_id != value.organization_id
            || permit.resource_id != value.resource_id
            || permit.identity_id != value.identity_id
            || permit.action != ContentAction::Execute
        {
            return Err(IdentityError::Corrupt);
        }
        let mut immutable = value.clone();
        immutable.state = ReservationState::Prepared;
        immutable.consumed = None;
        immutable.completion_evidence_sha256 = None;
        if immutable != original
            || !value.held.valid()
            || value.held.concurrent_runs != 1
            || value.created_at <= 0
            || value.expires_at <= value.created_at
            || digest_request(&("quota_reserve", &permit, value.held, value.expires_at))?
                != value.request_sha256
        {
            return Err(IdentityError::Corrupt);
        }
        match value.state {
            ReservationState::Prepared | ReservationState::Activated => {
                if value.consumed.is_some() || value.completion_evidence_sha256.is_some() {
                    return Err(IdentityError::Corrupt);
                }
            }
            ReservationState::Settled | ReservationState::Aborted => {
                let used = value.consumed.ok_or(IdentityError::Corrupt)?;
                let evidence = value
                    .completion_evidence_sha256
                    .as_deref()
                    .ok_or(IdentityError::Corrupt)?;
                if !used.valid()
                    || used.concurrent_runs != 0
                    || !used.fits(value.held)
                    || (value.state == ReservationState::Aborted && used != QuotaAmounts::default())
                    || evidence.len() != 64
                    || !evidence
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                {
                    return Err(IdentityError::Corrupt);
                }
            }
        }
        Ok((value, ContentPermit(permit)))
    }
    pub async fn activate_quota(
        &self,
        actor: &Principal,
        original: &QuotaReservation,
    ) -> Result<QuotaExecutionPermit> {
        let mut tx = self.pool.begin().await?;
        let value = self
            .lock_quota_in_transaction(&mut tx)
            .await?
            .activate_quota_in_transaction(actor, original)
            .await?;
        tx.commit().await?;
        Ok(value)
    }

    async fn activate_quota_locked(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
        original: &QuotaReservation,
    ) -> Result<QuotaExecutionPermit> {
        let (mut value, permit) = self.reservation(tx, original.id).await?;
        if value.identity_id != actor.identity_id
            || permit.0.session_id != actor.session_id
            || value.request_sha256 != original.request_sha256
            || value.held != original.held
        {
            return Err(IdentityError::Forbidden);
        }
        self.validate_content_locked(tx, &permit).await?;
        if self.sample(tx).await? >= value.expires_at {
            return Err(IdentityError::Forbidden);
        }
        if value.state != ReservationState::Prepared {
            return Err(IdentityError::Conflict);
        }
        value.state = ReservationState::Activated;
        sqlx::query("UPDATE identity_reservations SET payload = $1 WHERE id = $2")
            .bind(encoded(&value)?)
            .bind(value.id.to_string())
            .execute(&mut **tx)
            .await?;
        self.audit_event(
            tx,
            Some(value.organization_id),
            actor.identity_id,
            "quota_activated",
            value.id,
        )
        .await?;
        Ok(QuotaExecutionPermit {
            reservation: value,
            content: permit,
        })
    }
    pub async fn validate_execution(&self, execution: &QuotaExecutionPermit) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        self.lock_quota_in_transaction(&mut tx)
            .await?
            .validate_execution_in_transaction(execution)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn validate_execution_locked(
        &self,
        tx: &mut Transaction<'_, Any>,
        execution: &QuotaExecutionPermit,
    ) -> Result<()> {
        self.validate_content_locked(tx, &execution.content).await?;
        let (value, _) = self.reservation(tx, execution.reservation.id).await?;
        if value != execution.reservation
            || value.state != ReservationState::Activated
            || self.sample(tx).await? >= value.expires_at
        {
            return Err(IdentityError::Conflict);
        }
        Ok(())
    }
    /// Trusted executor settlement, not a user/admin release endpoint. The
    /// separately configured secret is never a content credential. An activated
    /// job stays fully held until the caller supplies verified positive closure
    /// evidence from the actual executor; this layer only binds that evidence.
    pub async fn finalize_quota(
        &self,
        settlement_secret: &str,
        id: Uuid,
        expected_request_sha256: &str,
        consumed: QuotaAmounts,
        evidence_sha256: &str,
        aborted_before_dispatch: bool,
    ) -> Result<QuotaReservation> {
        let mut tx = self.pool.begin().await?;
        let value = self
            .lock_quota_in_transaction(&mut tx)
            .await?
            .finalize_quota_in_transaction(
                settlement_secret,
                id,
                expected_request_sha256,
                consumed,
                evidence_sha256,
                aborted_before_dispatch,
            )
            .await?;
        tx.commit().await?;
        Ok(value)
    }

    #[allow(clippy::too_many_arguments)]
    async fn finalize_quota_locked(
        &self,
        tx: &mut Transaction<'_, Any>,
        settlement_secret: &str,
        id: Uuid,
        expected_request_sha256: &str,
        consumed: QuotaAmounts,
        evidence_sha256: &str,
        aborted_before_dispatch: bool,
    ) -> Result<QuotaReservation> {
        if !consumed.valid()
            || consumed.concurrent_runs != 0
            || evidence_sha256.len() != 64
            || !evidence_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(IdentityError::Validation);
        }
        let expected: String =
            sqlx::query("SELECT settlement_sha256 FROM identity_installation WHERE id = 1")
                .fetch_one(&mut **tx)
                .await?
                .try_get("settlement_sha256")?;
        if expected != secret_hash("settlement-v1", settlement_secret) {
            return Err(IdentityError::Forbidden);
        }
        let (mut value, _) = self.reservation(tx, id).await?;
        if value.request_sha256 != expected_request_sha256 || !consumed.fits(value.held) {
            return Err(IdentityError::Conflict);
        }
        let target = if aborted_before_dispatch {
            ReservationState::Aborted
        } else {
            ReservationState::Settled
        };
        if matches!(
            value.state,
            ReservationState::Aborted | ReservationState::Settled
        ) {
            if value.state != target
                || value.consumed != Some(consumed)
                || value.completion_evidence_sha256.as_deref() != Some(evidence_sha256)
            {
                return Err(IdentityError::Conflict);
            }
            return Ok(value);
        }
        if (aborted_before_dispatch
            && (value.state != ReservationState::Prepared || consumed != QuotaAmounts::default()))
            || (!aborted_before_dispatch && value.state != ReservationState::Activated)
        {
            return Err(IdentityError::Conflict);
        }
        for scope in [
            QuotaScope::Platform,
            QuotaScope::Organization(value.organization_id),
        ] {
            let mut quota = self.quota(tx, scope).await?;
            quota.held = quota
                .held
                .subtract(value.held)
                .ok_or(IdentityError::Corrupt)?;
            quota.consumed = quota
                .consumed
                .add(consumed)
                .ok_or(IdentityError::QuotaExceeded)?;
            if !quota
                .held
                .add(quota.consumed)
                .ok_or(IdentityError::QuotaExceeded)?
                .fits(quota.limit)
            {
                return Err(IdentityError::Corrupt);
            }
            quota.revision += 1;
            self.save_quota(tx, &quota).await?;
        }
        value.state = target;
        value.consumed = Some(consumed);
        value.completion_evidence_sha256 = Some(evidence_sha256.into());
        sqlx::query("UPDATE identity_reservations SET payload = $1 WHERE id = $2")
            .bind(encoded(&value)?)
            .bind(value.id.to_string())
            .execute(&mut **tx)
            .await?;
        self.audit_event(
            tx,
            Some(value.organization_id),
            value.identity_id,
            "quota_finalized_by_executor",
            value.id,
        )
        .await?;
        Ok(value)
    }
    async fn operation<T: DeserializeOwned>(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: Uuid,
        org: Uuid,
        key: &str,
        request: &str,
    ) -> Result<Option<T>> {
        let row = sqlx::query("SELECT request_sha256,receipt FROM identity_operations WHERE actor_id = $1 AND organization_id = $2 AND intent_key = $3")
            .bind(actor.to_string()).bind(org.to_string()).bind(key).fetch_optional(&mut **tx).await?;
        if let Some(row) = row {
            if row.try_get::<String, _>("request_sha256")? != request {
                return Err(IdentityError::Conflict);
            }
            return Ok(Some(decoded(&row.try_get::<String, _>("receipt")?)?));
        }
        Ok(None)
    }
    async fn save_operation<T: Serialize>(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: Uuid,
        org: Uuid,
        key: &str,
        request: &str,
        receipt: &T,
    ) -> Result<()> {
        sqlx::query("INSERT INTO identity_operations(actor_id,organization_id,intent_key,request_sha256,receipt) VALUES($1,$2,$3,$4,$5)")
            .bind(actor.to_string()).bind(org.to_string()).bind(key).bind(request).bind(encoded(receipt)?).execute(&mut **tx).await?;
        Ok(())
    }
    async fn audit_event(
        &self,
        tx: &mut Transaction<'_, Any>,
        org: Option<Uuid>,
        actor: Uuid,
        action: &str,
        target: Uuid,
    ) -> Result<()> {
        let observed = self.sample(tx).await?;
        let sequence: i64 =
            sqlx::query("SELECT COALESCE(MAX(sequence),0) AS sequence FROM identity_audit")
                .fetch_one(&mut **tx)
                .await?
                .try_get("sequence")?;
        let sequence = sequence.checked_add(1).ok_or(IdentityError::Corrupt)?;
        let event_id = Uuid::new_v4();
        sqlx::query("INSERT INTO identity_audit(id,sequence,organization_id,actor_id,action,target_id,observed_at) VALUES($1,$2,$3,$4,$5,$6,$7)")
            .bind(event_id.to_string()).bind(sequence).bind(org.map(|id| id.to_string())).bind(actor.to_string())
            .bind(action).bind(target.to_string()).bind(observed).execute(&mut **tx).await?;
        let scopes = if let Some(org) = org {
            vec![format!("organization:{org}")]
        } else {
            let mut values = vec![format!("account:{actor}")];
            if target != actor {
                values.push(format!("account:{target}"));
            }
            values
        };
        for scope in scopes {
            self.audit_backfill(tx, &scope).await?;
            crate::audit::append(
                tx,
                &scope,
                crate::audit::MetadataEvent {
                    event_id: &event_id.to_string(),
                    action,
                    actor_id: Some(actor.to_string()),
                    target_id: Some(target.to_string()),
                    observed_at: observed,
                    origin: cowork_contracts::AuditOrigin::NativeV2,
                    source_sha256: None,
                },
            )
            .await?;
        }
        Ok(())
    }
    pub async fn audit(
        &self,
        actor: &Principal,
        org: Uuid,
        after: i64,
        limit: i64,
    ) -> Result<Vec<AuditEntry>> {
        if after < 0 || !(1..=500).contains(&limit) {
            return Err(IdentityError::Validation);
        }
        let mut tx = self.begin().await?;
        let (_, member) = self.org_member(&mut tx, actor, org).await?;
        if !member.role.manages_members() {
            return Err(IdentityError::Forbidden);
        }
        let rows = sqlx::query("SELECT * FROM identity_audit WHERE organization_id = $1 AND sequence > $2 ORDER BY sequence LIMIT $3")
            .bind(org.to_string()).bind(after).bind(limit).fetch_all(&mut *tx).await?;
        let mut values = Vec::with_capacity(rows.len());
        for row in rows {
            values.push(AuditEntry {
                sequence: row.try_get("sequence")?,
                id: identifier(row.try_get("id")?)?,
                organization_id: Some(org),
                actor_id: identifier(row.try_get("actor_id")?)?,
                action: row.try_get("action")?,
                target_id: identifier(row.try_get("target_id")?)?,
                observed_at: row.try_get("observed_at")?,
            });
        }
        tx.commit().await?;
        Ok(values)
    }
}
