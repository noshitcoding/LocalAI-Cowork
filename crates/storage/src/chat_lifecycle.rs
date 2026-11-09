use super::*;
pub const INDEPENDENT_CHATS: &str = "__cowork_independent_chats__";

impl Store {
    pub async fn project_all_chats(&self, project: &str) -> Result<Vec<Order>> {
        self.all_payloads("SELECT payload FROM orders WHERE project_id=$1 AND NOT EXISTS(SELECT 1 FROM native_chat_retirements r WHERE r.order_id=orders.id) ORDER BY id",Some(project)).await
    }
    /// Historical records remain readable; this check only grants live chat use.
    pub async fn native_chat_available(&self, order: &str) -> Result<()> {
        let exists: Option<String> = sqlx::query_scalar("SELECT id FROM orders WHERE id=$1 AND NOT EXISTS(SELECT 1 FROM native_chat_retirements r WHERE r.order_id=orders.id)")
            .bind(order).fetch_optional(&self.pool).await.map_err(storage_error)?;
        exists.map(|_| ()).ok_or(Error::NotFound)
    }
    pub async fn native_chat_deletion_receipt(
        &self,
        order: &str,
        input: &DeleteLibraryEntry,
    ) -> Result<Option<LibraryDeleted>> {
        valid_text(&input.idempotency_key, 128)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let saved = workspace_management::original(
            &mut tx,
            &format!("chat:{order}"),
            &input.idempotency_key,
            &canonical_hash(&json!(["delete", input])),
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(saved)
    }
    /// Retires a chat and its future work without deleting the run/audit graph.
    pub async fn delete_native_chat(
        &self,
        order: &str,
        input: &DeleteLibraryEntry,
    ) -> Result<LibraryDeleted> {
        valid_text(&input.idempotency_key, 128)?;
        // Same first lock as schedule claims/writes: a claimed occurrence cannot race deletion.
        let mut tx = productivity::begin_productivity(self).await?;
        let scope = format!("chat:{order}");
        let digest = canonical_hash(&json!(["delete", input]));
        if let Some(saved) =
            workspace_management::original(&mut tx, &scope, &input.idempotency_key, &digest).await?
        {
            tx.commit().await.map_err(storage_error)?;
            return Ok(saved);
        }
        let mut current = workspace_management::order_lock(&mut tx, order).await?;
        if current.revision != input.expected_revision {
            return Err(Error::Conflict);
        }
        workspace_management::no_active_runs(&mut tx, order).await?;
        let mut schedules = Vec::new();
        for row in sqlx::query("SELECT payload FROM productivity_schedules")
            .fetch_all(&mut *tx)
            .await
            .map_err(storage_error)?
        {
            let schedule: ProductSchedule =
                parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
            if schedule.template.order_id != order {
                continue;
            }
            if schedule.active_occurrence.is_some() {
                return Err(Error::WorkspaceBusy);
            }
            schedules.push(schedule);
        }
        current.revision = current.revision.checked_add(1).ok_or(Error::Conflict)?;
        workspace_management::put_order(&mut tx, &current, input.expected_revision).await?;
        sqlx::query(
            "INSERT INTO native_chat_retirements(order_id,revision,retired_at) VALUES($1,$2,$3)",
        )
        .bind(order)
        .bind(current.revision)
        .bind(now())
        .execute(&mut *tx)
        .await
        .map_err(storage_error)?;
        for row in sqlx::query(
            "SELECT payload FROM native_workspace_tasks WHERE order_id=$1 AND archived=0",
        )
        .bind(order)
        .fetch_all(&mut *tx)
        .await
        .map_err(storage_error)?
        {
            let mut task: WorkspaceTask =
                parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
            task.archived = true;
            task.revision = task.revision.checked_add(1).ok_or(Error::Conflict)?;
            task.updated_at = now();
            sqlx::query(
                "UPDATE native_workspace_tasks SET archived=1,revision=$1,payload=$2 WHERE id=$3",
            )
            .bind(task.revision)
            .bind(json_string(&task)?)
            .bind(&task.id)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        }
        for mut schedule in schedules {
            if schedule.paused {
                continue;
            }
            schedule.paused = true;
            schedule.revision = schedule.revision.checked_add(1).ok_or(Error::Conflict)?;
            schedule.updated_at = now();
            sqlx::query(
                "UPDATE productivity_schedules SET paused=1,revision=$1,payload=$2 WHERE id=$3",
            )
            .bind(schedule.revision)
            .bind(json_string(&schedule)?)
            .bind(&schedule.id)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        }
        let saved = LibraryDeleted {
            id: order.into(),
            revision: current.revision,
            deleted: true,
        };
        workspace_management::receipt(&mut tx, &scope, &input.idempotency_key, &digest, &saved)
            .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(saved)
    }
    /// Internal FK anchor, deliberately absent from the user's project list.
    pub async fn independent_chat_space(&self) -> Result<Project> {
        let project = Project {
            id: INDEPENDENT_CHATS.into(),
            name: "Chats".into(),
            revision: 1,
            created_at: now(),
        };
        sqlx::query("INSERT INTO projects(id,payload) VALUES($1,$2) ON CONFLICT(id) DO NOTHING")
            .bind(&project.id)
            .bind(json_string(&project)?)
            .execute(&self.pool)
            .await
            .map_err(storage_error)?;
        let payload: String = sqlx::query_scalar("SELECT payload FROM projects WHERE id=$1")
            .bind(INDEPENDENT_CHATS)
            .fetch_one(&self.pool)
            .await
            .map_err(storage_error)?;
        parse(&payload)
    }

    /// Deletes the project from the live workspace, detaches chats, disables
    /// schedules/tasks, and retains the immutable run/audit graph for exports.
    pub async fn delete_native_project(
        &self,
        project: &str,
        input: &DeleteLibraryEntry,
    ) -> Result<LibraryDeleted> {
        if project == INDEPENDENT_CHATS {
            return Err(Error::Forbidden);
        }
        valid_text(&input.idempotency_key, 128)?;
        self.independent_chat_space().await?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let scope = format!("delete-project:{project}");
        let digest = canonical_hash(input);
        if let Some(saved) =
            workspace_management::original(&mut tx, &scope, &input.idempotency_key, &digest).await?
        {
            tx.commit().await.map_err(storage_error)?;
            return Ok(saved);
        }
        let current = workspace_management::project_lock(&mut tx, project).await?;
        if current.revision != input.expected_revision {
            return Err(Error::Conflict);
        }
        let rows = sqlx::query("SELECT payload FROM orders WHERE project_id=$1 AND NOT EXISTS(SELECT 1 FROM native_chat_retirements r WHERE r.order_id=orders.id) ORDER BY id").bind(project).fetch_all(&mut *tx).await.map_err(storage_error)?;
        for row in rows {
            let mut order: Order =
                parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
            workspace_management::no_active_runs(&mut tx, &order.id).await?;
            let revision = order.revision;
            order.project_id = INDEPENDENT_CHATS.into();
            order.revision += 1;
            workspace_management::put_order(&mut tx, &order, revision).await?;
        }
        for row in sqlx::query(
            "SELECT id,payload FROM native_workspace_tasks WHERE project_id=$1 AND archived=0",
        )
        .bind(project)
        .fetch_all(&mut *tx)
        .await
        .map_err(storage_error)?
        {
            let mut task: WorkspaceTask =
                parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
            task.archived = true;
            task.revision += 1;
            task.updated_at = now();
            sqlx::query(
                "UPDATE native_workspace_tasks SET archived=1,revision=$1,payload=$2 WHERE id=$3",
            )
            .bind(task.revision)
            .bind(json_string(&task)?)
            .bind(&task.id)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        }
        for row in sqlx::query("SELECT id,payload FROM productivity_schedules WHERE project_id=$1")
            .bind(project)
            .fetch_all(&mut *tx)
            .await
            .map_err(storage_error)?
        {
            let id: String = row.try_get("id").map_err(storage_error)?;
            let mut value: Value =
                parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
            value["paused"] = json!(true);
            if let Some(revision) = value["revision"].as_i64() {
                value["revision"] = json!(revision + 1);
            }
            sqlx::query("UPDATE productivity_schedules SET paused=1,revision=revision+1,payload=$1 WHERE id=$2").bind(json_string(&value)?).bind(id).execute(&mut *tx).await.map_err(storage_error)?;
        }
        sqlx::query("INSERT INTO native_project_retirements(project_id,retired_at) VALUES($1,$2)")
            .bind(project)
            .bind(now())
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        let saved = LibraryDeleted {
            id: project.into(),
            revision: current.revision + 1,
            deleted: true,
        };
        workspace_management::receipt(&mut tx, &scope, &input.idempotency_key, &digest, &saved)
            .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(saved)
    }
}
