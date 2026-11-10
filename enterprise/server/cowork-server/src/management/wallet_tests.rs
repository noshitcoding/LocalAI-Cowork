use super::*;
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
        let schema = format!("wallet_{}", Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let scope = schema.clone();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(8)
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
struct AuditKey(std::path::PathBuf, Option<std::ffi::OsString>);
impl AuditKey {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("cowork-wallet-audit-{}", Uuid::new_v4()));
        std::fs::write(
            &path,
            b"fixture-only-wallet-audit-key-000000000000000000000000000000",
        )
        .unwrap();
        let previous = std::env::var_os("COWORK_MANAGEMENT_AUDIT_KEY_FILE");
        std::env::set_var("COWORK_MANAGEMENT_AUDIT_KEY_FILE", &path);
        Self(path, previous)
    }
}
impl Drop for AuditKey {
    fn drop(&mut self) {
        if let Some(value) = &self.1 {
            std::env::set_var("COWORK_MANAGEMENT_AUDIT_KEY_FILE", value)
        } else {
            std::env::remove_var("COWORK_MANAGEMENT_AUDIT_KEY_FILE")
        };
        let _ = std::fs::remove_file(&self.0);
    }
}

#[tokio::test]
#[ignore = "requires owned COWORK_TEST_DATABASE_URL; run --test-threads=1 for process-scoped fixture key"]
async fn postgres_wallet_permissions_cas_idempotency_reservations_holds_and_settlement() {
    let db = Database::new().await;
    let pool = db.pool.clone();
    let result=tokio::spawn(async move{
        let _key=AuditKey::new();let admin=Uuid::new_v4();let user=Uuid::new_v4();let other=Uuid::new_v4();let org=Uuid::new_v4();let foreign=Uuid::new_v4();let instance=Uuid::new_v4();let model=Uuid::new_v4();
        let mut principals=std::collections::HashMap::new();
        for id in [admin,user,other]{
            sqlx::query("INSERT INTO users(id,etag,email,display_name) VALUES($1,'fixture',$2,'Wallet fixture')").bind(id).bind(format!("{id}@example.invalid")).execute(&pool).await.unwrap();
            let sid=Uuid::new_v4();sqlx::query("INSERT INTO auth_sessions(id,user_id,device_id,refresh_token_hash,refresh_family_id,expires_at) VALUES($1,$2,$3,$4,$5,clock_timestamp()+interval '1 hour')").bind(sid).bind(id).bind(Uuid::new_v4()).bind(id.as_bytes().as_slice()).bind(Uuid::new_v4()).execute(&pool).await.unwrap();
            principals.insert(id,Principal{user_id:id,session_id:Some(sid),bootstrap:false});
        }
        for id in [org,foreign]{sqlx::query("INSERT INTO management_organisations(id,name,policy) VALUES($1,'Wallet fixture',$2)").bind(id).bind(serde_json::to_value(PolicyRevision::default()).unwrap()).execute(&pool).await.unwrap();sqlx::query("INSERT INTO organisation_mfa_policies(organisation_id,epoch) VALUES($1,$2)").bind(id).bind(Uuid::new_v4()).execute(&pool).await.unwrap();}
        for(o,u,r)in [(org,admin,"admin"),(org,user,"member"),(foreign,other,"admin")]{sqlx::query("INSERT INTO management_members(organisation_id,user_id,role) VALUES($1,$2,$3)").bind(o).bind(u).bind(r).execute(&pool).await.unwrap();}
        sqlx::query("INSERT INTO management_instances(id,organisation_id,owner_user_id,name,backend,state,policy) VALUES($1,$2,$3,'Wallet fixture','compose','ready',$4)").bind(instance).bind(org).bind(user).bind(serde_json::to_value(PolicyRevision::default()).unwrap()).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO management_resource_catalog(id,kind,name,configuration,created_by,updated_by) VALUES($1,'model','Wallet fixture','{}',$2,$2)").bind(model).bind(admin).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO management_resource_assignment_sets(organisation_id,user_id,updated_by) VALUES($1,$2,$3)").bind(org).bind(user).bind(admin).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO management_resource_assignments(id,organisation_id,user_id,resource_id,assigned_by) VALUES($1,$2,$3,$4,$5)").bind(Uuid::new_v4()).bind(org).bind(user).bind(model).bind(admin).execute(&pool).await.unwrap();
        let state=AppState{pool:pool.clone(),management_mode:true,auth_secrets:None,bootstrap_token_digest:[0;32],bootstrap_user_id:Uuid::new_v4(),lease_seconds:15,server_capabilities:Arc::new(vec![]),object_store:None,runner:None,executor_hub:crate::executor_ws::ExecutorHub::new(None),api_replica:None,api_cluster:None,event_notifications:crate::event_notifications::EventNotifications::start(std::env::var("COWORK_TEST_DATABASE_URL").unwrap()),push:None,webauthn:None,oidc:None,organisation_oidc:Arc::new(sso::Registry::new()),directory:Arc::new(directory::catalog::Registry::new())};
        let read=|who,user|inspect(State(state.clone()),Extension(principals[&who].clone()),Path((org,user)));
        let Json(initial)=read(user,user).await.unwrap();assert_eq!(initial["balance_micros"],0);assert_eq!(initial["enabled"],false);
        assert!(read(user,admin).await.is_err());assert!(read(other,user).await.is_err());
        let Json(own)=list(State(state.clone()),Extension(principals[&user].clone()),Path(org)).await.unwrap();assert_eq!(own["wallets"].as_array().unwrap().len(),1);
        let credit_id=Uuid::new_v4();let credit=|amount,rev|Entry{request_id:credit_id,expected_revision:rev,kind:"credit".into(),amount_micros:amount,reason:"Initial internal API allocation".into()};
        assert!(entry(State(state.clone()),Extension(principals[&user].clone()),Path((org,user)),Json(credit(1000,1))).await.is_err());
        let Json(credited)=entry(State(state.clone()),Extension(principals[&admin].clone()),Path((org,user)),Json(credit(1000,1))).await.unwrap();assert_eq!(credited["balance_micros"],1000);assert_eq!(credited["revision"],2);
        // Lost response retry is idempotent even though the old CAS is stale.
        let Json(replay)=entry(State(state.clone()),Extension(principals[&admin].clone()),Path((org,user)),Json(credit(1000,1))).await.unwrap();assert_eq!(replay["revision"],2);
        assert!(entry(State(state.clone()),Extension(principals[&admin].clone()),Path((org,user)),Json(credit(999,2))).await.is_err());
        assert!(update(State(state.clone()),Extension(principals[&user].clone()),Path((org,user)),Json(Update{expected_revision:2,enabled:true,monthly_limit_micros:Some(800)})).await.is_err());
        assert!(update(State(state.clone()),Extension(principals[&admin].clone()),Path((org,user)),Json(Update{expected_revision:1,enabled:true,monthly_limit_micros:Some(800)})).await.is_err());
        update(State(state.clone()),Extension(principals[&admin].clone()),Path((org,user)),Json(Update{expected_revision:2,enabled:true,monthly_limit_micros:Some(800)})).await.unwrap();
        let request=|max,currency:&str|WalletReservationRequest{organisation_id:org,instance_id:instance,user_id:user,model_resource_id:model,request_id:Uuid::new_v4(),request_sha256:"a".repeat(64),maximum_micros:max,currency:Some(currency.into()),metadata:json!({"test":true})};
        assert!(reserve(&state,request(None,"EUR")).await.is_err());assert!(reserve(&state,request(Some(10),"USD")).await.is_err());
        // Both requests can afford their individual maximum, but only one may
        // commit: the real database row lock fences concurrent monthly spends.
        let(a,b)=tokio::join!(reserve(&state,request(Some(600),"EUR")),reserve(&state,request(Some(600),"EUR")));
        assert_eq!(usize::from(a.is_ok())+usize::from(b.is_ok()),1);let winner=a.ok().or_else(||b.ok()).unwrap().reservation.unwrap();
        let Json(reserved)=read(user,user).await.unwrap();assert_eq!(reserved["reserved_micros"],600);assert_eq!(reserved["available_micros"],400);
        let rev=reserved["revision"].as_i64().unwrap();
        assert!(entry(State(state.clone()),Extension(principals[&admin].clone()),Path((org,user)),Json(Entry{request_id:Uuid::new_v4(),expected_revision:rev,kind:"debit".into(),amount_micros:401,reason:"Cannot withdraw reserved funds".into()})).await.is_err());
        assert!(update(State(state.clone()),Extension(principals[&admin].clone()),Path((org,user)),Json(Update{expected_revision:rev,enabled:true,monthly_limit_micros:Some(599)})).await.is_err());
        hold(&state,winner.id,"transport_timeout").await.unwrap();hold(&state,winner.id,"transport_timeout").await.unwrap();
        assert!(reserve(&state,request(Some(201),"EUR")).await.is_err());
        assert!(settle(&state,winner.id,601,&"b".repeat(64),json!({})).await.is_err());
        settle(&state,winner.id,350,&"b".repeat(64),json!({"verified_tokens":true})).await.unwrap();
        settle(&state,winner.id,350,&"b".repeat(64),json!({"verified_tokens":true})).await.unwrap();
        assert!(settle(&state,winner.id,351,&"b".repeat(64),json!({})).await.is_err());
        let Json(settled)=read(user,user).await.unwrap();assert_eq!(settled["balance_micros"],650);assert_eq!(settled["reserved_micros"],0);assert_eq!(settled["monthly_spent_micros"],350);assert_eq!(settled["entries"].as_array().unwrap().len(),2);
        // Balance permits 451, while the remaining monthly budget is only450.
        assert!(reserve(&state,request(Some(451),"EUR")).await.is_err());
        let held=reserve(&state,request(Some(450),"EUR")).await.unwrap().reservation.unwrap();hold(&state,held.id,"unverifiable_provider_usage").await.unwrap();
        let Json(detail)=read(user,user).await.unwrap();let rev=detail["revision"].as_i64().unwrap();
        assert!(resolve(State(state.clone()),Extension(principals[&user].clone()),Path((org,user,held.id)),Json(Resolve{request_id:Uuid::new_v4(),expected_revision:rev,actual_micros:0,reason:"Provider evidence: never executed".into()})).await.is_err());
        let resolution_id=Uuid::new_v4();let resolution=||Resolve{request_id:resolution_id,expected_revision:rev,actual_micros:0,reason:"Provider evidence: never executed".into()};
        resolve(State(state.clone()),Extension(principals[&admin].clone()),Path((org,user,held.id)),Json(resolution())).await.unwrap();
        resolve(State(state.clone()),Extension(principals[&admin].clone()),Path((org,user,held.id)),Json(resolution())).await.unwrap();
        assert!(sqlx::query("UPDATE management_wallet_ledger SET amount_micros=1 WHERE organisation_id=$1").bind(org).execute(&pool).await.is_err());assert!(sqlx::query("DELETE FROM management_wallet_ledger WHERE organisation_id=$1").bind(org).execute(&pool).await.is_err());
        assert!(settings(State(state.clone()),Extension(principals[&admin].clone()),Path(org),Json(SettingsUpdate{expected_revision:1,currency:"USD".into()})).await.is_err());
        // A crash can skip hold(). It must keep funds blocked and become
        // explicitly reviewable after the provider timeout, never auto-refund.
        let crashed=reserve(&state,request(Some(1),"EUR")).await.unwrap().reservation.unwrap();
        let Json(detail)=read(admin,user).await.unwrap();let revision=detail["revision"].as_i64().unwrap();
        assert!(resolve(State(state.clone()),Extension(principals[&admin].clone()),Path((org,user,crashed.id)),Json(Resolve{request_id:Uuid::new_v4(),expected_revision:revision,actual_micros:0,reason:"Too early to conclude billing".into()})).await.is_err());
        sqlx::query("UPDATE management_wallet_reservations SET created_at=clock_timestamp()-interval '11 minutes' WHERE id=$1").bind(crashed.id).execute(&pool).await.unwrap();
        // Fresh catalog entitlement, membership and archive checks also apply at
        // reservation time, after an earlier proxy catalog snapshot.
        sqlx::query("DELETE FROM management_resource_assignments WHERE organisation_id=$1").bind(org).execute(&pool).await.unwrap();assert!(reserve(&state,request(Some(1),"EUR")).await.is_err());
        // Ownership is an independent membership FK. Transfer the fixture's
        // instance before testing removal of this historical wallet member.
        sqlx::query("UPDATE management_instances SET owner_user_id=$2 WHERE id=$1").bind(instance).bind(admin).execute(&pool).await.unwrap();
        sqlx::query("DELETE FROM management_members WHERE organisation_id=$1 AND user_id=$2").bind(org).bind(user).execute(&pool).await.unwrap();assert!(reserve(&state,request(Some(1),"EUR")).await.is_err());
        assert!(read(user,user).await.is_err());
        let Json(historical)=list(State(state.clone()),Extension(principals[&admin].clone()),Path(org)).await.unwrap();assert!(historical["wallets"].as_array().unwrap().iter().any(|w|w["user_id"]==json!(user)));
        let Json(detail)=read(admin,user).await.unwrap();assert!(detail["reservations"].as_array().unwrap().iter().any(|r|r["id"]==json!(crashed.id)&&r["state"]=="held"));assert_eq!(detail["reserved_micros"],1);
        resolve(State(state.clone()),Extension(principals[&admin].clone()),Path((org,user,crashed.id)),Json(Resolve{request_id:Uuid::new_v4(),expected_revision:revision,actual_micros:0,reason:"Confirmed provider never received interrupted request".into()})).await.unwrap();
        assert!(entry(State(state.clone()),Extension(principals[&admin].clone()),Path((org,user)),Json(Entry{request_id:Uuid::new_v4(),expected_revision:revision+1,kind:"credit".into(),amount_micros:1,reason:"Removed member cannot receive new allocations".into()})).await.is_err());
        let historical:i64=sqlx::query_scalar("SELECT count(*) FROM management_wallet_ledger WHERE organisation_id=$1 AND user_id=$2").bind(org).bind(user).fetch_one(&pool).await.unwrap();assert_eq!(historical,4);
    }).await;
    db.finish(result).await;
}

#[test]
fn wallet_money_and_receipt_validation_is_bounded() {
    for value in [0, 1, MAX_MICROS] {
        assert!(micros(value).is_ok());
    }
    for value in [-1, MAX_MICROS + 1, i64::MAX] {
        assert!(micros(value).is_err());
    }
    assert!(hash_valid(&"f".repeat(64)));
    assert!(!hash_valid(&"A".repeat(64)));
    assert!(!metadata_valid(&json!([])));
}
