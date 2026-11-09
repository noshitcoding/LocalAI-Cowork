use cowork_contracts::*;
use cowork_daemon::{AppState, ProductServices, router_with_services};
use cowork_runtime::{ConfiguredProvider, Runtime};
use cowork_storage::Store;
use std::{path::PathBuf, sync::Arc};
use tokio_util::sync::CancellationToken;
mod authority_token_file;
mod document_workspace;
mod execution_target_config;
mod historical_import_config;
mod personal_money_config;
mod private_runner_authority_config;
#[cfg(windows)]
mod private_secret_file;
mod private_workflow_config;
mod process_runner_config;
mod server_key_config;
mod team_host_trust_config;
mod team_key_file;
mod toolbox_config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let data = PathBuf::from(std::env::var("COWORK_DATA_DIR").unwrap_or_else(|_| ".local".into()));
    let web = std::env::var("COWORK_WEB_DIR").ok().map(PathBuf::from);
    let port: u16 = std::env::var("COWORK_PORT")
        .unwrap_or_else(|_| "43821".into())
        .parse()?;
    let public_origin = match std::env::var("COWORK_PUBLIC_ORIGIN") {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(_) => anyhow::bail!("COWORK_PUBLIC_ORIGIN must contain a valid HTTPS origin"),
    };
    let origins = cowork_daemon::server_config::origins(port, public_origin.as_deref())?;
    let _data_guard = Arc::new(authority_token_file::private_directory(
        &data,
        web.as_deref(),
    )?);
    let store = if let Ok(url) = std::env::var("COWORK_DATABASE_URL") {
        Store::connect(&url).await?
    } else {
        Store::sqlite(&data.join("cowork-v2.sqlite3")).await?
    };
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments
        .first()
        .is_some_and(|arg| arg == "execution-target")
    {
        return execution_target_config::command(&store, &arguments[1..]).await;
    }
    if arguments.first().is_some_and(|arg| arg == "host-trust") {
        return team_host_trust_config::command(&store, &data, web.as_deref(), &arguments[1..])
            .await;
    }
    if arguments.first().is_some_and(|arg| arg == "server-keys") {
        return server_key_config::command(&store, &data, web.as_deref(), &arguments[1..]).await;
    }
    if arguments.first().is_some_and(|arg| arg == "private-runner") {
        return private_runner_authority_config::command(
            &store,
            &data,
            web.as_deref(),
            &arguments[1..],
        )
        .await;
    }
    if arguments
        .first()
        .is_some_and(|arg| arg == "private-workflow")
    {
        return private_workflow_config::command(&store, &data, web.as_deref(), &arguments[1..])
            .await;
    }
    if arguments
        .first()
        .is_some_and(|arg| arg == "historical-import-storage")
    {
        return historical_import_config::command(&store, &data, web.as_deref(), &arguments[1..])
            .await;
    }
    let authorities = authority_token_file::load_or_create(&data, web.as_deref())?;
    let token = authorities.daemon().to_owned();
    let native_token = authorities.native().to_owned();
    let (team_keys, team_key_id) = server_key_config::open(&store, &data, web.as_deref()).await?;
    let _audit_authority =
        cowork_identity::audit::AuditAuthority::new(team_keys.clone(), team_key_id)?;
    let toolbox = toolbox_config::open(&data, web.as_deref())?;
    let terminals = cowork_daemon::owner_terminal::TerminalService::open(toolbox.clone())?;
    let maintenance = cowork_daemon::maintenance::MaintenanceService::open(
        store.clone(),
        &data,
        Some(toolbox.clone()),
    )?;
    let objects = cowork_daemon::s3_store::S3Store::from_environment(data.join("team-objects"))?;
    // Persist incomplete offline effects as unknown before any worker may claim work.
    store.recover_offline_runs().await?;
    let offline_authority = Arc::new(cowork_storage::offline_execution::OfflineAuthority::new(
        team_keys.clone(),
        team_key_id,
        public_origin
            .clone()
            .unwrap_or_else(|| format!("http://127.0.0.1:{port}")),
    )?);
    let private_crew_configuration = private_workflow_config::configured(&data, web.as_deref())?;
    let android_devices =
        cowork_daemon::android_device_services::AndroidDeviceServices::from_environment_with_workflows(
            store.clone(),
            team_keys.clone(),
            team_key_id,
            &data,
            web.as_deref(),
            private_crew_configuration,
            _data_guard.clone(),
        )
        .await?;
    store
        .android_private_runner_register(android_devices.private_runner_binding().as_ref())
        .await?;
    let config_path = data.join("providers.json");
    let mut configured: Vec<ConfiguredProvider> = if config_path.exists() {
        serde_json::from_slice(&std::fs::read(config_path)?)?
    } else {
        Vec::new()
    };
    let allow_test = std::env::var("COWORK_ENABLE_TEST_PROVIDER").as_deref() == Ok("1");
    if allow_test {
        configured.push(ConfiguredProvider {
            profile: ProviderProfile {
                generation: Default::default(),
                id: "deterministic-test".into(),
                name: "Deterministic test provider".into(),
                kind: ProviderKind::Test,
                endpoint: String::new(),
                model: "test".into(),
                revision: 1,
                context_tokens: 32768,
                credential_required: false,
                credential_ready: true,
                capabilities: Default::default(),
            },
            credential_env: None,
        });
    }
    configured.retain(|provider| !provider.profile.id.is_empty());
    let mut active = Vec::new();
    for provider in configured {
        if !store.provider_retired(&provider.profile.id).await? {
            active.push(provider);
        }
    }
    let mut configured = active;
    for provider in &configured {
        store.insert_provider(&provider.profile).await?;
    }
    for profile in store.providers().await? {
        if let Some(existing) = configured.iter_mut().find(|p| p.profile.id == profile.id) {
            if profile.revision > existing.profile.revision {
                *existing = ConfiguredProvider {
                    profile,
                    credential_env: None,
                };
            }
        } else {
            configured.push(ConfiguredProvider {
                profile,
                credential_env: None,
            });
        }
    }
    if let Some(path) = personal_money_config::configured_path()? {
        let configuration = personal_money_config::read(&path, &data, web.as_deref())?;
        store.configure_personal_money(&configuration).await?;
    }
    // A missing env file cannot turn off an already activated installation.
    let shared_money = store.personal_money_configuration().await?.is_some();
    tracing::info!(
        personal_shared_money_enabled = shared_money,
        "Personal shared budget configuration inspected before worker startup"
    );
    let mut runtime = Runtime::new(store.clone(), configured, allow_test)?;
    runtime = runtime.with_codex_data_dir(data.clone());
    if let Some(runner) = process_runner_config::configured()? {
        runtime = runtime.with_configured_process_runner(runner).await?;
    }
    runtime = runtime.with_toolbox(toolbox.clone());
    runtime = runtime.with_personal_provider_keys(team_keys.clone(), team_key_id);
    if store.is_local_sqlite() {
        runtime = runtime.with_native_key_provider(Arc::new(cowork_secrets::DesktopKeyProvider));
    }
    let folders = data.join("folders.json");
    let configs: Vec<cowork_broker::FolderConfig> = if folders.exists() {
        serde_json::from_slice(&std::fs::read(folders)?)?
    } else {
        Vec::new()
    };
    let projects = store.projects().await?;
    anyhow::ensure!(
        configs
            .iter()
            .all(|c| projects.iter().any(|p| p.id == c.project_id)),
        "A configured folder references an unknown project"
    );
    runtime = runtime.with_file_broker(cowork_broker::FileBroker::open_with_protected(
        &data.join("file-broker"),
        configs,
        std::slice::from_ref(&data),
    )?);
    let mut identity_files = Vec::new();
    let identity = if std::env::var("COWORK_ENABLE_TENANT_IDENTITY").as_deref() != Ok("0") {
        let bootstrap = identity_secret(
            "COWORK_IDENTITY_BOOTSTRAP_SECRET_FILE",
            "identity-bootstrap",
            &data,
            web.as_deref(),
            &mut identity_files,
        )?;
        let settlement = identity_secret(
            "COWORK_IDENTITY_SETTLEMENT_SECRET_FILE",
            "identity-settlement",
            &data,
            web.as_deref(),
            &mut identity_files,
        )?;
        anyhow::ensure!(
            !bootstrap.trim().eq_ignore_ascii_case(&token)
                && !bootstrap.trim().eq_ignore_ascii_case(&native_token)
                && !settlement.trim().eq_ignore_ascii_case(&token)
                && !settlement.trim().eq_ignore_ascii_case(&native_token)
                && !bootstrap.trim().eq_ignore_ascii_case(settlement.trim()),
            "Tenant installation secrets must be separate from daemon authorities"
        );
        let identity = store.identity();
        identity
            .initialize_credentials(bootstrap.trim(), settlement.trim())
            .await?;
        Some(identity)
    } else {
        None
    };
    let stop = CancellationToken::new();
    let lifecycle = cowork_daemon::native_lifecycle::LifecycleControl::new(stop.clone())
        .with_terminals(terminals.clone());
    let mut team_tools = cowork_daemon::team_toolbox::TeamToolbox::new(toolbox.clone());
    let _team_control = if let Ok(image) = std::env::var("COWORK_TEAM_TOOL_IMAGE") {
        let directory =
            authority_token_file::private_directory(&data.join("team-containers"), web.as_deref())?;
        let host_workspace_root =
            PathBuf::from(std::env::var("COWORK_TEAM_TOOL_HOST_ROOT").map_err(|_| {
                anyhow::anyhow!("Team container tools require COWORK_TEAM_TOOL_HOST_ROOT")
            })?);
        team_tools = team_tools.with_container(cowork_daemon::team_toolbox::ContainerConfig {
            image,
            runtime: std::env::var("COWORK_TEAM_TOOL_RUNTIME")
                .ok()
                .filter(|value| !value.is_empty()),
            host_workspace_root,
            control_directory: directory.path().to_path_buf(),
            network: std::env::var("COWORK_TEAM_TOOL_NETWORK").unwrap_or_else(|_| "none".into()),
            host_id: std::env::var("COWORK_DOCKER_HOST_ID")
                .ok()
                .filter(|v| !v.is_empty())
                .map(|v| uuid::Uuid::parse_str(&v))
                .transpose()?,
            node_name: std::env::var("COWORK_DOCKER_NODE_NAME")
                .ok()
                .filter(|v| !v.is_empty()),
            node_bindings_file: std::env::var("COWORK_DOCKER_NODE_BINDINGS_FILE")
                .ok()
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
        })?;
        Some(directory)
    } else {
        None
    };
    if !std::env::args().any(|a| a == "--no-worker") {
        team_tools.recover_containers()?;
    }
    let team_tools = Arc::new(team_tools);
    let teams = cowork_storage::team_service::TeamService::new(
        store.clone(),
        team_keys.clone(),
        team_key_id,
        Arc::new(cowork_storage::team_service::LiteLlmExecutor::new(Some(
            team_tools.clone(),
        ))?),
    )
    .with_http_provider_origins(
        std::env::var("COWORK_TEAM_LITELLM_HTTP_ORIGINS")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect(),
    )?
    .with_mcp_origins(
        std::env::var("COWORK_TEAM_MCP_ALLOWED_ORIGINS")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect(),
    )?;
    let historical_guard = Arc::new(authority_token_file::private_directory(
        &data.join("historical-import-staging"),
        web.as_deref(),
    )?);
    let historical_imports =
        cowork_daemon::historical_import_routes::HistoricalImportServices::from_startup(
            team_keys.clone(),
            team_key_id,
            historical_guard.path().to_owned(),
            historical_guard,
            objects.clone(),
            runtime.files.clone(),
        );
    let android_worker = if !std::env::args().any(|a| a == "--no-worker") {
        let devices = android_devices.clone();
        let stop = stop.clone();
        Some(tokio::spawn(async move { devices.run(stop).await }))
    } else {
        None
    };
    let android_schedule_worker = if !std::env::args().any(|a| a == "--no-worker") {
        let devices = android_devices.clone();
        let stop = stop.clone();
        let admission = lifecycle.admission();
        Some(tokio::spawn(async move {
            devices.run_private_schedules(stop, admission).await
        }))
    } else {
        None
    };
    let product_worker = if !std::env::args().any(|a| a == "--no-worker") {
        let teams = teams.clone();
        let stop = stop.clone();
        Some(tokio::spawn(async move {
            loop {
                tokio::select! { _ = stop.cancelled() => break, result = teams.poll_once() => {
                    if result.is_err() { tracing::warn!("Team dispatch could not be completed; recorded effects are preserved"); }
                } }
                tokio::select! { _ = stop.cancelled() => break, _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {} }
            }
        }))
    } else {
        None
    };
    let schedule_worker = if !std::env::args().any(|a| a == "--no-worker") {
        let runtime = runtime.clone();
        let teams = teams.clone();
        let stop = stop.clone();
        let admission = lifecycle.admission();
        Some(tokio::spawn(async move {
            let mut clock = tokio::time::interval(std::time::Duration::from_secs(5));
            loop {
                tokio::select! { _ = stop.cancelled() => break, _ = clock.tick() => {
                    let _admission = admission.read().await;
                    if stop.is_cancelled() { break; }
                    if cowork_daemon::schedule_tick(&runtime).await.is_err() { tracing::warn!("Schedule tick could not be completed; uncertain dispatch remains paused"); }
                    if runtime.automation_tick().await.is_err() { tracing::warn!("Automation tick could not be completed; original admissions remain retained"); }
                    if teams.store().team_schedule_tick(&teams).await.is_err() { tracing::warn!("Team schedule tick could not be completed; uncertain dispatch remains paused"); }
                } }
            }
        }))
    } else {
        None
    };
    let worker = if !std::env::args().any(|a| a == "--no-worker") {
        let runtime = runtime.clone();
        let stop = stop.clone();
        Some(tokio::spawn(async move { runtime.serve(2, stop).await }))
    } else {
        None
    };
    let maintenance_worker = if !std::env::args().any(|a| a == "--no-worker") {
        let service = maintenance.clone();
        let stop = stop.clone();
        let admission = lifecycle.admission();
        Some(tokio::spawn(
            async move { service.run(stop, admission).await },
        ))
    } else {
        None
    };
    let signal = stop.clone();
    // A PTY input write can be waiting on a full OS pipe. Close owned terminals
    // when cancellation begins, before HTTP graceful shutdown waits for it.
    let terminal_shutdown = {
        let terminals = terminals.clone();
        let stop = stop.clone();
        tokio::spawn(async move {
            stop.cancelled().await;
            tokio::task::spawn_blocking(move || terminals.shutdown()).await
        })
    };
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        signal.cancel();
    });
    if std::env::args().any(|a| a == "--worker-only") {
        stop.cancelled().await;
    } else {
        let state = AppState {
            runtime: runtime.clone(),
            token: Arc::new(token),
            native_token: Some(Arc::new(native_token)),
            origins: Arc::new(origins),
        };
        let app = router_with_services(
            state.clone(),
            web,
            identity,
            Some(ProductServices {
                terminals: terminals.clone(),
                maintenance: maintenance.clone(),
                teams,
                toolbox: toolbox.clone(),
                team_tools: team_tools.clone(),
                objects,
                keys: team_keys,
                key_id: team_key_id,
                offline_authority,
                android_devices: android_devices.clone(),
                historical_imports: Some(historical_imports.clone()),
            }),
        );
        let bind: std::net::IpAddr = std::env::var("COWORK_BIND_ADDRESS")
            .unwrap_or_else(|_| "127.0.0.1".into())
            .parse()?;
        let app =
            cowork_daemon::native_lifecycle::attach(app, state, lifecycle, bind.is_loopback());
        let listener = tokio::net::TcpListener::bind((bind, port)).await?;
        tracing::info!(address=%listener.local_addr()?,"V2 single-owner daemon ready; token remains in the private data directory");
        axum::serve(listener, app)
            .with_graceful_shutdown(stop.clone().cancelled_owned())
            .await?;
    }
    stop.cancel();
    // Team guards still need the toolbox to confirm and remove their exact
    // containers when cancellation drops the admitted execution future.
    if let Some(worker) = product_worker {
        worker.await?;
    }
    if let Some(worker) = android_worker {
        worker.await?;
    }
    if let Some(worker) = android_schedule_worker {
        worker.await?;
    }
    android_devices.shutdown().await;
    historical_imports.shutdown().await;
    runtime.shutdown_offline().await;
    team_tools.shutdown().await;
    terminals.shutdown();
    terminal_shutdown.await??;
    toolbox.shutdown();
    if let Some(worker) = maintenance_worker {
        worker.await?;
    }
    if let Some(worker) = schedule_worker {
        worker.await?;
    }
    if let Some(worker) = worker {
        worker.await?;
    }
    store.close().await;
    // Keep the validated authority namespace and read-only files held until
    // both the worker and API have completely stopped.
    drop(authorities);
    drop(identity_files);
    Ok(())
}

fn identity_secret(
    variable: &str,
    name: &str,
    data: &std::path::Path,
    web: Option<&std::path::Path>,
    held: &mut Vec<authority_token_file::PrivateValue>,
) -> anyhow::Result<zeroize::Zeroizing<String>> {
    if std::env::var_os(variable).is_some() {
        return installation_secret(variable, web);
    }
    let file = authority_token_file::private_value(&data.join(name), web)?;
    let value = zeroize::Zeroizing::new(file.value().to_owned());
    held.push(file);
    Ok(value)
}

fn installation_secret(
    variable: &str,
    web: Option<&std::path::Path>,
) -> anyhow::Result<zeroize::Zeroizing<String>> {
    use std::io::Read;
    let path = PathBuf::from(
        std::env::var(variable)
            .map_err(|_| anyhow::anyhow!("Tenant installation secret file is not configured"))?,
    )
    .canonicalize()?;
    if let Some(web) = web {
        anyhow::ensure!(
            !path.starts_with(web.canonicalize()?),
            "Tenant installation secret file must be outside the served web directory"
        );
    }
    let file = std::fs::File::open(&path)?;
    let metadata = file.metadata()?;
    anyhow::ensure!(
        metadata.is_file() && metadata.len() <= 256,
        "Tenant installation secret file is invalid"
    );
    #[cfg(windows)]
    private_secret_file::validate(&file)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        anyhow::ensure!(
            metadata.permissions().mode() & 0o077 == 0,
            "Tenant installation secret file must be private"
        );
    }
    let mut secret = zeroize::Zeroizing::new(String::new());
    file.take(257).read_to_string(&mut secret)?;
    let value = secret.trim();
    anyhow::ensure!(
        (64..=128).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "Tenant installation secret must contain 64 to 128 hexadecimal characters"
    );
    Ok(secret)
}
