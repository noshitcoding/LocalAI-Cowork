//! Copied inert bytes. No path here is ever opened or handed to an executor.
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};

pub const SCHEMA: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS v1_import_attachments(id TEXT PRIMARY KEY,preview_id TEXT NOT NULL,order_id TEXT NOT NULL,project_id TEXT NOT NULL,scope TEXT NOT NULL CHECK(scope IN('personal','team')),team_id TEXT,source_id TEXT NOT NULL,metadata TEXT NOT NULL,content_base64 TEXT NOT NULL,state TEXT NOT NULL CHECK(state IN('preview','committed')),UNIQUE(preview_id,order_id,source_id))",
    "CREATE INDEX IF NOT EXISTS v1_import_attachment_order ON v1_import_attachments(scope,project_id,order_id,state)",
    "CREATE TABLE IF NOT EXISTS v1_team_import_receipts(id TEXT PRIMARY KEY,source_preview_id TEXT NOT NULL REFERENCES v1_import_receipts(id),team_id TEXT NOT NULL REFERENCES identity_organizations(id),project_id TEXT NOT NULL REFERENCES tenant_projects(id),creator_id TEXT NOT NULL REFERENCES identity_subjects(id),plan_sha256 TEXT NOT NULL,state TEXT NOT NULL CHECK(state IN('preview','committed')),payload TEXT NOT NULL)",
    "CREATE TABLE IF NOT EXISTS v1_team_import_history(order_id TEXT PRIMARY KEY REFERENCES team_orders(id),team_id TEXT NOT NULL REFERENCES identity_organizations(id),project_id TEXT NOT NULL REFERENCES tenant_projects(id),payload TEXT NOT NULL)",
];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct V1BinaryContent {
    pub source_id: String,
    pub name: String,
    pub media_type: String,
    pub size: u64,
    pub sha256: String,
    pub relative_path: String,
    pub capture_identity: String,
    pub content_base64: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct V1ImportedAttachment {
    pub id: String,
    pub source_id: String,
    pub name: String,
    pub media_type: String,
    pub size: u64,
    pub sha256: String,
    pub relative_path: String,
    pub capture_identity: String,
}
pub fn prepare(value: V1BinaryContent) -> Result<(V1ImportedAttachment, String)> {
    valid_text(&value.source_id, 300)?;
    valid_text(&value.name, 300)?;
    valid_text(&value.media_type, 120)?;
    valid_text(&value.capture_identity, 256)?;
    valid_text(&value.relative_path, 4096)?;
    if value.name.contains(['/', '\\', '\r', '\n'])
        || value.relative_path.starts_with('/')
        || value.relative_path.contains(['\\', ':'])
        || value
            .relative_path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || value.size > 16 * 1024 * 1024
        || value.content_base64.len() > 24 * 1024 * 1024
        || !cowork_domain::processes::valid_sha256(&value.sha256)
    {
        return Err(Error::Validation(
            "Invalid copied historical attachment".into(),
        ));
    }
    let bytes = STANDARD
        .decode(&value.content_base64)
        .map_err(storage_error)?;
    if bytes.len() as u64 != value.size
        || hash(&bytes) != value.sha256
        || STANDARD.encode(&bytes) != value.content_base64
    {
        return Err(Error::Conflict);
    }
    Ok((
        V1ImportedAttachment {
            id: id(),
            source_id: value.source_id,
            name: value.name,
            media_type: value.media_type,
            size: value.size,
            sha256: value.sha256,
            relative_path: value.relative_path,
            capture_identity: value.capture_identity,
        },
        value.content_base64,
    ))
}
pub async fn insert(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    preview: &str,
    order: &str,
    project: &str,
    scope: &str,
    team: Option<&str>,
    metadata: &V1ImportedAttachment,
    encoded: &str,
) -> Result<()> {
    sqlx::query("INSERT INTO v1_import_attachments(id,preview_id,order_id,project_id,scope,team_id,source_id,metadata,content_base64,state) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,'preview')")
        .bind(&metadata.id).bind(preview).bind(order).bind(project).bind(scope).bind(team)
        .bind(&metadata.source_id).bind(json_string(metadata)?).bind(encoded)
        .execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
pub async fn validate_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    preview: &str,
    scope: &str,
    expected: &[(String, String, V1ImportedAttachment)],
    expected_team: Option<&str>,
) -> Result<()> {
    let rows=sqlx::query("SELECT id,order_id,project_id,team_id,source_id,metadata,content_base64,state FROM v1_import_attachments WHERE preview_id=$1 AND scope=$2 ORDER BY id")
        .bind(preview).bind(scope).fetch_all(&mut **tx).await.map_err(storage_error)?;
    if rows.len() != expected.len() {
        return Err(Error::Conflict);
    }
    for row in rows {
        let identifier: String = row.try_get("id").map_err(storage_error)?;
        let order: String = row.try_get("order_id").map_err(storage_error)?;
        let metadata: V1ImportedAttachment = parse(
            &row.try_get::<String, _>("metadata")
                .map_err(storage_error)?,
        )?;
        let selected = expected
            .iter()
            .find(|(o, _, m)| o == &order && m.id == identifier)
            .ok_or(Error::Conflict)?;
        if json_string(&metadata)? != json_string(&selected.2)?
            || row
                .try_get::<String, _>("project_id")
                .map_err(storage_error)?
                != selected.1
            || row
                .try_get::<Option<String>, _>("team_id")
                .map_err(storage_error)?
                .as_deref()
                != expected_team
            || row
                .try_get::<String, _>("source_id")
                .map_err(storage_error)?
                != metadata.source_id
            || row.try_get::<String, _>("state").map_err(storage_error)? != "preview"
        {
            return Err(Error::Conflict);
        }
        decode(
            &metadata,
            &row.try_get::<String, _>("content_base64")
                .map_err(storage_error)?,
        )?;
    }
    Ok(())
}
pub fn decode(metadata: &V1ImportedAttachment, encoded: &str) -> Result<Vec<u8>> {
    if encoded.len() > 24 * 1024 * 1024 || metadata.size > 16 * 1024 * 1024 {
        return Err(Error::Conflict);
    }
    let bytes = STANDARD.decode(encoded).map_err(storage_error)?;
    if bytes.len() as u64 != metadata.size || hash(&bytes) != metadata.sha256 {
        return Err(Error::Conflict);
    }
    Ok(bytes)
}
impl Store {
    pub async fn v1_imported_attachments(
        &self,
        project: &str,
        order: &str,
    ) -> Result<Vec<V1ImportedAttachment>> {
        if sqlx::query("SELECT id FROM orders WHERE id=$1 AND project_id=$2")
            .bind(order)
            .bind(project)
            .fetch_optional(&self.pool)
            .await
            .map_err(storage_error)?
            .is_none()
        {
            return Err(Error::NotFound);
        }
        let rows=sqlx::query("SELECT metadata FROM v1_import_attachments WHERE scope='personal' AND project_id=$1 AND order_id=$2 AND state='committed' ORDER BY id")
            .bind(project).bind(order).fetch_all(&self.pool).await.map_err(storage_error)?;
        rows.into_iter()
            .map(|row| {
                parse(
                    &row.try_get::<String, _>("metadata")
                        .map_err(storage_error)?,
                )
            })
            .collect()
    }
    pub async fn v1_imported_attachment(
        &self,
        project: &str,
        order: &str,
        attachment: &str,
    ) -> Result<(V1ImportedAttachment, Vec<u8>)> {
        let row=sqlx::query("SELECT a.metadata,a.content_base64 FROM v1_import_attachments a JOIN orders o ON o.id=a.order_id AND o.project_id=a.project_id WHERE a.id=$1 AND a.project_id=$2 AND a.order_id=$3 AND a.scope='personal' AND a.state='committed'")
            .bind(attachment).bind(project).bind(order).fetch_optional(&self.pool).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        let metadata = parse(
            &row.try_get::<String, _>("metadata")
                .map_err(storage_error)?,
        )?;
        let bytes = decode(
            &metadata,
            &row.try_get::<String, _>("content_base64")
                .map_err(storage_error)?,
        )?;
        Ok((metadata, bytes))
    }
}
