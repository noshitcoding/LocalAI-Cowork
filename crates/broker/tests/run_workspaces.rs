use cowork_broker::{FileBroker, FolderConfig, RetainedContent, RunWorkspaceIntent};
use cowork_contracts::*;
use cowork_domain::{Error, execution_inputs::CapturedExecutionInput, hash};
use serde_json::json;

struct Fixture {
    broker: FileBroker,
    configs: Vec<FolderConfig>,
    source: RunSpec,
    capture: CapturedExecutionInput,
    directory: tempfile::TempDir,
}
impl Fixture {
    fn new(selected: &[&str]) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let mut configs = Vec::new();
        for id in ["a", "b"] {
            let path = directory.path().join(id);
            std::fs::create_dir(&path).unwrap();
            std::fs::write(path.join("note.txt"), format!("original-{id}")).unwrap();
            configs.push(FolderConfig {
                id: id.into(),
                project_id: "p".into(),
                name: id.into(),
                path,
                writable: true,
            });
        }
        let broker = FileBroker::open(&directory.path().join("state"), configs.clone()).unwrap();
        let order = Order {
            id: uuid::Uuid::new_v4().to_string(),
            project_id: "p".into(),
            title: "Copies".into(),
            draft: String::new(),
            revision: 1,
            created_at: 1,
        };
        let workspace = broker.ensure_workspace(&order, None, None).unwrap();
        std::fs::write(
            directory
                .path()
                .join("state/workspaces")
                .join(&order.id)
                .join("files/live.txt"),
            "not selected",
        )
        .unwrap();
        let prepared = broker
            .prepare_execution_input(
                &order,
                &PrepareExecutionInput {
                    idempotency_key: "freeze".into(),
                    base_revision: 1,
                    folder_ids: selected.iter().map(|s| (*s).to_string()).collect(),
                },
                &uuid::Uuid::new_v4().to_string(),
                1,
            )
            .unwrap();
        let capture = prepared.capture.clone();
        drop(prepared);
        let mut folders = capture.origins.clone();
        folders.push(workspace.folder);
        let source = serde_json::from_value(json!({ "order_id":order.id, "project_id":"p", "prompt":"Copy only",
            "provider":{"id":"fixture","name":"Fixture","kind":"test","endpoint":"","model":"test","revision":1,"context_tokens":8192,"credential_required":false,"credential_ready":true},
            "access_mode":"confirm_mutations", "executor":"local-v2", "policy_revision":1,
            "allowed_tools":["list_files","read_text_file","write_text_file","create_directory","rename_file","remove_file","restore_file"],
            "budgets":{"max_steps":64,"max_seconds":1800,"max_output_tokens":4096}, "folders":folders,
        })).unwrap();
        Self {
            directory,
            broker,
            configs,
            source,
            capture,
        }
    }
    fn intent(&self) -> RunWorkspaceIntent {
        RunWorkspaceIntent::for_spec(
            &self.capture,
            &self.source,
            &uuid::Uuid::new_v4().to_string(),
            &uuid::Uuid::new_v4().to_string(),
        )
        .unwrap()
    }
    fn run(&self, intent: &RunWorkspaceIntent) -> RunRecord {
        let mut spec = self.source.clone();
        spec.input_workspace = Some(intent.binding.clone());
        spec.folders = intent.folders();
        serde_json::from_value(json!({"id":uuid::Uuid::new_v4().to_string(),"spec":spec,"state":"running",
            "revision":1,"last_sequence":1,"steps":0,"created_at":1,"updated_at":1,"started_at":1,"error_code":null,"usage":{}})).unwrap()
    }
    fn files(&self, intent: &RunWorkspaceIntent, alias: &str) -> std::path::PathBuf {
        self.directory
            .path()
            .join("state/run-workspaces")
            .join(&intent.binding.id)
            .join("files")
            .join(alias)
    }
}
fn location(folder: &str, path: &str) -> FileLocation {
    FileLocation {
        folder_id: folder.into(),
        path: path.into(),
    }
}
fn write(folder: &str, path: &str, content: &str, expected: Option<&str>) -> ToolCall {
    ToolCall {
        id: uuid::Uuid::new_v4().to_string(),
        name: "write_text_file".into(),
        arguments: json!({"folder_id":folder,"path":path,"content":content,"expected_sha256":expected}),
    }
}

#[test]
fn frozen_inputs_create_independent_versions_and_never_import_the_live_workspace() {
    let f = Fixture::new(&["a"]);
    std::fs::write(
        f.directory.path().join("a/note.txt"),
        "original changed after freeze",
    )
    .unwrap();
    let a = f.intent();
    let b = f.intent();
    let prepared = f.broker.prepare_run_workspace(&a, &f.capture).unwrap();
    drop(prepared);
    drop(f.broker.prepare_run_workspace(&b, &f.capture).unwrap());
    let run_a = f.run(&a);
    let run_b = f.run(&b);
    assert_eq!(
        f.broker
            .read(&run_a, &location("a", "note.txt"))
            .unwrap()
            .content,
        "original-a"
    );
    assert!(
        f.broker
            .list(
                &run_a,
                &location(&format!("workspace:{}", f.source.order_id), ".")
            )
            .unwrap()
            .entries
            .is_empty()
    );
    let mut change = f
        .broker
        .prepare_write(
            &run_a,
            &write("a", "note.txt", "version-a", Some(&hash(b"original-a"))),
        )
        .unwrap();
    change.apply().unwrap();
    drop(change);
    assert_eq!(
        f.broker
            .read(&run_a, &location("a", "note.txt"))
            .unwrap()
            .content,
        "version-a"
    );
    assert_eq!(
        f.broker
            .read(&run_b, &location("a", "note.txt"))
            .unwrap()
            .content,
        "original-a"
    );
    assert_eq!(
        std::fs::read_to_string(f.directory.path().join("a/note.txt")).unwrap(),
        "original changed after freeze"
    );
    let snapshot = f.broker.snapshot_workspace(&run_a).unwrap();
    assert!(snapshot.snapshot.entries.iter().any(
        |e| e.path == "input-0001/note.txt" && e.sha256.as_deref() == Some(&hash(b"version-a"))
    ));
    assert!(
        !snapshot
            .snapshot
            .entries
            .iter()
            .any(|e| e.path.contains("live.txt"))
    );
    assert_ne!(
        run_a.spec.folders[0].resource_id,
        f.capture.origins[0].resource_id
    );
    assert_ne!(
        run_a.spec.folders[0].resource_id,
        run_b.spec.folders[0].resource_id
    );
}

#[test]
fn delegated_scopes_cannot_see_other_aliases_or_escalate_write_rights() {
    let f = Fixture::new(&["a", "b"]);
    let intent = f.intent();
    drop(f.broker.prepare_run_workspace(&intent, &f.capture).unwrap());
    let mut run = f.run(&intent);
    run.spec.folders.retain(|g| g.id == "a");
    run.spec.folders[0].writable = false;
    assert!(f.broker.read(&run, &location("a", "note.txt")).is_ok());
    assert!(f.broker.read(&run, &location("b", "note.txt")).is_err());
    assert!(
        f.broker
            .prepare_write(&run, &write("a", "new.txt", "no", None))
            .is_err()
    );
    let snapshot = f.broker.snapshot_workspace(&run).unwrap();
    assert!(
        snapshot
            .snapshot
            .entries
            .iter()
            .all(|e| e.path == "input-0001" || e.path.starts_with("input-0001/"))
    );
    drop(snapshot);
    run.spec.folders[0] = f.capture.origins[0].clone();
    assert!(f.broker.read(&run, &location("a", "note.txt")).is_err());
    let mut missing = f.run(&intent);
    missing.spec.input_workspace.as_mut().unwrap().id = uuid::Uuid::new_v4().to_string();
    assert!(f.broker.read(&missing, &location("a", "note.txt")).is_err());
    let mut forged = f.run(&intent);
    forged.spec.input_workspace.as_mut().unwrap().input.sha256 = hash(b"other input");
    assert!(f.broker.read(&forged, &location("a", "note.txt")).is_err());
}

#[test]
fn accepted_custody_reopens_without_resetting_edits_and_refuses_replaced_roots() {
    let f = Fixture::new(&["a"]);
    let intent = f.intent();
    let prepared = f.broker.prepare_run_workspace(&intent, &f.capture).unwrap();
    let record = prepared.record.clone();
    drop(prepared);
    let run = f.run(&intent);
    let mut change = f
        .broker
        .prepare_write(
            &run,
            &write("a", "note.txt", "kept", Some(&hash(b"original-a"))),
        )
        .unwrap();
    change.apply().unwrap();
    drop(change);
    assert!(
        f.broker.prepare_run_workspace(&intent, &f.capture).is_err(),
        "unknown publication must not adopt edited bytes"
    );
    let reopened = FileBroker::open(&f.directory.path().join("state"), f.configs.clone()).unwrap();
    reopened
        .restore_run_workspace_record(&record, &run.spec)
        .unwrap();
    assert_eq!(
        reopened
            .read(&run, &location("a", "note.txt"))
            .unwrap()
            .content,
        "kept"
    );
    let path = f.files(&intent, "input-0001");
    // Windows pins deny rename while these capabilities are alive. Release
    // them to test a replacement observed by the next daemon instance.
    drop(reopened);
    drop(f.broker);
    std::fs::rename(&path, path.with_file_name("replaced-native-object")).unwrap();
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("note.txt"), "kept").unwrap();
    let replacement_reader =
        FileBroker::open(&f.directory.path().join("state"), f.configs.clone()).unwrap();
    assert!(
        replacement_reader
            .restore_run_workspace_record(&record, &run.spec)
            .is_err()
    );
}

#[test]
fn initial_publication_is_idempotent_but_changed_intents_or_markers_are_not_adopted() {
    let f = Fixture::new(&["a"]);
    let intent = f.intent();
    let first = f.broker.prepare_run_workspace(&intent, &f.capture).unwrap();
    let record = first.record.clone();
    drop(first);
    let retry = f.broker.prepare_run_workspace(&intent, &f.capture).unwrap();
    assert_eq!(retry.record, record);
    drop(retry);
    let mut other = intent.clone();
    other.nonce = uuid::Uuid::new_v4().to_string();
    assert!(f.broker.prepare_run_workspace(&other, &f.capture).is_err());
    let marker = f
        .directory
        .path()
        .join("state/run-workspaces")
        .join(&intent.binding.id)
        .join("workspace.json");
    std::fs::write(marker, "{}").unwrap();
    assert!(
        f.broker
            .restore_run_workspace_record(&record, &f.run(&intent).spec)
            .is_err()
    );
}

#[test]
fn revocation_fences_admission_and_source_locks_last_until_custody_is_recorded() {
    let f = Fixture::new(&["a"]);
    let intent = f.intent();
    let origin = &f.capture.origins[0];
    let prepared = f.broker.prepare_run_workspace(&intent, &f.capture).unwrap();
    assert!(matches!(
        f.broker.revoke_native_folder("p", "a", &origin.revision),
        Err(Error::WorkspaceBusy)
    ));
    drop(prepared);
    f.broker
        .revoke_native_folder("p", "a", &origin.revision)
        .unwrap();
    assert!(
        f.broker
            .prepare_run_workspace(&f.intent(), &f.capture)
            .is_err()
    );
    // Revoking a source prevents new copies. Accepted private bytes have their
    // own task scope and remain readable without reopening the original root.
    assert!(
        f.broker
            .read(&f.run(&intent), &location("a", "note.txt"))
            .is_ok()
    );
}

#[test]
fn empty_selection_stays_empty_and_legacy_none_does_not_change_serialized_bytes() {
    let f = Fixture::new(&[]);
    let intent = f.intent();
    assert_eq!(intent.copies.len(), 1);
    assert_eq!(intent.copies[0].alias, "output");
    drop(f.broker.prepare_run_workspace(&intent, &f.capture).unwrap());
    assert!(
        std::fs::read_dir(f.files(&intent, "output"))
            .unwrap()
            .next()
            .is_none()
    );
    let value = serde_json::to_value(&f.source).unwrap();
    assert!(value.get("input_workspace").is_none());
    let mut explicit = value.clone();
    explicit["input_workspace"] = serde_json::Value::Null;
    let parsed: RunSpec = serde_json::from_value(explicit).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap(), value);
}

#[test]
fn controller_witness_is_stable_but_not_reused_after_state_is_relocated() {
    let f = Fixture::new(&[]);
    let original = f.broker.local_controller_origin().unwrap();
    assert_eq!(original, f.broker.local_controller_origin().unwrap());
    let reopened = FileBroker::open(&f.directory.path().join("state"), f.configs.clone()).unwrap();
    assert_eq!(original, reopened.local_controller_origin().unwrap());
    let previous = f.directory.path().join("state");
    let moved = f.directory.path().join("state-relocated");
    #[cfg(windows)]
    assert!(
        std::fs::rename(&previous, &moved).is_err(),
        "live capabilities must prevent directory relocation"
    );
    drop(reopened);
    #[cfg(windows)]
    drop(f.broker);
    std::fs::rename(&previous, &moved).unwrap();
    #[cfg(not(windows))]
    assert!(f.broker.local_controller_origin().is_err());
    let rebound = FileBroker::open(&moved, f.configs.clone()).unwrap();
    assert_ne!(original, rebound.local_controller_origin().unwrap());
}

#[test]
fn namespace_capacity_is_shared_and_uncertain_temporary_bytes_remain_charged() {
    let f = Fixture::new(&[]);
    let intent = f.intent();
    drop(f.broker.prepare_run_workspace(&intent, &f.capture).unwrap());
    let run = f.run(&intent);
    let folder = run.spec.folders[0].id.clone();
    let path = f.files(&intent, "output");
    for index in 0..8 {
        std::fs::File::create(path.join(format!("file-{index}.bin")))
            .unwrap()
            .set_len(64 * 1024 * 1024)
            .unwrap();
    }
    assert!(
        f.broker
            .prepare_write(&run, &write(&folder, "extra.txt", "over total limit", None))
            .is_err()
    );
    // A physically retained tombstone is not reclaimed by an absent user path.
    std::fs::rename(
        path.join("file-7.bin"),
        path.join(".cowork-deleted-uncertain"),
    )
    .unwrap();
    let mut okay = f
        .broker
        .prepare_write(
            &run,
            &write(&folder, "within.txt", "within visible limit", None),
        )
        .unwrap();
    okay.apply().unwrap();
    drop(okay);
    std::fs::File::create(path.join(".cowork-orphaned-stage"))
        .unwrap()
        .set_len(64 * 1024 * 1024)
        .unwrap();
    assert!(
        f.broker
            .prepare_write(
                &run,
                &write(&folder, "blocked.txt", "no hidden capacity release", None)
            )
            .is_err()
    );
    assert!(!path.join("blocked.txt").exists());
}

#[test]
fn portable_alias_collisions_and_oversized_result_manifests_fail_before_staging() {
    let f = Fixture::new(&["a"]);
    let intent = f.intent();
    drop(f.broker.prepare_run_workspace(&intent, &f.capture).unwrap());
    let run = f.run(&intent);
    assert!(
        f.broker
            .prepare_write(
                &run,
                &write("a", "NOTE.txt", "ambiguous on another platform", None)
            )
            .is_err()
    );
    assert!(!f.files(&intent, "input-0001").join("NOTE.txt").exists() || cfg!(windows));
    let entries: Vec<_> = (0..7000)
        .map(|index| WorkspaceEntry {
            path: format!("{index:04}-{}.txt", "x".repeat(230)),
            sha256: Some(hash(b"")),
            size: 0,
        })
        .collect();
    let content = RetainedContent::Directory {
        sha256: cowork_domain::canonical_hash(&entries),
        entries,
    };
    let call = ToolCall {
        id: "large-manifest".into(),
        name: "restore_file".into(),
        arguments: json!({"folder_id":"a","path":"restored",
        "source":{"run_id":"fixture","tool_call_id":"fixture","version":"operation_before"}}),
    };
    assert!(f.broker.prepare_restore(&run, &call, &content).is_err());
    let names: Vec<_> = std::fs::read_dir(f.files(&intent, "input-0001"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(
        names,
        vec![std::ffi::OsString::from("note.txt")],
        "rejected metadata must not leave a private stage"
    );
}

#[test]
fn an_unselected_role_cannot_write_around_the_shared_copy_capacity() {
    let f = Fixture::new(&["a", "b"]);
    let intent = f.intent();
    drop(f.broker.prepare_run_workspace(&intent, &f.capture).unwrap());
    for (alias, count) in [("input-0001", 4), ("input-0002", 4)] {
        for index in 0..count {
            std::fs::File::create(f.files(&intent, alias).join(format!("large-{index}")))
                .unwrap()
                .set_len(64 * 1024 * 1024 - 2)
                .unwrap();
        }
    }
    let mut role = f.run(&intent);
    role.spec.folders.retain(|g| g.id == "a");
    assert!(
        f.broker
            .prepare_write(&role, &write("a", "extra.txt", &"x".repeat(128), None))
            .is_err()
    );
    assert!(!f.files(&intent, "input-0001").join("extra.txt").exists());
}
