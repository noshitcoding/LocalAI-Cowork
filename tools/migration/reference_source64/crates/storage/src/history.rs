use super::*;
use std::collections::{BTreeMap, HashSet};

const ACTIVE: &str = "('queued','waiting_for_executor','waiting_for_snapshot','running','waiting_approval','waiting_input','cancel_requested','interrupted')";

pub(super) async fn index_document(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    id: &str,
    order: &str,
    kind: &str,
    text: &str,
    created_at: i64,
) -> Result<()> {
    sqlx::query("INSERT INTO search_documents(id,order_id,kind,original,normalized,created_at) VALUES($1,$2,$3,$4,$5,$6)")
        .bind(id).bind(order).bind(kind).bind(text).bind(text.to_lowercase()).bind(created_at)
        .execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}

pub(super) fn verify_artifact(artifact: &Artifact) -> Result<()> {
    if artifact.size != artifact.content.len() as u64
        || artifact.sha256 != hash(artifact.content.as_bytes())
    {
        return Err(Error::Validation("Artifact integrity check failed".into()));
    }
    Ok(())
}

fn snapshot_hash(snapshot: &HistorySnapshot) -> String {
    let mut data = json!({"source_order_id":snapshot.source_order_id,
        "through_message_id":snapshot.through_message_id,"created_at":snapshot.created_at,
        "messages":snapshot.messages,"artifacts":snapshot.artifacts});
    // Preserve the digest of pre-workspace historical exports.
    if !snapshot.workspace_snapshots.is_empty() {
        data["workspace_snapshots"] = json!(snapshot.workspace_snapshots);
    }
    canonical_hash(&data)
}

pub(super) async fn read_history(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    order: &str,
) -> Result<OrderHistory> {
    let payload: Option<String> =
        sqlx::query_scalar("SELECT payload FROM order_history WHERE order_id=$1")
            .bind(order)
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?;
    let inherited: Option<HistorySnapshot> = payload.map(|p| parse(&p)).transpose()?;
    if let Some(snapshot) = &inherited {
        if snapshot.sha256 != snapshot_hash(snapshot) {
            return Err(Error::Validation(
                "History snapshot integrity check failed".into(),
            ));
        }
        for w in &snapshot.workspace_snapshots {
            verify_workspace_snapshot(w)?;
        }
        for a in &snapshot.artifacts {
            verify_artifact(a)?;
        }
    }
    Ok(OrderHistory { inherited })
}

async fn payloads<T: serde::de::DeserializeOwned>(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    query: &str,
    order: &str,
) -> Result<Vec<T>> {
    sqlx::query(query)
        .bind(order)
        .fetch_all(&mut **tx)
        .await
        .map_err(storage_error)?
        .into_iter()
        .map(|r| parse(&r.try_get::<String, _>("payload").map_err(storage_error)?))
        .collect()
}

impl Store {
    pub(crate) async fn snapshot_transaction(&self) -> Result<sqlx::Transaction<'_, sqlx::Any>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        if !self.sqlite {
            sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
                .execute(&mut *tx)
                .await
                .map_err(storage_error)?;
        }
        Ok(tx)
    }

    pub async fn history(&self, order: &str) -> Result<OrderHistory> {
        self.order(order).await?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let result = read_history(&mut tx, order).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }

    /// Captures one database snapshot, including event sequences and exact approvals.
    pub async fn export_order(&self, order: &str) -> Result<OrderExport> {
        let mut tx = self.snapshot_transaction().await?;
        let data = archive(&mut tx, order).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(OrderExport {
            format_version: 13,
            exported_at: now(),
            sha256: canonical_hash(&data),
            data,
        })
    }

    pub async fn branch_order(&self, source: &str, input: BranchOrder) -> Result<Order> {
        valid_text(&input.title, 300)?;
        valid_text(&input.idempotency_key, 128)?;
        valid_text(&input.through_message_id, 128)?;
        if let Some(order) = self.existing_branch(source, &input).await? {
            return Ok(order);
        }
        let result = self.branch_once(source, &input).await;
        if result.is_err() {
            // Handles a concurrent successful request, or a lost response, without a second branch.
            if let Some(order) = self.existing_branch(source, &input).await? {
                return Ok(order);
            }
        }
        result
    }

    async fn existing_branch(&self, source: &str, input: &BranchOrder) -> Result<Option<Order>> {
        let row = sqlx::query("SELECT order_id,request_hash FROM order_history WHERE source_order_id=$1 AND idempotency_key=$2")
            .bind(source).bind(&input.idempotency_key).fetch_optional(&self.pool).await.map_err(storage_error)?;
        if let Some(row) = row {
            let request_hash: String = row.try_get("request_hash").map_err(storage_error)?;
            if request_hash != canonical_hash(input) {
                return Err(Error::Conflict);
            }
            return Ok(Some(
                self.order(
                    &row.try_get::<String, _>("order_id")
                        .map_err(storage_error)?,
                )
                .await?,
            ));
        }
        Ok(None)
    }

    async fn branch_once(&self, source: &str, input: &BranchOrder) -> Result<Order> {
        let mut tx = self.snapshot_transaction().await?;
        let data = archive(&mut tx, source).await?;
        let mut messages = data
            .history
            .inherited
            .as_ref()
            .map(|s| s.messages.clone())
            .unwrap_or_default();
        messages.extend(data.messages);
        let cutoff = messages
            .iter()
            .position(|m| m.id == input.through_message_id)
            .ok_or(Error::NotFound)?;
        // Branches start at an observed assistant result; pending actions and approvals never transfer.
        if messages[cutoff].role != "assistant" {
            return Err(Error::Validation(
                "Choose an assistant result as the branch boundary".into(),
            ));
        }
        messages.truncate(cutoff + 1);
        let completed_context: HashSet<&str> = messages
            .iter()
            .filter(|m| m.role == "assistant")
            .map(|m| m.run_id.as_str())
            .collect();
        let mut workspace_snapshots = data
            .history
            .inherited
            .as_ref()
            .map(|s| s.workspace_snapshots.clone())
            .unwrap_or_default();
        workspace_snapshots.extend(
            data.runs
                .iter()
                .filter_map(|r| r.workspace_snapshot.clone()),
        );
        workspace_snapshots.retain(|w| completed_context.contains(w.run_id.as_str()));
        for run in &data.runs {
            if completed_context.contains(run.run.id.as_str())
                && run.run.spec.folders.iter().any(|f| f.order_id.is_some())
                && run.workspace_snapshot.is_none()
            {
                return Err(Error::Validation(
                    "Completed run is missing its workspace snapshot".into(),
                ));
            }
        }
        let mut artifacts = data
            .history
            .inherited
            .map(|s| s.artifacts)
            .unwrap_or_default();
        artifacts.extend(data.artifacts);
        artifacts.retain(|a| completed_context.contains(a.run_id.as_str()));
        let mut snapshot = HistorySnapshot {
            workspace_snapshots,
            source_order_id: source.into(),
            through_message_id: input.through_message_id.clone(),
            created_at: now(),
            sha256: String::new(),
            messages,
            artifacts,
        };
        snapshot.sha256 = snapshot_hash(&snapshot);
        let order = Order {
            id: id(),
            project_id: data.order.project_id,
            title: input.title.trim().into(),
            draft: String::new(),
            revision: 1,
            created_at: now(),
        };
        sqlx::query("INSERT INTO orders(id,project_id,revision,payload) VALUES($1,$2,$3,$4)")
            .bind(&order.id)
            .bind(&order.project_id)
            .bind(order.revision)
            .bind(json_string(&order)?)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        sqlx::query("INSERT INTO order_history(order_id,source_order_id,idempotency_key,request_hash,payload) VALUES($1,$2,$3,$4,$5)")
            .bind(&order.id).bind(source).bind(&input.idempotency_key).bind(canonical_hash(input)).bind(json_string(&snapshot)?)
            .execute(&mut *tx).await.map_err(storage_error)?;
        index_document(
            &mut tx,
            &order.id,
            &order.id,
            "order",
            &order.title,
            order.created_at,
        )
        .await?;
        for m in &snapshot.messages {
            index_document(
                &mut tx,
                &format!("{}:{}", order.id, m.id),
                &order.id,
                "message",
                &m.content,
                m.created_at,
            )
            .await?;
        }
        for a in &snapshot.artifacts {
            index_document(
                &mut tx,
                &format!("{}:{}", order.id, a.id),
                &order.id,
                "artifact",
                &format!("{}\n{}", a.name, a.content),
                a.created_at,
            )
            .await?;
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(order)
    }

    pub async fn activity(
        &self,
        attention_only: bool,
        offset: u32,
        limit: u32,
    ) -> Result<ActivityPage> {
        validate_page(offset, limit)?;
        for attempt in 0..12 {
            match self.activity_once(attention_only, offset, limit).await {
                Err(error) if workflows::product::retryable_projection(&error) && attempt < 11 => {
                    tokio::time::sleep(Duration::from_millis(5 * (attempt + 1))).await;
                }
                result => return result,
            }
        }
        Err(Error::Conflict)
    }
    async fn activity_once(
        &self,
        attention_only: bool,
        offset: u32,
        limit: u32,
    ) -> Result<ActivityPage> {
        let mut tx = self.snapshot_transaction().await?;
        workflows::product::refresh_all_attention(&mut tx).await?;
        let timestamp = now();
        let attention_predicate = "(r.state IN ('waiting_approval','waiting_input','interrupted') OR EXISTS(SELECT 1 FROM workflow_attention a WHERE a.run_id=r.id AND a.actionable_count>0 AND a.valid_until>$3))";
        let filter = if attention_only {
            attention_predicate.to_owned()
        } else {
            format!("r.state IN {ACTIVE}")
        };
        let total: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM runs WHERE state IN {ACTIVE}"
        ))
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        let attention: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM runs r WHERE r.state IN ('waiting_approval','waiting_input','interrupted') OR EXISTS(SELECT 1 FROM workflow_attention a WHERE a.run_id=r.id AND a.actionable_count>0 AND a.valid_until>$1)")
            .bind(timestamp).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let rows = sqlx::query(&format!("SELECT r.payload AS run,o.payload AS order_data,p.payload AS project FROM runs r JOIN orders o ON o.id=r.order_id JOIN projects p ON p.id=o.project_id WHERE {filter} ORDER BY CASE WHEN {attention_predicate} THEN 0 ELSE 1 END,r.updated_at DESC,r.id LIMIT $1 OFFSET $2"))
            .bind(i64::from(limit)+1).bind(i64::from(offset)).bind(timestamp).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let next_offset = (rows.len() > limit as usize).then_some(offset + limit);
        let mut items = Vec::new();
        for row in rows.into_iter().take(limit as usize) {
            let run: StoredRun = parse(&row.try_get::<String, _>("run").map_err(storage_error)?)?;
            let order: Order = parse(
                &row.try_get::<String, _>("order_data")
                    .map_err(storage_error)?,
            )?;
            let project: Project =
                parse(&row.try_get::<String, _>("project").map_err(storage_error)?)?;
            items.push(ActivityItem {
                run: run.record,
                project_name: project.name,
                order_title: order.title,
            });
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(ActivityPage {
            items,
            total,
            attention,
            next_offset,
        })
    }

    pub async fn search(
        &self,
        query: &str,
        project: Option<&str>,
        offset: u32,
        limit: u32,
    ) -> Result<SearchPage> {
        valid_text(query.trim(), 200)?;
        validate_page(offset, limit)?;
        let normalized = query.trim().to_lowercase();
        let pattern = format!(
            "%{}%",
            normalized
                .replace('!', "!!")
                .replace('%', "!%")
                .replace('_', "!_")
        );
        let rows = sqlx::query("SELECT s.id,s.kind,s.original,o.payload AS order_data,p.payload AS project FROM search_documents s JOIN orders o ON o.id=s.order_id JOIN projects p ON p.id=o.project_id WHERE s.normalized LIKE $1 ESCAPE '!' AND ($2='' OR o.project_id=$2) ORDER BY s.created_at DESC,s.id LIMIT $3 OFFSET $4")
            .bind(pattern).bind(project.unwrap_or("")).bind(i64::from(limit)+1).bind(i64::from(offset))
            .fetch_all(&self.pool).await.map_err(storage_error)?;
        let next_offset = (rows.len() > limit as usize).then_some(offset + limit);
        let mut items = Vec::new();
        for row in rows.into_iter().take(limit as usize) {
            let order: Order = parse(
                &row.try_get::<String, _>("order_data")
                    .map_err(storage_error)?,
            )?;
            let project: Project =
                parse(&row.try_get::<String, _>("project").map_err(storage_error)?)?;
            let kind = match row
                .try_get::<String, _>("kind")
                .map_err(storage_error)?
                .as_str()
            {
                "order" => SearchKind::Order,
                "message" => SearchKind::Message,
                "artifact" => SearchKind::Artifact,
                _ => return Err(Error::Validation("Unknown search document kind".into())),
            };
            items.push(SearchHit {
                id: row.try_get("id").map_err(storage_error)?,
                project_id: project.id,
                project_name: project.name,
                order_id: order.id,
                order_title: order.title,
                kind,
                excerpt: excerpt(
                    &row.try_get::<String, _>("original")
                        .map_err(storage_error)?,
                    &normalized,
                ),
            });
        }
        Ok(SearchPage { items, next_offset })
    }
}

fn validate_page(offset: u32, limit: u32) -> Result<()> {
    if limit == 0 || limit > 100 || offset > 1_000_000 {
        return Err(Error::Validation("Invalid pagination".into()));
    }
    Ok(())
}

fn excerpt(text: &str, needle: &str) -> String {
    let normalized_start = text.to_lowercase().find(needle).unwrap_or(0);
    let mut bytes = 0;
    let start = text
        .chars()
        .position(|c| {
            bytes += c.to_lowercase().map(char::len_utf8).sum::<usize>();
            bytes > normalized_start
        })
        .unwrap_or(0)
        .saturating_sub(60);
    let excerpt: String = text.chars().skip(start).take(300).collect();
    format!(
        "{}{}{}",
        if start > 0 { "…" } else { "" },
        excerpt,
        if text.chars().count() > start + 300 {
            "…"
        } else {
            ""
        }
    )
}

async fn archive(tx: &mut sqlx::Transaction<'_, sqlx::Any>, order: &str) -> Result<OrderArchive> {
    let order: Order = payloads(tx, "SELECT payload FROM orders WHERE id=$1", order)
        .await?
        .into_iter()
        .next()
        .ok_or(Error::NotFound)?;
    let project: Project = payloads(
        tx,
        "SELECT payload FROM projects WHERE id=$1",
        &order.project_id,
    )
    .await?
    .into_iter()
    .next()
    .ok_or(Error::NotFound)?;
    let history = read_history(tx, &order.id).await?;
    let messages: Vec<Message> = payloads(
        tx,
        "SELECT payload FROM messages WHERE order_id=$1 ORDER BY position",
        &order.id,
    )
    .await?;
    let artifacts: Vec<Artifact> = payloads(
        tx,
        "SELECT payload FROM artifacts WHERE order_id=$1 ORDER BY id",
        &order.id,
    )
    .await?;
    for a in &artifacts {
        verify_artifact(a)?;
    }
    let stored: Vec<StoredRun> = payloads(
        tx,
        "SELECT payload FROM runs WHERE order_id=$1 ORDER BY id",
        &order.id,
    )
    .await?;
    let mut workflows = Vec::new();
    for run in &stored {
        if run.record.spec.workflow.is_some() {
            let workflow = workflows::runs::load(tx, &run.record).await?;
            workflows.push(WorkflowSnapshot {
                run_id: workflow.run_id,
                project_id: workflow.project_id,
                binding: workflow.binding,
                progress_sha256: canonical_hash(&workflow.progress),
                plan: serde_json::to_value(&workflow.plan).map_err(storage_error)?,
                progress: serde_json::to_value(&workflow.progress).map_err(storage_error)?,
                human_inputs: if workflow.inputs.is_empty() {
                    None
                } else {
                    Some(serde_json::to_value(&workflow.inputs).map_err(storage_error)?)
                },
                agent_contexts: if workflow.agents.is_empty() {
                    None
                } else {
                    Some(serde_json::to_value(&workflow.agents).map_err(storage_error)?)
                },
            });
        }
    }
    let events: Vec<RunEvent> = payloads(tx,"SELECT e.payload FROM events e JOIN runs r ON r.id=e.run_id WHERE r.order_id=$1 ORDER BY e.run_id,e.sequence",&order.id).await?;
    for run in &stored {
        let run_events: Vec<_> = events
            .iter()
            .filter(|e| e.run_id == run.record.id)
            .collect();
        if run_events.len() as i64 != run.record.last_sequence
            || run_events
                .iter()
                .enumerate()
                .any(|(i, e)| e.sequence != i as i64 + 1)
        {
            return Err(Error::Validation(
                "Run event sequence integrity check failed".into(),
            ));
        }
    }
    let mut changes = BTreeMap::<String, Vec<FileChange>>::new();
    let file_changes:Vec<FileChange>=payloads(tx,"SELECT f.payload FROM file_changes f JOIN runs r ON r.id=f.run_id WHERE r.order_id=$1 ORDER BY f.run_id,f.tool_call_id",&order.id).await?;
    for change in file_changes {
        changes
            .entry(change.run_id.clone())
            .or_default()
            .push(change);
    }
    let snapshot_rows: Vec<WorkspaceSnapshot> = payloads(
        tx,
        "SELECT payload FROM workspace_snapshots WHERE order_id=$1 ORDER BY run_id",
        &order.id,
    )
    .await?;
    let mut snapshots = BTreeMap::new();
    for snapshot in snapshot_rows {
        verify_workspace_snapshot(&snapshot)?;
        snapshots.insert(snapshot.run_id.clone(), snapshot);
    }
    let operations: Vec<FileOperation> = payloads(tx,"SELECT f.payload FROM file_operations f JOIN runs r ON r.id=f.run_id WHERE r.order_id=$1 ORDER BY f.run_id,f.tool_call_id",&order.id).await?;
    let mut operations_by_run = BTreeMap::<String, Vec<FileOperation>>::new();
    for operation in operations {
        operations_by_run
            .entry(operation.run_id.clone())
            .or_default()
            .push(operation);
    }
    let mut processes_by_run = BTreeMap::new();
    let mut mcp_by_run = BTreeMap::new();
    for run in &stored {
        let rows = sqlx::query(
            "SELECT id,tool_call_id,revision,payload FROM mcp_attempts WHERE run_id=$1 ORDER BY id",
        )
        .bind(&run.record.id)
        .fetch_all(&mut **tx)
        .await
        .map_err(storage_error)?;
        mcp_by_run.insert(
            run.record.id.clone(),
            super::mcp::records(rows, &run.record)?,
        );
        let rows = sqlx::query(
            "SELECT id,tool_call_id,revision,payload FROM process_attempts WHERE run_id=$1 ORDER BY id",
        )
        .bind(&run.record.id)
        .fetch_all(&mut **tx)
        .await
        .map_err(storage_error)?;
        let attempts = super::processes::records(rows, &run.record)?;
        processes_by_run.insert(run.record.id.clone(), attempts);
    }
    let workflow_input_answer_records = super::workflow_input_drafts::read_verified_input_export(
        tx,
        &stored
            .iter()
            .map(|run| run.record.clone())
            .collect::<Vec<_>>(),
    )
    .await?;
    let workflow_input_resume_operations =
        super::workflow_operations::read_resume_export(tx, &stored).await?;
    let runs = stored
        .into_iter()
        .map(|s| RunDetail {
            mcp_attempts: mcp_by_run.remove(&s.record.id).unwrap_or_default(),
            process_attempts: processes_by_run.remove(&s.record.id).unwrap_or_default(),
            token_budget: token_budget(&s.record),
            file_operations: operations_by_run.remove(&s.record.id).unwrap_or_default(),
            uncertain_file_operation: s.checkpoint.pending_file_operation,
            workspace_snapshot: snapshots.remove(&s.record.id),
            uncertain_file_fingerprint: s
                .checkpoint
                .in_flight
                .as_ref()
                .filter(|tool| {
                    !matches!(
                        tool.name.as_str(),
                        "execute_program" | "browser" | "office" | "github"
                    ) && !tool.name.starts_with("mcp_")
                })
                .map(|tool| approval_fingerprint(&s.record, tool)),
            file_changes: changes.remove(&s.record.id).unwrap_or_default(),
            uncertain_file_change: s.checkpoint.pending_file_change,
            artifacts: artifacts
                .iter()
                .filter(|a| a.run_id == s.record.id)
                .cloned()
                .collect(),
            run: s.record,
            approvals: s.checkpoint.approvals,
            input_request: s.checkpoint.input_request,
        })
        .collect();
    let uploads = payloads(
        tx,
        "SELECT payload FROM file_uploads WHERE order_id=$1 ORDER BY created_at,id",
        &order.id,
    )
    .await?;
    Ok(OrderArchive {
        workflow_input_resume_operations,
        workflow_input_answer_records,
        workflows,
        project,
        order,
        history,
        messages,
        artifacts,
        runs,
        events,
        uploads,
    })
}
