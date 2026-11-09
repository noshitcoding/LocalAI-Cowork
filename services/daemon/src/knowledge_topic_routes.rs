//! Owner-only hierarchy and graph APIs under the existing authenticated API.
use super::*;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicQuery {
    #[serde(default)]
    personal: bool,
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project}/knowledge-folders",
            get(folders).put(write_folders),
        )
        .route(
            "/projects/{project}/knowledge-graphs",
            get(graphs).post(build_graph).delete(delete_graph),
        )
        .route(
            "/projects/{project}/knowledge-graphs/search",
            post(search_graph),
        )
        .layer(DefaultBodyLimit::max(256 * 1024))
}
async fn folders(
    State(s): State<AppState>,
    Path(project): Path<String>,
    Query(q): Query<TopicQuery>,
) -> Answer<KnowledgeFolders> {
    Ok(Json(
        s.runtime
            .store
            .knowledge_folders(&project, q.personal)
            .await?,
    ))
}
async fn write_folders(
    State(s): State<AppState>,
    Path(project): Path<String>,
    Query(q): Query<TopicQuery>,
    body: Input<WriteKnowledgeFolders>,
) -> Answer<KnowledgeFolders> {
    Ok(Json(
        s.runtime
            .store
            .write_knowledge_folders(&project, q.personal, input(body)?)
            .await?,
    ))
}
async fn graphs(
    State(s): State<AppState>,
    Path(project): Path<String>,
    Query(q): Query<TopicQuery>,
) -> Answer<Vec<KnowledgeTopicGraph>> {
    Ok(Json(
        s.runtime
            .store
            .knowledge_graphs(&project, q.personal)
            .await?,
    ))
}
async fn build_graph(
    State(s): State<AppState>,
    Path(project): Path<String>,
    Query(q): Query<TopicQuery>,
    body: Input<BuildKnowledgeGraph>,
) -> Answer<KnowledgeTopicGraph> {
    Ok(Json(
        s.runtime
            .store
            .build_knowledge_graph(&project, q.personal, input(body)?)
            .await?,
    ))
}
async fn search_graph(
    State(s): State<AppState>,
    Path(project): Path<String>,
    Query(q): Query<TopicQuery>,
    body: Input<SearchKnowledgeGraph>,
) -> Answer<KnowledgeGraphResults> {
    Ok(Json(
        s.runtime
            .store
            .search_knowledge_graph(&project, q.personal, input(body)?)
            .await?,
    ))
}
async fn delete_graph(
    State(s): State<AppState>,
    Path(project): Path<String>,
    Query(q): Query<TopicQuery>,
    body: Input<BuildKnowledgeGraph>,
) -> Answer<Value> {
    s.runtime
        .store
        .delete_knowledge_graph(&project, q.personal, input(body)?)
        .await?;
    Ok(Json(json!({"deleted":true})))
}
