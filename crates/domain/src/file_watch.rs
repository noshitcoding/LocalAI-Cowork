//! Persistent debounce transitions. Observations are supplied by the native broker.
use crate::{Error, Result};
use cowork_contracts::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

fn invalid() -> Error {
    Error::Validation("automation_file_watch_invalid".into())
}

pub fn valid_watch_path(path: &str, root: bool) -> bool {
    if root && path == "." {
        return true;
    }
    !path.is_empty()
        && path.len() <= 4096
        && path.split('/').all(|part| {
            !part.is_empty()
                && !matches!(part, "." | "..")
                && part.len() <= 255
                && !part.ends_with(['.', ' '])
                && !part.to_ascii_lowercase().starts_with(".cowork-")
                && !part.chars().any(|c| {
                    c.is_control() || matches!(c, '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
                })
        })
}

pub fn validate_file_watch(
    source: &AutomationEventSource,
    watch: Option<&AutomationFileWatch>,
) -> Result<()> {
    let Some(watch) = watch else {
        return if *source == AutomationEventSource::File {
            Err(invalid())
        } else {
            Ok(())
        };
    };
    if *source != AutomationEventSource::File
        || watch.folder.writable
        || watch.folder.order_id.is_some()
        || !valid_watch_path(&watch.path, true)
        || watch.changes.is_empty()
        || watch.changes.len() > 3
        || watch.extensions.len() > 32
        || watch.exclude_paths.len() > 32
        || !(1000..=300_000).contains(&watch.debounce_ms)
        || !(5000..=3_600_000).contains(&watch.poll_interval_ms)
        || !(1..=10_000).contains(&watch.max_files)
        || !(1..=256 * 1024 * 1024).contains(&watch.max_file_bytes)
        || watch.max_scan_bytes < watch.max_file_bytes
        || watch.max_scan_bytes > 1024 * 1024 * 1024
    {
        return Err(invalid());
    }
    let changes: BTreeSet<_> = watch.changes.iter().map(|v| format!("{v:?}")).collect();
    if changes.len() != watch.changes.len()
        || watch
            .exclude_paths
            .iter()
            .any(|path| !valid_watch_path(path, false))
        || watch.extensions.iter().any(|ext| {
            ext.len() < 2
                || ext.len() > 20
                || !ext.starts_with('.')
                || !ext[1..]
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
    {
        return Err(invalid());
    }
    Ok(())
}

pub fn path_selected(watch: &AutomationFileWatch, path: &str) -> bool {
    let relative = if watch.path == "." {
        path
    } else if let Some(relative) = path.strip_prefix(&format!("{}/", watch.path)) {
        relative
    } else {
        return false;
    };
    if !watch.recursive && relative.contains('/') {
        return false;
    }
    if path_excluded(watch, path) {
        return false;
    }
    watch.extensions.is_empty()
        || watch
            .extensions
            .iter()
            .any(|ext| path.to_ascii_lowercase().ends_with(ext))
}

pub fn path_excluded(watch: &AutomationFileWatch, path: &str) -> bool {
    let normalized = |value: &str| {
        if cfg!(windows) {
            value.to_lowercase()
        } else {
            value.to_owned()
        }
    };
    let path_key = normalized(path);
    watch.exclude_paths.iter().any(|excluded| {
        let excluded = normalized(excluded);
        path_key == excluded || path_key.starts_with(&format!("{excluded}/"))
    })
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileWatchState {
    pub initialized: bool,
    pub last_observed_at: Option<i64>,
    pub files: BTreeMap<String, TrackedFile>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrackedFile {
    pub accepted: Option<FileWatchObservation>,
    pub pending: Option<PendingFileChange>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingFileChange {
    pub event_id: String,
    pub first_seen_at: i64,
    pub change: FileWatchChange,
    pub target: Option<FileWatchObservation>,
}
#[derive(Clone, Debug)]
pub struct ReadyFileChange {
    pub path: String,
    pub event_id: String,
    pub change: FileWatchChange,
    pub before: Option<FileWatchObservation>,
    pub after: Option<FileWatchObservation>,
}

pub fn observe_files(
    state: &mut FileWatchState,
    watch: &AutomationFileWatch,
    files: &[FileWatchObservation],
    observed_at: i64,
) -> Result<Vec<ReadyFileChange>> {
    observe_files_with_unavailable(state, watch, files, &[], observed_at)
}

pub fn observe_files_with_unavailable(
    state: &mut FileWatchState,
    watch: &AutomationFileWatch,
    files: &[FileWatchObservation],
    unavailable_paths: &[String],
    observed_at: i64,
) -> Result<Vec<ReadyFileChange>> {
    validate_file_watch(&AutomationEventSource::File, Some(watch))?;
    if observed_at < 0
        || state
            .last_observed_at
            .is_some_and(|previous| observed_at < previous)
    {
        return Err(Error::Validation("automation_watch_clock_rollback".into()));
    }
    if files.len() > watch.max_files as usize {
        return Err(invalid());
    }
    if unavailable_paths.len() > 20_000
        || unavailable_paths.iter().any(|path| {
            !valid_watch_path(path, false)
                || (watch.path != "."
                    && path != &watch.path
                    && !path.starts_with(&format!("{}/", watch.path)))
        })
    {
        return Err(invalid());
    }
    let unavailable: BTreeSet<String> = unavailable_paths
        .iter()
        .map(|path| {
            if cfg!(windows) {
                path.to_lowercase()
            } else {
                path.clone()
            }
        })
        .collect();
    let mut observed = BTreeMap::new();
    let mut total_bytes = 0u64;
    for file in files {
        total_bytes = total_bytes.checked_add(file.size).ok_or_else(invalid)?;
        if total_bytes > watch.max_scan_bytes {
            return Err(Error::Validation("automation_watch_byte_limit".into()));
        }
        if !valid_watch_path(&file.path, false)
            || !path_selected(watch, &file.path)
            || file.size > watch.max_file_bytes
            || file.sha256.len() != 64
            || !file
                .sha256
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            || observed.insert(file.path.clone(), file.clone()).is_some()
        {
            return Err(invalid());
        }
    }
    if !state.initialized && !watch.include_existing {
        state.files = observed
            .into_iter()
            .map(|(path, file)| {
                (
                    path,
                    TrackedFile {
                        accepted: Some(file),
                        pending: None,
                    },
                )
            })
            .collect();
        state.initialized = true;
        state.last_observed_at = Some(observed_at);
        return Ok(vec![]);
    }
    state.initialized = true;
    state.last_observed_at = Some(observed_at);
    let paths: BTreeSet<_> = state.files.keys().chain(observed.keys()).cloned().collect();
    let mut ready = Vec::new();
    let mut remove = Vec::new();
    for path in paths {
        let normalized = if cfg!(windows) {
            path.to_lowercase()
        } else {
            path.clone()
        };
        let mut prefix = normalized.as_str();
        let blocked = loop {
            if unavailable.contains(prefix) {
                break true;
            }
            match prefix.rfind('/') {
                Some(end) => prefix = &prefix[..end],
                None => break false,
            }
        };
        if blocked {
            if let Some(tracked) = state.files.get_mut(&path) {
                tracked.pending = None;
            }
            continue;
        }
        let target = observed.get(&path).cloned();
        let tracked = state.files.entry(path.clone()).or_insert(TrackedFile {
            accepted: None,
            pending: None,
        });
        if target == tracked.accepted {
            tracked.pending = None;
            if target.is_none() {
                remove.push(path);
            }
            continue;
        }
        let change = if target.is_none() {
            FileWatchChange::Removed
        } else if tracked.accepted.is_none() {
            FileWatchChange::Created
        } else {
            FileWatchChange::Modified
        };
        if !watch.changes.contains(&change) {
            tracked.accepted = target;
            tracked.pending = None;
            if tracked.accepted.is_none() {
                remove.push(path);
            }
            continue;
        }
        if !tracked
            .pending
            .as_ref()
            .is_some_and(|pending| pending.target == target)
        {
            tracked.pending = Some(PendingFileChange {
                event_id: uuid::Uuid::new_v4().to_string(),
                first_seen_at: observed_at,
                change: change.clone(),
                target: target.clone(),
            });
        }
        let pending = tracked.pending.as_ref().ok_or(Error::Conflict)?;
        if ready.len() < 64
            && observed_at.saturating_sub(pending.first_seen_at) >= i64::from(watch.debounce_ms)
        {
            ready.push(ReadyFileChange {
                path,
                event_id: pending.event_id.clone(),
                change: pending.change.clone(),
                before: tracked.accepted.clone(),
                after: pending.target.clone(),
            });
        }
    }
    for path in remove {
        state.files.remove(&path);
    }
    Ok(ready)
}

pub fn acknowledge_file(state: &mut FileWatchState, change: &ReadyFileChange) -> Result<()> {
    let tracked = state.files.get_mut(&change.path).ok_or(Error::Conflict)?;
    if tracked
        .pending
        .as_ref()
        .map(|pending| pending.event_id.as_str())
        != Some(change.event_id.as_str())
    {
        return Err(Error::Conflict);
    }
    tracked.accepted = change.after.clone();
    tracked.pending = None;
    if tracked.accepted.is_none() {
        state.files.remove(&change.path);
    }
    Ok(())
}
