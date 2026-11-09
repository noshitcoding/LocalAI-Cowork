//! Private workflow engine administration from the actual installation inputs.
use cowork_storage::Store;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub(super) fn configured(data: &Path, web: Option<&Path>) -> anyhow::Result<Option<Value>> {
    match std::env::var_os("LOCALAI_ANDROID_PRIVATE_CREW_CONFIG") {
        None => Ok(None),
        Some(path) if !path.is_empty() => Ok(Some(serde_json::from_slice(
            &super::authority_token_file::read_operator_file(
                &PathBuf::from(path),
                data,
                web,
                64 * 1024,
            )?,
        )?)),
        Some(_) => {
            anyhow::bail!("LOCALAI_ANDROID_PRIVATE_CREW_CONFIG must name a private operator file")
        }
    }
}

pub(super) async fn command(
    store: &Store,
    data: &Path,
    web: Option<&Path>,
    arguments: &[String],
) -> anyhow::Result<()> {
    let usage = "Usage: cowork-daemon private-workflow inspect <workflow|crew> | activate|revoke <workflow|crew> <expected-revision> <original-request-uuid>";
    let (action, kind) = match arguments {
        [action, kind] if action == "inspect" => (action.as_str(), kind.as_str()),
        [action, kind, _, _] if action == "activate" || action == "revoke" => {
            (action.as_str(), kind.as_str())
        }
        _ => anyhow::bail!("{usage}"),
    };
    anyhow::ensure!(matches!(kind, "workflow" | "crew"), "{usage}");
    let (keys, key_id) = super::server_key_config::open(store, data, web).await?;
    let runner = cowork_daemon::android_device_services::AndroidDeviceServices::private_runner_operator_binding(
        data, web, store.clone(), keys,
    )?;
    let configuration =
        cowork_daemon::android_device_services::PrivateWorkflowServices::operator_configuration(
            key_id.instance,
            runner.as_ref(),
            configured(data, web)?,
            kind,
        )?;
    let result = if action == "inspect" {
        store
            .android_workflow_engine_inspect(&configuration)
            .await?
    } else {
        let revision: i64 = arguments[2].parse()?;
        let request: uuid::Uuid = arguments[3].parse()?;
        anyhow::ensure!(
            revision > 0 && !request.is_nil(),
            "Expected revision and original request UUID must be valid"
        );
        store
            .android_workflow_engine_change(&configuration, revision, request, action == "activate")
            .await?
    };
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}
