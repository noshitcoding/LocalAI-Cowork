use super::*;
use axum::http::StatusCode;
use std::sync::Arc;

struct Database {
    pool: sqlx::PgPool,
    admin: sqlx::PgPool,
    schema: String,
}
impl Database {
    async fn new() -> Self {
        let url = std::env::var("COWORK_TEST_DATABASE_URL").expect("owned PostgreSQL test URL");
        let admin = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        let schema = format!("environment_access_{}", Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let scope = schema.clone();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .after_connect(move |connection, _| {
                let scope = scope.clone();
                Box::pin(async move {
                    sqlx::query(&format!("SET search_path TO {scope}"))
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        crate::db::migrate(&pool).await.unwrap();
        Self {
            pool,
            admin,
            schema,
        }
    }
    async fn finish(self, result: Result<(), tokio::task::JoinError>) {
        self.pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .unwrap();
        self.admin.close().await;
        result.unwrap();
    }
}

struct TestKeys(
    std::path::PathBuf,
    Option<std::ffi::OsString>,
    Option<std::ffi::OsString>,
);
impl TestKeys {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "cowork-environment-access-fixture-{}",
            Uuid::new_v4()
        ));
        std::fs::write(
            &path,
            b"fixture-only-environment-access-keys-0000000000000000000000000000",
        )
        .unwrap();
        let audit = std::env::var_os("COWORK_MANAGEMENT_AUDIT_KEY_FILE");
        let broker = std::env::var_os("COWORK_PROVISIONER_TOKEN_FILE");
        std::env::set_var("COWORK_MANAGEMENT_AUDIT_KEY_FILE", &path);
        std::env::set_var("COWORK_PROVISIONER_TOKEN_FILE", &path);
        Self(path, audit, broker)
    }
}
impl Drop for TestKeys {
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

async fn allowed(
    pool: &sqlx::PgPool,
    org: Uuid,
    instance: Uuid,
    env: Uuid,
    user: Uuid,
    role: Role,
) -> bool {
    let mut tx = pool.begin().await.unwrap();
    let result = require(&mut tx, org, instance, env, user, role)
        .await
        .is_ok();
    tx.rollback().await.unwrap();
    result
}

#[tokio::test]
#[ignore = "requires isolated COWORK_TEST_DATABASE_URL; run with --test-threads=1 because fixture keys are process-scoped"]
async fn postgres_content_launch_and_instance_listing_preserve_scope_and_archive_guards() {
    let db = Database::new().await;
    let pool = db.pool.clone();
    let result = tokio::spawn(async move {
        let _keys = TestKeys::new();
        struct LaunchTemplate(Option<std::ffi::OsString>);
        impl Drop for LaunchTemplate {
            fn drop(&mut self) {
                if let Some(value) = &self.0 {
                    std::env::set_var("COWORK_INSTANCE_URL_TEMPLATE", value);
                } else {
                    std::env::remove_var("COWORK_INSTANCE_URL_TEMPLATE");
                }
            }
        }
        let _template = LaunchTemplate(std::env::var_os("COWORK_INSTANCE_URL_TEMPLATE"));
        std::env::set_var("COWORK_INSTANCE_URL_TEMPLATE", "http://127.0.0.1:18442/{instance_id}");
        let admin = Uuid::new_v4();
        let viewer = Uuid::new_v4();
        let unassigned = Uuid::new_v4();
        let auditor = Uuid::new_v4();
        let outsider = Uuid::new_v4();
        let org = Uuid::new_v4();
        let foreign = Uuid::new_v4();
        let instance = Uuid::new_v4();
        for user in [admin, viewer, unassigned, auditor, outsider] {
            sqlx::query("INSERT INTO users(id,etag,email,display_name) VALUES($1,'fixture',$2,'Content launch fixture')")
                .bind(user).bind(format!("{user}@example.invalid")).execute(&pool).await.unwrap();
        }
        for organisation in [org, foreign] {
            sqlx::query("INSERT INTO management_organisations(id,name,policy) VALUES($1,'Content launch fixture',$2)")
                .bind(organisation).bind(serde_json::to_value(PolicyRevision::default()).unwrap()).execute(&pool).await.unwrap();
        }
        for (organisation, user, role) in [(org,admin,"admin"),(org,viewer,"member"),(org,unassigned,"member"),(org,auditor,"auditor"),(foreign,outsider,"admin")] {
            sqlx::query("INSERT INTO management_members(organisation_id,user_id,role) VALUES($1,$2,$3)")
                .bind(organisation).bind(user).bind(role).execute(&pool).await.unwrap();
        }
        sqlx::query("INSERT INTO management_instances(id,organisation_id,owner_user_id,name,backend,state,policy) VALUES($1,$2,$3,'Content launch fixture','compose','ready',$4)")
            .bind(instance).bind(org).bind(admin).bind(serde_json::to_value(PolicyRevision::default()).unwrap()).execute(&pool).await.unwrap();
        for user in [viewer, auditor] {
            sqlx::query("INSERT INTO management_instance_members(organisation_id,instance_id,user_id,role) VALUES($1,$2,$3,'viewer')")
                .bind(org).bind(instance).bind(user).execute(&pool).await.unwrap();
        }
        sqlx::query("INSERT INTO management_instance_features(organisation_id,instance_id,content_access_version) VALUES($1,$2,2)")
            .bind(org).bind(instance).execute(&pool).await.unwrap();
        let state = AppState {
            pool:pool.clone(),management_mode:true,auth_secrets:None,bootstrap_token_digest:[0;32],bootstrap_user_id:Uuid::new_v4(),lease_seconds:15,
            server_capabilities:Arc::new(vec![]),object_store:None,runner:None,executor_hub:crate::executor_ws::ExecutorHub::new(None),api_replica:None,api_cluster:None,
            event_notifications:crate::event_notifications::EventNotifications::start(std::env::var("COWORK_TEST_DATABASE_URL").unwrap()),push:None,webauthn:None,oidc:None,
            organisation_oidc:Arc::new(sso::Registry::new()),directory:Arc::new(directory::catalog::Registry::new()),
        };
        let actor = |user_id| Principal { user_id,session_id:None,bootstrap:false };
        // Execute the production route, including both database-backed launch
        // decisions, rather than merely checking that SQL contains a JOIN.
        let app = axum::Router::new()
            .route("/organisations/{org}/instances", axum::routing::get(crate::management::instances))
            .layer(Extension(actor(viewer)))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        struct Server(tokio::task::JoinHandle<()>);
        impl Drop for Server { fn drop(&mut self) { self.0.abort(); } }
        let _server = Server(server);
        let response = reqwest::Client::new().get(format!("http://{address}/organisations/{org}/instances")).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let listed: Vec<Value> = response.json().await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0]["id"], json!(instance));
        assert_eq!(listed[0]["can_open"], true);
        assert_eq!(listed[0]["can_open_support"], false);
        assert_eq!(listed[0]["can_manage"], false);
        let Json(admin_list) = crate::management::instances(State(state.clone()),Extension(actor(admin)),Path(org)).await.unwrap();
        assert_eq!(admin_list.len(), 1);
        assert_eq!(admin_list[0]["can_open_support"], true);
        assert_eq!(admin_list[0]["can_manage"], true);
        let Json(auditor_list) = crate::management::instances(State(state.clone()),Extension(actor(auditor)),Path(org)).await.unwrap();
        assert_eq!(auditor_list.len(), 1);
        assert_eq!(auditor_list[0]["can_open"], false);
        assert_eq!(auditor_list[0]["can_open_support"], false);
        let Json(empty) = crate::management::instances(State(state.clone()),Extension(actor(unassigned)),Path(org)).await.unwrap();
        assert!(empty.is_empty());
        assert!(matches!(crate::management::instances(State(state.clone()),Extension(actor(outsider)),Path(org)).await,Err(ApiError::NotFound(_))));
        assert!(content::can_launch_on(&pool,org,instance,admin).await.unwrap());
        assert!(content::can_launch_viewer_on(&pool,org,instance,viewer).await.unwrap());
        for user in [viewer,unassigned,auditor,outsider] {
            assert!(!content::can_launch_on(&pool,org,instance,user).await.unwrap());
        }
        for user in [admin,unassigned,auditor,outsider] {
            assert!(!content::can_launch_viewer_on(&pool,org,instance,user).await.unwrap());
        }
        assert!(!content::can_launch_on(&pool,foreign,instance,outsider).await.unwrap());
        assert!(!content::can_launch_viewer_on(&pool,foreign,instance,viewer).await.unwrap());
        // An old capability heartbeat cannot authorize either support or viewer.
        sqlx::query("UPDATE management_instance_features SET refreshed_at=now()-interval '91 seconds' WHERE instance_id=$1").bind(instance).execute(&pool).await.unwrap();
        assert!(!content::can_launch_on(&pool,org,instance,admin).await.unwrap());
        assert!(!content::can_launch_viewer_on(&pool,org,instance,viewer).await.unwrap());
        sqlx::query("UPDATE management_instance_features SET refreshed_at=now() WHERE instance_id=$1").bind(instance).execute(&pool).await.unwrap();
        sqlx::query("DELETE FROM management_instance_members WHERE instance_id=$1 AND user_id=$2").bind(instance).bind(viewer).execute(&pool).await.unwrap();
        assert!(!content::can_launch_viewer_on(&pool,org,instance,viewer).await.unwrap());
        sqlx::query("INSERT INTO management_instance_members(organisation_id,instance_id,user_id,role) VALUES($1,$2,$3,'viewer')").bind(org).bind(instance).bind(viewer).execute(&pool).await.unwrap();
        sqlx::query("UPDATE management_organisations SET archived_at=now(),archived_by=$2 WHERE id=$1").bind(org).bind(admin).execute(&pool).await.unwrap();
        assert!(!content::can_launch_on(&pool,org,instance,admin).await.unwrap());
        assert!(!content::can_launch_viewer_on(&pool,org,instance,viewer).await.unwrap());
        let response = reqwest::Client::new().get(format!("http://{address}/organisations/{org}/instances")).send().await.unwrap();
        assert_eq!(response.status(),StatusCode::NOT_FOUND);
        assert!(matches!(crate::management::instances(State(state),Extension(actor(admin)),Path(org)).await,Err(ApiError::NotFound(_))));
    }).await;
    db.finish(result).await;
}

#[tokio::test]
#[ignore = "requires isolated COWORK_TEST_DATABASE_URL; run with --test-threads=1 because fixture keys are process-scoped"]
async fn postgres_environment_access_permissions_scopes_revocation_and_inflight_fencing() {
    let db = Database::new().await;
    let pool = db.pool.clone();
    let result = tokio::spawn(async move {
        let _keys = TestKeys::new();
        let admin = Uuid::new_v4(); let user = Uuid::new_v4(); let viewer = Uuid::new_v4(); let outsider = Uuid::new_v4();
        let org = Uuid::new_v4(); let foreign = Uuid::new_v4(); let instance = Uuid::new_v4(); let env = Uuid::new_v4(); let template = Uuid::new_v4();
        let mut principals = std::collections::HashMap::new();
        for id in [admin,user,viewer,outsider] {
            sqlx::query("INSERT INTO users(id,etag,email,display_name) VALUES($1,'fixture',$2,'Environment fixture')").bind(id).bind(format!("{id}@example.invalid")).execute(&pool).await.unwrap();
            let session = Uuid::new_v4();
            sqlx::query("INSERT INTO auth_sessions(id,user_id,device_id,refresh_token_hash,refresh_family_id,expires_at) VALUES($1,$2,$3,$4,$5,clock_timestamp()+interval '1 hour')")
                .bind(session).bind(id).bind(Uuid::new_v4()).bind(id.as_bytes().as_slice()).bind(Uuid::new_v4()).execute(&pool).await.unwrap();
            principals.insert(id, Principal { user_id:id,session_id:Some(session),bootstrap:false });
        }
        for id in [org,foreign] {
            sqlx::query("INSERT INTO management_organisations(id,name,policy) VALUES($1,'Environment fixture',$2)").bind(id).bind(serde_json::to_value(PolicyRevision::default()).unwrap()).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO organisation_mfa_policies(organisation_id,epoch) VALUES($1,$2)").bind(id).bind(Uuid::new_v4()).execute(&pool).await.unwrap();
        }
        for (o,u,role) in [(org,admin,"admin"),(org,user,"member"),(org,viewer,"member"),(foreign,outsider,"admin")] {
            sqlx::query("INSERT INTO management_members(organisation_id,user_id,role) VALUES($1,$2,$3)").bind(o).bind(u).bind(role).execute(&pool).await.unwrap();
        }
        sqlx::query("INSERT INTO management_instances(id,organisation_id,owner_user_id,name,backend,state,policy) VALUES($1,$2,$3,'Environment fixture','compose','ready',$4)")
            .bind(instance).bind(org).bind(admin).bind(serde_json::to_value(PolicyRevision::default()).unwrap()).execute(&pool).await.unwrap();
        for (u,role) in [(user,"member"),(viewer,"viewer")] {
            sqlx::query("INSERT INTO management_instance_members(organisation_id,instance_id,user_id,role) VALUES($1,$2,$3,$4)").bind(org).bind(instance).bind(u).bind(role).execute(&pool).await.unwrap();
        }
        sqlx::query("INSERT INTO management_templates(id,organisation_id,revision,name,backend,isolation,image_reference) VALUES($1,$2,1,'Environment fixture','docker','container',$3)")
            .bind(template).bind(org).bind(format!("fixture@sha256:{}","a".repeat(64))).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO management_environments(id,organisation_id,instance_id,template_id,template_revision,spec,state,runtime) VALUES($1,$2,$3,$4,1,$5,'running','{}')")
            .bind(env).bind(org).bind(instance).bind(template).bind(json!({"name":"Environment fixture","instance_id":instance,"template_id":template,"template_revision":1,"backend":"docker","isolation":"container","persistent":true,"shared_workspace":false,"limits":ResourceLimits::default()})).execute(&pool).await.unwrap();
        let state = AppState {
            pool:pool.clone(),management_mode:true,auth_secrets:None,bootstrap_token_digest:[0;32],bootstrap_user_id:Uuid::new_v4(),lease_seconds:15,
            server_capabilities:Arc::new(vec![]),object_store:None,runner:None,executor_hub:crate::executor_ws::ExecutorHub::new(None),api_replica:None,api_cluster:None,
            event_notifications:crate::event_notifications::EventNotifications::start(std::env::var("COWORK_TEST_DATABASE_URL").unwrap()),push:None,webauthn:None,oidc:None,
            organisation_oidc:Arc::new(sso::Registry::new()),directory:Arc::new(directory::catalog::Registry::new()),
        };
        let Json(initial) = inspect(State(state.clone()),Extension(principals[&admin].clone()),Path((org,instance,env))).await.unwrap();
        assert_eq!(initial["revision"],1); assert_eq!(initial["mode"],"inherit_instance");
        assert!(allowed(&pool,org,instance,env,user,Role::Runner).await);
        assert!(!allowed(&pool,org,instance,env,viewer,Role::Runner).await);
        assert!(allowed(&pool,org,instance,env,viewer,Role::Viewer).await);
        assert!(!allowed(&pool,foreign,instance,env,outsider,Role::Viewer).await);
        assert!(inspect(State(state.clone()),Extension(principals[&user].clone()),Path((org,instance,env))).await.is_err());
        let body = |revision| Update { expected_revision:revision,mode:Mode::Restricted,members:vec![Member { user_id:user,role:Role::Runner },Member { user_id:viewer,role:Role::Viewer }] };
        assert!(update(State(state.clone()),Extension(principals[&user].clone()),Path((org,instance,env)),Json(body(1))).await.is_err());
        assert!(update(State(state.clone()),Extension(principals[&admin].clone()),Path((org,instance,env)),Json(Update { expected_revision:1,mode:Mode::Restricted,members:vec![Member{user_id:outsider,role:Role::Runner}] })).await.is_err());
        let Json(restricted) = update(State(state.clone()),Extension(principals[&admin].clone()),Path((org,instance,env)),Json(body(1))).await.unwrap();
        assert_eq!(restricted["revision"],2);
        assert!(allowed(&pool,org,instance,env,user,Role::Runner).await);
        assert!(allowed(&pool,org,instance,env,viewer,Role::Viewer).await);
        assert!(!allowed(&pool,org,instance,env,viewer,Role::Runner).await);
        assert!(!allowed(&pool,org,instance,env,admin,Role::Runner).await);
        assert!(matches!(update(State(state.clone()),Extension(principals[&admin].clone()),Path((org,instance,env)),Json(body(1))).await,Err(ApiError::Conflict(_))));
        let job = Uuid::new_v4();
        sqlx::query("INSERT INTO management_environment_jobs(id,organisation_id,instance_id,environment_id,actor_user_id,request_id,request_sha256,action,plan,plan_sha256,state) VALUES($1,$2,$3,$4,$5,$6,$7,'inspect','{}',$7,'held')")
            .bind(job).bind(org).bind(instance).bind(env).bind(admin).bind(Uuid::new_v4()).bind("a".repeat(64)).execute(&pool).await.unwrap();
        assert!(matches!(update(State(state.clone()),Extension(principals[&admin].clone()),Path((org,instance,env)),Json(body(2))).await,Err(ApiError::Conflict(_))));
        assert!(allowed(&pool,org,instance,env,user,Role::Runner).await);
        sqlx::query("UPDATE management_environment_jobs SET state='completed' WHERE id=$1").bind(job).execute(&pool).await.unwrap();
        let Json(revoked) = update(State(state.clone()),Extension(principals[&admin].clone()),Path((org,instance,env)),Json(Update { expected_revision:2,mode:Mode::Restricted,members:vec![] })).await.unwrap();
        assert_eq!(revoked["revision"],3);
        let epoch: Uuid = sqlx::query_scalar("SELECT execution_epoch FROM management_instances WHERE id=$1").bind(instance).fetch_one(&pool).await.unwrap();
        let mut headers = HeaderMap::new(); headers.insert("x-cowork-instance-key", crate::fleet::instance_key(org,instance).unwrap().parse().unwrap());
        let request_id = Uuid::new_v4(); let workflow_id = Uuid::new_v4(); let step_id = Uuid::new_v4();
        let request = json!({"format":1,"request_id":request_id,"workflow_id":workflow_id,"step_id":step_id,"organisation_id":org,"instance_id":instance,"execution_epoch":epoch,"user_id":user,"management_session_id":principals[&user].session_id.unwrap(),"environment_id":env,"expected_environment_revision":1,"profile_sha256":"a".repeat(64),"task_input":{"operation":"process","command":["true"]},"capabilities":[]});
        let parsed: environment_execution::Request = serde_json::from_value(request.clone()).unwrap();
        let digest = hex::encode(Sha256::digest(serde_json::to_vec(&parsed).unwrap()));
        sqlx::query("INSERT INTO management_environment_executions(id,organisation_id,instance_id,environment_id,user_id,request_id,workflow_id,step_id,request_sha256,plan,plan_sha256,state) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$9,'halted')")
            .bind(Uuid::new_v4()).bind(org).bind(instance).bind(env).bind(user).bind(request_id).bind(workflow_id).bind(step_id).bind(digest).bind(json!({"organisation_id":org,"instance_id":instance,"user_id":user,"environment":{"id":env}})).execute(&pool).await.unwrap();
        assert!(matches!(environment_execution::submit(State(state.clone()),headers.clone(),Json(parsed)).await,Err(ApiError::NotFound(_))));
        assert!(!allowed(&pool,org,instance,env,user,Role::Runner).await);
        assert!(!allowed(&pool,org,instance,env,viewer,Role::Viewer).await);
        update(State(state.clone()),Extension(principals[&admin].clone()),Path((org,instance,env)),Json(Update { expected_revision:3,mode:Mode::InheritInstance,members:vec![] })).await.unwrap();
        sqlx::query("DELETE FROM management_instance_members WHERE instance_id=$1 AND user_id=$2").bind(instance).bind(user).execute(&pool).await.unwrap();
        assert!(!allowed(&pool,org,instance,env,user,Role::Runner).await);
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM management_audit WHERE organisation_id=$1 AND action='environment.access.changed'").bind(org).fetch_one(&pool).await.unwrap(),3);
        drop(state);
    }).await;
    db.finish(result).await;
}
