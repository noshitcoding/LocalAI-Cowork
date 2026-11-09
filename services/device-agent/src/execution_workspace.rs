//! Run-scoped snapshot materialization and inert result export.
use super::*;

const MAX_FILES: usize = 2000;
const MAX_TOTAL: u64 = 256 * 1024 * 1024;
const MAX_FILE: u64 = 32 * 1024 * 1024;
const CHUNK: usize = 4 * 1024 * 1024;

pub(super) struct Inventory {
    pub files: Vec<Value>,
    pub fingerprints: BTreeMap<String, String>,
    pub total_bytes: u64,
}

pub(super) fn relative(value: &str) -> Result<PathBuf> {
    ensure!(
        !value.is_empty()
            && value.len() <= 1000
            && !value.starts_with('/')
            && !value.contains(['\\', ':', '\0'])
            && !value.chars().any(char::is_control)
            && value
                .split('/')
                .all(|c| !c.is_empty() && c != "." && c != ".."),
        "device_workspace_path_invalid"
    );
    for part in value.split('/') {
        ensure!(
            !part.ends_with(['.', ' ']),
            "device_workspace_path_has_windows_alias"
        );
        let base = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        let numbered = base.len() == 4
            && (base.starts_with("COM") || base.starts_with("LPT"))
            && matches!(base.as_bytes()[3], b'1'..=b'9');
        ensure!(
            !matches!(
                base.as_str(),
                "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
            ) && !numbered,
            "device_workspace_device_path_forbidden"
        );
    }
    Ok(PathBuf::from(value))
}

pub(super) async fn materialize(
    toolbox: &Arc<Toolbox>,
    control: &dyn DeviceControl,
    run: Uuid,
    lease: Uuid,
    project: Uuid,
    stop: &CancellationToken,
) -> Result<()> {
    let manifest = control
        .run_json(run, lease, "GET", "snapshot", Value::Null)
        .await?;
    ensure!(
        uuid(&manifest, "project_id")? == project,
        "device_snapshot_project_mismatch"
    );
    let files = manifest["files"]
        .as_array()
        .context("device_snapshot_files_missing")?;
    ensure!(files.len() <= MAX_FILES, "device_snapshot_file_limit");
    let total = manifest["total_bytes"]
        .as_u64()
        .context("device_snapshot_total_missing")?;
    ensure!(total <= MAX_TOTAL, "device_snapshot_byte_limit");
    let root = toolbox
        .workspace(&run.to_string())
        .map_err(|_| anyhow::anyhow!("device_workspace_unavailable"))?;
    let mut paths = BTreeSet::new();
    let mut measured = 0u64;
    for entry in files {
        ensure!(!stop.is_cancelled(), "device_run_canceled");
        let path = entry["path"]
            .as_str()
            .context("device_snapshot_path_missing")?;
        let relative = relative(path)?;
        ensure!(paths.insert(path), "device_snapshot_duplicate_path");
        let size = entry["size"]
            .as_u64()
            .context("device_snapshot_file_size_missing")?;
        ensure!(size <= MAX_FILE, "device_snapshot_file_exceeds_32_mib");
        measured = measured
            .checked_add(size)
            .context("device_snapshot_byte_overflow")?;
        ensure!(measured <= MAX_TOTAL, "device_snapshot_byte_limit");
        let target = root.join(relative);
        let parent = target.parent().context("device_snapshot_parent_missing")?;
        private_directory(parent)?;
        ensure!(
            parent.canonicalize()?.starts_with(&root),
            "device_snapshot_path_escapes_workspace"
        );
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&target)
            .context("device_snapshot_target_already_exists")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        let chunks = entry["chunks"]
            .as_array()
            .context("device_snapshot_chunks_missing")?;
        ensure!(chunks.len() <= 2000, "device_snapshot_chunk_limit");
        let mut written = 0u64;
        let mut fingerprint = Sha256::new();
        for chunk in chunks {
            let expected = chunk["digest"]
                .as_str()
                .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
                .context("device_snapshot_chunk_digest_invalid")?;
            let chunk_size = chunk["plaintext_size"]
                .as_u64()
                .filter(|n| *n <= CHUNK as u64)
                .context("device_snapshot_chunk_size_invalid")?;
            let bytes = tokio::select! {_=stop.cancelled()=>bail!("device_run_canceled"),b=control.get_chunk(run,lease,expected)=>b?};
            ensure!(
                bytes.len() as u64 == chunk_size && digest(&bytes) == expected,
                "device_snapshot_chunk_integrity_failed"
            );
            written = written
                .checked_add(chunk_size)
                .context("device_snapshot_byte_overflow")?;
            ensure!(written <= size, "device_snapshot_file_size_mismatch");
            file.write_all(&bytes)?;
            fingerprint.update(&bytes);
        }
        ensure!(written == size, "device_snapshot_file_size_mismatch");
        if let Some(expected) = entry["digest"].as_str() {
            let actual: String = fingerprint
                .finalize()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            ensure!(actual == expected, "device_snapshot_file_integrity_failed");
        }
        file.sync_all()?;
    }
    ensure!(measured == total, "device_snapshot_total_size_mismatch");
    Ok(())
}

pub(super) fn inventory(toolbox: &Arc<Toolbox>, run: Uuid) -> Result<Inventory> {
    let root = toolbox
        .workspace(&run.to_string())
        .map_err(|_| anyhow::anyhow!("device_workspace_unavailable"))?;
    let mut paths = Vec::new();
    let mut directories = vec![root.clone()];
    let mut visited = 0usize;
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            visited += 1;
            ensure!(visited <= 10_000, "device_workspace_inventory_limit");
            let metadata = fs::symlink_metadata(entry.path())?;
            ensure!(
                !metadata.is_symlink(),
                "device_workspace_alias_cannot_be_exported"
            );
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                ensure!(
                    metadata.file_attributes() & 0x400 == 0,
                    "device_workspace_reparse_point_cannot_be_exported"
                );
            }
            if entry.path() == root.join("tmp") || entry.file_name() == ".git" {
                continue;
            }
            if metadata.is_dir() {
                directories.push(entry.path());
            } else if metadata.is_file() {
                paths.push(entry.path());
                ensure!(paths.len() <= MAX_FILES, "device_workspace_file_limit");
            } else {
                bail!("device_workspace_nonregular_file_cannot_be_exported")
            }
        }
    }
    paths.sort();
    let mut fingerprints = BTreeMap::new();
    let mut files = Vec::new();
    let mut total = 0u64;
    for path in paths {
        let relative_path = path
            .strip_prefix(&root)?
            .to_string_lossy()
            .replace('\\', "/");
        relative(&relative_path)?;
        let bytes = toolbox
            .read_output_file(&run.to_string(), &relative_path)
            .map_err(|_| anyhow::anyhow!("device_workspace_inventory_read_failed"))?;
        ensure!(
            bytes.len() as u64 <= MAX_FILE,
            "device_workspace_file_exceeds_32_mib"
        );
        total = total
            .checked_add(bytes.len() as u64)
            .context("device_workspace_byte_overflow")?;
        ensure!(total <= MAX_TOTAL, "device_workspace_byte_limit");
        let sha = digest(&bytes);
        let chunks=bytes.chunks(CHUNK).map(|chunk|json!({"digest":digest(chunk),"plaintext_size":chunk.len(),"ciphertext_size":chunk.len()})).collect::<Vec<_>>();
        fingerprints.insert(relative_path.clone(), sha.clone());
        files.push(json!({"path":relative_path,"size":bytes.len(),"digest":sha,"chunks":chunks}));
    }
    Ok(Inventory {
        files,
        fingerprints,
        total_bytes: total,
    })
}

pub(super) fn diff(before: &Inventory, after: &Inventory) -> Value {
    json!({"added":after.fingerprints.keys().filter(|p|!before.fingerprints.contains_key(*p)).collect::<Vec<_>>(),"changed":after.fingerprints.iter().filter(|(p,h)|before.fingerprints.get(*p).is_some_and(|old|old!=*h)).map(|(p,_)|p).collect::<Vec<_>>(),"removed":before.fingerprints.keys().filter(|p|!after.fingerprints.contains_key(*p)).collect::<Vec<_>>(),"total_bytes":after.total_bytes})
}

pub(super) async fn publish(
    toolbox: &Arc<Toolbox>,
    control: &dyn DeviceControl,
    run: Uuid,
    lease: Uuid,
    project: Uuid,
    inventory: &Inventory,
    stop: &CancellationToken,
) -> Result<Uuid> {
    let session=control.run_json(run,lease,"POST","result-snapshot",json!({"project_id":project,"total_bytes":inventory.total_bytes,"files":inventory.files})).await?;
    let manifest = uuid(&session, "manifest_id")?;
    let missing = session["missing_chunks"]
        .as_array()
        .context("device_result_snapshot_chunks_missing")?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .context("device_result_snapshot_digest_invalid")
        })
        .collect::<Result<BTreeSet<_>>>()?;
    let mut uploaded = BTreeSet::new();
    for file in &inventory.files {
        ensure!(!stop.is_cancelled(), "device_run_canceled");
        let path = file["path"]
            .as_str()
            .context("device_result_snapshot_path_missing")?;
        let bytes = toolbox
            .read_output_file(&run.to_string(), path)
            .map_err(|_| anyhow::anyhow!("device_result_snapshot_read_failed"))?;
        ensure!(
            file["digest"].as_str() == Some(digest(&bytes).as_str()),
            "device_workspace_changed_during_export"
        );
        for chunk in bytes.chunks(CHUNK) {
            let sha = digest(chunk);
            if missing.contains(&sha) && uploaded.insert(sha.clone()) {
                tokio::select! {_=stop.cancelled()=>bail!("device_run_canceled"),v=control.put_chunk(run,lease,manifest,&sha,chunk.to_vec())=>{v?;}}
            }
        }
    }
    ensure!(
        uploaded == missing,
        "device_result_snapshot_missing_unknown_chunk"
    );
    control
        .run_json(
            run,
            lease,
            "POST",
            &format!("result-snapshot/{manifest}/commit"),
            Value::Null,
        )
        .await?;
    Ok(manifest)
}
