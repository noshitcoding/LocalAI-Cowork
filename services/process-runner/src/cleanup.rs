//! Retention applies only after exact signed completion and positive physical stop.
//! Unknown attempts retain their journals and consume the admission quota.
use super::*;
use cap_fs_ext::MetadataExt;

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum Proof {
    Execution { outcome: Signed<PhysicalOutcome> },
    Qualification { installed: Installed },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Finished {
    at: i64,
    proof: Proof,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Garbage {
    finished: Finished,
    ownership: platform::Ownership,
}

fn attempt_path(config: &Configuration, path: &FsPath) -> Result<()> {
    let root = config.data_directory.join("attempts");
    config::ordinary(&root, true)?;
    ensure!(
        path.parent() == Some(root.as_path()),
        "Cleanup is outside the exact receiver attempt directory"
    );
    match path.symlink_metadata() {
        Ok(_) => {
            config::ordinary(path, true)?;
            ensure!(
                path.canonicalize()?.parent() == Some(root.canonicalize()?.as_path()),
                "Resolved cleanup path escaped attempt directory"
            );
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .context("Invalid owned attempt name")?;
    ensure!(
        (name.len() == 64
            && name
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()))
            || name
                .strip_prefix("qualification-")
                .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok()),
        "Unrecognized receiver attempt directory"
    );
    Ok(())
}
fn ownership(config: &Configuration, path: &FsPath) -> Result<Option<platform::Ownership>> {
    attempt_path(config, path)?;
    let file = path.join("ownership.json");
    if !file.exists() {
        return Ok(None);
    }
    let record: platform::Ownership = serde_json::from_slice(&config::private_read(&file, 65536)?)?;
    ensure!(
        record.owned_directory == path
            && record.owned_directory.canonicalize()? == path.canonicalize()?
            && uuid::Uuid::parse_str(&record.nonce).is_ok(),
        "Ownership journal does not name this exact attempt"
    );
    ensure!(
        record.definition_sha256.len() == 64 && record.artifact.len() == 64,
        "Invalid ownership digest"
    );
    Ok(Some(record))
}
pub fn recover_owned(config: &Configuration, path: &FsPath) -> Result<()> {
    if let Some(record) = ownership(config, path)? {
        platform::recover(config, &record)?;
    }
    Ok(())
}
async fn verify(app: &App, path: &FsPath, proof: &Proof) -> Result<()> {
    let record =
        ownership(&app.0.config, path)?.context("Completed attempt ownership is unavailable")?;
    verify_owned(app, path, proof, &record).await
}
async fn verify_owned(
    app: &App,
    path: &FsPath,
    proof: &Proof,
    record: &platform::Ownership,
) -> Result<()> {
    attempt_path(&app.0.config, path)?;
    ensure!(
        record.owned_directory == path && uuid::Uuid::parse_str(&record.nonce).is_ok(),
        "Retained cleanup ownership differs"
    );
    let (definition, artifact) = match proof {
        Proof::Execution { outcome } => {
            outcome.verify("physical_outcome", &app.0.config.binding.runner_public_key)?;
            let receipt = app
                .0
                .broker
                .receipt(&outcome.payload.request.process_attempt_id)
                .await?;
            ensure!(
                receipt.payload.request == outcome.payload.request
                    && matches!(&receipt.payload.state,ExecutionState::Completed {result} if result==&outcome.payload.result)
                    && path.file_name().and_then(|s| s.to_str())
                        == Some(canonical_hash(&outcome.payload.request).as_str()),
                "Completion differs from exact durable admission"
            );
            (
                outcome
                    .payload
                    .request
                    .environment
                    .definition_sha256
                    .clone(),
                None,
            )
        }
        Proof::Qualification { installed } => {
            installed.request.verify(
                "environment_install",
                &app.0.config.binding.control_public_key,
            )?;
            installed.qualification.verify(
                "environment_qualification",
                &app.0.config.binding.runner_public_key,
            )?;
            let q = &installed.qualification.payload;
            ensure!(
                q.runner_id == app.0.config.binding.runner_id
                    && q.runner_generation == app.0.config.binding.generation
                    && q.adapter_build_sha256 == app.0.config.binding.adapter_build_sha256
                    && q.definition_sha256 == canonical_hash(&installed.request.payload.definition)
                    && q.artifact_sha256 == installed.request.payload.definition.artifact.sha256(),
                "Qualification completion identity differs"
            );
            ensure!(
                path.file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.starts_with("qualification-")),
                "Qualification attempt identity differs"
            );
            (q.definition_sha256.clone(), Some(q.artifact_sha256.clone()))
        }
    };
    ensure!(
        record.definition_sha256 == definition && artifact.is_none_or(|a| record.artifact == a),
        "Physical ownership differs from signed completion"
    );
    Ok(())
}
/// Reclaim costly physical resources immediately; snapshots remain for retention.
pub async fn finish(app: &App, path: &FsPath, proof: Proof) -> Result<()> {
    verify(app, path, &proof).await?;
    let config = app.0.config.clone();
    let path = path.to_path_buf();
    let work = path.clone();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let record = ownership(&config, &work)?.context("Owned resource missing")?;
        platform::release(&config, &record)?;
        config::durable_json(
            &work.join("resources-released.json"),
            &serde_json::json!({"ownership_sha256":canonical_hash(&record),"at":platform::now()}),
        )?;
        Ok(())
    })
    .await??;
    config::durable_json(
        &path.join("completed.json"),
        &Finished {
            at: platform::now(),
            proof,
        },
    )?;
    Ok(())
}
fn validate_tree(root: &FsPath) -> Result<()> {
    // Check the complete target immediately before recursive deletion. Never
    // follow links/reparse points or allow a mounted volume in the delete tree.
    let mut pending = vec![root.to_path_buf()];
    let mut count = 0usize;
    while let Some(path) = pending.pop() {
        count += 1;
        ensure!(count <= 100_000, "Retained tree exceeds cleanup bound");
        let meta = path.symlink_metadata()?;
        config::ordinary(&path, meta.is_dir())?;
        ensure!(path.starts_with(root), "Cleanup path escaped owned tree");
        if meta.is_dir() {
            for child in std::fs::read_dir(&path)? {
                pending.push(child?.path());
            }
        } else {
            ensure!(
                cap_std::fs::File::from_std(OpenOptions::new().read(true).open(&path)?)
                    .metadata()?
                    .nlink()
                    == 1,
                "Linked retained artifact cannot be deleted"
            );
        }
    }
    Ok(())
}
pub fn capacity(config: &Configuration) -> Result<()> {
    let root = config.data_directory.join("attempts");
    config::ordinary(&root, true)?;
    let count = std::fs::read_dir(root)?
        .take(config.max_retained_attempts + 1)
        .count();
    ensure!(
        count < config.max_retained_attempts,
        "Retained attempt quota exhausted; unresolved evidence is never deleted automatically"
    );
    Ok(())
}
fn retention_cutoff(now: i64, seconds: u64) -> i64 {
    let milliseconds = seconds.saturating_mul(1000).min(i64::MAX as u64) as i64;
    now.saturating_sub(milliseconds)
}
fn retention_elapsed(at: i64, now: i64, seconds: u64) -> bool {
    at > 0 && at <= now && at <= retention_cutoff(now, seconds)
}
pub async fn collect(app: &App) -> Result<()> {
    collect_garbage(app).await?;
    let root = app.0.config.data_directory.join("attempts");
    let cutoff = retention_cutoff(platform::now(), app.0.config.retention_seconds);
    let mut count = 0usize;
    for entry in std::fs::read_dir(&root)? {
        count += 1;
        ensure!(count <= 10_000, "Attempt registry requires operator review");
        let path = entry?.path();
        attempt_path(&app.0.config, &path)?;
        let finished = path.join("completed.json");
        if !finished.exists() {
            continue;
        }
        let finished: Finished =
            serde_json::from_slice(&config::private_read(&finished, 4 * 1024 * 1024)?)?;
        ensure!(
            finished.at > 0 && finished.at <= platform::now(),
            "Retention clock moved backwards"
        );
        if finished.at > cutoff {
            continue;
        }
        verify(app, &path, &finished.proof).await?;
        let ownership = ownership(&app.0.config, &path)?.context("Retained ownership missing")?;
        let journal = app.0.config.data_directory.join("garbage").join(format!(
            "{}.json",
            path.file_name()
                .context("Missing attempt name")?
                .to_string_lossy()
        ));
        // This separate durable journal survives deletion of any attempt file.
        // A crash halfway through remove_dir_all can resume cleanup, not execution.
        config::durable_json(
            &journal,
            &Garbage {
                finished,
                ownership,
            },
        )?;
        collect_one(app, &journal).await?;
    }
    Ok(())
}
async fn collect_one(app: &App, journal: &FsPath) -> Result<()> {
    let garbage: Garbage =
        serde_json::from_slice(&config::private_read(journal, 4 * 1024 * 1024)?)?;
    let path = garbage.ownership.owned_directory.clone();
    ensure!(
        journal.parent() == Some(app.0.config.data_directory.join("garbage").as_path())
            && journal.file_stem() == path.file_name(),
        "Garbage receipt names a foreign path"
    );
    ensure!(
        retention_elapsed(
            garbage.finished.at,
            platform::now(),
            app.0.config.retention_seconds
        ),
        "Retention is not elapsed"
    );
    verify_owned(app, &path, &garbage.finished.proof, &garbage.ownership).await?;
    let config = app.0.config.clone();
    let journal = journal.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<()> {
        platform::release(&config, &garbage.ownership)?;
        attempt_path(&config, &path)?;
        if path.exists() {
            validate_tree(&path)?;
            let root = config.data_directory.join("attempts");
            let dir = cap_std::fs::Dir::open_ambient_dir(&root, cap_std::ambient_authority())?;
            dir.remove_dir_all(path.file_name().context("Owned attempt name missing")?)?;
            config::sync_directory(&root)?;
        }
        config::ordinary(&journal, false)?;
        std::fs::remove_file(&journal)?;
        config::sync_directory(journal.parent().context("Missing garbage parent")?)?;
        Ok(())
    })
    .await??;
    Ok(())
}
pub async fn collect_garbage(app: &App) -> Result<()> {
    let root = app.0.config.data_directory.join("garbage");
    config::private_directory(&root)?;
    let mut count = 0usize;
    for entry in std::fs::read_dir(root)? {
        count += 1;
        ensure!(count <= 10_000, "Garbage registry exceeds bound");
        let path = entry?.path();
        ensure!(
            path.extension().is_some_and(|e| e == "json"),
            "Unrecognized garbage record"
        );
        collect_one(app, &path).await?;
    }
    Ok(())
}
pub async fn recover_finished(app: &App, path: &FsPath) -> Result<()> {
    // A crash after result persistence but before GC marking does not erase the
    // source proof. This only releases resources; it never restarts an effect.
    if path.join("completed.json").exists() {
        return Ok(());
    }
    let physical = path.join("physical-outcome.json");
    let qualification = path.join("qualification-outcome.json");
    if physical.exists() {
        let outcome = serde_json::from_slice(&config::private_read(&physical, 2 * 1024 * 1024)?)?;
        finish(app, path, Proof::Execution { outcome }).await?;
    } else if qualification.exists() {
        let installed =
            serde_json::from_slice(&config::private_read(&qualification, 2 * 1024 * 1024)?)?;
        finish(app, path, Proof::Qualification { installed }).await?;
    }
    Ok(())
}

#[cfg(test)]
mod retention_tests {
    use super::*;
    #[test]
    fn seven_days_retain_one_day_expire_eight_days_and_saturate_overflow() {
        let day = 24 * 60 * 60 * 1000i64;
        let now = 10 * day;
        let seven_days = 7 * 24 * 60 * 60;
        assert!(!retention_elapsed(now - day, now, seven_days));
        assert!(retention_elapsed(now - 8 * day, now, seven_days));
        assert!(retention_elapsed(now - 7 * day, now, seven_days));
        assert!(!retention_elapsed(now + 1, now, seven_days));
        assert!(!retention_elapsed(1, now, u64::MAX));
        assert_eq!(retention_cutoff(i64::MIN, u64::MAX), i64::MIN);
        assert_eq!(retention_cutoff(i64::MAX, u64::MAX), 0);
    }
}
