mod auth;
mod config;
mod contract_artifacts;
mod db;
mod desktop;
mod error;
mod executor_ws;
mod governance;
mod oidc;
mod operations;
mod organization;
mod passkey;
mod push;
mod routes;
mod storage;
mod sync;
mod terminal;
mod worker;
mod workflow;

use std::sync::Arc;

use anyhow::{Context, Result};
use axum::{
    extract::DefaultBodyLimit,
    middleware,
    routing::{get, post, put},
    Router,
};
use config::{Config, ProcessMode};
use cowork_contracts::Capability;
use sha2::{Digest, Sha256};
use sqlx::{postgres::PgPoolOptions, PgPool};
use tower_http::{catch_panic::CatchPanicLayer, trace::TraceLayer};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub bootstrap_token_digest: [u8; 32],
    pub bootstrap_user_id: Uuid,
    pub lease_seconds: i64,
    pub server_capabilities: Arc<Vec<Capability>>,
    pub object_store: Option<Arc<storage::ObjectStore>>,
    pub runner: Option<Arc<desktop::RunnerControl>>,
    pub executor_hub: executor_ws::ExecutorHub,
    pub push: Option<Arc<push::PushService>>,
    pub webauthn: Option<Arc<webauthn_rs::Webauthn>>,
    pub oidc: Option<Arc<oidc::OidcService>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    init_tracing();
    let config = Config::from_env()?;
    let pool = PgPoolOptions::new()
        .max_connections(config.database_max_connections)
        .connect(&config.database_url)
        .await
        .context("failed to connect to PostgreSQL")?;
    db::migrate(&pool)
        .await
        .context("database migration failed")?;

    match config.mode {
        ProcessMode::Api => serve_api(pool, config).await,
        ProcessMode::Worker => worker::run(pool, config).await,
        ProcessMode::All => {
            let api_pool = pool.clone();
            let api_config = config.clone();
            tokio::select! {
                result = serve_api(api_pool, api_config) => result,
                result = worker::run(pool, config) => result,
                _ = shutdown_signal() => Ok(()),
            }
        }
    }
}

async fn serve_api(pool: PgPool, config: Config) -> Result<()> {
    let object_store = config
        .object_store
        .as_ref()
        .map(storage::ObjectStore::from_config)
        .transpose()
        .context("invalid object-store configuration")?
        .map(Arc::new);
    let webauthn = config
        .passkeys
        .as_ref()
        .map(passkey::build_webauthn)
        .transpose()
        .context("invalid WebAuthn configuration")?
        .map(Arc::new);
    let oidc = oidc::build(config.oidc.as_ref())
        .await
        .context("invalid OIDC configuration")?
        .map(Arc::new);
    let state = AppState {
        pool,
        bootstrap_token_digest: Sha256::digest(config.bootstrap_token.as_bytes()).into(),
        bootstrap_user_id: config.bootstrap_user_id,
        lease_seconds: config.lease_duration.as_secs() as i64,
        server_capabilities: Arc::new(config.server_capabilities),
        object_store,
        runner: config
            .runner_url
            .clone()
            .zip(config.runner_signing_key.clone())
            .map(|(url, key)| Arc::new(desktop::RunnerControl::new(url, key))),
        executor_hub: executor_ws::ExecutorHub::default(),
        push: push::PushService::from_config(&config.push)
            .context("invalid push configuration")?
            .map(Arc::new),
        webauthn,
        oidc,
    };
    tokio::spawn(push::run_dispatcher(state.clone()));
    let protected = Router::new()
        .route("/auth/bootstrap", post(auth::bootstrap_admin))
        .route("/auth/logout", post(auth::logout))
        .route("/auth/sessions", get(auth::list_sessions))
        .route(
            "/auth/sessions/{session_id}",
            axum::routing::delete(auth::revoke_session),
        )
        .route("/auth/oidc/link/start", post(oidc::start_link))
        .route("/auth/reauthenticate", post(auth::reauthenticate))
        .route("/auth/totp", get(auth::totp_status))
        .route("/auth/totp/setup", post(auth::setup_totp))
        .route("/auth/totp/enable", post(auth::enable_totp))
        .route("/auth/totp/disable", post(auth::disable_totp))
        .route(
            "/auth/totp/recovery-codes",
            post(auth::regenerate_recovery_codes),
        )
        .route("/auth/passkeys", get(passkey::list))
        .route(
            "/auth/passkeys/register/start",
            post(passkey::start_registration),
        )
        .route(
            "/auth/passkeys/register/finish",
            post(passkey::finish_registration),
        )
        .route(
            "/auth/passkeys/{passkey_id}",
            axum::routing::delete(passkey::remove),
        )
        .route("/auth/invitations", post(auth::create_invitation))
        .route(
            "/support-grants",
            post(governance::create_support_grant).get(governance::list_support_grants),
        )
        .route(
            "/support-grants/{grant_id}",
            axum::routing::delete(governance::revoke_support_grant),
        )
        .route(
            "/quotas/{scope_type}/{scope_id}",
            get(governance::get_quota).put(governance::set_quota),
        )
        .route("/operations/metrics", get(operations::metrics))
        .route(
            "/operations/support-bundle",
            get(operations::support_bundle),
        )
        .route("/push/config", get(push::configuration))
        .route("/push/subscriptions", post(push::register).get(push::list))
        .route(
            "/push/subscriptions/{subscription_id}",
            axum::routing::delete(push::remove),
        )
        .route("/version", get(routes::version))
        .route("/capabilities", get(routes::capabilities))
        .route(
            "/sync/changes",
            post(sync::push_changes).get(sync::pull_changes),
        )
        .route("/sync/events", get(sync::change_events))
        .route("/sync/entities/{entity_type}", get(sync::list_entities))
        .route(
            "/teams",
            post(organization::create_team).get(organization::list_teams),
        )
        .route(
            "/teams/{team_id}/members",
            post(organization::set_team_member),
        )
        .route(
            "/projects",
            post(organization::create_project).get(organization::list_projects),
        )
        .route(
            "/projects/{project_id}",
            get(organization::get_project)
                .put(organization::update_project)
                .delete(organization::delete_project),
        )
        .route(
            "/projects/{project_id}/members",
            post(organization::set_project_member),
        )
        .route("/threads", post(organization::create_thread))
        .route(
            "/threads/{thread_id}",
            put(organization::update_thread).delete(organization::delete_thread),
        )
        .route(
            "/threads/{thread_id}/messages",
            post(routes::create_thread_message).get(routes::list_thread_messages),
        )
        .route(
            "/projects/{project_id}/threads",
            get(organization::list_project_threads),
        )
        .route(
            "/tasks",
            post(workflow::create_task).get(workflow::list_tasks),
        )
        .route("/tasks/{task_id}", get(workflow::get_task))
        .route(
            "/tasks/{task_id}/versions",
            post(workflow::create_task_version),
        )
        .route(
            "/tasks/{task_id}/release",
            post(workflow::release_task_version),
        )
        .route(
            "/schedules",
            post(workflow::create_schedule).get(workflow::list_schedules),
        )
        .route(
            "/schedules/{schedule_id}",
            axum::routing::put(workflow::update_schedule).delete(workflow::delete_schedule),
        )
        .route("/snapshots", post(storage::begin_snapshot_upload))
        .route(
            "/snapshots/{manifest_id}",
            get(storage::get_snapshot).delete(storage::delete_snapshot),
        )
        .route(
            "/snapshots/{manifest_id}/upload",
            get(storage::snapshot_upload_status),
        )
        .route(
            "/snapshots/{manifest_id}/chunks/{digest}",
            put(storage::upload_snapshot_chunk)
                .get(storage::download_snapshot_chunk)
                .layer(DefaultBodyLimit::max(storage::MAX_CHUNK_BYTES)),
        )
        .route(
            "/snapshots/{manifest_id}/commit",
            post(storage::commit_snapshot),
        )
        .route(
            "/projects/{project_id}/versions",
            post(storage::create_project_version).get(storage::list_project_versions),
        )
        .route(
            "/projects/{project_id}/versions/{version_id}/apply",
            post(storage::apply_project_version),
        )
        .route(
            "/projects/{project_id}/merge-review",
            get(storage::review_project_merge),
        )
        .route(
            "/projects/{project_id}/merge-apply",
            post(storage::apply_project_merge),
        )
        .route(
            "/executor-pools",
            post(organization::create_executor_pool).get(organization::list_executor_pools),
        )
        .route(
            "/executor-pools/{pool_id}/projects",
            post(organization::grant_executor_pool),
        )
        .route("/runs", post(routes::create_run).get(routes::list_runs))
        .route("/runs/{run_id}", get(routes::get_run))
        .route("/runs/{run_id}/cancel", post(routes::cancel_run))
        .route("/runs/{run_id}/events", get(routes::run_events))
        .route(
            "/runs/{run_id}/terminal-sessions",
            post(terminal::create_session),
        )
        .route("/runs/{run_id}/artifacts", get(storage::list_run_artifacts))
        .route(
            "/runs/{run_id}/attachments",
            post(storage::upload_run_attachment)
                .layer(DefaultBodyLimit::max(storage::MAX_AGENT_ARTIFACT_BYTES)),
        )
        .route(
            "/runs/{run_id}/artifacts/{artifact_id}",
            get(storage::download_run_artifact),
        )
        .route("/runs/{run_id}/approvals", get(workflow::list_approvals))
        .route(
            "/runs/{run_id}/approvals/{approval_id}/resolve",
            post(workflow::resolve_approval),
        )
        .route(
            "/runs/{run_id}/input-requests",
            get(workflow::list_input_requests),
        )
        .route(
            "/runs/{run_id}/input-requests/{input_id}/respond",
            post(workflow::submit_input_response),
        )
        .route(
            "/runs/{run_id}/checkpoints",
            get(workflow::list_checkpoints),
        )
        .route(
            "/runs/{run_id}/desktop-sessions",
            post(desktop::start_session).get(desktop::list_sessions),
        )
        .route(
            "/runs/{run_id}/desktop-sessions/{session_id}",
            axum::routing::delete(desktop::stop_session),
        )
        .route(
            "/runs/{run_id}/desktop-sessions/{session_id}/tickets",
            post(desktop::create_stream_ticket),
        )
        .route(
            "/executors",
            post(routes::register_executor).get(routes::list_executors),
        )
        .route(
            "/executors/{executor_id}/credentials",
            post(routes::create_executor_credential),
        )
        .route(
            "/executors/{executor_id}/credentials/{credential_id}/revoke",
            post(routes::revoke_executor_credential),
        )
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_auth,
        ));
    let agent = Router::new()
        .route(
            "/agent/executors/{executor_id}/register",
            post(routes::register_executor_agent),
        )
        .route(
            "/agent/executors/{executor_id}/connect",
            get(executor_ws::connect),
        )
        .route(
            "/agent/executors/{executor_id}/desktop-streams/{stream_id}",
            get(executor_ws::connect_desktop_stream),
        )
        .route(
            "/agent/executors/{executor_id}/heartbeat",
            post(routes::heartbeat_executor),
        )
        .route(
            "/agent/executors/{executor_id}/sync/changes",
            get(sync::agent_pull_changes).post(sync::agent_push_changes),
        )
        .route(
            "/agent/executors/{executor_id}/sync/entities/{entity_type}",
            get(sync::agent_list_entities),
        )
        .route(
            "/agent/executors/{executor_id}/claim",
            post(routes::claim_executor_run),
        )
        .route(
            "/agent/executors/{executor_id}/runs/{run_id}/heartbeat",
            post(routes::renew_executor_lease),
        )
        .route(
            "/agent/executors/{executor_id}/runs/{run_id}/events",
            post(routes::append_executor_event),
        )
        .route(
            "/agent/executors/{executor_id}/runs/{run_id}/approvals",
            post(workflow::create_approval),
        )
        .route(
            "/agent/executors/{executor_id}/runs/{run_id}/approvals/{approval_id}",
            get(workflow::get_executor_approval),
        )
        .route(
            "/agent/executors/{executor_id}/runs/{run_id}/input-requests",
            post(workflow::create_input_request),
        )
        .route(
            "/agent/executors/{executor_id}/runs/{run_id}/input-requests/{input_id}",
            get(workflow::get_executor_input_request),
        )
        .route(
            "/agent/executors/{executor_id}/runs/{run_id}/checkpoints",
            post(workflow::create_checkpoint),
        )
        .route(
            "/agent/executors/{executor_id}/runs/{run_id}/snapshot",
            get(storage::get_executor_run_snapshot),
        )
        .route(
            "/agent/executors/{executor_id}/runs/{run_id}/snapshot/chunks/{digest}",
            get(storage::download_executor_run_chunk),
        )
        .route(
            "/agent/executors/{executor_id}/runs/{run_id}/result-snapshot",
            get(storage::get_executor_run_result_snapshot)
                .post(storage::begin_executor_run_result_snapshot),
        )
        .route(
            "/agent/executors/{executor_id}/runs/{run_id}/result-snapshot/{manifest_id}/chunks/{digest}",
            put(storage::upload_executor_run_result_chunk)
                .layer(DefaultBodyLimit::max(storage::MAX_CHUNK_BYTES)),
        )
        .route(
            "/agent/executors/{executor_id}/runs/{run_id}/result-snapshot/{manifest_id}/commit",
            post(storage::commit_executor_run_result_snapshot),
        )
        .route(
            "/agent/executors/{executor_id}/runs/{run_id}/artifacts",
            post(storage::upload_executor_run_artifact)
                .layer(DefaultBodyLimit::max(storage::MAX_AGENT_ARTIFACT_BYTES)),
        )
        .route(
            "/agent/executors/{executor_id}/runs/{run_id}/complete",
            post(routes::complete_executor_run),
        )
        .route(
            "/agent/executors/{executor_id}/runs/{run_id}/fail",
            post(routes::fail_executor_run),
        )
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_executor_auth,
        ));
    let public_auth = Router::new()
        .route("/openapi.json", get(contract_artifacts::openapi))
        .route(
            "/schemas/contracts.json",
            get(contract_artifacts::json_schemas),
        )
        .route("/auth/oidc/config", get(oidc::configuration))
        .route("/auth/oidc/start", post(oidc::start))
        .route("/auth/oidc/callback", get(oidc::callback))
        .route("/auth/login", post(auth::password_login))
        .route("/auth/native/authorize", post(auth::native_authorize))
        .route("/auth/native/token", post(auth::native_token))
        .route(
            "/auth/native/passkey/authorize",
            get(passkey::native_authorization_page),
        )
        .route(
            "/auth/native/passkey/client.js",
            get(passkey::native_authorization_script),
        )
        .route(
            "/auth/native/passkey/start",
            post(passkey::start_native_authentication),
        )
        .route(
            "/auth/native/passkey/finish",
            post(passkey::finish_native_authentication),
        )
        .route("/auth/refresh", post(auth::refresh_session))
        .route("/auth/browser/refresh", post(auth::browser_refresh_session))
        .route("/auth/invitations/accept", post(auth::accept_invitation))
        .route(
            "/auth/passkeys/authenticate/start",
            post(passkey::start_authentication),
        )
        .route(
            "/auth/passkeys/authenticate/finish",
            post(passkey::finish_authentication),
        )
        .route(
            "/desktop-sessions/{session_id}/stream",
            get(desktop::stream),
        )
        .route(
            "/terminal-sessions/{session_id}/stream",
            get(terminal::stream),
        );
    let api = public_auth
        .merge(protected)
        .merge(agent)
        .layer(middleware::from_fn(auth::browser_session_boundary));

    let app = Router::new()
        .route("/healthz", get(routes::health))
        .route("/readyz", get(routes::ready))
        .nest("/api/v1", api)
        .layer(CatchPanicLayer::new())
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(config.listen_addr)
        .await
        .with_context(|| format!("failed to bind {}", config.listen_addr))?;
    tracing::info!(address = %config.listen_addr, "control plane listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("HTTP server failed")
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "cowork_server=info,tower_http=info".into());
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().json())
        .init();
}
