//! Open only an explicitly marked, checksummed database copy for upgrade QA.
//! This utility never discovers or opens the running application profile.
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let database = PathBuf::from(args.next().ok_or("fixture database required")?).canonicalize()?;
    if args.next().is_some()
        || database.file_name().and_then(|v| v.to_str()) != Some("database.sqlite3")
    {
        return Err("one marked fixture database required".into());
    }
    let directory = database.parent().ok_or("fixture parent missing")?;
    let suffix = directory
        .file_name()
        .and_then(|v| v.to_str())
        .and_then(|v| v.strip_prefix("cowork-migration-fixture-"))
        .ok_or("fixture directory required")?;
    if suffix.len() != 32 || !suffix.bytes().all(|v| v.is_ascii_hexdigit()) {
        return Err("fixture identity invalid".into());
    }
    let marker: serde_json::Value =
        serde_json::from_slice(&std::fs::read(directory.join("fixture.json"))?)?;
    let marked_database =
        PathBuf::from(marker["database"].as_str().ok_or("fixture path missing")?).canonicalize()?;
    if !matches!(
        marker["purpose"].as_str(),
        Some(
            "retained112_to113_copy"
                | "retained113_to114_copy"
                | "retained114_to115_copy"
                | "retained113_to116_copy"
        )
    ) || marker["live_source_modified"] != false
        || marked_database != database
        || marker["sha256"] != cowork_domain::hash(&std::fs::read(&database)?)
    {
        return Err("fixture marker or checksum changed".into());
    }
    let store = cowork_storage::Store::sqlite(&database).await?;
    store.close().await;
    println!("Marked database copy migrated and closed; no application services started.");
    Ok(())
}
