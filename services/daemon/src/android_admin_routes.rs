//! Original V1 team/member shapes; all authorization comes from actual V2 sessions.
use super::*;
pub(super) fn routes() -> Router<CompatState> {
    Router::new()
        .route("/teams/{team}/members", get(members).post(set_member))
        .route(
            "/teams/{team}/members/{user}",
            axum::routing::delete(remove_member),
        )
}
pub(super) type NewTeam = AndroidCreateTeam;
pub(super) async fn create_team(
    State(s): State<CompatState>,
    Session(a): Session,
    headers: HeaderMap,
    Json(v): Json<NewTeam>,
) -> std::result::Result<(StatusCode, Json<Value>), Failure> {
    // V2 installation policy remains authoritative: only a real platform
    // administrator can create an organization. The adapter never weakens it.
    let key = operation(&headers)?;
    let value = s
        .service
        .create_team(
            &a,
            CreateTeam {
                name: v.name,
                idempotency_key: key,
            },
        )
        .await?;
    Ok((StatusCode::CREATED, Json(team_wire(&s, &a, &value).await?)))
}
type Member = AndroidSetTeamMember;
fn operation(headers: &HeaderMap) -> Result<String> {
    let mut keys = headers.get_all("idempotency-key").iter();
    let key = keys.next().and_then(|v| v.to_str().ok());
    if keys.next().is_some()
        || key.is_some_and(|v| v.is_empty() || v.len() > 200 || v.chars().any(char::is_control))
    {
        return Err(invalid());
    }
    // The original V1 request omitted a key. Its set-state operation naturally
    // no-ops when already applied; upgraded clients can retain an explicit key.
    Ok(key
        .map(str::to_owned)
        .unwrap_or_else(|| Uuid::new_v4().to_string()))
}
async fn members(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(team): Path<Uuid>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .android_admin_team_members(&a, team)
            .await?
    )))
}
async fn set_member(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(team): Path<Uuid>,
    headers: HeaderMap,
    Json(v): Json<Member>,
) -> std::result::Result<StatusCode, Failure> {
    s.service
        .store()
        .android_admin_team_member(&a, team, v.user_id, Some(&v.role), &operation(&headers)?)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn remove_member(
    State(s): State<CompatState>,
    Session(a): Session,
    Path((team, user)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
) -> std::result::Result<StatusCode, Failure> {
    s.service
        .store()
        .android_admin_team_member(&a, team, user, None, &operation(&headers)?)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
