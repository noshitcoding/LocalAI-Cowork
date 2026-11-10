use cowork_contracts::{WorkspaceTransferChunk, WorkspaceTransferFile};
use cowork_domain::{hash, workspace_transfer as transfer};

fn file(path: &str, size: u64) -> WorkspaceTransferFile {
    WorkspaceTransferFile {
        path: path.into(),
        size,
        sha256: hash(if size == 0 { b"" } else { b"fixture" }),
        chunks: (0..size.div_ceil(transfer::CHUNK_BYTES))
            .map(|index| WorkspaceTransferChunk {
                offset: index * transfer::CHUNK_BYTES,
                size: transfer::CHUNK_BYTES.min(size - index * transfer::CHUNK_BYTES),
                sha256: hash(b"chunk"),
            })
            .collect(),
    }
}

#[test]
fn manifest_requires_complete_content_structure_and_original_fingerprint() {
    let manifest = transfer::build(
        vec!["Berichte".into()],
        vec![
            file("Berichte/Übersicht.txt", transfer::CHUNK_BYTES + 3),
            file("empty", 0),
        ],
    )
    .unwrap();
    transfer::validate(&manifest).unwrap();
    let mut changed = manifest.clone();
    changed.files[0].chunks[1].offset += 1;
    assert!(transfer::validate(&changed).is_err());
    changed.sha256 = transfer::manifest_hash(&changed);
    assert!(transfer::validate(&changed).is_err());
    let mut changed = manifest.clone();
    changed.total_bytes += 1;
    changed.sha256 = transfer::manifest_hash(&changed);
    assert!(transfer::validate(&changed).is_err());
    let mut changed = manifest.clone();
    changed.files[0].chunks[0].size = 0;
    changed.sha256 = transfer::manifest_hash(&changed);
    assert!(transfer::validate(&changed).is_err());
    let mut changed = manifest;
    changed.format_version = 2;
    changed.sha256 = transfer::manifest_hash(&changed);
    assert!(transfer::validate(&changed).is_err());
}

#[test]
fn cross_platform_aliases_and_noncanonical_trees_never_materialize() {
    for path in [
        "../escape",
        "/host",
        "a//b",
        "a/./b",
        "C:drive",
        "a\\b",
        "NUL.txt",
        "COM1",
        "LPT².log",
        "trailing.",
        "trailing ",
        "a?b",
        "a:b",
        ".cowork-token",
        "x\0y",
    ] {
        assert!(transfer::validate_path(path).is_err(), "{path}");
    }
    assert!(transfer::build(vec![], vec![file("missing/child", 1)]).is_err());
    assert!(transfer::build(vec![], vec![file("A.txt", 1), file("a.txt", 1)]).is_err());
    assert!(transfer::build(vec!["A".into()], vec![file("a", 1)]).is_err());
    assert!(transfer::build(vec![], vec![file("b", 1), file("a", 1)]).is_err());
    assert!(transfer::build(vec!["missing/child".into()], vec![]).is_err());
    assert!(
        transfer::build(
            vec![],
            vec![file("too-large", transfer::MAX_FILE_BYTES + 1)]
        )
        .is_err()
    );
}

#[test]
fn empty_files_and_empty_directories_keep_distinct_content_identity() {
    let empty = transfer::build(vec![], vec![]).unwrap();
    let directory = transfer::build(vec!["empty".into()], vec![]).unwrap();
    let file = transfer::build(vec![], vec![file("empty", 0)]).unwrap();
    assert_ne!(empty.sha256, directory.sha256);
    assert_ne!(file.sha256, directory.sha256);
    assert_ne!(file.sha256, empty.sha256);
}
