use super::*;
use std::sync::Arc;

fn input(kind: Kind, configuration: Value) -> ResourceInput {
    ResourceInput {
        kind,
        name: "Fixture resource".into(),
        description: String::new(),
        configuration,
        secret_reference: None,
        enabled: true,
    }
}
fn actor(user_id: Uuid) -> Principal {
    Principal {
        user_id,
        session_id: None,
        bootstrap: false,
    }
}

#[test]
fn typed_configuration_rejects_credentials_and_authorisation_roles() {
    assert!(validate(&mut input(Kind::Endpoint,json!({"base_url":"https://models.example/v1","protocol":"openai_compatible","api_key":"private"}))).is_err());
    assert!(validate(&mut input(
        Kind::Endpoint,
        json!({"base_url":"https://user:password@models.example/v1","protocol":"openai_compatible"})
    ))
    .is_err());
    assert!(validate(&mut input(
        Kind::Endpoint,
        json!({"base_url":"https://models.example/v1?token=value","protocol":"openai_compatible"})
    ))
    .is_err());
    let mut resource = input(
        Kind::Skill,
        json!({"instructions":"Summarise task-oriented documents."}),
    );
    assert!(validate(&mut resource).is_ok());
    resource.secret_reference = Some("sk-not-a-secret-reference".into());
    assert!(validate(&mut resource).is_err());
    resource.secret_reference = Some("env:PROVIDER_KEY".into());
    assert!(validate(&mut resource).is_ok());
    assert!(serde_json::from_value::<ResourceInput>(json!({"kind":"agent_role","name":"Admin","configuration":{"system_prompt":"Assist"},"platform_admin":true})).is_err());
    assert!(validate(&mut input(
        Kind::AgentRole,
        json!({"system_prompt":"Assist","organisation_role":"admin"})
    ))
    .is_err());
    assert!(validate(&mut input(
        Kind::Mcp,
        json!({"transport":"stdio","command":"mcp-server","args":["--api-key","private"]})
    ))
    .is_err());
}

#[tokio::test]
#[ignore = "requires COWORK_TEST_DATABASE_URL; only a fresh UUID schema is mutated"]
async fn postgres_resource_catalog_cas_scopes_revocation_archive_and_private_delivery() {
    let url = std::env::var("COWORK_TEST_DATABASE_URL").expect("owned test PostgreSQL URL");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    let schema = format!("cowork_resources_{}", Uuid::new_v4().simple());
    sqlx::raw_sql(&format!(
        "CREATE SCHEMA {schema}; SET search_path TO {schema};"
    ))
    .execute(&pool)
    .await
    .unwrap();
    crate::db::migrate(&pool).await.unwrap();
    let key_path = std::env::temp_dir().join(format!(
        "cowork-resource-fixture-{}",
        Uuid::new_v4().simple()
    ));
    std::fs::write(
        &key_path,
        b"fixture-only-resource-catalog-key-00000000000000000000000000000000",
    )
    .unwrap();
    let previous_audit = std::env::var_os("COWORK_MANAGEMENT_AUDIT_KEY_FILE");
    let previous_broker = std::env::var_os("COWORK_PROVISIONER_TOKEN_FILE");
    std::env::set_var("COWORK_MANAGEMENT_AUDIT_KEY_FILE", &key_path);
    std::env::set_var("COWORK_PROVISIONER_TOKEN_FILE", &key_path);
    struct Keys(
        std::path::PathBuf,
        Option<std::ffi::OsString>,
        Option<std::ffi::OsString>,
    );
    impl Drop for Keys {
        fn drop(&mut self) {
            for (name, value) in [
                ("COWORK_MANAGEMENT_AUDIT_KEY_FILE", &self.1),
                ("COWORK_PROVISIONER_TOKEN_FILE", &self.2),
            ] {
                if let Some(value) = value {
                    std::env::set_var(name, value)
                } else {
                    std::env::remove_var(name)
                }
            }
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let keys = Keys(key_path, previous_audit, previous_broker);
    let platform = Uuid::new_v4();
    let admin = Uuid::new_v4();
    let user = Uuid::new_v4();
    let outsider = Uuid::new_v4();
    for id in [platform, admin, user, outsider] {
        sqlx::query("INSERT INTO users(id,etag,email,display_name,platform_admin) VALUES($1,'fixture',$2,'Resource fixture',$3)").bind(id).bind(format!("{id}@example.invalid")).bind(id==platform).execute(&pool).await.unwrap();
    }
    let org = Uuid::new_v4();
    let foreign = Uuid::new_v4();
    let policy = serde_json::to_value(PolicyRevision::default()).unwrap();
    for id in [org, foreign] {
        sqlx::query(
            "INSERT INTO management_organisations(id,name,policy) VALUES($1,'Resource fixture',$2)",
        )
        .bind(id)
        .bind(&policy)
        .execute(&pool)
        .await
        .unwrap();
    }
    for (o, u, role) in [
        (org, admin, "admin"),
        (org, user, "member"),
        (foreign, outsider, "admin"),
    ] {
        sqlx::query(
            "INSERT INTO management_members(organisation_id,user_id,role) VALUES($1,$2,$3)",
        )
        .bind(o)
        .bind(u)
        .bind(role)
        .execute(&pool)
        .await
        .unwrap();
    }
    let instance = Uuid::new_v4();
    let foreign_instance = Uuid::new_v4();
    for (o, i, owner) in [
        (org, instance, admin),
        (foreign, foreign_instance, outsider),
    ] {
        sqlx::query("INSERT INTO management_instances(id,organisation_id,owner_user_id,name,backend,state,policy) VALUES($1,$2,$3,'Resource fixture','compose','ready',$4)").bind(i).bind(o).bind(owner).bind(&policy).execute(&pool).await.unwrap();
    }
    sqlx::query("INSERT INTO management_instance_members(organisation_id,instance_id,user_id,role) VALUES($1,$2,$3,'member')").bind(org).bind(instance).bind(user).execute(&pool).await.unwrap();
    let state = AppState {
        pool: pool.clone(),
        management_mode: true,
        auth_secrets: None,
        bootstrap_token_digest: [0; 32],
        bootstrap_user_id: Uuid::new_v4(),
        lease_seconds: 15,
        server_capabilities: Arc::new(vec![]),
        object_store: None,
        runner: None,
        executor_hub: crate::executor_ws::ExecutorHub::new(None),
        api_replica: None,
        api_cluster: None,
        event_notifications: crate::event_notifications::EventNotifications::start(url),
        push: None,
        webauthn: None,
        oidc: None,
        organisation_oidc: Arc::new(sso::Registry::new()),
        directory: Arc::new(directory::catalog::Registry::new()),
    };
    let endpoint_config =
        json!({"base_url":"https://provider.example/v1","protocol":"openai_compatible"});
    assert!(matches!(
        create(
            State(state.clone()),
            Extension(actor(admin)),
            Json(input(Kind::Endpoint, endpoint_config.clone()))
        )
        .await,
        Err(ApiError::NotFound(_))
    ));
    let (status, Json(endpoint)) = create(
        State(state.clone()),
        Extension(actor(platform)),
        Json(input(Kind::Endpoint, endpoint_config.clone())),
    )
    .await
    .unwrap();
    assert_eq!(status, StatusCode::CREATED);
    let endpoint_id: Uuid = serde_json::from_value(endpoint["id"].clone()).unwrap();
    let (_, Json(model)) = create(
        State(state.clone()),
        Extension(actor(platform)),
        Json(input(
            Kind::Model,
            json!({"model_id":"fixture-model","endpoint_resource_id":endpoint_id}),
        )),
    )
    .await
    .unwrap();
    let model_id: Uuid = serde_json::from_value(model["id"].clone()).unwrap();
    let schedule_guard = crate::providers::ensure_profile_for_schedule(
        &pool,
        user,
        Uuid::nil(),
        Some(model_id),
        &cowork_contracts::ExecutorTarget::ServerLinux { pool_id: None },
    )
    .await;
    assert!(
        matches!(schedule_guard, Err(ApiError::Unprocessable(ref message)) if message.contains("immediate server runs only")),
        "virtual catalog UUIDs must be rejected before a schedule FK write"
    );
    assert!(crate::providers::ensure_profile_for_schedule(
        &pool,
        user,
        Uuid::nil(),
        None,
        &cowork_contracts::ExecutorTarget::ServerLinux { pool_id: None }
    )
    .await
    .is_ok());
    let (_, Json(skill)) = create(
        State(state.clone()),
        Extension(actor(platform)),
        Json(input(
            Kind::Skill,
            json!({"instructions":"Summarise documents"}),
        )),
    )
    .await
    .unwrap();
    let skill_id: Uuid = serde_json::from_value(skill["id"].clone()).unwrap();
    let grants = vec![
        Assignment {
            resource_id: model_id,
            instance_id: None,
            environment_id: None,
        },
        Assignment {
            resource_id: skill_id,
            instance_id: Some(instance),
            environment_id: None,
        },
    ];
    let Json(initial) = assignments(
        State(state.clone()),
        Extension(actor(admin)),
        Path((org, user)),
    )
    .await
    .unwrap();
    assert_eq!(initial["revision"], 0);
    let Json(saved) = set_assignments(
        State(state.clone()),
        Extension(actor(admin)),
        Path((org, user)),
        Json(SetAssignments {
            expected_revision: 0,
            assignments: grants.clone(),
        }),
    )
    .await
    .unwrap();
    assert_eq!(saved["revision"], 1);
    assert_eq!(saved["delivery"]["enforced"], false);
    assert!(matches!(
        set_assignments(
            State(state.clone()),
            Extension(actor(admin)),
            Path((org, user)),
            Json(SetAssignments {
                expected_revision: 0,
                assignments: vec![]
            })
        )
        .await,
        Err(ApiError::Conflict(_))
    ));
    assert!(set_assignments(
        State(state.clone()),
        Extension(actor(outsider)),
        Path((org, user)),
        Json(SetAssignments {
            expected_revision: 1,
            assignments: vec![]
        })
    )
    .await
    .is_err());
    assert!(set_assignments(
        State(state.clone()),
        Extension(actor(admin)),
        Path((org, user)),
        Json(SetAssignments {
            expected_revision: 1,
            assignments: vec![Assignment {
                resource_id: skill_id,
                instance_id: Some(foreign_instance),
                environment_id: None
            }]
        })
    )
    .await
    .is_err());
    assert!(set_assignments(
        State(state.clone()),
        Extension(actor(admin)),
        Path((org, user)),
        Json(SetAssignments {
            expected_revision: 1,
            assignments: vec![Assignment {
                resource_id: skill_id,
                instance_id: Some(instance),
                environment_id: Some(Uuid::new_v4())
            }]
        })
    )
    .await
    .is_err());
    let Json(own) = visible(State(state.clone()), Extension(actor(user)), Path(org))
        .await
        .unwrap();
    assert_eq!(own["items"].as_array().unwrap().len(), 2);
    let query = || EntitlementScope {
        organisation_id: org,
        instance_id: instance,
        user_id: user,
        environment_id: None,
        consumer_kind: None,
        consumer_version: None,
    };
    assert!(
        entitlements(State(state.clone()), HeaderMap::new(), Query(query()))
            .await
            .is_err()
    );
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-cowork-instance-key",
        crate::fleet::instance_key(org, instance)
            .unwrap()
            .parse()
            .unwrap(),
    );
    let Json(delivered) = entitlements(State(state.clone()), headers.clone(), Query(query()))
        .await
        .unwrap();
    assert_eq!(delivered["items"].as_array().unwrap().len(), 2);
    assert_eq!(delivered["enforced"], false);
    assert_eq!(delivered["dependencies"].as_array().unwrap().len(), 1);
    let mut model_query = query();
    model_query.consumer_kind = Some("model_provider".into());
    model_query.consumer_version = Some(1);
    let Json(model_manifest) =
        entitlements(State(state.clone()), headers.clone(), Query(model_query))
            .await
            .unwrap();
    assert_eq!(model_manifest["items"].as_array().unwrap().len(), 1);
    assert_eq!(model_manifest["items"][0]["id"], json!(model_id));
    assert_eq!(model_manifest["dependencies"][0]["id"], json!(endpoint_id));
    assert_eq!(model_manifest["consumer"]["version"], 1);
    let foreign_query = EntitlementScope {
        organisation_id: foreign,
        instance_id: foreign_instance,
        user_id: outsider,
        environment_id: None,
        consumer_kind: None,
        consumer_version: None,
    };
    assert!(
        entitlements(State(state.clone()), headers.clone(), Query(foreign_query))
            .await
            .is_err()
    );
    let update_endpoint = |revision, enabled| UpdateResource {
        expected_revision: revision,
        kind: Kind::Endpoint,
        name: "Fixture endpoint".into(),
        description: String::new(),
        configuration: endpoint_config.clone(),
        secret_reference: None,
        enabled,
    };
    update(
        State(state.clone()),
        Extension(actor(platform)),
        Path(endpoint_id),
        Json(update_endpoint(1, false)),
    )
    .await
    .unwrap();
    let Json(inactive) = visible(State(state.clone()), Extension(actor(user)), Path(org))
        .await
        .unwrap();
    assert_eq!(
        inactive["items"].as_array().unwrap().len(),
        1,
        "endpoint disable suppresses dependent model grant"
    );
    assert!(matches!(
        update(
            State(state.clone()),
            Extension(actor(platform)),
            Path(endpoint_id),
            Json(update_endpoint(1, true))
        )
        .await,
        Err(ApiError::Conflict(_))
    ));
    update(
        State(state.clone()),
        Extension(actor(platform)),
        Path(endpoint_id),
        Json(update_endpoint(2, true)),
    )
    .await
    .unwrap();
    assert!(matches!(
        remove(
            State(state.clone()),
            Extension(actor(platform)),
            Path(endpoint_id),
            Json(DeleteResource {
                expected_revision: 3
            })
        )
        .await,
        Err(ApiError::Conflict(_))
    ));
    remove(
        State(state.clone()),
        Extension(actor(platform)),
        Path(model_id),
        Json(DeleteResource {
            expected_revision: 1,
        }),
    )
    .await
    .unwrap();
    let Json(revoked) = assignments(
        State(state.clone()),
        Extension(actor(admin)),
        Path((org, user)),
    )
    .await
    .unwrap();
    assert_eq!(revoked["revision"], 2);
    assert_eq!(revoked["assignments"].as_array().unwrap().len(), 1);
    sqlx::query("DELETE FROM management_instance_members WHERE instance_id=$1 AND user_id=$2")
        .bind(instance)
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    assert!(entitlements(State(state.clone()), headers, Query(query()))
        .await
        .is_err());
    let Json(hidden) = visible(State(state.clone()), Extension(actor(user)), Path(org))
        .await
        .unwrap();
    assert!(hidden["items"].as_array().unwrap().is_empty());
    verify_real_wallet_proxy_limits(&state, org, instance, admin, platform).await;
    sqlx::query("UPDATE management_organisations SET archived_at=now(),archived_by=$2 WHERE id=$1")
        .bind(org)
        .bind(platform)
        .execute(&pool)
        .await
        .unwrap();
    assert!(assignments(
        State(state.clone()),
        Extension(actor(admin)),
        Path((org, user))
    )
    .await
    .is_err());
    assert!(sqlx::query("INSERT INTO management_resource_assignments(id,organisation_id,user_id,resource_id,assigned_by) VALUES($1,$2,$3,$4,$5)").bind(Uuid::new_v4()).bind(org).bind(user).bind(skill_id).bind(admin).execute(&pool).await.is_err());
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action LIKE 'resource_catalog.%'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(audits >= 6);
    drop(state);
    drop(keys);
    sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
}

async fn verify_real_wallet_proxy_limits(
    state: &AppState,
    org: Uuid,
    instance: Uuid,
    user: Uuid,
    platform: Uuid,
) {
    use axum::{routing::post, Router};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app=Router::new().route("/v1/chat/completions",post(move|Json(body):Json<Value>|{let counter=counter.clone();async move{
        assert_eq!(body["max_tokens"],1024);assert_eq!(body["stream"],false);assert_eq!(body["n"],1);
        counter.fetch_add(1,Ordering::SeqCst);tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        Json(json!({"id":"fixture","model":"proxy-model","choices":[{"index":0,"message":{"role":"assistant","content":"Fixture"},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":3,"total_tokens":8}}))
    }}));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let (_, Json(endpoint)) = create(
        State(state.clone()),
        Extension(actor(platform)),
        Json(input(
            Kind::Endpoint,
            json!({"base_url":format!("{url}/v1"),"protocol":"openai_compatible"}),
        )),
    )
    .await
    .unwrap();
    let endpoint_id: Uuid = serde_json::from_value(endpoint["id"].clone()).unwrap();
    let (_,Json(model))=create(State(state.clone()),Extension(actor(platform)),Json(input(Kind::Model,json!({"model_id":"proxy-model","endpoint_resource_id":endpoint_id,"context_window":8192,"max_output_tokens":1024,"pricing":{"currency":"EUR","input_micros_per_million":1000000,"output_micros_per_million":1000000}})))).await.unwrap();
    let model_id: Uuid = serde_json::from_value(model["id"].clone()).unwrap();
    set_assignments(
        State(state.clone()),
        Extension(actor(platform)),
        Path((org, user)),
        Json(SetAssignments {
            expected_revision: 0,
            assignments: vec![Assignment {
                resource_id: model_id,
                instance_id: Some(instance),
                environment_id: None,
            }],
        }),
    )
    .await
    .unwrap();
    sqlx::query("INSERT INTO management_member_wallets(organisation_id,user_id,enabled,balance_micros,monthly_limit_micros) VALUES($1,$2,TRUE,0,8192)")
        .bind(org).bind(user).execute(&state.pool).await.unwrap();
    let key = crate::fleet::instance_key(org, instance).unwrap();
    let token = crate::managed_resources::model_proxy_token(
        &key,
        org,
        instance,
        user,
        model_id,
        chrono::Utc::now().timestamp(),
    )
    .unwrap();
    let mut headers = HeaderMap::new();
    headers.insert("authorization", format!("Bearer {token}").parse().unwrap());
    let request = json!({"model":"proxy-model","messages":[{"role":"user","content":"Hello"}]});
    let invoke = || {
        model_proxy::complete(
            State(state.clone()),
            Path((org, instance, user, model_id)),
            headers.clone(),
            Json(request.clone()),
        )
    };
    assert!(matches!(invoke().await, Err(ApiError::Quota(_))));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "an exhausted real wallet must cause zero provider HTTP requests"
    );
    sqlx::query("UPDATE management_member_wallets SET balance_micros=8192 WHERE organisation_id=$1 AND user_id=$2").bind(org).bind(user).execute(&state.pool).await.unwrap();
    let (first, second) = tokio::join!(invoke(), invoke());
    assert_eq!(
        usize::from(first.is_ok()) + usize::from(second.is_ok()),
        1,
        "concurrent actual proxy requests must reserve atomically"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let remaining:i64=sqlx::query_scalar("SELECT balance_micros FROM management_member_wallets WHERE organisation_id=$1 AND user_id=$2").bind(org).bind(user).fetch_one(&state.pool).await.unwrap();
    assert_eq!(
        remaining, 8184,
        "actual provider usage is settled at the explicit tariff"
    );
    let charged:i64=sqlx::query_scalar("SELECT sum(charged_micros)::BIGINT FROM management_wallet_reservations WHERE organisation_id=$1 AND user_id=$2 AND state='settled'").bind(org).bind(user).fetch_one(&state.pool).await.unwrap();
    assert_eq!(charged, 8);
    set_assignments(
        State(state.clone()),
        Extension(actor(platform)),
        Path((org, user)),
        Json(SetAssignments {
            expected_revision: 1,
            assignments: vec![],
        }),
    )
    .await
    .unwrap();
    assert!(invoke().await.is_err());
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "revoked model grants must never reach provider"
    );
    server.abort();
    let _ = server.await;
}
