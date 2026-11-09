use super::*;

impl Store {
    pub async fn reserve_upload(&self, order: &str, input: &ReserveUpload) -> Result<FileUpload> {
        valid_text(&input.idempotency_key, 128)?;
        valid_text(&input.file.file_name, 1024)?;
        if input.file.size > MAX_DOWNLOAD_BYTES
            || input.file.sha256.len() != 64
            || !input
                .file
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::Validation("Invalid upload metadata".into()));
        }
        let owner = self.order(order).await?;
        let record = FileUpload {
            id: id(),
            order_id: owner.id,
            project_id: owner.project_id,
            file: input.file.clone(),
            state: UploadState::Reserved,
            revision: 1,
            created_at: now(),
        };
        let request_hash = canonical_hash(input);
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        sqlx::query("INSERT INTO file_uploads(id,order_id,idempotency_key,request_hash,revision,created_at,payload) VALUES($1,$2,$3,$4,1,$5,$6) ON CONFLICT(order_id,idempotency_key) DO NOTHING")
            .bind(&record.id).bind(order).bind(&input.idempotency_key).bind(&request_hash).bind(record.created_at).bind(json_string(&record)?)
            .execute(&mut *tx).await.map_err(storage_error)?;
        let row = sqlx::query("SELECT request_hash,payload FROM file_uploads WHERE order_id=$1 AND idempotency_key=$2")
            .bind(order).bind(&input.idempotency_key).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if row
            .try_get::<String, _>("request_hash")
            .map_err(storage_error)?
            != request_hash
        {
            return Err(Error::Conflict);
        }
        let upload = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(upload)
    }

    pub async fn upload(&self, order: &str, upload: &str) -> Result<FileUpload> {
        let payload: Option<String> =
            sqlx::query_scalar("SELECT payload FROM file_uploads WHERE id=$1 AND order_id=$2")
                .bind(upload)
                .bind(order)
                .fetch_optional(&self.pool)
                .await
                .map_err(storage_error)?;
        parse(&payload.ok_or(Error::NotFound)?)
    }

    /// Call only after the immutable blob has been flushed and verified by the broker.
    pub async fn confirm_upload(&self, order: &str, upload: &str) -> Result<FileUpload> {
        let mut record = self.upload(order, upload).await?;
        if record.state == UploadState::Ready {
            return Ok(record);
        }
        let base = record.revision;
        record.state = UploadState::Ready;
        record.revision += 1;
        let changed = sqlx::query("UPDATE file_uploads SET revision=$1,payload=$2 WHERE id=$3 AND order_id=$4 AND revision=$5")
            .bind(record.revision).bind(json_string(&record)?).bind(upload).bind(order).bind(base)
            .execute(&self.pool).await.map_err(storage_error)?.rows_affected();
        if changed == 0 {
            let current = self.upload(order, upload).await?;
            if current.state != UploadState::Ready {
                return Err(Error::Conflict);
            }
            return Ok(current);
        }
        Ok(record)
    }

    pub async fn uploads(&self, order: &str, offset: u32, limit: u32) -> Result<UploadPage> {
        self.order(order).await?;
        if limit == 0 || limit > 100 || offset > 1_000_000 {
            return Err(Error::Validation("Invalid page".into()));
        }
        let rows = sqlx::query("SELECT payload FROM file_uploads WHERE order_id=$1 ORDER BY created_at DESC,id DESC LIMIT $2 OFFSET $3")
            .bind(order).bind(i64::from(limit)+1).bind(i64::from(offset)).fetch_all(&self.pool).await.map_err(storage_error)?;
        let next_offset = (rows.len() > limit as usize).then_some(offset + limit);
        let items = rows
            .into_iter()
            .take(limit as usize)
            .map(|r| parse(&r.try_get::<String, _>("payload").map_err(storage_error)?))
            .collect::<Result<Vec<_>>>()?;
        Ok(UploadPage { items, next_offset })
    }

    pub async fn existing_import_request(
        &self,
        input: &CreateImportRun,
    ) -> Result<Option<RunRecord>> {
        self.existing_run_request(
            &input.order_id,
            &input.idempotency_key,
            &canonical_hash(&json!({"import":input})),
        )
        .await
    }
    pub async fn create_import_run(
        &self,
        input: &CreateImportRun,
        spec: RunSpec,
    ) -> Result<RunRecord> {
        if spec.provider.is_some()
            || !spec
                .file_action
                .as_ref()
                .is_some_and(|t| t.name == "import_file" && t.arguments == json!(input.import))
        {
            return Err(Error::Forbidden);
        }
        let upload = self
            .upload(&input.order_id, &input.import.upload_id)
            .await?;
        if upload.state != UploadState::Ready
            || upload.project_id != spec.project_id
            || canonical_hash(&upload.file) != canonical_hash(&input.import.file)
        {
            return Err(Error::Conflict);
        }
        let request = RunRequest {
            workflow: None,
            order_id: input.order_id.clone(),
            base_revision: input.base_revision,
            idempotency_key: input.idempotency_key.clone(),
            prompt: input.prompt.clone(),
        };
        self.insert_run(
            &request,
            spec,
            canonical_hash(&json!({"import":input})),
            true,
            json!({"kind":"import","request":input}),
        )
        .await
    }
}
