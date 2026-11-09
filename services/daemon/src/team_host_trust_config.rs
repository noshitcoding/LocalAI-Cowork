//! Private local operator enrollment; no login or synthetic tenant identity.
use cowork_daemon::team_toolbox::{ContainerConfig, TeamToolbox};
use cowork_storage::{Store, team_service::OperatorHostProfile};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
pub(super) async fn command(
    store: &Store,
    data: &Path,
    web: Option<&Path>,
    arguments: &[String],
) -> anyhow::Result<()> {
    let usage = "Usage: cowork-daemon host-trust inspect|activate|revoke <private-profile.json> <expected-revision> <original-operation-uuid>";
    let [action, source, revision, operation] = arguments else {
        anyhow::bail!("{usage}")
    };
    anyhow::ensure!(
        matches!(action.as_str(), "inspect" | "activate" | "revoke"),
        "{usage}"
    );
    let expected: i64 = revision.parse()?;
    let operation: uuid::Uuid = operation.parse()?;
    anyhow::ensure!(expected >= 0 && !operation.is_nil(), "{usage}");
    let source = super::authority_token_file::read_operator_file(
        &PathBuf::from(source),
        data,
        web,
        192 * 1024,
    )?;
    let profile: OperatorHostProfile = serde_json::from_slice(&source)?;
    let guard = Arc::new(super::authority_token_file::private_directory(
        &data.join("team-containers"),
        web,
    )?);
    let config = ContainerConfig {
        image: std::env::var("COWORK_TEAM_TOOL_IMAGE")?,
        runtime: std::env::var("COWORK_TEAM_TOOL_RUNTIME")
            .ok()
            .filter(|v| !v.is_empty()),
        host_workspace_root: PathBuf::from(std::env::var("COWORK_TEAM_TOOL_HOST_ROOT")?),
        control_directory: guard.path().to_path_buf(),
        network: std::env::var("COWORK_TEAM_TOOL_NETWORK").unwrap_or_else(|_| "none".into()),
        host_id: std::env::var("COWORK_DOCKER_HOST_ID")
            .ok()
            .filter(|v| !v.is_empty())
            .map(|v| v.parse())
            .transpose()?,
        node_name: std::env::var("COWORK_DOCKER_NODE_NAME")
            .ok()
            .filter(|v| !v.is_empty()),
        node_bindings_file: std::env::var_os("COWORK_DOCKER_NODE_BINDINGS_FILE")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from),
    };
    let tools_guard = super::authority_token_file::private_directory(
        &super::toolbox_config::host_trust_directory(data),
        web,
    )?;
    let tools = TeamToolbox::new(super::toolbox_config::open_host_trust(data, web)?)
        .with_container(config)?;
    let observed = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let _tools_guard = tools_guard;
        tools.original_host_binding()
    })
    .await??;
    profile.validate(&observed)?;
    let receipt = store
        .operator_team_host_trust(profile, observed, action, expected, operation)
        .await?;
    println!("{}", serde_json::to_string(&receipt)?);
    Ok(())
}
