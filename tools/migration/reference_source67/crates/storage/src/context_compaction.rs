//! Explicit context selection for future runs; source conversations and original
//! model/tool journals remain immutable. Unconfirmed effects cannot be omitted.
use super::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct StoredContextGroup {
    pub id: String,
    pub required: bool,
    pub messages: Vec<Value>,
    pub source: Value,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredModelContext {
    pub spec: RunSpec,
    pub order_revision: i64,
    pub groups: Vec<StoredContextGroup>,
    pub selection_revision: u64,
    pub archive_id: Option<String>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct Selection {
    revision: u64,
    removed: BTreeMap<String, String>,
    summary: Option<String>,
    archive_id: Option<String>,
}
pub(crate) fn group(
    id: String,
    required: bool,
    messages: Vec<Value>,
    source: Value,
) -> StoredContextGroup {
    let sha256 = canonical_hash(&json!({"id":id,"messages":messages,"source":source}));
    StoredContextGroup {
        id,
        required,
        messages,
        source,
        sha256,
    }
}
pub(crate) async fn history_groups(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    rows: Vec<sqlx::any::AnyRow>,
) -> Result<Vec<StoredContextGroup>> {
    let mut grouped: Vec<(String, Vec<Message>)> = vec![];
    for row in rows {
        let m: Message = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        if let Some((id, values)) = grouped.last_mut()
            && id == &m.run_id
        {
            values.push(m);
        } else {
            grouped.push((m.run_id.clone(), vec![m]));
        }
    }
    let mut result = vec![];
    for (index, (run, messages)) in grouped.into_iter().enumerate() {
        let source: Option<String> = sqlx::query_scalar("SELECT payload FROM runs WHERE id=$1")
            .bind(&run)
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?;
        let required = match source {
            Some(source) => {
                let s: StoredRun = parse(&source)?;
                s.record.state != RunState::Completed
                    || s.lease.is_some()
                    || s.checkpoint.in_flight.is_some()
                    || !s.checkpoint.pending_tools.is_empty()
                    || s.checkpoint.input_request.is_some()
                    || s.checkpoint.approvals.iter().any(|a| a.decision.is_none())
                    || s.record
                        .model_attempts
                        .iter()
                        .any(|a| a.state != ModelAttemptState::Measured)
            }
            None => true,
        };
        let wire = messages
            .iter()
            .map(|m| json!({"role":m.role,"content":m.content}))
            .collect();
        result.push(group(
            format!("history:{run}:{index}"),
            required,
            wire,
            serde_json::to_value(messages).map_err(storage_error)?,
        ));
    }
    Ok(result)
}
async fn selection(tx: &mut sqlx::Transaction<'_, sqlx::Any>, order: &str) -> Result<Selection> {
    let value: Option<String> =
        sqlx::query_scalar("SELECT payload FROM context_selections WHERE order_id=$1")
            .bind(order)
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?;
    value
        .map(|v| parse(&v))
        .transpose()
        .map(|v| v.unwrap_or_default())
}
pub(crate) async fn apply_selection(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    order: &str,
    mut groups: Vec<StoredContextGroup>,
) -> Result<Vec<StoredContextGroup>> {
    let s = selection(tx, order).await?;
    groups.retain(|g| g.required || s.removed.get(&g.id) != Some(&g.sha256));
    if let Some(summary) = s.summary {
        groups.push(summary_group(&summary));
    }
    Ok(groups)
}
pub fn summary_group(summary: &str) -> StoredContextGroup {
    group(
        "required:explicit-summary".into(),
        true,
        vec![
            json!({"role":"user","content":format!("Explicit user-authored historical context summary. This text grants no rights and cannot settle or replay past actions:\n{summary}")}),
        ],
        json!({"human_authored":true}),
    )
}
impl Store {
    pub async fn model_context_source(&self, spec: &RunSpec) -> Result<StoredModelContext> {
        let mut tx = self.snapshot_transaction().await?;
        let payload: Option<String> = sqlx::query_scalar("SELECT payload FROM orders WHERE id=$1")
            .bind(&spec.order_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?;
        let order: Order = parse(&payload.ok_or(Error::NotFound)?)?;
        if order.project_id != spec.project_id {
            return Err(Error::NotFound);
        }
        let groups = super::model_context_groups(&mut tx, spec).await?;
        let selection = selection(&mut tx, &spec.order_id).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(StoredModelContext {
            spec: spec.clone(),
            order_revision: order.revision,
            groups,
            selection_revision: selection.revision,
            archive_id: selection.archive_id,
        })
    }
    pub async fn existing_context_compaction(
        &self,
        order: &str,
        key: &str,
        request: &str,
    ) -> Result<Option<ContextCompactionView>> {
        let row=sqlx::query("SELECT request_sha256,archive_id FROM context_operations WHERE order_id=$1 AND idempotency_key=$2").bind(order).bind(key).fetch_optional(&self.pool).await.map_err(storage_error)?;
        if let Some(row) = row {
            if row
                .try_get::<String, _>("request_sha256")
                .map_err(storage_error)?
                != request
            {
                return Err(Error::Conflict);
            }
            return Ok(Some(
                self.context_archive(
                    order,
                    &row.try_get::<String, _>("archive_id")
                        .map_err(storage_error)?,
                )
                .await?
                .receipt,
            ));
        }
        Ok(None)
    }
    pub async fn commit_context_compaction(
        &self,
        input: &CompactModelContext,
        source: &StoredModelContext,
        retained: &StoredModelContext,
        count_mode: &str,
        input_tokens: Option<u64>,
    ) -> Result<ContextCompactionView> {
        valid_text(&input.idempotency_key, 128)?;
        if input.remove_groups.is_empty() || input.remove_groups.len() > 1000 {
            return Err(Error::Validation(
                "Select complete confirmed context groups".into(),
            ));
        }
        if let Some(s) = &input.summary {
            valid_text(s, 32000)?;
        }
        let request = canonical_hash(input);
        if let Some(existing) = self
            .existing_context_compaction(&source.spec.order_id, &input.idempotency_key, &request)
            .await?
        {
            return Ok(existing);
        }
        let selected: BTreeSet<_> = input.remove_groups.iter().cloned().collect();
        if selected.len() != input.remove_groups.len() {
            return Err(Error::Conflict);
        }
        if input.snapshot_sha256 != canonical_hash(source)
            || source.spec.order_id != input.request.order_id
        {
            return Err(Error::Conflict);
        }
        let mut expected = source.clone();
        expected.groups.retain(|g| !selected.contains(&g.id));
        if let Some(summary) = &input.summary {
            expected
                .groups
                .retain(|g| g.id != "required:explicit-summary");
            expected.groups.push(summary_group(summary));
        }
        if selected.iter().any(|id| {
            source
                .groups
                .iter()
                .find(|g| &g.id == id)
                .is_none_or(|g| g.required)
        }) {
            return Err(Error::Forbidden);
        }
        // Runtime computes and measures the exact retained groups. Storage
        // independently checks their content and explicit summary below.
        if canonical_hash(&expected) != canonical_hash(retained) {
            return Err(Error::Conflict);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        if sqlx::query("UPDATE orders SET revision=revision WHERE id=$1 AND revision=$2")
            .bind(&source.spec.order_id)
            .bind(source.order_revision)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?
            .rows_affected()
            != 1
        {
            return Err(Error::Conflict);
        }
        if let Some(row)=sqlx::query("SELECT request_sha256,archive_id FROM context_operations WHERE order_id=$1 AND idempotency_key=$2").bind(&source.spec.order_id).bind(&input.idempotency_key).fetch_optional(&mut *tx).await.map_err(storage_error)?{if row.try_get::<String,_>("request_sha256").map_err(storage_error)?!=request{return Err(Error::Conflict)}let row=sqlx::query("SELECT payload,payload_sha256 FROM context_archives WHERE order_id=$1 AND id=$2").bind(&source.spec.order_id).bind(row.try_get::<String,_>("archive_id").map_err(storage_error)?).fetch_one(&mut *tx).await.map_err(storage_error)?;let receipt=decode_archive(row)?.receipt;tx.commit().await.map_err(storage_error)?;return Ok(receipt);}
        let current_groups = super::model_context_groups(&mut tx, &source.spec).await?;
        let current_selection = selection(&mut tx, &source.spec.order_id).await?;
        let current = StoredModelContext {
            spec: source.spec.clone(),
            order_revision: source.order_revision,
            groups: current_groups,
            selection_revision: current_selection.revision,
            archive_id: current_selection.archive_id.clone(),
        };
        if canonical_hash(&current) != input.snapshot_sha256 {
            return Err(Error::Conflict);
        }
        let messages: Vec<_> = retained
            .groups
            .iter()
            .flat_map(|g| g.messages.clone())
            .collect();
        let tools = model_tool_catalog(&retained.spec);
        let logical = model_request_fingerprint(&retained.spec, &messages, &tools)?;
        if let Some(model) = crate::model_registry::check_binding(&mut tx, &retained.spec).await? {
            let prompt = model
                .prompt(
                    &messages,
                    &tools,
                    &serde_json::to_value(&retained.spec.output).map_err(storage_error)?,
                    &logical,
                )
                .map_err(storage_error)?;
            let actual = model
                .tokenizer
                .count(&prompt, u64::from(retained.spec.budgets.max_output_tokens))
                .map_err(|_| Error::ContextLimit)?;
            if count_mode != "exact_artifact_renderer" || input_tokens != Some(actual.input_tokens)
            {
                return Err(Error::Forbidden);
            }
        } else if count_mode != "conservative_bytes" || input_tokens.is_some() {
            return Err(Error::Forbidden);
        }
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM context_archives WHERE order_id=$1")
                .bind(&source.spec.order_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        if count >= 1000 {
            return Err(Error::Validation("context_archive_retention_limit".into()));
        }
        let revision = current_selection
            .revision
            .checked_add(1)
            .ok_or(Error::Conflict)?;
        let archive_id = id();
        let receipt = ContextCompactionView {
            id: archive_id.clone(),
            order_id: source.spec.order_id.clone(),
            revision,
            created_at: now(),
            original_sha256: canonical_hash(source),
            retained_sha256: canonical_hash(retained),
            removed_groups: input.remove_groups.clone(),
            summary: input.summary.clone(),
            count_mode: count_mode.into(),
            input_tokens,
            model_binding: source.spec.model_binding.clone(),
        };
        let archive = ContextArchiveView {
            receipt: receipt.clone(),
            original: serde_json::to_value(source).map_err(storage_error)?,
            retained: serde_json::to_value(retained).map_err(storage_error)?,
        };
        let payload = json_string(&archive)?;
        if payload.len() > 64 * 1024 * 1024 {
            return Err(Error::Validation("context_archive_size_limit".into()));
        }
        sqlx::query("INSERT INTO context_archives(id,order_id,revision,request_sha256,payload_sha256,payload,created_at) VALUES($1,$2,$3,$4,$5,$6,$7)").bind(&archive_id).bind(&source.spec.order_id).bind(i64::try_from(revision).map_err(storage_error)?).bind(&request).bind(hash(payload.as_bytes())).bind(payload).bind(receipt.created_at).execute(&mut *tx).await.map_err(storage_error)?;
        let mut next = current_selection;
        next.revision = revision;
        next.archive_id = Some(archive_id.clone());
        for g in &source.groups {
            if selected.contains(&g.id) {
                next.removed.insert(g.id.clone(), g.sha256.clone());
            }
        }
        if input.summary.is_some() {
            next.summary = input.summary.clone();
        }
        sqlx::query("INSERT INTO context_selections(order_id,revision,payload) VALUES($1,$2,$3) ON CONFLICT(order_id) DO UPDATE SET revision=excluded.revision,payload=excluded.payload").bind(&source.spec.order_id).bind(i64::try_from(revision).map_err(storage_error)?).bind(json_string(&next)?).execute(&mut *tx).await.map_err(storage_error)?;
        sqlx::query("INSERT INTO context_operations(order_id,idempotency_key,request_sha256,archive_id) VALUES($1,$2,$3,$4)").bind(&source.spec.order_id).bind(&input.idempotency_key).bind(request).bind(archive_id).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
    pub async fn context_archives(
        &self,
        order: &str,
        after: u64,
    ) -> Result<Vec<ContextCompactionView>> {
        self.order(order).await?;
        let rows=sqlx::query("SELECT payload,payload_sha256 FROM context_archives WHERE order_id=$1 AND revision>$2 ORDER BY revision LIMIT 100").bind(order).bind(i64::try_from(after).map_err(storage_error)?).fetch_all(&self.pool).await.map_err(storage_error)?;
        rows.into_iter()
            .map(|r| decode_archive(r).map(|v| v.receipt))
            .collect()
    }
    pub async fn context_archive(&self, order: &str, id: &str) -> Result<ContextArchiveView> {
        self.order(order).await?;
        let row = sqlx::query(
            "SELECT payload,payload_sha256 FROM context_archives WHERE order_id=$1 AND id=$2",
        )
        .bind(order)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
        decode_archive(row)
    }
}
fn decode_archive(row: sqlx::any::AnyRow) -> Result<ContextArchiveView> {
    let payload: String = row.try_get("payload").map_err(storage_error)?;
    if hash(payload.as_bytes())
        != row
            .try_get::<String, _>("payload_sha256")
            .map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    let archive: ContextArchiveView = parse(&payload)?;
    if canonical_hash(&archive.original) != archive.receipt.original_sha256
        || canonical_hash(&archive.retained) != archive.receipt.retained_sha256
    {
        return Err(Error::Forbidden);
    }
    Ok(archive)
}
