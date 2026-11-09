//! Real HTTP/manager gates with synthetic operator prerequisites in a private DB.
//! This does not prove an engine transfer, backup artifact or physical stop.
use super::*;
use std::{future::IntoFuture, sync::Arc, time::Duration};

struct OwnedApi {
    task: tokio::task::JoinHandle<std::io::Result<()>>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    base: String,
    auth_base: String,
}
impl Drop for OwnedApi {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        self.task.abort();
    }
}
impl OwnedApi {
    async fn start(state: AppState) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let base = format!("{origin}/api/v1/management");
        let auth_base = format!("{origin}/api/v1");
        let logout = Router::new()
            .route("/auth/logout", post(auth::logout))
            .route_layer(middleware::from_fn_with_state(
                state.clone(),
                auth::require_auth,
            ));
        let router = Router::new()
            .nest("/api/v1/management",crate::management::router(state.clone()))
            .nest("/api/v1",logout)
            .with_state(state);
        let (stop, stopping) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = stopping.await;
                })
                .into_future(),
        );
        Self {
            task,
            stop: Some(stop),
            base,
            auth_base,
        }
    }
    async fn joined(mut self) {
        self.stop.take().unwrap().send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), &mut self.task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        println!("import-http-owned-api: graceful shutdown joined");
    }
}

#[allow(clippy::too_many_arguments)]
async fn http(
    client: &reqwest::Client,
    api: &OwnedApi,
    method: reqwest::Method,
    path: &str,
    token: Option<&str>,
    payload: Option<Value>,
    expected: u16,
    label: &str,
) -> Value {
    let mut request = client.request(method, format!("{}{path}", api.base));
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    if let Some(payload) = payload {
        request = request.json(&payload);
    }
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    let value: Value = response.json().await.unwrap();
    assert_eq!(status, expected, "{label}: {value}");
    println!("import-http-check: {label}");
    value
}

async fn member(pool: &sqlx::PgPool, org: Uuid, role: &str) -> cowork_contracts::AuthTokens {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users(id,etag,email,display_name) VALUES($1,'private-http-contract',$2,'private HTTP contract')")
        .bind(id).bind(format!("{id}@example.invalid")).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO management_members(organisation_id,user_id,role) VALUES($1,$2,$3)")
        .bind(org)
        .bind(id)
        .bind(role)
        .execute(pool)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    let tokens = auth::create_session_tx(&mut tx, id, Uuid::new_v4())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    tokens
}

#[derive(Clone, Copy)]
enum AuthorityChange {
    Logout,
    SessionExpiry,
    TokenExpiry,
    TokenRevocation,
    DeviceRebinding,
    SessionUserRebinding(Uuid),
}
impl AuthorityChange {
    fn label(self) -> &'static str {
        match self {
            Self::Logout => "real HTTP logout",
            Self::SessionExpiry => "natural session expiry with current database clock",
            Self::TokenExpiry => "natural access token expiry with current database clock",
            Self::TokenRevocation => "exact access token revocation",
            Self::DeviceRebinding => "session device rebinding",
            Self::SessionUserRebinding(_) => "session user rebinding",
        }
    }
}

async fn import_state(pool: &sqlx::PgPool) -> Value {
    sqlx::query_scalar("SELECT jsonb_build_object('imports',(SELECT coalesce(jsonb_agg(to_jsonb(i) ORDER BY id),'[]') FROM management_instance_imports i),'operations',(SELECT coalesce(jsonb_agg(to_jsonb(o) ORDER BY id),'[]') FROM management_operations o),'locks',(SELECT coalesce(jsonb_agg(to_jsonb(l) ORDER BY instance_id),'[]') FROM management_instance_import_locks l),'routes',(SELECT coalesce(jsonb_agg(to_jsonb(r) ORDER BY source_instance_id),'[]') FROM management_instance_routes r),'allocations',(SELECT coalesce(jsonb_agg(to_jsonb(a) ORDER BY instance_id),'[]') FROM management_host_allocations a),'audit',(SELECT coalesce(jsonb_agg(to_jsonb(a) ORDER BY sequence),'[]') FROM management_audit a))")
        .fetch_one(pool).await.unwrap()
}

async fn blocked_query(pool: &sqlx::PgPool, blockers: &[i32], prefix: &str) -> i32 {
    tokio::time::timeout(Duration::from_secs(2),async {
        loop {
            let pid:Option<i32>=sqlx::query_scalar("SELECT a.pid FROM pg_stat_activity a WHERE a.datname=current_database() AND a.query LIKE ($2 || '%') AND pg_blocking_pids(a.pid) && $1 LIMIT 1")
                .bind(blockers).bind(prefix).fetch_optional(pool).await.unwrap();
            if let Some(pid)=pid { return pid; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap_or_else(|_|panic!("the owned request query must be observably blocked: {prefix}"))
}

#[allow(clippy::too_many_arguments)]
async fn authority_expiry_after_audit_wait(
    pool: &sqlx::PgPool,
    client: &reqwest::Client,
    api: &OwnedApi,
    org: Uuid,
    user: Uuid,
    path: &str,
    payload: Value,
    label: &str,
    session_expiry: bool,
) {
    let mut session = pool.begin().await.unwrap();
    let tokens = auth::create_session_tx(&mut session, user, Uuid::new_v4())
        .await
        .unwrap();
    session.commit().await.unwrap();
    let (update, expired) = if session_expiry {
        ("UPDATE auth_sessions SET expires_at=clock_timestamp()+interval '2 seconds' WHERE id=$1",
         "SELECT s.expires_at<=clock_timestamp() AND EXISTS(SELECT 1 FROM pg_stat_activity a WHERE a.pid=$2 AND a.xact_start<s.expires_at AND cardinality(pg_blocking_pids(a.pid))>0) FROM auth_sessions s WHERE s.id=$1")
    } else {
        ("UPDATE access_tokens SET expires_at=clock_timestamp()+interval '2 seconds' WHERE session_id=$1",
         "SELECT t.expires_at<=clock_timestamp() AND EXISTS(SELECT 1 FROM pg_stat_activity a WHERE a.pid=$2 AND a.xact_start<t.expires_at AND cardinality(pg_blocking_pids(a.pid))>0) FROM access_tokens t WHERE t.session_id=$1")
    };
    sqlx::query(update)
        .bind(tokens.session_id)
        .execute(pool)
        .await
        .unwrap();
    let before = import_state(pool).await;
    let mut gate = pool.begin().await.unwrap();
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *gate)
        .await
        .unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("management-audit:{org}"))
        .execute(&mut *gate)
        .await
        .unwrap();
    let request = client
        .post(format!("{}{path}", api.base))
        .bearer_auth(&tokens.access_token)
        .json(&payload);
    let pending = tokio::spawn(async move { request.send().await.unwrap() });
    let backend = blocked_query(
        pool,
        &[blocker],
        "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
    )
    .await;
    // This wait is after the authority row locks and tentative import writes.
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if sqlx::query_scalar::<_, bool>(expired)
                .bind(tokens.session_id)
                .bind(backend)
                .fetch_one(pool)
                .await
                .unwrap()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("actual DB expiry must occur after transaction start during its live audit wait");
    gate.commit().await.unwrap();
    let response = pending.await.unwrap();
    let status = response.status().as_u16();
    let value: Value = response.json().await.unwrap();
    assert_eq!(
        status, 401,
        "{label}: authority expired behind audit gate: {value}"
    );
    assert_eq!(
        import_state(pool).await,
        before,
        "expiry after tentative writes must roll back every import mutation"
    );
    println!("import-http-check: {label}: {} expires during verified final audit wait; entire transaction rolled back",if session_expiry {"session"} else {"access token"});
}

async fn real_logout_session_lock_order(
    pool: &sqlx::PgPool,
    client: &reqwest::Client,
    api: &OwnedApi,
    user: Uuid,
    path: &str,
    payload: Value,
    label: &str,
) {
    let mut session = pool.begin().await.unwrap();
    let tokens = auth::create_session_tx(&mut session, user, Uuid::new_v4())
        .await
        .unwrap();
    session.commit().await.unwrap();
    let before = import_state(pool).await;
    let mut gate = pool.begin().await.unwrap();
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *gate)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM auth_sessions WHERE id=$1 FOR UPDATE")
        .bind(tokens.session_id)
        .fetch_one(&mut *gate)
        .await
        .unwrap();
    let logout = client
        .post(format!("{}/auth/logout", api.auth_base))
        .bearer_auth(&tokens.access_token);
    let logout = tokio::spawn(async move { logout.send().await.unwrap() });
    let logout_backend = blocked_query(
        pool,
        &[blocker],
        "UPDATE auth_sessions SET revoked_at = now(), revoke_reason = 'user_logout'",
    )
    .await;
    let request = client
        .post(format!("{}{path}", api.base))
        .bearer_auth(&tokens.access_token)
        .json(&payload);
    let pending = tokio::spawn(async move { request.send().await.unwrap() });
    blocked_query(
        pool,
        &[blocker, logout_backend],
        // The production MFA middleware now validates and share-locks the
        // session before the import handler takes its own authority locks.
        "SELECT s.*,u.platform_admin,u.deleted_at,clock_timestamp() AS checked_at FROM auth_sessions",
    )
    .await;
    gate.commit().await.unwrap();
    let logout = tokio::time::timeout(Duration::from_secs(2), logout)
        .await
        .expect("session-to-token logout must not deadlock with import authority")
        .unwrap();
    assert_eq!(logout.status().as_u16(), 204);
    let response = tokio::time::timeout(Duration::from_secs(2), pending)
        .await
        .unwrap()
        .unwrap();
    let status = response.status().as_u16();
    let value: Value = response.json().await.unwrap();
    assert_eq!(
        status, 401,
        "{label}: pending logout authority was reused: {value}"
    );
    assert_eq!(import_state(pool).await, before);
    println!("import-http-check: {label}: real HTTP logout and import session-row wait finish without lock inversion or mutation");
}

#[allow(clippy::too_many_arguments)]
async fn authority_after_verified_import_lock_wait(
    pool: &sqlx::PgPool,
    client: &reqwest::Client,
    api: &OwnedApi,
    user: Uuid,
    path: &str,
    payload: Value,
    label: &str,
    change: AuthorityChange,
) {
    let mut session = pool.begin().await.unwrap();
    let tokens = auth::create_session_tx(&mut session, user, Uuid::new_v4())
        .await
        .unwrap();
    session.commit().await.unwrap();
    let before = import_state(pool).await;
    let mut gate = pool.begin().await.unwrap();
    rollouts::lock(&mut gate).await.unwrap();
    let request = client
        .post(format!("{}{path}", api.base))
        .bearer_auth(&tokens.access_token)
        .json(&payload);
    let pending = tokio::spawn(async move { request.send().await.unwrap() });
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks l JOIN pg_stat_activity a ON a.pid=l.pid WHERE l.locktype='advisory' AND NOT l.granted AND a.datname=current_database() AND a.query='SELECT pg_advisory_xact_lock(7182483622)')")
                .fetch_one(pool).await.unwrap();
            if waiting { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("actual HTTP handler must wait after middleware authenticated it");
    match change {
        AuthorityChange::Logout => {
            let logout = client
                .post(format!("{}/auth/logout", api.auth_base))
                .bearer_auth(&tokens.access_token)
                .send()
                .await
                .unwrap();
            assert_eq!(
                logout.status().as_u16(),
                204,
                "real logout completes before the waiting import resumes"
            );
        }
        AuthorityChange::SessionExpiry | AuthorityChange::TokenExpiry => {
            let (update, expired) = if matches!(change, AuthorityChange::SessionExpiry) {
                ("UPDATE auth_sessions SET expires_at=clock_timestamp()+interval '150 milliseconds' WHERE id=$1",
                 "SELECT s.expires_at<=clock_timestamp() AND EXISTS(SELECT 1 FROM pg_locks l JOIN pg_stat_activity a ON a.pid=l.pid WHERE l.locktype='advisory' AND NOT l.granted AND a.datname=current_database() AND a.query='SELECT pg_advisory_xact_lock(7182483622)' AND a.xact_start<s.expires_at) FROM auth_sessions s WHERE s.id=$1")
            } else {
                ("UPDATE access_tokens SET expires_at=clock_timestamp()+interval '150 milliseconds' WHERE session_id=$1",
                 "SELECT t.expires_at<=clock_timestamp() AND EXISTS(SELECT 1 FROM pg_locks l JOIN pg_stat_activity a ON a.pid=l.pid WHERE l.locktype='advisory' AND NOT l.granted AND a.datname=current_database() AND a.query='SELECT pg_advisory_xact_lock(7182483622)' AND a.xact_start<t.expires_at) FROM access_tokens t WHERE t.session_id=$1")
            };
            sqlx::query(update)
                .bind(tokens.session_id)
                .execute(pool)
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(2),async {
                loop {
                    if sqlx::query_scalar::<_,bool>(expired).bind(tokens.session_id).fetch_one(pool).await.unwrap() { break; }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }).await.expect("actual DB deadline must expire after handler transaction began, while its lock wait remains live");
        }
        AuthorityChange::TokenRevocation => {
            sqlx::query(
                "UPDATE access_tokens SET revoked_at=clock_timestamp() WHERE token_hash=$1",
            )
            .bind(auth::opaque_token_hash(&tokens.access_token).as_slice())
            .execute(pool)
            .await
            .unwrap();
        }
        AuthorityChange::DeviceRebinding => {
            sqlx::query("UPDATE auth_sessions SET device_id=$2 WHERE id=$1")
                .bind(tokens.session_id)
                .bind(Uuid::new_v4())
                .execute(pool)
                .await
                .unwrap();
        }
        AuthorityChange::SessionUserRebinding(other) => {
            sqlx::query("UPDATE auth_sessions SET user_id=$2 WHERE id=$1")
                .bind(tokens.session_id)
                .bind(other)
                .execute(pool)
                .await
                .unwrap();
        }
    }
    gate.commit().await.unwrap();
    let response = pending.await.unwrap();
    let status = response.status().as_u16();
    let value: Value = response.json().await.unwrap();
    assert_eq!(
        status, 401,
        "{label}: stale authority crossed actual import lock wait: {value}"
    );
    assert_eq!(import_state(pool).await,before,"denied authority must roll back every import, route, operation, lock, allocation and audit mutation");
    println!(
        "import-http-check: {label}: {} after verified lock wait; no import mutation",
        change.label()
    );
}

async fn refresh_capacity(pool: &sqlx::PgPool, host: Uuid) {
    sqlx::query("INSERT INTO management_host_capacity_proofs(host_id,host_revision,allocation_revision,qualified,observed_capacity,inventory_sha256,expires_at) SELECT id,revision,allocation_revision,true,capacity,$2,clock_timestamp()+interval '2 minutes' FROM management_executor_hosts WHERE id=$1 ON CONFLICT(host_id) DO UPDATE SET allocation_revision=EXCLUDED.allocation_revision,expires_at=EXCLUDED.expires_at")
        .bind(host).bind("e".repeat(64)).execute(pool).await.unwrap();
}

async fn pair(
    pool: &sqlx::PgPool,
    org: Uuid,
    owner: Uuid,
    host: Uuid,
    release: &str,
) -> PrepareInstanceImport {
    let source = Uuid::new_v4();
    let target = Uuid::new_v4();
    let source_epoch = Uuid::new_v4();
    let target_epoch = Uuid::new_v4();
    let policy = PolicyRevision::default();
    let value = serde_json::to_value(&policy).unwrap();
    for (id, epoch, assigned) in [
        (source, source_epoch, None),
        (target, target_epoch, Some(host)),
    ] {
        let evidence =
            json!({"active_epoch":epoch,"runtime":{"version":1,"manifest_sha256":"c".repeat(64)}});
        sqlx::query("INSERT INTO management_instances(id,organisation_id,owner_user_id,name,backend,state,policy,executor_host_id,current_release,execution_required,execution_epoch,execution_evidence) VALUES($1,$2,$3,'private HTTP contract','compose','stopped',$4,$5,$6,true,$7,$8)")
            .bind(id).bind(org).bind(owner).bind(&value).bind(assigned).bind(release).bind(epoch).bind(evidence)
            .execute(pool).await.unwrap();
    }
    let services = json!({"api":{"cpu_millis":100,"memory_mib":64,"pids":16},"worker":{"cpu_millis":100,"memory_mib":64,"pids":16},"runner":{"cpu_millis":100,"memory_mib":64,"pids":16},"egress-proxy":{"cpu_millis":100,"memory_mib":64,"pids":16}});
    let plan = json!({"version":1,"instance_id":target,"services":services,"sandbox":{"cpu_millis":policy.limits.cpu_millis,"memory_mib":policy.limits.memory_mib,"concurrent_runs":policy.limits.concurrent_runs},"total":{"cpu_millis":policy.limits.cpu_millis+400,"memory_mib":policy.limits.memory_mib+256}});
    sqlx::query("INSERT INTO management_host_allocations(instance_id,host_id,organisation_id,plan,plan_sha256,cpu_millis,memory_mib,state,verified_epoch,verified_manifest_sha256,verified_revision,verified_budget) VALUES($1,$2,$3,$4,$5,$6,$7,'verified',$8,$9,1,$4)")
        .bind(target).bind(host).bind(org).bind(&plan).bind(digest(&plan).unwrap())
        .bind(i64::from(policy.limits.cpu_millis)+400).bind(i64::from(policy.limits.memory_mib)+256)
        .bind(target_epoch).bind("c".repeat(64)).execute(pool).await.unwrap();
    refresh_capacity(pool, host).await;
    let operation = Uuid::new_v4();
    sqlx::query("INSERT INTO management_operations(id,organisation_id,instance_id,actor_user_id,action,state,idempotency_key,request_digest,updated_at) VALUES($1,$2,$3,$4,'backup','succeeded',$5,$6,now()-interval '3 seconds')")
        .bind(operation).bind(org).bind(source).bind(owner).bind(Uuid::new_v4().to_string()).bind("b".repeat(64)).execute(pool).await.unwrap();
    let backup = Uuid::new_v4();
    sqlx::query("INSERT INTO management_backups(id,organisation_id,instance_id,operation_id,sha256,size_bytes,manifest_version,created_at) VALUES($1,$2,$3,$4,$5,128,1,clock_timestamp())")
        .bind(backup).bind(org).bind(source).bind(operation).bind("a".repeat(64)).execute(pool).await.unwrap();
    PrepareInstanceImport {
        source_instance_id: source,
        target_instance_id: target,
        source_revision: 1,
        target_revision: 1,
        backup_id: backup,
        idempotency_key: Uuid::new_v4(),
    }
}

async fn exercise(pool: sqlx::PgPool, url: String) {
    // The helper enables the real release gate and supplies its own private audit key.
    crate::management::initialize().unwrap();
    assert!(provisioning_releases::enabled());
    let org = Uuid::new_v4();
    let foreign_org = Uuid::new_v4();
    let policy = serde_json::to_value(PolicyRevision::default()).unwrap();
    for id in [org, foreign_org] {
        sqlx::query("INSERT INTO management_organisations(id,name,policy) VALUES($1,'private HTTP contract',$2)")
            .bind(id).bind(&policy).execute(&pool).await.unwrap();
        let mut tx=pool.begin().await.unwrap();
        crate::management::mfa::ensure_policy(&mut tx,id).await.unwrap();
        tx.commit().await.unwrap();
    }
    let actor = member(&pool, org, "admin").await;
    let operator = member(&pool, org, "admin").await;
    let auditor = member(&pool, org, "auditor").await;
    let outsider = member(&pool, foreign_org, "admin").await;
    let host = Uuid::new_v4();
    let capacity = json!({"cpu_millis":50000,"memory_mib":65536,"reserved_cpu_millis":1000,"reserved_memory_mib":1024});
    let services = json!({"api":{"cpu_millis":100,"memory_mib":64,"pids":16},"worker":{"cpu_millis":100,"memory_mib":64,"pids":16},"runner":{"cpu_millis":100,"memory_mib":64,"pids":16},"egress-proxy":{"cpu_millis":100,"memory_mib":64,"pids":16}});
    sqlx::query("INSERT INTO management_executor_hosts(id,trust_domain,name,daemon_id,operator_state_id,created_by,capacity,service_plan) VALUES($1,$2,'private HTTP contract',$3,$4,$5,$6,$7)")
        .bind(host).bind(org).bind(format!("private-http-contract-{}",Uuid::new_v4().simple()))
        .bind(Uuid::new_v4()).bind(actor.user_id).bind(capacity).bind(services).execute(&pool).await.unwrap();
    let release = "f".repeat(64);
    sqlx::query("INSERT INTO management_releases(id,manifest) VALUES($1,'{}')")
        .bind(&release)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO management_executor_host_releases(host_id,release_id) VALUES($1,$2)")
        .bind(host)
        .bind(&release)
        .execute(&pool)
        .await
        .unwrap();
    let request = pair(&pool, org, actor.user_id, host, &release).await;
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
    let api = OwnedApi::start(state).await;
    let client = reqwest::Client::builder()
        .no_proxy()
        .pool_max_idle_per_host(0)
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let prepare_path = format!("/organisations/{org}/instance-imports");
    let payload = serde_json::to_value(&request).unwrap();
    http(
        &client,
        &api,
        reqwest::Method::POST,
        &prepare_path,
        None,
        Some(payload.clone()),
        401,
        "missing bearer denied",
    )
    .await;
    http(
        &client,
        &api,
        reqwest::Method::POST,
        &prepare_path,
        Some("private-invalid-token"),
        Some(payload.clone()),
        401,
        "invalid bearer denied",
    )
    .await;
    http(
        &client,
        &api,
        reqwest::Method::POST,
        &prepare_path,
        Some(&auditor.access_token),
        Some(payload.clone()),
        404,
        "auditor cannot prepare",
    )
    .await;
    http(
        &client,
        &api,
        reqwest::Method::POST,
        &prepare_path,
        Some(&outsider.access_token),
        Some(payload.clone()),
        404,
        "foreign organisation cannot prepare",
    )
    .await;
    for change in [
        AuthorityChange::Logout,
        AuthorityChange::SessionExpiry,
        AuthorityChange::TokenExpiry,
        AuthorityChange::TokenRevocation,
        AuthorityChange::DeviceRebinding,
        AuthorityChange::SessionUserRebinding(operator.user_id),
    ] {
        authority_after_verified_import_lock_wait(
            &pool,
            &client,
            &api,
            actor.user_id,
            &prepare_path,
            payload.clone(),
            "prepare revalidates authority",
            change,
        )
        .await;
    }
    for session_expiry in [true, false] {
        authority_expiry_after_audit_wait(
            &pool,
            &client,
            &api,
            org,
            actor.user_id,
            &prepare_path,
            payload.clone(),
            "prepare validates final authority",
            session_expiry,
        )
        .await;
    }
    real_logout_session_lock_order(
        &pool,
        &client,
        &api,
        actor.user_id,
        &prepare_path,
        payload.clone(),
        "prepare session lock order",
    )
    .await;
    let prepared = http(
        &client,
        &api,
        reqwest::Method::POST,
        &prepare_path,
        Some(&actor.access_token),
        Some(payload.clone()),
        200,
        "prepare atomically accepts bound source and target",
    )
    .await;
    let id: Uuid = serde_json::from_value(prepared["id"].clone()).unwrap();
    let replay = http(
        &client,
        &api,
        reqwest::Method::POST,
        &prepare_path,
        Some(&actor.access_token),
        Some(payload.clone()),
        200,
        "prepare exact replay",
    )
    .await;
    assert_eq!(
        replay, prepared,
        "prepare response must replay its durable original receipt"
    );
    let operations: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM management_operations WHERE instance_import_id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(operations, 1);
    let mut changed = payload.clone();
    changed["target_revision"] = json!(2);
    http(
        &client,
        &api,
        reqwest::Method::POST,
        &prepare_path,
        Some(&actor.access_token),
        Some(changed),
        409,
        "prepare key cannot change its frozen request",
    )
    .await;
    let import_path = format!("{prepare_path}/{id}");
    let cutover_path = format!("{import_path}/cutover");
    let route_path = format!(
        "/organisations/{org}/instance-routes/{}",
        request.source_instance_id
    );
    http(
        &client,
        &api,
        reqwest::Method::GET,
        &import_path,
        Some(&outsider.access_token),
        None,
        404,
        "foreign import inspection hidden",
    )
    .await;
    let cutover = CutoverInstanceImport {
        expected_revision: 5,
        expected_route_revision: 1,
        target_instance_id: request.target_instance_id,
        idempotency_key: Uuid::new_v4(),
    };
    let cutover_payload = serde_json::to_value(&cutover).unwrap();
    http(
        &client,
        &api,
        reqwest::Method::POST,
        &cutover_path,
        Some(&operator.access_token),
        Some(cutover_payload.clone()),
        409,
        "cutover before qualification denied",
    )
    .await;
    // Synthetic already-completed operator prerequisites. No process or backup was restored.
    let binding: InstanceImportBinding = serde_json::from_value(
        sqlx::query_scalar::<_, Value>(
            "SELECT binding FROM management_instance_imports WHERE id=$1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap(),
    )
    .unwrap();
    sqlx::query("UPDATE management_instances SET execution_epoch=$2,execution_evidence=jsonb_build_object('active_epoch',$2::uuid,'runtime',jsonb_build_object('version',1,'manifest_sha256',$3::text)) WHERE id=$1")
        .bind(request.target_instance_id).bind(binding.candidate_epoch).bind("c".repeat(64)).execute(&pool).await.unwrap();
    sqlx::query("UPDATE management_host_allocations SET revision=2,verified_revision=2,verified_epoch=$2 WHERE instance_id=$1")
        .bind(request.target_instance_id).bind(binding.candidate_epoch).execute(&pool).await.unwrap();
    refresh_capacity(&pool, host).await;
    sqlx::query("UPDATE management_operations SET state='succeeded' WHERE instance_import_id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE management_instance_imports SET phase='qualified',revision=5 WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE management_members SET role='member' WHERE organisation_id=$1 AND user_id=$2",
    )
    .bind(org)
    .bind(actor.user_id)
    .execute(&pool)
    .await
    .unwrap();
    http(
        &client,
        &api,
        reqwest::Method::POST,
        &cutover_path,
        Some(&operator.access_token),
        Some(cutover_payload.clone()),
        404,
        "cutover revalidates original actor after revocation",
    )
    .await;
    http(
        &client,
        &api,
        reqwest::Method::POST,
        &prepare_path,
        Some(&actor.access_token),
        Some(payload.clone()),
        404,
        "prepare replay revalidates current actor membership",
    )
    .await;
    sqlx::query(
        "UPDATE management_members SET role='admin' WHERE organisation_id=$1 AND user_id=$2",
    )
    .bind(org)
    .bind(actor.user_id)
    .execute(&pool)
    .await
    .unwrap();
    let mut stale = cutover_payload.clone();
    stale["expected_route_revision"] = json!(2);
    http(
        &client,
        &api,
        reqwest::Method::POST,
        &cutover_path,
        Some(&operator.access_token),
        Some(stale),
        409,
        "cutover stale route CAS denied",
    )
    .await;
    let mut stale = cutover_payload.clone();
    stale["expected_revision"] = json!(4);
    http(
        &client,
        &api,
        reqwest::Method::POST,
        &cutover_path,
        Some(&operator.access_token),
        Some(stale),
        409,
        "cutover stale import revision denied",
    )
    .await;
    sqlx::query("UPDATE management_host_capacity_proofs SET expires_at=clock_timestamp()-interval '1 second' WHERE host_id=$1").bind(host).execute(&pool).await.unwrap();
    http(
        &client,
        &api,
        reqwest::Method::POST,
        &cutover_path,
        Some(&operator.access_token),
        Some(cutover_payload.clone()),
        503,
        "cutover revalidates current host capacity proof",
    )
    .await;
    refresh_capacity(&pool, host).await;
    let retained: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM management_instance_import_locks WHERE import_id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(retained, 2);
    for change in [
        AuthorityChange::Logout,
        AuthorityChange::SessionExpiry,
        AuthorityChange::TokenExpiry,
        AuthorityChange::TokenRevocation,
        AuthorityChange::DeviceRebinding,
        AuthorityChange::SessionUserRebinding(actor.user_id),
    ] {
        authority_after_verified_import_lock_wait(
            &pool,
            &client,
            &api,
            operator.user_id,
            &cutover_path,
            cutover_payload.clone(),
            "cutover revalidates authority",
            change,
        )
        .await;
    }
    for session_expiry in [true, false] {
        authority_expiry_after_audit_wait(
            &pool,
            &client,
            &api,
            org,
            operator.user_id,
            &cutover_path,
            cutover_payload.clone(),
            "cutover validates final authority",
            session_expiry,
        )
        .await;
    }
    real_logout_session_lock_order(
        &pool,
        &client,
        &api,
        operator.user_id,
        &cutover_path,
        cutover_payload.clone(),
        "cutover session lock order",
    )
    .await;
    let receipt = http(
        &client,
        &api,
        reqwest::Method::POST,
        &cutover_path,
        Some(&operator.access_token),
        Some(cutover_payload.clone()),
        200,
        "qualified exact cutover accepted",
    )
    .await;
    let late_prepare = http(
        &client,
        &api,
        reqwest::Method::POST,
        &prepare_path,
        Some(&actor.access_token),
        Some(payload.clone()),
        200,
        "prepare replay preserves first receipt after committed cutover",
    )
    .await;
    assert_eq!(late_prepare, prepared);
    let replay = http(
        &client,
        &api,
        reqwest::Method::POST,
        &cutover_path,
        Some(&operator.access_token),
        Some(cutover_payload.clone()),
        200,
        "cutover exact immutable receipt replay",
    )
    .await;
    assert_eq!(replay, receipt);
    let route = http(
        &client,
        &api,
        reqwest::Method::GET,
        &route_path,
        Some(&operator.access_token),
        None,
        200,
        "explicit route resolves accepted target",
    )
    .await;
    assert_eq!(
        route["target_instance_id"],
        json!(request.target_instance_id)
    );
    assert_eq!(route["revision"], 2);
    let mut different = cutover_payload;
    different["idempotency_key"] = json!(Uuid::new_v4());
    http(
        &client,
        &api,
        reqwest::Method::POST,
        &cutover_path,
        Some(&operator.access_token),
        Some(different),
        409,
        "cutover cannot be rebound after acceptance",
    )
    .await;
    let locks: Vec<Uuid> = sqlx::query_scalar(
        "SELECT instance_id FROM management_instance_import_locks WHERE import_id=$1",
    )
    .bind(id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(locks, vec![request.source_instance_id]);
    let second = pair(&pool, org, actor.user_id, host, &release).await;
    let prepared = http(
        &client,
        &api,
        reqwest::Method::POST,
        &prepare_path,
        Some(&actor.access_token),
        Some(serde_json::to_value(&second).unwrap()),
        200,
        "independent second import prepared",
    )
    .await;
    let abort_id: Uuid = serde_json::from_value(prepared["id"].clone()).unwrap();
    let abort_path = format!("{prepare_path}/{abort_id}/abort");
    let abort = AbortInstanceImport {
        expected_revision: 2,
        idempotency_key: Uuid::new_v4(),
    };
    let abort_payload = serde_json::to_value(&abort).unwrap();
    http(
        &client,
        &api,
        reqwest::Method::POST,
        &abort_path,
        Some(&operator.access_token),
        Some(abort_payload.clone()),
        409,
        "abort before source quiescence denied",
    )
    .await;
    sqlx::query("UPDATE management_operations SET state='succeeded' WHERE instance_import_id=$1")
        .bind(abort_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE management_instance_imports SET phase='source_quiescent',revision=2 WHERE id=$1",
    )
    .bind(abort_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE management_members SET role='member' WHERE organisation_id=$1 AND user_id=$2",
    )
    .bind(org)
    .bind(actor.user_id)
    .execute(&pool)
    .await
    .unwrap();
    http(
        &client,
        &api,
        reqwest::Method::POST,
        &abort_path,
        Some(&operator.access_token),
        Some(abort_payload.clone()),
        404,
        "abort revalidates original actor",
    )
    .await;
    sqlx::query(
        "UPDATE management_members SET role='admin' WHERE organisation_id=$1 AND user_id=$2",
    )
    .bind(org)
    .bind(actor.user_id)
    .execute(&pool)
    .await
    .unwrap();
    let mut stale = abort_payload.clone();
    stale["expected_revision"] = json!(1);
    http(
        &client,
        &api,
        reqwest::Method::POST,
        &abort_path,
        Some(&operator.access_token),
        Some(stale),
        409,
        "abort stale revision denied",
    )
    .await;
    for change in [
        AuthorityChange::Logout,
        AuthorityChange::SessionExpiry,
        AuthorityChange::TokenExpiry,
        AuthorityChange::TokenRevocation,
        AuthorityChange::DeviceRebinding,
        AuthorityChange::SessionUserRebinding(actor.user_id),
    ] {
        authority_after_verified_import_lock_wait(
            &pool,
            &client,
            &api,
            operator.user_id,
            &abort_path,
            abort_payload.clone(),
            "abort revalidates authority",
            change,
        )
        .await;
    }
    for session_expiry in [true, false] {
        authority_expiry_after_audit_wait(
            &pool,
            &client,
            &api,
            org,
            operator.user_id,
            &abort_path,
            abort_payload.clone(),
            "abort validates final authority",
            session_expiry,
        )
        .await;
    }
    real_logout_session_lock_order(
        &pool,
        &client,
        &api,
        operator.user_id,
        &abort_path,
        abort_payload.clone(),
        "abort session lock order",
    )
    .await;
    let aborted = http(
        &client,
        &api,
        reqwest::Method::POST,
        &abort_path,
        Some(&operator.access_token),
        Some(abort_payload.clone()),
        200,
        "proven pre-write abort accepted",
    )
    .await;
    let replay = http(
        &client,
        &api,
        reqwest::Method::POST,
        &abort_path,
        Some(&operator.access_token),
        Some(abort_payload.clone()),
        200,
        "abort exact immutable receipt replay",
    )
    .await;
    assert_eq!(replay, aborted);
    assert_eq!(aborted["charges_retained"], true);
    let locks: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM management_instance_import_locks WHERE import_id=$1",
    )
    .bind(abort_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(locks, 0);
    let unknown = pair(&pool, org, actor.user_id, host, &release).await;
    let prepared = http(
        &client,
        &api,
        reqwest::Method::POST,
        &prepare_path,
        Some(&actor.access_token),
        Some(serde_json::to_value(&unknown).unwrap()),
        200,
        "separate uncertain-write import prepared",
    )
    .await;
    let unknown_id: Uuid = serde_json::from_value(prepared["id"].clone()).unwrap();
    sqlx::query("UPDATE management_operations SET state='succeeded' WHERE instance_import_id=$1")
        .bind(unknown_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE management_instance_imports SET phase='export_verified',revision=3 WHERE id=$1",
    )
    .bind(unknown_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO management_operations(id,organisation_id,instance_id,actor_user_id,action,state,idempotency_key,request_digest,instance_import_id) VALUES($1,$2,$3,$4,'import_restore','failed',$5,$6,$7)")
        .bind(Uuid::new_v4()).bind(org).bind(unknown.target_instance_id).bind(actor.user_id).bind(Uuid::new_v4().to_string()).bind("b".repeat(64)).bind(unknown_id).execute(&pool).await.unwrap();
    let abort_path = format!("{prepare_path}/{unknown_id}/abort");
    http(
        &client,
        &api,
        reqwest::Method::POST,
        &abort_path,
        Some(&operator.access_token),
        Some(
            serde_json::to_value(AbortInstanceImport {
                expected_revision: 3,
                idempotency_key: Uuid::new_v4(),
            })
            .unwrap(),
        ),
        409,
        "unknown target writes retain both locks and deny abort",
    )
    .await;
    let locks: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM management_instance_import_locks WHERE import_id=$1",
    )
    .bind(unknown_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(locks, 2);
    sqlx::query("UPDATE auth_sessions SET revoked_at=clock_timestamp() WHERE id=$1")
        .bind(operator.session_id)
        .execute(&pool)
        .await
        .unwrap();
    http(
        &client,
        &api,
        reqwest::Method::GET,
        &route_path,
        Some(&operator.access_token),
        None,
        401,
        "revoked session cannot inspect accepted route",
    )
    .await;
    drop(client);
    api.joined().await;
}

#[tokio::test]
#[ignore = "requires own COWORK_TEST_DATABASE_URL, private audit key and enabled release selection; UUID schema only"]
async fn postgres_import_actual_http_auth_prepare_cutover_abort_revalidation_and_replay() {
    let url = std::env::var("COWORK_TEST_DATABASE_URL").expect("private fixture URL");
    let cleanup = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    let schema = format!("cowork_import_http_{}", Uuid::new_v4().simple());
    sqlx::raw_sql(&format!("CREATE SCHEMA {schema}"))
        .execute(&cleanup)
        .await
        .unwrap();
    let options: sqlx::postgres::PgConnectOptions = url.parse().unwrap();
    let options = options.options([("search_path", schema.as_str())]);
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await
        .unwrap();
    let result = tokio::spawn(async move {
        db::migrate(&pool).await.unwrap();
        exercise(pool.clone(), url).await;
        pool.close().await;
    })
    .await;
    sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&cleanup)
        .await
        .unwrap();
    cleanup.close().await;
    result.unwrap();
}
