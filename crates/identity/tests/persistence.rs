use cowork_identity::*;
use sqlx::{AnyPool, Row};
use tempfile::TempDir;

const BOOTSTRAP: &str = "synthetic-bootstrap-credential-32-bytes-minimum";
const SETTLEMENT: &str = "synthetic-executor-settlement-32-bytes-minimum";
const PASSWORD: &str = "Synthetic-password-not-for-real-use!";

struct Fixture {
    directory: TempDir,
    store: IdentityStore,
    admin: Principal,
    org: Organization,
    session: SessionIssued,
}
impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let store = IdentityStore::sqlite(&directory.path().join("identity.sqlite3"))
            .await
            .unwrap();
        store
            .initialize_credentials(BOOTSTRAP, SETTLEMENT)
            .await
            .unwrap();
        let (org, session) = store
            .bootstrap(BOOTSTRAP, "root-user", "Root", PASSWORD, "Tenant Alpha")
            .await
            .unwrap();
        let admin = store.authenticate_session(&session.token).await.unwrap();
        Self {
            directory,
            store,
            admin,
            org,
            session,
        }
    }
    async fn pool(&self) -> AnyPool {
        AnyPool::connect(&format!(
            "sqlite:{}?mode=rw",
            self.directory.path().join("identity.sqlite3").display()
        ))
        .await
        .unwrap()
    }
    async fn user(&self, name: &str) -> (Identity, Principal) {
        let identity = self
            .store
            .register_local_identity(name, name, PASSWORD)
            .await
            .unwrap();
        let issued = self.store.login(name, PASSWORD, 3_600_000).await.unwrap();
        let actor = self
            .store
            .authenticate_session(&issued.token)
            .await
            .unwrap();
        (identity, actor)
    }
    async fn resource(&self, key: &str) -> ContentResource {
        self.store
            .create_resource(&self.admin, self.org.id, ContentKind::Project, key)
            .await
            .unwrap()
    }
    async fn quota(&self, limit: QuotaAmounts) {
        self.store
            .configure_quota(&self.admin, QuotaScope::Platform, 0, limit)
            .await
            .unwrap();
        self.store
            .configure_quota(&self.admin, QuotaScope::Organization(self.org.id), 0, limit)
            .await
            .unwrap();
    }
}
fn amounts(tokens: i64, runs: i64) -> QuotaAmounts {
    QuotaAmounts {
        model_tokens: tokens,
        cost_microunits: tokens * 2,
        storage_bytes: tokens * 3,
        concurrent_runs: runs,
    }
}
fn forbidden<T: std::fmt::Debug>(result: Result<T>) {
    assert!(
        matches!(
            result,
            Err(IdentityError::Forbidden)
                | Err(IdentityError::NotFound)
                | Err(IdentityError::Unauthenticated)
        ),
        "{result:?}"
    );
}

#[tokio::test]
async fn credentials_sessions_and_bootstrap_are_persistent_and_separate() {
    let f = Fixture::new().await;
    assert!(matches!(
        f.store
            .authenticate_session("single-owner-daemon-token")
            .await,
        Err(IdentityError::Unauthenticated)
    ));
    forbidden(
        f.store
            .login("root-user", "incorrect synthetic password", 60_000)
            .await,
    );
    assert!(matches!(
        f.store
            .bootstrap(BOOTSTRAP, "root2", "Root2", PASSWORD, "Another")
            .await,
        Err(IdentityError::Conflict)
    ));
    let pool = f.pool().await;
    let row =
        sqlx::query("SELECT password_hash,password_salt FROM identity_subjects WHERE id = $1")
            .bind(f.admin.identity_id().to_string())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_ne!(row.get::<String, _>("password_hash"), PASSWORD);
    assert_eq!(row.get::<String, _>("password_hash").len(), 64);
    let persisted: String = sqlx::query("SELECT token_sha256 FROM identity_sessions WHERE id = $1")
        .bind(f.admin.session_id().to_string())
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("token_sha256");
    assert_ne!(persisted, f.session.token);
    assert!(!format!("{:?}", f.session).contains(&f.session.token));
    pool.close().await;
    f.store.close().await;
    let reopened = IdentityStore::sqlite(&f.directory.path().join("identity.sqlite3"))
        .await
        .unwrap();
    reopened
        .initialize_credentials(BOOTSTRAP, SETTLEMENT)
        .await
        .unwrap();
    let principal = reopened
        .authenticate_session(&f.session.token)
        .await
        .unwrap();
    assert_eq!(
        reopened.organizations(&principal).await.unwrap()[0].id,
        f.org.id
    );
    forbidden(
        reopened
            .initialize_credentials("replacement-bootstrap-secret-32-bytes-minimum", SETTLEMENT)
            .await,
    );
    reopened.logout(&principal).await.unwrap();
    forbidden(reopened.authenticate_session(&f.session.token).await);
    forbidden(reopened.self_identity(&principal).await);
    reopened.close().await;
}

#[tokio::test]
async fn password_change_and_administrative_revocation_fence_every_previous_session() {
    let f = Fixture::new().await;
    let another = f.store.login("root-user", PASSWORD, 60_000).await.unwrap();
    let another_actor = f.store.authenticate_session(&another.token).await.unwrap();
    let new_password = "Second-synthetic-password-not-for-real-use!";
    forbidden(
        f.store
            .change_password(&f.admin, "wrong synthetic password", new_password, 60_000)
            .await,
    );
    let changed = f
        .store
        .change_password(&f.admin, PASSWORD, new_password, 60_000)
        .await
        .unwrap();
    forbidden(f.store.self_identity(&f.admin).await);
    forbidden(f.store.self_identity(&another_actor).await);
    forbidden(f.store.login("root-user", PASSWORD, 60_000).await);
    let principal = f.store.authenticate_session(&changed.token).await.unwrap();
    assert_eq!(changed.identity.revision, 2);
    // Last-owner protection does not block emergency session revocation.
    f.store
        .revoke_identity_sessions(&principal, principal.identity_id(), 2)
        .await
        .unwrap();
    forbidden(f.store.authenticate_session(&changed.token).await);
    let new = f
        .store
        .login("root-user", new_password, 60_000)
        .await
        .unwrap();
    let principal = f.store.authenticate_session(&new.token).await.unwrap();
    assert_eq!(
        f.store.organizations(&principal).await.unwrap()[0].id,
        f.org.id
    );
    f.store.close().await;
}

async fn isolation_contract(store: &IdentityStore, admin: &Principal, org: &Organization) {
    let user = store
        .register_local_identity("member-one", "Member", PASSWORD)
        .await
        .unwrap();
    assert!(!user.platform_admin);
    let session = store
        .login("MEMBER-ONE", PASSWORD, 3_600_000)
        .await
        .unwrap();
    let member = store.authenticate_session(&session.token).await.unwrap();
    assert!(store.organizations(&member).await.unwrap().is_empty());
    forbidden(store.organization(&member, org.id).await);
    let membership = store
        .add_member(admin, org.id, user.id, OrganizationRole::Admin)
        .await
        .unwrap();
    let private = store
        .create_resource(admin, org.id, ContentKind::Project, "admin-document")
        .await
        .unwrap();
    assert!(store.resources(&member, org.id).await.unwrap().is_empty());
    forbidden(
        store
            .authorize_content(&member, org.id, private.id, ContentAction::Read)
            .await,
    );
    forbidden(store.content_grants(&member, org.id, private.id).await);
    let owned_by_member = store
        .create_resource(&member, org.id, ContentKind::Knowledge, "member-document")
        .await
        .unwrap();
    // Both organization Owner and platform administrator still need a content ACL.
    forbidden(
        store
            .authorize_content(admin, org.id, owned_by_member.id, ContentAction::Read)
            .await,
    );
    let owner_membership = store
        .members(admin, org.id)
        .await
        .unwrap()
        .into_iter()
        .find(|m| m.identity_id == admin.identity_id())
        .unwrap();
    store
        .grant_content(
            &member,
            org.id,
            owned_by_member.id,
            owner_membership.id,
            0,
            Permissions::read_only(),
            true,
        )
        .await
        .unwrap();
    store
        .authorize_content(admin, org.id, owned_by_member.id, ContentAction::Read)
        .await
        .unwrap();
    forbidden(
        store
            .authorize_content(admin, org.id, owned_by_member.id, ContentAction::Share)
            .await,
    );
    store
        .grant_content(
            admin,
            org.id,
            private.id,
            membership.id,
            0,
            Permissions::all(),
            true,
        )
        .await
        .unwrap();
    let permit = store
        .authorize_content(&member, org.id, private.id, ContentAction::Execute)
        .await
        .unwrap();
    store.validate_content(&permit).await.unwrap();
    let other = store
        .create_organization(admin, "Other tenant", "other-tenant")
        .await
        .unwrap();
    forbidden(
        store
            .authorize_content(&member, other.id, private.id, ContentAction::Read)
            .await,
    );
    forbidden(
        store
            .authorize_content(admin, other.id, private.id, ContentAction::Read)
            .await,
    );
    let revoked = store
        .change_member(
            admin,
            org.id,
            membership.id,
            membership.revision,
            membership.role,
            false,
        )
        .await
        .unwrap();
    assert!(!revoked.active);
    forbidden(store.validate_content(&permit).await);
    let replacement = store
        .add_member(admin, org.id, user.id, OrganizationRole::Member)
        .await
        .unwrap();
    assert_ne!(replacement.id, membership.id);
    assert!(store.resources(&member, org.id).await.unwrap().is_empty());
    forbidden(
        store
            .authorize_content(&member, org.id, private.id, ContentAction::Read)
            .await,
    );
    // Retrying a prior object-creation receipt cannot adopt its old ACL.
    forbidden(
        store
            .create_resource(&member, org.id, ContentKind::Knowledge, "member-document")
            .await,
    );
    assert!(matches!(
        store
            .change_member(
                admin,
                org.id,
                membership.id,
                revoked.revision,
                OrganizationRole::Member,
                true
            )
            .await,
        Err(IdentityError::Conflict)
    ));
}

#[tokio::test]
async fn roles_do_not_bypass_content_isolation_and_reenrollment_does_not_restore_grants() {
    let f = Fixture::new().await;
    isolation_contract(&f.store, &f.admin, &f.org).await;
    f.store.close().await;
}

#[tokio::test]
async fn viewer_and_admin_cannot_escalate_and_last_owner_is_retained() {
    let f = Fixture::new().await;
    let (viewer, actor) = f.user("viewer-user").await;
    let viewer_member = f
        .store
        .add_member(&f.admin, f.org.id, viewer.id, OrganizationRole::Viewer)
        .await
        .unwrap();
    let resource = f.resource("viewer-content").await;
    forbidden(
        f.store
            .grant_content(
                &f.admin,
                f.org.id,
                resource.id,
                viewer_member.id,
                0,
                Permissions::all(),
                true,
            )
            .await,
    );
    f.store
        .grant_content(
            &f.admin,
            f.org.id,
            resource.id,
            viewer_member.id,
            0,
            Permissions::read_only(),
            true,
        )
        .await
        .unwrap();
    f.store
        .authorize_content(&actor, f.org.id, resource.id, ContentAction::Read)
        .await
        .unwrap();
    forbidden(
        f.store
            .authorize_content(&actor, f.org.id, resource.id, ContentAction::Execute)
            .await,
    );
    forbidden(
        f.store
            .create_resource(&actor, f.org.id, ContentKind::Project, "viewer-new")
            .await,
    );
    let admin_member = f
        .store
        .change_member(
            &f.admin,
            f.org.id,
            viewer_member.id,
            1,
            OrganizationRole::Admin,
            true,
        )
        .await
        .unwrap();
    let owner = f
        .store
        .members(&f.admin, f.org.id)
        .await
        .unwrap()
        .into_iter()
        .find(|m| m.role == OrganizationRole::Owner)
        .unwrap();
    forbidden(
        f.store
            .change_member(
                &actor,
                f.org.id,
                owner.id,
                owner.revision,
                OrganizationRole::Member,
                false,
            )
            .await,
    );
    forbidden(
        f.store
            .change_member(
                &actor,
                f.org.id,
                admin_member.id,
                admin_member.revision,
                OrganizationRole::Owner,
                true,
            )
            .await,
    );
    forbidden(
        f.store
            .change_member(
                &f.admin,
                f.org.id,
                owner.id,
                owner.revision,
                OrganizationRole::Member,
                false,
            )
            .await,
    );
    forbidden(
        f.store
            .disable_identity(&f.admin, f.admin.identity_id(), 1)
            .await,
    );
    f.store.close().await;
}

#[tokio::test]
async fn organization_suspension_and_acl_revision_invalidate_previously_issued_permits() {
    let f = Fixture::new().await;
    let resource = f.resource("epoch-resource").await;
    let original = f
        .store
        .authorize_content(&f.admin, f.org.id, resource.id, ContentAction::Execute)
        .await
        .unwrap();
    let current = f.store.organization(&f.admin, f.org.id).await.unwrap();
    let suspended = f
        .store
        .set_organization_state(
            &f.admin,
            f.org.id,
            current.revision,
            OrganizationState::Suspended,
        )
        .await
        .unwrap();
    forbidden(f.store.validate_content(&original).await);
    let active = f
        .store
        .set_organization_state(
            &f.admin,
            f.org.id,
            suspended.revision,
            OrganizationState::Active,
        )
        .await
        .unwrap();
    assert!(matches!(
        f.store.validate_content(&original).await,
        Err(IdentityError::Conflict)
    ));
    let fresh = f
        .store
        .authorize_content(&f.admin, f.org.id, resource.id, ContentAction::Execute)
        .await
        .unwrap();
    f.store.validate_content(&fresh).await.unwrap();
    let owner = f
        .store
        .members(&f.admin, f.org.id)
        .await
        .unwrap()
        .pop()
        .unwrap();
    f.store
        .grant_content(
            &f.admin,
            f.org.id,
            resource.id,
            owner.id,
            1,
            Permissions::read_only(),
            true,
        )
        .await
        .unwrap();
    forbidden(f.store.validate_content(&fresh).await);
    let closed = f
        .store
        .set_organization_state(
            &f.admin,
            f.org.id,
            active.revision + 1,
            OrganizationState::Closed,
        )
        .await
        .unwrap();
    assert!(matches!(
        f.store
            .set_organization_state(
                &f.admin,
                f.org.id,
                closed.revision,
                OrganizationState::Active
            )
            .await,
        Err(IdentityError::Conflict)
    ));
    f.store.close().await;
}

async fn quota_contract(
    store: &IdentityStore,
    admin: &Principal,
    org: &Organization,
    expires_at: i64,
) {
    let resource = store
        .create_resource(admin, org.id, ContentKind::Order, "quota-order")
        .await
        .unwrap();
    let permit = store
        .authorize_content(admin, org.id, resource.id, ContentAction::Execute)
        .await
        .unwrap();
    assert!(matches!(
        store
            .reserve_quota(
                admin,
                &permit,
                "missing-global-plan",
                amounts(10, 1),
                expires_at
            )
            .await,
        Err(IdentityError::QuotaExceeded)
    ));
    store
        .configure_quota(admin, QuotaScope::Platform, 0, amounts(100, 1))
        .await
        .unwrap();
    assert!(matches!(
        store
            .reserve_quota(
                admin,
                &permit,
                "missing-org-plan",
                amounts(10, 1),
                expires_at
            )
            .await,
        Err(IdentityError::QuotaExceeded)
    ));
    assert_eq!(
        store
            .quota_status(admin, QuotaScope::Platform)
            .await
            .unwrap()
            .held,
        QuotaAmounts::default()
    );
    store
        .configure_quota(admin, QuotaScope::Organization(org.id), 0, amounts(100, 1))
        .await
        .unwrap();
    let receipt = store
        .reserve_quota(admin, &permit, "operation-one", amounts(70, 1), expires_at)
        .await
        .unwrap();
    assert_eq!(
        receipt,
        store
            .reserve_quota(admin, &permit, "operation-one", amounts(70, 1), expires_at)
            .await
            .unwrap()
    );
    assert!(matches!(
        store
            .reserve_quota(admin, &permit, "operation-one", amounts(69, 1), expires_at)
            .await,
        Err(IdentityError::Conflict)
    ));
    assert!(matches!(
        store
            .reserve_quota(admin, &permit, "operation-two", amounts(1, 1), expires_at)
            .await,
        Err(IdentityError::QuotaExceeded)
    ));
    let status = store
        .quota_status(admin, QuotaScope::Platform)
        .await
        .unwrap();
    assert_eq!(status.held, amounts(70, 1));
    assert!(matches!(
        store
            .configure_quota(admin, QuotaScope::Platform, status.revision, amounts(50, 1))
            .await,
        Err(IdentityError::QuotaExceeded)
    ));
    let execution = store.activate_quota(admin, &receipt).await.unwrap();
    store.validate_execution(&execution).await.unwrap();
    forbidden(
        store
            .finalize_quota(
                "wrong executor credential",
                receipt.id,
                &receipt.request_sha256,
                amounts(20, 0),
                &"a".repeat(64),
                false,
            )
            .await,
    );
    assert!(matches!(
        store
            .finalize_quota(
                SETTLEMENT,
                receipt.id,
                &receipt.request_sha256,
                QuotaAmounts::default(),
                &"a".repeat(64),
                true
            )
            .await,
        Err(IdentityError::Conflict)
    ));
    // Withdrawal prevents dispatch yet cannot refund an already activated run.
    store.logout(admin).await.unwrap();
    forbidden(store.validate_execution(&execution).await);
    let new_session = store.login("root-user", PASSWORD, 3_600_000).await.unwrap();
    let new_admin = store
        .authenticate_session(&new_session.token)
        .await
        .unwrap();
    assert_eq!(
        store
            .quota_status(&new_admin, QuotaScope::Platform)
            .await
            .unwrap()
            .held,
        amounts(70, 1)
    );
    let settled = store
        .finalize_quota(
            SETTLEMENT,
            receipt.id,
            &receipt.request_sha256,
            amounts(20, 0),
            &"a".repeat(64),
            false,
        )
        .await
        .unwrap();
    assert_eq!(settled.state, ReservationState::Settled);
    assert_eq!(
        settled,
        store
            .finalize_quota(
                SETTLEMENT,
                receipt.id,
                &receipt.request_sha256,
                amounts(20, 0),
                &"a".repeat(64),
                false
            )
            .await
            .unwrap()
    );
    assert!(matches!(
        store
            .finalize_quota(
                SETTLEMENT,
                receipt.id,
                &receipt.request_sha256,
                amounts(19, 0),
                &"a".repeat(64),
                false
            )
            .await,
        Err(IdentityError::Conflict)
    ));
    assert!(matches!(
        store
            .finalize_quota(
                SETTLEMENT,
                receipt.id,
                &receipt.request_sha256,
                amounts(20, 0),
                &"b".repeat(64),
                false
            )
            .await,
        Err(IdentityError::Conflict)
    ));
    for scope in [QuotaScope::Platform, QuotaScope::Organization(org.id)] {
        let status = store.quota_status(&new_admin, scope).await.unwrap();
        assert_eq!(status.held, QuotaAmounts::default());
        assert_eq!(status.consumed, amounts(20, 0));
    }
}

#[tokio::test]
async fn quota_reservations_are_atomic_idempotent_and_require_executor_closure() {
    let f = Fixture::new().await;
    quota_contract(&f.store, &f.admin, &f.org, f.session.expires_at - 1).await;
    f.store.close().await;
}

#[tokio::test]
async fn concurrent_quota_admission_never_overbooks_the_global_limit() {
    let f = Fixture::new().await;
    f.quota(amounts(100, 1)).await;
    let resource = f.resource("concurrent-resource").await;
    let permit = f
        .store
        .authorize_content(&f.admin, f.org.id, resource.id, ContentAction::Execute)
        .await
        .unwrap();
    let other = f
        .store
        .create_organization(&f.admin, "Second competing tenant", "global-competitor")
        .await
        .unwrap();
    f.store
        .configure_quota(
            &f.admin,
            QuotaScope::Organization(other.id),
            0,
            amounts(100, 1),
        )
        .await
        .unwrap();
    let other_resource = f
        .store
        .create_resource(
            &f.admin,
            other.id,
            ContentKind::Order,
            "other-concurrent-resource",
        )
        .await
        .unwrap();
    let other_permit = f
        .store
        .authorize_content(
            &f.admin,
            other.id,
            other_resource.id,
            ContentAction::Execute,
        )
        .await
        .unwrap();
    let (first, second) = tokio::join!(
        f.store.reserve_quota(
            &f.admin,
            &permit,
            "concurrent-one",
            amounts(40, 1),
            f.session.expires_at - 1
        ),
        f.store.reserve_quota(
            &f.admin,
            &other_permit,
            "concurrent-two",
            amounts(40, 1),
            f.session.expires_at - 1
        ),
    );
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    let failure = if first.is_ok() { second } else { first };
    assert!(matches!(failure, Err(IdentityError::QuotaExceeded)));
    let status = f
        .store
        .quota_status(&f.admin, QuotaScope::Platform)
        .await
        .unwrap();
    assert_eq!(status.held, amounts(40, 1));
    let one = f
        .store
        .quota_status(&f.admin, QuotaScope::Organization(f.org.id))
        .await
        .unwrap();
    let two = f
        .store
        .quota_status(&f.admin, QuotaScope::Organization(other.id))
        .await
        .unwrap();
    assert_eq!(
        one.held.model_tokens + two.held.model_tokens,
        status.held.model_tokens
    );
    assert_eq!(one.held.concurrent_runs + two.held.concurrent_runs, 1);
    f.store.close().await;
}

#[tokio::test]
async fn finalized_quota_replay_keeps_the_original_receipt_without_reactivation() {
    let f = Fixture::new().await;
    f.quota(amounts(100, 1)).await;
    let resource = f.resource("immutable-receipt-resource").await;
    let permit = f
        .store
        .authorize_content(&f.admin, f.org.id, resource.id, ContentAction::Execute)
        .await
        .unwrap();
    let original = f
        .store
        .reserve_quota(
            &f.admin,
            &permit,
            "immutable-receipt",
            amounts(40, 1),
            f.session.expires_at - 1,
        )
        .await
        .unwrap();
    f.store.activate_quota(&f.admin, &original).await.unwrap();
    f.store
        .finalize_quota(
            SETTLEMENT,
            original.id,
            &original.request_sha256,
            amounts(20, 0),
            &"a".repeat(64),
            false,
        )
        .await
        .unwrap();
    assert_eq!(
        f.store
            .reserve_quota(
                &f.admin,
                &permit,
                "immutable-receipt",
                amounts(40, 1),
                f.session.expires_at - 1
            )
            .await
            .unwrap(),
        original
    );
    assert!(matches!(
        f.store.activate_quota(&f.admin, &original).await,
        Err(IdentityError::Conflict)
    ));
    let status = f
        .store
        .quota_status(&f.admin, QuotaScope::Platform)
        .await
        .unwrap();
    assert_eq!(status.held, QuotaAmounts::default());
    assert_eq!(status.consumed, amounts(20, 0));
    f.store.close().await;
}

#[tokio::test]
async fn unknown_schema_version_is_refused_before_creating_tenant_tables() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("unsupported.sqlite3");
    sqlx::any::install_default_drivers();
    let pool = AnyPool::connect(&format!("sqlite:{}?mode=rwc", path.display()))
        .await
        .unwrap();
    sqlx::query("CREATE TABLE identity_schema_guard (version BIGINT PRIMARY KEY)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO identity_schema_guard(version) VALUES(2)")
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        IdentityStore::sqlite(&path).await,
        Err(IdentityError::Conflict)
    ));
    let count: i64 =
        sqlx::query("SELECT COUNT(*) AS count FROM sqlite_master WHERE name = 'identity_subjects'")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get("count");
    assert_eq!(count, 0);
    pool.close().await;
}

#[tokio::test]
async fn reservation_payload_corruption_cannot_reduce_a_retained_charge() {
    let f = Fixture::new().await;
    f.quota(amounts(100, 1)).await;
    let resource = f.resource("corruption-resource").await;
    let permit = f
        .store
        .authorize_content(&f.admin, f.org.id, resource.id, ContentAction::Execute)
        .await
        .unwrap();
    let original = f
        .store
        .reserve_quota(
            &f.admin,
            &permit,
            "corruption-operation",
            amounts(40, 1),
            f.session.expires_at - 1,
        )
        .await
        .unwrap();
    let pool = f.pool().await;
    let mut corrupted = original.clone();
    corrupted.held.model_tokens = 1;
    sqlx::query("UPDATE identity_reservations SET payload = $1 WHERE id = $2")
        .bind(serde_json::to_string(&corrupted).unwrap())
        .bind(original.id.to_string())
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        f.store.activate_quota(&f.admin, &original).await,
        Err(IdentityError::Corrupt)
    ));
    assert!(matches!(
        f.store
            .finalize_quota(
                SETTLEMENT,
                original.id,
                &original.request_sha256,
                QuotaAmounts::default(),
                &"a".repeat(64),
                true
            )
            .await,
        Err(IdentityError::Corrupt)
    ));
    assert_eq!(
        f.store
            .quota_status(&f.admin, QuotaScope::Platform)
            .await
            .unwrap()
            .held,
        amounts(40, 1)
    );
    pool.close().await;
    f.store.close().await;
}

#[tokio::test]
async fn expiry_and_clock_rollback_fail_closed_without_refunding_holds() {
    let f = Fixture::new().await;
    f.quota(amounts(100, 1)).await;
    let resource = f.resource("expired-resource").await;
    let permit = f
        .store
        .authorize_content(&f.admin, f.org.id, resource.id, ContentAction::Execute)
        .await
        .unwrap();
    let receipt = f
        .store
        .reserve_quota(
            &f.admin,
            &permit,
            "expired-operation",
            amounts(10, 1),
            f.session.expires_at - 1,
        )
        .await
        .unwrap();
    let pool = f.pool().await;
    sqlx::query("UPDATE identity_sessions SET expires_at = 1 WHERE id = $1")
        .bind(f.admin.session_id().to_string())
        .execute(&pool)
        .await
        .unwrap();
    forbidden(f.store.activate_quota(&f.admin, &receipt).await);
    let new_session = f.store.login("root-user", PASSWORD, 60_000).await.unwrap();
    let new_admin = f
        .store
        .authenticate_session(&new_session.token)
        .await
        .unwrap();
    assert_eq!(
        f.store
            .quota_status(&new_admin, QuotaScope::Platform)
            .await
            .unwrap()
            .held,
        amounts(10, 1)
    );
    sqlx::query("UPDATE identity_installation SET utc_highwater = $1 WHERE id = 1")
        .bind(i64::MAX - 1)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        f.store.quota_status(&new_admin, QuotaScope::Platform).await,
        Err(IdentityError::ClockRollback)
    ));
    assert!(matches!(
        f.store
            .finalize_quota(
                SETTLEMENT,
                receipt.id,
                &receipt.request_sha256,
                QuotaAmounts::default(),
                &"a".repeat(64),
                true
            )
            .await,
        Err(IdentityError::ClockRollback)
    ));
    pool.close().await;
    f.store.close().await;
}

#[tokio::test]
async fn content_object_and_acl_creation_roll_back_with_the_product_transaction() {
    let f = Fixture::new().await;
    let pool = f.pool().await;
    let mut tx = pool.begin().await.unwrap();
    let transient = f
        .store
        .create_resource_in_transaction(
            &mut tx,
            &f.admin,
            f.org.id,
            ContentKind::Project,
            "transaction-resource",
        )
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE product_tenant_objects (id TEXT PRIMARY KEY, organization_id TEXT NOT NULL)",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("INSERT INTO product_tenant_objects(id,organization_id) VALUES($1,$2)")
        .bind(transient.id.to_string())
        .bind(f.org.id.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert!(
        f.store
            .resources(&f.admin, f.org.id)
            .await
            .unwrap()
            .is_empty()
    );
    let mut tx = pool.begin().await.unwrap();
    let persisted = f
        .store
        .create_resource_in_transaction(
            &mut tx,
            &f.admin,
            f.org.id,
            ContentKind::Project,
            "transaction-resource",
        )
        .await
        .unwrap();
    assert_ne!(transient.id, persisted.id);
    tx.commit().await.unwrap();
    let permit = f
        .store
        .authorize_content(&f.admin, f.org.id, persisted.id, ContentAction::Write)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    f.store
        .validate_content_in_transaction(&mut tx, &permit)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    pool.close().await;
    f.store.close().await;
}

#[tokio::test]
async fn audit_pagination_has_a_stable_sequence_and_no_secret_values() {
    let f = Fixture::new().await;
    f.resource("audit-first").await;
    f.resource("audit-second").await;
    let whole = f.store.audit(&f.admin, f.org.id, 0, 500).await.unwrap();
    let mut cursor = 0;
    let mut paged = Vec::new();
    loop {
        let mut next = f.store.audit(&f.admin, f.org.id, cursor, 1).await.unwrap();
        if next.is_empty() {
            break;
        }
        cursor = next[0].sequence;
        paged.append(&mut next);
    }
    assert_eq!(whole, paged);
    assert!(
        whole
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence)
    );
    let serialized = serde_json::to_string(&whole).unwrap();
    for value in [BOOTSTRAP, SETTLEMENT, PASSWORD, &f.session.token] {
        assert!(!serialized.contains(value));
    }
    f.store.close().await;
}

/// Requires a fresh private disposable PostgreSQL database supplied by the
/// qualification fixture. Absence is an explicit ignored test, never a pass.
#[tokio::test]
#[ignore = "requires a fresh private PostgreSQL fixture database"]
async fn postgres_persistent_authorization_and_quota_contract() {
    let url =
        std::env::var("COWORK_IDENTITY_TEST_POSTGRES_URL").expect("fixture database URL required");
    assert!(url.starts_with("postgres:") || url.starts_with("postgresql:"));
    sqlx::any::install_default_drivers();
    let preflight = AnyPool::connect(&url).await.unwrap();
    let existing: i64 = sqlx::query("SELECT COUNT(*) AS count FROM information_schema.tables WHERE table_schema = 'public' AND table_name LIKE 'identity_%'").fetch_one(&preflight).await.unwrap().get("count");
    assert_eq!(
        existing, 0,
        "identity test requires an unused fixture database"
    );
    preflight.close().await;
    let store = IdentityStore::connect(&url).await.unwrap();
    store
        .initialize_credentials(BOOTSTRAP, SETTLEMENT)
        .await
        .unwrap();
    let (org, session) = store
        .bootstrap(BOOTSTRAP, "root-user", "Root", PASSWORD, "PG tenant")
        .await
        .unwrap();
    let admin = store.authenticate_session(&session.token).await.unwrap();
    isolation_contract(&store, &admin, &org).await;
    quota_contract(&store, &admin, &org, session.expires_at - 1).await;
    store.close().await;
    let reopened = IdentityStore::connect(&url).await.unwrap();
    let issued = reopened.login("root-user", PASSWORD, 60_000).await.unwrap();
    let actor = reopened.authenticate_session(&issued.token).await.unwrap();
    assert_eq!(
        reopened
            .quota_status(&actor, QuotaScope::Platform)
            .await
            .unwrap()
            .consumed,
        amounts(20, 0)
    );
    reopened.close().await;
}
