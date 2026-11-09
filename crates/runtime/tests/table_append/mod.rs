//! Real native Excel publication through the existing plan, approval and effect journal.
use super::*;
use serde_json::Value;

async fn table_fixture(duplicate: bool) -> Fixture {
    let mut fixture = Fixture::new(false).await;
    let original: Value = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/workflow-table-append-result.json"
    ))
    .unwrap();
    let patch = &original["tool"]["arguments"]["spec"]["patches"][0];
    let fields = ["invoice", "gross", "issued", "paid", "note"];
    let columns: Vec<_> = patch["columns"]
        .as_array()
        .unwrap()
        .iter()
        .zip(fields)
        .map(|(column, field)| {
            let mut column = column.clone();
            column["field"] = json!(field);
            column
        })
        .collect();
    let operation: WorkflowDocumentOperation =
        serde_json::from_value(json!({"kind":"append_table_rows",
        "path_input":"path","sha256_input":"sha","output_path_input":"output","rows_input":"rows",
        "sheet":"Invoices","table":"InvoiceTable","columns":columns,"key_fields":["invoice"]}))
        .unwrap();
    let inputs: BTreeMap<_,_>=operation.input_types().into_iter().map(|(name,kind)|{
        let value=match name.as_str(){
            "path"=>json!({"source":"input","pointer":"/source_path"}),
            "sha"=>json!({"source":"input","pointer":"/source_sha256"}),
            "output"=>json!({"source":"literal","value":"updated.xlsx"}),
            "rows"=>json!({"source":"literal","value":[{"invoice":if duplicate {"INV-001"} else {"INV-002"},
                "gross":11900,"issued":"2026-11-01","paid":false,"note":"=1+1"}]}),
            _=>unreachable!(),
        };
        (name,json!({"value_type":kind,"value":value}))
    }).collect();
    let mut source = serde_json::to_value(definition(false)).unwrap();
    let rights = json!({"current_workspace":true,"tools":["document_write"],"folders":[]});
    source["rights"] = rights.clone();
    source["documents"] = json!({"append-invoice":{"name":"Append invoice","dependencies":[],"folder":{"kind":"current_workspace"},
        "operation":operation,"rights":rights,"budgets":source["budgets"]}});
    source["dataflow"]["steps"] =
        json!({"append-invoice":{"inputs":inputs,"output":operation.output_type()}});
    let source: WorkflowDefinition = serde_json::from_value(source).unwrap();
    source.validate_definition().unwrap();
    let entry = fixture
        .runtime
        .store
        .create_workflow(&fixture.project.id, "native-table-source", &source)
        .await
        .unwrap();
    fixture.workflow = entry.id;
    fixture.version = entry.versions[0].id.clone();
    let bytes = include_bytes!("../../../../tests/fixtures/workflow-table-source.xlsx");
    std::fs::write(fixture.original().join("invoices.xlsx"), bytes).unwrap();
    fixture.source_name = "invoices.xlsx".into();
    fixture.source_sha = hash(bytes);
    fixture
}

fn results(fixture: &Fixture) -> Vec<PathBuf> {
    fn visit(path: &Path, found: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                visit(&path, found);
            } else if path.file_name().and_then(|name| name.to_str()) == Some("updated.xlsx") {
                found.push(path);
            }
        }
    }
    let mut found = vec![];
    visit(&fixture.temp.path().join("broker"), &mut found);
    found
}

#[tokio::test]
#[ignore = "requires explicitly configured actual document Python"]
async fn native_table_append_requires_original_approval_and_publishes_one_typed_result_without_a_model()
 {
    let f = table_fixture(false).await;
    let run = f.commit(AccessMode::ConfirmMutations).await;
    std::fs::write(
        f.original().join("invoices.xlsx"),
        b"changed unbound original",
    )
    .unwrap();
    let worker = Worker::start(f.runtime.clone());
    let approval = f.approval(&run.id).await;
    assert!(results(&f).is_empty());
    f.runtime
        .store
        .resolve_approval(
            &run.id,
            &approval.id,
            ResolveApproval {
                fingerprint: approval.fingerprint,
                allow: true,
            },
        )
        .await
        .unwrap();
    let done = f.completed(&run.id).await;
    assert_eq!(done.approvals.len(), 1);
    assert_eq!(done.file_operations.len(), 1);
    assert!(done.run.model_attempts.is_empty());
    f.assert_native(&run.id, 1).await;
    let paths = results(&f);
    assert_eq!(paths.len(), 1);
    let output=std::process::Command::new(python()).args(["-I","-c",
        "import sys,openpyxl,datetime; b=openpyxl.load_workbook(sys.argv[1]); s=b['Invoices']; assert s.tables['InvoiceTable'].ref=='A1:E3'; assert s['B3'].value==119 and s['B3'].data_type=='n' and s['B3'].number_format=='#,##0.00'; assert s['C3'].value==datetime.datetime(2026,11,1); assert s['D3'].value is False; assert s['E3'].value=='=1+1' and s['E3'].data_type=='s'; assert s['A2'].value=='INV-001' and s['B2'].value==42.15; assert b['Other']['A1'].comment.text=='original annotation'"
    ]).arg(&paths[0]).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stored = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    let WorkflowNodeState::Completed { output, .. } =
        &stored.progress.nodes()["append-invoice"].state
    else {
        panic!("missing native table output")
    };
    let output: Value = serde_json::from_str(output).unwrap();
    assert_eq!(output["source"]["sha256"], f.source_sha);
    assert_eq!(output["table"]["range"], "A1:E3");
    assert_eq!(
        output["file"]["sha256"],
        hash(&std::fs::read(&paths[0]).unwrap())
    );
    assert_eq!(
        std::fs::read(f.original().join("invoices.xlsx")).unwrap(),
        b"changed unbound original"
    );
    let bytes = std::fs::read(&paths[0]).unwrap();
    assert!(matches!(
        f.runtime
            .reconcile_workflow_effects(&f.project.id, &run.id)
            .await,
        Err(cowork_domain::Error::Conflict)
    ));
    assert_eq!(std::fs::read(&paths[0]).unwrap(), bytes);
    assert_eq!(results(&f).len(), 1);
    worker.close().await;
}

#[tokio::test]
#[ignore = "requires explicitly configured actual document Python"]
async fn native_table_duplicate_key_produces_no_file_effect_and_keeps_the_source() {
    let f = table_fixture(true).await;
    let original = std::fs::read(f.original().join("invoices.xlsx")).unwrap();
    let run = f.commit(AccessMode::Autonomous).await;
    let worker = Worker::start(f.runtime.clone());
    let done = f
        .wait(&run.id, |detail| {
            matches!(detail.run.state, RunState::Failed | RunState::Interrupted)
        })
        .await;
    assert!(done.file_operations.is_empty() && done.run.model_attempts.is_empty());
    assert!(results(&f).is_empty());
    assert_eq!(
        std::fs::read(f.original().join("invoices.xlsx")).unwrap(),
        original
    );
    let stored = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    assert_eq!(stored.progress.charged_tokens(), 0);
    assert!(
        stored.agents["append-invoice"].turns[0]
            .tool_results
            .iter()
            .any(|result| !result.succeeded && result.result.to_string().contains("duplicate_key"))
    );
    worker.close().await;
}

#[tokio::test]
#[ignore = "requires explicitly configured actual document Python"]
async fn native_table_denied_approval_keeps_the_source_and_creates_no_output() {
    let f = table_fixture(false).await;
    let original = std::fs::read(f.original().join("invoices.xlsx")).unwrap();
    let run = f.commit(AccessMode::ConfirmMutations).await;
    let worker = Worker::start(f.runtime.clone());
    let approval = f.approval(&run.id).await;
    f.runtime
        .store
        .resolve_approval(
            &run.id,
            &approval.id,
            ResolveApproval {
                fingerprint: approval.fingerprint,
                allow: false,
            },
        )
        .await
        .unwrap();
    let done = f
        .wait(&run.id, |detail| {
            matches!(detail.run.state, RunState::Failed | RunState::Interrupted)
        })
        .await;
    assert!(done.file_operations.is_empty() && done.run.model_attempts.is_empty());
    assert!(results(&f).is_empty());
    assert_eq!(
        std::fs::read(f.original().join("invoices.xlsx")).unwrap(),
        original
    );
    worker.close().await;
}

async fn table_publication_recovery(source_ack: bool) {
    let f = table_fixture(false).await;
    let original = std::fs::read(f.original().join("invoices.xlsx")).unwrap();
    let run = f.commit(AccessMode::Autonomous).await;
    let path = f.temp.path().join("native-documents.sqlite3");
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
    let trigger = if source_ack {
        "CREATE TRIGGER lose_table_ack BEFORE UPDATE ON workflow_external_evidence WHEN json_extract(NEW.payload,'$.kind')='file_operation' AND NEW.result_sha256 IS NOT NULL BEGIN SELECT RAISE(ABORT,'lost table source outcome'); END"
    } else {
        "CREATE TRIGGER lose_table_ack BEFORE INSERT ON events WHEN json_extract(NEW.payload,'$.kind')='workflow_native_tool_completed' AND json_extract(NEW.payload,'$.payload.node_id')='append-invoice' BEGIN SELECT RAISE(ABORT,'lost table graph outcome'); END"
    };
    sqlx::query(trigger).execute(&pool).await.unwrap();
    assert!(
        f.runtime
            .run_native_workflow(&f.project.id, &run.id, &CancellationToken::new())
            .await
            .is_err()
    );
    assert_eq!(
        f.runtime.store.detail(&run.id).await.unwrap().run.state,
        RunState::Interrupted
    );
    let before = f
        .runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    let attempt = before
        .progress
        .attempts()
        .values()
        .find(|attempt| {
            attempt.claim.node_id == "append-invoice"
                && matches!(attempt.operation, WorkflowOperation::Tool { .. })
        })
        .unwrap();
    let evidence = f
        .runtime
        .store
        .workflow_external_evidence(&f.project.id, &run.id, &attempt.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(evidence.kind, "file_operation");
    assert_eq!(evidence.result.is_none(), source_ack);
    let paths = results(&f);
    assert_eq!(paths.len(), 1);
    let published = std::fs::read(&paths[0]).unwrap();
    let expected = hash(&published);
    assert_eq!(
        evidence.preparation["operation_receipt"]["after"]["sha256"],
        expected
    );
    assert_eq!(
        evidence.preparation["document"]["engine"]["result"]["table_append"]["rows_added"],
        1
    );
    sqlx::query("DROP TRIGGER lose_table_ack")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    f.runtime.store.close().await;
    let runtime = Runtime::new(Store::sqlite(&path).await.unwrap(), vec![], false)
        .unwrap()
        .with_file_broker(
            cowork_broker::FileBroker::open(&f.temp.path().join("broker"), vec![]).unwrap(),
        )
        .with_toolbox(f.toolbox.clone());
    let settled = runtime
        .reconcile_workflow_effects(&f.project.id, &run.id)
        .await
        .unwrap();
    assert_eq!(settled["settled_calls"].as_array().unwrap().len(), 1);
    assert!(
        settled["unresolved_calls"].as_array().unwrap().is_empty(),
        "{settled}"
    );
    let again = runtime
        .reconcile_workflow_effects(&f.project.id, &run.id)
        .await
        .unwrap();
    assert!(again["settled_calls"].as_array().unwrap().is_empty());
    runtime.store.resume(&run.id).await.unwrap();
    let done = runtime
        .run_native_workflow(&f.project.id, &run.id, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(done.state, RunState::Completed);
    let detail = runtime.store.detail(&run.id).await.unwrap();
    assert_eq!(detail.file_operations.len(), 1);
    assert!(detail.run.model_attempts.is_empty());
    let graph = runtime
        .store
        .stored_workflow(&f.project.id, &run.id)
        .await
        .unwrap();
    assert_eq!(
        cowork_domain::canonical_hash(&before.plan),
        cowork_domain::canonical_hash(&graph.plan)
    );
    assert_eq!(graph.progress.charged_tokens(), 0);
    assert!(graph.plan.profiles.is_empty());
    assert_eq!(graph.agents["append-invoice"].turns.len(), 1);
    let WorkflowNodeState::Completed { output, .. } =
        &graph.progress.nodes()["append-invoice"].state
    else {
        panic!("missing recovered table output")
    };
    let output: Value = serde_json::from_str(output).unwrap();
    assert_eq!(output["table"]["rows_added"], 1);
    assert_eq!(output["source"]["sha256"], f.source_sha);
    assert_eq!(output["file"]["sha256"], expected);
    assert_eq!(results(&f).len(), 1);
    assert_eq!(std::fs::read(&paths[0]).unwrap(), published);
    assert_eq!(
        std::fs::read(f.original().join("invoices.xlsx")).unwrap(),
        original
    );
    runtime.store.close().await;
}

#[tokio::test]
#[ignore = "requires explicitly configured actual document Python"]
async fn native_table_lost_graph_ack_reopens_without_another_row_or_file() {
    table_publication_recovery(false).await;
}

#[tokio::test]
#[ignore = "requires explicitly configured actual document Python"]
async fn native_table_lost_source_outcome_reopens_without_another_row_or_file() {
    table_publication_recovery(true).await;
}
