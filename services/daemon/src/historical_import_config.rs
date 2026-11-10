//! Explicit installation operator quota configuration; no tenant HTTP authority.
use cowork_storage::Store;
use std::path::Path;
pub(super) async fn command(
    store: &Store,
    data: &Path,
    web: Option<&Path>,
    arguments: &[String],
) -> anyhow::Result<()> {
    if arguments.len() != 2 {
        anyhow::bail!(
            "Usage: cowork-daemon historical-import-storage <expected-policy-revision> <storage-bytes>"
        )
    }
    let revision: i64 = arguments[0].parse()?;
    let bytes: u64 = arguments[1].parse()?;
    let (_, key) = super::server_key_config::open(store, data, web).await?;
    let original = store
        .configure_historical_import_limit(key.instance, revision, bytes)
        .await?;
    println!("{}", serde_json::to_string(&original)?);
    Ok(())
}
