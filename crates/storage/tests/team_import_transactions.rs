//! Owned synthetic SQLite imports through the public Store/Identity APIs.
//! No native capture, historical DB, provider, model, executor or live credential.
use base64::{Engine, engine::general_purpose::STANDARD};
use cowork_contracts::{TeamDocument, TeamDocumentKind};
use cowork_domain::Error;
use cowork_identity::{
    IdentityError, OrganizationRole, Permissions, Principal, QuotaAmounts, QuotaScope, QuotaStatus,
    StorageImportCharge,
};
use cowork_storage::{
    CreateTenantProject, Store, TenantProject,
    import_basis::{
        V1ArtifactContent, V1BinaryContent, V1FreshPassword, V1IdentityTargetPlan, V1ImportContent,
        V1ImportPreview, V1ImportedAttachment, V1MessageContent, V1MigrationCatalog,
        V1MigrationMapping, V1MigrationObject, V1MigrationSecrets, V1OrderContent,
        V1ProjectContent, V1ProjectDestination, V1SourceUser, V1TeamImportPreview, V1UserMapping,
    },
};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{AnyPool, Row, any::AnyPoolOptions};
use std::collections::BTreeMap;
use uuid::Uuid;

const ORIGINAL: &str = "Original Gr\u{00fc}\u{00df}e 119,00\n";
const MESSAGE: &str = "An original inert historical message";
const ARTIFACT: &str = "An original inert text artifact\n";
const KNOWLEDGE: &str = "An original imported knowledge definition\n";
const SOURCE_BYTES: &[u8] = b"Owned synthetic typed-import source fixture, not native authority";
const SOURCE_PROJECT: &str = "source-project";
const SOURCE_USER: &str = "source-user";
const STAMP: i64 = 1_700_000_000_000;
const STORAGE_LIMIT: i64 = 1_000_000;

// All-column snapshots of exactly these owned product/Identity tables. The
// installation clock and authentication metadata are deliberately not frozen.
const TABLES: [&str; 21] = [
    "team_orders",
    "v1_import_receipts",
    "v1_team_import_receipts",
    "v1_team_import_history",
    "v1_import_attachments",
    "v1_migration_configurations",
    "team_documents",
    "knowledge_revisions",
    "identity_subjects",
    "identity_sessions",
    "identity_session_scopes",
    "identity_memberships",
    "identity_organizations",
    "identity_resources",
    "identity_content_grants",
    "identity_storage_charges",
    "identity_quotas",
    "identity_operations",
    "identity_audit",
    "tenant_projects",
    "team_project_profiles",
];
type State = BTreeMap<String, Vec<Vec<String>>>;

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn value<T: Serialize>(input: &T) -> Value {
    serde_json::to_value(input).unwrap()
}
fn synthetic_secret(purpose: &str) -> String {
    format!("Synthetic-only-{purpose}-{}-Aa9!", Uuid::new_v4())
}
fn amounts(storage_bytes: i64) -> QuotaAmounts {
    QuotaAmounts {
        model_tokens: 0,
        cost_microunits: 0,
        storage_bytes,
        concurrent_runs: 0,
    }
}
fn forbidden<T>(result: cowork_domain::Result<T>) {
    match result {
        Err(Error::Forbidden) => (),
        Err(other) => panic!("Expected Forbidden, got {other:?}"),
        Ok(_) => panic!("Read-only authority unexpectedly permitted a write"),
    }
}
fn conflict<T>(result: cowork_domain::Result<T>) {
    match result {
        Err(Error::Conflict) => (),
        Err(other) => panic!("Expected Conflict, got {other:?}"),
        Ok(_) => panic!("A conflicting import unexpectedly succeeded"),
    }
}
fn not_found<T>(result: cowork_domain::Result<T>) {
    match result {
        Err(Error::NotFound) => (),
        Err(other) => panic!("Expected NotFound, got {other:?}"),
        Ok(_) => panic!("Uncommitted team content unexpectedly became readable"),
    }
}
fn quota_exceeded(result: cowork_domain::Result<V1TeamImportPreview>) {
    match result {
        Err(Error::Storage(source)) => assert!(
            matches!(
                source.downcast_ref::<IdentityError>(),
                Some(IdentityError::QuotaExceeded)
            ),
            "Expected boxed IdentityError::QuotaExceeded, got {source:?}"
        ),
        Err(other) => panic!("Expected boxed Identity QuotaExceeded, got {other:?}"),
        Ok(_) => panic!("Organization storage quota unexpectedly admitted the import"),
    }
}

struct Fixture {
    directory: tempfile::TempDir,
    store: Store,
    pool: AnyPool,
    owner: Principal,
    team: Uuid,
    project: TenantProject,
    platform_quota: QuotaStatus,
    organization_quota: QuotaStatus,
}

impl Fixture {
    async fn new(organization_storage: i64) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("team-import.sqlite");
        let store = Store::sqlite(&database).await.unwrap();
        let encoded = percent_encoding::utf8_percent_encode(
            database.to_str().unwrap(),
            percent_encoding::NON_ALPHANUMERIC,
        )
        .to_string();
        let pool = AnyPoolOptions::new()
            .max_connections(2)
            .after_connect(|connection, _| {
                Box::pin(async move {
                    for pragma in ["PRAGMA foreign_keys=ON", "PRAGMA busy_timeout=10000"] {
                        sqlx::query(pragma).execute(&mut *connection).await?;
                    }
                    Ok(())
                })
            })
            .connect(&format!("sqlite:{encoded}?mode=rw"))
            .await
            .unwrap();
        // Observe the actual migration default; never disable management MFA.
        let policy: i64 = sqlx::query_scalar(
            "SELECT admin_required FROM android_auth_security_policy WHERE id=1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            policy, 0,
            "This fixture requires the original fresh-install policy"
        );
        let identity = store.identity();
        let bootstrap = synthetic_secret("bootstrap");
        let settlement = synthetic_secret("settlement");
        let password = synthetic_secret("owner-password");
        identity
            .initialize_credentials(&bootstrap, &settlement)
            .await
            .unwrap();
        let (organization, issued) = identity
            .bootstrap(
                &bootstrap,
                "team-import-owner",
                "Synthetic import owner",
                &password,
                "Synthetic import organization",
            )
            .await
            .unwrap();
        let owner = identity.authenticate_session(&issued.token).await.unwrap();
        let project = store
            .create_tenant_project(
                &owner,
                organization.id,
                CreateTenantProject {
                    name: "Synthetic current target".into(),
                    idempotency_key: "team-import-target".into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(project.organization_id, organization.id);
        assert_eq!(project.revision, 1);
        let platform_quota = identity
            .configure_quota(&owner, QuotaScope::Platform, 0, amounts(STORAGE_LIMIT))
            .await
            .unwrap();
        let organization_quota = identity
            .configure_quota(
                &owner,
                QuotaScope::Organization(organization.id),
                0,
                amounts(organization_storage),
            )
            .await
            .unwrap();
        assert_eq!(platform_quota.consumed, QuotaAmounts::default());
        assert_eq!(organization_quota.consumed, QuotaAmounts::default());
        Self {
            directory,
            store,
            pool,
            owner,
            team: organization.id,
            project,
            platform_quota,
            organization_quota,
        }
    }

    fn mapping(&self, membership_revision: i64) -> V1MigrationMapping {
        V1MigrationMapping {
            users: vec![V1UserMapping {
                source_user_id: SOURCE_USER.into(),
                target: V1IdentityTargetPlan::New {
                    username: "synthetic-imported-member".into(),
                    display_name: "Synthetic imported member".into(),
                },
                role: OrganizationRole::Member,
                expected_membership_revision: membership_revision,
                project_grants: vec![],
            }],
            project_destinations: vec![V1ProjectDestination {
                source_project_id: SOURCE_PROJECT.into(),
                project_id: self.project.id,
            }],
            selected_object_ids: vec!["knowledge:source-knowledge".into()],
            object_selection_complete: true,
            provider: None,
        }
    }

    fn content() -> V1ImportContent {
        V1ImportContent {
            format: "localai-cowork-v1-user-content/1".into(),
            source_sha256: hash(SOURCE_BYTES),
            source_profile: "owned-synthetic-team-import-fixture".into(),
            source_root_identity: None,
            source_entities: vec![],
            missing_attachments: vec![],
            projects: vec![V1ProjectContent {
                source_id: SOURCE_PROJECT.into(),
                name: "Synthetic source project".into(),
                orders: vec![V1OrderContent {
                    source_id: "source-order".into(),
                    source_kind: "order".into(),
                    title: "Synthetic historical order".into(),
                    draft: "Original inert draft".into(),
                    messages: vec![V1MessageContent {
                        source_id: "source-message".into(),
                        role: "user".into(),
                        content: MESSAGE.into(),
                        created_at: Some(STAMP),
                    }],
                    artifacts: vec![V1ArtifactContent {
                        source_id: "source-artifact".into(),
                        source_kind: "artifact_export".into(),
                        name: "original-artifact.txt".into(),
                        media_type: "text/plain".into(),
                        content: ARTIFACT.into(),
                        created_at: Some(STAMP),
                    }],
                    attachments: vec![V1BinaryContent {
                        source_id: "source-file".into(),
                        name: "source.txt".into(),
                        media_type: "text/plain".into(),
                        size: ORIGINAL.len() as u64,
                        sha256: hash(ORIGINAL.as_bytes()),
                        relative_path: "files/source.txt".into(),
                        capture_identity: "synthetic-owned-fixture-only".into(),
                        content_base64: STANDARD.encode(ORIGINAL.as_bytes()),
                    }],
                }],
            }],
            catalog: V1MigrationCatalog {
                users: vec![V1SourceUser {
                    source_id: SOURCE_USER.into(),
                    username: "original-source-user".into(),
                    display_name: "Original source user".into(),
                    active: true,
                    source_revision: 1,
                }],
                objects: vec![V1MigrationObject {
                    kind: "knowledge".into(),
                    source_id: "source-knowledge".into(),
                    source_project_id: Some(SOURCE_PROJECT.into()),
                    name: "Original imported knowledge".into(),
                    description: "Inert fixture".into(),
                    instructions: KNOWLEDGE.into(),
                    source_revision: 1,
                    versions: vec![],
                    workflow: None,
                    schedule_expression: None,
                    timezone: None,
                    base_url: None,
                    model: None,
                    paused: true,
                }],
                ..V1MigrationCatalog::default()
            },
        }
    }

    fn secrets() -> V1MigrationSecrets {
        V1MigrationSecrets {
            passwords: vec![V1FreshPassword {
                source_user_id: SOURCE_USER.into(),
                password: synthetic_secret("new-member-password"),
            }],
            provider_api_key: None,
        }
    }

    async fn previews(&self, membership_revision: i64) -> (V1ImportPreview, V1TeamImportPreview) {
        let source = self.store.preview_v1_import(Self::content()).await.unwrap();
        let preview = self
            .store
            .preview_v1_team_import_mapped(
                &self.owner,
                self.team,
                self.project.id,
                &source.id,
                &source.plan_sha256,
                self.mapping(membership_revision),
            )
            .await
            .unwrap();
        assert_eq!(source.state, "preview");
        assert_eq!(preview.state, "preview");
        assert_eq!(preview.source_preview_id, source.id);
        assert_eq!(preview.source_plan_sha256, source.plan_sha256);
        assert_eq!(preview.creator_id, self.owner.identity_id());
        assert_eq!(preview.team_id, self.team);
        assert_eq!(preview.project_id, self.project.id);
        assert_eq!(preview.project_revision, self.project.revision);
        assert_eq!(preview.profile_revision, 0);
        assert_eq!(preview.project_pins.len(), 1);
        assert_eq!(preview.project_pins[0].project_id, self.project.id);
        assert_eq!(
            preview.project_pins[0].project_revision,
            self.project.revision
        );
        assert_eq!(preview.project_pins[0].profile_revision, 0);
        assert_eq!(preview.orders.len(), 1);
        assert_eq!(preview.configurations.len(), 1);
        assert!(preview.configurations[0].document_id.is_none());
        assert!(preview.identity_bindings.is_empty());
        assert!(preview.provider_binding.is_none());
        let original = &source.projects[0].orders[0].attachments[0];
        let copied = &preview.orders[0].attachments[0];
        assert_ne!(original.id, copied.id);
        let mut expected = original.clone();
        expected.id = copied.id.clone();
        assert_eq!(value(&expected), value(copied));
        assert_eq!(preview.attachments_count, 1);
        assert_eq!(preview.attachment_bytes, ORIGINAL.len() as u64);
        (source, preview)
    }

    async fn commit(
        &self,
        preview: &V1TeamImportPreview,
    ) -> cowork_domain::Result<V1TeamImportPreview> {
        self.store
            .commit_v1_team_import_complete(
                &self.owner,
                self.team,
                self.project.id,
                &preview.id,
                &preview.plan_sha256,
                None,
                Self::secrets(),
            )
            .await
    }

    async fn state(&self) -> State {
        let mut result = BTreeMap::new();
        for table in TABLES {
            let columns = sqlx::query(&format!("PRAGMA table_info(\"{table}\")"))
                .fetch_all(&self.pool)
                .await
                .unwrap();
            assert!(!columns.is_empty(), "Missing fixture table {table}");
            let projection = columns
                .iter()
                .map(|column| {
                    let name: String = column.try_get("name").unwrap();
                    format!("quote(\"{}\")", name.replace('"', "\"\""))
                })
                .collect::<Vec<_>>()
                .join(",");
            let rows = sqlx::query(&format!(
                "SELECT {projection} FROM \"{table}\" ORDER BY rowid",
            ))
            .fetch_all(&self.pool)
            .await
            .unwrap();
            let values = rows
                .iter()
                .map(|row| {
                    (0..columns.len())
                        .map(|index| row.try_get::<String, _>(index).unwrap())
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            result.insert(table.to_owned(), values);
        }
        result
    }

    async fn quota(&self, scope: QuotaScope) -> QuotaStatus {
        let key = match scope {
            QuotaScope::Platform => "platform".to_owned(),
            QuotaScope::Organization(id) => id.to_string(),
        };
        let row = sqlx::query(
            "SELECT revision,limit_payload,held_payload,consumed_payload FROM identity_quotas WHERE scope=$1",
        ).bind(key).fetch_one(&self.pool).await.unwrap();
        let read = |column: &str| -> QuotaAmounts {
            serde_json::from_str(&row.try_get::<String, _>(column).unwrap()).unwrap()
        };
        QuotaStatus {
            scope,
            revision: row.try_get("revision").unwrap(),
            limit: read("limit_payload"),
            held: read("held_payload"),
            consumed: read("consumed_payload"),
        }
    }

    async fn assert_uncommitted(&self, preview: &V1TeamImportPreview) {
        let receipts = self
            .store
            .v1_team_import_receipts(&self.owner, self.team, self.project.id)
            .await
            .unwrap();
        assert_eq!(receipts.len(), 1);
        assert_eq!(value(&receipts[0]), value(preview));
        not_found(
            self.store
                .v1_team_imported_history(
                    &self.owner,
                    self.team,
                    self.project.id,
                    preview.orders[0].order.id,
                )
                .await,
        );
        not_found(
            self.store
                .v1_team_imported_attachment(
                    &self.owner,
                    self.team,
                    self.project.id,
                    &preview.orders[0].attachments[0].id,
                )
                .await,
        );
        assert!(
            self.store
                .v1_team_imported_configurations(&self.owner, self.team, self.project.id,)
                .await
                .unwrap()
                .is_empty()
        );
        let state = self.state().await;
        for table in [
            "team_orders",
            "v1_team_import_history",
            "v1_migration_configurations",
            "team_documents",
            "knowledge_revisions",
            "identity_storage_charges",
        ] {
            assert!(
                state[table].is_empty(),
                "Unexpected pre-commit effect in {table}"
            );
        }
    }

    async fn assert_committed_content(&self, actor: &Principal, committed: &V1TeamImportPreview) {
        assert_eq!(committed.state, "committed");
        assert_eq!(committed.team_id, self.team);
        assert_eq!(committed.project_id, self.project.id);
        let receipts = self
            .store
            .v1_team_import_receipts(actor, self.team, self.project.id)
            .await
            .unwrap();
        assert_eq!(receipts.len(), 1);
        assert_eq!(value(&receipts[0]), value(committed));
        let order = &committed.orders[0].order;
        assert_eq!(order.team_id, self.team);
        assert_eq!(order.project_id, self.project.id);
        assert_eq!(order.revision, 1);
        assert!(!order.archived);
        assert_eq!(order.prompt, "Original inert draft");
        let history = self
            .store
            .v1_team_imported_history(actor, self.team, self.project.id, order.id)
            .await
            .unwrap();
        assert!(history.inert);
        assert_eq!(history.import_receipt_id, committed.id);
        assert_eq!(history.project_id, self.project.id);
        assert_eq!(history.order_id, order.id);
        assert_eq!(history.source_project_id, SOURCE_PROJECT);
        assert_eq!(history.messages.len(), 1);
        assert_eq!(
            history.messages[0].source_id.as_deref(),
            Some("source-message")
        );
        assert_eq!(history.messages[0].role, "user");
        assert_eq!(history.messages[0].content, MESSAGE);
        assert_eq!(history.messages[0].created_at, STAMP);
        assert_eq!(history.artifacts.len(), 1);
        assert_eq!(
            history.artifacts[0].source_id.as_deref(),
            Some("source-artifact")
        );
        assert_eq!(history.artifacts[0].content, ARTIFACT);
        assert_eq!(history.artifacts[0].media_type, "text/plain");
        assert_eq!(history.artifacts[0].size, ARTIFACT.len() as u64);
        assert_eq!(history.artifacts[0].sha256, hash(ARTIFACT.as_bytes()));
        assert_eq!(history.artifacts[0].created_at, STAMP);
        assert_eq!(history.attachments.len(), 1);
        let (metadata, bytes) = self
            .store
            .v1_team_imported_attachment(
                actor,
                self.team,
                self.project.id,
                &committed.orders[0].attachments[0].id,
            )
            .await
            .unwrap();
        assert_eq!(value(&metadata), value(&committed.orders[0].attachments[0]));
        assert_eq!(value(&history.attachments[0]), value(&metadata));
        assert_eq!(bytes, ORIGINAL.as_bytes());
        assert_eq!(metadata.size, ORIGINAL.len() as u64);
        assert_eq!(metadata.sha256, hash(ORIGINAL.as_bytes()));
        assert_eq!(metadata.source_id, "source-file");
        assert_eq!(metadata.relative_path, "files/source.txt");
        assert_eq!(metadata.capture_identity, "synthetic-owned-fixture-only");
        let configurations = self
            .store
            .v1_team_imported_configurations(actor, self.team, self.project.id)
            .await
            .unwrap();
        assert_eq!(value(&configurations), value(&committed.configurations));
        assert_eq!(configurations.len(), 1);
        let configuration = &configurations[0];
        assert_eq!(configuration.project_id, self.project.id.to_string());
        assert_eq!(configuration.source.source_id, "source-knowledge");
        assert_eq!(configuration.source.instructions, KNOWLEDGE);
        let document_id = configuration.document_id.as_ref().unwrap();
        let raw: String = sqlx::query_scalar(
            "SELECT payload FROM team_documents WHERE id=$1 AND team_id=$2 AND project_id=$3",
        )
        .bind(document_id)
        .bind(self.team.to_string())
        .bind(self.project.id.to_string())
        .fetch_one(&self.pool)
        .await
        .unwrap();
        let document: TeamDocument = serde_json::from_str(&raw).unwrap();
        assert_eq!(document.id.to_string(), *document_id);
        assert_eq!(document.team_id, self.team);
        assert_eq!(document.project_id, self.project.id);
        assert_eq!(document.kind, TeamDocumentKind::Knowledge);
        assert_eq!(document.content, KNOWLEDGE);
        assert_eq!(document.revision, 1);
        assert!(!document.archived);
        let raw: String = sqlx::query_scalar(
            "SELECT payload FROM knowledge_revisions WHERE scope=$1 AND document_id=$2 AND revision=1",
        ).bind(format!("team:{}:{}", self.team, self.project.id)).bind(document_id)
            .fetch_one(&self.pool).await.unwrap();
        let revision: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(revision["document"], value(&document));
        assert_eq!(revision["content_sha256"], hash(KNOWLEDGE.as_bytes()));
        assert_eq!(revision["provenance"], value(&document.provenance));
        assert_eq!(committed.identity_bindings.len(), 1);
        let binding = &committed.identity_bindings[0];
        assert_eq!(binding.source_user_id, SOURCE_USER);
        assert_eq!(binding.identity.username, "synthetic-imported-member");
        assert!(!binding.identity.platform_admin);
        assert!(binding.identity.active);
        assert_eq!(binding.membership.organization_id, self.team);
        assert_eq!(binding.membership.identity_id, binding.identity.id);
        assert_eq!(binding.membership.role, OrganizationRole::Member);
        assert_eq!(binding.membership.revision, 1);
        assert!(binding.membership.active);
        assert!(binding.grants.is_empty());
        assert!(committed.provider_binding.is_none());
    }

    async fn assert_single_charge(&self, committed: &V1TeamImportPreview) -> StorageImportCharge {
        let rows = sqlx::query(
            "SELECT id,organization_id,identity_id,resource_id,intent_key,bytes,provenance_sha256,request_sha256,created_at,receipt FROM identity_storage_charges",
        ).fetch_all(&self.pool).await.unwrap();
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        let charge: StorageImportCharge =
            serde_json::from_str(&row.try_get::<String, _>("receipt").unwrap()).unwrap();
        assert_eq!(
            charge.id.to_string(),
            row.try_get::<String, _>("id").unwrap()
        );
        assert_eq!(charge.organization_id, self.team);
        assert_eq!(
            charge.organization_id.to_string(),
            row.try_get::<String, _>("organization_id").unwrap()
        );
        assert_eq!(charge.identity_id, self.owner.identity_id());
        assert_eq!(
            charge.identity_id.to_string(),
            row.try_get::<String, _>("identity_id").unwrap()
        );
        assert_eq!(charge.resource_id, self.project.id);
        assert_eq!(
            charge.resource_id.to_string(),
            row.try_get::<String, _>("resource_id").unwrap()
        );
        assert_eq!(charge.key, format!("v1-import:{}", committed.id));
        assert_eq!(charge.key, row.try_get::<String, _>("intent_key").unwrap());
        assert!(charge.bytes > 0);
        assert_eq!(charge.bytes, row.try_get::<i64, _>("bytes").unwrap());
        assert_eq!(charge.provenance_sha256, committed.plan_sha256);
        assert_eq!(
            charge.provenance_sha256,
            row.try_get::<String, _>("provenance_sha256").unwrap()
        );
        assert_eq!(charge.request_sha256.len(), 64);
        assert_eq!(
            charge.request_sha256,
            row.try_get::<String, _>("request_sha256").unwrap()
        );
        assert_eq!(
            charge.created_at,
            row.try_get::<i64, _>("created_at").unwrap()
        );
        assert!(charge.created_at > 0);
        for original in [&self.platform_quota, &self.organization_quota] {
            let actual = self.quota(original.scope).await;
            assert_eq!(actual.revision, original.revision + 1);
            assert_eq!(actual.held, QuotaAmounts::default());
            assert_eq!(actual.consumed, amounts(charge.bytes));
        }
        let state = self.state().await;
        assert_eq!(state["team_orders"].len(), 1);
        assert_eq!(state["v1_team_import_history"].len(), 1);
        assert_eq!(state["v1_migration_configurations"].len(), 1);
        assert_eq!(state["team_documents"].len(), 1);
        assert_eq!(state["knowledge_revisions"].len(), 1);
        assert_eq!(state["identity_subjects"].len(), 2);
        assert_eq!(state["identity_memberships"].len(), 2);
        charge
    }

    async fn close(self) {
        self.store.close().await;
        self.pool.close().await;
        drop(self.store);
        drop(self.pool);
        self.directory.close().unwrap();
    }
}

#[tokio::test]
async fn committed_team_import_keeps_original_content_and_replays_at_exhausted_quota() {
    let f = Fixture::new(STORAGE_LIMIT).await;
    let (source, preview) = f.previews(0).await;
    f.assert_uncommitted(&preview).await;
    let original_source_rows = f.state().await;
    let committed = f.commit(&preview).await.unwrap();
    assert_eq!(committed.id, preview.id);
    assert_eq!(committed.plan_sha256, preview.plan_sha256);
    f.assert_committed_content(&f.owner, &committed).await;
    let charge = f.assert_single_charge(&committed).await;
    let after = f.state().await;
    assert_eq!(
        after["v1_import_receipts"],
        original_source_rows["v1_import_receipts"]
    );
    let original_id = &source.projects[0].orders[0].attachments[0].id;
    let row = sqlx::query(
        "SELECT metadata,content_base64,state FROM v1_import_attachments WHERE id=$1 AND preview_id=$2 AND scope='personal'",
    ).bind(original_id).bind(&source.id).fetch_one(&f.pool).await.unwrap();
    let metadata: V1ImportedAttachment =
        serde_json::from_str(&row.try_get::<String, _>("metadata").unwrap()).unwrap();
    assert_eq!(
        value(&metadata),
        value(&source.projects[0].orders[0].attachments[0])
    );
    assert_eq!(row.try_get::<String, _>("state").unwrap(), "preview");
    assert_eq!(
        STANDARD
            .decode(row.try_get::<String, _>("content_base64").unwrap())
            .unwrap(),
        ORIGINAL.as_bytes()
    );
    for scope in [QuotaScope::Platform, QuotaScope::Organization(f.team)] {
        let current = f.quota(scope).await;
        assert_eq!(current.consumed.storage_bytes, charge.bytes);
        let mut limit = current.limit;
        limit.storage_bytes = current.consumed.storage_bytes;
        let exact = f
            .store
            .identity()
            .configure_quota(&f.owner, scope, current.revision, limit)
            .await
            .unwrap();
        assert_eq!(exact.limit.storage_bytes, exact.consumed.storage_bytes);
        assert_eq!(exact.held, QuotaAmounts::default());
    }
    // Snapshot after the intentional public quota changes. No reauthentication
    // or other public setup mutation is performed within this comparison.
    let before_replay = f.state().await;
    let replay = f
        .store
        .commit_v1_team_import_complete(
            &f.owner,
            f.team,
            f.project.id,
            &preview.id,
            &preview.plan_sha256,
            None,
            V1MigrationSecrets::default(),
        )
        .await
        .unwrap();
    assert_eq!(value(&replay), value(&committed));
    assert_eq!(f.state().await, before_replay);
    let wrong_plan = hash(b"A different unapproved plan");
    assert_ne!(wrong_plan, preview.plan_sha256);
    conflict(
        f.store
            .commit_v1_team_import_complete(
                &f.owner,
                f.team,
                f.project.id,
                &preview.id,
                &wrong_plan,
                None,
                V1MigrationSecrets::default(),
            )
            .await,
    );
    assert_eq!(f.state().await, before_replay);
    f.close().await;
}

#[tokio::test]
async fn late_organization_quota_failure_rolls_back_then_same_original_preview_recovers() {
    let mut f = Fixture::new(0).await;
    let (_, preview) = f.previews(0).await;
    f.assert_uncommitted(&preview).await;
    let before = f.state().await;
    quota_exceeded(f.commit(&preview).await);
    // Includes original epoch/audit, both quotas, no retained Identity mapping,
    // products, configurations, document/provenance or storage charge.
    assert_eq!(f.state().await, before);
    assert_eq!(f.quota(QuotaScope::Platform).await, f.platform_quota);
    assert_eq!(
        f.quota(QuotaScope::Organization(f.team)).await,
        f.organization_quota
    );
    f.assert_uncommitted(&preview).await;
    assert_eq!(f.state().await, before);
    // Only the original organization quota is changed, using its unchanged CAS.
    // No recapture, re-preview, replacement receipt, or different plan/intent.
    let original_revision = f.organization_quota.revision;
    f.organization_quota = f
        .store
        .identity()
        .configure_quota(
            &f.owner,
            QuotaScope::Organization(f.team),
            original_revision,
            amounts(STORAGE_LIMIT),
        )
        .await
        .unwrap();
    assert_eq!(f.organization_quota.revision, original_revision + 1);
    let committed = f.commit(&preview).await.unwrap();
    assert_eq!(committed.id, preview.id);
    assert_eq!(committed.plan_sha256, preview.plan_sha256);
    assert_eq!(committed.source_preview_id, preview.source_preview_id);
    assert_eq!(committed.orders[0].order.id, preview.orders[0].order.id);
    assert_eq!(
        committed.orders[0].attachments[0].id,
        preview.orders[0].attachments[0].id
    );
    f.assert_committed_content(&f.owner, &committed).await;
    f.assert_single_charge(&committed).await;
    f.close().await;
}

#[tokio::test]
async fn late_new_member_cas_conflict_preserves_every_original_preview_row() {
    let f = Fixture::new(STORAGE_LIMIT).await;
    let (_, preview) = f.previews(1).await;
    f.assert_uncommitted(&preview).await;
    let before = f.state().await;
    conflict(f.commit(&preview).await);
    assert_eq!(f.state().await, before);
    assert_eq!(f.quota(QuotaScope::Platform).await, f.platform_quota);
    assert_eq!(
        f.quota(QuotaScope::Organization(f.team)).await,
        f.organization_quota
    );
    f.assert_uncommitted(&preview).await;
    assert_eq!(f.state().await, before);
    f.close().await;
}

#[tokio::test]
async fn independent_real_viewer_reads_committed_import_but_cannot_preview_or_commit() {
    let f = Fixture::new(STORAGE_LIMIT).await;
    let (source, preview) = f.previews(0).await;
    let committed = f.commit(&preview).await.unwrap();
    f.assert_single_charge(&committed).await;
    let identity = f.store.identity();
    let password = synthetic_secret("viewer-password");
    let subject = identity
        .register_local_identity(
            "synthetic-import-viewer",
            "Synthetic independent viewer",
            &password,
        )
        .await
        .unwrap();
    let issued = identity
        .login(&subject.username, &password, 3_600_000)
        .await
        .unwrap();
    let viewer = identity.authenticate_session(&issued.token).await.unwrap();
    assert_ne!(viewer.identity_id(), f.owner.identity_id());
    let member = identity
        .add_member(&f.owner, f.team, subject.id, OrganizationRole::Viewer)
        .await
        .unwrap();
    identity
        .grant_content(
            &f.owner,
            f.team,
            f.project.id,
            member.id,
            0,
            Permissions::read_only(),
            true,
        )
        .await
        .unwrap();
    f.assert_committed_content(&viewer, &committed).await;
    let before = f.state().await;
    forbidden(
        f.store
            .preview_v1_team_import_mapped(
                &viewer,
                f.team,
                f.project.id,
                &source.id,
                &source.plan_sha256,
                f.mapping(0),
            )
            .await,
    );
    assert_eq!(f.state().await, before);
    forbidden(
        f.store
            .commit_v1_team_import_complete(
                &viewer,
                f.team,
                f.project.id,
                &preview.id,
                &preview.plan_sha256,
                None,
                V1MigrationSecrets::default(),
            )
            .await,
    );
    assert_eq!(f.state().await, before);
    f.assert_committed_content(&f.owner, &committed).await;
    assert_eq!(f.state().await, before);
    drop(identity);
    f.close().await;
}
