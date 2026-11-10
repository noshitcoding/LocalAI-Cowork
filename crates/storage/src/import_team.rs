//! Fresh current team authority adopts content, never legacy principals or runs.
use super::*;
use crate::team_service::identity_error;
use cowork_identity::{ContentAction, LockedIdentityQuota, Principal};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct V1TeamImportedOrder {
    pub source_project_id: String,
    pub source_id: String,
    pub source_kind: String,
    pub order: TeamOrder,
    pub messages: Vec<V1MessageContent>,
    pub artifacts: Vec<V1ArtifactContent>,
    pub attachments: Vec<V1ImportedAttachment>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct V1TeamImportPreview {
    pub id: String,
    pub source_preview_id: String,
    pub source_plan_sha256: String,
    pub team_id: Uuid,
    pub project_id: Uuid,
    pub creator_id: Uuid,
    pub project_revision: i64,
    pub profile_revision: i64,
    pub plan_sha256: String,
    pub state: String,
    pub orders: Vec<V1TeamImportedOrder>,
    pub source_entities: Vec<V1SourceEntity>,
    pub missing_attachments: Vec<V1MissingAttachment>,
    pub content_bytes: u64,
    pub attachments_count: u64,
    pub attachment_bytes: u64,
    #[serde(default, skip_serializing_if = "V1MigrationCatalog::is_empty")]
    pub catalog: V1MigrationCatalog,
    #[serde(default, skip_serializing_if = "V1MigrationMapping::is_empty")]
    pub mapping: V1MigrationMapping,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub project_pins: Vec<V1ProjectPin>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub configurations: Vec<V1ImportedConfiguration>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub identity_bindings: Vec<V1IdentityBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_binding: Option<cowork_contracts::TeamProvider>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct V1TeamImportedHistory {
    pub import_receipt_id: String,
    pub project_id: Uuid,
    pub order_id: Uuid,
    pub source_project_id: String,
    pub messages: Vec<ArchivedProductMessage>,
    pub artifacts: Vec<ArchivedProductArtifact>,
    pub attachments: Vec<V1ImportedAttachment>,
    pub source_entities: Vec<V1SourceEntity>,
    pub imported_at: i64,
    pub inert: bool,
}
fn team_plan_hash(value: &V1TeamImportPreview) -> Result<String> {
    let mut copy = value.clone();
    copy.plan_sha256.clear();
    copy.state = "preview".into();
    copy.identity_bindings.clear();
    copy.provider_binding = None;
    for c in &mut copy.configurations {
        c.document_id = None;
    }
    Ok(hash(json_string(&copy)?.as_bytes()))
}
async fn project_pin(
    locked: &mut LockedIdentityQuota<'_, '_, '_>,
    team: Uuid,
    project: Uuid,
    write: bool,
) -> Result<(i64, i64)> {
    let row=sqlx::query("SELECT p.organization_id,p.revision,COALESCE(f.revision,0) AS profile_revision,COALESCE(f.archived,0) AS archived FROM tenant_projects p LEFT JOIN team_project_profiles f ON f.project_id=p.id WHERE p.id=$1")
        .bind(project.to_string()).fetch_optional(&mut **locked.transaction().map_err(identity_error)?).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
    if row
        .try_get::<String, _>("organization_id")
        .map_err(storage_error)?
        != team.to_string()
    {
        return Err(Error::Forbidden);
    }
    let archived: i64 = row.try_get("archived").map_err(storage_error)?;
    if !matches!(archived, 0 | 1) || (write && archived == 1) {
        return Err(Error::Conflict);
    }
    Ok((
        row.try_get("revision").map_err(storage_error)?,
        row.try_get("profile_revision").map_err(storage_error)?,
    ))
}
impl Store {
    /// Caller must additionally prove native source consent before calling this.
    /// The source reference is private owner capture, not tenant file authority.
    pub async fn preview_v1_team_import(
        &self,
        actor: &Principal,
        team: Uuid,
        project: Uuid,
        source_preview: &str,
        source_plan: &str,
    ) -> Result<V1TeamImportPreview> {
        self.preview_v1_team_import_mapped(
            actor,
            team,
            project,
            source_preview,
            source_plan,
            V1MigrationMapping::default(),
        )
        .await
    }
    pub async fn preview_v1_team_import_mapped(
        &self,
        actor: &Principal,
        team: Uuid,
        project: Uuid,
        source_preview: &str,
        source_plan: &str,
        mapping: V1MigrationMapping,
    ) -> Result<V1TeamImportPreview> {
        let identity = self.identity();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let preview = {
            let mut locked = identity
                .lock_quota_in_transaction(&mut tx)
                .await
                .map_err(identity_error)?;
            let permit = locked
                .authorize_content_in_transaction(actor, team, project, ContentAction::Write)
                .await
                .map_err(identity_error)?;
            let (project_revision, profile_revision) =
                project_pin(&mut locked, team, project, true).await?;
            let row = sqlx::query("SELECT plan_sha256,payload FROM v1_import_receipts WHERE id=$1")
                .bind(source_preview)
                .fetch_optional(&mut **locked.transaction().map_err(identity_error)?)
                .await
                .map_err(storage_error)?
                .ok_or(Error::NotFound)?;
            let source: V1ImportPreview =
                parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
            if source.plan_sha256 != source_plan
                || row
                    .try_get::<String, _>("plan_sha256")
                    .map_err(storage_error)?
                    != source_plan
                || plan_hash(&source)? != source_plan
            {
                return Err(Error::Conflict);
            }
            catalog::validate_mapping(&source, &mapping)?;
            let mut project_pins = vec![];
            let mut permits = vec![];
            let destinations = mapping
                .project_destinations
                .iter()
                .map(|p| (p.source_project_id.clone(), p.project_id.to_string()))
                .collect::<Vec<_>>();
            let mut target_projects = mapping
                .project_destinations
                .iter()
                .map(|p| p.project_id)
                .collect::<std::collections::BTreeSet<_>>();
            target_projects.insert(project);
            for target in target_projects {
                let p = locked
                    .authorize_content_in_transaction(actor, team, target, ContentAction::Write)
                    .await
                    .map_err(identity_error)?;
                let (revision, profile) = project_pin(&mut locked, team, target, true).await?;
                project_pins.push(V1ProjectPin {
                    project_id: target,
                    project_revision: revision,
                    profile_revision: profile,
                });
                permits.push(p);
            }
            let configurations =
                if mapping.object_selection_complete && mapping.selected_object_ids.is_empty() {
                    vec![]
                } else {
                    catalog::configurations(
                        &source.catalog,
                        &destinations,
                        &project.to_string(),
                        &mapping.selected_object_ids,
                    )?
                };
            let mut orders = Vec::new();
            let mut binaries = Vec::new();
            for legacy_project in &source.projects {
                let destination = mapping
                    .project_destinations
                    .iter()
                    .find(|p| p.source_project_id == legacy_project.source_id)
                    .map(|p| p.project_id)
                    .unwrap_or(project);
                for legacy in &legacy_project.orders {
                    valid_text(&legacy.order.title, 200)?;
                    let order = TeamOrder {
                        id: Uuid::new_v4(),
                        team_id: team,
                        project_id: destination,
                        title: legacy.order.title.clone(),
                        prompt: legacy.order.draft.clone(),
                        revision: 1,
                        archived: false,
                        created_at: now(),
                    };
                    let mut copied = Vec::new();
                    for attachment in &legacy.attachments {
                        let row=sqlx::query("SELECT metadata,content_base64 FROM v1_import_attachments WHERE id=$1 AND preview_id=$2 AND order_id=$3 AND project_id=$4 AND scope='personal'")
                        .bind(&attachment.id).bind(source_preview).bind(&legacy.order.id).bind(&legacy_project.project.id)
                        .fetch_optional(&mut **locked.transaction().map_err(identity_error)?).await.map_err(storage_error)?.ok_or(Error::Conflict)?;
                        let metadata: V1ImportedAttachment = parse(
                            &row.try_get::<String, _>("metadata")
                                .map_err(storage_error)?,
                        )?;
                        let encoded: String =
                            row.try_get("content_base64").map_err(storage_error)?;
                        if json_string(&metadata)? != json_string(attachment)? {
                            return Err(Error::Conflict);
                        }
                        attachments::decode(&metadata, &encoded)?;
                        let mut metadata = metadata;
                        metadata.id = id();
                        binaries.push((
                            order.id.to_string(),
                            destination.to_string(),
                            metadata.clone(),
                            encoded,
                        ));
                        copied.push(metadata);
                    }
                    orders.push(V1TeamImportedOrder {
                        source_project_id: legacy_project.source_id.clone(),
                        source_id: legacy.source_id.clone(),
                        source_kind: legacy.source_kind.clone(),
                        order,
                        messages: legacy.messages.clone(),
                        artifacts: legacy.artifacts.clone(),
                        attachments: copied,
                    });
                }
            }
            let mut preview = V1TeamImportPreview {
                id: id(),
                source_preview_id: source.id,
                source_plan_sha256: source.plan_sha256,
                team_id: team,
                project_id: project,
                creator_id: actor.identity_id(),
                project_revision,
                profile_revision,
                plan_sha256: String::new(),
                state: "preview".into(),
                orders,
                source_entities: source.source_entities,
                missing_attachments: source.missing_attachments,
                content_bytes: source.content_bytes,
                attachments_count: source.attachments_count,
                attachment_bytes: source.attachment_bytes,
                catalog: source.catalog,
                mapping,
                project_pins,
                configurations,
                identity_bindings: vec![],
                provider_binding: None,
            };
            preview.plan_sha256 = team_plan_hash(&preview)?;
            sqlx::query("INSERT INTO v1_team_import_receipts(id,source_preview_id,team_id,project_id,creator_id,plan_sha256,state,payload) VALUES($1,$2,$3,$4,$5,$6,'preview',$7)")
                .bind(&preview.id).bind(&preview.source_preview_id).bind(team.to_string()).bind(project.to_string())
                .bind(actor.identity_id().to_string()).bind(&preview.plan_sha256).bind(json_string(&preview)?)
                .execute(&mut **locked.transaction().map_err(identity_error)?).await.map_err(storage_error)?;
            for (order, destination, metadata, encoded) in binaries {
                attachments::insert(
                    locked.transaction().map_err(identity_error)?,
                    &preview.id,
                    &order,
                    &destination,
                    "team",
                    Some(&team.to_string()),
                    &metadata,
                    &encoded,
                )
                .await?;
            }
            locked
                .validate_content_in_transaction(&permit)
                .await
                .map_err(identity_error)?;
            for p in &permits {
                locked
                    .validate_content_in_transaction(p)
                    .await
                    .map_err(identity_error)?;
            }
            preview
        };
        tx.commit().await.map_err(storage_error)?;
        Ok(preview)
    }
    pub async fn commit_v1_team_import(
        &self,
        actor: &Principal,
        team: Uuid,
        project: Uuid,
        preview_id: &str,
        approved_plan: &str,
    ) -> Result<V1TeamImportPreview> {
        self.commit_v1_team_import_complete(
            actor,
            team,
            project,
            preview_id,
            approved_plan,
            None,
            V1MigrationSecrets::default(),
        )
        .await
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "commit_v1_team_import_complete: Keep importing actor/team/project, approved original preview/plan and migration secret custody explicit"
    )]
    pub async fn commit_v1_team_import_complete(
        &self,
        actor: &Principal,
        team: Uuid,
        project: Uuid,
        preview_id: &str,
        approved_plan: &str,
        service: Option<&crate::team_service::TeamService>,
        mut secrets: V1MigrationSecrets,
    ) -> Result<V1TeamImportPreview> {
        let identity = self.identity();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        // Retain preview and fresh identity secrets through the caller's commit.
        let mut preview: V1TeamImportPreview;
        let identities;
        'quota: {
            let mut locked = identity
                .lock_quota_in_transaction(&mut tx)
                .await
                .map_err(identity_error)?;
            let permit = locked
                .authorize_content_in_transaction(actor, team, project, ContentAction::Write)
                .await
                .map_err(identity_error)?;
            let pins = project_pin(&mut locked, team, project, true).await?;
            let row=sqlx::query("SELECT creator_id,plan_sha256,state,payload FROM v1_team_import_receipts WHERE id=$1 AND team_id=$2 AND project_id=$3")
                .bind(preview_id).bind(team.to_string()).bind(project.to_string()).fetch_optional(&mut **locked.transaction().map_err(identity_error)?).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
            preview = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
            if row
                .try_get::<String, _>("creator_id")
                .map_err(storage_error)?
                != actor.identity_id().to_string()
                || preview.creator_id != actor.identity_id()
                || preview.team_id != team
                || preview.project_id != project
                || preview.plan_sha256 != approved_plan
                || team_plan_hash(&preview)? != approved_plan
                || row
                    .try_get::<String, _>("plan_sha256")
                    .map_err(storage_error)?
                    != approved_plan
            {
                return Err(Error::Conflict);
            }
            let state: String = row.try_get("state").map_err(storage_error)?;
            if state == "committed" && preview.state == state {
                locked
                    .validate_content_in_transaction(&permit)
                    .await
                    .map_err(identity_error)?;
                break 'quota;
            }
            if state != "preview"
                || preview.state != state
                || pins != (preview.project_revision, preview.profile_revision)
            {
                return Err(Error::Conflict);
            }
            if preview.mapping.users.len() != preview.catalog.users.len() {
                return Err(Error::Validation(
                    "Explicit disposition required for every source user".into(),
                ));
            }
            let mut permits = vec![];
            for p in &preview.project_pins {
                let pin = project_pin(&mut locked, team, p.project_id, true).await?;
                if pin != (p.project_revision, p.profile_revision) {
                    return Err(Error::Conflict);
                }
                permits.push(
                    locked
                        .authorize_content_in_transaction(
                            actor,
                            team,
                            p.project_id,
                            ContentAction::Write,
                        )
                        .await
                        .map_err(identity_error)?,
                );
            }
            let expected = preview
                .orders
                .iter()
                .flat_map(|o| {
                    o.attachments
                        .iter()
                        .cloned()
                        .map(|a| (o.order.id.to_string(), o.order.project_id.to_string(), a))
                })
                .collect::<Vec<_>>();
            attachments::validate_rows(
                locked.transaction().map_err(identity_error)?,
                preview_id,
                "team",
                &expected,
                Some(&team.to_string()),
            )
            .await?;
            let imported_at = now();
            let mut histories = Vec::new();
            let mut charged = json_string(&preview)?.len() as u64;
            for entry in &preview.orders {
                if entry.order.team_id != team
                    || !(entry.order.project_id == project
                        || preview
                            .project_pins
                            .iter()
                            .any(|p| p.project_id == entry.order.project_id))
                    || entry.order.revision != 1
                    || entry.order.archived
                {
                    return Err(Error::Conflict);
                }
                let history = V1TeamImportedHistory {
                    import_receipt_id: preview.id.clone(),
                    project_id: entry.order.project_id,
                    order_id: entry.order.id,
                    source_project_id: entry.source_project_id.clone(),
                    messages: entry
                        .messages
                        .iter()
                        .map(|m| ArchivedProductMessage {
                            id: id(),
                            source_id: Some(m.source_id.clone()),
                            role: m.role.clone(),
                            content: m.content.clone(),
                            created_at: m.created_at.unwrap_or(imported_at),
                        })
                        .collect(),
                    artifacts: entry
                        .artifacts
                        .iter()
                        .map(|a| ArchivedProductArtifact {
                            id: id(),
                            source_id: Some(a.source_id.clone()),
                            name: a.name.clone(),
                            media_type: a.media_type.clone(),
                            size: a.content.len() as u64,
                            sha256: hash(a.content.as_bytes()),
                            content: a.content.clone(),
                            created_at: a.created_at.unwrap_or(imported_at),
                        })
                        .collect(),
                    attachments: entry.attachments.clone(),
                    source_entities: preview
                        .source_entities
                        .iter()
                        .filter(|e| {
                            e.source_project_id.as_deref() == Some(entry.source_project_id.as_str())
                                || (e.kind == "project" && e.source_id == entry.source_project_id)
                        })
                        .cloned()
                        .collect(),
                    imported_at,
                    inert: true,
                };
                charged = charged
                    .checked_add(json_string(&history)?.len() as u64)
                    .and_then(|v| v.checked_add((json_string(&entry.order).ok()?.len() * 2) as u64))
                    .ok_or(Error::Conflict)?;
                histories.push(history);
            }
            // Charge retained logical bytes, including base64 storage and metadata.
            for (_, _, metadata) in &expected {
                charged = charged
                    .checked_add(metadata.size.div_ceil(3) * 4)
                    .and_then(|v| v.checked_add(json_string(metadata).ok()?.len() as u64))
                    .ok_or(Error::Conflict)?;
            }
            for (entry, history) in preview.orders.iter().zip(histories) {
                let payload = json_string(&entry.order)?;
                let key = format!("v1-import:{}:{}", preview_id, entry.order.id);
                sqlx::query("INSERT INTO team_orders(id,team_id,project_id,creator_id,creation_key,request_sha,initial_receipt,payload) VALUES($1,$2,$3,$4,$5,$6,$7,$7)")
                    .bind(entry.order.id.to_string()).bind(team.to_string()).bind(entry.order.project_id.to_string()).bind(actor.identity_id().to_string())
                    .bind(key).bind(hash(payload.as_bytes())).bind(payload).execute(&mut **locked.transaction().map_err(identity_error)?).await.map_err(storage_error)?;
                sqlx::query("INSERT INTO v1_team_import_history(order_id,team_id,project_id,payload) VALUES($1,$2,$3,$4)")
                    .bind(entry.order.id.to_string()).bind(team.to_string()).bind(entry.order.project_id.to_string()).bind(json_string(&history)?)
                    .execute(&mut **locked.transaction().map_err(identity_error)?).await.map_err(storage_error)?;
            }
            sqlx::query("UPDATE v1_import_attachments SET state='committed' WHERE preview_id=$1 AND scope='team' AND state='preview'").bind(preview_id).execute(&mut **locked.transaction().map_err(identity_error)?).await.map_err(storage_error)?;
            catalog::insert_configurations(
                locked.transaction().map_err(identity_error)?,
                "team",
                Some(team),
                preview_id,
                &mut preview.configurations,
            )
            .await?;
            if let Some(provider) = &preview.mapping.provider {
                let key = secrets.provider_api_key.take().ok_or_else(|| {
                    Error::Validation("Fresh selected provider key required".into())
                })?;
                preview.provider_binding = Some(
                    service
                        .ok_or(Error::Conflict)?
                        .configure_migration_provider_in_transaction(
                            locked.transaction().map_err(identity_error)?,
                            actor,
                            team,
                            cowork_contracts::ConfigureTeamProvider {
                                expected_revision: provider.expected_revision,
                                base_url: provider.base_url.clone(),
                                model: provider.model.clone(),
                                api_key: key,
                            },
                        )
                        .await?,
                );
            } else if secrets.provider_api_key.is_some() {
                return Err(Error::Validation("Unmapped provider secret".into()));
            }
            identities = catalog::identity_inputs(&preview.mapping, &mut secrets)?;
            if !identities.is_empty() {
                preview.identity_bindings = locked
                    .apply_migration_identities_in_transaction(actor, team, &identities)
                    .await
                    .map_err(identity_error)?;
                // Administrative changes invalidate old epoch-bound permits. Obtain
                // current Write permits again before the caller transaction commits.
                let fresh = locked
                    .authorize_content_in_transaction(actor, team, project, ContentAction::Write)
                    .await
                    .map_err(identity_error)?;
                locked
                    .validate_content_in_transaction(&fresh)
                    .await
                    .map_err(identity_error)?;
                for p in &preview.project_pins {
                    let fresh = locked
                        .authorize_content_in_transaction(
                            actor,
                            team,
                            p.project_id,
                            ContentAction::Write,
                        )
                        .await
                        .map_err(identity_error)?;
                    locked
                        .validate_content_in_transaction(&fresh)
                        .await
                        .map_err(identity_error)?;
                }
            } else {
                locked
                    .validate_content_in_transaction(&permit)
                    .await
                    .map_err(identity_error)?;
                for p in &permits {
                    locked
                        .validate_content_in_transaction(p)
                        .await
                        .map_err(identity_error)?;
                }
            }
            charged = charged
                .checked_add(
                    catalog::configuration_bytes(
                        locked.transaction().map_err(identity_error)?,
                        Some(team),
                        &preview.configurations,
                    )
                    .await?,
                )
                .and_then(|v| {
                    v.checked_add(json_string(&preview.identity_bindings).ok()?.len() as u64)
                })
                .ok_or(Error::Conflict)?;
            if preview.provider_binding.is_some() {
                let sealed: String =
                    sqlx::query_scalar("SELECT sealed_key FROM team_provider WHERE team_id=$1")
                        .bind(team.to_string())
                        .fetch_one(&mut **locked.transaction().map_err(identity_error)?)
                        .await
                        .map_err(storage_error)?;
                charged = charged
                    .checked_add(sealed.len() as u64)
                    .ok_or(Error::Conflict)?;
            }
            let charge_permit = locked
                .authorize_content_in_transaction(actor, team, project, ContentAction::Write)
                .await
                .map_err(identity_error)?;
            locked
                .charge_storage_in_transaction(
                    actor,
                    &charge_permit,
                    &format!("v1-import:{}", preview_id),
                    i64::try_from(charged).map_err(storage_error)?,
                    &preview.plan_sha256,
                )
                .await
                .map_err(identity_error)?;
            preview.state = "committed".into();
            sqlx::query("UPDATE v1_team_import_receipts SET state='committed',payload=$1 WHERE id=$2 AND state='preview'")
                .bind(json_string(&preview)?).bind(preview_id).execute(&mut **locked.transaction().map_err(identity_error)?).await.map_err(storage_error)?;
            locked
                .validate_content_in_transaction(&charge_permit)
                .await
                .map_err(identity_error)?;
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(preview)
    }
    pub async fn v1_team_import_receipts(
        &self,
        actor: &Principal,
        team: Uuid,
        project: Uuid,
    ) -> Result<Vec<V1TeamImportPreview>> {
        let identity = self.identity();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let result = {
            let mut locked = identity
                .lock_quota_in_transaction(&mut tx)
                .await
                .map_err(identity_error)?;
            let permit = locked
                .authorize_content_in_transaction(actor, team, project, ContentAction::Read)
                .await
                .map_err(identity_error)?;
            project_pin(&mut locked, team, project, false).await?;
            let rows=sqlx::query("SELECT payload FROM v1_team_import_receipts WHERE team_id=$1 AND project_id=$2 AND (state='committed' OR creator_id=$3) ORDER BY id")
                .bind(team.to_string()).bind(project.to_string()).bind(actor.identity_id().to_string()).fetch_all(&mut **locked.transaction().map_err(identity_error)?).await.map_err(storage_error)?;
            let result: Vec<V1TeamImportPreview> = rows
                .into_iter()
                .map(|r| parse(&r.try_get::<String, _>("payload").map_err(storage_error)?))
                .collect::<Result<Vec<_>>>()?;
            // A complete immutable receipt may span several explicitly selected
            // projects. Reading its contents requires current Read in every target.
            let mut other_permits = vec![];
            for value in &result {
                for pin in &value.project_pins {
                    if pin.project_id != project {
                        other_permits.push(
                            locked
                                .authorize_content_in_transaction(
                                    actor,
                                    team,
                                    pin.project_id,
                                    ContentAction::Read,
                                )
                                .await
                                .map_err(identity_error)?,
                        );
                    }
                }
            }
            locked
                .validate_content_in_transaction(&permit)
                .await
                .map_err(identity_error)?;
            for p in &other_permits {
                locked
                    .validate_content_in_transaction(p)
                    .await
                    .map_err(identity_error)?;
            }
            result
        };
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn v1_team_imported_history(
        &self,
        actor: &Principal,
        team: Uuid,
        project: Uuid,
        order: Uuid,
    ) -> Result<V1TeamImportedHistory> {
        let identity = self.identity();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let result = {
            let mut locked = identity
                .lock_quota_in_transaction(&mut tx)
                .await
                .map_err(identity_error)?;
            let permit = locked
                .authorize_content_in_transaction(actor, team, project, ContentAction::Read)
                .await
                .map_err(identity_error)?;
            project_pin(&mut locked, team, project, false).await?;
            let row=sqlx::query("SELECT h.payload FROM v1_team_import_history h JOIN team_orders o ON o.id=h.order_id AND o.team_id=h.team_id AND o.project_id=h.project_id WHERE h.order_id=$1 AND h.team_id=$2 AND h.project_id=$3")
                .bind(order.to_string()).bind(team.to_string()).bind(project.to_string()).fetch_optional(&mut **locked.transaction().map_err(identity_error)?).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
            let mut result: V1TeamImportedHistory =
                parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
            let source=sqlx::query("SELECT plan_sha256,payload FROM v1_team_import_receipts WHERE id=$1 AND team_id=$2 AND state='committed'")
                .bind(&result.import_receipt_id).bind(team.to_string())
                .fetch_optional(&mut **locked.transaction().map_err(identity_error)?).await.map_err(storage_error)?.ok_or(Error::Conflict)?;
            let provenance: V1TeamImportPreview = parse(
                &source
                    .try_get::<String, _>("payload")
                    .map_err(storage_error)?,
            )?;
            if result.project_id != project
                || result.order_id != order
                || !result.inert
                || provenance.team_id != team
                || provenance.state != "committed"
                || provenance.plan_sha256
                    != source
                        .try_get::<String, _>("plan_sha256")
                        .map_err(storage_error)?
                || team_plan_hash(&provenance)? != provenance.plan_sha256
                || !provenance
                    .orders
                    .iter()
                    .any(|entry| entry.order.id == order && entry.order.project_id == project)
            {
                return Err(Error::Conflict);
            }
            result.source_entities = provenance.source_entities;
            locked
                .validate_content_in_transaction(&permit)
                .await
                .map_err(identity_error)?;
            result
        };
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn v1_team_imported_attachment(
        &self,
        actor: &Principal,
        team: Uuid,
        project: Uuid,
        attachment: &str,
    ) -> Result<(V1ImportedAttachment, Vec<u8>)> {
        let identity = self.identity();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let (metadata, bytes) = {
            let mut locked = identity
                .lock_quota_in_transaction(&mut tx)
                .await
                .map_err(identity_error)?;
            let permit = locked
                .authorize_content_in_transaction(actor, team, project, ContentAction::Read)
                .await
                .map_err(identity_error)?;
            project_pin(&mut locked, team, project, false).await?;
            let row=sqlx::query("SELECT a.metadata,a.content_base64 FROM v1_import_attachments a JOIN team_orders o ON o.id=a.order_id AND o.project_id=a.project_id AND o.team_id=a.team_id WHERE a.id=$1 AND a.team_id=$2 AND a.project_id=$3 AND a.scope='team' AND a.state='committed'")
                .bind(attachment).bind(team.to_string()).bind(project.to_string()).fetch_optional(&mut **locked.transaction().map_err(identity_error)?).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
            let metadata = parse(
                &row.try_get::<String, _>("metadata")
                    .map_err(storage_error)?,
            )?;
            let bytes = attachments::decode(
                &metadata,
                &row.try_get::<String, _>("content_base64")
                    .map_err(storage_error)?,
            )?;
            locked
                .validate_content_in_transaction(&permit)
                .await
                .map_err(identity_error)?;
            (metadata, bytes)
        };
        tx.commit().await.map_err(storage_error)?;
        Ok((metadata, bytes))
    }
}
