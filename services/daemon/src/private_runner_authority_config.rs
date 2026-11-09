//! Explicit installation operator changes to one original runner authority.
//! Reading a binding or inspecting a slot does not activate it or start guests.
use cowork_storage::Store;
use serde_json::Value;
use std::path::Path;

pub(super) async fn command(
    store: &Store,
    data: &Path,
    web: Option<&Path>,
    arguments: &[String],
) -> anyhow::Result<()> {
    let usage = "Usage: cowork-daemon private-runner inspect <private-binding-file|--configured> | revoke|activate <private-binding-file|--configured> <expected-revision>";
    let (action, file) = match arguments {
        [action, file] if action == "inspect" => (action.as_str(), file),
        [action, file, _] if action == "revoke" || action == "activate" => (action.as_str(), file),
        _ => anyhow::bail!("{usage}"),
    };
    let binding: Value = if file == "--configured" {
        let (keys, _) = super::server_key_config::open(store, data, web).await?;
        cowork_daemon::android_device_services::AndroidDeviceServices::private_runner_operator_binding(
            data, web, store.clone(), keys,
        )?.ok_or_else(|| anyhow::anyhow!("No private runner is configured"))?
    } else {
        let raw = super::authority_token_file::read_operator_file(
            Path::new(file),
            data,
            web,
            128 * 1024,
        )?;
        serde_json::from_slice(&raw)?
    };
    let result = if action == "inspect" {
        store.android_private_runner_inspect(&binding).await?
    } else {
        let revision: i64 = arguments[2].parse()?;
        anyhow::ensure!(revision > 0, "Expected runner revision must be positive");
        if action == "revoke" {
            store
                .android_private_runner_revoke(&binding, revision)
                .await?
        } else {
            store
                .android_private_runner_activate(&binding, revision)
                .await?
        }
    };
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}
