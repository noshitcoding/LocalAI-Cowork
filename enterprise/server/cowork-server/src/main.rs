mod api_cluster;
mod api_replica;
mod auth;
mod auth_assurance;
mod chat_workspaces;
mod client_origins;
mod config;
mod contract_artifacts;
mod db;
mod desktop;
mod enterprise_grants;
mod enterprise_offline;
mod enterprise_transfers;
mod enterprise_workflows;
mod error;
mod event_notifications;
mod executor_ws;
mod fleet;
mod fleet_content;
mod fleet_execution;
mod governance;
mod instance_disk_quotas;
mod instance_gateway;
mod management;
mod management_transport;
mod managed_resources;
mod v2_runtime_contract;
mod mcp_bindings;
mod oidc;
mod oidc_assurance;
mod operations;
mod organisation_mfa;
mod organization;
mod passkey;
mod providers;
mod push;
mod routes;
mod run_resume;
mod runner_admission;
mod runner_desktop_proof;
mod runtime_status;
mod storage;
mod sync;
mod terminal;
mod worker;
mod workflow;

use std::{future::IntoFuture, sync::Arc};

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
use tokio::{sync::watch, task::JoinSet};
use tower_http::{
    catch_panic::CatchPanicLayer,
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::TraceLayer,
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub management_mode: bool,
    pub auth_secrets: Option<Arc<storage::SecretStore>>,
    pub bootstrap_token_digest: [u8; 32],
    pub bootstrap_user_id: Uuid,
    pub lease_seconds: i64,
    pub server_capabilities: Arc<Vec<Capability>>,
    pub object_store: Option<Arc<storage::ObjectStore>>,
    pub runner: Option<Arc<desktop::RunnerControl>>,
    pub executor_hub: executor_ws::ExecutorHub,
    pub api_replica: Option<api_replica::ApiReplicaGuard>,
    pub api_cluster: Option<api_cluster::ApiCluster>,
    pub event_notifications: event_notifications::EventNotifications,
    pub push: Option<Arc<push::PushService>>,
    pub webauthn: Option<Arc<webauthn_rs::Webauthn>>,
    pub oidc: Option<Arc<oidc::OidcService>>,
    pub organisation_oidc: Arc<management::sso::Registry>,
    pub directory: Arc<management::directory::catalog::Registry>,
}

impl AppState {
    pub async fn ensure_api_active(&self) -> anyhow::Result<()> {
        match self.api_ownership() {
            Some(owner) => owner.ensure_active().await,
            None if self.management_mode => Ok(()),
            None => anyhow::bail!("API admission has no valid ownership guard"),
        }
    }

    pub fn api_ownership(&self) -> Option<runtime_status::ApiOwnership> {
        match (&self.api_replica, &self.api_cluster) {
            (Some(single), None) => Some(runtime_status::ApiOwnership::Single(single.clone())),
            (None, Some(cluster)) => Some(runtime_status::ApiOwnership::Cluster(cluster.clone())),
            _ => None,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    v2_runtime_contract::validate_embedded().context("invalid common V2 contract artifact")?;
    if std::env::args().collect::<Vec<_>>().get(1..)
        == Some(&["--v2-runtime-contract".to_owned()][..])
    {
        println!("{}", v2_runtime_contract::descriptor()?);
        return Ok(());
    }
    if std::env::args().collect::<Vec<_>>().get(1..)
        == Some(&["--execution-admission-contract".to_owned()][..])
    {
        println!("{}", fleet_execution::compatibility_contract());
        return Ok(());
    }
    if std::env::args().collect::<Vec<_>>().get(1..)
        == Some(&["--organisation-mfa-contract".to_owned()][..])
    {
        println!(
            "{}",
            serde_json::json!({"version":1,"current_organisation_policy":true,"original_session_admission":true,"worker_and_device_fences":true,"scoped_recovery":true,"stream_fences":true})
        );
        return Ok(());
    }
    dotenvy::dotenv().ok();
    init_tracing();
    let config = Config::from_env()?;
    if config.mode == ProcessMode::Management {
        instance_gateway::validate_configuration()?;
    }
    let pool = PgPoolOptions::new()
        .max_connections(config.database_max_connections)
        .connect(&config.database_url)
        .await
        .context("failed to connect to PostgreSQL")?;
    db::migrate(&pool)
        .await
        .context("database migration failed")?;
    let arguments: Vec<String> = std::env::args().collect();
    if arguments.get(1).is_some_and(|a| a == "directory") {
        anyhow::ensure!(
            config.mode == ProcessMode::Management,
            "directory operator CLI requires management mode"
        );
        let key = storage::SecretStore::from_key(config.auth_master_key.as_deref().ok_or_else(
            || anyhow::anyhow!("directory operator CLI requires COWORK_AUTH_MASTER_KEY"),
        )?)?;
        let catalog = management::directory::catalog::load(true)?;
        return management::directory::run_cli(&pool, &key, &catalog, &arguments[2..]).await;
    }
    if arguments
        .get(1)
        .is_some_and(|argument| argument == "oidc-assurance")
    {
        return oidc_assurance::run_cli(&pool, &arguments[2..]).await;
    }
    organisation_mfa::initialize()?;
    instance_disk_quotas::initialize()?;
    fleet_execution::initialize()?;
    fleet_execution::spawn_relay(pool.clone());

    // Acquire the selected owner before admitting worker work in all mode.
    // Explicit cluster configuration replaces the singleton session lock.
    let (api_replica, api_cluster) = if let Some(cluster) = config.api_cluster.clone() {
        (
            None,
            Some(api_cluster::ApiCluster::acquire(pool.clone(), cluster).await?),
        )
    } else if matches!(config.mode, ProcessMode::Api | ProcessMode::All) {
        (
            Some(api_replica::ApiReplicaGuard::acquire(&config.database_url).await?),
            None,
        )
    } else {
        (None, None)
    };

    let (stop, stopped) = watch::channel(false);
    let (publication_stop, publication_stopped) = watch::channel(false);
    let publication = tokio::spawn(runtime_status::publish(
        pool.clone(),
        config.clone(),
        publication_stopped,
    ));
    let mut services = JoinSet::new();
    if matches!(
        config.mode,
        ProcessMode::Api | ProcessMode::All | ProcessMode::Management
    ) {
        services.spawn(serve_api(
            pool.clone(),
            config.clone(),
            stopped.clone(),
            api_replica,
            api_cluster,
        ));
    }
    if matches!(config.mode, ProcessMode::Worker | ProcessMode::All) {
        services.spawn(worker::run_until_shutdown(
            pool,
            config,
            runtime_status::wait_for_shutdown(stopped),
        ));
    }
    let mut failure = None;
    tokio::select! {
        _ = shutdown_signal() => {},
        result = services.join_next() => record_service_result(result, &mut failure),
    }
    runtime_status::set_draining();
    stop.send_replace(true);
    // Await the worker's owned tasks and bounded cleanup instead of dropping a
    // running worker future when HTTP or a signal finishes first.
    while let Some(result) = services.join_next().await {
        record_service_result(Some(result), &mut failure);
    }
    publication_stop.send_replace(true);
    let mut publication = publication;
    if tokio::time::timeout(std::time::Duration::from_secs(3), &mut publication)
        .await
        .is_err()
    {
        publication.abort();
        let _ = publication.await;
    }
    failure.map_or(Ok(()), Err)
}

fn record_service_result(
    result: Option<Result<Result<()>, tokio::task::JoinError>>,
    failure: &mut Option<anyhow::Error>,
) {
    let error = match result {
        Some(Ok(Err(error))) => Some(error),
        Some(Err(error)) => Some(error.into()),
        _ => None,
    };
    if let Some(error) = error {
        tracing::error!(?error, "server service stopped");
        if failure.is_none() {
            *failure = Some(error);
        }
    }
}

async fn serve_api(
    pool: PgPool,
    config: Config,
    shutdown: watch::Receiver<bool>,
    api_replica: Option<api_replica::ApiReplicaGuard>,
    api_cluster: Option<api_cluster::ApiCluster>,
) -> Result<()> {
    let management_mode = config.mode == ProcessMode::Management;
    let event_notifications =
        event_notifications::EventNotifications::start(config.database_url.clone());
    if management_mode {
        management::initialize()?;
        management::register_execution_admission(&pool).await?;
    }
    let object_store = config
        .object_store
        .as_ref()
        .map(storage::ObjectStore::from_config)
        .transpose()
        .context("invalid object-store configuration")?
        .map(Arc::new);
    let auth_secrets = config
        .auth_master_key
        .as_deref()
        .map(storage::SecretStore::from_key)
        .transpose()
        .context("invalid authentication encryption key")?
        .or_else(|| object_store.as_ref().map(|store| store.secret_store()))
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
        management_mode,
        auth_secrets,
        bootstrap_token_digest: Sha256::digest(config.bootstrap_token.as_bytes()).into(),
        bootstrap_user_id: config.bootstrap_user_id,
        lease_seconds: config.lease_duration.as_secs() as i64,
        server_capabilities: Arc::new(config.server_capabilities),
        object_store,
        runner: config
            .runner_url
            .clone()
            .zip(config.runner_signing_key.clone())
            .map(|(url, key)| desktop::RunnerControl::new(url, key))
            .transpose()?
            .map(Arc::new),
        executor_hub: api_cluster.as_ref().map_or_else(
            || executor_ws::ExecutorHub::new(api_replica.clone()),
            |cluster| executor_ws::ExecutorHub::new_cluster(cluster.clone()),
        ),
        api_replica: api_replica.clone(),
        api_cluster: api_cluster.clone(),
        event_notifications,
        push: push::PushService::from_config(&config.push)
            .context("invalid push configuration")?
            .map(Arc::new),
        webauthn,
        oidc,
        organisation_oidc: Arc::new(management::sso::load(management_mode)?),
        directory: Arc::new(management::directory::catalog::load(management_mode)?),
    };
    if management_mode {
        management::sso::reconcile(&state).await?;
    }
    oidc_assurance::register(&state).await?;
    if management_mode {
        management::directory::register(&state).await?;
    }
    let ownership = state.api_ownership();
    let mut peer_services = JoinSet::new();
    if let Some(cluster) = &api_cluster {
        let peers = cluster
            .start_peer_server(state.executor_hub.clone())
            .await?;
        peer_services.spawn(peers.run(state.runner.clone()));
    }
    let mut background = JoinSet::new();
    if management_mode {
        background.spawn(management::directory::run(state.clone(), shutdown.clone()));
    }
    background.spawn(push::run_dispatcher(state.clone()));
    if !management_mode {
        background.spawn(fleet_content::announce(state.pool.clone()));
        background.spawn(enterprise_workflows::run(state.clone(), shutdown.clone()));
    }
    let protected_auth = Router::new()
        .merge(v2_runtime_contract::router())
        .route("/auth/bootstrap", post(auth::bootstrap_admin))
        .route("/auth/logout", post(auth::logout))
        .route("/auth/assurance", get(auth_assurance::status))
        .route("/auth/assurance/verify", post(auth_assurance::verify))
        .route("/auth/sessions", get(auth::list_sessions))
        .route(
            "/auth/sessions/{session_id}",
            axum::routing::delete(auth::revoke_session),
        )
        .route("/auth/oidc/link/start", post(oidc::start_link))
        .route(
            "/auth/organisations/{org}/oidc/link/start",
            post(oidc::link_organisation),
        )
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
        .route("/operations/metrics", get(operations::metrics))
        .route(
            "/operations/support-bundle",
            get(operations::support_bundle),
        )
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_auth,
        ));
    let protected = Router::new()
        .merge(protected_auth.clone())
        .route("/content-access", get(fleet_content::status))
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
        .route("/push/config", get(push::configuration))
        .route("/push/subscriptions", post(push::register).get(push::list))
        .route(
            "/push/subscriptions/{subscription_id}",
            axum::routing::delete(push::remove),
        )
        .route("/version", get(routes::version))
        .route("/capabilities", get(routes::capabilities))
        .route("/managed-resources", get(managed_resources::execution::list))
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
            post(organization::set_team_member).get(organization::list_team_members),
        )
        .route(
            "/teams/{team_id}/members/{user_id}",
            axum::routing::delete(organization::remove_team_member),
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
        .route(
            "/projects/{project_id}/mcp-bindings",
            get(mcp_bindings::list),
        )
        .route(
            "/projects/{project_id}/mcp-bindings/{mcp_entity_id}",
            put(mcp_bindings::set).delete(mcp_bindings::delete),
        )
        .route(
            "/provider-profiles",
            post(providers::create).get(providers::list),
        )
        .route(
            "/provider-profiles/{profile_id}",
            put(providers::update).delete(providers::delete),
        )
        .route(
            "/provider-profiles/{profile_id}/secret",
            put(providers::set_secret),
        )
        .route("/threads", post(organization::create_thread))
        .route("/thread-runs", post(routes::create_thread_run))
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
        .route(
            "/tasks/{task_id}",
            get(workflow::get_task).delete(workflow::delete_task),
        )
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
        .route("/thread-schedules", post(workflow::create_thread_schedule))
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
        .route("/runs/{run_id}/waiting", get(routes::get_run_waiting))
        .route("/runs/{run_id}/cancel", post(routes::cancel_run))
        .route("/runs/{run_id}/resume", post(run_resume::resume_agent_run))
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
        .merge(enterprise_grants::router().layer(middleware::from_fn(
            enterprise_grants::original_actor_boundary,
        )))
        .merge(enterprise_workflows::router().layer(middleware::from_fn(
            enterprise_grants::original_actor_boundary,
        )))
        .merge(enterprise_transfers::router().layer(middleware::from_fn(
            enterprise_grants::original_actor_boundary,
        )))
        .merge(storage::content_federation::router())
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
        .merge(enterprise_grants::agent_router())
        .merge(enterprise_offline::router())
        .merge(enterprise_transfers::agent_router())
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
        .route("/auth/bootstrap/status", get(auth::bootstrap_status))
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
    let api = if management_mode {
        public_auth
            .merge(protected_auth)
            .merge(management::invitations::public_router())
            .route(
                "/auth/organisations/{org}/oidc/config",
                get(management::sso::public_config),
            )
            .route(
                "/auth/organisations/{org}/oidc/start",
                post(oidc::start_organisation),
            )
    } else {
        public_auth
            .merge(protected)
            .merge(agent)
            .merge(fleet::router())
            .merge(enterprise_transfers::private_router())
            .merge(enterprise_workflows::private_router())
            .merge(storage::content_federation::proof_router())
    }
    .layer(middleware::from_fn(auth::browser_session_boundary));

    let app = Router::new()
        .route("/healthz", get(routes::health))
        .route("/readyz", get(routes::ready))
        .nest("/api/v1", api);
    let app = if management_mode {
        app.nest("/management/v1", management::router(state.clone()))
            .nest("/management/scim/v2", management::directory::scim_router())
            .nest("/management/runner/v1", management::provisioner_router())
    } else {
        app
    };
    let app = app
        .layer(middleware::from_fn_with_state(state.clone(), api_ownership_boundary))
        .layer(client_origins::layer(&std::env::var("COWORK_DESKTOP_ORIGINS").unwrap_or_default())?)
        .layer(CatchPanicLayer::new())
        .layer(TraceLayer::new_for_http().make_span_with(|request: &axum::http::Request<axum::body::Body>| {
            tracing::info_span!("http_request", method = %request.method(),
                request_id = request.headers().get("x-request-id").and_then(|value| value.to_str().ok()).unwrap_or("unknown"))
        }))
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(config.listen_addr)
        .await
        .with_context(|| format!("failed to bind {}", config.listen_addr))?;
    tracing::info!(address = %config.listen_addr, "control plane listening");
    let http = axum::serve(listener, app)
        .with_graceful_shutdown(runtime_status::wait_for_shutdown(shutdown.clone()))
        .into_future();
    tokio::pin!(http);
    let lost = async {
        match ownership.as_ref() {
            Some(guard) => guard.wait_until_lost().await,
            None => std::future::pending::<()>().await,
        }
    };
    tokio::pin!(lost);
    let outcome = tokio::select! {
        biased;
        _ = runtime_status::wait_for_shutdown(shutdown) => {
            if let Some(guard) = &ownership { guard.begin_draining(); }
            match tokio::time::timeout(std::time::Duration::from_secs(10), &mut http).await {
                Ok(result) => result.context("HTTP server failed"),
                Err(_) => Err(anyhow::anyhow!("HTTP drain exceeded ten seconds")),
            }
        },
        _ = &mut lost => Err(anyhow::anyhow!("API ownership lost; HTTP server fenced")),
        result = peer_services.join_next(), if !peer_services.is_empty() => {
            match result {
                Some(Ok(Err(error))) => Err(error.context("private peer service failed")),
                Some(Err(error)) => Err(error.into()),
                _ => Err(anyhow::anyhow!("private peer service stopped unexpectedly")),
            }
        },
        result = &mut http => result.context("HTTP server failed"),
    };
    if let Some(guard) = &ownership {
        guard.begin_draining();
    }
    if tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while let Some(result) = peer_services.join_next().await {
            if let Err(error) = result {
                tracing::warn!(?error, "private peer service join failed");
            }
        }
    })
    .await
    .is_err()
    {
        peer_services.abort_all();
        while peer_services.join_next().await.is_some() {}
    }
    outcome
}

async fn api_ownership_boundary(
    axum::extract::State(state): axum::extract::State<AppState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, error::ApiError> {
    state
        .ensure_api_active()
        .await
        .map_err(|error| error::ApiError::Unavailable(error.to_string()))?;
    Ok(next.run(request).await)
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
