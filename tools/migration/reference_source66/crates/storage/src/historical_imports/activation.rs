//! Current content writes share the same identity/quota/receipt transaction.
use super::*;
use crate::*;
use cowork_identity::{ContentAction, ContentPermit, LockedIdentityQuota};
use cowork_secrets::{KeyId, KeyStore};
use sqlx::{Any, Transaction};

pub(super) async fn current_target(
    locked: &mut LockedIdentityQuota<'_, '_, '_>,
    authority: &HistoricalAuthority<'_>,
    target: &HistoricalProjectTarget,
) -> Result<Option<ContentPermit>> {
    match (authority, target) {
        (_, HistoricalProjectTarget::HistoryOnly { reason }) => {
            if reason.trim().is_empty() || reason.len() > 2048 {
                return Err(Error::Validation(
                    "Explicit history-only disposition requires a reason".into(),
                ));
            }
            Ok(None)
        }
        (HistoricalAuthority::PersonalNative(_), HistoricalProjectTarget::NewPersonal) => Ok(None),
        (
            HistoricalAuthority::PersonalNative(_),
            HistoricalProjectTarget::ExistingPersonal {
                project_id,
                expected_revision,
            },
        ) => {
            sqlx::query("UPDATE projects SET payload=payload WHERE id=$1")
                .bind(project_id)
                .execute(&mut **sqltx(locked)?)
                .await
                .map_err(storage_error)?;
            let raw = sqlx::query_scalar::<_, String>("SELECT payload FROM projects WHERE id=$1")
                .bind(project_id)
                .fetch_optional(&mut **sqltx(locked)?)
                .await
                .map_err(storage_error)?
                .ok_or(Error::NotFound)?;
            let p: Project = parse(&raw)?;
            if &p.id != project_id || p.revision != *expected_revision {
                return Err(Error::Conflict);
            }
            Ok(None)
        }
        (
            HistoricalAuthority::Organization {
                actor,
                organization_id,
                ..
            },
            HistoricalProjectTarget::ExistingOrganization {
                project_id,
                expected_revision,
                expected_profile_revision,
            },
        ) => {
            let permit = locked
                .authorize_content_in_transaction(
                    actor,
                    *organization_id,
                    *project_id,
                    ContentAction::Write,
                )
                .await
                .map_err(identity_error)?;
            sqlx::query(
                "UPDATE tenant_projects SET revision=revision WHERE id=$1 AND organization_id=$2",
            )
            .bind(project_id.to_string())
            .bind(organization_id.to_string())
            .execute(&mut **sqltx(locked)?)
            .await
            .map_err(storage_error)?;
            sqlx::query("UPDATE team_project_profiles SET revision=revision WHERE project_id=$1")
                .bind(project_id.to_string())
                .execute(&mut **sqltx(locked)?)
                .await
                .map_err(storage_error)?;
            let r=sqlx::query("SELECT p.revision,COALESCE(f.revision,0) AS profile_revision,COALESCE(f.archived,0) AS archived FROM tenant_projects p LEFT JOIN team_project_profiles f ON f.project_id=p.id WHERE p.id=$1 AND p.organization_id=$2")
                .bind(project_id.to_string()).bind(organization_id.to_string()).fetch_optional(&mut **sqltx(locked)?).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
            if r.try_get::<i64, _>("revision").map_err(storage_error)? != *expected_revision
                || r.try_get::<i64, _>("profile_revision")
                    .map_err(storage_error)?
                    != *expected_profile_revision
                || r.try_get::<i64, _>("archived").map_err(storage_error)? != 0
            {
                return Err(Error::Conflict);
            }
            Ok(Some(permit))
        }
        _ => Err(Error::Forbidden),
    }
}
async fn check_project_capacity(
    tx: &mut Transaction<'_, Any>,
    organization: Option<Uuid>,
    projects: &[HistoricalPreparedProject],
    history_files: &[String],
) -> Result<()> {
    use futures_util::TryStreamExt;
    // Every selected source part to one destination shares this capacity calculation.
    // Physical byte charges remain the actual native/blob operation's original quota domain.
    let mut totals = std::collections::BTreeMap::<String, (usize, usize, usize, u64)>::new();
    for p in projects {
        if matches!(p.target, HistoricalProjectTarget::HistoryOnly { .. }) {
            continue;
        }
        let count = totals.entry(p.project_id.clone()).or_default();
        count.0 = count
            .0
            .checked_add(p.source.documents.len())
            .ok_or(Error::BudgetExceeded)?;
        count.1 = count
            .1
            .checked_add(
                p.source
                    .files
                    .iter()
                    .filter(|f| !history_files.contains(&f.source_id))
                    .count(),
            )
            .ok_or(Error::BudgetExceeded)?;
        count.2 = count
            .2
            .checked_add(p.source.orders.len())
            .ok_or(Error::BudgetExceeded)?;
        let overhead = (p.source.orders.len() as u64)
            .checked_mul(4096)
            .and_then(|v| v.checked_add((p.source.documents.len() as u64) * 4096))
            .ok_or(Error::BudgetExceeded)?;
        count.3 = count
            .3
            .checked_add(p.content_bytes)
            .and_then(|v| v.checked_add(overhead))
            .ok_or(Error::BudgetExceeded)?;
    }
    for (project, (documents, files, orders, mut bytes)) in totals {
        let (queries, doc_count, order_count) = if let Some(team) = organization {
            let existing_files: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM team_blobs WHERE team_id=$1 AND project_id=$2",
            )
            .bind(team.to_string())
            .bind(&project)
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
            if existing_files < 0
                || (existing_files as usize)
                    .checked_add(files)
                    .is_none_or(|v| v > 2000)
            {
                return Err(Error::Validation(
                    "Team project file limit reached; choose additional destinations".into(),
                ));
            }
            let docs: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM team_documents WHERE team_id=$1 AND project_id=$2",
            )
            .bind(team.to_string())
            .bind(&project)
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
            let orders: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM team_orders WHERE team_id=$1 AND project_id=$2",
            )
            .bind(team.to_string())
            .bind(&project)
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
            (
                vec![
                    "SELECT payload FROM team_orders WHERE project_id=$1 AND team_id=$2",
                    "SELECT payload FROM team_documents WHERE project_id=$1 AND team_id=$2",
                    "SELECT payload FROM v1_team_import_history WHERE project_id=$1 AND team_id=$2",
                ],
                docs,
                orders,
            )
        } else {
            let docs:i64=sqlx::query_scalar("SELECT COUNT(*) FROM productivity_objects WHERE project_id=$1 AND kind='knowledge'").bind(&project).fetch_one(&mut **tx).await.map_err(storage_error)?;
            let orders: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM orders WHERE project_id=$1")
                .bind(&project)
                .fetch_one(&mut **tx)
                .await
                .map_err(storage_error)?;
            (
                vec![
                    "SELECT payload FROM orders WHERE project_id=$1",
                    "SELECT payload FROM productivity_objects WHERE project_id=$1 AND kind IN('knowledge','history')",
                ],
                docs,
                orders,
            )
        };
        if doc_count < 0
            || (doc_count as usize)
                .checked_add(documents)
                .is_none_or(|v| v > 2000)
        {
            return Err(Error::Validation(
                "Project knowledge limit exceeded; choose additional destinations".into(),
            ));
        }
        if order_count < 0
            || (order_count as usize)
                .checked_add(orders)
                .is_none_or(|v| v > 1000)
        {
            return Err(Error::Validation(
                "Project current-order capacity exceeded; choose additional destinations".into(),
            ));
        }
        for query in queries {
            let mut query = sqlx::query(query).bind(&project);
            if let Some(team) = organization {
                query = query.bind(team.to_string());
            }
            let mut rows = query.fetch(&mut **tx);
            while let Some(row) = rows.try_next().await.map_err(storage_error)? {
                let raw: String = row.try_get("payload").map_err(storage_error)?;
                bytes = bytes
                    .checked_add(raw.len() as u64)
                    .ok_or(Error::BudgetExceeded)?;
                if bytes > 256 * 1024 * 1024 {
                    return Err(Error::Validation("Current editable content exceeds one target project; choose additional destinations".into()));
                }
            }
        }
        if bytes > 256 * 1024 * 1024 {
            return Err(Error::Validation("Current editable content exceeds one target project; choose additional destinations".into()));
        }
    }
    Ok(())
}
fn projected_target(p: &HistoricalProjectTarget) -> String {
    match p {
        HistoricalProjectTarget::HistoryOnly { .. } => String::new(),
        HistoricalProjectTarget::NewPersonal => id(),
        HistoricalProjectTarget::ExistingPersonal { project_id, .. } => project_id.clone(),
        HistoricalProjectTarget::ExistingOrganization { project_id, .. } => project_id.to_string(),
    }
}
async fn activated_part(
    tx: &mut Transaction<'_, Any>,
    owner: &HistoricalOwner,
    import: Uuid,
    source: &str,
) -> Result<bool> {
    let r=sqlx::query("SELECT owner,preview_id,part_receipt_sha256,part_receipt FROM historical_import_activated_parts WHERE import_id=$1 AND source_id=$2")
        .bind(import.to_string()).bind(source).fetch_optional(&mut **tx).await.map_err(storage_error)?;
    let Some(r) = r else { return Ok(false) };
    let raw: String = r.try_get("part_receipt").map_err(storage_error)?;
    let v: Value = parse(&raw)?;
    if r.try_get::<String, _>("owner").map_err(storage_error)? != owner.locator()
        || r.try_get::<String, _>("part_receipt_sha256")
            .map_err(storage_error)?
            != hash(raw.as_bytes())
        || v["format"] != "historical-current-part-activation/1"
        || v["import_id"] != import.to_string()
        || v["source_part_id"] != source
        || v["owner"] != serde_json::to_value(owner).map_err(storage_error)?
        || v["preview_id"]
            != r.try_get::<String, _>("preview_id")
                .map_err(storage_error)?
    {
        return Err(Error::Conflict);
    }
    Ok(true)
}
fn first_target(
    checks: &mut std::collections::BTreeMap<String, String>,
    target: &HistoricalProjectTarget,
) -> Result<bool> {
    let key = match target {
        HistoricalProjectTarget::ExistingPersonal { project_id, .. } => {
            format!("personal:{project_id}")
        }
        HistoricalProjectTarget::ExistingOrganization { project_id, .. } => {
            format!("organization:{project_id}")
        }
        _ => return Ok(true),
    };
    let pins = json_string(target)?;
    if let Some(old) = checks.get(&key) {
        if old != &pins {
            return Err(Error::Conflict);
        }
        Ok(false)
    } else {
        checks.insert(key, pins);
        Ok(true)
    }
}
fn bounded_batch_receipt(plan: &HistoricalImportPreview) -> Result<()> {
    if plan.remainder_history_only_reason.is_none() {
        return Ok(());
    }
    let mut objects = vec![];
    for p in &plan.projects {
        if matches!(p.target, HistoricalProjectTarget::HistoryOnly { .. }) {
            objects.push(HistoricalActivatedObject {
                source_id: p.source.source_id.clone(),
                kind: "retained_history".into(),
                id: plan.import_id.to_string(),
                project_id: String::new(),
            });
            continue;
        }
        let object = |source: &str, kind: &str, id: &str| HistoricalActivatedObject {
            source_id: source.into(),
            kind: kind.into(),
            id: id.into(),
            project_id: p.project_id.clone(),
        };
        objects.push(object(&p.source.source_id, "project", &p.project_id));
        for (source, id) in p.source.orders.iter().zip(&p.order_ids) {
            objects.push(object(&source.source_id, &source.source_kind, id));
        }
        for (source, id) in p.source.documents.iter().zip(&p.document_ids) {
            objects.push(object(&source.source_id, "knowledge", id));
        }
        for (source, id) in p.source.files.iter().zip(&p.file_ids) {
            if !plan.history_only_file_ids.contains(&source.source_id) {
                objects.push(object(&source.source_id, "file", id));
            }
        }
    }
    let receipt = HistoricalActivationReceipt {
        preview_id: plan.id,
        import_id: plan.import_id,
        plan_sha256: "f".repeat(64),
        source_sha256: plan.source_sha256.clone(),
        owner: plan.owner.clone(),
        activated_at: i64::MAX,
        objects,
        charged_bytes: u64::MAX,
        catalog_sha256: plan.catalog_sha256.clone(),
        remainder_history_only_reason: plan.remainder_history_only_reason.clone(),
        remainder_parts: Some(u32::MAX),
    };
    if json_string(&receipt)?.len() > 3 * 1024 * 1024 {
        return Err(Error::Validation("Select a smaller explicit batch; its original activation receipt exceeds the native response bound".into()));
    }
    Ok(())
}
impl Store {
    pub async fn configure_native_historical_import_limit(
        &self,
        authority: HistoricalAuthority<'_>,
        key_id: KeyId,
        input: ConfigureHistoricalImportLimit,
    ) -> Result<HistoricalImportLimit> {
        if !matches!(authority, HistoricalAuthority::PersonalNative(_))
            || input.expected_revision < 0
            || input.expected_revision == i64::MAX
            || input.limit_bytes == 0
            || input.limit_bytes > MAX_INTAKE_BYTES * 16
        {
            return Err(Error::Forbidden);
        }
        let identity = self.identity();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let mut locked = identity
            .lock_quota_in_transaction(&mut tx)
            .await
            .map_err(identity_error)?;
        authorize(&mut locked, &authority, key_id, ContentAction::Write).await?;
        let value = configure_limit_in(
            sqltx(&mut locked)?,
            key_id.instance,
            input.expected_revision,
            input.limit_bytes,
        )
        .await?;
        drop(locked);
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn historical_import_targets(
        &self,
        authority: HistoricalAuthority<'_>,
        key_id: KeyId,
    ) -> Result<Vec<HistoricalImportTarget>> {
        let identity = self.identity();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let mut locked = identity
            .lock_quota_in_transaction(&mut tx)
            .await
            .map_err(identity_error)?;
        let permit = authorize(&mut locked, &authority, key_id, ContentAction::Read).await?;
        let mut result = vec![];
        match &authority {
            HistoricalAuthority::PersonalNative(_) => {
                let rows = sqlx::query("SELECT payload FROM projects ORDER BY id LIMIT 10001")
                    .fetch_all(&mut **sqltx(&mut locked)?)
                    .await
                    .map_err(storage_error)?;
                if rows.len() > 10000 {
                    return Err(Error::BudgetExceeded);
                }
                for row in rows {
                    let p: Project =
                        parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
                    result.push(HistoricalImportTarget {
                        id: p.id,
                        name: p.name,
                        revision: p.revision,
                        profile_revision: 0,
                    });
                }
            }
            HistoricalAuthority::Organization {
                actor,
                organization_id,
                ..
            } => {
                let rows=sqlx::query("SELECT p.id,p.revision,p.payload,COALESCE(f.revision,0) AS profile_revision,COALESCE(f.name,'') AS name FROM tenant_projects p LEFT JOIN team_project_profiles f ON f.project_id=p.id JOIN identity_resources r ON r.id=p.id AND r.organization_id=p.organization_id JOIN identity_memberships m ON m.organization_id=p.organization_id AND m.identity_id=$2 AND m.active=1 JOIN identity_content_grants g ON g.resource_id=p.id AND g.membership_id=m.id AND g.active=1 AND (g.permissions & 2)=2 WHERE p.organization_id=$1 AND COALESCE(f.archived,0)=0 ORDER BY p.id LIMIT 10001").bind(organization_id.to_string()).bind(actor.identity_id().to_string()).fetch_all(&mut **sqltx(&mut locked)?).await.map_err(storage_error)?;
                if rows.len() > 10000 {
                    return Err(Error::BudgetExceeded);
                }
                for row in rows {
                    let id: String = row.try_get("id").map_err(storage_error)?;
                    let project = Uuid::parse_str(&id).map_err(storage_error)?;
                    let permit = locked
                        .authorize_content_in_transaction(
                            actor,
                            *organization_id,
                            project,
                            ContentAction::Write,
                        )
                        .await
                        .map_err(identity_error)?;
                    let p: crate::TenantProject =
                        parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
                    let name: String = row.try_get("name").map_err(storage_error)?;
                    result.push(HistoricalImportTarget {
                        id,
                        name: if name.is_empty() { p.name } else { name },
                        revision: row.try_get("revision").map_err(storage_error)?,
                        profile_revision: row.try_get("profile_revision").map_err(storage_error)?,
                    });
                    locked
                        .validate_content_in_transaction(&permit)
                        .await
                        .map_err(identity_error)?;
                }
            }
        }
        recheck(&mut locked, &permit).await?;
        drop(locked);
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    /// Trusted staging adapters replay the durable original receipt rather than minting a new IO proof.
    pub async fn historical_staged_file(
        &self,
        authority: HistoricalAuthority<'_>,
        keys: &dyn KeyStore,
        key_id: KeyId,
        preview: Uuid,
        source: &str,
    ) -> Result<HistoricalStagedFile> {
        let identity = self.identity();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let mut locked = identity
            .lock_quota_in_transaction(&mut tx)
            .await
            .map_err(identity_error)?;
        let permit = authorize(&mut locked, &authority, key_id, ContentAction::Read).await?;
        let p = load_plan(sqltx(&mut locked)?, keys, &authority.owner(), preview).await?;
        authorize_plan_read(&mut locked, &authority, &p).await?;
        let r=sqlx::query("SELECT o.payload AS operation,o.original_receipt,f.payload AS confirmed_payload,f.target_id FROM historical_import_file_operations o JOIN historical_import_activation_files f ON f.preview_id=o.preview_id AND f.source_id=o.source_id AND f.original_receipt=o.original_receipt WHERE o.preview_id=$1 AND o.source_id=$2 AND o.owner=$3 AND o.state='received' AND o.dispatch_admitted=1 AND o.original_unsent_receipt IS NULL").bind(preview.to_string()).bind(source).bind(p.owner.locator()).fetch_optional(&mut **sqltx(&mut locked)?).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        let operation: HistoricalFileOperation =
            parse(&r.try_get::<String, _>("operation").map_err(storage_error)?)?;
        if operation.plan_sha256 != p.plan_sha256
            || operation.preview_id != preview
            || operation.source_id != source
            || operation.owner != p.owner
            || operation.target_id != r.try_get::<String, _>("target_id").map_err(storage_error)?
        {
            return Err(Error::Conflict);
        }
        let value = HistoricalStagedFile::from_confirmed_adapter(
            preview,
            p.plan_sha256,
            source.into(),
            operation.target_id,
            operation.project_id,
            operation.size,
            operation.sha256,
            parse(
                &r.try_get::<String, _>("confirmed_payload")
                    .map_err(storage_error)?,
            )?,
            parse(
                &r.try_get::<String, _>("original_receipt")
                    .map_err(storage_error)?,
            )?,
        );
        recheck(&mut locked, &permit).await?;
        drop(locked);
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn preview_historical_import(
        &self,
        authority: HistoricalAuthority<'_>,
        keys: &dyn KeyStore,
        key_id: KeyId,
        import: Uuid,
        input: PreviewHistoricalImport,
    ) -> Result<HistoricalImportPreview> {
        if input.request_id.is_nil() {
            return Err(Error::Validation(
                "An original preview request ID is required".into(),
            ));
        }
        let key_id = writer(keys, key_id)?;
        let owner = authority.owner();
        let fingerprint =
            canonical_hash(&("historical-current-preview/v1", import, &owner, &input));
        let identity = self.identity();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let mut locked = identity
            .lock_quota_in_transaction(&mut tx)
            .await
            .map_err(identity_error)?;
        let permit = authorize(&mut locked, &authority, key_id, ContentAction::Write).await?;
        crate::server_keys::cipher_writer(sqltx(&mut locked)?, key_id).await?;
        if let Some(existing)=sqlx::query("SELECT id,request_sha256 FROM historical_import_previews WHERE import_id=$1 AND request_id=$2 AND owner=$3").bind(import.to_string()).bind(input.request_id.to_string()).bind(owner.locator()).fetch_optional(&mut **sqltx(&mut locked)?).await.map_err(storage_error)?{
            if existing.try_get::<String,_>("request_sha256").map_err(storage_error)?!=fingerprint{return Err(Error::Conflict)}
            let preview=Uuid::parse_str(&existing.try_get::<String,_>("id").map_err(storage_error)?).map_err(storage_error)?;
            let value=load_plan(sqltx(&mut locked)?,keys,&owner,preview).await?;recheck(&mut locked,&permit).await?;drop(locked);tx.commit().await.map_err(storage_error)?;return Ok(value)
        }
        let current = record(sqltx(&mut locked)?, import, &owner).await?;
        if !matches!(current.state.as_str(), "verified" | "preview")
            || current.revision != input.expected_import_revision
        {
            return Err(Error::Conflict);
        }
        let catalog =
            super::catalog::catalog_manifest(sqltx(&mut locked)?, keys, &owner, import).await?;
        if catalog.source_sha256 != current.pins.quarantine_database {
            return Err(Error::Conflict);
        }
        let batch = input.remainder_history_only_reason.is_some();
        if let Some(reason) = &input.remainder_history_only_reason {
            if reason.trim().is_empty()
                || reason.len() > 2048
                || reason.contains('\0')
                || input.projects.is_empty()
                || input.catalog_sha256.as_deref() != Some(catalog.sha256.as_str())
                || input.catalog_count != Some(catalog.count)
            {
                return Err(Error::Validation("Explicit batch scope requires the original complete catalog pin and retained-history reason".into()));
            }
        } else if input.catalog_sha256.is_some() != input.catalog_count.is_some()
            || input
                .catalog_sha256
                .as_ref()
                .is_some_and(|v| v != &catalog.sha256)
            || input.catalog_count.is_some_and(|v| v != catalog.count)
        {
            return Err(Error::Conflict);
        }
        if !batch && catalog.count as usize != input.projects.len() {
            return Err(Error::Validation("Every source part requires an explicit current destination or an explicit pinned batch remainder".into()));
        }
        let mut source_ids = std::collections::BTreeSet::new();
        let mut file_ids = std::collections::BTreeSet::new();
        let mut projects = vec![];
        let mut permits = vec![];
        let mut target_checks = std::collections::BTreeMap::new();
        let mut metadata_bytes = 0usize;
        for mapping in &input.projects {
            if !source_ids.insert(mapping.source_project_id.clone()) {
                return Err(Error::Conflict);
            }
            let entry = super::catalog::find_catalog_entry(
                sqltx(&mut locked)?,
                keys,
                &owner,
                import,
                &mapping.source_project_id,
            )
            .await?;
            let mut source = entry.source;
            if source.source_id != mapping.source_project_id {
                return Err(Error::Conflict);
            }
            if entry.unsupported_reason.is_some()
                && !matches!(mapping.target, HistoricalProjectTarget::HistoryOnly { .. })
            {
                return Err(Error::Validation("Unsupported current-content mappings require explicit retained-history disposition".into()));
            }
            if !matches!(mapping.target, HistoricalProjectTarget::HistoryOnly { .. })
                && activated_part(sqltx(&mut locked)?, &owner, import, &source.source_id).await?
            {
                return Err(Error::Conflict);
            }
            if first_target(&mut target_checks, &mapping.target)? {
                if let Some(p) = current_target(&mut locked, &authority, &mapping.target).await? {
                    permits.push(p)
                }
            }
            if !source.files.is_empty() {
                source.orders.push(HistoricalContentOrder {
                    source_id: format!("historical-files:{}", source.source_id),
                    source_kind: "import_files".into(),
                    title: "Importierte Dateien".into(),
                    draft: "Dateien aus dem ausdrücklich ausgewählten historischen Inhalt.".into(),
                    messages: vec![],
                    artifacts: vec![],
                });
            }
            for f in &source.files {
                if !file_ids.insert(f.source_id.clone()) {
                    return Err(Error::Conflict);
                }
            }
            let p = HistoricalPreparedProject {
                project_id: projected_target(&mapping.target),
                order_ids: source.orders.iter().map(|_| id()).collect(),
                document_ids: source.documents.iter().map(|_| id()).collect(),
                file_ids: source.files.iter().map(|_| id()).collect(),
                source,
                target: mapping.target.clone(),
                content_pages: entry.pages,
                content_bytes: entry.content_bytes,
                unsupported_reason: entry.unsupported_reason,
            };
            metadata_bytes = metadata_bytes
                .checked_add(json_string(&p)?.len())
                .ok_or(Error::BudgetExceeded)?;
            if batch && metadata_bytes > 3 * 1024 * 1024 {
                return Err(Error::Validation("Select a smaller explicit batch; current preview metadata exceeds the native response bound".into()));
            }
            projects.push(p);
        }
        let organization = match &authority {
            HistoricalAuthority::Organization {
                organization_id, ..
            } => Some(*organization_id),
            _ => None,
        };
        check_project_capacity(
            sqltx(&mut locked)?,
            organization,
            &projects,
            &input.history_only_file_ids,
        )
        .await?;
        if source_ids.len() != input.projects.len()
            || input.history_only_file_ids.len() > file_ids.len()
        {
            return Err(Error::Conflict);
        }
        let dispositions = input
            .history_only_file_ids
            .iter()
            .collect::<std::collections::BTreeSet<_>>();
        if dispositions.len() != input.history_only_file_ids.len()
            || dispositions.iter().any(|f| !file_ids.contains(*f))
        {
            return Err(Error::Conflict);
        }
        let limit = lock_limit(sqltx(&mut locked)?, key_id.instance).await?;
        if limit.revision != input.expected_limit_revision {
            return Err(Error::Conflict);
        }
        let mut value = HistoricalImportPreview {
            id: Uuid::new_v4(),
            import_id: import,
            owner: owner.clone(),
            request_id: input.request_id,
            request_sha256: fingerprint,
            plan_sha256: String::new(),
            source_sha256: current.pins.quarantine_database.clone(),
            source_profile: current.source_profile.clone().ok_or(Error::Conflict)?,
            original_rows: current.original_rows.ok_or(Error::Conflict)?,
            limit: limit.clone(),
            projected_bytes: 0,
            projects,
            history_only_file_ids: input.history_only_file_ids,
            state: "preview".into(),
            catalog_sha256: input.catalog_sha256,
            catalog_count: input.catalog_count,
            remainder_history_only_reason: input.remainder_history_only_reason,
        };
        // Exact content plus duplicate current receipts/indexes are charged before publication.
        let preview_metadata_bytes = json_string(&value)?.len() as u64;
        value.projected_bytes = value
            .projects
            .iter()
            .try_fold(0u64, |total, p| {
                total
                    .checked_add(p.content_bytes)
                    .ok_or(Error::BudgetExceeded)
            })?
            .checked_mul(4)
            .and_then(|v| v.checked_add(preview_metadata_bytes * 2))
            .and_then(|v| v.checked_add(16384))
            .ok_or(Error::BudgetExceeded)?;
        bounded_batch_receipt(&value)?;
        value.plan_sha256 = plan_sha(&value)?;
        let bytes = serde_json::to_vec(&value).map_err(storage_error)?;
        if batch && bytes.len() > 3 * 1024 * 1024 {
            return Err(Error::Validation(
                "Select a smaller explicit batch; preview response exceeds its bound".into(),
            ));
        }
        reserve(
            &mut locked,
            &authority,
            &permit,
            key_id.instance,
            limit.revision,
            &format!("{}:plan", value.id),
            bytes.len() as u64 * 2 + 4096,
            &value.plan_sha256,
        )
        .await?;
        cipher::put_pages(
            sqltx(&mut locked)?,
            keys,
            key_id,
            &owner.locator(),
            import,
            &stream(&value.id.to_string()),
            &bytes,
        )
        .await?;
        let metadata = json_string(&(
            value.id,
            value.import_id,
            &value.owner,
            value.request_id,
            &value.request_sha256,
            &value.plan_sha256,
        ))?;
        sqlx::query("INSERT INTO historical_import_previews(id,import_id,owner,request_id,request_sha256,plan_sha256,state,payload) VALUES($1,$2,$3,$4,$5,$6,'preview',$7)").bind(value.id.to_string()).bind(import.to_string()).bind(owner.locator()).bind(value.request_id.to_string()).bind(&value.request_sha256).bind(&value.plan_sha256).bind(metadata).execute(&mut **sqltx(&mut locked)?).await.map_err(storage_error)?;
        let mut next = current.clone();
        next.state = "preview".into();
        next.revision += 1;
        save(sqltx(&mut locked)?, &current, &next).await?;
        for p in permits {
            locked
                .validate_content_in_transaction(&p)
                .await
                .map_err(identity_error)?;
        }
        recheck(&mut locked, &permit).await?;
        drop(locked);
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn historical_import_preview(
        &self,
        authority: HistoricalAuthority<'_>,
        keys: &dyn KeyStore,
        key_id: KeyId,
        preview: Uuid,
    ) -> Result<HistoricalImportPreview> {
        let identity = self.identity();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let mut locked = identity
            .lock_quota_in_transaction(&mut tx)
            .await
            .map_err(identity_error)?;
        let permit = authorize(&mut locked, &authority, key_id, ContentAction::Read).await?;
        let p = load_plan(sqltx(&mut locked)?, keys, &authority.owner(), preview).await?;
        authorize_plan_read(&mut locked, &authority, &p).await?;
        recheck(&mut locked, &permit).await?;
        drop(locked);
        tx.commit().await.map_err(storage_error)?;
        Ok(p)
    }
    pub async fn prepare_historical_file(
        &self,
        authority: HistoricalAuthority<'_>,
        keys: &dyn KeyStore,
        key_id: KeyId,
        preview: Uuid,
        input: HistoricalFileStageRequest,
    ) -> Result<HistoricalFileStagePermit> {
        if input.size
            > match authority {
                HistoricalAuthority::PersonalNative(_) => MAX_DOWNLOAD_BYTES,
                _ => crate::TEAM_BLOB_MAX_BYTES,
            }
            || !cowork_domain::processes::valid_sha256(&input.sha256)
            || input.backend_id.is_empty()
            || input.backend_id.len() > 200
        {
            return Err(Error::Validation(
                "Invalid current historical file staging request".into(),
            ));
        }
        let identity = self.identity();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let mut locked = identity
            .lock_quota_in_transaction(&mut tx)
            .await
            .map_err(identity_error)?;
        let permit = authorize(&mut locked, &authority, key_id, ContentAction::Read).await?;
        let p = load_plan(sqltx(&mut locked)?, keys, &authority.owner(), preview).await?;
        if p.history_only_file_ids.contains(&input.source_id) {
            return Err(Error::Conflict);
        }
        let (project, index) = p
            .projects
            .iter()
            .find_map(|p| {
                p.source
                    .files
                    .iter()
                    .position(|f| f.source_id == input.source_id)
                    .map(|i| (p, i))
            })
            .ok_or(Error::NotFound)?;
        if matches!(project.target, HistoricalProjectTarget::HistoryOnly { .. }) {
            return Err(Error::Forbidden);
        }
        let file = &project.source.files[index];
        if file.size.is_some_and(|n| n != input.size)
            || file.sha256.as_ref().is_some_and(|s| s != &input.sha256)
        {
            return Err(Error::Conflict);
        }
        let request = canonical_hash(&(
            "historical-file-stage/v1",
            &p.owner,
            p.id,
            &p.plan_sha256,
            &input,
            &project.project_id,
            &project.file_ids[index],
        ));
        if let Some(r)=sqlx::query("SELECT request_sha256,payload,state,original_unsent_receipt FROM historical_import_file_operations WHERE preview_id=$1 AND source_id=$2 AND owner=$3").bind(preview.to_string()).bind(&input.source_id).bind(p.owner.locator()).fetch_optional(&mut **sqltx(&mut locked)?).await.map_err(storage_error)?{
            if r.try_get::<String,_>("request_sha256").map_err(storage_error)?!=request{return Err(Error::Conflict)}let mut operation:HistoricalFileOperation=parse(&r.try_get::<String,_>("payload").map_err(storage_error)?)?;operation.state=r.try_get("state").map_err(storage_error)?;
            if r.try_get::<Option<String>,_>("original_unsent_receipt").map_err(storage_error)?.is_some(){return Err(Error::Conflict);}if operation.request_sha256!=request||operation.plan_sha256!=p.plan_sha256{return Err(Error::Conflict)}authorize_plan_read(&mut locked,&authority,&p).await?;recheck(&mut locked,&permit).await?;drop(locked);tx.commit().await.map_err(storage_error)?;return Ok(HistoricalFileStagePermit{operation,send:false})
        }
        if p.state != "preview" {
            return Err(Error::Conflict);
        }
        let write_permit = authorize(&mut locked, &authority, key_id, ContentAction::Write).await?;
        let target_permit = current_target(&mut locked, &authority, &project.target).await?;
        let operation = HistoricalFileOperation {
            preview_id: preview,
            import_id: p.import_id,
            plan_sha256: p.plan_sha256.clone(),
            owner: p.owner.clone(),
            source_id: input.source_id,
            target_id: project.file_ids[index].clone(),
            project_id: project.project_id.clone(),
            order_id: project.order_ids.last().cloned().ok_or(Error::Conflict)?,
            name: file.name.clone(),
            media_type: file.media_type.clone(),
            backend_id: input.backend_id,
            size: input.size,
            sha256: input.sha256,
            request_sha256: request,
            state: "prepared".into(),
            created_at: now(),
        };
        let limit = lock_limit(sqltx(&mut locked)?, key_id.instance).await?;
        if limit.revision != p.limit.revision {
            return Err(Error::Conflict);
        }
        reserve(
            &mut locked,
            &authority,
            &write_permit,
            key_id.instance,
            p.limit.revision,
            &format!("{}:file:{}", preview, hash(operation.source_id.as_bytes())),
            operation.size + json_string(&operation)?.len() as u64 * 4 + 4096,
            &operation.request_sha256,
        )
        .await?;
        sqlx::query("INSERT INTO historical_import_file_operations(preview_id,source_id,owner,request_sha256,state,payload,dispatch_admitted,reserved_bytes) VALUES($1,$2,$3,$4,'prepared',$5,0,$6)").bind(preview.to_string()).bind(&operation.source_id).bind(operation.owner.locator()).bind(&operation.request_sha256).bind(json_string(&operation)?).bind((operation.size+json_string(&operation)?.len() as u64*4+4096) as i64).execute(&mut **sqltx(&mut locked)?).await.map_err(storage_error)?;
        recheck(&mut locked, &write_permit).await?;
        recheck(&mut locked, &target_permit).await?;
        recheck(&mut locked, &permit).await?;
        drop(locked);
        tx.commit().await.map_err(storage_error)?;
        Ok(HistoricalFileStagePermit {
            operation,
            send: false,
        })
    }
    /// Rechecks the original admission immediately before IO. It never creates a resend permit.
    pub async fn validate_historical_file_admission(
        &self,
        authority: HistoricalAuthority<'_>,
        keys: &dyn KeyStore,
        key_id: KeyId,
        operation: &HistoricalFileOperation,
    ) -> Result<()> {
        let p = self
            .historical_import_preview(
                authority_ref(&authority),
                keys,
                key_id,
                operation.preview_id,
            )
            .await?;
        if operation.owner != authority.owner()
            || operation.plan_sha256 != p.plan_sha256
            || p.state != "preview"
        {
            return Err(Error::Conflict);
        }
        let identity = self.identity();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let mut locked = identity
            .lock_quota_in_transaction(&mut tx)
            .await
            .map_err(identity_error)?;
        let permit = authorize(&mut locked, &authority, key_id, ContentAction::Write).await?;
        let target = p
            .projects
            .iter()
            .find(|p| {
                p.project_id == operation.project_id && p.file_ids.contains(&operation.target_id)
            })
            .ok_or(Error::Conflict)?;
        let target_permit = current_target(&mut locked, &authority, &target.target).await?;
        let raw:String=sqlx::query_scalar("SELECT payload FROM historical_import_file_operations WHERE preview_id=$1 AND source_id=$2 AND owner=$3 AND request_sha256=$4 AND state IN('prepared','uncertain')").bind(operation.preview_id.to_string()).bind(&operation.source_id).bind(operation.owner.locator()).bind(&operation.request_sha256).fetch_optional(&mut **sqltx(&mut locked)?).await.map_err(storage_error)?.ok_or(Error::Conflict)?;
        let mut expected = operation.clone();
        expected.state = "prepared".into();
        if raw != json_string(&expected)? {
            return Err(Error::Conflict);
        }
        recheck(&mut locked, &target_permit).await?;
        recheck(&mut locked, &permit).await?;
        drop(locked);
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    pub async fn mark_historical_file_uncertain(
        &self,
        operation: &HistoricalFileOperation,
    ) -> Result<()> {
        let mut original = operation.clone();
        original.state = "prepared".into();
        let changed=sqlx::query("UPDATE historical_import_file_operations SET state='uncertain' WHERE preview_id=$1 AND source_id=$2 AND owner=$3 AND request_sha256=$4 AND payload=$5 AND state='prepared' AND dispatch_admitted=1 AND original_unsent_receipt IS NULL").bind(operation.preview_id.to_string()).bind(&operation.source_id).bind(operation.owner.locator()).bind(&operation.request_sha256).bind(json_string(&original)?).execute(&self.pool).await.map_err(storage_error)?.rows_affected();
        if changed == 0 {
            let state:Option<String>=sqlx::query_scalar("SELECT state FROM historical_import_file_operations WHERE preview_id=$1 AND source_id=$2 AND owner=$3 AND request_sha256=$4 AND dispatch_admitted=1 AND original_unsent_receipt IS NULL").bind(operation.preview_id.to_string()).bind(&operation.source_id).bind(operation.owner.locator()).bind(&operation.request_sha256).fetch_optional(&self.pool).await.map_err(storage_error)?;
            if !state.is_some_and(|s| matches!(s.as_str(), "uncertain" | "received")) {
                return Err(Error::Conflict);
            }
        }
        Ok(())
    }
    /// Original physical IO observer: immutable admission/confirmation only.
    /// Today's ACL/project revision is checked separately when content is adopted.
    pub async fn receive_historical_file(
        &self,
        operation: &HistoricalFileOperation,
        confirmed: HistoricalStagedFile,
    ) -> Result<()> {
        if confirmed.preview_id != operation.preview_id
            || confirmed.plan_sha256 != operation.plan_sha256
            || confirmed.source_id != operation.source_id
            || confirmed.target_id != operation.target_id
            || confirmed.project_id != operation.project_id
            || confirmed.size != operation.size
            || confirmed.sha256 != operation.sha256
        {
            return Err(Error::Conflict);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        // Serialize only this original operation. No fresh execution/content or
        // ciphertext permission is minted by a delayed IO acknowledgment.
        sqlx::query("UPDATE historical_import_file_operations SET state=state WHERE preview_id=$1 AND source_id=$2 AND owner=$3 AND request_sha256=$4")
            .bind(operation.preview_id.to_string()).bind(&operation.source_id).bind(operation.owner.locator()).bind(&operation.request_sha256).execute(&mut *tx).await.map_err(storage_error)?;
        let original=sqlx::query("SELECT import_id,owner,plan_sha256 FROM historical_import_previews WHERE id=$1 AND owner=$2")
            .bind(operation.preview_id.to_string()).bind(operation.owner.locator()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        if original
            .try_get::<String, _>("plan_sha256")
            .map_err(storage_error)?
            != operation.plan_sha256
            || original
                .try_get::<String, _>("import_id")
                .map_err(storage_error)?
                != operation.import_id.to_string()
        {
            return Err(Error::Conflict);
        }
        let r=sqlx::query("SELECT payload,state,original_receipt,dispatch_admitted,original_unsent_receipt FROM historical_import_file_operations WHERE preview_id=$1 AND source_id=$2 AND owner=$3 AND request_sha256=$4").bind(operation.preview_id.to_string()).bind(&operation.source_id).bind(operation.owner.locator()).bind(&operation.request_sha256).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        if r.try_get::<i64, _>("dispatch_admitted")
            .map_err(storage_error)?
            != 1
            || r.try_get::<Option<String>, _>("original_unsent_receipt")
                .map_err(storage_error)?
                .is_some()
        {
            return Err(Error::Conflict);
        }
        let mut expected = operation.clone();
        expected.state = "prepared".into();
        if r.try_get::<String, _>("payload").map_err(storage_error)? != json_string(&expected)? {
            return Err(Error::Conflict);
        }
        let payload = json_string(&confirmed.payload)?;
        let receipt = json_string(&confirmed.original_receipt)?;
        if payload.len() > 65536 || receipt.len() > 65536 {
            return Err(Error::BudgetExceeded);
        }
        let state: String = r.try_get("state").map_err(storage_error)?;
        if state == "received" {
            let stored=sqlx::query("SELECT payload,original_receipt FROM historical_import_activation_files WHERE preview_id=$1 AND source_id=$2 AND target_id=$3").bind(operation.preview_id.to_string()).bind(&operation.source_id).bind(&operation.target_id).fetch_one(&mut *tx).await.map_err(storage_error)?;
            if stored
                .try_get::<String, _>("payload")
                .map_err(storage_error)?
                != payload
                || stored
                    .try_get::<String, _>("original_receipt")
                    .map_err(storage_error)?
                    != receipt
            {
                return Err(Error::Conflict);
            }
        } else {
            if !matches!(state.as_str(), "prepared" | "uncertain") {
                return Err(Error::Conflict);
            }
            sqlx::query("INSERT INTO historical_import_activation_files(preview_id,source_id,target_id,payload,original_receipt) VALUES($1,$2,$3,$4,$5)").bind(operation.preview_id.to_string()).bind(&operation.source_id).bind(&operation.target_id).bind(payload).bind(&receipt).execute(&mut *tx).await.map_err(storage_error)?;
            let n=sqlx::query("UPDATE historical_import_file_operations SET state='received',original_receipt=$1 WHERE preview_id=$2 AND source_id=$3 AND owner=$4 AND request_sha256=$5 AND state=$6 AND original_receipt IS NULL").bind(receipt).bind(operation.preview_id.to_string()).bind(&operation.source_id).bind(operation.owner.locator()).bind(&operation.request_sha256).bind(state).execute(&mut *tx).await.map_err(storage_error)?.rows_affected();
            if n != 1 {
                return Err(Error::Conflict);
            }
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    pub async fn activate_historical_import(
        &self,
        authority: HistoricalAuthority<'_>,
        keys: &dyn KeyStore,
        key_id: KeyId,
        preview: Uuid,
        input: ActivateHistoricalImport,
    ) -> Result<HistoricalActivationReceipt> {
        if !input.explicit_activation {
            return Err(Error::Validation(
                "Explicit current-content activation is required".into(),
            ));
        }
        let identity = self.identity();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let mut locked = identity
            .lock_quota_in_transaction(&mut tx)
            .await
            .map_err(identity_error)?;
        let permit = authorize(&mut locked, &authority, key_id, ContentAction::Write).await?;
        let p = load_plan(sqltx(&mut locked)?, keys, &authority.owner(), preview).await?;
        if p.plan_sha256 != input.plan_sha256 {
            return Err(Error::Conflict);
        }
        if p.state == "active" {
            authorize_plan_read(&mut locked, &authority, &p).await?;
            let original = receipt_in(sqltx(&mut locked)?, &p).await?;
            recheck(&mut locked, &permit).await?;
            drop(locked);
            tx.commit().await.map_err(storage_error)?;
            return Ok(original);
        }
        if p.state != "preview" || input.expected_limit_revision != p.limit.revision {
            return Err(Error::Conflict);
        }
        let batch = p.remainder_history_only_reason.is_some();
        let catalog =
            super::catalog::catalog_manifest(sqltx(&mut locked)?, keys, &p.owner, p.import_id)
                .await?;
        if catalog.source_sha256 != p.source_sha256
            || p.catalog_sha256
                .as_ref()
                .is_some_and(|s| s != &catalog.sha256)
            || p.catalog_count.is_some_and(|n| n != catalog.count)
            || (batch && (p.catalog_sha256.is_none() || p.catalog_count.is_none()))
        {
            return Err(Error::Conflict);
        }
        let mut permits = vec![];
        let mut target_checks = std::collections::BTreeMap::new();
        let organization = match &authority {
            HistoricalAuthority::Organization {
                organization_id, ..
            } => Some(*organization_id),
            _ => None,
        };
        for target in &p.projects {
            if !matches!(target.target, HistoricalProjectTarget::HistoryOnly { .. })
                && activated_part(
                    sqltx(&mut locked)?,
                    &p.owner,
                    p.import_id,
                    &target.source.source_id,
                )
                .await?
            {
                return Err(Error::Conflict);
            }
            if first_target(&mut target_checks, &target.target)? {
                if let Some(permit) =
                    current_target(&mut locked, &authority, &target.target).await?
                {
                    permits.push(permit)
                }
            }
        }
        check_project_capacity(
            sqltx(&mut locked)?,
            organization,
            &p.projects,
            &p.history_only_file_ids,
        )
        .await?;
        // A changed destination or expired actor after staging cannot activate.
        let original = record(sqltx(&mut locked)?, p.import_id, &p.owner).await?;
        if original.state != "preview" {
            return Err(Error::Conflict);
        }
        reserve(
            &mut locked,
            &authority,
            &permit,
            key_id.instance,
            p.limit.revision,
            &format!("{}:activate", p.id),
            p.projected_bytes,
            &p.plan_sha256,
        )
        .await?;
        let mut objects = vec![];
        let stamp = now();
        for target in &p.projects {
            let first = objects.len();
            activate_project(sqltx(&mut locked)?, keys, &p, target, stamp, &mut objects).await?;
            if !matches!(target.target, HistoricalProjectTarget::HistoryOnly { .. }) {
                let receipt = json!({"format":"historical-current-part-activation/1","import_id":p.import_id,"owner":p.owner,"source_part_id":target.source.source_id,
                    "source_sha256":p.source_sha256,"catalog_sha256":p.catalog_sha256,"preview_id":p.id,"plan_sha256":p.plan_sha256,
                    "target":target.target,"project_id":target.project_id,"objects":&objects[first..],"activated_at":stamp});
                let encoded = json_string(&receipt)?;
                sqlx::query("INSERT INTO historical_import_activated_parts(import_id,owner,source_id,preview_id,part_receipt_sha256,part_receipt,activated_at) VALUES($1,$2,$3,$4,$5,$6,$7)")
                    .bind(p.import_id.to_string()).bind(p.owner.locator()).bind(&target.source.source_id).bind(p.id.to_string()).bind(hash(encoded.as_bytes())).bind(encoded).bind(stamp)
                    .execute(&mut **sqltx(&mut locked)?).await.map_err(storage_error)?;
            }
        }
        let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM historical_import_activated_parts WHERE import_id=$1 AND owner=$2")
            .bind(p.import_id.to_string()).bind(p.owner.locator()).fetch_one(&mut **sqltx(&mut locked)?).await.map_err(storage_error)?;
        let count = u32::try_from(count).map_err(storage_error)?;
        let remainder = catalog.count.checked_sub(count).ok_or(Error::Conflict)?;
        let receipt = HistoricalActivationReceipt {
            preview_id: p.id,
            import_id: p.import_id,
            plan_sha256: p.plan_sha256.clone(),
            source_sha256: p.source_sha256.clone(),
            owner: p.owner.clone(),
            activated_at: stamp,
            objects,
            charged_bytes: p.projected_bytes,
            catalog_sha256: p.catalog_sha256.clone(),
            remainder_history_only_reason: p.remainder_history_only_reason.clone(),
            remainder_parts: batch.then_some(remainder),
        };
        if batch && json_string(&receipt)?.len() > 3 * 1024 * 1024 {
            return Err(Error::Validation(
                "Original activation receipt exceeds the explicit batch bound".into(),
            ));
        }
        let changed=sqlx::query("UPDATE historical_import_previews SET state='active',original_receipt=$1 WHERE id=$2 AND owner=$3 AND plan_sha256=$4 AND state='preview' AND original_receipt IS NULL").bind(json_string(&receipt)?).bind(preview.to_string()).bind(p.owner.locator()).bind(&p.plan_sha256).execute(&mut **sqltx(&mut locked)?).await.map_err(storage_error)?.rows_affected();
        if changed != 1 {
            return Err(Error::Conflict);
        }
        let mut next = original.clone();
        next.state = if batch && remainder != 0 {
            "preview"
        } else {
            "active"
        }
        .into();
        next.revision += 1;
        save(sqltx(&mut locked)?, &original, &next).await?;
        for permit in permits {
            locked
                .validate_content_in_transaction(&permit)
                .await
                .map_err(identity_error)?;
        }
        recheck(&mut locked, &permit).await?;
        drop(locked);
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
    pub async fn historical_activation_receipt(
        &self,
        authority: HistoricalAuthority<'_>,
        keys: &dyn KeyStore,
        key_id: KeyId,
        preview: Uuid,
    ) -> Result<HistoricalActivationReceipt> {
        let identity = self.identity();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let mut locked = identity
            .lock_quota_in_transaction(&mut tx)
            .await
            .map_err(identity_error)?;
        let permit = authorize(&mut locked, &authority, key_id, ContentAction::Read).await?;
        let p = load_plan(sqltx(&mut locked)?, keys, &authority.owner(), preview).await?;
        authorize_plan_read(&mut locked, &authority, &p).await?;
        let receipt = receipt_in(sqltx(&mut locked)?, &p).await?;
        recheck(&mut locked, &permit).await?;
        drop(locked);
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
}
pub(super) async fn authorize_plan_read(
    locked: &mut LockedIdentityQuota<'_, '_, '_>,
    authority: &HistoricalAuthority<'_>,
    p: &HistoricalImportPreview,
) -> Result<()> {
    if let HistoricalAuthority::Organization {
        actor,
        organization_id,
        ..
    } = authority
    {
        for project in &p.projects {
            if matches!(project.target, HistoricalProjectTarget::HistoryOnly { .. }) {
                continue;
            }
            let id = Uuid::parse_str(&project.project_id).map_err(storage_error)?;
            let permit = locked
                .authorize_content_in_transaction(actor, *organization_id, id, ContentAction::Read)
                .await
                .map_err(identity_error)?;
            locked
                .validate_content_in_transaction(&permit)
                .await
                .map_err(identity_error)?;
        }
    }
    Ok(())
}
async fn receipt_in(
    tx: &mut Transaction<'_, Any>,
    p: &HistoricalImportPreview,
) -> Result<HistoricalActivationReceipt> {
    let raw:Option<String>=sqlx::query_scalar("SELECT original_receipt FROM historical_import_previews WHERE id=$1 AND owner=$2 AND state='active' AND plan_sha256=$3").bind(p.id.to_string()).bind(p.owner.locator()).bind(&p.plan_sha256).fetch_optional(&mut **tx).await.map_err(storage_error)?.flatten();
    let r: HistoricalActivationReceipt = parse(&raw.ok_or(Error::NotFound)?)?;
    if r.preview_id != p.id
        || r.import_id != p.import_id
        || r.plan_sha256 != p.plan_sha256
        || r.owner != p.owner
        || r.source_sha256 != p.source_sha256
    {
        return Err(Error::Conflict);
    }
    Ok(r)
}
async fn activate_project(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    plan: &HistoricalImportPreview,
    p: &HistoricalPreparedProject,
    stamp: i64,
    objects: &mut Vec<HistoricalActivatedObject>,
) -> Result<()> {
    if matches!(p.target, HistoricalProjectTarget::HistoryOnly { .. }) {
        objects.push(HistoricalActivatedObject {
            source_id: p.source.source_id.clone(),
            kind: "retained_history".into(),
            id: plan.import_id.to_string(),
            project_id: String::new(),
        });
        return Ok(());
    }
    let mut hydrated = p.clone();
    if let Some(first) = p.content_pages.first() {
        let bytes = cipher::get_pages(
            tx,
            keys,
            &plan.owner.locator(),
            plan.import_id,
            &first.stream,
        )
        .await?;
        let full: HistoricalContentProject =
            serde_json::from_slice(&bytes).map_err(storage_error)?;
        validate_content_part(&full, bytes.len())?;
        let plain_pages = bytes.chunks(CHUNK_BYTES).collect::<Vec<_>>();
        if p.content_pages.len() != plain_pages.len()
            || p.content_bytes != bytes.len() as u64
            || full.source_id != p.source.source_id
            || full.name != p.source.name
            || json_string(&full.files)? != json_string(&p.source.files)?
        {
            return Err(Error::Conflict);
        }
        for (i, (page, reference)) in plain_pages.iter().zip(&p.content_pages).enumerate() {
            if reference.stream != first.stream
                || reference.chunk_index != i as u32
                || reference.size != page.len() as u64
                || reference.sha256 != hash(page)
            {
                return Err(Error::Conflict);
            }
        }
        if full.orders.len() + usize::from(!full.files.is_empty()) != p.source.orders.len()
            || full.documents.len() != p.source.documents.len()
        {
            return Err(Error::Conflict);
        }
        for (o, h) in full.orders.iter().zip(&p.source.orders) {
            if o.source_id != h.source_id || o.source_kind != h.source_kind || o.title != h.title {
                return Err(Error::Conflict);
            }
        }
        for (d, h) in full.documents.iter().zip(&p.source.documents) {
            if d.source_id != h.source_id || d.title != h.title {
                return Err(Error::Conflict);
            }
        }
        hydrated.source.orders = full.orders;
        if !full.files.is_empty() {
            hydrated
                .source
                .orders
                .push(p.source.orders.last().cloned().ok_or(Error::Conflict)?)
        }
        hydrated.source.documents = full.documents;
    } else {
        return Err(Error::Conflict);
    }
    let p = &hydrated;
    if p.order_ids.len() != p.source.orders.len()
        || p.document_ids.len() != p.source.documents.len()
        || p.file_ids.len() != p.source.files.len()
    {
        return Err(Error::Conflict);
    }
    let object = |source: &str, kind: &str, id: &str| HistoricalActivatedObject {
        source_id: source.into(),
        kind: kind.into(),
        id: id.into(),
        project_id: p.project_id.clone(),
    };
    if matches!(p.target, HistoricalProjectTarget::NewPersonal) {
        let project = Project {
            id: p.project_id.clone(),
            name: p.source.name.clone(),
            revision: 1,
            created_at: stamp,
        };
        sqlx::query("INSERT INTO projects(id,payload) VALUES($1,$2)")
            .bind(&project.id)
            .bind(json_string(&project)?)
            .execute(&mut **tx)
            .await
            .map_err(storage_error)?;
        crate::productivity::product_change(
            tx,
            &project.id,
            "project",
            &project.id,
            false,
            serde_json::to_value(&project).map_err(storage_error)?,
        )
        .await?;
    }
    objects.push(object(&p.source.source_id, "project", &p.project_id));
    for (source, id) in p.source.orders.iter().zip(&p.order_ids) {
        match &plan.owner {
            HistoricalOwner::PersonalNative { .. } => {
                let order = Order {
                    id: id.clone(),
                    project_id: p.project_id.clone(),
                    title: source.title.clone(),
                    draft: source.draft.clone(),
                    revision: 1,
                    created_at: stamp,
                };
                sqlx::query(
                    "INSERT INTO orders(id,project_id,revision,payload) VALUES($1,$2,1,$3)",
                )
                .bind(id)
                .bind(&p.project_id)
                .bind(json_string(&order)?)
                .execute(&mut **tx)
                .await
                .map_err(storage_error)?;
                crate::history::index_document(
                    tx,
                    id,
                    id,
                    "order",
                    &format!("{}\n{}", order.title, order.draft),
                    stamp,
                )
                .await?;
                crate::productivity::product_change(
                    tx,
                    &p.project_id,
                    "order",
                    id,
                    false,
                    serde_json::to_value(&order).map_err(storage_error)?,
                )
                .await?;
                if !source.messages.is_empty() || !source.artifacts.is_empty() {
                    crate::productivity_archive::insert_imported_history_in_transaction(
                        tx,
                        ImportedProductHistory {
                            project_id: p.project_id.clone(),
                            order_id: id.clone(),
                            messages: original_messages(&source.messages),
                            artifacts: original_artifacts(&source.artifacts),
                            revision: 1,
                            imported_at: stamp,
                        },
                    )
                    .await?;
                }
            }
            HistoricalOwner::Organization {
                organization_id,
                creator_id,
                ..
            } => {
                let order = TeamOrder {
                    id: Uuid::parse_str(id).map_err(storage_error)?,
                    team_id: *organization_id,
                    project_id: Uuid::parse_str(&p.project_id).map_err(storage_error)?,
                    title: source.title.clone(),
                    prompt: source.draft.clone(),
                    revision: 1,
                    archived: source.source_kind.ends_with("_content_part"),
                    created_at: stamp,
                };
                let payload = json_string(&order)?;
                sqlx::query("INSERT INTO team_orders(id,team_id,project_id,creator_id,creation_key,request_sha,initial_receipt,payload) VALUES($1,$2,$3,$4,$5,$6,$7,$7)").bind(id).bind(organization_id.to_string()).bind(&p.project_id).bind(creator_id.to_string()).bind(format!("m9:{}:{id}",plan.id)).bind(hash(payload.as_bytes())).bind(payload).execute(&mut **tx).await.map_err(storage_error)?;
                let history = crate::import_basis::V1TeamImportedHistory {
                    import_receipt_id: plan.id.to_string(),
                    project_id: order.project_id,
                    order_id: order.id,
                    source_project_id: p.source.source_id.clone(),
                    messages: original_messages(&source.messages),
                    artifacts: original_artifacts(&source.artifacts),
                    attachments: vec![],
                    source_entities: vec![],
                    imported_at: stamp,
                    inert: true,
                };
                sqlx::query("INSERT INTO v1_team_import_history(order_id,team_id,project_id,payload) VALUES($1,$2,$3,$4)").bind(id).bind(organization_id.to_string()).bind(&p.project_id).bind(json_string(&history)?).execute(&mut **tx).await.map_err(storage_error)?;
            }
        }
        objects.push(object(&source.source_id, &source.source_kind, id));
    }
    for (source, id) in p.source.documents.iter().zip(&p.document_ids) {
        let provenance = crate::knowledge_provenance::imported_source(
            &(plan.import_id, &plan.source_sha256, &source.source_id),
            1,
            "m9-original-renderer-content",
        );
        match &plan.owner {
            HistoricalOwner::PersonalNative { .. } => {
                let document = KnowledgeDocument {
                    provenance,
                    id: id.clone(),
                    project_id: p.project_id.clone(),
                    kind: KnowledgeKind::Knowledge,
                    title: source.title.clone(),
                    content: source.content.clone(),
                    tags: vec!["m9-import".into()],
                    revision: 1,
                    created_at: stamp,
                    updated_at: stamp,
                };
                crate::productivity::save_object(tx, &p.project_id, id, "knowledge", 1, &document)
                    .await?;
                crate::knowledge_provenance::record(
                    tx,
                    &crate::knowledge_provenance::scope(&p.project_id, None),
                    id,
                    1,
                    &document.content,
                    &document.provenance,
                    &document,
                )
                .await?;
            }
            HistoricalOwner::Organization {
                organization_id, ..
            } => {
                let document = TeamDocument {
                    provenance,
                    id: Uuid::parse_str(id).map_err(storage_error)?,
                    team_id: *organization_id,
                    project_id: Uuid::parse_str(&p.project_id).map_err(storage_error)?,
                    kind: TeamDocumentKind::Knowledge,
                    name: source.title.clone(),
                    content: source.content.clone(),
                    revision: 1,
                    archived: false,
                };
                sqlx::query(
                    "INSERT INTO team_documents(id,team_id,project_id,payload) VALUES($1,$2,$3,$4)",
                )
                .bind(id)
                .bind(organization_id.to_string())
                .bind(&p.project_id)
                .bind(json_string(&document)?)
                .execute(&mut **tx)
                .await
                .map_err(storage_error)?;
                crate::knowledge_provenance::record(
                    tx,
                    &crate::knowledge_provenance::scope(&p.project_id, Some(*organization_id)),
                    id,
                    1,
                    &document.content,
                    &document.provenance,
                    &document,
                )
                .await?;
            }
        }
        objects.push(object(&source.source_id, "knowledge", id));
    }
    for (source, id) in p.source.files.iter().zip(&p.file_ids) {
        if plan.history_only_file_ids.contains(&source.source_id) {
            continue;
        }
        let r=sqlx::query("SELECT f.payload,f.original_receipt,o.payload AS operation,o.original_receipt AS operation_receipt FROM historical_import_activation_files f JOIN historical_import_file_operations o ON o.preview_id=f.preview_id AND o.source_id=f.source_id WHERE f.preview_id=$1 AND f.source_id=$2 AND f.target_id=$3 AND o.state='received' AND o.dispatch_admitted=1 AND o.original_unsent_receipt IS NULL AND o.owner=$4")
            .bind(plan.id.to_string()).bind(&source.source_id).bind(id).bind(plan.owner.locator()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or_else(||Error::Validation("Every selected original file requires confirmed native or object-store staging".into()))?;
        let op: HistoricalFileOperation =
            parse(&r.try_get::<String, _>("operation").map_err(storage_error)?)?;
        if op.preview_id != plan.id
            || op.plan_sha256 != plan.plan_sha256
            || op.source_id != source.source_id
            || op.target_id != *id
            || op.project_id != p.project_id
            || op.owner != plan.owner
            || r.try_get::<String, _>("original_receipt")
                .map_err(storage_error)?
                != r.try_get::<String, _>("operation_receipt")
                    .map_err(storage_error)?
        {
            return Err(Error::Conflict);
        }
        let value: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
        match &plan.owner {
            HistoricalOwner::PersonalNative { .. } => {
                let upload: FileUpload = serde_json::from_value(value).map_err(storage_error)?;
                if upload.id != *id
                    || upload.project_id != p.project_id
                    || upload.order_id != op.order_id
                    || upload.state != UploadState::Ready
                    || upload.revision != 2
                    || upload.file.sha256 != op.sha256
                    || upload.file.size != op.size
                    || upload.file.file_name != op.name
                {
                    return Err(Error::Conflict);
                }
                sqlx::query("INSERT INTO file_uploads(id,order_id,idempotency_key,request_hash,revision,created_at,payload) VALUES($1,$2,$3,$4,$5,$6,$7)").bind(id).bind(&upload.order_id).bind(format!("m9:{}:{}",plan.id,hash(source.source_id.as_bytes()))).bind(&op.request_sha256).bind(upload.revision).bind(upload.created_at).bind(json_string(&upload)?).execute(&mut **tx).await.map_err(storage_error)?;
            }
            HistoricalOwner::Organization {
                organization_id, ..
            } => {
                let blob: TeamBlob =
                    serde_json::from_value(value["blob"].clone()).map_err(storage_error)?;
                let version: TeamBlobVersion =
                    serde_json::from_value(value["version"].clone()).map_err(storage_error)?;
                if blob.id.to_string() != *id
                    || blob.team_id != *organization_id
                    || blob.project_id.to_string() != p.project_id
                    || blob.state != TeamBlobState::Ready
                    || blob.version != 1
                    || blob.revision != 2
                    || blob.size != op.size
                    || blob.sha256 != op.sha256
                    || blob.name != op.name
                    || blob.media_type != op.media_type
                    || version.blob_id != blob.id
                    || version.version != 1
                    || version.state != TeamBlobState::Ready
                    || version.size != op.size
                    || version.sha256 != op.sha256
                    || version.backend_id != op.backend_id
                    || version.object_key
                        != format!(
                            "cowork/v1/{}/{}/{}/1/{}",
                            organization_id, p.project_id, id, op.sha256
                        )
                {
                    return Err(Error::Conflict);
                }
                sqlx::query("INSERT INTO team_blobs(id,team_id,project_id,revision,payload) VALUES($1,$2,$3,2,$4)").bind(id).bind(organization_id.to_string()).bind(&p.project_id).bind(json_string(&blob)?).execute(&mut **tx).await.map_err(storage_error)?;
                sqlx::query(
                    "INSERT INTO team_blob_versions(blob_id,version,payload) VALUES($1,1,$2)",
                )
                .bind(id)
                .bind(json_string(&version)?)
                .execute(&mut **tx)
                .await
                .map_err(storage_error)?;
            }
        }
        objects.push(object(&source.source_id, "file", id));
    }
    Ok(())
}

fn validate_content_part(p: &HistoricalContentProject, bytes: usize) -> Result<()> {
    if bytes > 256 * 1024 * 1024
        || p.source_id.is_empty()
        || p.source_id.len() > 300
        || p.name.is_empty()
        || p.name.len() > 200
        || p.name.contains('\0')
    {
        return Err(Error::Conflict);
    }
    let mut identities = std::collections::BTreeSet::new();
    for o in &p.orders {
        if !identities.insert(("order", &o.source_id))
            || o.source_id.len() > 300
            || o.title.len() > 200
            || o.draft.len() > 128 * 1024
            || o.title.contains('\0')
            || o.draft.contains('\0')
            || o.messages.len() > 10000
            || o.artifacts.len() > 1000
            || json_string(&(&o.messages, &o.artifacts))?.len()
                > crate::productivity_archive::PRODUCTIVITY_ARCHIVE_MAX_BYTES - 4096
        {
            return Err(Error::Conflict);
        }
        let mut history_ids = std::collections::BTreeSet::new();
        for m in &o.messages {
            if !history_ids.insert(&m.id)
                || m.content.len() > 1024 * 1024
                || m.content.contains('\0')
                || !matches!(m.role.as_str(), "system" | "user" | "assistant" | "tool")
                || m.source_id
                    .as_ref()
                    .is_some_and(|v| v.is_empty() || v.len() > 200 || v.contains('\0'))
            {
                return Err(Error::Conflict);
            }
        }
        for a in &o.artifacts {
            if !history_ids.insert(&a.id)
                || a.content.len() > crate::productivity_archive::PRODUCTIVITY_ARCHIVE_MAX_BYTES
                || a.content.contains('\0')
                || a.name.len() > 300
                || a.size != a.content.len() as u64
                || a.sha256 != hash(a.content.as_bytes())
                || a.source_id
                    .as_ref()
                    .is_some_and(|v| v.is_empty() || v.len() > 200 || v.contains('\0'))
            {
                return Err(Error::Conflict);
            }
        }
    }
    for d in &p.documents {
        if !identities.insert(("document", &d.source_id))
            || d.title.len() > 200
            || d.content.len() > 128 * 1024
            || d.content.contains('\0')
            || d.title.contains('\0')
        {
            return Err(Error::Conflict);
        }
    }
    for f in &p.files {
        if !identities.insert(("file", &f.source_id))
            || f.source_id.len() > 300
            || f.name.is_empty()
            || f.name.len() > 300
            || f.name.contains(['\0', '/', '\\', '\r', '\n'])
        {
            return Err(Error::Conflict);
        }
    }
    Ok(())
}

fn original_messages(values: &[HistoricalContentMessage]) -> Vec<crate::ArchivedProductMessage> {
    values
        .iter()
        .map(|v| crate::ArchivedProductMessage {
            id: v.id.clone(),
            source_id: v.source_id.clone(),
            role: v.role.clone(),
            content: v.content.clone(),
            created_at: v.created_at,
        })
        .collect()
}
fn original_artifacts(values: &[HistoricalContentArtifact]) -> Vec<crate::ArchivedProductArtifact> {
    values
        .iter()
        .map(|v| crate::ArchivedProductArtifact {
            id: v.id.clone(),
            source_id: v.source_id.clone(),
            name: v.name.clone(),
            media_type: v.media_type.clone(),
            size: v.size,
            sha256: v.sha256.clone(),
            content: v.content.clone(),
            created_at: v.created_at,
        })
        .collect()
}
