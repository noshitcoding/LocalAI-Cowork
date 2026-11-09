//! Durable document review. Run creation uses the same atomic journal as other file runs.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct StoredDocumentPreparation {
    pub view: DocumentPreparation,
    pub spec: RunSpec,
}

impl Store {
    pub async fn document_draft(&self, order: &str, draft: &str) -> Result<DocumentDraft> {
        let payload: Option<String> =
            sqlx::query_scalar("SELECT payload FROM document_drafts WHERE order_id=$1 AND id=$2")
                .bind(order)
                .bind(draft)
                .fetch_optional(&self.pool)
                .await
                .map_err(storage_error)?;
        parse(&payload.ok_or(Error::NotFound)?)
    }

    pub async fn document_draft_snapshots(
        &self,
        order: &str,
        draft: &str,
    ) -> Result<Vec<DocumentDraftSnapshot>> {
        self.document_draft(order, draft).await?;
        let rows = sqlx::query("SELECT payload FROM document_draft_snapshots WHERE order_id=$1 AND draft_id=$2 ORDER BY created_at DESC,id DESC LIMIT 50")
            .bind(order).bind(draft).fetch_all(&self.pool).await.map_err(storage_error)?;
        rows.iter()
            .map(|r| parse(&r.try_get::<String, _>("payload").map_err(storage_error)?))
            .collect()
    }

    pub async fn save_document_draft(
        &self,
        order: &str,
        request: &SaveDocumentDraft,
    ) -> Result<DocumentDraftSave> {
        valid_text(&request.id, 128)?;
        valid_text(&request.idempotency_key, 128)?;
        if request.base_revision < 0
            || serde_json::to_vec(&request.content)
                .map_err(storage_error)?
                .len()
                > 512 * 1024
        {
            return Err(Error::Validation(
                "Invalid document draft revision or content size".into(),
            ));
        }
        self.order(order).await?;
        let request_hash = canonical_hash(request);
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let inserted = sqlx::query("INSERT INTO document_draft_saves(order_id,idempotency_key,request_hash) VALUES($1,$2,$3) ON CONFLICT(order_id,idempotency_key) DO NOTHING")
            .bind(order).bind(&request.idempotency_key).bind(&request_hash).execute(&mut *tx).await.map_err(storage_error)?.rows_affected();
        if inserted == 0 {
            let row = sqlx::query("SELECT request_hash,payload FROM document_draft_saves WHERE order_id=$1 AND idempotency_key=$2")
                .bind(order).bind(&request.idempotency_key).fetch_one(&mut *tx).await.map_err(storage_error)?;
            if row
                .try_get::<String, _>("request_hash")
                .map_err(storage_error)?
                != request_hash
            {
                return Err(Error::Conflict);
            }
            let result = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(result);
        }
        let time = now();
        let candidate = DocumentDraft {
            id: request.id.clone(),
            order_id: order.into(),
            revision: request
                .base_revision
                .checked_add(1)
                .ok_or(Error::Conflict)?,
            sha256: canonical_hash(&request.content),
            content: request.content.clone(),
            updated_at: time,
        };
        let payload = json_string(&candidate)?;
        let saved = if request.base_revision == 0 {
            sqlx::query("INSERT INTO document_drafts(id,order_id,revision,payload) VALUES($1,$2,1,$3) ON CONFLICT(order_id,id) DO NOTHING")
                .bind(&request.id).bind(order).bind(&payload).execute(&mut *tx).await.map_err(storage_error)?.rows_affected() == 1
        } else {
            sqlx::query("UPDATE document_drafts SET revision=$1,payload=$2 WHERE order_id=$3 AND id=$4 AND revision=$5")
                .bind(candidate.revision).bind(&payload).bind(order).bind(&request.id).bind(request.base_revision)
                .execute(&mut *tx).await.map_err(storage_error)?.rows_affected() == 1
        };
        let current: Option<String> =
            sqlx::query_scalar("SELECT payload FROM document_drafts WHERE order_id=$1 AND id=$2")
                .bind(order)
                .bind(&request.id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?;
        let draft: DocumentDraft = parse(&current.ok_or(Error::NotFound)?)?;
        let snapshot = DocumentDraftSnapshot {
            id: id(),
            draft_id: request.id.clone(),
            order_id: order.into(),
            base_revision: request.base_revision,
            sha256: candidate.sha256,
            content: request.content.clone(),
            conflict: !saved,
            created_at: time,
        };
        sqlx::query("INSERT INTO document_draft_snapshots(id,order_id,draft_id,created_at,payload) VALUES($1,$2,$3,$4,$5)")
            .bind(&snapshot.id).bind(order).bind(&request.id).bind(time).bind(json_string(&snapshot)?)
            .execute(&mut *tx).await.map_err(storage_error)?;
        let result = DocumentDraftSave {
            saved,
            draft,
            snapshot,
        };
        sqlx::query(
            "UPDATE document_draft_saves SET payload=$1 WHERE order_id=$2 AND idempotency_key=$3",
        )
        .bind(json_string(&result)?)
        .bind(order)
        .bind(&request.idempotency_key)
        .execute(&mut *tx)
        .await
        .map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }

    pub async fn document_preparation_by_key(
        &self,
        order: &str,
        key: &str,
    ) -> Result<StoredDocumentPreparation> {
        valid_text(key, 128)?;
        let payload: Option<String> = sqlx::query_scalar(
            "SELECT payload FROM document_preparations WHERE order_id=$1 AND idempotency_key=$2",
        )
        .bind(order)
        .bind(key)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_error)?;
        parse(&payload.ok_or(Error::NotFound)?)
    }

    pub async fn existing_document_preparation(
        &self,
        order: &str,
        request: &DocumentActionRequest,
    ) -> Result<Option<StoredDocumentPreparation>> {
        let row = sqlx::query("SELECT request_hash,payload FROM document_preparations WHERE order_id=$1 AND idempotency_key=$2")
            .bind(order).bind(&request.idempotency_key).fetch_optional(&self.pool).await.map_err(storage_error)?;
        match row {
            None => Ok(None),
            Some(row) => {
                if row
                    .try_get::<String, _>("request_hash")
                    .map_err(storage_error)?
                    != canonical_hash(request)
                {
                    return Err(Error::Conflict);
                }
                Ok(Some(parse(
                    &row.try_get::<String, _>("payload").map_err(storage_error)?,
                )?))
            }
        }
    }

    pub async fn retain_document_preparation(
        &self,
        order: &str,
        request: &DocumentActionRequest,
        prepared: &StoredDocumentPreparation,
    ) -> Result<StoredDocumentPreparation> {
        if prepared.view.order_id != order || prepared.spec.order_id != order {
            return Err(Error::Forbidden);
        }
        sqlx::query("INSERT INTO document_preparations(id,order_id,idempotency_key,request_hash,payload) VALUES($1,$2,$3,$4,$5) ON CONFLICT(order_id,idempotency_key) DO NOTHING")
            .bind(&prepared.view.id).bind(order).bind(&request.idempotency_key).bind(canonical_hash(request)).bind(json_string(prepared)?)
            .execute(&self.pool).await.map_err(storage_error)?;
        self.existing_document_preparation(order, request)
            .await?
            .ok_or(Error::Conflict)
    }

    pub async fn document_preparation(
        &self,
        order: &str,
        preparation: &str,
    ) -> Result<StoredDocumentPreparation> {
        let payload: Option<String> = sqlx::query_scalar(
            "SELECT payload FROM document_preparations WHERE id=$1 AND order_id=$2",
        )
        .bind(preparation)
        .bind(order)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_error)?;
        parse(&payload.ok_or(Error::NotFound)?)
    }

    pub async fn existing_document_run(
        &self,
        prepared: &StoredDocumentPreparation,
    ) -> Result<Option<RunRecord>> {
        self.existing_run_request(
            &prepared.view.order_id,
            &prepared.view.idempotency_key,
            &prepared.view.fingerprint,
        )
        .await
    }

    pub async fn create_document_run(
        &self,
        prepared: &StoredDocumentPreparation,
    ) -> Result<RunRecord> {
        let view = &prepared.view;
        let spec = &prepared.spec;
        if spec.provider.is_some()
            || spec.model_binding.is_some()
            || spec.workflow.is_some()
            || !spec.mcp_tools.is_empty()
            || spec.environment.is_some()
            || spec.file_action.as_ref() != Some(&view.tool)
            || !cowork_domain::document_tool(&view.tool.name)
            || spec.allowed_tools != [view.tool.name.clone()]
        {
            return Err(Error::Forbidden);
        }
        let request = RunRequest {
            workflow: None,
            order_id: view.order_id.clone(),
            base_revision: view.base_revision,
            idempotency_key: view.idempotency_key.clone(),
            prompt: spec.prompt.clone(),
        };
        self.insert_run(
            &request,
            spec.clone(),
            view.fingerprint.clone(),
            true,
            json!({"kind":"document","preparation":view.id,"fingerprint":view.fingerprint}),
        )
        .await
    }
}
