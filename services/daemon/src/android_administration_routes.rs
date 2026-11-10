//! Administrative endpoints remain inside the authenticated V1 Actor router.
use super::*;
#[path = "android_administration_extended_routes.rs"]
mod extended;
pub(super) fn routes() -> Router<CompatState> {
    Router::new()
        .merge(extended::routes())
        .route("/content-access", get(access))
        .route(
            "/projects/{project}/members/operations/{key}",
            get(private_original),
        )
        .route("/projects/{project}/members", get(members).post(set_member))
        .route(
            "/projects/{project}/members/{user}",
            axum::routing::delete(remove_member),
        )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    after: Option<Uuid>,
    #[serde(default)]
    limit: Option<u32>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    project_id: Option<Uuid>,
}
fn key(headers: &HeaderMap, body: Option<&str>) -> Result<Option<String>> {
    let mut values = headers.get_all("idempotency-key").iter();
    let value = values
        .next()
        .map(|h| h.to_str().map_err(|_| invalid()))
        .transpose()?;
    if values.next().is_some() || value.zip(body).is_some_and(|(a, b)| a != b) {
        return Err(invalid());
    }
    let chosen = value.or(body);
    if chosen.is_some_and(|s| s.is_empty() || s.len() > 200 || s.chars().any(char::is_control)) {
        return Err(invalid());
    }
    Ok(chosen.map(str::to_owned))
}
async fn members(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(p): Path<Uuid>,
    Query(q): Query<Page>,
) -> Answer {
    if s.service.store().android_private_project_exists(p).await? {
        return Ok(Json(
            s.service
                .store()
                .android_private_project_members(&a, p, q.after, q.limit.unwrap_or(50))
                .await?,
        ));
    }
    Ok(Json(json!(
        s.service
            .store()
            .android_project_member_page(&a, p, q.after, q.limit.unwrap_or(50))
            .await?
    )))
}
async fn set_member(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(p): Path<Uuid>,
    headers: HeaderMap,
    Json(v): Json<AndroidProjectMemberWrite>,
) -> Answer {
    let operation = key(&headers, v.idempotency_key.as_deref())?;
    Ok(Json(
        s.service
            .store()
            .android_project_member_write(
                &a,
                p,
                v.user_id,
                Some(&v.role),
                v.expected_revision,
                operation.as_deref(),
            )
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Revision {
    expected_revision: Option<i64>,
}
async fn remove_member(
    State(s): State<CompatState>,
    Session(a): Session,
    Path((p, user)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Query(q): Query<Revision>,
) -> Answer {
    let operation = key(&headers, None)?;
    Ok(Json(
        s.service
            .store()
            .android_project_member_write(
                &a,
                p,
                user,
                None,
                q.expected_revision,
                operation.as_deref(),
            )
            .await?,
    ))
}
async fn access(
    State(s): State<CompatState>,
    Session(a): Session,
    Query(q): Query<Target>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .android_content_access(&a, q.project_id)
            .await?
    )))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OriginalQuery {
    request_sha256: String,
}
async fn private_original(
    State(s): State<CompatState>,
    Session(a): Session,
    Path((project, key)): Path<(Uuid, String)>,
    Query(q): Query<OriginalQuery>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_private_sharing_original(&a, project, &key, &q.request_sha256)
            .await?,
    ))
}
