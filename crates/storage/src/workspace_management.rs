use super::*;
use serde::de::DeserializeOwned;
use sqlx::{Any, Transaction};

fn key(value: &str) -> Result<()> {
    valid_text(value, 128)
}
fn bounded(value: &str, max: usize) -> Result<()> {
    if value.len() > max || value.contains('\0') {
        return Err(Error::Validation(
            "Workspace content exceeds its supported bounds".into(),
        ));
    }
    Ok(())
}
pub fn workspace_task_run_key(task: &str, key: &str) -> String {
    format!("task:{task}:{}", canonical_hash(&key))
}
pub(super) async fn original<T: DeserializeOwned>(
    tx: &mut Transaction<'_, Any>,
    scope: &str,
    key: &str,
    digest: &str,
) -> Result<Option<T>> {
    let row=sqlx::query("SELECT request_sha256,receipt FROM workspace_management_operations WHERE scope=$1 AND request_key=$2").bind(scope).bind(key).fetch_optional(&mut **tx).await.map_err(storage_error)?;
    if let Some(row) = row {
        if row
            .try_get::<String, _>("request_sha256")
            .map_err(storage_error)?
            != digest
        {
            return Err(Error::Conflict);
        }
        return Ok(Some(parse(
            &row.try_get::<String, _>("receipt").map_err(storage_error)?,
        )?));
    }
    Ok(None)
}
pub(super) async fn receipt<T: serde::Serialize>(
    tx: &mut Transaction<'_, Any>,
    scope: &str,
    key: &str,
    digest: &str,
    value: &T,
) -> Result<()> {
    sqlx::query("INSERT INTO workspace_management_operations(scope,request_key,request_sha256,receipt) VALUES($1,$2,$3,$4)").bind(scope).bind(key).bind(digest).bind(json_string(value)?).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
pub(super) async fn project_lock(tx: &mut Transaction<'_, Any>, project: &str) -> Result<Project> {
    if sqlx::query("UPDATE projects SET payload=payload WHERE id=$1 AND id NOT IN (SELECT project_id FROM native_project_retirements)")
        .bind(project)
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?
        .rows_affected()
        != 1
    {
        return Err(Error::NotFound);
    }
    let raw: String = sqlx::query_scalar("SELECT payload FROM projects WHERE id=$1")
        .bind(project)
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)?;
    parse(&raw)
}
pub(super) async fn order_lock(tx: &mut Transaction<'_, Any>, order: &str) -> Result<Order> {
    if sqlx::query("UPDATE orders SET revision=revision WHERE id=$1 AND NOT EXISTS(SELECT 1 FROM native_chat_retirements r WHERE r.order_id=orders.id)")
        .bind(order)
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?
        .rows_affected()
        != 1
    {
        return Err(Error::NotFound);
    }
    let raw: String = sqlx::query_scalar("SELECT payload FROM orders WHERE id=$1 AND NOT EXISTS(SELECT 1 FROM native_chat_retirements r WHERE r.order_id=orders.id)")
        .bind(order)
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?.ok_or(Error::NotFound)?;
    parse(&raw)
}
pub(super) async fn no_active_runs(tx: &mut Transaction<'_, Any>, order: &str) -> Result<()> {
    let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM runs WHERE order_id=$1 AND state NOT IN ('completed','failed','canceled','expired')").bind(order).fetch_one(&mut **tx).await.map_err(storage_error)?;
    if count > 0 {
        return Err(Error::WorkspaceBusy);
    }
    Ok(())
}
pub(super) async fn archived(tx: &mut Transaction<'_, Any>, order: &str) -> Result<bool> {
    let value: Option<i64> =
        sqlx::query_scalar("SELECT archived FROM native_chat_management WHERE order_id=$1")
            .bind(order)
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?;
    Ok(value == Some(1))
}
pub(super) async fn put_order(
    tx: &mut Transaction<'_, Any>,
    order: &Order,
    expected: i64,
) -> Result<()> {
    if sqlx::query(
        "UPDATE orders SET project_id=$1,revision=$2,payload=$3 WHERE id=$4 AND revision=$5",
    )
    .bind(&order.project_id)
    .bind(order.revision)
    .bind(json_string(order)?)
    .bind(&order.id)
    .bind(expected)
    .execute(&mut **tx)
    .await
    .map_err(storage_error)?
    .rows_affected()
        != 1
    {
        return Err(Error::Conflict);
    }
    sqlx::query(
        "UPDATE search_documents SET original=$1,normalized=$2 WHERE id=$3 AND kind='order'",
    )
    .bind(&order.title)
    .bind(order.title.to_lowercase())
    .bind(&order.id)
    .execute(&mut **tx)
    .await
    .map_err(storage_error)?;
    Ok(())
}

impl Store {
    pub async fn chat_configuration(&self, order: &str) -> Result<ChatConfiguration> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        order_lock(&mut tx, order).await?;
        let raw: Option<String> =
            sqlx::query_scalar("SELECT payload FROM native_chat_configuration WHERE order_id=$1")
                .bind(order)
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?;
        let value = raw
            .map(|value| parse(&value))
            .transpose()?
            .unwrap_or(ChatConfiguration {
                order_id: order.into(),
                revision: 0,
                updated_at: 0,
                value: None,
            });
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    /// Saves future-run intent only. Admission revalidates model and capability references.
    pub async fn write_chat_configuration(
        &self,
        order: &str,
        input: &WriteChatConfiguration,
    ) -> Result<ChatConfiguration> {
        key(&input.idempotency_key)?;
        validate_budgets(&input.value.budgets)?;
        crate::library_config::validate_effective_agent_config(&input.value.agent_config)?;
        match (&input.value.provider_id, input.value.provider_revision) {
            (Some(id), Some(revision)) if revision > 0 => valid_text(id, 200)?,
            (None, None) => {}
            _ => return Err(Error::Validation("Invalid saved model selection".into())),
        }
        if input.value.native_tools.len() > 4
            || input.value.folders.len() > 128
            || input.value.mcp_tools.len() > 128
            || json_string(&input.value)?.len() > 1_048_576
        {
            return Err(Error::Validation(
                "Chat configuration exceeds supported bounds".into(),
            ));
        }
        let mut tools = std::collections::BTreeSet::new();
        for tool in &input.value.native_tools {
            if !["execute_program", "browser", "office", "github"].contains(&tool.as_str())
                || !tools.insert(tool)
            {
                return Err(Error::Validation("Invalid saved native tool".into()));
            }
        }
        let mut selections = std::collections::BTreeSet::new();
        for tool in &input.value.mcp_tools {
            valid_text(&tool.server_id, 200)?;
            valid_text(&tool.tool_name, 200)?;
            if tool.server_revision < 1 || !selections.insert((&tool.server_id, &tool.tool_name)) {
                return Err(Error::Validation("Invalid saved MCP tool".into()));
            }
        }
        let mut folders = std::collections::BTreeSet::new();
        for folder in &input.value.folders {
            for value in [
                &folder.id,
                &folder.project_id,
                &folder.name,
                &folder.revision,
                &folder.resource_id,
            ] {
                valid_text(value, 300)?;
            }
            if !folders.insert(&folder.id) {
                return Err(Error::Validation("Duplicate saved folder".into()));
            }
        }
        if let Some(workflow) = &input.value.workflow {
            valid_text(&workflow.workflow_id, 200)?;
            valid_text(&workflow.version_id, 200)?;
            if let Some(inputs) = &workflow.inputs {
                cowork_domain::workflows::validate_data_value_budget(inputs)?;
            }
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        order_lock(&mut tx, order).await?;
        let scope = format!("chat-configuration:{order}");
        let digest = canonical_hash(input);
        if let Some(saved) = original(&mut tx, &scope, &input.idempotency_key, &digest).await? {
            tx.commit().await.map_err(storage_error)?;
            return Ok(saved);
        }
        if archived(&mut tx, order).await? {
            return Err(Error::Forbidden);
        }
        let current: Option<i64> =
            sqlx::query_scalar("SELECT revision FROM native_chat_configuration WHERE order_id=$1")
                .bind(order)
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?;
        if current.unwrap_or(0) != input.expected_revision {
            return Err(Error::Conflict);
        }
        let saved = ChatConfiguration {
            order_id: order.into(),
            revision: input.expected_revision + 1,
            updated_at: now(),
            value: Some(input.value.clone()),
        };
        sqlx::query("INSERT INTO native_chat_configuration(order_id,revision,payload) VALUES($1,$2,$3) ON CONFLICT(order_id) DO UPDATE SET revision=excluded.revision,payload=excluded.payload").bind(order).bind(saved.revision).bind(json_string(&saved)?).execute(&mut *tx).await.map_err(storage_error)?;
        receipt(&mut tx, &scope, &input.idempotency_key, &digest, &saved).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(saved)
    }
    pub async fn project_settings(&self, project: &str) -> Result<ProjectSettings> {
        let project_row: String = sqlx::query_scalar("SELECT payload FROM projects WHERE id=$1 AND id NOT IN (SELECT project_id FROM native_project_retirements)")
            .bind(project)
            .fetch_optional(&self.pool)
            .await
            .map_err(storage_error)?
            .ok_or(Error::NotFound)?;
        let current: Project = parse(&project_row)?;
        let raw: Option<String> =
            sqlx::query_scalar("SELECT payload FROM native_project_settings WHERE project_id=$1")
                .bind(project)
                .fetch_optional(&self.pool)
                .await
                .map_err(storage_error)?;
        let mut value = if let Some(raw) = raw {
            parse::<ProjectSettings>(&raw)?
        } else {
            ProjectSettings {
                project_id: project.into(),
                revision: current.revision,
                name: current.name.clone(),
                instructions: String::new(),
                sources: vec![],
                working_folder: None,
            }
        };
        value.revision = current.revision;
        value.name = current.name;
        Ok(value)
    }
    pub async fn write_project_settings(
        &self,
        project: &str,
        input: &WriteProjectSettings,
    ) -> Result<ProjectSettings> {
        key(&input.idempotency_key)?;
        valid_text(&input.name, 200)?;
        bounded(&input.instructions, 64000)?;
        if input.sources.len() > 128 {
            return Err(Error::Validation("Too many project sources".into()));
        }
        let mut ids = std::collections::BTreeSet::new();
        for source in &input.sources {
            valid_text(&source.id, 200)?;
            valid_text(&source.label, 300)?;
            valid_text(&source.reference, 8000)?;
            if !ids.insert(&source.id)
                || !["file", "folder", "link"].contains(&source.kind.as_str())
                || !["read_only", "read_write"].contains(&source.access.as_str())
                || source.kind == "link" && !source.reference.starts_with("https://")
            {
                return Err(Error::Validation("Invalid project source".into()));
            }
        }
        if let Some(folder) = &input.working_folder {
            valid_text(folder, 200)?;
            if !input
                .sources
                .iter()
                .any(|source| source.id == *folder && source.kind == "folder" && source.enabled)
            {
                return Err(Error::Validation(
                    "Working folder must reference an enabled folder source".into(),
                ));
            }
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let mut current = project_lock(&mut tx, project).await?;
        let scope = format!("project:{project}");
        let digest = canonical_hash(input);
        if let Some(saved) = original(&mut tx, &scope, &input.idempotency_key, &digest).await? {
            tx.commit().await.map_err(storage_error)?;
            return Ok(saved);
        }
        if current.revision != input.expected_revision {
            return Err(Error::Conflict);
        }
        current.revision += 1;
        current.name = input.name.trim().into();
        let result = ProjectSettings {
            project_id: project.into(),
            revision: current.revision,
            name: current.name.clone(),
            instructions: input.instructions.clone(),
            sources: input.sources.clone(),
            working_folder: input.working_folder.clone(),
        };
        sqlx::query("UPDATE projects SET payload=$1 WHERE id=$2")
            .bind(json_string(&current)?)
            .bind(project)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        sqlx::query("INSERT INTO native_project_settings(project_id,payload) VALUES($1,$2) ON CONFLICT(project_id) DO UPDATE SET payload=excluded.payload").bind(project).bind(json_string(&result)?).execute(&mut *tx).await.map_err(storage_error)?;
        receipt(&mut tx, &scope, &input.idempotency_key, &digest, &result).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn archived_chats(&self, project: &str) -> Result<Vec<ChatManagement>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        project_lock(&mut tx, project).await?;
        let rows = sqlx::query("SELECT o.payload FROM orders o JOIN native_chat_management m ON m.order_id=o.id WHERE o.project_id=$1 AND m.archived=1 AND NOT EXISTS(SELECT 1 FROM native_chat_retirements r WHERE r.order_id=o.id) ORDER BY o.id LIMIT 501")
            .bind(project).fetch_all(&mut *tx).await.map_err(storage_error)?;
        rows.into_iter()
            .map(|row| {
                Ok(ChatManagement {
                    order: parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?,
                    archived: true,
                })
            })
            .collect()
    }
    pub async fn chat_management(&self, order: &str) -> Result<ChatManagement> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let order = order_lock(&mut tx, order).await?;
        let result = ChatManagement {
            archived: archived(&mut tx, &order.id).await?,
            order,
        };
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn manage_chat(&self, order: &str, input: &ManageChat) -> Result<ChatManagement> {
        key(&input.idempotency_key)?;
        valid_text(&input.title, 300)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        project_lock(&mut tx, &input.project_id).await?;
        let mut current = order_lock(&mut tx, order).await?;
        let scope = format!("chat:{order}");
        let digest = canonical_hash(&json!(["manage", input]));
        if let Some(saved) = original(&mut tx, &scope, &input.idempotency_key, &digest).await? {
            tx.commit().await.map_err(storage_error)?;
            return Ok(saved);
        }
        if current.revision != input.expected_revision {
            return Err(Error::Conflict);
        }
        if current.project_id != input.project_id {
            no_active_runs(&mut tx, order).await?;
            let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM native_workspace_tasks WHERE project_id<>$1 AND archived=0 AND order_id=$2").bind(&input.project_id).bind(order).fetch_one(&mut *tx).await.map_err(storage_error)?;
            if count > 0 {
                return Err(Error::Validation(
                    "Archive tasks linked to this chat before moving it".into(),
                ));
            }
        }
        current.project_id = input.project_id.clone();
        current.title = input.title.trim().into();
        current.revision += 1;
        put_order(&mut tx, &current, input.expected_revision).await?;
        let result = ChatManagement {
            archived: archived(&mut tx, order).await?,
            order: current,
        };
        receipt(&mut tx, &scope, &input.idempotency_key, &digest, &result).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn archive_chat(&self, order: &str, input: &ArchiveChat) -> Result<ChatManagement> {
        key(&input.idempotency_key)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let mut current = order_lock(&mut tx, order).await?;
        let scope = format!("chat:{order}");
        let digest = canonical_hash(&json!(["archive", input]));
        if let Some(saved) = original(&mut tx, &scope, &input.idempotency_key, &digest).await? {
            tx.commit().await.map_err(storage_error)?;
            return Ok(saved);
        }
        if current.revision != input.expected_revision {
            return Err(Error::Conflict);
        }
        no_active_runs(&mut tx, order).await?;
        current.revision += 1;
        put_order(&mut tx, &current, input.expected_revision).await?;
        sqlx::query("INSERT INTO native_chat_management(order_id,archived) VALUES($1,$2) ON CONFLICT(order_id) DO UPDATE SET archived=excluded.archived").bind(order).bind(if input.archived{1_i64}else{0_i64}).execute(&mut *tx).await.map_err(storage_error)?;
        let result = ChatManagement {
            order: current,
            archived: input.archived,
        };
        receipt(&mut tx, &scope, &input.idempotency_key, &digest, &result).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn workspace_tasks(&self, project: &str) -> Result<Vec<WorkspaceTask>> {
        self.project_settings(project).await?;
        let rows=sqlx::query("SELECT payload FROM native_workspace_tasks WHERE project_id=$1 AND archived=0 ORDER BY id").bind(project).fetch_all(&self.pool).await.map_err(storage_error)?;
        let mut result = vec![];
        for row in rows {
            let mut task: WorkspaceTask =
                parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
            task.last_run_id = self.workspace_task_last_run(&task.id).await?;
            result.push(task);
        }
        Ok(result)
    }
    async fn workspace_task_last_run(&self, task: &str) -> Result<Option<String>> {
        sqlx::query_scalar("SELECT run_id FROM (SELECT run_id,created_at,request_key FROM native_workspace_task_runs WHERE task_id=$1 AND run_id IS NOT NULL UNION ALL SELECT run_id,received_at AS created_at,id AS request_key FROM automation_events WHERE task_id=$1 AND run_id IS NOT NULL) history ORDER BY created_at DESC,request_key DESC LIMIT 1").bind(task).fetch_optional(&self.pool).await.map_err(storage_error)
    }
    pub async fn workspace_task(&self, id: &str) -> Result<WorkspaceTask> {
        let raw: Option<String> =
            sqlx::query_scalar("SELECT payload FROM native_workspace_tasks WHERE id=$1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await
                .map_err(storage_error)?;
        let mut task: WorkspaceTask = parse(&raw.ok_or(Error::NotFound)?)?;
        task.last_run_id = self.workspace_task_last_run(id).await?;
        Ok(task)
    }
    pub async fn write_workspace_task(
        &self,
        project: &str,
        task: Option<&str>,
        input: &WriteWorkspaceTask,
    ) -> Result<WorkspaceTask> {
        key(&input.idempotency_key)?;
        valid_text(&input.value.name, 200)?;
        valid_text(&input.value.prompt, 128000)?;
        bounded(&input.value.description, 8000)?;
        bounded(&input.value.expected_output, 16000)?;
        validate_budgets(&input.value.budgets)?;
        if let Some(config) = &input.value.agent_config {
            crate::library_config::validate_effective_agent_config(config)?;
        }
        if let WorkspaceTaskExecution::Model {
            provider_id,
            provider_revision,
        } = &input.value.execution
        {
            valid_text(provider_id, 200)?;
            if *provider_revision < 1 {
                return Err(Error::Validation("Choose a current model profile".into()));
            }
        }
        if let WorkspaceTaskExecution::Workflow {
            inputs,
            workflow_id,
            version_id,
        } = &input.value.execution
        {
            valid_text(workflow_id, 200)?;
            valid_text(version_id, 200)?;
            if let Some(inputs) = inputs {
                cowork_domain::workflows::validate_data_value_budget(inputs)?;
            }
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        project_lock(&mut tx, project).await?;
        let scope = format!("task:{}", task.unwrap_or(project));
        let digest = canonical_hash(input);
        if let Some(saved) = original(&mut tx, &scope, &input.idempotency_key, &digest).await? {
            tx.commit().await.map_err(storage_error)?;
            return Ok(saved);
        }
        let order = order_lock(&mut tx, &input.value.order_id).await?;
        if order.project_id != project || archived(&mut tx, &order.id).await? {
            return Err(Error::Forbidden);
        }
        let mut result = if let Some(id) = task {
            let raw: Option<String> = sqlx::query_scalar(
                "SELECT payload FROM native_workspace_tasks WHERE id=$1 AND project_id=$2",
            )
            .bind(id)
            .bind(project)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?;
            let old: WorkspaceTask = parse(&raw.ok_or(Error::NotFound)?)?;
            if old.revision != input.expected_revision || old.archived {
                return Err(Error::Conflict);
            }
            old
        } else {
            if input.expected_revision != 0 {
                return Err(Error::Conflict);
            }
            WorkspaceTask {
                id: id(),
                project_id: project.into(),
                revision: 0,
                created_at: now(),
                updated_at: now(),
                archived: false,
                last_run_id: None,
                value: input.value.clone(),
            }
        };
        result.revision += 1;
        result.updated_at = now();
        result.value = input.value.clone();
        sqlx::query("INSERT INTO native_workspace_tasks(id,project_id,order_id,revision,archived,payload) VALUES($1,$2,$3,$4,0,$5) ON CONFLICT(id) DO UPDATE SET order_id=excluded.order_id,revision=excluded.revision,payload=excluded.payload").bind(&result.id).bind(project).bind(&result.value.order_id).bind(result.revision).bind(json_string(&result)?).execute(&mut *tx).await.map_err(storage_error)?;
        receipt(&mut tx, &scope, &input.idempotency_key, &digest, &result).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn archive_workspace_task(
        &self,
        id: &str,
        input: &ArchiveWorkspaceTask,
    ) -> Result<WorkspaceTask> {
        key(&input.idempotency_key)?;
        let task = self.workspace_task(id).await?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        project_lock(&mut tx, &task.project_id).await?;
        let scope = format!("task:{id}");
        let digest = canonical_hash(&json!(["archive", input]));
        if let Some(saved) = original(&mut tx, &scope, &input.idempotency_key, &digest).await? {
            tx.commit().await.map_err(storage_error)?;
            return Ok(saved);
        }
        let raw: String =
            sqlx::query_scalar("SELECT payload FROM native_workspace_tasks WHERE id=$1")
                .bind(id)
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        let mut task: WorkspaceTask = parse(&raw)?;
        if task.revision != input.expected_revision {
            return Err(Error::Conflict);
        }
        no_active_runs(&mut tx, &task.value.order_id).await?;
        task.archived = true;
        task.revision += 1;
        task.updated_at = now();
        sqlx::query(
            "UPDATE native_workspace_tasks SET archived=1,revision=$1,payload=$2 WHERE id=$3",
        )
        .bind(task.revision)
        .bind(json_string(&task)?)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(storage_error)?;
        receipt(&mut tx, &scope, &input.idempotency_key, &digest, &task).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(task)
    }
    /// Records the exact task version before admitting a run. Retries keep that snapshot.
    pub async fn begin_workspace_task_run(
        &self,
        id: &str,
        input: &StartWorkspaceTask,
    ) -> Result<WorkspaceTask> {
        key(&input.idempotency_key)?;
        let task = self.workspace_task(id).await?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        project_lock(&mut tx, &task.project_id).await?;
        let digest = canonical_hash(input);
        if let Some(row)=sqlx::query("SELECT request_sha256,task_snapshot FROM native_workspace_task_runs WHERE task_id=$1 AND request_key=$2").bind(id).bind(&input.idempotency_key).fetch_optional(&mut *tx).await.map_err(storage_error)?{
            if row.try_get::<String,_>("request_sha256").map_err(storage_error)?!=digest{return Err(Error::Conflict);}
            let result:WorkspaceTask=parse(&row.try_get::<String,_>("task_snapshot").map_err(storage_error)?)?;
            let admitted:i64=sqlx::query_scalar("SELECT COUNT(*) FROM runs WHERE order_id=$1 AND idempotency_key=$2").bind(&result.value.order_id).bind(workspace_task_run_key(id,&input.idempotency_key)).fetch_one(&mut *tx).await.map_err(storage_error)?;
            if admitted==0 {
                let is_archived:i64=sqlx::query_scalar("SELECT archived FROM native_workspace_tasks WHERE id=$1").bind(id).fetch_one(&mut *tx).await.map_err(storage_error)?;
                if is_archived==1{return Err(Error::Conflict);}
                let order=order_lock(&mut tx,&result.value.order_id).await?;
                if order.project_id!=result.project_id||archived(&mut tx,&order.id).await?{return Err(Error::Forbidden);}
                if order.revision!=input.order_revision{return Err(Error::Conflict);}
            }
            tx.commit().await.map_err(storage_error)?;return Ok(result);
        }
        let raw: String =
            sqlx::query_scalar("SELECT payload FROM native_workspace_tasks WHERE id=$1")
                .bind(id)
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        let task: WorkspaceTask = parse(&raw)?;
        if task.archived || task.revision != input.expected_revision {
            return Err(Error::Conflict);
        }
        let order = order_lock(&mut tx, &task.value.order_id).await?;
        if order.project_id != task.project_id || archived(&mut tx, &order.id).await? {
            return Err(Error::Forbidden);
        }
        if order.revision != input.order_revision {
            return Err(Error::Conflict);
        }
        sqlx::query("INSERT INTO native_workspace_task_runs(task_id,request_key,request_sha256,task_snapshot,created_at) VALUES($1,$2,$3,$4,$5)").bind(id).bind(&input.idempotency_key).bind(digest).bind(json_string(&task)?).bind(now()).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(task)
    }
    pub async fn complete_workspace_task_start(
        &self,
        id: &str,
        key: &str,
        run: &str,
    ) -> Result<()> {
        let changed=sqlx::query("UPDATE native_workspace_task_runs SET run_id=$1 WHERE task_id=$2 AND request_key=$3 AND (run_id IS NULL OR run_id=$1)").bind(run).bind(id).bind(key).execute(&self.pool).await.map_err(storage_error)?;
        if changed.rows_affected() != 1 {
            return Err(Error::Conflict);
        }
        Ok(())
    }
}
