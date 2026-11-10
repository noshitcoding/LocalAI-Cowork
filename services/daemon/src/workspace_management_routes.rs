//! Native owner workspace features, merged under the existing owner authorization.
use super::*;
use cowork_workflow_contracts::{WorkflowRunKind, WorkflowRunRequest};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/independent-chat-space", get(independent_chat_space))
        .route("/projects/{project}/delete", post(delete_project))
        .route("/orders/{order}/regenerate", post(regenerate))
        .route(
            "/projects/{project}/settings",
            get(project_settings).put(write_project_settings),
        )
        .route(
            "/orders/{order}/management",
            get(chat_management).put(manage_chat),
        )
        .route("/orders/{order}/archive", post(archive_chat))
        .route("/orders/{order}/delete", post(delete_chat))
        .route(
            "/orders/{order}/configuration",
            get(chat_configuration).put(write_chat_configuration),
        )
        .route("/projects/{project}/archived-chats", get(archived_chats))
        .route("/projects/{project}/tasks", get(tasks).post(create_task))
        .route(
            "/workspace-tasks/{task}",
            get(task).put(update_task).delete(archive_task),
        )
        .route("/workspace-tasks/{task}/run", post(start_task))
        .layer(DefaultBodyLimit::max(1_100_000))
}
async fn independent_chat_space(State(s): State<AppState>) -> Answer<Project> {
    Ok(Json(s.runtime.store.independent_chat_space().await?))
}
async fn delete_chat(
    State(s): State<AppState>,
    Path(order): Path<String>,
    body: Input<DeleteLibraryEntry>,
) -> Answer<LibraryDeleted> {
    Ok(Json(
        s.runtime.delete_native_chat(&order, &input(body)?).await?,
    ))
}
async fn delete_project(
    State(s): State<AppState>,
    Path(project): Path<String>,
    body: Input<DeleteLibraryEntry>,
) -> Answer<LibraryDeleted> {
    Ok(Json(
        s.runtime
            .delete_native_project(&project, &input(body)?)
            .await?,
    ))
}
async fn regenerate(
    State(s): State<AppState>,
    Path(order): Path<String>,
    body: Input<BranchOrder>,
) -> Answer<Order> {
    Ok(Json(
        s.runtime
            .store
            .regenerate_order(&order, input(body)?)
            .await?,
    ))
}
async fn project_settings(
    State(s): State<AppState>,
    Path(project): Path<String>,
) -> Answer<ProjectSettings> {
    Ok(Json(s.runtime.store.project_settings(&project).await?))
}
async fn write_project_settings(
    State(s): State<AppState>,
    Path(project): Path<String>,
    body: Input<WriteProjectSettings>,
) -> Answer<ProjectSettings> {
    Ok(Json(
        s.runtime
            .store
            .write_project_settings(&project, &input(body)?)
            .await?,
    ))
}
async fn chat_management(
    State(s): State<AppState>,
    Path(order): Path<String>,
) -> Answer<ChatManagement> {
    Ok(Json(s.runtime.store.chat_management(&order).await?))
}
async fn chat_configuration(
    State(s): State<AppState>,
    Path(order): Path<String>,
) -> Answer<ChatConfiguration> {
    Ok(Json(s.runtime.store.chat_configuration(&order).await?))
}
async fn write_chat_configuration(
    State(s): State<AppState>,
    Path(order): Path<String>,
    body: Input<WriteChatConfiguration>,
) -> Answer<ChatConfiguration> {
    Ok(Json(
        s.runtime
            .store
            .write_chat_configuration(&order, &input(body)?)
            .await?,
    ))
}
async fn archived_chats(
    State(s): State<AppState>,
    Path(project): Path<String>,
) -> Answer<Vec<ChatManagement>> {
    Ok(Json(s.runtime.store.archived_chats(&project).await?))
}
async fn manage_chat(
    State(s): State<AppState>,
    Path(order): Path<String>,
    body: Input<ManageChat>,
) -> Answer<ChatManagement> {
    Ok(Json(
        s.runtime.store.manage_chat(&order, &input(body)?).await?,
    ))
}
async fn archive_chat(
    State(s): State<AppState>,
    Path(order): Path<String>,
    body: Input<ArchiveChat>,
) -> Answer<ChatManagement> {
    Ok(Json(
        s.runtime.store.archive_chat(&order, &input(body)?).await?,
    ))
}
async fn tasks(
    State(s): State<AppState>,
    Path(project): Path<String>,
) -> Answer<Vec<WorkspaceTask>> {
    Ok(Json(s.runtime.store.workspace_tasks(&project).await?))
}
async fn task(State(s): State<AppState>, Path(task): Path<String>) -> Answer<WorkspaceTask> {
    Ok(Json(s.runtime.store.workspace_task(&task).await?))
}
async fn create_task(
    State(s): State<AppState>,
    Path(project): Path<String>,
    body: Input<WriteWorkspaceTask>,
) -> Answer<WorkspaceTask> {
    Ok(Json(
        s.runtime
            .store
            .write_workspace_task(&project, None, &input(body)?)
            .await?,
    ))
}
async fn update_task(
    State(s): State<AppState>,
    Path(task): Path<String>,
    body: Input<WriteWorkspaceTask>,
) -> Answer<WorkspaceTask> {
    let old = s.runtime.store.workspace_task(&task).await?;
    Ok(Json(
        s.runtime
            .store
            .write_workspace_task(&old.project_id, Some(&task), &input(body)?)
            .await?,
    ))
}
async fn archive_task(
    State(s): State<AppState>,
    Path(task): Path<String>,
    body: Input<ArchiveWorkspaceTask>,
) -> Answer<WorkspaceTask> {
    Ok(Json(
        s.runtime
            .store
            .archive_workspace_task(&task, &input(body)?)
            .await?,
    ))
}
async fn start_task(
    State(s): State<AppState>,
    Path(task): Path<String>,
    body: Input<StartWorkspaceTask>,
) -> Answer<RunRecord> {
    let request = input(body)?;
    let snapshot = s
        .runtime
        .store
        .begin_workspace_task_run(&task, &request)
        .await?;
    let value = snapshot.value;
    let prompt = if value.expected_output.trim().is_empty() {
        value.prompt
    } else {
        format!(
            "{}\n\nExpected output:\n{}",
            value.prompt, value.expected_output
        )
    };
    let run_key = cowork_storage::workspace_management::workspace_task_run_key(
        &task,
        &request.idempotency_key,
    );
    let input = match value.execution {
        WorkspaceTaskExecution::Model {
            provider_id,
            provider_revision,
        } => RunRequest::Single(CreateRun {
            agent_config: value.agent_config,
            model_binding: None,
            output: None,
            native_tools: value.native_tools,
            environment: value.environment,
            mcp_tools: value.mcp_tools,
            budgets: value.budgets,
            order_id: value.order_id,
            base_revision: request.order_revision,
            idempotency_key: run_key,
            prompt,
            provider_id,
            provider_revision,
            access_mode: value.access_mode,
            folders: value.folders,
        }),
        WorkspaceTaskExecution::Workflow {
            inputs,
            workflow_id,
            version_id,
        } => RunRequest::Workflow(WorkflowRunRequest {
            input_source: None,
            inputs,
            agent_config: value.agent_config,
            folders: value.folders,
            mcp_tools: value.mcp_tools,
            environment: value.environment,
            native_tools: value.native_tools,
            kind: WorkflowRunKind::Workflow,
            order_id: value.order_id,
            base_revision: request.order_revision,
            idempotency_key: run_key,
            prompt,
            workflow_id,
            version_id,
            access_mode: value.access_mode,
            budgets: value.budgets,
            expected_binding: None,
        }),
    };
    let run = s.runtime.create_run_request(&input).await?;
    s.runtime
        .store
        .complete_workspace_task_start(&task, &request.idempotency_key, &run.id)
        .await?;
    Ok(Json(run))
}
