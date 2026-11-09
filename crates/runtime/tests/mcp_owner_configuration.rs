#[path = "../../storage/tests/support/native_keys.rs"]
mod native_keys;
#[path = "../../storage/tests/support/mcp.rs"]
mod support;
use cowork_contracts::*;
use cowork_runtime::Runtime;
use cowork_secrets::{KeyId, KeyStore};
use cowork_storage::{NativeMcpFactory, Store, id};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use uuid::Uuid;
use zeroize::Zeroizing;
struct Keys(KeyId);
impl KeyStore for Keys {
    fn load(&self, id: &KeyId) -> cowork_secrets::Result<Zeroizing<[u8; 32]>> {
        if id != &self.0 {
            return Err(cowork_secrets::Error::Missing);
        }
        Ok(Zeroizing::new([7; 32]))
    }
}
fn keys() -> Arc<Keys> {
    Arc::new(Keys(KeyId {
        instance: Uuid::new_v4(),
        key: Uuid::new_v4(),
    }))
}

#[tokio::test]
async fn server_owner_keys_are_encrypted_revision_scoped_and_never_substitute_for_native_keys() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "owner".into(),
        })
        .await
        .unwrap();
    let other = store
        .create_project(CreateProject {
            name: "other".into(),
        })
        .await
        .unwrap();
    let keys = keys();
    let runtime = Runtime::new(store.clone(), vec![], false)
        .unwrap()
        .with_personal_provider_keys(keys.clone(), keys.0);
    let server = id();
    let endpoint = "https://tools.example/mcp";
    let secret = "synthetic-secret-value";
    let (reference, revision) = runtime
        .save_server_mcp_bearer(&project.id, &server, endpoint, secret.into(), 0)
        .await
        .unwrap();
    assert_eq!(revision, 1);
    assert_eq!(
        runtime
            .save_server_mcp_bearer(&project.id, &server, endpoint, secret.into(), 0)
            .await
            .unwrap(),
        (reference.clone(), revision)
    );
    assert!(
        runtime
            .save_server_mcp_bearer(&project.id, &server, endpoint, "different-secret".into(), 0)
            .await
            .is_err()
    );
    let record = store.mcp_credential(&project.id, &reference).await.unwrap();
    assert!(!serde_json::to_string(&record).unwrap().contains(secret));
    assert_eq!(
        runtime
            .native_mcp_bearer(&project.id, &server, endpoint, &reference, revision)
            .await
            .unwrap()
            .as_str(),
        secret
    );
    assert!(
        runtime
            .native_mcp_bearer(&other.id, &server, endpoint, &reference, revision)
            .await
            .is_err()
    );
    assert!(
        runtime
            .native_mcp_bearer(
                &project.id,
                &server,
                "https://other.example/mcp",
                &reference,
                revision
            )
            .await
            .is_err()
    );
    let native = runtime
        .clone()
        .with_native_key_provider(Arc::new(native_keys::FakeProvider::default()));
    assert!(!native.server_owner_mcp_available());
    assert!(
        native
            .save_server_mcp_bearer(&project.id, &server, endpoint, "not-native".into(), 0)
            .await
            .is_err()
    );
    assert!(
        native
            .save_native_mcp_bearer(&project.id, &id(), endpoint, "not-server".into(), 0)
            .await
            .is_err()
    );
}
#[tokio::test]
async fn stdio_environment_roundtrips_without_plaintext_factory_fields_or_secret_fingerprints() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let project = store
        .create_project(CreateProject { name: "env".into() })
        .await
        .unwrap();
    let keys = keys();
    let runtime = Runtime::new(store.clone(), vec![], false)
        .unwrap()
        .with_personal_provider_keys(keys.clone(), keys.0);
    let server = id();
    let endpoint = format!("http://127.0.0.1:43821/native-mcp/{server}");
    let environment = BTreeMap::from([
        ("API_KEY".into(), "private value with spaces/ä".into()),
        ("EMPTY".into(), "".into()),
    ]);
    let versions = runtime
        .save_mcp_stdio_environment(&project.id, &server, &endpoint, &environment, false)
        .await
        .unwrap();
    assert_eq!(
        versions,
        runtime
            .save_mcp_stdio_environment(&project.id, &server, &endpoint, &environment, false)
            .await
            .unwrap()
    );
    let factory = NativeMcpFactory {
        server_id: server,
        project_id: project.id,
        program: "node".into(),
        arguments: vec![],
        cwd: Some(".".into()),
        environment: versions,
        native_only: false,
        endpoint,
        credential_reference: reference_placeholder(),
        credential_revision: 1,
        revision: 1,
        enabled: true,
    };
    assert!(
        !serde_json::to_string(&factory)
            .unwrap()
            .contains("private value")
    );
    assert_eq!(
        runtime.mcp_stdio_environment(&factory).await.unwrap(),
        environment
    );
    let mut stale = factory.clone();
    stale.environment.insert("API_KEY".into(), 99);
    assert!(runtime.mcp_stdio_environment(&stale).await.is_err());
}
fn reference_placeholder() -> String {
    "server-mcp:test".into()
}

async fn owner_admission_fixture() -> (Runtime, Arc<Keys>, Project, McpServer, ProviderProfile) {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Owner admission".into(),
        })
        .await
        .unwrap();
    let profile = ProviderProfile {
        id: "owner-admission".into(),
        name: "Test".into(),
        kind: ProviderKind::Test,
        endpoint: String::new(),
        model: "test".into(),
        revision: 1,
        context_tokens: 32768,
        credential_required: false,
        credential_ready: true,
        generation: Default::default(),
        capabilities: Default::default(),
    };
    store.insert_provider(&profile).await.unwrap();
    let keys = keys();
    let runtime = Runtime::new(
        store,
        vec![cowork_runtime::ConfiguredProvider {
            profile: profile.clone(),
            credential_env: None,
        }],
        true,
    )
    .unwrap()
    .with_personal_provider_keys(keys.clone(), keys.0);
    let server_id = id();
    let endpoint = "https://tools.example/mcp";
    let (reference, revision) = runtime
        .save_server_mcp_bearer(
            &project.id,
            &server_id,
            endpoint,
            "owner-admission-secret".into(),
            0,
        )
        .await
        .unwrap();
    let server = runtime
        .store
        .configure_mcp_server(
            &project.id,
            &server_id,
            0,
            "configure",
            &McpServerConfiguration {
                name: "Owner endpoint".into(),
                endpoint: endpoint.into(),
                credential_reference: Some(reference),
                credential_revision: revision,
                catalog: vec![support::definition()],
                selected_tools: vec!["append_note".into()],
                enabled: true,
            },
        )
        .await
        .unwrap()
        .public_view();
    (runtime, keys, project, server, profile)
}
async fn single_request(
    runtime: &Runtime,
    project: &Project,
    server: &McpServer,
    profile: &ProviderProfile,
) -> CreateRun {
    let order = runtime
        .store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Owner admission".into(),
        })
        .await
        .unwrap();
    CreateRun {
        agent_config: None,
        model_binding: None,
        output: None,
        native_tools: vec![],
        environment: None,
        mcp_tools: vec![McpToolSelection {
            server_id: server.id.clone(),
            server_revision: server.revision,
            tool_name: "append_note".into(),
        }],
        budgets: Budgets::default(),
        order_id: order.id,
        base_revision: order.revision,
        idempotency_key: id(),
        prompt: "Use the selected owner tool.".into(),
        provider_id: profile.id.clone(),
        provider_revision: profile.revision,
        access_mode: AccessMode::ConfirmMutations,
        folders: vec![],
    }
}
async fn workflow_request(
    runtime: &Runtime,
    project: &Project,
    server: &McpServer,
    profile: &ProviderProfile,
) -> cowork_workflow_contracts::WorkflowRunRequest {
    use cowork_domain::workflows::*;
    let single = single_request(runtime, project, server, profile).await;
    let binding = runtime
        .store
        .select_mcp_tools(&project.id, &single.mcp_tools)
        .await
        .unwrap()
        .remove(0);
    let rights = WorkflowRights {
        current_workspace: false,
        tools: [binding.alias].into_iter().collect(),
        folders: Default::default(),
    };
    let budgets = Budgets::default();
    let definition = WorkflowDefinition {
        documents: Default::default(),
        iterations: Default::default(),
        subflows: Default::default(),
        controls: Default::default(),
        gates: Default::default(),
        dataflow: None,
        format_version: 1,
        name: "Owner MCP workflow".into(),
        mode: WorkflowMode::Sequential,
        failure: WorkflowFailure::Stop,
        rights: rights.clone(),
        budgets: budgets.clone(),
        roles: vec![WorkflowRole {
            id: "owner".into(),
            instructions: "Use the explicitly selected tool.".into(),
            provider_id: profile.id.clone(),
            provider_revision: profile.revision,
            rights: rights.clone(),
            budgets: budgets.clone(),
            delegate_to: Default::default(),
            agent_config: None,
            model_binding: None,
            output: None,
        }],
        steps: vec![WorkflowStep {
            execution: None,
            zero_context: false,
            id: "selected".into(),
            role_id: "owner".into(),
            goal: "Use the selected tool.".into(),
            dependencies: vec![],
            rights,
            budgets: budgets.clone(),
        }],
    };
    let library = runtime
        .store
        .create_workflow(&project.id, &id(), &definition)
        .await
        .unwrap();
    cowork_workflow_contracts::WorkflowRunRequest {
        input_source: None,
        inputs: None,
        agent_config: None,
        native_tools: vec![],
        folders: vec![],
        mcp_tools: single.mcp_tools,
        environment: None,
        kind: cowork_workflow_contracts::WorkflowRunKind::Workflow,
        order_id: single.order_id,
        base_revision: single.base_revision,
        idempotency_key: id(),
        prompt: single.prompt,
        workflow_id: library.id,
        version_id: library.versions[0].id.clone(),
        access_mode: single.access_mode,
        budgets,
        expected_binding: None,
    }
}
#[tokio::test]
async fn public_single_and_workflow_admission_accept_explicit_server_owner_custody() {
    let (runtime, _keys, project, server, profile) = owner_admission_fixture().await;
    let single = single_request(&runtime, &project, &server, &profile).await;
    let flight = runtime.preflight(&single).await.unwrap();
    assert_eq!(flight.mcp_tools.len(), 1);
    let accepted = runtime.create_run(&single).await.unwrap();
    assert_eq!(accepted.spec.mcp_tools, flight.mcp_tools);
    let mut workflow = workflow_request(&runtime, &project, &server, &profile).await;
    let flight = runtime.preflight_workflow(&workflow).await.unwrap();
    workflow.expected_binding = Some(flight.workflow.clone());
    let accepted = runtime
        .create_product_workflow_run(&workflow)
        .await
        .unwrap();
    assert_eq!(accepted.spec.workflow, Some(flight.workflow));
    assert_eq!(accepted.spec.mcp_tools.len(), 1);
}
#[tokio::test]
async fn public_owner_admission_rejects_native_factory_and_native_secret_custody_on_server() {
    let (runtime, keys, project, mut server, profile) = owner_admission_fixture().await;
    let single = single_request(&runtime, &project, &server, &profile).await;
    let workflow = workflow_request(&runtime, &project, &server, &profile).await;
    let factory = NativeMcpFactory {
        server_id: server.id.clone(),
        project_id: project.id.clone(),
        program: "node".into(),
        arguments: vec![],
        cwd: None,
        environment: Default::default(),
        native_only: true,
        endpoint: server.configuration.endpoint.clone(),
        credential_reference: server.configuration.credential_reference.clone().unwrap(),
        credential_revision: 1,
        revision: 1,
        enabled: true,
    };
    runtime
        .store
        .put_native_mcp_factory(&factory, 0)
        .await
        .unwrap();
    assert!(runtime.preflight(&single).await.is_err());
    assert!(runtime.create_run(&single).await.is_err());
    assert!(runtime.preflight_workflow(&workflow).await.is_err());
    assert!(
        runtime
            .create_product_workflow_run(&workflow)
            .await
            .is_err()
    );
    assert!(
        runtime
            .store
            .runs(&single.order_id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        runtime
            .store
            .runs(&workflow.order_id)
            .await
            .unwrap()
            .is_empty()
    );

    // Even sharing the same synthetic key bytes cannot turn native custody into
    // server custody. The namespace still requires the native key provider.
    let scope = cowork_secrets::CredentialScope {
        project_id: project.id.clone(),
        server_id: server.id.clone(),
        reference: format!("owner-mcp:{}", server.id),
        revision: 1,
        endpoint: server.configuration.endpoint.clone(),
    };
    let sealed =
        cowork_secrets::seal_bearer(keys.as_ref(), keys.0, &scope, "native-only-secret").unwrap();
    runtime
        .store
        .put_mcp_credential(&scope, &sealed, 0)
        .await
        .unwrap();
    server.configuration.credential_reference = Some(scope.reference);
    server.configuration.credential_revision = 1;
    server = runtime
        .store
        .configure_mcp_server(
            &project.id,
            &server.id,
            server.revision,
            "native-reference",
            &server.configuration,
        )
        .await
        .unwrap()
        .public_view();
    let mut shared_factory = factory;
    shared_factory.native_only = false;
    shared_factory.revision = 2;
    runtime
        .store
        .put_native_mcp_factory(&shared_factory, 1)
        .await
        .unwrap();
    let single = single_request(&runtime, &project, &server, &profile).await;
    let workflow = workflow_request(&runtime, &project, &server, &profile).await;
    assert!(runtime.preflight(&single).await.is_err());
    assert!(runtime.create_run(&single).await.is_err());
    assert!(runtime.preflight_workflow(&workflow).await.is_err());
    assert!(
        runtime
            .create_product_workflow_run(&workflow)
            .await
            .is_err()
    );
    assert!(
        runtime
            .store
            .runs(&single.order_id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        runtime
            .store
            .runs(&workflow.order_id)
            .await
            .unwrap()
            .is_empty()
    );
}
#[tokio::test]
async fn single_owner_server_dispatch_uses_its_explicit_server_key_and_actual_mcp_transport() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::sqlite(&dir.path().join("db")).await.unwrap();
    let remote = support::Remote::new(dir.path().join("effects"), Duration::ZERO, false).await;
    *remote.state.required_bearer.lock().unwrap() = Some("owner-transport-fixture".into());
    let keys = keys();
    let fixture = support::setup_with_credential(
        &store,
        &remote,
        AccessMode::Autonomous,
        Some((keys.as_ref(), keys.0, "owner-transport-fixture")),
    )
    .await;
    let runtime = Runtime::new(store, vec![], false)
        .unwrap()
        .with_personal_provider_keys(keys.clone(), keys.0);
    let result = runtime
        .dispatch_configured_mcp_tool(
            &fixture.run.id,
            &fixture.token,
            &fixture.tool,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(matches!(result.state, McpAttemptState::Completed { .. }));
    assert_eq!(remote.calls(), 1);
}
