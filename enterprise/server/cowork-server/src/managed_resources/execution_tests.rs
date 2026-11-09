use super::*;
fn resource(id: Uuid, kind: &str, configuration: Value) -> Resource {
    serde_json::from_value(json!({"id":id,"kind":kind,"name":"Fixture","configuration":configuration,"secret_reference":null,"revision":1,"enabled":true,"available":true,"created_at":"2026-10-24T00:00:00Z","updated_at":"2026-10-24T00:00:00Z","deleted_at":null})).unwrap()
}
fn catalog(items: Vec<Resource>) -> ExecutionManifest {
    ExecutionManifest {
        contract_version: 2,
        organisation_id: Uuid::new_v4(),
        instance_id: Uuid::new_v4(),
        user_id: Uuid::new_v4(),
        environment_id: None,
        assignment_revision: 1,
        items,
        consumer: Consumer {
            kind: "execution_context".into(),
            version: 2,
        },
    }
}
#[test]
fn role_dependencies_do_not_grant_unassigned_resources() {
    let role = Uuid::new_v4();
    let skill = Uuid::new_v4();
    let mut manifest = catalog(vec![resource(
        role,
        "agent_role",
        json!({"system_prompt":"Assist","resource_ids":[skill]}),
    )]);
    assert!(resolve(&manifest, &[role]).is_err());
    manifest
        .items
        .push(resource(skill, "skill", json!({"instructions":"SKILL_OK"})));
    let resolved = resolve(&manifest, &[role]).unwrap();
    assert_eq!(resolved.len(), 2);
    assert!(resolved.iter().any(|r| r.id == skill));
}
#[test]
fn selected_ids_are_bounded_distinct_and_one_role_is_allowed() {
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    assert!(selected(&json!({"managed_resource_ids":[first,first]})).is_err());
    assert!(selected(&json!({"managed_resource_ids":"not-an-array"})).is_err());
    let manifest = catalog(vec![
        resource(first, "agent_role", json!({"resource_ids":[]})),
        resource(second, "agent_role", json!({"resource_ids":[]})),
    ]);
    assert!(resolve(&manifest, &[first, second]).is_err());
}
#[tokio::test]
async fn a_client_binding_without_selection_is_discarded() {
    let mut model = None;
    let user = Uuid::new_v4();
    let result = freeze(
        user,
        json!({"prompt":"Hello","_managed_resource_binding":{"version":2,"resources":["forged"]}}),
        &ExecutorTarget::ServerLinux { pool_id: None },
        &mut model,
    )
    .await
    .unwrap();
    assert!(result.get("_managed_resource_binding").is_none());
}

#[test]
fn mcp_secret_references_cannot_read_model_or_process_secrets() {
    for reference in [
        "env:PATH",
        "env:COWORK_AUTH_MASTER_KEY",
        "env:COWORK_MANAGED_MODEL_SECRET_TEST",
        "secret:../token",
        "secret:nested/token",
        "vault:service/key",
    ] {
        assert!(resolve_mcp_secret(Some(reference)).is_err());
    }
    assert!(supported_resource_reference(
        Some("env:COWORK_MANAGED_MCP_SECRET_TEST"),
        "COWORK_MANAGED_MCP_SECRET_"
    ));
    assert_eq!(resolve_mcp_secret(None).unwrap(), None);
}

#[tokio::test]
async fn fresh_authority_rejects_revoked_revised_and_foreign_execution_bindings() {
    use axum::{extract::State, routing::get, Router};
    use std::sync::{Arc, Mutex};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = Configuration {
        base: format!("http://{}", listener.local_addr().unwrap()),
        organisation: Uuid::new_v4(),
        instance: Uuid::new_v4(),
        key: "fixture-instance-key".into(),
    };
    let user = Uuid::new_v4();
    let skill = Uuid::new_v4();
    let original = json!({"contract_version":2,"organisation_id":config.organisation,"instance_id":config.instance,"user_id":user,"environment_id":null,"assignment_revision":1,"consumer":{"kind":"execution_context","version":2},"items":[{"id":skill,"kind":"skill","name":"Assigned skill","configuration":{"instructions":"SKILL_OK"},"secret_reference":null,"revision":1,"enabled":true,"available":true,"created_at":"2026-10-24T00:00:00Z","updated_at":"2026-10-24T00:00:00Z","deleted_at":null}]});
    let body = Arc::new(Mutex::new(original.clone()));
    let app = Router::new()
        .route(
            "/management/runner/v1/resource-entitlements",
            get(|State(body): State<Arc<Mutex<Value>>>| async move {
                Json(body.lock().unwrap().clone())
            }),
        )
        .with_state(body.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let binding = Binding {
        version: 2,
        organisation_id: config.organisation,
        instance_id: config.instance,
        user_id: user,
        assignment_revision: 1,
        resources: vec![Revision {
            id: skill,
            revision: 1,
            kind: "skill".into(),
        }],
    };
    assert_eq!(
        verify_binding(
            &binding,
            &config,
            user,
            &manifest(&config, user).await.unwrap()
        )
        .unwrap()
        .len(),
        1
    );
    body.lock().unwrap()["items"] = json!([]);
    assert!(verify_binding(
        &binding,
        &config,
        user,
        &manifest(&config, user).await.unwrap()
    )
    .is_err());
    *body.lock().unwrap() = original.clone();
    body.lock().unwrap()["items"][0]["revision"] = json!(2);
    assert!(verify_binding(
        &binding,
        &config,
        user,
        &manifest(&config, user).await.unwrap()
    )
    .is_err());
    *body.lock().unwrap() = original.clone();
    body.lock().unwrap()["assignment_revision"] = json!(2);
    assert!(verify_binding(
        &binding,
        &config,
        user,
        &manifest(&config, user).await.unwrap()
    )
    .is_err());
    for field in [
        "organisation_id",
        "instance_id",
        "user_id",
        "environment_id",
    ] {
        *body.lock().unwrap() = original.clone();
        body.lock().unwrap()[field] = json!(Uuid::new_v4());
        assert!(
            manifest(&config, user).await.is_err(),
            "foreign scope {field}"
        );
    }
    *body.lock().unwrap() = original.clone();
    body.lock().unwrap()["items"][0]["available"] = json!(false);
    assert!(manifest(&config, user).await.is_err());
    server.abort();
    let _ = server.await;
    assert!(
        manifest(&config, user).await.is_err(),
        "an authority outage must not reuse the old grant"
    );
}
