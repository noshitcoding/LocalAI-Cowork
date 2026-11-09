use super::*;
use cowork_domain::canonical_hash;
use cowork_workflow_contracts::{
    CreateWorkflow, PrepareWorkflowOperation, PublishWorkflow, RevokeWorkflow, SaveWorkflowDraft,
    WorkflowDraftContent, WorkflowDraftSave, WorkflowDraftSnapshotRef, WorkflowOperation,
    WorkflowOperationReceipt, WorkflowOperationRejection, WorkflowOperationRequest,
    WorkflowOperationState, WorkflowRunKind, WorkflowRunRequest,
};
fn save(id: &str, basis: i64, key: &str, name: &str) -> SaveWorkflowDraft {
    SaveWorkflowDraft {
        id: id.into(),
        base_revision: basis,
        idempotency_key: key.into(),
        content: WorkflowDraftContent {
            editor_format: "workflow-editor-v1".into(),
            data: json!({"name":name,"roles":[{"provider_id":"","provider_revision":0}],"max_steps":"unfinished"}),
            origin: None,
            archived: false,
        },
    }
}
fn intent(request: WorkflowOperationRequest) -> PrepareWorkflowOperation {
    PrepareWorkflowOperation {
        intent_id: id(),
        request,
        draft_snapshot: None,
    }
}
fn start(f: &Fixture, key: &str) -> WorkflowRunRequest {
    WorkflowRunRequest {
        input_source: None,
        inputs: None,
        agent_config: None,
        native_tools: vec![],
        folders: vec![],
        mcp_tools: vec![],
        environment: None,
        kind: WorkflowRunKind::Workflow,
        order_id: f.spec.order_id.clone(),
        base_revision: f.request.base_revision,
        idempotency_key: key.into(),
        prompt: f.spec.prompt.clone(),
        workflow_id: f.request.workflow_id.clone(),
        version_id: f.request.version_id.clone(),
        access_mode: f.spec.access_mode.clone(),
        budgets: f.spec.budgets.clone(),
        expected_binding: None,
    }
}
async fn trusted_binding(
    store: &Store,
    input: &WorkflowRunRequest,
    spec: &RunSpec,
) -> WorkflowBinding {
    let plan = store
        .product_preflight_compilation(input, spec)
        .await
        .unwrap();
    WorkflowBinding {
        inputs: None,
        workflow_id: input.workflow_id.clone(),
        version_id: input.version_id.clone(),
        definition_sha256: plan.source_sha256,
        plan_sha256: plan.sha256,
    }
}
fn receipt(op: &WorkflowOperation) -> &WorkflowOperationReceipt {
    assert_eq!(op.state, WorkflowOperationState::Accepted);
    op.receipt.as_ref().unwrap()
}
async fn drafts(store: &Store) {
    let p = store
        .create_project(CreateProject {
            name: "Partial draft fixture".into(),
        })
        .await
        .unwrap();
    let draft_id = id();
    let initial = save(&draft_id, 0, "first", "");
    let first = store.save_workflow_draft(&p.id, &initial).await.unwrap();
    let WorkflowDraftSave::Saved {
        draft: head,
        snapshot: first_snapshot,
    } = &first
    else {
        panic!("first save")
    };
    assert_eq!(head.revision, 1);
    assert_eq!(head.content.data["roles"][0]["provider_revision"], 0);
    let second_input = save(&draft_id, 1, "second", "Confirmed head");
    let second = store
        .save_workflow_draft(&p.id, &second_input)
        .await
        .unwrap();
    let WorkflowDraftSave::Saved {
        draft: second_head, ..
    } = &second
    else {
        panic!("second save")
    };
    assert_eq!(second_head.revision, 2);
    assert_eq!(
        canonical_hash(&store.save_workflow_draft(&p.id, &initial).await.unwrap()),
        canonical_hash(&first)
    );
    let conflicting = save(&draft_id, 1, "conflict", "Preserved incoming text");
    let conflict = store
        .save_workflow_draft(&p.id, &conflicting)
        .await
        .unwrap();
    let WorkflowDraftSave::Conflict { draft, snapshot } = &conflict else {
        panic!("conflict copy")
    };
    assert_eq!(draft.revision, 2);
    assert!(snapshot.conflict);
    assert_eq!(snapshot.content.data["name"], "Preserved incoming text");
    assert_eq!(
        store
            .workflow_draft(&p.id, &draft_id)
            .await
            .unwrap()
            .content
            .data["name"],
        "Confirmed head"
    );
    assert_eq!(
        canonical_hash(
            &store
                .save_workflow_draft(&p.id, &conflicting)
                .await
                .unwrap()
        ),
        canonical_hash(&conflict)
    );
    assert_eq!(
        store
            .list_workflow_draft_snapshots(&p.id, &draft_id, 0, 100)
            .await
            .unwrap()
            .total,
        3
    );
    let mut changed = initial.clone();
    changed.content.data["name"] = json!("same key changed body");
    assert!(matches!(
        store.save_workflow_draft(&p.id, &changed).await,
        Err(Error::Conflict)
    ));
    assert_eq!(
        store
            .workflow_draft_snapshot(&p.id, &draft_id, &first_snapshot.id)
            .await
            .unwrap()
            .content
            .data["name"],
        ""
    );
    let foreign = store
        .create_project(CreateProject {
            name: "Other draft scope".into(),
        })
        .await
        .unwrap();
    assert!(matches!(
        store.workflow_draft(&foreign.id, &draft_id).await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        store.save_workflow_draft(&foreign.id, &initial).await,
        Err(Error::NotFound)
    ));
    let overflow = save(&draft_id, i64::MAX, "overflow", "");
    assert!(matches!(
        store.save_workflow_draft(&p.id, &overflow).await,
        Err(Error::Validation(_))
    ));
    let listing = store.list_workflow_drafts(&p.id, 0, 1).await.unwrap();
    assert_eq!(listing.total, 1);
    assert_eq!(listing.items[0].revision, 2);
}
async fn identities(store: &Store) {
    let f = tool_cases::tool_fixture(store).await;
    let draft_id = id();
    let WorkflowDraftSave::Saved { snapshot, .. } = store
        .save_workflow_draft(
            &f.spec.project_id,
            &save(&draft_id, 0, "save", "Incomplete"),
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    let input = start(&f, "prepared-start");
    let mut prepare = intent(WorkflowOperationRequest::Start {
        request: input.clone().into(),
    });
    prepare.draft_snapshot = Some(WorkflowDraftSnapshotRef {
        draft_id: draft_id.clone(),
        snapshot_id: snapshot.id.clone(),
        sha256: snapshot.sha256.clone(),
    });
    let op = store
        .prepare_workflow_operation(&f.spec.project_id, &prepare)
        .await
        .unwrap();
    assert_eq!(op.state, WorkflowOperationState::Pending);
    assert_eq!(
        store
            .prepare_workflow_operation(&f.spec.project_id, &prepare)
            .await
            .unwrap()
            .id,
        op.id
    );
    let mut changed = prepare.clone();
    let WorkflowOperationRequest::Start { request } = &mut changed.request else {
        panic!()
    };
    request.base_revision += 1;
    assert!(matches!(
        store
            .prepare_workflow_operation(&f.spec.project_id, &changed)
            .await,
        Err(Error::Conflict)
    ));
    let mut changed = prepare.clone();
    changed.draft_snapshot = None;
    assert!(matches!(
        store
            .prepare_workflow_operation(&f.spec.project_id, &changed)
            .await,
        Err(Error::Conflict)
    ));
    let mut changed = prepare.clone();
    changed.intent_id = id();
    assert!(matches!(
        store
            .prepare_workflow_operation(&f.spec.project_id, &changed)
            .await,
        Err(Error::Conflict)
    ));
    assert!(matches!(
        store
            .acknowledge_workflow_operation(&f.spec.project_id, &op.id)
            .await,
        Err(Error::Conflict)
    ));
    assert!(matches!(
        store
            .reject_pending_workflow_operation(
                &f.spec.project_id,
                &op.id,
                &WorkflowOperationRejection {
                    code: "provider_unavailable".into(),
                    message: "Transient provider outage".into()
                }
            )
            .await,
        Err(Error::Validation(_))
    ));
    let foreign = store
        .create_project(CreateProject {
            name: "Foreign journal scope".into(),
        })
        .await
        .unwrap();
    assert!(matches!(
        store.workflow_operation(&foreign.id, &op.id).await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        store
            .prepare_workflow_operation(&foreign.id, &prepare)
            .await,
        Err(Error::NotFound)
    ));
    let rejected = store
        .reject_pending_workflow_operation(
            &f.spec.project_id,
            &op.id,
            &WorkflowOperationRejection {
                code: "revision_conflict".into(),
                message: "Review the saved basis".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(rejected.state, WorkflowOperationState::Rejected);
    assert_eq!(
        store
            .discard_workflow_operation(&f.spec.project_id, &op.id)
            .await
            .unwrap()
            .state,
        WorkflowOperationState::Rejected
    );
    let expected = trusted_binding(store, &input, &f.spec).await;
    assert_eq!(
        store
            .commit_workflow_run_operation(&f.spec.project_id, &op.id, f.spec.clone(), &expected)
            .await
            .unwrap()
            .state,
        WorkflowOperationState::Rejected
    );
    assert!(matches!(
        store
            .create_product_workflow_run(&input, f.spec.clone(), &expected)
            .await,
        Err(Error::Conflict)
    ));
    assert!(store.runs(&input.order_id).await.unwrap().is_empty());
    store
        .acknowledge_workflow_operation(&f.spec.project_id, &op.id)
        .await
        .unwrap();
    assert_eq!(
        store
            .list_workflow_operations(&f.spec.project_id, 0, 100, false)
            .await
            .unwrap()
            .total,
        0
    );
    assert_eq!(
        store
            .list_workflow_operations(&f.spec.project_id, 0, 100, true)
            .await
            .unwrap()
            .total,
        1
    );
}
async fn library(store: &Store) {
    let f = setup(store).await;
    let project = &f.spec.project_id;
    let create = intent(WorkflowOperationRequest::Create {
        request: CreateWorkflow {
            idempotency_key: "new-library".into(),
            definition: f.definition.clone(),
        },
    });
    let pending = store
        .prepare_workflow_operation(project, &create)
        .await
        .unwrap();
    assert!(matches!(
        store
            .create_workflow(project, "new-library", &f.definition)
            .await,
        Err(Error::Conflict)
    ));
    let committed = store
        .commit_workflow_library_operation(project, &pending.id)
        .await
        .unwrap();
    let WorkflowOperationReceipt::Created { publication } = receipt(&committed) else {
        panic!()
    };
    let created = publication.clone();
    let mut new_definition = f.definition.clone();
    new_definition.name = "Next immutable version".into();
    let publication = store
        .publish_workflow(
            project,
            &created.workflow_id,
            1,
            "new-version",
            &new_definition,
        )
        .await
        .unwrap();
    assert_eq!(publication.revision, 2);
    let replay = store
        .commit_workflow_library_operation(project, &pending.id)
        .await
        .unwrap();
    assert_eq!(
        canonical_hash(&replay.receipt),
        canonical_hash(&committed.receipt)
    );
    assert_eq!(
        store
            .discard_workflow_operation(project, &pending.id)
            .await
            .unwrap()
            .state,
        WorkflowOperationState::Accepted
    );
    let adoption = intent(WorkflowOperationRequest::Publish {
        workflow_id: created.workflow_id.clone(),
        request: PublishWorkflow {
            base_revision: 1,
            idempotency_key: "new-version".into(),
            definition: new_definition.clone(),
        },
    });
    let adopted = store
        .prepare_workflow_operation(project, &adoption)
        .await
        .unwrap();
    assert!(
        matches!(receipt(&adopted),WorkflowOperationReceipt::Published{publication:p} if p.revision==2)
    );
    let mut changed = adoption.clone();
    let WorkflowOperationRequest::Publish { request, .. } = &mut changed.request else {
        panic!()
    };
    request.base_revision = 2;
    assert!(matches!(
        store.prepare_workflow_operation(project, &changed).await,
        Err(Error::Conflict)
    ));
    let revoke = intent(WorkflowOperationRequest::Revoke {
        workflow_id: created.workflow_id.clone(),
        request: RevokeWorkflow {
            base_revision: 2,
            idempotency_key: "discard-revoke".into(),
        },
    });
    let pending = store
        .prepare_workflow_operation(project, &revoke)
        .await
        .unwrap();
    store
        .discard_workflow_operation(project, &pending.id)
        .await
        .unwrap();
    assert_eq!(
        store
            .commit_workflow_library_operation(project, &pending.id)
            .await
            .unwrap()
            .state,
        WorkflowOperationState::Discarded
    );
    assert!(matches!(
        store
            .revoke_workflow(project, &created.workflow_id, 2, "discard-revoke")
            .await,
        Err(Error::Conflict)
    ));
    assert!(
        !store
            .workflow(project, &created.workflow_id)
            .await
            .unwrap()
            .revoked
    );
    let publish = intent(WorkflowOperationRequest::Publish {
        workflow_id: created.workflow_id.clone(),
        request: PublishWorkflow {
            base_revision: 2,
            idempotency_key: "discard-publish".into(),
            definition: new_definition.clone(),
        },
    });
    let pending = store
        .prepare_workflow_operation(project, &publish)
        .await
        .unwrap();
    store
        .discard_workflow_operation(project, &pending.id)
        .await
        .unwrap();
    assert!(matches!(
        store
            .publish_workflow(
                project,
                &created.workflow_id,
                2,
                "discard-publish",
                &new_definition
            )
            .await,
        Err(Error::Conflict)
    ));
    let create = intent(WorkflowOperationRequest::Create {
        request: CreateWorkflow {
            idempotency_key: "discard-create".into(),
            definition: f.definition.clone(),
        },
    });
    let pending = store
        .prepare_workflow_operation(project, &create)
        .await
        .unwrap();
    store
        .discard_workflow_operation(project, &pending.id)
        .await
        .unwrap();
    assert!(matches!(
        store
            .create_workflow(project, "discard-create", &f.definition)
            .await,
        Err(Error::Conflict)
    ));
    let mut invalid = new_definition.clone();
    invalid.steps[0].dependencies = vec![invalid.steps[0].id.clone()];
    let invalid = intent(WorkflowOperationRequest::Publish {
        workflow_id: created.workflow_id.clone(),
        request: PublishWorkflow {
            base_revision: 2,
            idempotency_key: "invalid-publish".into(),
            definition: invalid,
        },
    });
    let pending = store
        .prepare_workflow_operation(project, &invalid)
        .await
        .unwrap();
    assert!(matches!(
        store
            .commit_workflow_library_operation(project, &pending.id)
            .await,
        Err(Error::Validation(_))
    ));
    assert_eq!(
        store
            .workflow_operation(project, &pending.id)
            .await
            .unwrap()
            .state,
        WorkflowOperationState::Pending
    );
    assert_eq!(
        store
            .workflow(project, &created.workflow_id)
            .await
            .unwrap()
            .revision,
        2
    );
}
async fn runs(store: &Store) {
    let f = tool_cases::tool_fixture(store).await;
    let project = &f.spec.project_id;
    let input = start(&f, "start-journal");
    let expected = trusted_binding(store, &input, &f.spec).await;
    let prepared = store
        .prepare_workflow_operation(
            project,
            &intent(WorkflowOperationRequest::Start {
                request: input.clone().into(),
            }),
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .create_product_workflow_run(&input, f.spec.clone(), &expected)
            .await,
        Err(Error::Conflict)
    ));
    let accepted = store
        .commit_workflow_run_operation(project, &prepared.id, f.spec.clone(), &expected)
        .await
        .unwrap();
    let WorkflowOperationReceipt::Started { run_id, .. } = receipt(&accepted) else {
        panic!()
    };
    let run_id = run_id.clone();
    assert_eq!(store.runs(&input.order_id).await.unwrap().len(), 1);
    assert_eq!(store.messages(&input.order_id).await.unwrap().len(), 1);
    assert_eq!(store.events(&run_id, 0).await.unwrap().len(), 2);
    assert_eq!(
        store.order(&input.order_id).await.unwrap().revision,
        input.base_revision + 1
    );
    let mut invalid_spec = f.spec.clone();
    invalid_spec.project_id = "unavailable-after-acceptance".into();
    assert_eq!(
        canonical_hash(
            &store
                .commit_workflow_run_operation(project, &prepared.id, invalid_spec, &expected)
                .await
                .unwrap()
                .receipt
        ),
        canonical_hash(&accepted.receipt)
    );
    assert_eq!(
        store
            .discard_workflow_operation(project, &prepared.id)
            .await
            .unwrap()
            .state,
        WorkflowOperationState::Accepted
    );
    assert_eq!(
        store
            .create_product_workflow_run(&input, f.spec.clone(), &expected)
            .await
            .unwrap()
            .id,
        run_id
    );
    let bounded = tool_cases::tool_fixture(store).await;
    let project = &bounded.spec.project_id;
    let mut input = start(&bounded, "broader-request");
    input.budgets.max_total_tokens *= 2;
    input.budgets.max_steps *= 2;
    input.budgets.max_seconds *= 2;
    input.budgets.max_output_tokens *= 2;
    let mut spec = bounded.spec.clone();
    spec.budgets = input.budgets.clone();
    let expected = trusted_binding(store, &input, &spec).await;
    let prepared = store
        .prepare_workflow_operation(
            project,
            &intent(WorkflowOperationRequest::Start {
                request: input.clone().into(),
            }),
        )
        .await
        .unwrap();
    let accepted = store
        .commit_workflow_run_operation(project, &prepared.id, spec.clone(), &expected)
        .await
        .unwrap();
    let WorkflowOperationReceipt::Started { spec: frozen, .. } = receipt(&accepted) else {
        panic!()
    };
    assert_eq!(
        canonical_hash(&frozen.budgets),
        canonical_hash(&bounded.spec.budgets)
    );
    assert_ne!(
        canonical_hash(&frozen.budgets),
        canonical_hash(&input.budgets)
    );
    assert_eq!(
        canonical_hash(
            &store
                .workflow_operation(project, &prepared.id)
                .await
                .unwrap()
                .receipt
        ),
        canonical_hash(&accepted.receipt)
    );
    store
        .revoke_workflow(
            project,
            &input.workflow_id,
            2,
            "revoke-after-bounded-acceptance",
        )
        .await
        .unwrap();
    assert_eq!(
        canonical_hash(
            &store
                .commit_workflow_run_operation(project, &prepared.id, spec, &expected)
                .await
                .unwrap()
                .receipt
        ),
        canonical_hash(&accepted.receipt)
    );
    assert!(
        store
            .list_workflow_operations(project, 0, 100, false)
            .await
            .is_ok()
    );
    let fresh = tool_cases::tool_fixture(store).await;
    let input = start(&fresh, "discarded-start");
    let expected = trusted_binding(store, &input, &fresh.spec).await;
    let pending = store
        .prepare_workflow_operation(
            &fresh.spec.project_id,
            &intent(WorkflowOperationRequest::Start {
                request: input.clone().into(),
            }),
        )
        .await
        .unwrap();
    store
        .discard_workflow_operation(&fresh.spec.project_id, &pending.id)
        .await
        .unwrap();
    assert_eq!(
        store
            .commit_workflow_run_operation(
                &fresh.spec.project_id,
                &pending.id,
                fresh.spec.clone(),
                &expected
            )
            .await
            .unwrap()
            .state,
        WorkflowOperationState::Discarded
    );
    assert!(matches!(
        store
            .create_product_workflow_run(&input, fresh.spec.clone(), &expected)
            .await,
        Err(Error::Conflict)
    ));
    let profile = store
        .providers()
        .await
        .unwrap()
        .into_iter()
        .find(|p| p.id == fresh.definition.roles[0].provider_id)
        .unwrap();
    let mut single_spec = fresh.spec.clone();
    single_spec.provider = Some(profile.clone());
    let single = CreateRun {
        agent_config: None,
        model_binding: None,
        output: None,
        native_tools: vec![],
        environment: None,
        mcp_tools: vec![],
        budgets: input.budgets.clone(),
        order_id: input.order_id.clone(),
        base_revision: input.base_revision,
        idempotency_key: input.idempotency_key.clone(),
        prompt: input.prompt.clone(),
        provider_id: profile.id,
        provider_revision: profile.revision,
        access_mode: input.access_mode.clone(),
        folders: vec![],
    };
    assert!(matches!(
        store.create_run(&single, single_spec).await,
        Err(Error::Conflict)
    ));
    assert!(store.runs(&input.order_id).await.unwrap().is_empty());
    assert!(store.messages(&input.order_id).await.unwrap().is_empty());
}
async fn races(store: &Store) {
    let f = setup(store).await;
    let project = &f.spec.project_id;
    for index in 0..8 {
        let key = format!("prepare-direct-{index}");
        let input = intent(WorkflowOperationRequest::Create {
            request: CreateWorkflow {
                idempotency_key: key.clone(),
                definition: f.definition.clone(),
            },
        });
        let (prepared, direct) = tokio::join!(
            store.prepare_workflow_operation(project, &input),
            store.create_workflow(project, &key, &f.definition)
        );
        let op = prepared.unwrap();
        match direct {
            Ok(entry) => {
                assert_eq!(op.state, WorkflowOperationState::Accepted);
                assert!(
                    matches!(op.receipt,Some(WorkflowOperationReceipt::Created{publication:p}) if p.workflow_id==entry.id)
                );
            }
            Err(Error::Conflict) => {
                assert_eq!(op.state, WorkflowOperationState::Pending);
                store
                    .commit_workflow_library_operation(project, &op.id)
                    .await
                    .unwrap();
            }
            other => panic!("Unexpected direct race: {other:?}"),
        }
        assert_eq!(
            store
                .prepare_workflow_operation(project, &input)
                .await
                .unwrap()
                .id,
            op.id
        );
    }
    for index in 0..8 {
        let input = intent(WorkflowOperationRequest::Create {
            request: CreateWorkflow {
                idempotency_key: format!("commit-discard-{index}"),
                definition: f.definition.clone(),
            },
        });
        let pending = store
            .prepare_workflow_operation(project, &input)
            .await
            .unwrap();
        let (commit, discard) = tokio::join!(
            store.commit_workflow_library_operation(project, &pending.id),
            store.discard_workflow_operation(project, &pending.id)
        );
        let commit = commit.unwrap();
        let discard = discard.unwrap();
        assert_eq!(commit.state, discard.state);
        let final_op = store
            .workflow_operation(project, &pending.id)
            .await
            .unwrap();
        assert_eq!(final_op.state, commit.state);
        assert!(matches!(
            final_op.state,
            WorkflowOperationState::Accepted | WorkflowOperationState::Discarded
        ));
    }
    for index in 0..4 {
        let fresh = setup(store).await;
        let project = &fresh.spec.project_id;
        let key = format!("publish-race-{index}");
        let mut definition = fresh.definition.clone();
        definition.name = "Exactly one raced publication".into();
        let input = intent(WorkflowOperationRequest::Publish {
            workflow_id: fresh.request.workflow_id.clone(),
            request: PublishWorkflow {
                base_revision: 1,
                idempotency_key: key.clone(),
                definition: definition.clone(),
            },
        });
        let (prepared, direct) = tokio::join!(
            store.prepare_workflow_operation(project, &input),
            store.publish_workflow(project, &fresh.request.workflow_id, 1, &key, &definition)
        );
        let op = prepared.unwrap();
        match direct {
            Ok(p) => {
                assert_eq!(op.state, WorkflowOperationState::Accepted);
                assert!(
                    matches!(op.receipt,Some(WorkflowOperationReceipt::Published{publication}) if publication.version_id==p.version_id)
                );
            }
            Err(Error::Conflict) => {
                assert_eq!(op.state, WorkflowOperationState::Pending);
                store
                    .commit_workflow_library_operation(project, &op.id)
                    .await
                    .unwrap();
            }
            other => panic!("Publication race failed: {other:?}"),
        };
        assert_eq!(
            store
                .workflow(project, &fresh.request.workflow_id)
                .await
                .unwrap()
                .versions
                .len(),
            2
        );
    }
    for index in 0..4 {
        let fresh = tool_cases::tool_fixture(store).await;
        let project = &fresh.spec.project_id;
        let request = start(&fresh, &format!("run-race-{index}"));
        let expected = trusted_binding(store, &request, &fresh.spec).await;
        let input = intent(WorkflowOperationRequest::Start {
            request: request.clone().into(),
        });
        let (prepared, direct) = tokio::join!(
            store.prepare_workflow_operation(project, &input),
            store.create_product_workflow_run(&request, fresh.spec.clone(), &expected)
        );
        let op = prepared.unwrap();
        match direct {
            Ok(run) => {
                assert_eq!(op.state, WorkflowOperationState::Accepted);
                assert!(
                    matches!(op.receipt,Some(WorkflowOperationReceipt::Started{run_id,..}) if run_id==run.id)
                );
            }
            Err(Error::Conflict) => {
                assert_eq!(op.state, WorkflowOperationState::Pending);
                store
                    .commit_workflow_run_operation(project, &op.id, fresh.spec.clone(), &expected)
                    .await
                    .unwrap();
            }
            other => panic!("Run race failed: {other:?}"),
        };
        assert_eq!(store.runs(&request.order_id).await.unwrap().len(), 1);
        assert_eq!(store.messages(&request.order_id).await.unwrap().len(), 1);
    }
    for index in 0..4 {
        let fresh = tool_cases::tool_fixture(store).await;
        let project = &fresh.spec.project_id;
        let request = start(&fresh, &format!("run-discard-race-{index}"));
        let expected = trusted_binding(store, &request, &fresh.spec).await;
        let pending = store
            .prepare_workflow_operation(
                project,
                &intent(WorkflowOperationRequest::Start {
                    request: request.clone().into(),
                }),
            )
            .await
            .unwrap();
        let (commit, discard) = tokio::join!(
            store.commit_workflow_run_operation(
                project,
                &pending.id,
                fresh.spec.clone(),
                &expected
            ),
            store.discard_workflow_operation(project, &pending.id)
        );
        let commit = commit.unwrap();
        let discard = discard.unwrap();
        assert_eq!(commit.state, discard.state);
        assert_eq!(
            store.runs(&request.order_id).await.unwrap().len(),
            usize::from(commit.state == WorkflowOperationState::Accepted)
        );
    }
}
async fn limits(store: &Store) {
    let p = store
        .create_project(CreateProject {
            name: "Draft capacity fixture".into(),
        })
        .await
        .unwrap();
    let draft_id = id();
    let mut oversized = save(&draft_id, 0, "big", "");
    oversized.content.data = json!("x".repeat(1_000_001));
    assert!(matches!(
        store.save_workflow_draft(&p.id, &oversized).await,
        Err(Error::Validation(_))
    ));
    let first = save(&draft_id, 0, "first", "");
    store.save_workflow_draft(&p.id, &first).await.unwrap();
    for index in 1..128 {
        store
            .save_workflow_draft(
                &p.id,
                &save(
                    &draft_id,
                    index,
                    &format!("save-{index}"),
                    "Within bounded history",
                ),
            )
            .await
            .unwrap();
    }
    assert!(matches!(
        store
            .save_workflow_draft(&p.id, &save(&draft_id, 128, "excess", "Not accepted"))
            .await,
        Err(Error::BudgetExceeded)
    ));
    assert_eq!(
        store
            .workflow_draft(&p.id, &draft_id)
            .await
            .unwrap()
            .revision,
        128
    );
    assert!(store.save_workflow_draft(&p.id, &first).await.is_ok());
    for index in 1..32 {
        store
            .save_workflow_draft(&p.id, &save(&id(), 0, &format!("draft-{index}"), ""))
            .await
            .unwrap();
    }
    assert!(matches!(
        store
            .save_workflow_draft(&p.id, &save(&id(), 0, "draft-excess", ""))
            .await,
        Err(Error::BudgetExceeded)
    ));
    assert_eq!(
        store.list_workflow_drafts(&p.id, 0, 1).await.unwrap().total,
        32
    );
    assert!(matches!(
        store.list_workflow_drafts(&p.id, 0, 0).await,
        Err(Error::Validation(_))
    ));
    assert!(matches!(
        store.list_workflow_operations(&p.id, 0, 101, false).await,
        Err(Error::Validation(_))
    ));
    let operation_fixture = setup(store).await;
    let project = &operation_fixture.spec.project_id;
    let mut first_operation = None;
    for index in 0..512 {
        let input = intent(WorkflowOperationRequest::Create {
            request: CreateWorkflow {
                idempotency_key: format!("bounded-op-{index}"),
                definition: operation_fixture.definition.clone(),
            },
        });
        let operation = store
            .prepare_workflow_operation(project, &input)
            .await
            .unwrap();
        if index == 0 {
            first_operation = Some(input);
        }
        store
            .discard_workflow_operation(project, &operation.id)
            .await
            .unwrap();
        store
            .acknowledge_workflow_operation(project, &operation.id)
            .await
            .unwrap();
    }
    let excess = intent(WorkflowOperationRequest::Create {
        request: CreateWorkflow {
            idempotency_key: "excess-op".into(),
            definition: operation_fixture.definition.clone(),
        },
    });
    assert!(matches!(
        store.prepare_workflow_operation(project, &excess).await,
        Err(Error::BudgetExceeded)
    ));
    assert_eq!(
        store
            .list_workflow_operations(project, 0, 100, true)
            .await
            .unwrap()
            .total,
        512
    );
    assert_eq!(
        store
            .list_workflow_operations(project, 0, 100, false)
            .await
            .unwrap()
            .total,
        0
    );
    assert_eq!(
        store
            .prepare_workflow_operation(project, &first_operation.unwrap())
            .await
            .unwrap()
            .state,
        WorkflowOperationState::Discarded
    );
    let storage_project = store
        .create_project(CreateProject {
            name: "Snapshot byte capacity".into(),
        })
        .await
        .unwrap();
    let byte_draft = id();
    let mut large = save(&byte_draft, 0, "bytes-0", "");
    large.content.data = json!({"incomplete_editor_text":"x".repeat(950_000)});
    store
        .save_workflow_draft(&storage_project.id, &large)
        .await
        .unwrap();
    for index in 1..35 {
        large.base_revision = index;
        large.idempotency_key = format!("bytes-{index}");
        store
            .save_workflow_draft(&storage_project.id, &large)
            .await
            .unwrap();
    }
    large.base_revision = 35;
    large.idempotency_key = "byte-excess".into();
    assert!(matches!(
        store.save_workflow_draft(&storage_project.id, &large).await,
        Err(Error::BudgetExceeded)
    ));
    assert_eq!(
        store
            .workflow_draft(&storage_project.id, &byte_draft)
            .await
            .unwrap()
            .revision,
        35
    );
}
pub(super) async fn contract(store: &Store) {
    Box::pin(drafts(store)).await;
    Box::pin(identities(store)).await;
    Box::pin(library(store)).await;
    Box::pin(runs(store)).await;
    Box::pin(races(store)).await;
    Box::pin(limits(store)).await;
}
#[tokio::test]
async fn journal_drafts_preserve_partial_values_conflicts_and_receipts() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    Box::pin(drafts(&store)).await;
}
#[tokio::test]
async fn journal_full_identity_snapshots_scopes_and_terminal_ack() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    Box::pin(identities(&store)).await;
}
#[tokio::test]
async fn journal_library_effects_and_discard_share_one_transaction() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    Box::pin(library(&store)).await;
}
#[tokio::test]
async fn journal_starts_replay_once_and_block_direct_bypass() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    Box::pin(runs(&store)).await;
}
#[tokio::test]
async fn journal_prepare_direct_and_commit_discard_races_are_fenced() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    Box::pin(races(&store)).await;
}
#[tokio::test]
async fn journal_bounded_durable_draft_history_and_pages() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    Box::pin(limits(&store)).await;
}

#[tokio::test]
#[ignore = "Requires a dedicated empty COWORK_TEST_POSTGRES database"]
async fn postgres_workflow_journal_contract() {
    let url = std::env::var("COWORK_TEST_POSTGRES").unwrap();
    let store = Store::connect(&url).await.unwrap();
    Box::pin(contract(&store)).await;
    store.close().await;
}

#[tokio::test]
async fn journal_restart_upgrade17_and_legacy_receipts_do_not_invent_originals() {
    let directory = tempfile::tempdir().unwrap();
    let source_path = directory.path().join("current-source.sqlite3");
    let path = directory.path().join("schema17.sqlite3");
    let store = Store::sqlite(&source_path).await.unwrap();
    let f = tool_cases::tool_fixture(&store).await;
    let request = start(&f, "pre18-run");
    let expected = trusted_binding(&store, &request, &f.spec).await;
    let old_run = store
        .create_product_workflow_run(&request, f.spec.clone(), &expected)
        .await
        .unwrap();
    let original_version = store
        .workflow(&f.spec.project_id, &f.request.workflow_id)
        .await
        .unwrap()
        .versions[0]
        .id
        .clone();
    let original_definition = store
        .workflow_version(
            &f.spec.project_id,
            &f.request.workflow_id,
            &original_version,
        )
        .await
        .unwrap()
        .definition;
    store.close().await;
    let pool = raw(&source_path).await;
    let tables = [
        (
            "workflows",
            "SELECT json_array(id,project_id,name,revision,revoked,request_key,request_sha256) FROM workflows ORDER BY id",
        ),
        (
            "workflow_versions",
            "SELECT json_array(id,workflow_id,project_id,revision,name,sha256,created_at,payload) FROM workflow_versions ORDER BY id",
        ),
        (
            "workflow_changes",
            "SELECT json_array(workflow_id,request_key,request_sha256,revision,payload) FROM workflow_changes ORDER BY workflow_id,request_key",
        ),
        (
            "runs",
            "SELECT json_array(id,order_id,idempotency_key,request_hash,revision,state,payload,updated_at,execution_kind) FROM runs ORDER BY id",
        ),
        (
            "events",
            "SELECT json_array(id,run_id,sequence,payload) FROM events ORDER BY id",
        ),
        (
            "messages",
            "SELECT json_array(id,run_id,order_id,created_at,position,payload) FROM messages ORDER BY id",
        ),
        (
            "workflow_attention",
            "SELECT json_array(run_id,run_revision,workflow_revision,actionable_count,valid_until) FROM workflow_attention ORDER BY run_id",
        ),
    ];
    let mut original = Vec::new();
    for (_, query) in tables {
        let rows: Vec<String> = sqlx::query_scalar(query).fetch_all(&pool).await.unwrap();
        original.push(rows);
    }
    let attention_before: Vec<(String, i64)> =
        sqlx::query_as("SELECT run_id,run_revision FROM workflow_attention ORDER BY run_id")
            .fetch_all(&pool)
            .await
            .unwrap();
    let expected_schema_version = legacy_fixture::copy(&source_path, &path, 17).await;
    pool.close().await;
    let store = Store::sqlite(&path).await.unwrap();
    assert_eq!(
        store
            .stored_run(&old_run.id)
            .await
            .unwrap()
            .record
            .spec
            .workflow
            .as_ref(),
        Some(&expected)
    );
    let create = intent(WorkflowOperationRequest::Create {
        request: CreateWorkflow {
            idempotency_key: "create".into(),
            definition: original_definition,
        },
    });
    assert!(matches!(
        store
            .prepare_workflow_operation(&f.spec.project_id, &create)
            .await,
        Err(Error::Conflict)
    ));
    let publication = intent(WorkflowOperationRequest::Publish {
        workflow_id: f.request.workflow_id.clone(),
        request: PublishWorkflow {
            base_revision: 1,
            idempotency_key: "publish-tool-fixture".into(),
            definition: f.definition.clone(),
        },
    });
    assert!(matches!(
        store
            .prepare_workflow_operation(&f.spec.project_id, &publication)
            .await,
        Err(Error::Conflict)
    ));
    assert!(matches!(
        store
            .prepare_workflow_operation(
                &f.spec.project_id,
                &intent(WorkflowOperationRequest::Start {
                    request: request.into()
                })
            )
            .await,
        Err(Error::Conflict)
    ));
    let draft_id = id();
    let first = save(&draft_id, 0, "lost-save", "");
    let first_receipt = store
        .save_workflow_draft(&f.spec.project_id, &first)
        .await
        .unwrap();
    let conflict_input = save(
        &draft_id,
        0,
        "lost-conflict",
        "Incoming preserved before restart",
    );
    let conflict_receipt = store
        .save_workflow_draft(&f.spec.project_id, &conflict_input)
        .await
        .unwrap();
    let pending_input = intent(WorkflowOperationRequest::Create {
        request: CreateWorkflow {
            idempotency_key: "after18".into(),
            definition: f.definition.clone(),
        },
    });
    let pending = store
        .prepare_workflow_operation(&f.spec.project_id, &pending_input)
        .await
        .unwrap();
    store.close().await;
    let pool = raw(&path).await;
    for (index, (table, query)) in tables.into_iter().enumerate() {
        let rows: Vec<String> = sqlx::query_scalar(query).fetch_all(&pool).await.unwrap();
        assert_eq!(rows, original[index], "Migration changed {table}");
    }
    let attention_after: Vec<(String, i64)> =
        sqlx::query_as("SELECT run_id,run_revision FROM workflow_attention ORDER BY run_id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(attention_before, attention_after);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT MAX(version) FROM schema_migrations")
            .fetch_one(&pool)
            .await
            .unwrap(),
        expected_schema_version
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM workflow_execution_clocks")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0,
        "Upgrading legacy history must never invent a clock origin"
    );
    pool.close().await;
    let store = Store::sqlite(&path).await.unwrap();
    assert_eq!(
        canonical_hash(
            &store
                .save_workflow_draft(&f.spec.project_id, &first)
                .await
                .unwrap()
        ),
        canonical_hash(&first_receipt)
    );
    assert_eq!(
        canonical_hash(
            &store
                .save_workflow_draft(&f.spec.project_id, &conflict_input)
                .await
                .unwrap()
        ),
        canonical_hash(&conflict_receipt)
    );
    assert_eq!(
        store
            .prepare_workflow_operation(&f.spec.project_id, &pending_input)
            .await
            .unwrap()
            .id,
        pending.id
    );
    let accepted = store
        .commit_workflow_library_operation(&f.spec.project_id, &pending.id)
        .await
        .unwrap();
    let frozen = canonical_hash(&accepted.receipt);
    store.close().await;
    let store = Store::sqlite(&path).await.unwrap();
    assert_eq!(
        canonical_hash(
            &store
                .commit_workflow_library_operation(&f.spec.project_id, &pending.id)
                .await
                .unwrap()
                .receipt
        ),
        frozen
    );
    store.close().await;
}
#[tokio::test]
async fn journal_corrupt_metadata_never_becomes_confirmed_head_or_accepted_effect() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("corruption.sqlite3");
    let store = Store::sqlite(&path).await.unwrap();
    let f = setup(&store).await;
    let draft_id = id();
    let initial_save = save(&draft_id, 0, "first", "");
    store
        .save_workflow_draft(&f.spec.project_id, &initial_save)
        .await
        .unwrap();
    let pool = raw(&path).await;
    let original: String = sqlx::query_scalar("SELECT payload FROM workflow_drafts WHERE id=$1")
        .bind(&draft_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE workflow_drafts SET revision=2 WHERE id=$1")
        .bind(&draft_id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        store.workflow_draft(&f.spec.project_id, &draft_id).await,
        Err(Error::Forbidden)
    ));
    assert!(
        store
            .list_workflow_drafts(&f.spec.project_id, 0, 100)
            .await
            .is_err()
    );
    sqlx::query("UPDATE workflow_drafts SET revision=1 WHERE id=$1")
        .bind(&draft_id)
        .execute(&pool)
        .await
        .unwrap();
    let mut changed: serde_json::Value = serde_json::from_str(&original).unwrap();
    changed["content"]["data"]["name"] = json!("corrupt content");
    changed["sha256"] = json!(canonical_hash(&changed["content"]));
    sqlx::query("UPDATE workflow_drafts SET payload=$1 WHERE id=$2")
        .bind(changed.to_string())
        .bind(&draft_id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        store.workflow_draft(&f.spec.project_id, &draft_id).await,
        Err(Error::Forbidden)
    ));
    sqlx::query("UPDATE workflow_drafts SET payload=$1 WHERE id=$2")
        .bind(original)
        .bind(&draft_id)
        .execute(&pool)
        .await
        .unwrap();
    // The immutable conflict receipt must keep its historical head after later saves.
    // A valid same-draft snapshot is insufficient if it belongs to another revision.
    let conflict_draft = id();
    store
        .save_workflow_draft(
            &f.spec.project_id,
            &save(&conflict_draft, 0, "head-v1", "Version one"),
        )
        .await
        .unwrap();
    store
        .save_workflow_draft(
            &f.spec.project_id,
            &save(&conflict_draft, 1, "head-v2", "Version two"),
        )
        .await
        .unwrap();
    let stale = save(
        &conflict_draft,
        1,
        "historical-conflict",
        "Incoming stale edit",
    );
    let original_conflict = store
        .save_workflow_draft(&f.spec.project_id, &stale)
        .await
        .unwrap();
    let WorkflowDraftSave::Conflict {
        draft: historical_head,
        snapshot: incoming,
    } = &original_conflict
    else {
        panic!()
    };
    assert_eq!(historical_head.revision, 2);
    assert_eq!(incoming.revision, 2);
    let later = store
        .save_workflow_draft(
            &f.spec.project_id,
            &save(&conflict_draft, 2, "head-v3", "Version three"),
        )
        .await
        .unwrap();
    let WorkflowDraftSave::Saved {
        snapshot: later_snapshot,
        ..
    } = later
    else {
        panic!()
    };
    assert_eq!(later_snapshot.revision, 3);
    assert_eq!(
        canonical_hash(
            &store
                .save_workflow_draft(&f.spec.project_id, &stale)
                .await
                .unwrap()
        ),
        canonical_hash(&original_conflict)
    );
    let original_receipt:String=sqlx::query_scalar("SELECT payload FROM workflow_draft_saves WHERE draft_id=$1 AND request_key='historical-conflict'").bind(&conflict_draft).fetch_one(&pool).await.unwrap();
    let mut corrupt: serde_json::Value = serde_json::from_str(&original_receipt).unwrap();
    corrupt["head_snapshot_id"] = json!(later_snapshot.id);
    sqlx::query("UPDATE workflow_draft_saves SET payload=$1 WHERE draft_id=$2 AND request_key='historical-conflict'").bind(corrupt.to_string()).bind(&conflict_draft).execute(&pool).await.unwrap();
    assert!(matches!(
        store.save_workflow_draft(&f.spec.project_id, &stale).await,
        Err(Error::Forbidden)
    ));
    assert_eq!(
        store
            .workflow_draft(&f.spec.project_id, &conflict_draft)
            .await
            .unwrap()
            .revision,
        3
    );
    sqlx::query("UPDATE workflow_draft_saves SET payload=$1 WHERE draft_id=$2 AND request_key='historical-conflict'").bind(original_receipt).bind(&conflict_draft).execute(&pool).await.unwrap();
    assert_eq!(
        canonical_hash(
            &store
                .save_workflow_draft(&f.spec.project_id, &stale)
                .await
                .unwrap()
        ),
        canonical_hash(&original_conflict)
    );
    let pending = store
        .prepare_workflow_operation(
            &f.spec.project_id,
            &intent(WorkflowOperationRequest::Create {
                request: CreateWorkflow {
                    idempotency_key: "receipt".into(),
                    definition: f.definition.clone(),
                },
            }),
        )
        .await
        .unwrap();
    let accepted = store
        .commit_workflow_library_operation(&f.spec.project_id, &pending.id)
        .await
        .unwrap();
    let original = serde_json::to_value(&accepted).unwrap();
    for mutation in [
        "foreign-project",
        "wrong-kind",
        "wrong-revision",
        "wrong-version",
    ] {
        let mut changed = original.clone();
        match mutation {
            "foreign-project" => changed["receipt"]["publication"]["project_id"] = json!("foreign"),
            "wrong-kind" => changed["receipt"]["kind"] = json!("published"),
            "wrong-revision" => changed["receipt"]["publication"]["revision"] = json!(2),
            "wrong-version" => {
                changed["receipt"]["publication"]["version_id"] = json!("foreign-version")
            }
            _ => unreachable!(),
        }
        sqlx::query("UPDATE workflow_operations SET payload=$1 WHERE id=$2")
            .bind(changed.to_string())
            .bind(&accepted.id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            store
                .workflow_operation(&f.spec.project_id, &accepted.id)
                .await
                .is_err(),
            "Corrupt receipt {mutation} was accepted"
        );
    }
    sqlx::query("UPDATE workflow_operations SET payload=$1 WHERE id=$2")
        .bind(original.to_string())
        .bind(&accepted.id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        store
            .workflow_operation(&f.spec.project_id, &accepted.id)
            .await
            .unwrap()
            .state,
        WorkflowOperationState::Accepted
    );
    let large_id = id();
    let mut large = save(&large_id, 0, "large-head", "");
    large.content.data = json!({"editor_text":"x".repeat(950_000)});
    store
        .save_workflow_draft(&f.spec.project_id, &large)
        .await
        .unwrap();
    for index in 0..3 {
        let input = save(
            &large_id,
            0,
            &format!("small-conflict-{index}"),
            "small incoming copy",
        );
        let response = store
            .save_workflow_draft(&f.spec.project_id, &input)
            .await
            .unwrap();
        let WorkflowDraftSave::Conflict { draft, .. } = response else {
            panic!()
        };
        assert!(draft.content.data["editor_text"].as_str().unwrap().len() > 900_000);
    }
    let receipt_sizes: Vec<i64> =
        sqlx::query_scalar("SELECT LENGTH(payload) FROM workflow_draft_saves WHERE draft_id=$1")
            .bind(&large_id)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(receipt_sizes.len(), 4);
    assert!(
        receipt_sizes.into_iter().all(|size| size < 1024),
        "CAS receipts duplicated the large confirmed head"
    );
    let bounded = tool_cases::tool_fixture(&store).await;
    let project = &bounded.spec.project_id;
    let mut request = start(&bounded, "bounded-profile-deleted");
    request.budgets.max_total_tokens *= 2;
    request.budgets.max_steps *= 2;
    request.budgets.max_seconds *= 2;
    request.budgets.max_output_tokens *= 2;
    let mut spec = bounded.spec.clone();
    spec.budgets = request.budgets.clone();
    let expected = trusted_binding(&store, &request, &spec).await;
    let prepare = intent(WorkflowOperationRequest::Start {
        request: request.clone().into(),
    });
    let pending = store
        .prepare_workflow_operation(project, &prepare)
        .await
        .unwrap();
    let accepted = store
        .commit_workflow_run_operation(project, &pending.id, spec.clone(), &expected)
        .await
        .unwrap();
    let frozen = canonical_hash(&accepted.receipt);
    sqlx::query("DELETE FROM providers WHERE id=$1")
        .bind(&bounded.definition.roles[0].provider_id)
        .execute(&pool)
        .await
        .unwrap();
    store
        .revoke_workflow(project, &request.workflow_id, 2, "revoke-profile-deleted")
        .await
        .unwrap();
    assert_eq!(
        canonical_hash(
            &store
                .workflow_operation(project, &pending.id)
                .await
                .unwrap()
                .receipt
        ),
        frozen
    );
    assert_eq!(
        canonical_hash(
            &store
                .prepare_workflow_operation(project, &prepare)
                .await
                .unwrap()
                .receipt
        ),
        frozen
    );
    assert_eq!(
        canonical_hash(
            &store
                .commit_workflow_run_operation(project, &pending.id, spec, &expected)
                .await
                .unwrap()
                .receipt
        ),
        frozen
    );
    assert_eq!(
        store
            .list_workflow_operations(project, 0, 100, false)
            .await
            .unwrap()
            .total,
        1
    );
    pool.close().await;
    store.close().await;
}
