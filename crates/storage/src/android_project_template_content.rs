//! Editable inert copies. Editing never creates provider credentials, runner or publication grants.
use super::*;
const KINDS: &[&str] = &[
    "model_configuration",
    "crew",
    "skill",
    "memory",
    "mcp_metadata",
    "document",
];
pub(super) async fn read_optional(
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    p: &ProjectScope,
    kind: &str,
    id: Uuid,
) -> Result<Option<(Value, Uuid, i64)>> {
    let rows=sqlx::query("SELECT owner_id,revision,CASE WHEN LENGTH(payload)<=16781312 THEN payload ELSE NULL END AS payload FROM android_wire_sync_current WHERE project_id=$1 AND entity_type=$2 AND entity_id=$3 AND tombstone=0 AND ((team_id IS NULL AND owner_id=$4) OR team_id=$5) LIMIT 2").bind(p.project.to_string()).bind(kind).bind(id.to_string()).bind(actor.identity_id().to_string()).bind(p.team.map(|v|v.to_string())).fetch_all(&mut **tx).await.map_err(storage_error)?;
    if rows.len() > 1 {
        return Err(Error::Conflict);
    }
    let Some(r) = rows.first() else {
        return Ok(None);
    };
    let raw: String = r
        .try_get::<Option<String>, _>("payload")
        .map_err(storage_error)?
        .ok_or(Error::BudgetExceeded)?;
    if raw.len() > content::MAX_OBJECT + 4096 {
        return Err(Error::BudgetExceeded);
    }
    let entity: Value = parse(&raw)?;
    let payload = entity.get("payload").cloned().ok_or(Error::Conflict)?;
    if payload["id"] != json!(id) || payload["project_id"] != json!(p.project) {
        return Err(Error::Conflict);
    }
    Ok(Some((
        payload,
        row_id(r, "owner_id")?,
        r.try_get("revision").map_err(storage_error)?,
    )))
}
fn secret(value: &Value) -> bool {
    match value {
        Value::Object(o) => o.iter().any(|(k, v)| {
            matches!(
                k.to_ascii_lowercase().as_str(),
                "api_key"
                    | "apikey"
                    | "access_token"
                    | "refresh_token"
                    | "authorization"
                    | "bearer"
                    | "password"
                    | "secret"
                    | "credential"
                    | "private_key"
            ) && !v.is_null()
                && v != &json!("")
                || (matches!(k.as_str(), "base_url" | "endpoint" | "url" | "uri")
                    && v.as_str()
                        .and_then(|s| reqwest::Url::parse(s).ok())
                        .is_some_and(|u| {
                            !u.username().is_empty()
                                || u.password().is_some()
                                || u.query_pairs().any(|(key, _)| {
                                    matches!(
                                        key.to_ascii_lowercase().as_str(),
                                        "token"
                                            | "access_token"
                                            | "api_key"
                                            | "apikey"
                                            | "password"
                                            | "secret"
                                            | "authorization"
                                    )
                                })
                        }))
                || secret(v)
        }),
        Value::Array(a) => a.iter().any(secret),
        _ => false,
    }
}
impl Store {
    pub async fn android_project_templates(
        &self,
        actor: &Principal,
        project: Uuid,
        after: Option<&str>,
    ) -> Result<Value> {
        let (kind, id) = if let Some(cursor) = after {
            let (k, id) = cursor.split_once(':').ok_or_else(device_invalid)?;
            if !KINDS.contains(&k) {
                return Err(device_invalid());
            }
            (
                k.to_owned(),
                Uuid::parse_str(id)
                    .map_err(|_| device_invalid())?
                    .to_string(),
            )
        } else {
            (String::new(), String::new())
        };
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let p = scope(self, &mut tx, actor, project, ContentAction::Read).await?;
        let rows=sqlx::query("SELECT entity_type,entity_id,revision FROM android_wire_sync_current WHERE project_id=$1 AND tombstone=0 AND entity_type IN('model_configuration','crew','skill','memory','mcp_metadata','document') AND ((team_id IS NULL AND owner_id=$2) OR team_id=$3) AND (entity_type>$4 OR (entity_type=$4 AND entity_id>$5)) ORDER BY entity_type,entity_id LIMIT 51").bind(project.to_string()).bind(actor.identity_id().to_string()).bind(p.team.map(|v|v.to_string())).bind(kind).bind(id).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let mut items = Vec::new();
        for r in rows.iter().take(50) {
            let kind: String = r.try_get("entity_type").map_err(storage_error)?;
            let id = row_id(r, "entity_id")?;
            let (value, _, revision) = read_optional(&mut tx, actor, &p, &kind, id)
                .await?
                .ok_or(Error::Conflict)?;
            items.push(json!({"kind":kind,"id":id,"revision":revision,"name":value["name"].as_str().or_else(||value["title"].as_str()).unwrap_or_default(),"unconfigured":value["unconfigured"],"copied_content_provenance":value["copied_content_provenance"]}));
        }
        let next = if rows.len() > 50 {
            let last = items.last().ok_or(Error::Conflict)?;
            Some(format!(
                "{}:{}",
                last["kind"].as_str().ok_or(Error::Conflict)?,
                last["id"].as_str().ok_or(Error::Conflict)?
            ))
        } else {
            None
        };
        scope(self, &mut tx, actor, project, ContentAction::Read).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(json!({"items":items,"next_cursor":next}))
    }
    pub async fn android_project_template(
        &self,
        actor: &Principal,
        project: Uuid,
        kind: &str,
        id: Uuid,
    ) -> Result<Value> {
        if !KINDS.contains(&kind) {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let p = scope(self, &mut tx, actor, project, ContentAction::Read).await?;
        let (value, _, revision) = read_optional(&mut tx, actor, &p, kind, id)
            .await?
            .ok_or(Error::NotFound)?;
        scope(self, &mut tx, actor, project, ContentAction::Read).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(json!({"kind":kind,"id":id,"revision":revision,"payload":value}))
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "android_project_template_edit: Keep actor/project/template identity, expected revision and original edit request explicit"
    )]
    pub async fn android_project_template_edit(
        &self,
        actor: &Principal,
        project: Uuid,
        kind: &str,
        id: Uuid,
        revision: i64,
        mut body: Value,
        key: &str,
    ) -> Result<Value> {
        if !KINDS.contains(&kind)
            || !body.is_object()
            || revision < 1
            || secret(&body)
            || json_string(&body)?.len() > content::MAX_OBJECT
        {
            return Err(device_invalid());
        }
        operation_key(key)?;
        let request = hash(
            json_string(&(
                "android-project-template-edit/1",
                project,
                kind,
                id,
                revision,
                &body,
            ))?
            .as_bytes(),
        );
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        if let Some(old) = original(
            self,
            &mut tx,
            actor,
            project,
            key,
            "template_edit",
            &request,
        )
        .await?
        {
            tx.commit().await.map_err(storage_error)?;
            return Ok(old);
        }
        let p = scope(self, &mut tx, actor, project, ContentAction::Write).await?;
        let (old, owner, actual) = read_optional(&mut tx, actor, &p, kind, id)
            .await?
            .ok_or(Error::NotFound)?;
        if actual != revision || old["unconfigured"] != true {
            return Err(Error::Conflict);
        }
        let next = revision.checked_add(1).ok_or(Error::Conflict)?;
        body["id"] = json!(id);
        body["project_id"] = json!(project);
        body["team_id"] = json!(p.team);
        body["owner_user_id"] = if p.team.is_none() {
            json!(owner)
        } else {
            Value::Null
        };
        body["schema_version"] = json!(2);
        body["revision"] = json!(next);
        body["unconfigured"] = json!(true);
        body["enabled"] = json!(false);
        body["has_secret"] = json!(false);
        body["credential_configured"] = json!(false);
        body["published_version_id"] = Value::Null;
        body["auto_run"] = json!(false);
        body["updated_at"] = json!(device_iso(current)?);
        body["copied_content_provenance"] = old["copied_content_provenance"].clone();
        if kind == "document"
            && let Some(team) = p.team
        {
            let text = body["content"].as_str().ok_or_else(device_invalid)?;
            let name = body["name"].as_str().ok_or_else(device_invalid)?;
            if text.len() > 128 * 1024 || name.len() > 200 {
                return Err(Error::BudgetExceeded);
            }
            let provenance: KnowledgeProvenance =
                serde_json::from_value(old["provenance"].clone()).map_err(storage_error)?;
            let document = TeamDocument {
                provenance,
                id,
                team_id: team,
                project_id: project,
                kind: serde_json::from_value(body["kind"].clone()).map_err(storage_error)?,
                name: name.into(),
                content: text.into(),
                revision: next,
                archived: false,
            };
            if sqlx::query(
                "UPDATE team_documents SET payload=$1 WHERE id=$2 AND team_id=$3 AND project_id=$4",
            )
            .bind(json_string(&document)?)
            .bind(id.to_string())
            .bind(team.to_string())
            .bind(project.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?
            .rows_affected()
                != 1
            {
                return Err(Error::Conflict);
            }
            crate::knowledge_provenance::record(
                &mut tx,
                &crate::knowledge_provenance::scope(&project.to_string(), Some(team)),
                &id.to_string(),
                next,
                &document.content,
                &document.provenance,
                &document,
            )
            .await?;
            crate::team_service::project_sync_record(
                &mut tx,
                team,
                project,
                "document",
                id,
                &serde_json::to_value(&document).map_err(storage_error)?,
                false,
            )
            .await?;
        }
        for field in [
            "provider_binding",
            "model_binding",
            "private_workflow",
            "private_runner_authority",
            "qualification",
            "lease_id",
            "run_id",
            "approval_id",
        ] {
            body.as_object_mut().ok_or(Error::Conflict)?.remove(field);
        }
        crate::android_wire_sync::record(
            &mut tx,
            owner,
            kind,
            id,
            p.team,
            Some(project),
            next,
            &body,
            false,
            current,
        )
        .await?;
        let result = json!({"id":id,"kind":kind,"revision":next,"payload_sha256":hash(json_string(&body)?.as_bytes()),"execution_authority_copied":false});
        receipt(
            &mut tx,
            actor.identity_id(),
            project,
            key,
            "template_edit",
            &request,
            &result,
        )
        .await?;
        scope(self, &mut tx, actor, project, ContentAction::Write).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
}
