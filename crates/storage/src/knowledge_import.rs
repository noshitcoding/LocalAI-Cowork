//! Atomic, replayable knowledge imports use the same documents and revision history as manual edits.
use super::*;
use crate::productivity::{
    begin_productivity, product_project, validate_knowledge, write_knowledge_in_tx,
};
pub use cowork_contracts::{KnowledgeImportRequest, KnowledgeImportResult};

impl Store {
    /// The receipt commits with every chunk, so an interrupted response can be retried safely.
    pub async fn import_knowledge(
        &self,
        project: &str,
        input: KnowledgeImportRequest,
    ) -> Result<KnowledgeImportResult> {
        if input.documents.is_empty() || input.documents.len() > 200 {
            return Err(Error::Validation("knowledge_import_chunk_limit".into()));
        }
        let mut bytes = 0usize;
        for document in &input.documents {
            validate_knowledge(document)?;
            if document.id.is_some()
                || document.expected_revision != 0
                || document.content.chars().count() > 4000
            {
                return Err(Error::Validation("invalid_knowledge_import_chunk".into()));
            }
            bytes = bytes
                .checked_add(document.content.len())
                .ok_or(Error::Conflict)?;
        }
        if bytes > 1_048_576 {
            return Err(Error::Validation("knowledge_import_size_limit".into()));
        }
        let fingerprint = canonical_hash(&("knowledge_import_v1", project, &input));
        let receipt = format!("knowledge:{project}:{}", input.idempotency_key);
        let mut tx = begin_productivity(self).await?;
        product_project(&mut tx, project).await?;
        if let Some(row) = sqlx::query(
            "SELECT request_hash,payload FROM productivity_imports WHERE idempotency_key=$1",
        )
        .bind(&receipt)
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?
        {
            if row
                .try_get::<String, _>("request_hash")
                .map_err(storage_error)?
                != fingerprint
            {
                return Err(Error::Conflict);
            }
            let result = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(result);
        }
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM productivity_objects WHERE project_id=$1 AND kind='knowledge'",
        )
        .bind(project)
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        if count + input.documents.len() as i64 > 2000 {
            return Err(Error::Validation("Project knowledge limit exceeded".into()));
        }
        let mut documents = Vec::with_capacity(input.documents.len());
        for document in input.documents {
            documents.push(write_knowledge_in_tx(&mut tx, project, document).await?);
        }
        let result = KnowledgeImportResult {
            project_id: project.into(),
            documents,
        };
        sqlx::query("INSERT INTO productivity_imports(idempotency_key,request_hash,payload) VALUES($1,$2,$3)")
            .bind(receipt).bind(fingerprint).bind(json_string(&result)?).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
}
