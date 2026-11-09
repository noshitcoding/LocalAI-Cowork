//! Local operator command, never an HTTP or tenant-session registration route.
use cowork_storage::Store;

pub(super) async fn command(store: &Store, arguments: &[String]) -> anyhow::Result<()> {
    let usage = "Usage: cowork-daemon execution-target inspect --configured | register|activate|revoke --configured <expected-revision> (COWORK_PROCESS_RUNNER_CONFIG required)";
    let (action, expected) = match arguments {
        [action, source] if action == "inspect" && source == "--configured" => {
            (action.as_str(), None)
        }
        [action, source, revision]
            if matches!(action.as_str(), "register" | "activate" | "revoke")
                && source == "--configured" =>
        {
            (action.as_str(), Some(revision.parse::<i64>()?))
        }
        _ => anyhow::bail!(usage),
    };
    let configuration =
        super::process_runner_config::configured()?.ok_or_else(|| anyhow::anyhow!(usage))?;
    let identity = configuration.execution_identity();
    let result = match action {
        "inspect" => store.execution_target(&identity).await?,
        "register" => Some(
            store
                .register_execution_target(&identity, expected.expect("validated shape"))
                .await?,
        ),
        "activate" | "revoke" => Some(
            store
                .set_execution_target_active(
                    &identity,
                    expected.expect("validated shape"),
                    action == "activate",
                )
                .await?,
        ),
        _ => unreachable!(),
    };
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}
