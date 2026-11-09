//! Owner-selected pinned definitions; installation authority remains exclusively
//! with the authenticated runner. No API accepts qualifications or observations.
use super::*;
use cowork_domain::environments::{
    EnvironmentChange, EnvironmentDefinition, EnvironmentRecord, EnvironmentVersionState,
};
use serde::Serialize;

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project}/orders/{order}/sandbox-session",
            post(sandbox_session),
        )
        .route(
            "/projects/{project}/orders/{order}/sandbox-operations",
            get(sandbox_history),
        )
        .route(
            "/projects/{project}/orders/{order}/sandbox-preview",
            post(sandbox_preview),
        )
        .route("/projects/{project}/environments", get(list).post(create))
        .route("/projects/{project}/environments/{environment}", get(read))
        .route(
            "/projects/{project}/environments/{environment}/stage",
            post(stage),
        )
        .route(
            "/projects/{project}/environments/{environment}/revoke",
            post(revoke),
        )
        .route(
            "/projects/{project}/environments/{environment}/versions/{version}/qualify",
            post(qualify),
        )
        .route(
            "/projects/{project}/environments/{environment}/versions/{version}/activate",
            post(activate),
        )
        .route(
            "/projects/{project}/environments/{environment}/versions/{version}/revoke",
            post(revoke_version),
        )
        .layer(DefaultBodyLimit::max(256 * 1024))
}
async fn sandbox_history(
    State(s): State<AppState>,
    Path((project, order)): Path<(String, String)>,
) -> Answer<Vec<SandboxSessionOperation>> {
    Ok(Json(
        s.runtime
            .sandbox_operation_history(&project, &order)
            .await?,
    ))
}
async fn sandbox_preview(
    State(s): State<AppState>,
    Path((project, order)): Path<(String, String)>,
    body: Input<CreateSandboxPreview>,
) -> Answer<SandboxPreview> {
    let request = input(body)?;
    let base = std::env::var("COWORK_SANDBOX_PREVIEW_BASE_DOMAIN").map_err(|_| {
        Error::Validation("Sandbox preview is not configured by the operator".into())
    })?;
    Ok(Json(
        s.runtime
            .sandbox_preview(&project, &order, &request, &base)
            .await?,
    ))
}
async fn sandbox_session(
    State(s): State<AppState>,
    Path((project, order)): Path<(String, String)>,
    body: Input<ManageSandboxSession>,
) -> Answer<SandboxSession> {
    let request = input(body)?;
    Ok(Json(
        s.runtime
            .manage_sandbox_session(&project, &order, &request)
            .await?,
    ))
}
#[derive(Serialize)]
struct View {
    #[serde(flatten)]
    record: EnvironmentRecord,
    active_reference: Option<ExecutionEnvironmentRef>,
}
fn view(record: EnvironmentRecord) -> View {
    let active_reference = if record.revoked {
        None
    } else {
        record
            .active_version_id
            .as_ref()
            .and_then(|id| record.versions.iter().find(|v| &v.id == id && !v.revoked))
            .and_then(|version| match &version.state {
                EnvironmentVersionState::Qualified { qualification } => {
                    Some(ExecutionEnvironmentRef {
                        environment_id: record.id.clone(),
                        version_id: version.id.clone(),
                        definition_sha256: version.sha256.clone(),
                        qualification_sha256: cowork_domain::canonical_hash(qualification),
                    })
                }
                _ => None,
            })
    };
    View {
        record,
        active_reference,
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    after: Option<String>,
    #[serde(default = "page_limit")]
    limit: u32,
}
fn page_limit() -> u32 {
    100
}
#[derive(Serialize)]
struct PageReply {
    items: Vec<View>,
    next_cursor: Option<String>,
    has_more: bool,
}
async fn list(
    State(s): State<AppState>,
    Path(project): Path<String>,
    Query(q): Query<Page>,
) -> Answer<PageReply> {
    if !(1..=500).contains(&q.limit) {
        return Err(Error::Validation("Environment page limit must be 1..500".into()).into());
    }
    let mut items = s
        .runtime
        .store
        .environments(&project, q.after.as_deref(), q.limit + 1)
        .await?;
    let has_more = items.len() > q.limit as usize;
    items.truncate(q.limit as usize);
    let next_cursor = has_more
        .then(|| items.last().map(|v| v.id.clone()))
        .flatten();
    Ok(Json(PageReply {
        items: items.into_iter().map(view).collect(),
        next_cursor,
        has_more,
    }))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Create {
    name: String,
    idempotency_key: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Mutation {
    expected_revision: i64,
    idempotency_key: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Stage {
    expected_revision: i64,
    idempotency_key: String,
    definition: EnvironmentDefinition,
}
async fn create(
    State(s): State<AppState>,
    Path(project): Path<String>,
    body: Input<Create>,
) -> Answer<View> {
    let q = input(body)?;
    Ok(Json(view(
        s.runtime
            .store
            .create_environment(&project, &q.name, &q.idempotency_key)
            .await?,
    )))
}
async fn read(
    State(s): State<AppState>,
    Path((project, environment)): Path<(String, String)>,
) -> Answer<View> {
    Ok(Json(view(
        s.runtime.store.environment(&project, &environment).await?,
    )))
}
async fn stage(
    State(s): State<AppState>,
    Path((project, environment)): Path<(String, String)>,
    body: Input<Stage>,
) -> Answer<View> {
    let q = input(body)?;
    q.definition.validate()?;
    s.runtime
        .store
        .change_environment(
            &project,
            &environment,
            q.expected_revision,
            &q.idempotency_key,
            &EnvironmentChange::Stage {
                definition: q.definition,
            },
        )
        .await?;
    read(State(s), Path((project, environment))).await
}
async fn revoke(
    State(s): State<AppState>,
    Path((project, environment)): Path<(String, String)>,
    body: Input<Mutation>,
) -> Answer<View> {
    let q = input(body)?;
    s.runtime
        .store
        .change_environment(
            &project,
            &environment,
            q.expected_revision,
            &q.idempotency_key,
            &EnvironmentChange::Revoke,
        )
        .await?;
    read(State(s), Path((project, environment))).await
}
async fn revoke_version(
    State(s): State<AppState>,
    Path((project, environment, version)): Path<(String, String, String)>,
    body: Input<Mutation>,
) -> Answer<View> {
    let q = input(body)?;
    s.runtime
        .store
        .change_environment(
            &project,
            &environment,
            q.expected_revision,
            &q.idempotency_key,
            &EnvironmentChange::RevokeVersion {
                version_id: version,
            },
        )
        .await?;
    read(State(s), Path((project, environment))).await
}
async fn qualify(
    State(s): State<AppState>,
    Path((project, environment, version)): Path<(String, String, String)>,
    body: Input<Mutation>,
) -> Answer<View> {
    let q = input(body)?;
    Ok(Json(view(
        s.runtime
            .qualify_process_environment(
                &project,
                &environment,
                &version,
                q.expected_revision,
                &q.idempotency_key,
            )
            .await?,
    )))
}
async fn activate(
    State(s): State<AppState>,
    Path((project, environment, version)): Path<(String, String, String)>,
    body: Input<Mutation>,
) -> Answer<View> {
    let q = input(body)?;
    Ok(Json(view(
        s.runtime
            .activate_process_environment(
                &project,
                &environment,
                &version,
                q.expected_revision,
                &q.idempotency_key,
            )
            .await?,
    )))
}
