use super::*;
use crate::{FolderConfig, NativeFolderSelection, inspect_native_folder};

struct BaseOverride(Option<Option<PathBuf>>);
impl BaseOverride {
    fn set(value: Option<PathBuf>) -> Self {
        Self(TEST_BASE.with(|base| base.replace(Some(value))))
    }
}
impl Drop for BaseOverride {
    fn drop(&mut self) {
        TEST_BASE.with(|base| {
            base.replace(self.0.take());
        });
    }
}
fn config(path: &Path) -> FolderConfig {
    FolderConfig {
        id: "chosen".into(),
        project_id: "p".into(),
        name: "Chosen".into(),
        path: path.into(),
        writable: true,
    }
}
fn selection(path: &Path, key: &str) -> NativeFolderSelection {
    NativeFolderSelection {
        idempotency_key: key.into(),
        project_id: "p".into(),
        name: "Chosen".into(),
        path: path.into(),
        expected_resource_id: inspect_native_folder(path).unwrap().1,
        writable: true,
    }
}
fn marker(path: &Path) {
    fs::create_dir_all(path).unwrap();
    fs::write(path.join("original.txt"), b"original").unwrap();
}
fn unchanged(path: &Path) {
    assert_eq!(fs::read(path.join("original.txt")).unwrap(), b"original");
}

#[test]
fn missing_reservation_is_not_created_and_both_constructors_reject_ancestor_grants() {
    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().join("native-local");
    marker(&parent);
    let base = parent.join("LocalAI-Cowork-Documents/work");
    let _override = BaseOverride::set(Some(base.clone()));
    let broker = FileBroker::open(&temp.path().join("state"), vec![]).unwrap();
    assert!(!base.exists());
    assert!(!base.parent().unwrap().exists());
    assert!(FileBroker::open(&temp.path().join("configured"), vec![config(&parent)]).is_err());
    assert!(
        FileBroker::open_with_protected(&temp.path().join("protected"), vec![config(&parent)], &[])
            .is_err()
    );
    assert!(
        broker
            .register_native_folder(&selection(&parent, "ancestor"))
            .is_err()
    );
    assert!(!base.parent().unwrap().exists());
    unchanged(&parent);
}

#[test]
fn existing_reserved_equal_child_and_parent_are_rejected_for_config_and_native_registration() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp
        .path()
        .join("native-local/LocalAI-Cowork-Documents/work");
    let child = base.join("profile/stages");
    marker(&child);
    marker(&base);
    let _override = BaseOverride::set(Some(base.clone()));
    let broker = FileBroker::open(&temp.path().join("state"), vec![]).unwrap();
    for (i, path) in [
        base.clone(),
        child.clone(),
        base.parent().unwrap().to_owned(),
    ]
    .iter()
    .enumerate()
    {
        assert!(
            FileBroker::open(&temp.path().join(format!("state-{i}")), vec![config(path)]).is_err()
        );
        assert!(
            broker
                .register_native_folder(&selection(path, &format!("native-{i}")))
                .is_err()
        );
    }
    assert!(broker.grants("p").unwrap().is_empty());
    unchanged(&base);
    unchanged(&child);
}

#[test]
fn independent_config_and_registration_remain_usable_when_the_reserved_base_appears() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp
        .path()
        .join("native-local/LocalAI-Cowork-Documents/work");
    let chosen = temp.path().join("chosen");
    let native = temp.path().join("native");
    marker(&chosen);
    marker(&native);
    let _override = BaseOverride::set(Some(base.clone()));
    let broker = FileBroker::open(&temp.path().join("state"), vec![config(&chosen)]).unwrap();
    let request = selection(&native, "independent");
    let original = broker.register_native_folder(&request).unwrap();
    assert!(!base.exists());
    marker(&base);
    let replay = broker.register_native_folder(&request).unwrap();
    assert_eq!(replay.grant, original.grant);
    assert_eq!(replay.canonical_path, original.canonical_path);
    assert_eq!(replay.revoked, original.revoked);
    assert_eq!(broker.grants("p").unwrap().len(), 2);
    let reopened = FileBroker::open(&temp.path().join("state"), vec![config(&chosen)]).unwrap();
    assert_eq!(reopened.grants("p").unwrap().len(), 2);
    unchanged(&chosen);
    unchanged(&native);
    unchanged(&base);
}

#[test]
fn stored_registration_is_denied_when_native_policy_newly_reserves_its_ancestor() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("native-local/doc-work");
    marker(&base.join("chosen"));
    let state = temp.path().join("state");
    {
        let _unreserved = BaseOverride::set(None);
        let broker = FileBroker::open(&state, vec![]).unwrap();
        broker
            .register_native_folder(&selection(&base.join("chosen"), "original"))
            .unwrap();
    }
    let registry = fs::read(state.join("folder-registry.json")).unwrap();
    let _reserved = BaseOverride::set(Some(base.clone()));
    assert!(FileBroker::open(&state, vec![]).is_err());
    assert!(FileBroker::open_with_protected(&state, vec![], &[]).is_err());
    assert_eq!(
        fs::read(state.join("folder-registry.json")).unwrap(),
        registry
    );
    unchanged(&base.join("chosen"));
}

#[test]
fn broker_state_cannot_create_or_occupy_the_reserved_namespace() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("reserved/doc-work");
    let _override = BaseOverride::set(Some(base.clone()));
    for path in [&base, &base.join("state")] {
        assert!(FileBroker::open(path, vec![]).is_err());
        assert!(!base.parent().unwrap().exists());
    }
}

#[test]
fn non_directory_reservation_and_nonabsolute_override_fail_closed_without_state_creation() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("not-a-directory");
    let base = file.join("doc-work");
    fs::write(&file, b"original").unwrap();
    for bad in [
        base.clone(),
        PathBuf::from("relative/doc-work"),
        temp.path().join("x/../doc-work"),
    ] {
        let _override = BaseOverride::set(Some(bad));
        assert!(FileBroker::open(&temp.path().join("state"), vec![]).is_err());
        assert!(!temp.path().join("state").exists());
    }
    assert_eq!(fs::read(&file).unwrap(), b"original");
}

#[cfg(windows)]
fn link_dir(source: &Path, link: &Path) {
    use std::os::windows::process::CommandExt;
    // cmd/mklink parses forward slashes as switches; use native component spelling.
    let source: PathBuf = source.components().collect();
    let link: PathBuf = link.components().collect();
    let output = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(source)
        .creation_flags(0x08000000)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "Owned junction creation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
#[cfg(unix)]
fn link_dir(source: &Path, link: &Path) {
    std::os::unix::fs::symlink(source, link).unwrap();
}

#[test]
fn aliased_reserved_ancestors_fail_closed_without_mutating_the_target() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target");
    marker(&target);
    let alias = temp.path().join("alias");
    link_dir(&target, &alias);
    let base = alias.join("doc-work");
    {
        let _override = BaseOverride::set(Some(base));
        assert!(FileBroker::open(&temp.path().join("state"), vec![]).is_err());
        assert!(!target.join("doc-work").exists());
        assert!(!temp.path().join("state").exists());
    }
    fs::remove_dir(&alias)
        .or_else(|_| fs::remove_file(&alias))
        .unwrap();
    unchanged(&target);
}

#[test]
fn grant_aliases_into_the_reserved_tree_do_not_bypass_native_checks() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("reserved/doc-work");
    marker(&base.join("child"));
    let alias = temp.path().join("alias");
    link_dir(&base, &alias);
    {
        let _override = BaseOverride::set(Some(base.clone()));
        let broker = FileBroker::open(&temp.path().join("state"), vec![]).unwrap();
        for path in [alias.clone(), alias.join("child")] {
            assert!(
                FileBroker::open(&temp.path().join("other-state"), vec![config(&path)]).is_err()
            );
            assert!(
                broker
                    .register_native_folder(&selection(&path, "aliased"))
                    .is_err()
            );
        }
    }
    fs::remove_dir(&alias)
        .or_else(|_| fs::remove_file(&alias))
        .unwrap();
    unchanged(&base.join("child"));
}

#[test]
fn live_broker_rechecks_reservation_before_cached_grants_and_idempotent_registration() {
    let temp = tempfile::tempdir().unwrap();
    let chosen = temp.path().join("chosen");
    marker(&chosen);
    let base = temp.path().join("reserved/doc-work");
    fs::create_dir(base.parent().unwrap()).unwrap();
    let _override = BaseOverride::set(Some(base.clone()));
    let broker = FileBroker::open(&temp.path().join("state"), vec![config(&chosen)]).unwrap();
    let native = temp.path().join("native");
    marker(&native);
    let request = selection(&native, "original");
    broker.register_native_folder(&request).unwrap();
    link_dir(&chosen, &base);
    assert!(broker.grants("p").is_err());
    assert!(broker.registered_folders("p").is_err());
    assert!(broker.register_native_folder(&request).is_err());
    fs::remove_dir(&base)
        .or_else(|_| fs::remove_file(&base))
        .unwrap();
    unchanged(&chosen);
    unchanged(&native);
}

#[cfg(windows)]
#[test]
fn native_known_folder_query_is_fixed_absolute_and_does_not_create_the_workspace() {
    let first = native_document_workspace_base().unwrap().unwrap();
    assert!(first.is_absolute());
    assert!(first.ends_with(".localai-cowork-documents/work"));
    let legacy = native_legacy_document_workspace_base().unwrap();
    assert!(legacy.is_absolute());
    assert!(legacy.ends_with("LocalAI-Cowork-Documents/work"));
    let before = fs::symlink_metadata(&first)
        .ok()
        .map(|m| (m.is_dir(), m.len()));
    assert_eq!(native_document_workspace_base().unwrap().unwrap(), first);
    assert_eq!(
        fs::symlink_metadata(&first)
            .ok()
            .map(|m| (m.is_dir(), m.len())),
        before
    );
}

#[cfg(not(windows))]
#[test]
fn native_non_windows_has_no_automatic_reservation() {
    assert!(native_document_workspace_base().unwrap().is_none());
}

#[test]
fn private_parent_and_unpublished_siblings_cannot_be_granted() {
    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().join("native-local/LocalAI-Cowork-Documents");
    let sibling = parent.join(".doc-work-unpublished-original");
    marker(&parent);
    marker(&sibling);
    let base = parent.join("doc-work");
    let _override = BaseOverride::set(Some(base.clone()));
    let broker = FileBroker::open(&temp.path().join("state"), vec![]).unwrap();
    for (index, path) in [&parent, &sibling].iter().enumerate() {
        assert!(
            FileBroker::open(
                &temp.path().join(format!("configured-{index}")),
                vec![config(path)]
            )
            .is_err()
        );
        assert!(
            broker
                .register_native_folder(&selection(path, &format!("sibling-{index}")))
                .is_err()
        );
    }
    assert!(!base.exists());
    unchanged(&parent);
    unchanged(&sibling);
    assert_eq!(fs::read_dir(&parent).unwrap().count(), 2);
}

#[test]
fn direct_initialization_siblings_are_reserved_before_and_after_creation() {
    let temp = tempfile::tempdir().unwrap();
    let local = temp.path().join("native-local");
    marker(&local);
    let base = local.join("LocalAI-Cowork-Documents/work");
    let initializing = local.join(format!(
        "{DOCUMENT_WORKSPACE_INIT_PREFIX}{}",
        uuid::Uuid::new_v4()
    ));
    let _override = BaseOverride::set(Some(base.clone()));
    assert!(FileBroker::open(&initializing.join("state"), vec![]).is_err());
    assert!(!initializing.exists());
    assert!(!base.parent().unwrap().exists());
    marker(&initializing);
    marker(&initializing.join("child"));
    let broker = FileBroker::open(&temp.path().join("state"), vec![]).unwrap();
    for (index, path) in [&initializing, &initializing.join("child"), &local]
        .iter()
        .enumerate()
    {
        assert!(
            FileBroker::open(
                &temp.path().join(format!("unsafe-{index}")),
                vec![config(path)]
            )
            .is_err()
        );
        assert!(
            broker
                .register_native_folder(&selection(path, &format!("initializing-{index}")))
                .is_err()
        );
    }
    // The prefix is direct-child policy, not a new arbitrary-name content filter.
    let unrelated = temp
        .path()
        .join("project")
        .join(format!("{DOCUMENT_WORKSPACE_INIT_PREFIX}ordinary"));
    marker(&unrelated);
    assert!(
        broker
            .register_native_folder(&selection(&unrelated, "independent"))
            .is_ok()
    );
    unchanged(&local);
    unchanged(&initializing);
    unchanged(&initializing.join("child"));
    unchanged(&unrelated);
    assert!(!base.parent().unwrap().exists());
}

#[test]
fn stored_initialization_grant_is_denied_without_rewriting_the_registry() {
    let temp = tempfile::tempdir().unwrap();
    let local = temp.path().join("native-local");
    let initializing = local.join(format!(
        "{DOCUMENT_WORKSPACE_INIT_PREFIX}{}",
        uuid::Uuid::new_v4()
    ));
    marker(&initializing);
    let state = temp.path().join("state");
    {
        let _unreserved = BaseOverride::set(None);
        let broker = FileBroker::open(&state, vec![]).unwrap();
        broker
            .register_native_folder(&selection(&initializing, "before-policy"))
            .unwrap();
    }
    let bytes = fs::read(state.join("folder-registry.json")).unwrap();
    let _override = BaseOverride::set(Some(local.join("LocalAI-Cowork-Documents/work")));
    assert!(FileBroker::open(&state, vec![]).is_err());
    assert_eq!(fs::read(state.join("folder-registry.json")).unwrap(), bytes);
    unchanged(&initializing);
}

#[test]
fn outward_links_inside_private_namespaces_cannot_become_configured_or_registered_grants() {
    let temp = tempfile::tempdir().unwrap();
    let local = temp.path().join("native-local");
    let parent = local.join("LocalAI-Cowork-Documents");
    marker(&parent);
    let outside = temp.path().join("independent");
    marker(&outside);
    let alias = parent.join("outward");
    link_dir(&outside, &alias);
    {
        let _override = BaseOverride::set(Some(parent.join("work")));
        let broker = FileBroker::open(&temp.path().join("state"), vec![]).unwrap();
        assert!(FileBroker::open(&temp.path().join("other-state"), vec![config(&alias)]).is_err());
        assert!(
            broker
                .register_native_folder(&selection(&alias, "outward"))
                .is_err()
        );
        assert!(broker.grants("p").unwrap().is_empty());
    }
    fs::remove_dir(&alias)
        .or_else(|_| fs::remove_file(&alias))
        .unwrap();
    unchanged(&parent);
    unchanged(&outside);
}

#[test]
fn stored_outward_link_selection_cannot_hide_its_private_original_namespace() {
    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().join("native-local/LocalAI-Cowork-Documents");
    marker(&parent);
    let outside = temp.path().join("independent");
    marker(&outside);
    let alias = parent.join("outward");
    link_dir(&outside, &alias);
    let state = temp.path().join("state");
    {
        let _unreserved = BaseOverride::set(None);
        let broker = FileBroker::open(&state, vec![]).unwrap();
        broker
            .register_native_folder(&selection(&alias, "legacy-outward"))
            .unwrap();
    }
    let bytes = fs::read(state.join("folder-registry.json")).unwrap();
    {
        let _reserved = BaseOverride::set(Some(parent.join("work")));
        assert!(FileBroker::open(&state, vec![]).is_err());
        assert_eq!(fs::read(state.join("folder-registry.json")).unwrap(), bytes);
    }
    fs::remove_dir(&alias)
        .or_else(|_| fs::remove_file(&alias))
        .unwrap();
    unchanged(&parent);
    unchanged(&outside);
}

struct ConfiguredReconciliationFixture {
    broker: FileBroker,
    run: cowork_contracts::RunRecord,
    change: cowork_contracts::FileChange,
    operation: cowork_contracts::FileOperation,
    base: PathBuf,
    chosen: PathBuf,
    _policy: BaseOverride,
    _additional_policy: AdditionalBases,
    _temp: tempfile::TempDir,
}
impl ConfiguredReconciliationFixture {
    fn new() -> Self {
        Self::with_legacy_policy(false)
    }
    fn with_legacy_policy(dual: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let chosen = temp.path().join("chosen");
        marker(&chosen);
        let base = temp.path().join(if dual {
            "native-profile/.localai-cowork-documents/work"
        } else {
            "native-local/LocalAI-Cowork-Documents/work"
        });
        fs::create_dir_all(base.parent().unwrap()).unwrap();
        let policy = BaseOverride::set(Some(base.clone()));
        let legacy = temp
            .path()
            .join("native-legacy/LocalAI-Cowork-Documents/work");
        let additional = AdditionalBases::set(if dual { vec![legacy.clone()] } else { vec![] });
        if dual {
            fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        }
        let broker = FileBroker::open(&temp.path().join("state"), vec![config(&chosen)]).unwrap();
        let grant = broker.grants("p").unwrap().pop().unwrap();
        let run: cowork_contracts::RunRecord = serde_json::from_value(serde_json::json!({
            "id":"run", "spec":{"order_id":"order", "project_id":"p", "prompt":"test", "provider":null,
            "access_mode":"confirm_mutations", "executor":"local-v2", "policy_revision":1,
            "allowed_tools":["read_text_file", "write_text_file", "remove_file"],
            "budgets":{"max_steps":64, "max_seconds":1800, "max_output_tokens":4096}, "folders":[grant]},
            "state":"interrupted", "revision":1, "last_sequence":1, "steps":0,
            "created_at":0, "updated_at":0, "started_at":null, "error_code":null, "usage":{}})).unwrap();
        let write = cowork_contracts::ToolCall {
            id: "original-write".into(),
            name: "write_text_file".into(),
            arguments: serde_json::json!({"folder_id":"chosen", "path":"original.txt", "content":"changed",
                "expected_sha256":cowork_domain::hash(b"original")}),
        };
        let prepared = broker.prepare_write(&run, &write).unwrap();
        let change = prepared.change.clone();
        drop(prepared);
        let remove = cowork_contracts::ToolCall {
            id: "original-remove".into(),
            name: "remove_file".into(),
            arguments: serde_json::json!({"folder_id":"chosen", "path":"original.txt",
                "expected_sha256":cowork_domain::hash(b"original")}),
        };
        let prepared = broker.prepare_operation(&run, &remove).unwrap();
        let operation = prepared.operation.clone();
        drop(prepared);
        let grant = &run.spec.folders[0];
        broker
            .revoke_native_folder("p", &grant.id, &grant.revision)
            .unwrap();
        Self {
            broker,
            run,
            change,
            operation,
            base,
            chosen,
            _policy: policy,
            _additional_policy: additional,
            _temp: temp,
        }
    }
}

#[test]
fn configured_revoked_reconciliation_preserves_original_observation_without_regranting_content() {
    let fixture = ConfiguredReconciliationFixture::new();
    assert!(fixture.broker.grants("p").unwrap().is_empty());
    let location = cowork_contracts::FileLocation {
        folder_id: "chosen".into(),
        path: "original.txt".into(),
    };
    assert!(matches!(
        fixture.broker.read(&fixture.run, &location),
        Err(Error::Forbidden)
    ));
    let (observed, guard) = fixture
        .broker
        .observe_change(&fixture.run, &fixture.change)
        .unwrap();
    assert_eq!(observed, Some(cowork_domain::hash(b"original")));
    drop(guard);
    let guard = fixture
        .broker
        .reconcile_operation(&fixture.run, &fixture.operation, false)
        .unwrap();
    drop(guard);
    unchanged(&fixture.chosen);
    assert!(!fixture.base.exists());
    println!(
        "BROKER_RECONCILIATION_OBSERVATION {{\"case\":\"normal_revoked\",\"observe_original\":true,\"operation_original\":true,\"content_regrant_denied\":true}}"
    );
}

#[test]
fn configured_revoked_reconciliation_denies_a_rebound_native_private_reservation() {
    let fixture = ConfiguredReconciliationFixture::new();
    link_dir(&fixture.chosen, &fixture.base);
    // Both real public reconciliation entry points must reject before reading
    // from the cached configured capability, even after its content grant is revoked.
    let observe = fixture.broker.observe_change(&fixture.run, &fixture.change);
    let (observe_forbidden, observed_original) = match observe {
        Err(Error::Forbidden) => (true, false),
        Ok((observed, guard)) => {
            drop(guard);
            (false, observed == Some(cowork_domain::hash(b"original")))
        }
        Err(error) => panic!("Unexpected observation result: {error}"),
    };
    let operation = fixture
        .broker
        .reconcile_operation(&fixture.run, &fixture.operation, false);
    let operation_forbidden = matches!(operation, Err(Error::Forbidden));
    drop(operation);
    unchanged(&fixture.chosen);
    fs::remove_dir(&fixture.base)
        .or_else(|_| fs::remove_file(&fixture.base))
        .unwrap();
    unchanged(&fixture.chosen);
    println!(
        "BROKER_RECONCILIATION_OBSERVATION {}",
        serde_json::json!({"case":"rebound_private_reservation",
        "observe_forbidden":observe_forbidden,"operation_forbidden":operation_forbidden,
        "observed_original_before_denial":observed_original,"sentinel_unchanged":true})
    );
    assert!(
        observe_forbidden && operation_forbidden,
        "revoked configured reconciliation must still enforce the native private reservation"
    );
}

struct AdditionalBases(Vec<PathBuf>);
impl AdditionalBases {
    fn set(value: Vec<PathBuf>) -> Self {
        Self(TEST_ADDITIONAL_BASES.with(|bases| bases.replace(value)))
    }
}
impl Drop for AdditionalBases {
    fn drop(&mut self) {
        TEST_ADDITIONAL_BASES.with(|bases| bases.replace(std::mem::take(&mut self.0)));
    }
}

#[test]
fn primary_and_legacy_reservations_remain_absent_and_deny_both_namespace_trees() {
    let temp = tempfile::tempdir().unwrap();
    let primary = temp.path().join("profile/.localai-cowork-documents/work");
    let legacy = temp.path().join("local/LocalAI-Cowork-Documents/work");
    for base in [&primary, &legacy] {
        marker(base.parent().unwrap().parent().unwrap());
    }
    let _primary = BaseOverride::set(Some(primary.clone()));
    let _legacy = AdditionalBases::set(vec![legacy.clone()]);
    assert_eq!(document_workspace_base().unwrap(), Some(primary.clone()));
    let state = temp.path().join("state");
    let broker = FileBroker::open(&state, vec![]).unwrap();
    for base in [&primary, &legacy] {
        assert!(!base.parent().unwrap().exists());
        let init = base
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join(format!("{DOCUMENT_WORKSPACE_INIT_PREFIX}owned"));
        assert!(FileBroker::open(&init, vec![]).is_err());
        assert!(!init.exists());
        marker(base);
        marker(&base.join("child"));
        marker(&init);
        marker(&init.join("child"));
        for (index, path) in [
            base.clone(),
            base.join("child"),
            base.parent().unwrap().to_owned(),
            base.parent().unwrap().parent().unwrap().to_owned(),
            init.clone(),
            init.join("child"),
        ]
        .iter()
        .enumerate()
        {
            assert!(
                FileBroker::open(&temp.path().join("denied-state"), vec![config(path)]).is_err()
            );
            assert!(
                broker
                    .register_native_folder(&selection(path, &format!("denied-{index}")))
                    .is_err()
            );
        }
        unchanged(base);
        unchanged(&base.join("child"));
        unchanged(&init);
        unchanged(&init.join("child"));
    }
    let independent = temp.path().join("independent");
    marker(&independent);
    assert!(
        broker
            .register_native_folder(&selection(&independent, "independent"))
            .is_ok()
    );
    assert_eq!(broker.grants("p").unwrap().len(), 1);
    unchanged(&independent);
}

#[test]
fn both_native_reservations_preserve_revoked_original_reconciliation_but_deny_rebinding() {
    let fixture = ConfiguredReconciliationFixture::with_legacy_policy(true);
    let original = cowork_domain::hash(b"original");
    let (observed, guard) = fixture
        .broker
        .observe_change(&fixture.run, &fixture.change)
        .unwrap();
    assert_eq!(observed, Some(original));
    drop(guard);
    drop(
        fixture
            .broker
            .reconcile_operation(&fixture.run, &fixture.operation, false)
            .unwrap(),
    );
    let location = cowork_contracts::FileLocation {
        folder_id: "chosen".into(),
        path: "original.txt".into(),
    };
    assert!(matches!(
        fixture.broker.read(&fixture.run, &location),
        Err(Error::Forbidden)
    ));
    let legacy = fixture
        ._temp
        .path()
        .join("native-legacy/LocalAI-Cowork-Documents/work");
    for (index, base) in [&fixture.base, &legacy].iter().enumerate() {
        link_dir(&fixture.chosen, base);
        let change_denied = matches!(
            fixture.broker.observe_change(&fixture.run, &fixture.change),
            Err(Error::Forbidden)
        );
        let operation_denied = matches!(
            fixture
                .broker
                .reconcile_operation(&fixture.run, &fixture.operation, false),
            Err(Error::Forbidden)
        );
        fs::remove_dir(base)
            .or_else(|_| fs::remove_file(base))
            .unwrap();
        println!(
            "DUAL_NATIVE_RESERVATION_OBSERVATION {{\"root_index\":{index},\"change_forbidden\":{change_denied},\"operation_forbidden\":{operation_denied}}}"
        );
        assert!(change_denied && operation_denied);
        unchanged(&fixture.chosen);
    }
}

#[test]
fn primary_and_legacy_literal_outward_aliases_and_stored_selections_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let primary = temp.path().join("profile/.localai-cowork-documents/work");
    let legacy = temp.path().join("local/LocalAI-Cowork-Documents/work");
    let outside = temp.path().join("outside");
    marker(&outside);
    for (index, base) in [&primary, &legacy].iter().enumerate() {
        let parent = base.parent().unwrap();
        marker(parent);
        let alias = parent.join("outward");
        link_dir(&outside, &alias);
        let state = temp.path().join(format!("stored-{index}"));
        {
            let _none = BaseOverride::set(None);
            let broker = FileBroker::open(&state, vec![]).unwrap();
            broker
                .register_native_folder(&selection(&alias, "original"))
                .unwrap();
        }
        let registry = fs::read(state.join("folder-registry.json")).unwrap();
        {
            let _primary = BaseOverride::set(Some(primary.clone()));
            let _legacy = AdditionalBases::set(vec![legacy.clone()]);
            let broker =
                FileBroker::open(&temp.path().join(format!("clean-{index}")), vec![]).unwrap();
            assert!(
                FileBroker::open(&temp.path().join("configured"), vec![config(&alias)]).is_err()
            );
            assert!(
                broker
                    .register_native_folder(&selection(&alias, "alias"))
                    .is_err()
            );
            assert!(FileBroker::open(&state, vec![]).is_err());
        }
        assert_eq!(
            fs::read(state.join("folder-registry.json")).unwrap(),
            registry
        );
        fs::remove_dir(&alias)
            .or_else(|_| fs::remove_file(&alias))
            .unwrap();
        unchanged(parent);
        unchanged(&outside);
    }
}
