use super::*;
use crate::library_config_routes::{RequestOwner, authenticate_actor};
use axum::Extension;
use cowork_runtime::codex_accounts::{AccountOperation, CodexBroker, login_view, models};
#[derive(Clone)]
struct CodexState {
    runtime: Runtime,
}
fn routes() -> Router<CodexState> {
    Router::new()
        .route("/codex/runtime", get(status))
        .route("/codex/accounts", get(accounts).post(create))
        .route(
            "/codex/accounts/{id}",
            get(account).put(update).delete(remove),
        )
        .route("/codex/accounts/{id}/status", get(account_status))
        .route("/codex/accounts/{id}/models", get(model_list))
        .route(
            "/codex/accounts/{id}/model-profile",
            post(materialize_model),
        )
        .route("/codex/accounts/{id}/login", post(login))
        .route("/codex/accounts/{id}/logout", post(logout))
        .layer(DefaultBodyLimit::max(32 * 1024))
}
async fn materialize_model(
    State(s): State<CodexState>,
    Extension(o): Extension<RequestOwner>,
    Path(id): Path<String>,
    body: Input<MaterializeCodexModelProfile>,
) -> Answer<ProviderProfile> {
    let input = input(body)?;
    if let Some(profile) = s
        .runtime
        .store
        .original_codex_model_profile(o.owner(), &id, &input)
        .await?
    {
        if matches!(o, RequestOwner::Local) {
            let mut profiles = s.runtime.providers.write().await;
            if !profiles.iter().any(|p| p.profile.id == profile.id)
                && !s.runtime.store.provider_retired(&profile.id).await?
            {
                profiles.push(cowork_runtime::ConfiguredProvider {
                    profile: profile.clone(),
                    credential_env: None,
                });
            }
        }
        return Ok(Json(profile));
    }
    let account = s.runtime.store.codex_account(o.owner(), &id).await?;
    if account.revision != input.expected_account_revision {
        return Err(Error::Conflict.into());
    }
    let b = ready_broker(&s).await?;
    let available = models(
        &b.call(o.owner().namespace(), id.clone(), AccountOperation::Models)
            .await?,
    )?;
    let model = available
        .iter()
        .find(|m| m.id == input.model)
        .ok_or_else(|| Error::Validation("codex_model_unavailable".into()))?;
    if input
        .reasoning_effort
        .as_ref()
        .is_some_and(|effort| !model.reasoning_efforts.contains(effort))
    {
        return Err(Error::Validation("codex_reasoning_effort_unavailable".into()).into());
    }
    let profile = s
        .runtime
        .store
        .materialize_codex_model_profile(o.owner(), &id, &input)
        .await?;
    if matches!(o, RequestOwner::Local) {
        let mut profiles = s.runtime.providers.write().await;
        if !profiles.iter().any(|p| p.profile.id == profile.id) {
            profiles.push(cowork_runtime::ConfiguredProvider {
                profile: profile.clone(),
                credential_env: None,
            });
        }
    }
    Ok(Json(profile))
}
pub(super) fn owner_router(runtime: Runtime) -> Router<AppState> {
    routes()
        .layer(Extension(RequestOwner::Local))
        .with_state(CodexState { runtime })
}
pub(super) fn actor_router(runtime: Runtime) -> Router {
    let store = runtime.store.clone();
    Router::new()
        .nest("/api/v1", routes())
        .layer(middleware::from_fn_with_state(store, authenticate_actor))
        .with_state(CodexState { runtime })
}
fn broker(state: &CodexState) -> std::result::Result<Arc<CodexBroker>, ApiFailure> {
    state
        .runtime
        .codex_broker()
        .ok_or_else(|| ApiFailure(Error::Provider("codex_runtime_not_configured".into())))
}
async fn ready_broker(state: &CodexState) -> std::result::Result<Arc<CodexBroker>, ApiFailure> {
    let value = broker(state)?;
    let checking = value.clone();
    let status = tokio::task::spawn_blocking(move || checking.status())
        .await
        .map_err(|_| Error::UncertainEffect)?;
    if !status.available {
        return Err(ApiFailure(Error::Provider(
            status
                .error_code
                .unwrap_or_else(|| "codex_runtime_unavailable".into()),
        )));
    }
    Ok(value)
}
async fn status(
    State(s): State<CodexState>,
    Extension(o): Extension<RequestOwner>,
) -> Answer<CodexRuntimeStatus> {
    // Even status must validate a current actor session.
    s.runtime.store.codex_accounts(o.owner()).await?;
    let status = if let Some(b) = s.runtime.codex_broker() {
        tokio::task::spawn_blocking(move || b.status())
            .await
            .map_err(|_| Error::UncertainEffect)?
    } else {
        CodexBroker::unavailable()
    };
    Ok(Json(status))
}
async fn accounts(
    State(s): State<CodexState>,
    Extension(o): Extension<RequestOwner>,
) -> Answer<Vec<CodexAccount>> {
    Ok(Json(s.runtime.store.codex_accounts(o.owner()).await?))
}
async fn account(
    State(s): State<CodexState>,
    Extension(o): Extension<RequestOwner>,
    Path(id): Path<String>,
) -> Answer<CodexAccount> {
    Ok(Json(s.runtime.store.codex_account(o.owner(), &id).await?))
}
async fn create(
    State(s): State<CodexState>,
    Extension(o): Extension<RequestOwner>,
    body: Input<WriteCodexAccount>,
) -> Answer<CodexAccount> {
    Ok(Json(
        s.runtime
            .store
            .write_codex_account(o.owner(), None, &input(body)?)
            .await?,
    ))
}
async fn update(
    State(s): State<CodexState>,
    Extension(o): Extension<RequestOwner>,
    Path(id): Path<String>,
    body: Input<WriteCodexAccount>,
) -> Answer<CodexAccount> {
    Ok(Json(
        s.runtime
            .store
            .write_codex_account(o.owner(), Some(&id), &input(body)?)
            .await?,
    ))
}
async fn remove(
    State(s): State<CodexState>,
    Extension(o): Extension<RequestOwner>,
    Path(id): Path<String>,
    body: Input<DeleteLibraryEntry>,
) -> Answer<LibraryDeleted> {
    let body = input(body)?;
    let profile = match s.runtime.store.codex_account(o.owner(), &id).await {
        Ok(value) => value,
        Err(Error::NotFound) => {
            return Ok(Json(
                s.runtime
                    .store
                    .delete_codex_account(o.owner(), &id, &body)
                    .await?,
            ));
        }
        Err(error) => return Err(error.into()),
    };
    if profile.revision != body.expected_revision {
        return Err(Error::Conflict.into());
    }
    // Explicit logout is required first; deletion does not pretend to remove an OS credential.
    if s.runtime
        .store
        .codex_account_has_actions(o.owner(), &id)
        .await?
    {
        let b = ready_broker(&s).await?;
        let current = b
            .call(o.owner().namespace(), id.clone(), AccountOperation::Read)
            .await?;
        if !current["account"].is_null() {
            return Err(ApiFailure(Error::Validation(
                "codex_logout_before_delete".into(),
            )));
        }
        b.stop_account(&o.owner().namespace(), &id)?;
    }
    Ok(Json(
        s.runtime
            .store
            .delete_codex_account(o.owner(), &id, &body)
            .await?,
    ))
}
async fn account_status(
    State(s): State<CodexState>,
    Extension(o): Extension<RequestOwner>,
    Path(id): Path<String>,
) -> Answer<CodexAccountStatus> {
    let account = s.runtime.store.codex_account(o.owner(), &id).await?;
    let b = ready_broker(&s).await?;
    let current = b
        .call(o.owner().namespace(), id.clone(), AccountOperation::Read)
        .await?;
    let (email, plan_type) = (
        &current["account"]["email"],
        &current["account"]["planType"],
    );
    let text = |v: &Value| {
        v.as_str()
            .filter(|s| s.len() <= 512 && !s.chars().any(char::is_control))
            .map(str::to_owned)
    };
    let limits = if current["account"].is_null() {
        Value::Null
    } else {
        let raw = b
            .call(o.owner().namespace(), id.clone(), AccountOperation::Limits)
            .await?;
        sanitize_limits(&raw["rateLimits"])
    };
    let status = if current["account"].is_null() {
        "signed_out"
    } else if !limits["rateLimitReachedType"].is_null() {
        "limited"
    } else {
        "ready"
    };
    // Revalidate access after external IO before returning private metadata.
    s.runtime.store.codex_account(o.owner(), &id).await?;
    Ok(Json(CodexAccountStatus {
        account,
        status: status.into(),
        email: text(email),
        plan_type: text(plan_type),
        rate_limits: limits,
    }))
}
fn sanitize_limits(value: &Value) -> Value {
    let mut output = serde_json::Map::new();
    for name in ["primary", "secondary"] {
        if let Some(object) = value[name].as_object() {
            let mut limit = serde_json::Map::new();
            for key in ["usedPercent", "windowDurationMins", "resetsAt"] {
                if let Some(v) = object.get(key).filter(|v| v.is_number()) {
                    limit.insert(key.into(), v.clone());
                }
            }
            output.insert(name.into(), Value::Object(limit));
        }
    }
    for key in ["planType", "rateLimitReachedType"] {
        if let Some(v) = value[key].as_str().filter(|s| s.len() <= 200) {
            output.insert(key.into(), json!(v));
        }
    }
    Value::Object(output)
}
async fn model_list(
    State(s): State<CodexState>,
    Extension(o): Extension<RequestOwner>,
    Path(id): Path<String>,
) -> Answer<Vec<CodexModel>> {
    s.runtime.store.codex_account(o.owner(), &id).await?;
    let result = broker(&s)?
        .call(o.owner().namespace(), id.clone(), AccountOperation::Models)
        .await?;
    let result = models(&result)?;
    s.runtime.store.codex_account(o.owner(), &id).await?;
    Ok(Json(result))
}
async fn login(
    State(s): State<CodexState>,
    Extension(o): Extension<RequestOwner>,
    Path(id): Path<String>,
    body: Input<CodexLoginRequest>,
) -> Answer<CodexLoginView> {
    let body = input(body)?;
    let (operation, name) = match body.flow {
        CodexLoginFlow::Browser => (AccountOperation::LoginBrowser, "login_browser"),
        CodexLoginFlow::Device => (AccountOperation::LoginDevice, "login_device"),
    };
    if let Some(value) = s
        .runtime
        .store
        .original_codex_account_call(
            o.owner(),
            &id,
            &body.idempotency_key,
            body.expected_revision,
            name,
        )
        .await?
    {
        return Ok(Json(
            serde_json::from_value(value).map_err(|_| Error::Conflict)?,
        ));
    }
    let b = ready_broker(&s).await?;
    let original = s
        .runtime
        .store
        .prepare_codex_account_call(
            o.owner(),
            &id,
            &body.idempotency_key,
            body.expected_revision,
            name,
        )
        .await?;
    if let Some(v) = original {
        return Ok(Json(
            serde_json::from_value(v).map_err(|_| Error::Conflict)?,
        ));
    }
    let value = login_view(
        b.call(o.owner().namespace(), id.clone(), operation).await?,
        body.flow,
    )?;
    s.runtime
        .store
        .complete_codex_account_call(
            o.owner(),
            &id,
            &body.idempotency_key,
            &serde_json::to_value(&value).map_err(|_| Error::Conflict)?,
        )
        .await?;
    Ok(Json(value))
}
async fn logout(
    State(s): State<CodexState>,
    Extension(o): Extension<RequestOwner>,
    Path(id): Path<String>,
    body: Input<CodexAccountAction>,
) -> Answer<Value> {
    let body = input(body)?;
    if let Some(value) = s
        .runtime
        .store
        .original_codex_account_call(
            o.owner(),
            &id,
            &body.idempotency_key,
            body.expected_revision,
            "logout",
        )
        .await?
    {
        return Ok(Json(value));
    }
    let b = ready_broker(&s).await?;
    if let Some(v) = s
        .runtime
        .store
        .prepare_codex_account_call(
            o.owner(),
            &id,
            &body.idempotency_key,
            body.expected_revision,
            "logout",
        )
        .await?
    {
        return Ok(Json(v));
    }
    b.call(o.owner().namespace(), id.clone(), AccountOperation::Logout)
        .await?;
    let value = json!({"signed_out":true});
    s.runtime
        .store
        .complete_codex_account_call(o.owner(), &id, &body.idempotency_key, &value)
        .await?;
    Ok(Json(value))
}
