//! Separate tenant project storage; no personal project row is adopted or made
//! visible to a tenant, and legacy single-owner queries cannot list these rows.
use super::*;
use cowork_identity::{ContentAction, ContentKind, IdentityError, Principal};
use serde::{Deserialize, Serialize};
use sqlx::{any::AnyRow, Any, Transaction};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CreateTenantProject {
    pub name: String,
    pub idempotency_key: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TenantProject {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub name: String,
    pub revision: i64,
    pub created_at: i64,
}

pub(crate) async fn migrate(tx: &mut Transaction<'_, Any>) -> Result<()> {
    sqlx::query("CREATE TABLE IF NOT EXISTS tenant_project_schema_guard (version BIGINT PRIMARY KEY CHECK(version = 1))")
        .execute(&mut **tx).await.map_err(storage_error)?;
    let versions = sqlx::query("SELECT version FROM tenant_project_schema_guard")
        .fetch_all(&mut **tx)
        .await
        .map_err(storage_error)?;
    if versions.is_empty() {
        sqlx::query("INSERT INTO tenant_project_schema_guard(version) VALUES(1)")
            .execute(&mut **tx)
            .await
            .map_err(storage_error)?;
    } else if versions.len() != 1
        || versions[0]
            .try_get::<i64, _>("version")
            .map_err(storage_error)?
            != 1
    {
        return Err(Error::Validation(
            "Unsupported tenant project schema".into(),
        ));
    }
    sqlx::query("CREATE TABLE IF NOT EXISTS tenant_projects (id TEXT PRIMARY KEY REFERENCES identity_resources(id), organization_id TEXT NOT NULL REFERENCES identity_organizations(id), creator_identity_id TEXT NOT NULL REFERENCES identity_subjects(id), creation_key TEXT NOT NULL, request_sha256 TEXT NOT NULL, creation_payload TEXT NOT NULL, revision BIGINT NOT NULL CHECK(revision > 0), payload TEXT NOT NULL, initial_receipt TEXT NOT NULL, UNIQUE(organization_id,creator_identity_id,creation_key))")
        .execute(&mut **tx).await.map_err(storage_error)?;
    sqlx::query("CREATE INDEX IF NOT EXISTS tenant_project_organization ON tenant_projects(organization_id,id)")
        .execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
fn authority(error: IdentityError) -> Error {
    match error {
        IdentityError::Validation => Error::Validation("Invalid tenant project request".into()),
        IdentityError::Unauthenticated | IdentityError::Forbidden => Error::Forbidden,
        IdentityError::NotFound => Error::NotFound,
        IdentityError::Conflict => Error::Conflict,
        IdentityError::QuotaExceeded => Error::BudgetExceeded,
        other => storage_error(other),
    }
}
fn validate(input: &CreateTenantProject) -> Result<()> {
    valid_text(&input.name, 200)?;
    if input.name.chars().any(char::is_control)
        || input.idempotency_key.trim().is_empty()
        || input.idempotency_key.len() > 128
        || input.idempotency_key.chars().any(char::is_control)
    {
        return Err(Error::Validation("Invalid tenant project request".into()));
    }
    Ok(())
}
fn request(org: Uuid, creator: Uuid, input: &CreateTenantProject) -> Result<String> {
    Ok(hash(
        json_string(&("tenant_project_create/v1", org, creator, input))?.as_bytes(),
    ))
}
fn canonical_id(value: String) -> Result<Uuid> {
    let id = Uuid::parse_str(&value).map_err(storage_error)?;
    if id.is_nil() || value != id.to_string() {
        return Err(Error::Conflict);
    }
    Ok(id)
}
fn record(row: &AnyRow) -> Result<TenantProject> {
    let value: TenantProject = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    let original: TenantProject = parse(
        &row.try_get::<String, _>("initial_receipt")
            .map_err(storage_error)?,
    )?;
    let input: CreateTenantProject = parse(
        &row.try_get::<String, _>("creation_payload")
            .map_err(storage_error)?,
    )?;
    let creator = canonical_id(row.try_get("creator_identity_id").map_err(storage_error)?)?;
    validate(&input)?;
    if value != original
        || value.id != canonical_id(row.try_get("id").map_err(storage_error)?)?
        || value.organization_id
            != canonical_id(row.try_get("organization_id").map_err(storage_error)?)?
        || value.revision != 1
        || value.revision != row.try_get::<i64, _>("revision").map_err(storage_error)?
        || value.created_at <= 0
        || value.name != input.name.trim()
        || input.idempotency_key
            != row
                .try_get::<String, _>("creation_key")
                .map_err(storage_error)?
        || request(value.organization_id, creator, &input)?
            != row
                .try_get::<String, _>("request_sha256")
                .map_err(storage_error)?
    {
        return Err(Error::Conflict);
    }
    Ok(value)
}

impl Store {
    pub async fn create_tenant_project(
        &self,
        actor: &Principal,
        org: Uuid,
        input: CreateTenantProject,
    ) -> Result<TenantProject> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let value = self
            .create_tenant_project_in_transaction(&mut tx, actor, org, input)
            .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub(crate) async fn create_tenant_project_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
        org: Uuid,
        input: CreateTenantProject,
    ) -> Result<TenantProject> {
        validate(&input)?;
        let identity = self.identity();
        // Typed namespace avoids collisions with generic identity resource keys.
        let resource_key = format!(
            "tenant_project.v1:{}",
            hash(input.idempotency_key.as_bytes())
        );
        let resource = identity
            .create_resource_in_transaction(tx, actor, org, ContentKind::Project, &resource_key)
            .await
            .map_err(authority)?;
        let permit = identity
            .authorize_content_in_transaction(tx, actor, org, resource.id, ContentAction::Write)
            .await
            .map_err(authority)?;
        sqlx::query(
            "UPDATE tenant_projects SET revision = revision WHERE id = $1 AND organization_id = $2",
        )
        .bind(resource.id.to_string())
        .bind(org.to_string())
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?;
        let expected = request(org, actor.identity_id(), &input)?;
        let row =
            sqlx::query("SELECT * FROM tenant_projects WHERE id = $1 AND organization_id = $2")
                .bind(resource.id.to_string())
                .bind(org.to_string())
                .fetch_optional(&mut **tx)
                .await
                .map_err(storage_error)?;
        let value = if let Some(row) = row {
            if row
                .try_get::<String, _>("request_sha256")
                .map_err(storage_error)?
                != expected
                || row
                    .try_get::<String, _>("creation_key")
                    .map_err(storage_error)?
                    != input.idempotency_key
                || row
                    .try_get::<String, _>("creator_identity_id")
                    .map_err(storage_error)?
                    != actor.identity_id().to_string()
            {
                return Err(Error::Conflict);
            }
            record(&row)?
        } else {
            let clock = if self.sqlite {
                "SELECT CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER) AS observed"
            } else {
                "SELECT CAST(FLOOR(EXTRACT(EPOCH FROM clock_timestamp()) * 1000) AS BIGINT) AS observed"
            };
            let created_at = sqlx::query(clock)
                .fetch_one(&mut **tx)
                .await
                .map_err(storage_error)?
                .try_get("observed")
                .map_err(storage_error)?;
            let value = TenantProject {
                id: resource.id,
                organization_id: org,
                name: input.name.trim().into(),
                revision: 1,
                created_at,
            };
            sqlx::query("INSERT INTO tenant_projects(id,organization_id,creator_identity_id,creation_key,request_sha256,creation_payload,revision,payload,initial_receipt) VALUES($1,$2,$3,$4,$5,$6,1,$7,$8)")
                .bind(value.id.to_string()).bind(org.to_string()).bind(actor.identity_id().to_string())
                .bind(&input.idempotency_key).bind(expected).bind(json_string(&input)?)
                .bind(json_string(&value)?).bind(json_string(&value)?).execute(&mut **tx).await.map_err(storage_error)?;
            value
        };
        identity
            .validate_content_in_transaction(tx, &permit)
            .await
            .map_err(authority)?;
        Ok(value)
    }
    pub async fn tenant_projects(
        &self,
        actor: &Principal,
        org: Uuid,
    ) -> Result<Vec<TenantProject>> {
        let identity = self.identity();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        identity
            .organization_in_transaction(&mut tx, actor, org)
            .await
            .map_err(authority)?;
        // Current membership generation and explicit Read ACL filter candidates.
        let rows = sqlx::query("SELECT p.* FROM tenant_projects p JOIN identity_resources r ON r.id = p.id AND r.organization_id = p.organization_id AND r.kind = 'project' JOIN identity_memberships m ON m.organization_id = p.organization_id AND m.identity_id = $2 AND m.active = 1 JOIN identity_content_grants g ON g.resource_id = p.id AND g.membership_id = m.id AND g.active = 1 AND (g.permissions & 1) = 1 WHERE p.organization_id = $1 ORDER BY p.id")
            .bind(org.to_string()).bind(actor.identity_id().to_string()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let mut values = Vec::with_capacity(rows.len());
        for row in rows {
            let value = record(&row)?;
            let permit = identity
                .authorize_content_in_transaction(
                    &mut tx,
                    actor,
                    org,
                    value.id,
                    ContentAction::Read,
                )
                .await
                .map_err(authority)?;
            identity
                .validate_content_in_transaction(&mut tx, &permit)
                .await
                .map_err(authority)?;
            values.push(value);
        }
        // Includes empty results and samples actual time after all real waits.
        identity
            .organization_in_transaction(&mut tx, actor, org)
            .await
            .map_err(authority)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(values)
    }
    pub async fn tenant_project(
        &self,
        actor: &Principal,
        org: Uuid,
        project: Uuid,
    ) -> Result<TenantProject> {
        let identity = self.identity();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let permit = identity
            .authorize_content_in_transaction(&mut tx, actor, org, project, ContentAction::Read)
            .await
            .map_err(authority)?;
        sqlx::query(
            "UPDATE tenant_projects SET revision = revision WHERE id = $1 AND organization_id = $2",
        )
        .bind(project.to_string())
        .bind(org.to_string())
        .execute(&mut *tx)
        .await
        .map_err(storage_error)?;
        let row =
            sqlx::query("SELECT * FROM tenant_projects WHERE id = $1 AND organization_id = $2")
                .bind(project.to_string())
                .bind(org.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?
                .ok_or(Error::NotFound)?;
        let value = record(&row)?;
        identity
            .validate_content_in_transaction(&mut tx, &permit)
            .await
            .map_err(authority)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cowork_identity::{OrganizationRole, Permissions};
    const BOOTSTRAP: &str = "tenant-fixture-bootstrap-secret-at-least-32-bytes";
    const SETTLEMENT: &str = "tenant-fixture-settlement-secret-at-least-32-bytes";
    const PASSWORD: &str = "Tenant-fixture-local-password-synthetic!";

    async fn fixture() -> (tempfile::TempDir, Store, Principal, Uuid) {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::sqlite(&directory.path().join("tenant.sqlite3"))
            .await
            .unwrap();
        let identity = store.identity();
        identity
            .initialize_credentials(BOOTSTRAP, SETTLEMENT)
            .await
            .unwrap();
        let (org, issued) = identity
            .bootstrap(
                BOOTSTRAP,
                "tenant-owner",
                "Owner",
                PASSWORD,
                "Product tenant",
            )
            .await
            .unwrap();
        let actor = identity.authenticate_session(&issued.token).await.unwrap();
        (directory, store, actor, org.id)
    }
    fn input(name: &str, key: &str) -> CreateTenantProject {
        CreateTenantProject {
            name: name.into(),
            idempotency_key: key.into(),
        }
    }

    #[tokio::test]
    async fn tenant_project_receipt_is_immutable_and_legacy_projects_remain_separate() {
        let (directory, store, actor, org) = fixture().await;
        let personal = store
            .create_project(CreateProject {
                name: "Personal".into(),
            })
            .await
            .unwrap();
        let project = store
            .create_tenant_project(&actor, org, input("  Tenant original  ", "product-intent"))
            .await
            .unwrap();
        assert_eq!(project.name, "Tenant original");
        assert_eq!(
            store
                .create_tenant_project(&actor, org, input("  Tenant original  ", "product-intent"))
                .await
                .unwrap(),
            project
        );
        assert!(matches!(
            store
                .create_tenant_project(&actor, org, input("Changed", "product-intent"))
                .await,
            Err(Error::Conflict)
        ));
        let legacy = store.projects().await.unwrap();
        assert_eq!(legacy.len(), 1);
        assert_eq!(legacy[0].id, personal.id);
        assert_eq!(
            store.tenant_projects(&actor, org).await.unwrap(),
            vec![project.clone()]
        );
        assert_eq!(
            store.tenant_project(&actor, org, project.id).await.unwrap(),
            project
        );
        assert!(store
            .tenant_project(&actor, org, Uuid::parse_str(&personal.id).unwrap())
            .await
            .is_err());
        store.close().await;
        let reopened = Store::sqlite(&directory.path().join("tenant.sqlite3"))
            .await
            .unwrap();
        let session = reopened
            .identity()
            .login("tenant-owner", PASSWORD, 60_000)
            .await
            .unwrap();
        let actor = reopened
            .identity()
            .authenticate_session(&session.token)
            .await
            .unwrap();
        assert_eq!(
            reopened
                .create_tenant_project(&actor, org, input("  Tenant original  ", "product-intent"))
                .await
                .unwrap(),
            project
        );
        assert_eq!(reopened.projects().await.unwrap()[0].id, personal.id);
        reopened.close().await;
    }
    #[tokio::test]
    async fn tenant_projects_require_current_acl_and_reenrollment_does_not_restore_access() {
        let (_dir, store, owner, org) = fixture().await;
        let identity = store.identity();
        let user = identity
            .register_local_identity("tenant-admin", "Admin", PASSWORD)
            .await
            .unwrap();
        let session = identity
            .login("tenant-admin", PASSWORD, 60_000)
            .await
            .unwrap();
        let admin = identity.authenticate_session(&session.token).await.unwrap();
        let member = identity
            .add_member(&owner, org, user.id, OrganizationRole::Admin)
            .await
            .unwrap();
        let project = store
            .create_tenant_project(
                &owner,
                org,
                input("Owner private", "private-product-intent"),
            )
            .await
            .unwrap();
        assert!(store.tenant_projects(&admin, org).await.unwrap().is_empty());
        assert!(store.tenant_project(&admin, org, project.id).await.is_err());
        identity
            .grant_content(
                &owner,
                org,
                project.id,
                member.id,
                0,
                Permissions::read_only(),
                true,
            )
            .await
            .unwrap();
        assert_eq!(
            store.tenant_projects(&admin, org).await.unwrap()[0].id,
            project.id
        );
        identity
            .change_member(&owner, org, member.id, member.revision, member.role, false)
            .await
            .unwrap();
        assert!(store.tenant_project(&admin, org, project.id).await.is_err());
        let replacement = identity
            .add_member(&owner, org, user.id, OrganizationRole::Viewer)
            .await
            .unwrap();
        assert_ne!(replacement.id, member.id);
        assert!(store.tenant_projects(&admin, org).await.unwrap().is_empty());
        assert!(store
            .create_tenant_project(&admin, org, input("Viewer create", "forbidden-intent"))
            .await
            .is_err());
        let other = identity
            .create_organization(&owner, "Other product tenant", "other-product-org")
            .await
            .unwrap();
        assert!(store
            .tenant_project(&owner, other.id, project.id)
            .await
            .is_err());
        assert!(store.tenant_projects(&admin, other.id).await.is_err());
        store.close().await;
    }
    #[tokio::test]
    async fn failed_product_insert_rolls_back_the_resource_acl_and_original_receipt() {
        let (_dir, store, actor, org) = fixture().await;
        sqlx::query("CREATE TRIGGER tenant_fixture_reject_insert BEFORE INSERT ON tenant_projects BEGIN SELECT RAISE(ABORT,'fixture product write rejected'); END")
            .execute(&store.pool).await.unwrap();
        assert!(store
            .create_tenant_project(&actor, org, input("Atomic product", "atomic-intent"))
            .await
            .is_err());
        assert!(store
            .identity()
            .resources(&actor, org)
            .await
            .unwrap()
            .is_empty());
        let count: i64 = sqlx::query("SELECT COUNT(*) AS count FROM identity_operations")
            .fetch_one(&store.pool)
            .await
            .unwrap()
            .get("count");
        assert_eq!(count, 0);
        sqlx::query("DROP TRIGGER tenant_fixture_reject_insert")
            .execute(&store.pool)
            .await
            .unwrap();
        store
            .create_tenant_project(&actor, org, input("Atomic product", "atomic-intent"))
            .await
            .unwrap();
        assert_eq!(store.tenant_projects(&actor, org).await.unwrap().len(), 1);
        store.close().await;
    }
    #[tokio::test]
    async fn tenant_lists_authenticate_empty_scopes_and_expired_sessions() {
        let (_dir, store, actor, org) = fixture().await;
        assert!(store.tenant_projects(&actor, org).await.unwrap().is_empty());
        store.identity().logout(&actor).await.unwrap();
        assert!(matches!(
            store.tenant_projects(&actor, org).await,
            Err(Error::Forbidden)
        ));
        store.close().await;
    }

    #[tokio::test]
    #[ignore = "requires a fresh private PostgreSQL fixture database"]
    async fn postgres_tenant_projects_contract() {
        let url = std::env::var("COWORK_TENANT_PROJECT_TEST_POSTGRES_URL")
            .expect("private tenant project fixture URL required");
        assert!(url.starts_with("postgres:") || url.starts_with("postgresql:"));
        sqlx::any::install_default_drivers();
        let preflight = AnyPool::connect(&url).await.unwrap();
        let count: i64 = sqlx::query(
            "SELECT COUNT(*) AS count FROM information_schema.tables WHERE table_schema = 'public'",
        )
        .fetch_one(&preflight)
        .await
        .unwrap()
        .get("count");
        assert_eq!(
            count, 0,
            "tenant project contract requires an unused private fixture database"
        );
        preflight.close().await;
        let store = Store::connect(&url).await.unwrap();
        let identity = store.identity();
        identity
            .initialize_credentials(BOOTSTRAP, SETTLEMENT)
            .await
            .unwrap();
        let (org, issued) = identity
            .bootstrap(
                BOOTSTRAP,
                "tenant-owner",
                "Owner",
                PASSWORD,
                "Private PG tenant",
            )
            .await
            .unwrap();
        let owner = identity.authenticate_session(&issued.token).await.unwrap();
        let personal = store
            .create_project(CreateProject {
                name: "Personal PG project".into(),
            })
            .await
            .unwrap();
        let project = store
            .create_tenant_project(
                &owner,
                org.id,
                input("PG scoped project", "pg-project-intent"),
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .create_tenant_project(
                    &owner,
                    org.id,
                    input("PG scoped project", "pg-project-intent")
                )
                .await
                .unwrap(),
            project
        );
        assert!(matches!(
            store
                .create_tenant_project(
                    &owner,
                    org.id,
                    input("Changed PG project", "pg-project-intent")
                )
                .await,
            Err(Error::Conflict)
        ));
        assert_eq!(store.projects().await.unwrap().len(), 1);
        assert_eq!(store.projects().await.unwrap()[0].id, personal.id);
        assert!(store
            .tenant_project(&owner, org.id, Uuid::parse_str(&personal.id).unwrap())
            .await
            .is_err());
        let user = identity
            .register_local_identity("pg-tenant-admin", "Admin", PASSWORD)
            .await
            .unwrap();
        let issued = identity
            .login("pg-tenant-admin", PASSWORD, 60_000)
            .await
            .unwrap();
        let actor = identity.authenticate_session(&issued.token).await.unwrap();
        let member = identity
            .add_member(&owner, org.id, user.id, OrganizationRole::Admin)
            .await
            .unwrap();
        assert!(store
            .tenant_projects(&actor, org.id)
            .await
            .unwrap()
            .is_empty());
        assert!(store
            .tenant_project(&actor, org.id, project.id)
            .await
            .is_err());
        identity
            .grant_content(
                &owner,
                org.id,
                project.id,
                member.id,
                0,
                Permissions::read_only(),
                true,
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .tenant_project(&actor, org.id, project.id)
                .await
                .unwrap(),
            project
        );
        identity
            .change_member(&owner, org.id, member.id, 1, member.role, false)
            .await
            .unwrap();
        identity
            .add_member(&owner, org.id, user.id, OrganizationRole::Member)
            .await
            .unwrap();
        assert!(store
            .tenant_projects(&actor, org.id)
            .await
            .unwrap()
            .is_empty());
        let other = identity
            .create_organization(&owner, "Other PG tenant", "other-pg-tenant")
            .await
            .unwrap();
        assert!(store
            .tenant_project(&owner, other.id, project.id)
            .await
            .is_err());
        store.close().await;
        let reopened = Store::connect(&url).await.unwrap();
        let issued = reopened
            .identity()
            .login("tenant-owner", PASSWORD, 60_000)
            .await
            .unwrap();
        let actor = reopened
            .identity()
            .authenticate_session(&issued.token)
            .await
            .unwrap();
        assert_eq!(
            reopened
                .create_tenant_project(
                    &actor,
                    org.id,
                    input("PG scoped project", "pg-project-intent")
                )
                .await
                .unwrap(),
            project
        );
        assert_eq!(
            reopened.tenant_projects(&actor, org.id).await.unwrap(),
            vec![project]
        );
        reopened.close().await;
    }
}
