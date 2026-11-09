//! Server administrator policy for agent access to knowledge bases. Only a
//! platform administrator may read or change the policy; signed-in users may
//! read their own effective access and that of team projects they can read.
use super::*;
use cowork_storage::{
    ActorKnowledgeAccess, KnowledgeAccessDirectory, KnowledgeAccessPolicy,
    TeamProjectKnowledgeAccess, WriteKnowledgeAccessPolicy,
};

pub(super) fn routes() -> Router<TeamState> {
    Router::new()
        .route("/knowledge-access/policy", get(policy).put(write_policy))
        .route("/knowledge-access/me", get(own_access))
        .route(
            "/{team}/projects/{project}/knowledge-access",
            get(project_access),
        )
        .layer(DefaultBodyLimit::max(128 * 1024))
}
async fn policy(
    State(s): State<TeamState>,
    Session(a): Session,
) -> Answer<KnowledgeAccessDirectory> {
    Ok(Json(
        s.service.store().knowledge_access_directory(&a).await?,
    ))
}
async fn write_policy(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictJson(v): StrictJson<WriteKnowledgeAccessPolicy>,
) -> Answer<KnowledgeAccessPolicy> {
    Ok(Json(
        s.service
            .store()
            .write_knowledge_access_policy(&a, &v)
            .await?,
    ))
}
async fn own_access(
    State(s): State<TeamState>,
    Session(a): Session,
) -> Answer<ActorKnowledgeAccess> {
    Ok(Json(s.service.store().actor_knowledge_access(&a).await?))
}
async fn project_access(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath((t, p)): StrictPath<(Uuid, Uuid)>,
) -> Answer<TeamProjectKnowledgeAccess> {
    // Authorizes the current membership and project read grant first.
    s.service.project(&a, t, p).await?;
    Ok(Json(
        s.service.store().team_project_knowledge_access(p).await?,
    ))
}
