use cowork_contracts::*;
use cowork_domain::{canonical_hash, file_watch::*};

fn config() -> AutomationFileWatch {
    AutomationFileWatch {
        folder: FolderGrant {
            id: "folder".into(),
            project_id: "p".into(),
            name: "Inbox".into(),
            writable: false,
            revision: "r1".into(),
            resource_id: "resource".into(),
            order_id: None,
        },
        path: ".".into(),
        recursive: true,
        changes: vec![
            FileWatchChange::Created,
            FileWatchChange::Modified,
            FileWatchChange::Removed,
        ],
        extensions: vec![],
        exclude_paths: vec![],
        include_existing: false,
        debounce_ms: 1000,
        poll_interval_ms: 5000,
        max_files: 1000,
        max_file_bytes: 1024,
        max_scan_bytes: 4096,
    }
}
fn file(path: &str, content: &str) -> FileWatchObservation {
    FileWatchObservation {
        path: path.into(),
        sha256: canonical_hash(&content),
        size: content.len() as u64,
    }
}

#[test]
fn existing_files_are_only_processed_when_explicitly_selected() {
    let mut watch = config();
    let files = vec![file("invoice.pdf", "original")];
    let mut state = FileWatchState::default();
    assert!(
        observe_files(&mut state, &watch, &files, 0)
            .unwrap()
            .is_empty()
    );
    assert!(
        observe_files(&mut state, &watch, &files, 10_000)
            .unwrap()
            .is_empty()
    );
    watch.include_existing = true;
    let mut state = FileWatchState::default();
    assert!(
        observe_files(&mut state, &watch, &files, 0)
            .unwrap()
            .is_empty()
    );
    let ready = observe_files(&mut state, &watch, &files, 1000).unwrap();
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].change, FileWatchChange::Created);
    acknowledge_file(&mut state, &ready[0]).unwrap();
    assert!(
        observe_files(&mut state, &watch, &files, 2000)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn debounce_resets_on_partial_writes_and_survives_restart_with_the_same_identifier() {
    let watch = config();
    let mut state = FileWatchState::default();
    observe_files(&mut state, &watch, &[], 0).unwrap();
    observe_files(&mut state, &watch, &[file("invoice.pdf", "half")], 100).unwrap();
    let old = state.files["invoice.pdf"]
        .pending
        .as_ref()
        .unwrap()
        .event_id
        .clone();
    observe_files(&mut state, &watch, &[file("invoice.pdf", "complete")], 800).unwrap();
    let current = state.files["invoice.pdf"]
        .pending
        .as_ref()
        .unwrap()
        .event_id
        .clone();
    assert_ne!(old, current);
    let mut restored: FileWatchState =
        serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
    assert!(
        observe_files(
            &mut restored,
            &watch,
            &[file("invoice.pdf", "complete")],
            1500
        )
        .unwrap()
        .is_empty()
    );
    let ready = observe_files(
        &mut restored,
        &watch,
        &[file("invoice.pdf", "complete")],
        1800,
    )
    .unwrap();
    assert_eq!(ready[0].event_id, current);
    assert_eq!(
        ready[0].after.as_ref().unwrap().sha256,
        file("invoice.pdf", "complete").sha256
    );
    acknowledge_file(&mut restored, &ready[0]).unwrap();
    assert!(
        observe_files(
            &mut restored,
            &watch,
            &[file("invoice.pdf", "complete")],
            4000
        )
        .unwrap()
        .is_empty()
    );
}

#[test]
fn temporary_disappearance_coalesces_and_confirmed_removal_allows_a_new_creation() {
    let watch = config();
    let mut state = FileWatchState::default();
    let files = [file("invoice.pdf", "original")];
    observe_files(&mut state, &watch, &files, 0).unwrap();
    observe_files(&mut state, &watch, &[], 100).unwrap();
    assert!(
        observe_files(&mut state, &watch, &files, 1200)
            .unwrap()
            .is_empty()
    );
    observe_files(&mut state, &watch, &[], 1300).unwrap();
    let ready = observe_files(&mut state, &watch, &[], 2300).unwrap();
    assert_eq!(ready[0].change, FileWatchChange::Removed);
    acknowledge_file(&mut state, &ready[0]).unwrap();
    assert!(state.files.is_empty());
    observe_files(&mut state, &watch, &files, 2400).unwrap();
    let created = observe_files(&mut state, &watch, &files, 3400).unwrap();
    assert_eq!(created[0].change, FileWatchChange::Created);
    assert_ne!(created[0].event_id, ready[0].event_id);
}

#[test]
fn bounded_batches_retain_every_remaining_candidate() {
    let mut watch = config();
    watch.include_existing = true;
    let mut state = FileWatchState::default();
    let files: Vec<_> = (0..150)
        .map(|n| file(&format!("{n:03}.pdf"), "content"))
        .collect();
    observe_files(&mut state, &watch, &files, 0).unwrap();
    let mut ids = std::collections::BTreeSet::new();
    for time in [1000, 2000, 3000] {
        let ready = observe_files(&mut state, &watch, &files, time).unwrap();
        assert!(ready.len() <= 64);
        for item in ready {
            assert!(ids.insert(item.event_id.clone()));
            acknowledge_file(&mut state, &item).unwrap();
        }
    }
    assert_eq!(ids.len(), 150);
    assert!(state.files.values().all(|entry| entry.pending.is_none()));
}

#[test]
fn scope_filters_and_invalid_observations_are_enforced() {
    let mut watch = config();
    watch.path = "incoming".into();
    watch.extensions = vec![".pdf".into()];
    watch.exclude_paths = vec!["incoming/processed".into()];
    assert!(path_selected(&watch, "incoming/new.PDF"));
    assert!(!path_selected(&watch, "incoming/new.txt"));
    assert!(!path_selected(&watch, "elsewhere/new.pdf"));
    assert!(!path_selected(&watch, "incoming/processed/new.pdf"));
    watch.recursive = false;
    assert!(!path_selected(&watch, "incoming/sub/new.pdf"));
    assert!(
        observe_files(
            &mut FileWatchState::default(),
            &watch,
            &[file("outside.pdf", "x")],
            0
        )
        .is_err()
    );
    for path in [
        "..",
        "../secret",
        "/absolute",
        "x\\y",
        "a//b",
        ".cowork-private",
    ] {
        watch.path = path.into();
        assert!(validate_file_watch(&AutomationEventSource::File, Some(&watch)).is_err());
    }
    assert!(validate_file_watch(&AutomationEventSource::File, None).is_err());
}

#[test]
fn rollback_and_stale_acknowledgements_cannot_advance_observations() {
    let mut watch = config();
    watch.include_existing = true;
    let mut state = FileWatchState::default();
    observe_files(&mut state, &watch, &[file("a.pdf", "v1")], 1000).unwrap();
    let ready = observe_files(&mut state, &watch, &[file("a.pdf", "v1")], 2000).unwrap();
    observe_files(&mut state, &watch, &[file("a.pdf", "v2")], 3000).unwrap();
    assert!(acknowledge_file(&mut state, &ready[0]).is_err());
    assert!(observe_files(&mut state, &watch, &[], 2500).is_err());
    assert_eq!(state.last_observed_at, Some(3000));
}

#[test]
fn unobservable_entries_preserve_their_previous_state_and_restart_debounce() {
    let watch = config();
    let mut state = FileWatchState::default();
    let original = [file("linked/a.pdf", "original"), file("regular.pdf", "v1")];
    observe_files(&mut state, &watch, &original, 0).unwrap();
    // A removal candidate must not survive learning that its directory still
    // exists but has become a link that the broker deliberately does not follow.
    observe_files(&mut state, &watch, &[], 100).unwrap();
    let available = [file("regular.pdf", "v2")];
    let unavailable = ["linked".to_owned()];
    assert!(
        observe_files_with_unavailable(&mut state, &watch, &available, &unavailable, 500)
            .unwrap()
            .is_empty()
    );
    let ready =
        observe_files_with_unavailable(&mut state, &watch, &available, &unavailable, 1500).unwrap();
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].path, "regular.pdf");
    acknowledge_file(&mut state, &ready[0]).unwrap();
    assert_eq!(
        state.files["linked/a.pdf"].accepted,
        Some(original[0].clone())
    );
    assert!(state.files["linked/a.pdf"].pending.is_none());
    // Once a complete scan can prove absence, a fresh full debounce is required.
    assert!(
        observe_files(&mut state, &watch, &available, 1600)
            .unwrap()
            .is_empty()
    );
    let removed = observe_files(&mut state, &watch, &available, 2600).unwrap();
    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0].change, FileWatchChange::Removed);
    assert_eq!(removed[0].before.as_ref(), Some(&original[0]));
}
