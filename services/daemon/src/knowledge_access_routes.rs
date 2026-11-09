//! Owner switches for agent access to the project and personal knowledge bases.
//! Merged under the existing owner bearer/origin middleware; team sessions use
//! the administrator policy in team_knowledge_access_routes instead.
use super::*;
use cowork_storage::{
    OwnerKnowledgeAccess, ProjectKnowledgeAccess, WriteOwnerKnowledgeAccess,
    WriteProjectKnowledgeAccess,
};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/knowledge-access",
            get(owner_access).put(write_owner_access),
        )
        .route(
            "/projects/{project}/knowledge-access",
            get(project_access).put(write_project_access),
        )
        .layer(DefaultBodyLimit::max(8 * 1024))
}
async fn owner_access(State(s): State<AppState>) -> Answer<OwnerKnowledgeAccess> {
    Ok(Json(s.runtime.store.owner_knowledge_access().await?))
}
async fn write_owner_access(
    State(s): State<AppState>,
    body: Input<WriteOwnerKnowledgeAccess>,
) -> Answer<OwnerKnowledgeAccess> {
    Ok(Json(
        s.runtime
            .store
            .write_owner_knowledge_access(&input(body)?)
            .await?,
    ))
}
async fn project_access(
    State(s): State<AppState>,
    Path(project): Path<String>,
) -> Answer<ProjectKnowledgeAccess> {
    Ok(Json(
        s.runtime.store.project_knowledge_access(&project).await?,
    ))
}
async fn write_project_access(
    State(s): State<AppState>,
    Path(project): Path<String>,
    body: Input<WriteProjectKnowledgeAccess>,
) -> Answer<ProjectKnowledgeAccess> {
    Ok(Json(
        s.runtime
            .store
            .write_project_knowledge_access(&project, &input(body)?)
            .await?,
    ))
}
