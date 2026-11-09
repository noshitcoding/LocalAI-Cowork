//! Actor-only repository credential custody; responses contain metadata only.
use super::*;
struct SecretBody(Value);
impl std::ops::Deref for SecretBody {
    type Target = Value;
    fn deref(&self) -> &Value {
        &self.0
    }
}
impl Drop for SecretBody {
    fn drop(&mut self) {
        if let Some(Value::String(token)) = self.0.get_mut("token") {
            use zeroize::Zeroize;
            token.zeroize();
        }
    }
}
pub(super) fn routes() -> Router<CompatState> {
    Router::new()
        .route("/projects/{project}/github-bindings", get(list))
        .route(
            "/projects/{project}/github-bindings/{binding}",
            axum::routing::put(set).delete(remove),
        )
}
fn operation(headers: &HeaderMap) -> Result<Uuid> {
    Uuid::parse_str(
        headers
            .get("idempotency-key")
            .or_else(|| headers.get("x-operation-id"))
            .ok_or_else(invalid)?
            .to_str()
            .map_err(|_| invalid())?,
    )
    .map_err(|_| invalid())
}
async fn list(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(project): Path<Uuid>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .android_private_github_bindings(&a, project)
            .await?
    )))
}
async fn set(
    State(s): State<CompatState>,
    Session(a): Session,
    Path((project, binding)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Answer {
    let body = SecretBody(body);
    let value = s
        .service
        .store()
        .android_private_github_mutate(
            &a,
            project,
            binding,
            &body,
            false,
            operation(&headers)?,
            s.keys.as_ref(),
            s.key_id,
        )
        .await;
    Ok(Json(value?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Delete {
    expected_revision: i64,
}
async fn remove(
    State(s): State<CompatState>,
    Session(a): Session,
    Path((project, binding)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Query(q): Query<Delete>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_private_github_mutate(
                &a,
                project,
                binding,
                &json!({"expected_revision":q.expected_revision}),
                true,
                operation(&headers)?,
                s.keys.as_ref(),
                s.key_id,
            )
            .await?,
    ))
}
