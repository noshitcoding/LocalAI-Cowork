use cowork_contracts::*;
use cowork_domain::Error;
use cowork_storage::{
    Store, WriteKnowledgeAccessPolicy, WriteOwnerKnowledgeAccess, WriteProjectKnowledgeAccess,
};
use serde_json::json;
use uuid::Uuid;

async fn project(store: &Store, name: &str) -> (Project, Order) {
    let project = store
        .create_project(CreateProject { name: name.into() })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Chat".into(),
        })
        .await
        .unwrap();
    (project, order)
}
fn entry(title: &str, scope: Option<MemoryScope>) -> WriteKnowledge {
    let kind = if scope.is_some() {
        KnowledgeKind::Memory
    } else {
        KnowledgeKind::Knowledge
    };
    WriteKnowledge {
        location: None,
        memory: scope.map(|scope| MemoryMetadata {
            scope,
            profile_key: None,
            ..Default::default()
        }),
        id: None,
        expected_revision: 0,
        kind,
        title: title.into(),
        content: format!("Evidence for {title}"),
        tags: vec![],
        provenance: None,
    }
}
fn spec(project: &Project, order: &Order) -> RunSpec {
    serde_json::from_value(json!({"project_id":project.id,"order_id":order.id,"prompt":"Inspect evidence","provider":{"id":"fixture","name":"Fixture","kind":"test","endpoint":"","model":"test","revision":1,"context_tokens":32768,"credential_required":false,"credential_ready":true},"access_mode":"read_only","executor":"local-v2","policy_revision":1,"allowed_tools":[],"budgets":Budgets::default(),"folders":[],"mcp_tools":[],"environment":null,"file_action":null,"workflow":null,"output":null,"model_binding":null,"agent_config":null})).unwrap()
}
async fn preview(store: &Store, spec: &RunSpec) -> String {
    serde_json::to_string(&store.preview_model_context(spec).await.unwrap()).unwrap()
}

#[tokio::test]
async fn local_switches_narrow_project_and_personal_knowledge() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (source, _) = project(&store, "Source").await;
    let (target, chat) = project(&store, "Target").await;
    let project_doc = store
        .productivity_write_knowledge(&target.id, entry("PROJECT_DOC", None))
        .await
        .unwrap();
    let personal = store
        .productivity_write_knowledge(&source.id, entry("PERSONAL_NOTE", Some(MemoryScope::User)))
        .await
        .unwrap();
    let run = spec(&target, &chat);
    let text = preview(&store, &run).await;
    assert!(text.contains("PROJECT_DOC") && text.contains("PERSONAL_NOTE"));

    let defaults = store.project_knowledge_access(&target.id).await.unwrap();
    assert_eq!(defaults.revision, 0);
    assert!(
        defaults.project_knowledge && defaults.personal_knowledge && defaults.personal_available
    );
    let write = WriteProjectKnowledgeAccess {
        expected_revision: 0,
        project_knowledge: false,
        personal_knowledge: true,
    };
    let saved = store
        .write_project_knowledge_access(&target.id, &write)
        .await
        .unwrap();
    assert_eq!(saved.revision, 1);
    // A retry after a lost acknowledgement observes the same result.
    assert_eq!(
        store
            .write_project_knowledge_access(&target.id, &write)
            .await
            .unwrap(),
        saved
    );
    let text = preview(&store, &run).await;
    assert!(!text.contains("PROJECT_DOC") && text.contains("PERSONAL_NOTE"));
    // Other projects keep their own defaults.
    let (other, other_chat) = project(&store, "Other").await;
    store
        .productivity_write_knowledge(&other.id, entry("OTHER_DOC", None))
        .await
        .unwrap();
    assert!(
        preview(&store, &spec(&other, &other_chat))
            .await
            .contains("OTHER_DOC")
    );

    // An explicitly selected entry from a disabled knowledge base is refused.
    let mut explicit = run.clone();
    explicit.agent_config = Some(EffectiveAgentConfig {
        memory_ids: Some(vec![project_doc.id.clone()]),
        ..Default::default()
    });
    assert!(matches!(
        store.preview_model_context(&explicit).await,
        Err(Error::Validation(message)) if message == "knowledge_base_disabled"
    ));

    assert!(matches!(
        store
            .write_project_knowledge_access(
                &target.id,
                &WriteProjectKnowledgeAccess {
                    expected_revision: 0,
                    project_knowledge: true,
                    personal_knowledge: false,
                },
            )
            .await,
        Err(Error::Conflict)
    ));
    store
        .write_project_knowledge_access(
            &target.id,
            &WriteProjectKnowledgeAccess {
                expected_revision: 1,
                project_knowledge: true,
                personal_knowledge: false,
            },
        )
        .await
        .unwrap();
    let text = preview(&store, &run).await;
    assert!(text.contains("PROJECT_DOC") && !text.contains("PERSONAL_NOTE"));

    // The owner-wide switch disables the personal knowledge base everywhere.
    store
        .write_project_knowledge_access(
            &target.id,
            &WriteProjectKnowledgeAccess {
                expected_revision: 2,
                project_knowledge: true,
                personal_knowledge: true,
            },
        )
        .await
        .unwrap();
    assert!(preview(&store, &run).await.contains("PERSONAL_NOTE"));
    let owner = store
        .write_owner_knowledge_access(&WriteOwnerKnowledgeAccess {
            expected_revision: 0,
            personal_knowledge: false,
        })
        .await
        .unwrap();
    assert_eq!((owner.revision, owner.personal_knowledge), (1, false));
    assert!(!preview(&store, &run).await.contains("PERSONAL_NOTE"));
    assert!(
        !preview(&store, &spec(&source, &chat_in(&store, &source).await))
            .await
            .contains("PERSONAL_NOTE")
    );
    let view = store.project_knowledge_access(&target.id).await.unwrap();
    assert!(view.personal_knowledge && !view.personal_available);
    let mut personal_explicit = run;
    personal_explicit.agent_config = Some(EffectiveAgentConfig {
        memory_ids: Some(vec![personal.id]),
        ..Default::default()
    });
    assert!(matches!(
        store.preview_model_context(&personal_explicit).await,
        Err(Error::Validation(message)) if message == "knowledge_base_disabled"
    ));
    assert!(matches!(
        store.project_knowledge_access("missing-project").await,
        Err(Error::NotFound)
    ));
}
async fn chat_in(store: &Store, project: &Project) -> Order {
    store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Second chat".into(),
        })
        .await
        .unwrap()
}

async fn actors(store: &Store) -> (cowork_identity::Principal, cowork_identity::Principal) {
    let identity = store.identity();
    let bootstrap = "b".repeat(64);
    identity
        .initialize_credentials(&bootstrap, &"s".repeat(64))
        .await
        .unwrap();
    let (_, admin) = identity
        .bootstrap(
            &bootstrap,
            "knowledge-admin",
            "Knowledge admin",
            "SyntheticPassword-2026!",
            "Knowledge fixture",
        )
        .await
        .unwrap();
    identity
        .register_local_identity(
            "knowledge-user",
            "Knowledge user",
            "OtherSyntheticPassword-2026!",
        )
        .await
        .unwrap();
    let user = identity
        .login("knowledge-user", "OtherSyntheticPassword-2026!", 3_600_000)
        .await
        .unwrap();
    (
        identity.authenticate_session(&admin.token).await.unwrap(),
        identity.authenticate_session(&user.token).await.unwrap(),
    )
}
fn policy(expected: i64, users: Vec<Uuid>, projects: Vec<Uuid>) -> WriteKnowledgeAccessPolicy {
    WriteKnowledgeAccessPolicy {
        expected_revision: expected,
        private_projects: false,
        team_projects: false,
        enabled_users: users,
        enabled_projects: projects,
        personal_team_runs: None,
        enabled_personal_team_users: None,
    }
}

#[tokio::test]
async fn server_policy_is_admin_only_and_supports_exceptions() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (admin, user) = actors(&store).await;
    assert!(matches!(
        store.knowledge_access_directory(&user).await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        store
            .write_knowledge_access_policy(&user, &policy(0, vec![user.identity_id()], vec![]))
            .await,
        Err(Error::Forbidden)
    ));
    let directory = store.knowledge_access_directory(&admin).await.unwrap();
    assert_eq!(directory.policy.revision, 0);
    assert!(!directory.policy.private_projects && !directory.policy.team_projects);
    assert!(
        directory
            .users
            .iter()
            .any(|u| u.id == user.identity_id() && !u.private_projects)
    );
    // Server default: disabled for everyone.
    assert!(
        !store
            .actor_knowledge_access(&user)
            .await
            .unwrap()
            .private_projects
    );
    let unknown = Uuid::new_v4();
    assert!(
        !store
            .team_project_knowledge_access(unknown)
            .await
            .unwrap()
            .project_knowledge
    );

    // Global switch off, single user enabled as an exception.
    let saved = store
        .write_knowledge_access_policy(&admin, &policy(0, vec![user.identity_id()], vec![]))
        .await
        .unwrap();
    assert_eq!(saved.revision, 1);
    assert_eq!(saved.updated_by, Some(admin.identity_id()));
    assert!(
        store
            .actor_knowledge_access(&user)
            .await
            .unwrap()
            .private_projects
    );
    assert!(
        !store
            .actor_knowledge_access(&admin)
            .await
            .unwrap()
            .private_projects
    );
    assert!(matches!(
        store
            .write_knowledge_access_policy(&admin, &policy(0, vec![], vec![]))
            .await,
        Err(Error::Conflict)
    ));
    assert!(matches!(
        store
            .write_knowledge_access_policy(
                &admin,
                &policy(1, vec![user.identity_id(), user.identity_id()], vec![])
            )
            .await,
        Err(Error::Validation(_))
    ));
    assert!(matches!(
        store
            .write_knowledge_access_policy(&admin, &policy(1, vec![Uuid::new_v4()], vec![]))
            .await,
        Err(Error::Validation(_))
    ));
    assert!(matches!(
        store
            .write_knowledge_access_policy(&admin, &policy(1, vec![], vec![unknown]))
            .await,
        Err(Error::Validation(_))
    ));

    // Global switches enable everyone and every team project.
    let global = store
        .write_knowledge_access_policy(
            &admin,
            &WriteKnowledgeAccessPolicy {
                expected_revision: 1,
                private_projects: true,
                team_projects: true,
                enabled_users: vec![],
                enabled_projects: vec![],
                personal_team_runs: None,
                enabled_personal_team_users: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(global.revision, 2);
    // Neither global switch enables the personal knowledge base in team runs.
    assert!(!global.personal_team_runs);
    assert!(
        !store
            .actor_knowledge_access(&admin)
            .await
            .unwrap()
            .personal_team_runs
    );
    assert!(
        store
            .actor_knowledge_access(&admin)
            .await
            .unwrap()
            .private_projects
    );
    assert!(
        store
            .team_project_knowledge_access(unknown)
            .await
            .unwrap()
            .project_knowledge
    );
}

struct Keys;
impl cowork_secrets::KeyStore for Keys {
    fn load(
        &self,
        _: &cowork_secrets::KeyId,
    ) -> cowork_secrets::Result<zeroize::Zeroizing<[u8; 32]>> {
        Ok(zeroize::Zeroizing::new([43; 32]))
    }
}

#[tokio::test]
async fn team_runs_send_project_knowledge_only_when_enabled() {
    use cowork_storage::team_service::{LiteLlmExecutor, TeamService};
    use std::sync::Arc;
    let temporary = tempfile::tempdir().unwrap();
    let store = Store::sqlite(&temporary.path().join("team.sqlite"))
        .await
        .unwrap();
    let (admin, _) = actors(&store).await;
    let service = TeamService::new(
        store.clone(),
        Arc::new(Keys),
        cowork_secrets::KeyId {
            instance: Uuid::new_v4(),
            key: Uuid::new_v4(),
        },
        Arc::new(LiteLlmExecutor::new(None).unwrap()),
    );
    let team = service.teams(&admin).await.unwrap()[0].id;
    let project = service
        .create_project(
            &admin,
            team,
            CreateTeamProject {
                name: "Knowledge project".into(),
                idempotency_key: "knowledge-project".into(),
            },
        )
        .await
        .unwrap()
        .id;
    service
        .configure_provider(
            &admin,
            team,
            ConfigureTeamProvider {
                expected_revision: 0,
                base_url: "http://127.0.0.1:9/v1".into(),
                model: "fixture".into(),
                api_key: "synthetic-provider-key".into(),
            },
        )
        .await
        .unwrap();
    let order = service
        .create_order(
            &admin,
            team,
            project,
            CreateTeamOrder {
                title: "Order".into(),
                prompt: "Use the evidence".into(),
                idempotency_key: "knowledge-order".into(),
            },
        )
        .await
        .unwrap();
    let document = service
        .save_document(
            &admin,
            team,
            project,
            SaveTeamDocument {
                provenance: None,
                id: None,
                expected_revision: 0,
                kind: TeamDocumentKind::Knowledge,
                name: "Team evidence".into(),
                content: "Evidence".into(),
                archived: false,
            },
        )
        .await
        .unwrap();
    let run = |key: &str, knowledge: Vec<Uuid>| CreateTeamRun {
        order_id: order.id,
        prompt: "Use the evidence".into(),
        idempotency_key: key.into(),
        plan: TeamRunPlan {
            knowledge,
            ..Default::default()
        },
    };
    assert!(matches!(
        service
            .create_run(&admin, team, run("blocked", vec![document.id]))
            .await,
        Err(Error::Validation(message)) if message == "knowledge_base_disabled"
    ));
    // Runs without knowledge are unaffected by the policy.
    service
        .create_run(&admin, team, run("plain", vec![]))
        .await
        .unwrap();
    assert!(
        !store
            .team_project_knowledge_access(project)
            .await
            .unwrap()
            .project_knowledge
    );
    store
        .write_knowledge_access_policy(&admin, &policy(0, vec![], vec![project]))
        .await
        .unwrap();
    assert!(
        store
            .team_project_knowledge_access(project)
            .await
            .unwrap()
            .project_knowledge
    );
    service
        .create_run(&admin, team, run("allowed", vec![document.id]))
        .await
        .unwrap();
    let directory = store.knowledge_access_directory(&admin).await.unwrap();
    let listed = directory.projects.iter().find(|p| p.id == project).unwrap();
    assert_eq!(listed.team_id, team);
    assert!(listed.team_projects && listed.name == "Knowledge project");
}

async fn memory(
    store: &Store,
    actor: &cowork_identity::Principal,
    project: Option<Uuid>,
    key: &str,
    content: &str,
    enabled: bool,
) -> Uuid {
    let id = Uuid::new_v4();
    store
        .actor_context_mutate(
            actor,
            "memories",
            id,
            &json!({"request_id":Uuid::new_v4(),"expected_revision":0,"value":{"project_id":project,"key":key,"content":content,"category":"knowledge","confidence":null,"enabled":enabled}}),
            false,
        )
        .await
        .unwrap();
    id
}
type Captured = std::sync::Arc<
    std::sync::Mutex<
        Vec<(
            Uuid,
            Vec<cowork_storage::knowledge_access::TeamPersonalKnowledge>,
        )>,
    >,
>;
/// Receives exactly what the worker hands an executor after claiming a run.
struct Capture(Captured);
impl cowork_storage::team_service::TeamExecutor for Capture {
    fn supports(&self, plan: &TeamRunPlan) -> bool {
        plan.workflow.is_none() && plan.tools.is_empty()
    }
    fn execute(
        &self,
        execution: cowork_storage::team_service::TeamExecution,
    ) -> futures_util::future::BoxFuture<
        '_,
        cowork_domain::Result<cowork_storage::team_service::TeamExecutionResult>,
    > {
        self.0
            .lock()
            .unwrap()
            .push((execution.authority.run_id(), execution.personal_knowledge));
        Box::pin(async {
            Ok(cowork_storage::team_service::TeamExecutionResult {
                response: "Done".into(),
            })
        })
    }
}

#[tokio::test]
async fn team_runs_add_personal_knowledge_only_with_its_own_approval() {
    use cowork_storage::knowledge_access::team_personal_context;
    use cowork_storage::team_service::TeamService;
    use std::sync::Arc;
    let temporary = tempfile::tempdir().unwrap();
    let store = Store::sqlite(&temporary.path().join("personal.sqlite"))
        .await
        .unwrap();
    let (admin, user) = actors(&store).await;
    let captured = Captured::default();
    let service = TeamService::new(
        store.clone(),
        Arc::new(Keys),
        cowork_secrets::KeyId {
            instance: Uuid::new_v4(),
            key: Uuid::new_v4(),
        },
        Arc::new(Capture(captured.clone())),
    );
    let team = service.teams(&admin).await.unwrap()[0].id;
    let project = service
        .create_project(
            &admin,
            team,
            CreateTeamProject {
                name: "Release project".into(),
                idempotency_key: "personal-project".into(),
            },
        )
        .await
        .unwrap()
        .id;
    service
        .configure_provider(
            &admin,
            team,
            ConfigureTeamProvider {
                expected_revision: 0,
                base_url: "http://127.0.0.1:9/v1".into(),
                model: "fixture".into(),
                api_key: "synthetic-provider-key".into(),
            },
        )
        .await
        .unwrap();
    let order = service
        .create_order(
            &admin,
            team,
            project,
            CreateTeamOrder {
                title: "Release notes".into(),
                prompt: "Write the release notes".into(),
                idempotency_key: "personal-order".into(),
            },
        )
        .await
        .unwrap();

    // The run creator's account-wide memory, and entries that must stay out:
    // disabled, project-scoped and another user's account-wide memory.
    let note = memory(
        &store,
        &admin,
        None,
        "release_style",
        "PERSONAL_RELEASE_STYLE",
        true,
    )
    .await;
    memory(&store, &admin, None, "paused", "PERSONAL_DISABLED", false).await;
    let private = store
        .android_personal_create_project(
            &admin,
            json!({"privacy":"private_local","team_id":null,"name":"Private","description":"","policy":{}}),
        )
        .await
        .unwrap();
    let private = Uuid::parse_str(private["id"].as_str().unwrap()).unwrap();
    memory(
        &store,
        &admin,
        Some(private),
        "scoped",
        "PERSONAL_PROJECT_SCOPED",
        true,
    )
    .await;
    memory(&store, &user, None, "other", "OTHER_USER_NOTE", true).await;

    let run = |key: &str, personal_knowledge: bool| CreateTeamRun {
        order_id: order.id,
        prompt: "Use my release style".into(),
        idempotency_key: key.into(),
        plan: TeamRunPlan {
            personal_knowledge,
            ..Default::default()
        },
    };
    let refused = |result: Result<TeamRun, Error>| matches!(result, Err(Error::Validation(message)) if message == "knowledge_base_disabled");
    // Server default: refused.
    assert!(refused(
        service.create_run(&admin, team, run("default", true)).await
    ));
    assert!(
        !store
            .actor_knowledge_access(&admin)
            .await
            .unwrap()
            .personal_team_runs
    );
    // Team project knowledge and private projects do not imply this switch.
    let write = |expected: i64, team_projects: bool, personal: Option<Vec<Uuid>>| {
        WriteKnowledgeAccessPolicy {
            expected_revision: expected,
            private_projects: true,
            team_projects,
            enabled_users: vec![],
            enabled_projects: vec![],
            personal_team_runs: None,
            enabled_personal_team_users: personal,
        }
    };
    let wide = store
        .write_knowledge_access_policy(&admin, &write(0, true, None))
        .await
        .unwrap();
    assert!(!wide.personal_team_runs && wide.enabled_personal_team_users.is_empty());
    assert!(refused(
        service
            .create_run(&admin, team, run("team-projects", true))
            .await
    ));
    // An exception for another user does not cover the run creator.
    store
        .write_knowledge_access_policy(&admin, &write(1, true, Some(vec![user.identity_id()])))
        .await
        .unwrap();
    assert!(
        store
            .actor_knowledge_access(&user)
            .await
            .unwrap()
            .personal_team_runs
    );
    assert!(refused(
        service
            .create_run(&admin, team, run("other-user", true))
            .await
    ));
    // Runs that do not ask for it are unaffected, and their request JSON (hash
    // input) is unchanged because the field is omitted while false.
    assert!(
        !serde_json::to_string(&run("plain", false).plan)
            .unwrap()
            .contains("personal_knowledge")
    );
    let plain = service
        .create_run(&admin, team, run("plain", false))
        .await
        .unwrap();

    // Exception for the run creator.
    store
        .write_knowledge_access_policy(
            &admin,
            &write(2, true, Some(vec![user.identity_id(), admin.identity_id()])),
        )
        .await
        .unwrap();
    // Clients that omit the new fields keep the stored values.
    let kept = store
        .write_knowledge_access_policy(&admin, &write(3, false, None))
        .await
        .unwrap();
    assert!(
        kept.enabled_personal_team_users
            .contains(&admin.identity_id())
            && !kept.team_projects
    );
    assert!(
        store
            .actor_knowledge_access(&admin)
            .await
            .unwrap()
            .personal_team_runs
    );
    let directory = store.knowledge_access_directory(&admin).await.unwrap();
    assert!(
        directory
            .users
            .iter()
            .any(|u| u.id == admin.identity_id() && u.personal_team_runs)
    );
    let admitted = service
        .create_run(&admin, team, run("allowed", true))
        .await
        .unwrap();

    // Frozen at admission: later edits and a revoked approval change nothing, and an
    // exact retry returns the original receipt.
    store
        .actor_context_mutate(
            &admin,
            "memories",
            note,
            &json!({"request_id":Uuid::new_v4(),"expected_revision":1,"value":{"project_id":null,"key":"release_style","content":"PERSONAL_CHANGED_LATER","category":"knowledge","confidence":null,"enabled":true}}),
            false,
        )
        .await
        .unwrap();
    store
        .write_knowledge_access_policy(&admin, &write(4, false, Some(vec![])))
        .await
        .unwrap();
    assert!(refused(
        service.create_run(&admin, team, run("revoked", true)).await
    ));
    assert_eq!(
        service
            .create_run(&admin, team, run("allowed", true))
            .await
            .unwrap()
            .id,
        admitted.id
    );

    for _ in 0..4 {
        if !service.poll_once().await.unwrap() {
            break;
        }
    }
    for id in [admitted.id, plain.id] {
        let finished = service.run(&admin, team, id).await.unwrap();
        assert_eq!(
            finished.status,
            TeamRunStatus::Completed,
            "{:?}",
            finished.error
        );
    }
    // The claimed execution carries exactly the frozen entry, only for the run that asked.
    let captured = captured.lock().unwrap().clone();
    assert_eq!(captured.len(), 2);
    let frozen = &captured
        .iter()
        .find(|(id, _)| *id == admitted.id)
        .unwrap()
        .1;
    assert!(
        captured
            .iter()
            .find(|(id, _)| *id == plain.id)
            .unwrap()
            .1
            .is_empty()
    );
    assert_eq!(frozen.len(), 1);
    assert_eq!((frozen[0].id, frozen[0].revision), (note, 1));
    assert_eq!(
        (frozen[0].key.as_str(), frozen[0].content.as_str()),
        ("release_style", "PERSONAL_RELEASE_STYLE")
    );
    // The model context marks it as the run creator's personal knowledge.
    let text = team_personal_context(frozen);
    assert!(text.contains(&format!(
        "<untrusted_personal_context id=\"{note}\" revision=\"1\" key=\"release_style\""
    )));
    assert!(text.contains("personal knowledge of the person who started this run"));
    assert!(text.contains("PERSONAL_RELEASE_STYLE"));
    for absent in [
        "PERSONAL_CHANGED_LATER",
        "PERSONAL_DISABLED",
        "PERSONAL_PROJECT_SCOPED",
        "OTHER_USER_NOTE",
    ] {
        assert!(!text.contains(absent), "{absent}");
    }
    assert!(team_personal_context(&[]).is_empty());
}
