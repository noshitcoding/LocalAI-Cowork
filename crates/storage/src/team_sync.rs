//! Durable, permission-checked replication. Cursors are database sequences, never clocks.
use super::*;

#[derive(Clone, Serialize, Deserialize)]
pub struct TeamSyncChange {
    pub cursor: i64,
    pub kind: String,
    pub entity_id: Uuid,
    pub payload: Value,
    pub tombstone: bool,
    pub updated_at: i64,
}
#[derive(Serialize)]
pub struct TeamSyncPage {
    pub changes: Vec<TeamSyncChange>,
    pub next_cursor: i64,
    pub watermark_cursor: i64,
    pub has_more: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TeamSyncMutation {
    pub idempotency_key: Uuid,
    pub kind: String,
    pub entity_id: Uuid,
    pub expected_revision: i64,
    pub value: Value,
}
pub(super) async fn record(
    tx: &mut Transaction<'_, Any>,
    team: Uuid,
    project: Uuid,
    kind: &str,
    id: Uuid,
    payload: &Value,
    tombstone: bool,
) -> Result<()> {
    let encoded = json_string(payload)?;
    let digest = cowork_domain::hash(encoded.as_bytes());
    // All capture and mutation paths hold this same transactional sequencer.
    sqlx::query("UPDATE team_sync_clock SET sequence=sequence WHERE id=1")
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?;
    let old: Option<String> = sqlx::query_scalar("SELECT digest FROM team_sync_current WHERE team_id=$1 AND project_id=$2 AND kind=$3 AND entity_id=$4")
        .bind(team.to_string()).bind(project.to_string()).bind(kind).bind(id.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?;
    if old.as_deref() == Some(&digest) {
        return Ok(());
    }
    let cursor: i64 = sqlx::query_scalar(
        "UPDATE team_sync_clock SET sequence=sequence+1 WHERE id=1 RETURNING sequence",
    )
    .fetch_one(&mut **tx)
    .await
    .map_err(storage_error)?;
    sqlx::query("INSERT INTO team_sync_changes(cursor,team_id,project_id,kind,entity_id,payload,tombstone,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
        .bind(cursor).bind(team.to_string()).bind(project.to_string()).bind(kind).bind(id.to_string()).bind(&encoded).bind(i64::from(tombstone)).bind(now()).execute(&mut **tx).await.map_err(storage_error)?;
    sqlx::query("INSERT INTO team_sync_current(team_id,project_id,kind,entity_id,digest,cursor,payload) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(team_id,project_id,kind,entity_id) DO UPDATE SET digest=excluded.digest,cursor=excluded.cursor,payload=excluded.payload")
        .bind(team.to_string()).bind(project.to_string()).bind(kind).bind(id.to_string()).bind(digest).bind(cursor).bind(encoded).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
impl TeamService {
    pub async fn sync_metadata(
        &self,
        actor: &Principal,
        kind: &str,
        after: i64,
        limit: i64,
    ) -> Result<TeamSyncPage> {
        self.sync_metadata_inner(actor, kind, after, limit, None)
            .await
    }
    pub async fn sync_metadata_snapshot(
        &self,
        actor: &Principal,
        kind: &str,
        after: Option<Uuid>,
        limit: i64,
    ) -> Result<TeamSyncPage> {
        self.sync_metadata_inner(actor, kind, 0, limit, Some(after))
            .await
    }
    async fn sync_metadata_inner(
        &self,
        actor: &Principal,
        kind: &str,
        after: i64,
        limit: i64,
        snapshot: Option<Option<Uuid>>,
    ) -> Result<TeamSyncPage> {
        if !["skill", "memory", "mcp_metadata"].contains(&kind)
            || after < 0
            || !(1..=1000).contains(&limit)
        {
            return Err(Error::Validation("Invalid metadata cursor".into()));
        }
        let mut scopes = BTreeSet::new();
        for team in self.teams(actor).await? {
            if kind == "mcp_metadata" {
                let servers = self.mcp_servers(actor, team.id).await?;
                let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
                self.store
                    .identity()
                    .organization_in_transaction(&mut tx, actor, team.id)
                    .await
                    .map_err(identity_error)?;
                for server in servers {
                    let revision: Option<i64> = sqlx::query_scalar(
                        "SELECT revision FROM team_mcp_servers WHERE id=$1 AND team_id=$2",
                    )
                    .bind(server.id.to_string())
                    .bind(team.id.to_string())
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(storage_error)?;
                    if revision != Some(server.revision) {
                        return Err(Error::Conflict);
                    }
                    let catalog: Option<String> = sqlx::query_scalar(
                        "SELECT catalog FROM team_mcp_servers WHERE id=$1 AND team_id=$2",
                    )
                    .bind(server.id.to_string())
                    .bind(team.id.to_string())
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(storage_error)?;
                    let catalog: Vec<Value> =
                        catalog.map(|s| parse(&s)).transpose()?.unwrap_or_default();
                    let value = json!({"schema_version":2,"entity_type":kind,"entity_id":server.id,"revision":server.revision,"etag":format!("{}:{}",server.id,server.revision),"payload":{"team_id":team.id,"name":server.name,"endpoint":server.endpoint,"catalog_sha256":server.catalog_sha256,"tool_count":server.tool_count,"catalog":catalog},"tombstone":!server.enabled});
                    record(
                        &mut tx,
                        team.id,
                        Uuid::nil(),
                        kind,
                        server.id,
                        &value,
                        !server.enabled,
                    )
                    .await?;
                }
                tx.commit().await.map_err(storage_error)?;
                scopes.insert((team.id.to_string(), Uuid::nil().to_string()));
            } else {
                for project in self.projects(actor, team.id).await? {
                    let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
                    let permit = self
                        .project_permit(&mut tx, actor, team.id, project.id, ContentAction::Read)
                        .await?;
                    let rows=sqlx::query("SELECT payload FROM team_documents WHERE team_id=$1 AND project_id=$2 ORDER BY id LIMIT 10001").bind(team.id.to_string()).bind(project.id.to_string()).fetch_all(&mut *tx).await.map_err(storage_error)?;
                    if rows.len() > 10000 {
                        return Err(Error::Validation(
                            "Metadata inventory exceeds supported bound".into(),
                        ));
                    }
                    let mut retained = BTreeSet::new();
                    for row in rows {
                        let d: TeamDocument =
                            parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
                        if (kind == "skill") != (d.kind == TeamDocumentKind::Skill) {
                            continue;
                        }
                        retained.insert(d.id.to_string());
                        let value = json!({"schema_version":2,"entity_type":kind,"entity_id":d.id,"revision":d.revision,"etag":format!("{}:{}",d.id,d.revision),"payload":{"name":d.name,"content":d.content,"project_id":d.project_id,"team_id":d.team_id},"tombstone":d.archived});
                        record(&mut tx, team.id, project.id, kind, d.id, &value, d.archived)
                            .await?;
                    }
                    let old=sqlx::query("SELECT entity_id,payload FROM team_sync_current WHERE team_id=$1 AND project_id=$2 AND kind=$3").bind(team.id.to_string()).bind(project.id.to_string()).bind(kind).fetch_all(&mut *tx).await.map_err(storage_error)?;
                    for row in old {
                        let id: String = row.try_get("entity_id").map_err(storage_error)?;
                        if !retained.contains(&id) {
                            let mut value: Value = parse(
                                &row.try_get::<String, _>("payload").map_err(storage_error)?,
                            )?;
                            value["tombstone"] = json!(true);
                            record(
                                &mut tx,
                                team.id,
                                project.id,
                                kind,
                                Uuid::parse_str(&id).map_err(storage_error)?,
                                &value,
                                true,
                            )
                            .await?;
                        }
                    }
                    self.store
                        .identity()
                        .validate_content_in_transaction(&mut tx, &permit)
                        .await
                        .map_err(identity_error)?;
                    tx.commit().await.map_err(storage_error)?;
                    scopes.insert((team.id.to_string(), project.id.to_string()));
                }
            }
        }
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        // Revalidate every selected scope before returning even a previously cached change.
        let mut permits = Vec::new();
        for (team, project) in &scopes {
            let team = Uuid::parse_str(team).map_err(storage_error)?;
            let project = Uuid::parse_str(project).map_err(storage_error)?;
            if project.is_nil() {
                self.store
                    .identity()
                    .organization_in_transaction(&mut tx, actor, team)
                    .await
                    .map_err(identity_error)?;
            } else {
                permits.push(
                    self.project_permit(&mut tx, actor, team, project, ContentAction::Read)
                        .await?,
                );
            }
        }
        let mut query = sqlx::QueryBuilder::<Any>::new(if snapshot.is_some() {
            "SELECT e.* FROM team_sync_current c JOIN team_sync_changes e ON e.cursor=c.cursor WHERE e.kind="
        } else {
            "SELECT e.* FROM team_sync_changes e WHERE e.kind="
        });
        query.push_bind(kind);
        if let Some(snapshot_after) = snapshot {
            query
                .push(" AND e.entity_id>")
                .push_bind(snapshot_after.map(|v| v.to_string()).unwrap_or_default());
        } else {
            query.push(" AND e.cursor>").push_bind(after);
        }
        if scopes.is_empty() {
            query.push(" AND 1=0");
        } else {
            query.push(" AND (");
            for (i, (team, project)) in scopes.iter().enumerate() {
                if i > 0 {
                    query.push(" OR ");
                }
                query
                    .push("(e.team_id=")
                    .push_bind(team)
                    .push(" AND e.project_id=")
                    .push_bind(project)
                    .push(")");
            }
            query.push(")");
        }
        query
            .push(if snapshot.is_some() {
                " ORDER BY e.entity_id LIMIT "
            } else {
                " ORDER BY e.cursor LIMIT "
            })
            .push_bind(limit + 1);
        let rows = query
            .build()
            .fetch_all(&mut *tx)
            .await
            .map_err(storage_error)?;
        let more = rows.len() as i64 > limit;
        let mut changes = Vec::new();
        let mut scanned = after;
        for row in rows {
            let cursor: i64 = row.try_get("cursor").map_err(storage_error)?;
            let scope = (
                row.try_get::<String, _>("team_id").map_err(storage_error)?,
                row.try_get::<String, _>("project_id")
                    .map_err(storage_error)?,
            );
            if !scopes.contains(&scope) {
                scanned = cursor;
                continue;
            }
            if changes.len() as i64 >= limit {
                break;
            }
            scanned = cursor;
            changes.push(TeamSyncChange {
                cursor,
                kind: kind.into(),
                entity_id: Uuid::parse_str(
                    &row.try_get::<String, _>("entity_id")
                        .map_err(storage_error)?,
                )
                .map_err(storage_error)?,
                payload: parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?,
                tombstone: row.try_get::<i64, _>("tombstone").map_err(storage_error)? == 1,
                updated_at: row.try_get("updated_at").map_err(storage_error)?,
            });
        }
        let mut max = sqlx::QueryBuilder::<Any>::new(
            "SELECT COALESCE(MAX(cursor),0) FROM team_sync_changes WHERE kind=",
        );
        max.push_bind(kind);
        if scopes.is_empty() {
            max.push(" AND 1=0");
        } else {
            max.push(" AND (");
            for (i, (team, project)) in scopes.iter().enumerate() {
                if i > 0 {
                    max.push(" OR ");
                }
                max.push("(team_id=")
                    .push_bind(team)
                    .push(" AND project_id=")
                    .push_bind(project)
                    .push(")");
            }
            max.push(")");
        }
        let watermark: i64 = max
            .build_query_scalar()
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
        for permit in &permits {
            self.store
                .identity()
                .validate_content_in_transaction(&mut tx, permit)
                .await
                .map_err(identity_error)?;
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(TeamSyncPage {
            changes,
            next_cursor: scanned,
            watermark_cursor: watermark,
            has_more: more,
        })
    }
    pub async fn sync_project(
        &self,
        actor: &Principal,
        team: Uuid,
        project: Uuid,
        after: i64,
        limit: i64,
    ) -> Result<TeamSyncPage> {
        if after < 0 || !(1..=200).contains(&limit) {
            return Err(Error::Validation("Invalid replication cursor".into()));
        }
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        let permit = self
            .project_permit(&mut tx, actor, team, project, ContentAction::Read)
            .await?;
        let row=sqlx::query("SELECT p.payload,f.name AS profile_name,COALESCE(f.revision,1) AS revision,COALESCE(f.archived,0) AS archived FROM tenant_projects p LEFT JOIN team_project_profiles f ON f.project_id=p.id WHERE p.id=$1 AND p.organization_id=$2").bind(project.to_string()).bind(team.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let base: TenantProject =
            parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        let value = json!({"id":project,"team_id":team,"name":row.try_get::<Option<String>,_>("profile_name").map_err(storage_error)?.unwrap_or(base.name),"revision":row.try_get::<i64,_>("revision").map_err(storage_error)?,"archived":row.try_get::<i64,_>("archived").map_err(storage_error)?==1,"created_at":base.created_at});
        record(
            &mut tx,
            team,
            project,
            "project",
            project,
            &value,
            value["archived"] == true,
        )
        .await?;
        // Materialize current persisted objects too: imports and older installations
        // are included without inventing historical timestamps or replaying effects.
        for (table, kind) in [
            ("team_documents", "document"),
            ("team_orders", "order"),
            ("team_runs", "run"),
            ("team_blobs", "file"),
        ] {
            let sql = format!(
                "SELECT payload FROM {table} WHERE team_id=$1 AND project_id=$2 ORDER BY id LIMIT 10001"
            );
            let rows = sqlx::query(&sql)
                .bind(team.to_string())
                .bind(project.to_string())
                .fetch_all(&mut *tx)
                .await
                .map_err(storage_error)?;
            if rows.len() > 10000 {
                return Err(Error::Validation(
                    "Project replica exceeds its bounded page inventory".into(),
                ));
            }
            for row in rows {
                let value: Value =
                    parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
                let id = Uuid::parse_str(value["id"].as_str().ok_or(Error::Conflict)?)
                    .map_err(storage_error)?;
                let tombstone = value["archived"] == true || value["state"] == "deleted";
                record(&mut tx, team, project, kind, id, &value, tombstone).await?;
            }
        }
        let rows = sqlx::query("SELECT * FROM team_sync_changes WHERE team_id=$1 AND project_id=$2 AND cursor>$3 ORDER BY cursor LIMIT $4")
            .bind(team.to_string()).bind(project.to_string()).bind(after).bind(limit+1).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let has_more = rows.len() as i64 > limit;
        let mut changes = Vec::new();
        let mut bytes = 0usize;
        for row in rows.into_iter().take(limit as usize) {
            let raw: String = row.try_get("payload").map_err(storage_error)?;
            bytes += raw.len();
            if bytes > 2 * 1024 * 1024 && !changes.is_empty() {
                break;
            }
            changes.push(TeamSyncChange {
                cursor: row.try_get("cursor").map_err(storage_error)?,
                kind: row.try_get("kind").map_err(storage_error)?,
                entity_id: Uuid::parse_str(
                    &row.try_get::<String, _>("entity_id")
                        .map_err(storage_error)?,
                )
                .map_err(storage_error)?,
                payload: parse(&raw)?,
                tombstone: row.try_get::<i64, _>("tombstone").map_err(storage_error)? == 1,
                updated_at: row.try_get("updated_at").map_err(storage_error)?,
            });
        }
        let watermark: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(cursor),0) FROM team_sync_changes WHERE team_id=$1 AND project_id=$2")
            .bind(team.to_string()).bind(project.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let next_cursor = changes.last().map_or(after, |v| v.cursor);
        self.store
            .identity()
            .validate_content_in_transaction(&mut tx, &permit)
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(TeamSyncPage {
            changes,
            next_cursor,
            watermark_cursor: watermark,
            has_more: has_more || next_cursor < watermark,
        })
    }
    /// A write, its replication event and immutable operation receipt commit together.
    pub async fn apply_sync(
        &self,
        actor: &Principal,
        team: Uuid,
        project: Uuid,
        input: TeamSyncMutation,
    ) -> Result<Value> {
        if input.expected_revision < 0
            || input.entity_id.is_nil()
            || input.idempotency_key.is_nil()
            || !["document", "order", "project"].contains(&input.kind.as_str())
        {
            return Err(Error::Validation("Invalid replica mutation".into()));
        }
        let request = cowork_domain::hash(json_string(&input)?.as_bytes());
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        let permit = self
            .project_permit_checked(
                &mut tx,
                actor,
                team,
                project,
                ContentAction::Write,
                input.kind == "project",
            )
            .await?;
        let old = sqlx::query("SELECT request_sha256,receipt FROM team_sync_operations WHERE team_id=$1 AND project_id=$2 AND identity_id=$3 AND idempotency_key=$4")
            .bind(team.to_string()).bind(project.to_string()).bind(actor.identity_id().to_string()).bind(input.idempotency_key.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        if let Some(old) = old {
            if old
                .try_get::<String, _>("request_sha256")
                .map_err(storage_error)?
                != request
            {
                return Err(Error::Conflict);
            }
            let receipt = parse(&old.try_get::<String, _>("receipt").map_err(storage_error)?)?;
            self.store
                .identity()
                .validate_content_in_transaction(&mut tx, &permit)
                .await
                .map_err(identity_error)?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(receipt);
        }
        let result = if input.kind == "document" {
            let mut body: SaveTeamDocument =
                serde_json::from_value(input.value).map_err(storage_error)?;
            body.id = Some(input.entity_id);
            body.expected_revision = input.expected_revision;
            safe_text(&body.name, 200)?;
            safe_text(&body.content, 128 * 1024)?;
            let old: Option<String> = sqlx::query_scalar(
                "SELECT payload FROM team_documents WHERE id=$1 AND team_id=$2 AND project_id=$3",
            )
            .bind(input.entity_id.to_string())
            .bind(team.to_string())
            .bind(project.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?;
            let existing = old.map(|v| parse::<TeamDocument>(&v)).transpose()?;
            let revision = existing.as_ref().map_or(0, |value| value.revision);
            if revision != input.expected_revision {
                return Err(Error::Conflict);
            }
            let value = TeamDocument {
                provenance: {
                    let mut provenance = body.provenance.unwrap_or_else(|| {
                        existing
                            .as_ref()
                            .map_or_else(KnowledgeProvenance::user_authored, |value| {
                                value.provenance.clone()
                            })
                    });
                    crate::knowledge_provenance::bind_sources(
                        &mut tx,
                        &project.to_string(),
                        Some(team),
                        &mut provenance,
                    )
                    .await?;
                    provenance
                },
                id: input.entity_id,
                team_id: team,
                project_id: project,
                kind: body.kind,
                name: body.name.trim().into(),
                content: body.content,
                revision: revision.checked_add(1).ok_or(Error::Conflict)?,
                archived: body.archived,
            };
            let n=sqlx::query("INSERT INTO team_documents(id,team_id,project_id,payload) VALUES($1,$2,$3,$4) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload WHERE team_documents.team_id=excluded.team_id AND team_documents.project_id=excluded.project_id")
                .bind(input.entity_id.to_string()).bind(team.to_string()).bind(project.to_string()).bind(json_string(&value)?).execute(&mut *tx).await.map_err(storage_error)?.rows_affected();
            if n != 1 {
                return Err(Error::Conflict);
            }
            let provenance_scope =
                crate::knowledge_provenance::scope(&project.to_string(), Some(team));
            if let Some(previous) = &existing {
                crate::knowledge_provenance::record(
                    &mut tx,
                    &provenance_scope,
                    &previous.id.to_string(),
                    previous.revision,
                    &previous.content,
                    &previous.provenance,
                    previous,
                )
                .await?;
            }
            crate::knowledge_provenance::record(
                &mut tx,
                &provenance_scope,
                &value.id.to_string(),
                value.revision,
                &value.content,
                &value.provenance,
                &value,
            )
            .await?;
            serde_json::to_value(value).map_err(storage_error)?
        } else if input.kind == "order" {
            let body: UpdateTeamOrder =
                serde_json::from_value(input.value).map_err(storage_error)?;
            safe_text(&body.title, 200)?;
            safe_text(&body.prompt, 128 * 1024)?;
            let raw: Option<String> = sqlx::query_scalar(
                "SELECT payload FROM team_orders WHERE id=$1 AND team_id=$2 AND project_id=$3",
            )
            .bind(input.entity_id.to_string())
            .bind(team.to_string())
            .bind(project.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?;
            let value = if let Some(raw) = raw {
                let mut value: TeamOrder = parse(&raw)?;
                if value.id != input.entity_id
                    || value.team_id != team
                    || value.project_id != project
                    || value.revision != input.expected_revision
                {
                    return Err(Error::Conflict);
                }
                value.title = body.title.trim().into();
                value.prompt = body.prompt;
                value.archived = body.archived;
                value.revision = value.revision.checked_add(1).ok_or(Error::Conflict)?;
                let changed = sqlx::query(
                    "UPDATE team_orders SET payload=$1 WHERE id=$2 AND team_id=$3 AND project_id=$4 AND payload=$5",
                )
                .bind(json_string(&value)?)
                .bind(input.entity_id.to_string())
                .bind(team.to_string())
                .bind(project.to_string())
                .bind(raw)
                .execute(&mut *tx)
                .await
                .map_err(storage_error)?;
                if changed.rows_affected() != 1 {
                    return Err(Error::Conflict);
                }
                value
            } else {
                if input.expected_revision != 0 {
                    return Err(Error::Conflict);
                }
                let value = TeamOrder {
                    id: input.entity_id,
                    team_id: team,
                    project_id: project,
                    title: body.title.trim().into(),
                    prompt: body.prompt,
                    revision: 1,
                    archived: body.archived,
                    created_at: now(),
                };
                let payload = json_string(&value)?;
                // The local entity UUID remains stable. Creation, feed entry and
                // original receipt share the current content-authority writer.
                // A UUID or creation-key collision never adopts a foreign row.
                let inserted = sqlx::query("INSERT INTO team_orders(id,team_id,project_id,creator_id,creation_key,request_sha,initial_receipt,payload) VALUES($1,$2,$3,$4,$5,$6,$7,$7) ON CONFLICT DO NOTHING")
                    .bind(value.id.to_string()).bind(team.to_string()).bind(project.to_string())
                    .bind(actor.identity_id().to_string()).bind(format!("replica-order:{}",input.idempotency_key))
                    .bind(&request).bind(payload).execute(&mut *tx).await.map_err(storage_error)?;
                if inserted.rows_affected() != 1 {
                    return Err(Error::Conflict);
                }
                value
            };
            serde_json::to_value(value).map_err(storage_error)?
        } else {
            if input.entity_id != project {
                return Err(Error::Forbidden);
            }
            let body: UpdateTeamProject =
                serde_json::from_value(input.value).map_err(storage_error)?;
            safe_text(&body.name, 200)?;
            let current: Option<i64> = sqlx::query_scalar(
                "SELECT revision FROM team_project_profiles WHERE project_id=$1",
            )
            .bind(project.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?;
            if current.unwrap_or(1) != input.expected_revision {
                return Err(Error::Conflict);
            }
            let revision = input
                .expected_revision
                .checked_add(1)
                .ok_or(Error::Conflict)?;
            sqlx::query("INSERT INTO team_project_profiles(project_id,revision,name,archived) VALUES($1,$2,$3,$4) ON CONFLICT(project_id) DO UPDATE SET revision=excluded.revision,name=excluded.name,archived=excluded.archived")
                .bind(project.to_string()).bind(revision).bind(body.name.trim()).bind(i64::from(body.archived)).execute(&mut *tx).await.map_err(storage_error)?;
            sqlx::query("UPDATE identity_resources SET revision=revision+1 WHERE id=$1")
                .bind(project.to_string())
                .execute(&mut *tx)
                .await
                .map_err(storage_error)?;
            json!({"id":project,"team_id":team,"name":body.name.trim(),"revision":revision,"archived":body.archived})
        };
        record(
            &mut tx,
            team,
            project,
            &input.kind,
            input.entity_id,
            &result,
            result["archived"] == true,
        )
        .await?;
        let receipt = json!({"idempotency_key":input.idempotency_key,"kind":input.kind,"entity_id":input.entity_id,"value":result});
        sqlx::query("INSERT INTO team_sync_operations(team_id,project_id,identity_id,idempotency_key,request_sha256,receipt) VALUES($1,$2,$3,$4,$5,$6)")
            .bind(team.to_string()).bind(project.to_string()).bind(actor.identity_id().to_string()).bind(input.idempotency_key.to_string()).bind(request).bind(json_string(&receipt)?).execute(&mut *tx).await.map_err(storage_error)?;
        if input.kind == "project" {
            self.store
                .identity()
                .organization_in_transaction(&mut tx, actor, team)
                .await
                .map_err(identity_error)?;
        } else {
            self.store
                .identity()
                .validate_content_in_transaction(&mut tx, &permit)
                .await
                .map_err(identity_error)?;
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
}
