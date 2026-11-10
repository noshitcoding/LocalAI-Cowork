//! Native, capability-relative observation. No synthetic run or ambient path is used.
use super::*;
use cowork_domain::file_watch::{path_excluded, path_selected, validate_file_watch};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};

pub struct PreparedFileWatchScan {
    pub files: Vec<FileWatchObservation>,
    pub ignored_entries: u32,
    /// Existing entries which cannot be observed as regular files. They are not
    /// evidence that previously observed paths (or descendants) were removed.
    pub unavailable_paths: Vec<String>,
    _lock: ResourceLock,
    _root: Arc<Root>,
}

fn limit(reason: &str) -> Error {
    Error::Validation(reason.into())
}
fn check_time(deadline: Instant) -> Result<()> {
    if Instant::now() >= deadline {
        return Err(limit("automation_watch_scan_timeout"));
    }
    Ok(())
}

impl FileBroker {
    pub fn prepare_file_watch_scan(
        &self,
        project: &str,
        watch: &AutomationFileWatch,
    ) -> Result<PreparedFileWatchScan> {
        validate_file_watch(&AutomationEventSource::File, Some(watch))?;
        self.validate_grants(project, std::slice::from_ref(&watch.folder))?;
        let root = self.lookup_root(&watch.folder.id, false)?;
        let lock = self.lock(&root, false)?;
        let directory = open_directory(&root.dir, &relative_path(&watch.path, true)?)?;
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut pending = vec![(directory, watch.path.clone(), 0usize)];
        let mut files = Vec::new();
        let mut ignored_entries = 0u32;
        let mut unavailable_paths = Vec::new();
        let mut entries = 0usize;
        let mut directories = 1usize;
        let mut bytes = 0u64;
        while let Some((directory, prefix, depth)) = pending.pop() {
            check_time(deadline)?;
            for entry in directory.entries().map_err(storage_error)? {
                check_time(deadline)?;
                entries += 1;
                if entries > 20_000 {
                    return Err(limit("automation_watch_entry_limit"));
                }
                let entry = entry.map_err(storage_error)?;
                let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                    ignored_entries += 1;
                    continue;
                };
                let path = if prefix == "." {
                    name.clone()
                } else {
                    format!("{prefix}/{name}")
                };
                if relative_path(&path, false).is_err() || path_excluded(watch, &path) {
                    ignored_entries += 1;
                    continue;
                }
                let metadata = directory
                    .symlink_metadata(&name)
                    .map_err(|_| Error::FileConflict)?;
                if metadata.is_symlink() {
                    ignored_entries += 1;
                    unavailable_paths.push(path);
                    continue;
                }
                if metadata.is_dir() {
                    if !watch.recursive {
                        continue;
                    }
                    directories += 1;
                    if depth >= 32 || directories > 1024 {
                        return Err(limit("automation_watch_directory_limit"));
                    }
                    pending.push((
                        open_directory(&directory, Path::new(&name))?,
                        path,
                        depth + 1,
                    ));
                    continue;
                }
                if !metadata.is_file() || metadata.nlink() != 1 {
                    ignored_entries += 1;
                    unavailable_paths.push(path);
                    continue;
                }
                if !path_selected(watch, &path) {
                    continue;
                }
                if files.len() >= watch.max_files as usize {
                    return Err(limit("automation_watch_file_limit"));
                }
                let mut options = OpenOptions::new();
                options.read(true).follow(FollowSymlinks::No);
                #[cfg(unix)]
                {
                    use cap_std::fs::OpenOptionsExt;
                    options.custom_flags(libc::O_NONBLOCK);
                }
                let mut file = directory
                    .open_with(&name, &options)
                    .map_err(|_| Error::FileConflict)?;
                let before = file.metadata().map_err(storage_error)?;
                if !before.is_file()
                    || before.nlink() != 1
                    || before.dev() != metadata.dev()
                    || before.ino() != metadata.ino()
                {
                    return Err(Error::FileConflict);
                }
                if before.len() > watch.max_file_bytes {
                    return Err(limit("automation_watch_file_too_large"));
                }
                let mut digest = Sha256::new();
                let mut size = 0u64;
                let mut buffer = [0u8; 65_536];
                loop {
                    check_time(deadline)?;
                    let read = file.read(&mut buffer).map_err(storage_error)?;
                    if read == 0 {
                        break;
                    }
                    size = size.checked_add(read as u64).ok_or(Error::Forbidden)?;
                    bytes = bytes.checked_add(read as u64).ok_or(Error::Forbidden)?;
                    if size > watch.max_file_bytes {
                        return Err(limit("automation_watch_file_too_large"));
                    }
                    if bytes > watch.max_scan_bytes {
                        return Err(limit("automation_watch_byte_limit"));
                    }
                    digest.update(&buffer[..read]);
                }
                let after = file.metadata().map_err(storage_error)?;
                let current = directory
                    .symlink_metadata(&name)
                    .map_err(|_| Error::FileConflict)?;
                if size != before.len()
                    || after.len() != before.len()
                    || before.modified().ok() != after.modified().ok()
                    || after.nlink() != 1
                    || current.is_symlink()
                    || current.dev() != before.dev()
                    || current.ino() != before.ino()
                {
                    return Err(Error::FileConflict);
                }
                files.push(FileWatchObservation {
                    path,
                    sha256: format!("{:x}", digest.finalize()),
                    size,
                });
            }
        }
        self.validate_grants(project, std::slice::from_ref(&watch.folder))?;
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(PreparedFileWatchScan {
            files,
            ignored_entries,
            unavailable_paths,
            _lock: lock,
            _root: root,
        })
    }
}
