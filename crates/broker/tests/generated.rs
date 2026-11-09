use cowork_broker::{FileBroker, GeneratedContent};
use cowork_contracts::*;
use cowork_domain::{Error, hash};
use serde_json::json;

fn fixture(access: &str) -> (tempfile::TempDir, FileBroker, RunRecord, Workspace) {
    let temp = tempfile::tempdir().unwrap();
    let broker = FileBroker::open(temp.path(), vec![]).unwrap();
    let order = Order {
        id: uuid::Uuid::new_v4().to_string(),
        project_id: "project".into(),
        title: "Generated documents".into(),
        draft: String::new(),
        revision: 1,
        created_at: 0,
    };
    let workspace = broker.ensure_workspace(&order, None, None).unwrap();
    let run: RunRecord = serde_json::from_value(json!({"id":"run","spec":{"order_id":order.id,"project_id":order.project_id,"prompt":"Create a report","provider":null,"access_mode":access,"executor":"local-v2","policy_revision":1,"allowed_tools":["document_write","image_edit"],"budgets":{"max_steps":64,"max_seconds":1800,"max_output_tokens":4096},"folders":[workspace.folder]},"state":"running","revision":1,"last_sequence":1,"steps":0,"created_at":0,"updated_at":0,"started_at":null,"error_code":null,"usage":{}})).unwrap();
    (temp, broker, run, workspace)
}

fn tool(name: &str) -> ToolCall {
    ToolCall {
        id: format!("{name}-call"),
        name: name.into(),
        arguments: json!({}),
    }
}

fn provenance(input: &[u8]) -> DocumentProvenance {
    DocumentProvenance {
        tool: "document_write".into(),
        operation: "create".into(),
        engine_version: "1".into(),
        inputs: vec![DocumentProvenanceInput {
            folder_id: "source".into(),
            path: "a.docx".into(),
            sha256: hash(input),
        }],
        receipt_sha256: hash(b"receipt"),
    }
}

fn file_path(temp: &tempfile::TempDir, workspace: &Workspace, path: &str) -> std::path::PathBuf {
    temp.path()
        .join("workspaces")
        .join(&workspace.order_id)
        .join("files")
        .join(path)
}

#[test]
fn generated_file_is_published_create_only_with_provenance_and_reconcilable_state() {
    let (temp, broker, run, workspace) = fixture("confirm_mutations");
    let bytes = b"PK\x03\x04 generated docx".to_vec();
    let prepared = broker
        .prepare_generated(
            &run,
            &tool("document_write"),
            &workspace.folder.id,
            "report.docx",
            &GeneratedContent::File(bytes.clone()),
            provenance(b"a"),
        )
        .unwrap();
    let operation = prepared.operation.clone();
    assert_eq!(operation.kind, FileOperationKind::Generate);
    assert!(operation.before.is_none());
    assert_eq!(operation.after.as_ref().unwrap().sha256, hash(&bytes));
    assert_eq!(
        operation.generated_from.as_ref().unwrap().inputs[0].path,
        "a.docx"
    );
    // Nothing is visible before apply, and the prepared publish holds the folder lock.
    assert!(!file_path(&temp, &workspace, "report.docx").exists());
    assert!(matches!(
        broker.list_workspace(&workspace, "."),
        Err(Error::WorkspaceBusy)
    ));
    assert!(broker.reconcile_operation(&run, &operation, false).is_err());
    prepared.apply().unwrap();
    drop(prepared);
    assert_eq!(
        std::fs::read(file_path(&temp, &workspace, "report.docx")).unwrap(),
        bytes
    );
    let lock = broker.reconcile_operation(&run, &operation, true).unwrap();
    drop(lock);
    assert!(matches!(
        broker.reconcile_operation(&run, &operation, false),
        Err(Error::FileConflict)
    ));
    // An existing target is never replaced.
    assert!(matches!(
        broker.prepare_generated(
            &run,
            &tool("image_edit"),
            &workspace.folder.id,
            "report.docx",
            &GeneratedContent::File(b"other".to_vec()),
            provenance(b"a")
        ),
        Err(Error::FileConflict)
    ));
    assert_eq!(
        std::fs::read(file_path(&temp, &workspace, "report.docx")).unwrap(),
        bytes
    );
    let names: Vec<_> = std::fs::read_dir(file_path(&temp, &workspace, ""))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names.len(), 1, "no stage leftovers: {names:?}");
}

#[test]
fn generated_directory_and_dropped_stage_and_rights() {
    let (temp, broker, run, workspace) = fixture("confirm_mutations");
    let files = vec![
        ("part-1.pdf".to_string(), b"%PDF-1".to_vec()),
        ("more/part-2.pdf".to_string(), b"%PDF-2".to_vec()),
    ];
    let prepared = broker
        .prepare_generated(
            &run,
            &tool("document_write"),
            &workspace.folder.id,
            "parts",
            &GeneratedContent::Directory(files),
            provenance(b"a"),
        )
        .unwrap();
    assert_eq!(
        prepared.operation.after.as_ref().unwrap().kind,
        FileNodeKind::Directory
    );
    assert_eq!(
        prepared
            .operation
            .entries
            .iter()
            .filter(|e| e.sha256.is_some())
            .count(),
        2
    );
    prepared.apply().unwrap();
    drop(prepared);
    assert_eq!(
        std::fs::read(file_path(&temp, &workspace, "parts/more/part-2.pdf")).unwrap(),
        b"%PDF-2"
    );
    // A prepared but never applied publish leaves nothing behind.
    let abandoned = broker
        .prepare_generated(
            &run,
            &tool("image_edit"),
            &workspace.folder.id,
            "photo.png",
            &GeneratedContent::File(b"png".to_vec()),
            provenance(b"a"),
        )
        .unwrap();
    drop(abandoned);
    let mut names: Vec<_> = std::fs::read_dir(file_path(&temp, &workspace, ""))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, vec!["parts"]);
    // Only document write tools publish; missing parents and read-only runs are refused.
    assert!(matches!(
        broker.prepare_generated(
            &run,
            &tool("write_text_file"),
            &workspace.folder.id,
            "x.docx",
            &GeneratedContent::File(vec![1]),
            provenance(b"a")
        ),
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        broker.prepare_generated(
            &run,
            &tool("document_write"),
            &workspace.folder.id,
            "missing/x.docx",
            &GeneratedContent::File(vec![1]),
            provenance(b"a")
        ),
        Err(Error::Validation(_))
    ));
    let (_temp, readonly_broker, readonly, readonly_workspace) = fixture("read_only");
    assert!(
        readonly_broker
            .prepare_generated(
                &readonly,
                &tool("document_write"),
                &readonly_workspace.folder.id,
                "x.docx",
                &GeneratedContent::File(vec![1]),
                provenance(b"a")
            )
            .is_err()
    );
}

#[test]
fn binary_reads_are_bounded() {
    let (temp, broker, run, workspace) = fixture("confirm_mutations");
    std::fs::write(
        file_path(&temp, &workspace, "image.bin"),
        [0u8, 159, 146, 150],
    )
    .unwrap();
    let location = FileLocation {
        folder_id: workspace.folder.id.clone(),
        path: "image.bin".into(),
    };
    assert_eq!(
        broker.read_binary(&run, &location, 1024).unwrap(),
        vec![0u8, 159, 146, 150]
    );
    assert!(matches!(
        broker.read_binary(&run, &location, 2),
        Err(Error::Validation(_))
    ));
    let missing = FileLocation {
        folder_id: workspace.folder.id.clone(),
        path: "nope.bin".into(),
    };
    assert!(matches!(
        broker.read_binary(&run, &missing, 1024),
        Err(Error::NotFound)
    ));
}
