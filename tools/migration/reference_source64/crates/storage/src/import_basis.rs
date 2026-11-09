//! Explicitly selected legacy content becomes fresh projects/orders only. No
//! historical RunSpec, token, ACL, approval, budget, schedule or replay is adopted.
use super::*;
use serde::{Deserialize, Serialize};
#[path = "import_attachments.rs"]
mod attachments;
pub use attachments::{V1BinaryContent, V1ImportedAttachment};
#[path = "import_team.rs"]
mod team_import;
pub use team_import::{V1TeamImportPreview, V1TeamImportedHistory, V1TeamImportedOrder};
#[path = "import_catalog.rs"]
mod catalog;
pub use catalog::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct V1SourceEntity {
    pub kind: String,
    pub source_id: String,
    pub name: String,
    pub source_project_id: Option<String>,
    pub source_actor_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct V1MissingAttachment {
    pub source_id: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct V1ImportContent {
    pub format: String,
    pub source_sha256: String,
    pub source_profile: String,
    pub projects: Vec<V1ProjectContent>,
    #[serde(default)]
    pub source_root_identity: Option<String>,
    #[serde(default)]
    pub source_entities: Vec<V1SourceEntity>,
    #[serde(default)]
    pub missing_attachments: Vec<V1MissingAttachment>,
    #[serde(default, skip_serializing_if = "V1MigrationCatalog::is_empty")]
    pub catalog: V1MigrationCatalog,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct V1ProjectContent {
    pub source_id: String,
    pub name: String,
    pub orders: Vec<V1OrderContent>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct V1OrderContent {
    pub source_id: String,
    #[serde(default = "order_kind")]
    pub source_kind: String,
    pub title: String,
    pub draft: String,
    #[serde(default)]
    pub messages: Vec<V1MessageContent>,
    #[serde(default)]
    pub artifacts: Vec<V1ArtifactContent>,
    #[serde(default)]
    pub attachments: Vec<V1BinaryContent>,
}
fn order_kind() -> String {
    "order".into()
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct V1MessageContent {
    pub source_id: String,
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub created_at: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct V1ArtifactContent {
    pub source_id: String,
    pub source_kind: String,
    pub name: String,
    pub media_type: String,
    pub content: String,
    #[serde(default)]
    pub created_at: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct V1ImportedProject {
    pub source_id: String,
    pub project: Project,
    pub orders: Vec<V1ImportedOrder>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct V1ImportedOrder {
    pub source_id: String,
    pub source_kind: String,
    pub order: Order,
    pub messages: Vec<V1MessageContent>,
    #[serde(default)]
    pub artifacts: Vec<V1ArtifactContent>,
    #[serde(default)]
    pub attachments: Vec<V1ImportedAttachment>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct V1ImportPreview {
    pub id: String,
    pub source_sha256: String,
    pub source_profile: String,
    pub plan_sha256: String,
    pub state: String,
    pub projects: Vec<V1ImportedProject>,
    pub content_bytes: u64,
    #[serde(default)]
    pub source_root_identity: Option<String>,
    #[serde(default)]
    pub attachments_count: u64,
    #[serde(default)]
    pub attachment_bytes: u64,
    #[serde(default)]
    pub source_entities: Vec<V1SourceEntity>,
    #[serde(default)]
    pub missing_attachments: Vec<V1MissingAttachment>,
    #[serde(default, skip_serializing_if = "V1MigrationCatalog::is_empty")]
    pub catalog: V1MigrationCatalog,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub configurations: Vec<V1ImportedConfiguration>,
}

pub async fn migrate(tx: &mut sqlx::Transaction<'_, sqlx::Any>) -> Result<()> {
    sqlx::query("CREATE TABLE IF NOT EXISTS v1_import_receipts (id TEXT PRIMARY KEY, source_sha256 TEXT NOT NULL, plan_sha256 TEXT NOT NULL, state TEXT NOT NULL, payload TEXT NOT NULL)")
        .execute(&mut **tx).await.map_err(storage_error)?;
    for statement in attachments::SCHEMA {
        sqlx::query(statement)
            .execute(&mut **tx)
            .await
            .map_err(storage_error)?;
    }
    for statement in catalog::SCHEMA {
        sqlx::query(statement)
            .execute(&mut **tx)
            .await
            .map_err(storage_error)?;
    }
    Ok(())
}
fn plan_hash(value: &V1ImportPreview) -> Result<String> {
    let mut copy = value.clone();
    copy.plan_sha256.clear();
    copy.state = "preview".into();
    for c in &mut copy.configurations {
        c.document_id = None;
    }
    Ok(hash(json_string(&copy)?.as_bytes()))
}
impl Store {
    pub async fn preview_v1_import(&self, input: V1ImportContent) -> Result<V1ImportPreview> {
        input.catalog.validate()?;
        if input.format != "localai-cowork-v1-user-content/1"
            || !cowork_domain::processes::valid_sha256(&input.source_sha256)
            || input.projects.is_empty()
            || input.projects.len() > 100
        {
            return Err(Error::Validation(
                "Unsupported or oversized V1 content import".into(),
            ));
        }
        valid_text(&input.source_profile, 1024)?;
        input.catalog.validate()?;
        let source_projects = input
            .projects
            .iter()
            .map(|p| p.source_id.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        if input.catalog.objects.iter().any(|o| {
            o.source_project_id
                .as_deref()
                .is_some_and(|p| !source_projects.contains(p))
        }) || input
            .catalog
            .project_acl
            .iter()
            .any(|a| !source_projects.contains(a.source_project_id.as_str()))
        {
            return Err(Error::Validation(
                "Unknown original project in migration catalog".into(),
            ));
        }
        drop(source_projects);
        if let Some(root) = &input.source_root_identity {
            valid_text(root, 256)?;
        }
        let mut original = std::collections::HashSet::new();
        let mut original_messages = std::collections::HashSet::new();
        let mut original_artifacts = std::collections::HashSet::new();
        let mut projects = Vec::new();
        let mut bytes = 0u64;
        let mut order_count = 0;
        let mut message_count = 0;
        let mut artifact_count = 0;
        let mut copied = Vec::new();
        let mut attachment_bytes = 0u64;
        let mut source_entities = std::collections::HashSet::new();
        if input.source_entities.len() > 10000 || input.missing_attachments.len() > 1000 {
            return Err(Error::Validation("Oversized legacy mapping".into()));
        }
        for entity in &input.source_entities {
            valid_text(&entity.source_id, 256)?;
            valid_text(&entity.name, 300)?;
            if !matches!(
                entity.kind.as_str(),
                "project" | "tenant" | "actor" | "persona" | "daemon_run"
            ) || !source_entities.insert((entity.kind.clone(), entity.source_id.clone()))
            {
                return Err(Error::Validation(
                    "Invalid or duplicate inert entity mapping".into(),
                ));
            }
            if let Some(v) = &entity.source_project_id {
                valid_text(v, 256)?;
            }
            if let Some(v) = &entity.source_actor_id {
                valid_text(v, 256)?;
            }
        }
        for missing in &input.missing_attachments {
            valid_text(&missing.source_id, 300)?;
            valid_text(&missing.reason, 100)?;
        }
        for source in input.projects {
            valid_text(&source.source_id, 256)?;
            valid_text(&source.name, 200)?;
            if !original.insert(format!("project:{}", source.source_id)) {
                return Err(Error::Validation("Duplicate original project ID".into()));
            }
            let project = Project {
                id: id(),
                name: source.name,
                revision: 1,
                created_at: now(),
            };
            let mut orders = Vec::new();
            for content in source.orders {
                valid_text(&content.source_id, 256)?;
                valid_text(&content.source_kind, 64)?;
                valid_text(&content.title, 300)?;
                if !matches!(
                    content.source_kind.as_str(),
                    "order"
                        | "chat_thread"
                        | "tasks"
                        | "work_tasks"
                        | "project_instructions"
                        | "artifact_history"
                        | "daemon_run"
                        | "daemon_task"
                        | "task_definition"
                        | "scheduled_task"
                        | "crew_run"
                        | "project_resources"
                        | "project_version"
                ) {
                    return Err(Error::Validation("Unknown content source kind".into()));
                }
                if content.draft.len() > 128 * 1024
                    || !original.insert(format!(
                        "order:{}:{}",
                        content.source_kind, content.source_id
                    ))
                {
                    return Err(Error::Validation(
                        "Oversized order or conflicting original order mapping".into(),
                    ));
                }
                for message in &content.messages {
                    valid_text(&message.source_id, 200)?;
                    valid_text(&message.role, 64)?;
                    if !matches!(
                        message.role.as_str(),
                        "system" | "user" | "assistant" | "tool"
                    ) || message.created_at.is_some_and(|stamp| stamp < 0)
                        || message.content.contains('\0')
                        || message.content.len() > 128 * 1024
                        || !original_messages.insert(message.source_id.clone())
                    {
                        return Err(Error::Validation(
                            "Duplicate original message ID or oversized content".into(),
                        ));
                    }
                    message_count += 1;
                    bytes = bytes.saturating_add(message.content.len() as u64);
                }
                for artifact in &content.artifacts {
                    valid_text(&artifact.source_id, 200)?;
                    valid_text(&artifact.name, 300)?;
                    valid_text(&artifact.media_type, 120)?;
                    if !matches!(
                        artifact.source_kind.as_str(),
                        "json_artifact"
                            | "artifact_versions"
                            | "engine_run_artifacts"
                            | "daemon_artifact_event"
                            | "artifact_export"
                            | "project_resource"
                            | "server_artifact"
                            | "server_version_file"
                            | "chat_attachment"
                    ) || artifact.media_type != "text/plain"
                        || artifact.created_at.is_some_and(|stamp| stamp < 0)
                        || artifact.content.contains('\0')
                        || artifact.content.len() > 1024 * 1024
                        || !original_artifacts
                            .insert((artifact.source_kind.clone(), artifact.source_id.clone()))
                    {
                        return Err(Error::Validation(
                            "Invalid, conflicting or oversized historical artifact text".into(),
                        ));
                    }
                    artifact_count += 1;
                    bytes = bytes.saturating_add(artifact.content.len() as u64);
                }
                bytes = bytes.saturating_add(content.draft.len() as u64);
                order_count += 1;
                if bytes > 16 * 1024 * 1024
                    || order_count > 2000
                    || message_count > 10000
                    || artifact_count > 1000
                {
                    return Err(Error::Validation("V1 content import limit exceeded".into()));
                }
                let order = Order {
                    id: id(),
                    project_id: project.id.clone(),
                    title: content.title,
                    draft: content.draft,
                    revision: 1,
                    created_at: now(),
                };
                let mut binary = Vec::new();
                for value in content.attachments {
                    let (metadata, encoded) = attachments::prepare(value)?;
                    attachment_bytes = attachment_bytes
                        .checked_add(metadata.size)
                        .ok_or(Error::Conflict)?;
                    if copied.len() >= 1000 || attachment_bytes > 32 * 1024 * 1024 {
                        return Err(Error::Validation("Copied attachment limit exceeded".into()));
                    }
                    copied.push((
                        order.id.clone(),
                        project.id.clone(),
                        metadata.clone(),
                        encoded,
                    ));
                    binary.push(metadata);
                }
                orders.push(V1ImportedOrder {
                    source_id: content.source_id,
                    source_kind: content.source_kind,
                    order,
                    messages: content.messages,
                    artifacts: content.artifacts,
                    attachments: binary,
                });
            }
            projects.push(V1ImportedProject {
                source_id: source.source_id,
                project,
                orders,
            });
        }
        let destinations = projects
            .iter()
            .map(|p: &V1ImportedProject| (p.source_id.clone(), p.project.id.clone()))
            .collect::<Vec<_>>();
        let configurations =
            catalog::configurations(&input.catalog, &destinations, &projects[0].project.id, &[])?;
        let mut preview = V1ImportPreview {
            id: id(),
            source_sha256: input.source_sha256,
            source_profile: input.source_profile,
            plan_sha256: String::new(),
            state: "preview".into(),
            projects,
            content_bytes: bytes,
            source_root_identity: input.source_root_identity,
            attachments_count: copied.len() as u64,
            attachment_bytes,
            source_entities: input.source_entities,
            missing_attachments: input.missing_attachments,
            catalog: input.catalog,
            configurations,
        };
        preview.plan_sha256 = plan_hash(&preview)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        sqlx::query("INSERT INTO v1_import_receipts(id,source_sha256,plan_sha256,state,payload) VALUES($1,$2,$3,$4,$5)")
            .bind(&preview.id).bind(&preview.source_sha256).bind(&preview.plan_sha256).bind("preview").bind(json_string(&preview)?)
            .execute(&mut *tx).await.map_err(storage_error)?;
        for (order, project, metadata, encoded) in copied {
            attachments::insert(
                &mut tx,
                &preview.id,
                &order,
                &project,
                "personal",
                None,
                &metadata,
                &encoded,
            )
            .await?;
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(preview)
    }
    pub async fn commit_v1_import(
        &self,
        preview_id: &str,
        approved_plan_sha256: &str,
    ) -> Result<V1ImportPreview> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let lock = if self.sqlite {
            "UPDATE v1_import_receipts SET state=state WHERE id=$1"
        } else {
            "SELECT id FROM v1_import_receipts WHERE id=$1 FOR UPDATE"
        };
        sqlx::query(lock)
            .bind(preview_id)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        let row =
            sqlx::query("SELECT state,plan_sha256,payload FROM v1_import_receipts WHERE id=$1")
                .bind(preview_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?
                .ok_or(Error::NotFound)?;
        let mut preview: V1ImportPreview =
            parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        if approved_plan_sha256 != preview.plan_sha256
            || row
                .try_get::<String, _>("plan_sha256")
                .map_err(storage_error)?
                != preview.plan_sha256
            || plan_hash(&preview)? != preview.plan_sha256
        {
            return Err(Error::Conflict);
        }
        let state: String = row.try_get("state").map_err(storage_error)?;
        if state == "committed" {
            if preview.state != state {
                return Err(Error::Conflict);
            }
            tx.commit().await.map_err(storage_error)?;
            return Ok(preview);
        }
        if state != "preview" || preview.state != "preview" {
            return Err(Error::Conflict);
        }
        let expected = preview
            .projects
            .iter()
            .flat_map(|p| {
                p.orders.iter().flat_map(|o| {
                    o.attachments
                        .iter()
                        .cloned()
                        .map(|a| (o.order.id.clone(), p.project.id.clone(), a))
                })
            })
            .collect::<Vec<_>>();
        attachments::validate_rows(&mut tx, preview_id, "personal", &expected, None).await?;
        for entry in &preview.projects {
            sqlx::query("INSERT INTO projects(id,payload) VALUES($1,$2)")
                .bind(&entry.project.id)
                .bind(json_string(&entry.project)?)
                .execute(&mut *tx)
                .await
                .map_err(storage_error)?;
            for legacy in &entry.orders {
                let order = &legacy.order;
                if order.project_id != entry.project.id {
                    return Err(Error::Conflict);
                }
                sqlx::query(
                    "INSERT INTO orders(id,project_id,revision,payload) VALUES($1,$2,$3,$4)",
                )
                .bind(&order.id)
                .bind(&order.project_id)
                .bind(order.revision)
                .bind(json_string(order)?)
                .execute(&mut *tx)
                .await
                .map_err(storage_error)?;
                history::index_document(
                    &mut tx,
                    &order.id,
                    &order.id,
                    "order",
                    &format!("{}\n{}", order.title, order.draft),
                    order.created_at,
                )
                .await?;
                if !legacy.messages.is_empty() || !legacy.artifacts.is_empty() {
                    // Original IDs remain inert provenance. Fresh IDs are only
                    // content identifiers, and this same caller TX creates no run.
                    let imported_at = now();
                    productivity_archive::insert_imported_history_in_transaction(
                        &mut tx,
                        ImportedProductHistory {
                            project_id: order.project_id.clone(),
                            order_id: order.id.clone(),
                            messages: legacy
                                .messages
                                .iter()
                                .map(|message| ArchivedProductMessage {
                                    id: id(),
                                    source_id: Some(message.source_id.clone()),
                                    role: message.role.clone(),
                                    content: message.content.clone(),
                                    created_at: message.created_at.unwrap_or(imported_at),
                                })
                                .collect(),
                            artifacts: legacy
                                .artifacts
                                .iter()
                                .map(|artifact| ArchivedProductArtifact {
                                    id: id(),
                                    source_id: Some(artifact.source_id.clone()),
                                    name: artifact.name.clone(),
                                    media_type: artifact.media_type.clone(),
                                    size: artifact.content.len() as u64,
                                    sha256: hash(artifact.content.as_bytes()),
                                    content: artifact.content.clone(),
                                    created_at: artifact.created_at.unwrap_or(imported_at),
                                })
                                .collect(),
                            revision: 1,
                            imported_at,
                        },
                    )
                    .await?;
                }
            }
        }
        catalog::insert_configurations(
            &mut tx,
            "personal",
            None,
            preview_id,
            &mut preview.configurations,
        )
        .await?;
        preview.state = "committed".into();
        sqlx::query("UPDATE v1_import_attachments SET state='committed' WHERE preview_id=$1 AND scope='personal' AND state='preview'").bind(preview_id).execute(&mut *tx).await.map_err(storage_error)?;
        sqlx::query(
            "UPDATE v1_import_receipts SET state=$1,payload=$2 WHERE id=$3 AND state='preview'",
        )
        .bind("committed")
        .bind(json_string(&preview)?)
        .bind(preview_id)
        .execute(&mut *tx)
        .await
        .map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(preview)
    }
    pub async fn v1_import_receipts(&self) -> Result<Vec<V1ImportPreview>> {
        self.all_payloads("SELECT payload FROM v1_import_receipts ORDER BY id", None)
            .await
    }
    pub async fn v1_imported_configurations(
        &self,
        project: &str,
    ) -> Result<Vec<V1ImportedConfiguration>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        crate::productivity::product_project(&mut tx, project).await?;
        let rows=sqlx::query("SELECT payload FROM v1_migration_configurations WHERE scope='personal' AND project_id=$1 ORDER BY id").bind(project).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let result = rows
            .into_iter()
            .map(|r| parse(&r.try_get::<String, _>("payload").map_err(storage_error)?))
            .collect::<Result<_>>()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
}
