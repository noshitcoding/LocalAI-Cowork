//! Immutable document/source revisions. Only actual scoped snapshots earn the
//! SnapshotVerified label; a URI remains the author's declared citation.
use super::*;

pub(crate) async fn bind_sources(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    project: &str,
    team: Option<Uuid>,
    provenance: &mut KnowledgeProvenance,
) -> Result<()> {
    cowork_domain::knowledge_provenance::validate(provenance)?;
    for source in &mut provenance.sources {
        source.verification = KnowledgeSourceVerification::Declared;
        if let Some(request) = source
            .source_uri
            .as_ref()
            .and_then(|uri| uri.strip_prefix("urn:cowork:external-memory/"))
        {
            let request = Uuid::parse_str(request)
                .map_err(|_| Error::Validation("invalid_external_memory_source".into()))?;
            if team.is_none() {
                let raw:Option<String>=sqlx::query_scalar("SELECT receipt FROM external_memory_retrievals WHERE scope='installation-owner' AND request_id=$1 AND receipt IS NOT NULL").bind(request.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?;
                if let Some(raw) = raw {
                    let receipt: cowork_contracts::ExternalMemoryRetrieval = parse(&raw)?;
                    if receipt.response_sha256 != source.sha256
                        || receipt.provider_revision.to_string() != source.revision
                        || !receipt.items.iter().any(|item| item.id == source.locator)
                    {
                        return Err(Error::Conflict);
                    }
                    source.verification = KnowledgeSourceVerification::SnapshotVerified;
                }
            }
            continue;
        }
        if let Some(file) = &source.file_id {
            let revision: i64 = source
                .revision
                .parse()
                .map_err(|_| Error::Validation("invalid_source_revision".into()))?;
            let actual = if let Some(team) = team {
                let row = sqlx::query("SELECT v.payload FROM team_blob_versions v JOIN team_blobs b ON b.id=v.blob_id WHERE b.id=$1 AND b.team_id=$2 AND b.project_id=$3 AND v.version=$4")
                    .bind(file).bind(team.to_string()).bind(project).bind(revision).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
                let version: TeamBlobVersion =
                    parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
                if version.state != TeamBlobState::Ready {
                    return Err(Error::Conflict);
                }
                version.sha256
            } else {
                let row = sqlx::query("SELECT u.payload FROM file_uploads u JOIN orders o ON o.id=u.order_id WHERE u.id=$1 AND o.project_id=$2")
                    .bind(file).bind(project).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
                let upload: FileUpload =
                    parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
                if upload.project_id != project
                    || upload.revision != revision
                    || upload.state != UploadState::Ready
                {
                    return Err(Error::Conflict);
                }
                upload.file.sha256
            };
            if actual != source.sha256 {
                return Err(Error::Conflict);
            }
            source.verification = KnowledgeSourceVerification::SnapshotVerified;
        } else if let Some(artifact) = &source.artifact_id {
            let actual = if let Some(team) = team {
                let row = sqlx::query("SELECT a.payload FROM android_scoped_artifacts a JOIN android_scoped_runs r ON r.id=a.run_id WHERE a.id=$1 AND r.team_id=$2 AND r.project_id=$3")
                    .bind(artifact).bind(team.to_string()).bind(project).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
                let value: Value =
                    parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
                if value["deleted_at"] != Value::Null
                    || value["revision"].as_i64().map(|v| v.to_string()).as_deref()
                        != Some(&source.revision)
                {
                    return Err(Error::Conflict);
                }
                value["digest"].as_str().ok_or(Error::Conflict)?.to_owned()
            } else {
                let row = sqlx::query("SELECT a.payload FROM artifacts a JOIN orders o ON o.id=a.order_id WHERE a.id=$1 AND o.project_id=$2")
                    .bind(artifact).bind(project).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
                let value: Artifact =
                    parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
                if source.revision != "1" || hash(value.content.as_bytes()) != value.sha256 {
                    return Err(Error::Conflict);
                }
                value.sha256
            };
            if actual != source.sha256 {
                return Err(Error::Conflict);
            }
            source.verification = KnowledgeSourceVerification::SnapshotVerified;
        }
    }
    Ok(())
}
pub(crate) fn scope(project: &str, team: Option<Uuid>) -> String {
    team.map_or_else(
        || format!("personal:{project}"),
        |team| format!("team:{team}:{project}"),
    )
}
pub(crate) fn imported_source(
    source: &impl serde::Serialize,
    revision: i64,
    locator: &str,
) -> KnowledgeProvenance {
    let digest = canonical_hash(source);
    KnowledgeProvenance {
        format: 1,
        origin: KnowledgeOrigin::Imported,
        sources: vec![KnowledgeSource {
            format: 1,
            source_uri: Some(format!("urn:cowork:import:{digest}")),
            file_id: None,
            artifact_id: None,
            revision: revision.to_string(),
            sha256: digest,
            locator: locator.into(),
            verification: KnowledgeSourceVerification::Declared,
        }],
    }
}
pub(crate) fn archive_source(document: &KnowledgeDocument, project: &str) -> KnowledgeProvenance {
    let mut provenance = document.provenance.clone();
    provenance.origin = KnowledgeOrigin::Imported;
    for source in &mut provenance.sources {
        if let Some(id) = source.file_id.take().or_else(|| source.artifact_id.take()) {
            source.source_uri = Some(format!(
                "urn:cowork:archived-source:{}",
                canonical_hash(&json!({"project":project,"id":id,"revision":source.revision}))
            ));
        }
        source.verification = KnowledgeSourceVerification::Declared;
    }
    if provenance.sources.len() < 32 {
        provenance.sources.extend(
            imported_source(
                document,
                document.revision,
                &format!("document:{}", document.id),
            )
            .sources,
        );
    }
    provenance
}
pub(crate) async fn record(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    scope: &str,
    document_id: &str,
    revision: i64,
    content: &str,
    provenance: &KnowledgeProvenance,
    document: &impl serde::Serialize,
) -> Result<()> {
    cowork_domain::knowledge_provenance::validate(provenance)?;
    let value = KnowledgeRevision {
        document_id: document_id.into(),
        revision,
        content_sha256: hash(content.as_bytes()),
        provenance_sha256: canonical_hash(provenance),
        provenance: provenance.clone(),
        document: serde_json::to_value(document).map_err(storage_error)?,
        recorded_at: now(),
    };
    if let Some(row) = sqlx::query("SELECT payload_sha256,payload FROM knowledge_revisions WHERE scope=$1 AND document_id=$2 AND revision=$3")
        .bind(scope).bind(document_id).bind(revision).fetch_optional(&mut **tx).await.map_err(storage_error)? {
        let existing = read(&row)?;
        if existing.document != value.document || existing.provenance != value.provenance || existing.content_sha256 != value.content_sha256 { return Err(Error::Conflict); }
        return Ok(());
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM knowledge_revisions WHERE scope=$1 AND document_id=$2",
    )
    .bind(scope)
    .bind(document_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage_error)?;
    if count >= 10000 {
        return Err(Error::Validation(
            "knowledge_revision_retention_limit".into(),
        ));
    }
    sqlx::query("INSERT INTO knowledge_revisions(scope,document_id,revision,payload_sha256,payload) VALUES($1,$2,$3,$4,$5)")
        .bind(scope).bind(document_id).bind(revision).bind(canonical_hash(&value)).bind(json_string(&value)?).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
fn read(row: &sqlx::any::AnyRow) -> Result<KnowledgeRevision> {
    let value: KnowledgeRevision =
        parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    if canonical_hash(&value)
        != row
            .try_get::<String, _>("payload_sha256")
            .map_err(storage_error)?
        || value.provenance_sha256 != canonical_hash(&value.provenance)
    {
        return Err(Error::Conflict);
    }
    Ok(value)
}
pub(crate) async fn revisions(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    scope: &str,
    document: &str,
    after: i64,
    limit: i64,
) -> Result<Vec<KnowledgeRevision>> {
    if after < 0 || !(1..=100).contains(&limit) {
        return Err(Error::Validation("invalid_knowledge_revision_page".into()));
    }
    sqlx::query("SELECT payload_sha256,payload FROM knowledge_revisions WHERE scope=$1 AND document_id=$2 AND revision>$3 ORDER BY revision LIMIT $4")
        .bind(scope).bind(document).bind(after).bind(limit).fetch_all(&mut **tx).await.map_err(storage_error)?.iter().map(read).collect()
}
pub(crate) async fn all_revisions(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    scope: &str,
    document: &str,
) -> Result<Vec<KnowledgeRevision>> {
    sqlx::query("SELECT payload_sha256,payload FROM knowledge_revisions WHERE scope=$1 AND document_id=$2 ORDER BY revision LIMIT 10001")
        .bind(scope).bind(document).fetch_all(&mut **tx).await.map_err(storage_error)?.iter().map(read).collect()
}
pub(crate) async fn backfill_personal(tx: &mut sqlx::Transaction<'_, sqlx::Any>) -> Result<()> {
    for row in sqlx::query("SELECT payload FROM productivity_objects WHERE kind='knowledge'")
        .fetch_all(&mut **tx)
        .await
        .map_err(storage_error)?
    {
        let doc: KnowledgeDocument =
            parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        record(
            tx,
            &scope(&doc.project_id, None),
            &doc.id,
            doc.revision,
            &doc.content,
            &doc.provenance,
            &doc,
        )
        .await?;
    }
    Ok(())
}
pub(crate) async fn backfill_team(tx: &mut sqlx::Transaction<'_, sqlx::Any>) -> Result<()> {
    for row in sqlx::query("SELECT payload FROM team_documents")
        .fetch_all(&mut **tx)
        .await
        .map_err(storage_error)?
    {
        let doc: TeamDocument =
            parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        record(
            tx,
            &scope(&doc.project_id.to_string(), Some(doc.team_id)),
            &doc.id.to_string(),
            doc.revision,
            &doc.content,
            &doc.provenance,
            &doc,
        )
        .await?;
    }
    Ok(())
}
impl Store {
    pub async fn knowledge_revisions(
        &self,
        project: &str,
        document: &str,
        after: i64,
        limit: i64,
    ) -> Result<Vec<KnowledgeRevision>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        crate::productivity::product_project(&mut tx, project).await?;
        let result = revisions(&mut tx, &scope(project, None), document, after, limit).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
}
