//! A receiving API may adopt only an attested physical job with no live owner.
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use super::{
    directory::{self, Owner},
    ApiCluster,
};
use crate::{
    desktop::RunnerControl,
    runner_desktop_proof::{JobBinding, VerifiedJob},
};

pub(crate) async fn binding(pool: &PgPool, session: Uuid) -> Result<JobBinding> {
    let value: Option<Value> = sqlx::query_scalar(
        "SELECT runner_metadata->'runner_job_binding' FROM desktop_sessions WHERE id=$1",
    )
    .bind(session)
    .fetch_one(pool)
    .await?;
    let binding: JobBinding =
        serde_json::from_value(value.context("legacy desktop has no attested physical identity")?)?;
    binding.validate()?;
    ensure!(
        binding.session_id == session,
        "desktop job binding names another session"
    );
    Ok(binding)
}

async fn observed(pool: &PgPool, session: Uuid) -> Result<Option<Owner>> {
    let row=sqlx::query("SELECT group_id,node_id,process_epoch,executor_id,connection_epoch,credential_id FROM api_cluster_desktop_owners WHERE desktop_session_id=$1")
        .bind(session).fetch_optional(pool).await?;
    row.map(|row| {
        Ok(Owner {
            group_id: row.try_get("group_id")?,
            node_id: row.try_get("node_id")?,
            process_epoch: row.try_get("process_epoch")?,
            executor_id: row.try_get("executor_id")?,
            connection_epoch: row.try_get("connection_epoch")?,
            credential_id: row.try_get("credential_id")?,
        })
    })
    .transpose()
}

pub(crate) async fn ensure_owner(
    cluster: &ApiCluster,
    session: Uuid,
    runner: &RunnerControl,
) -> Result<Owner> {
    cluster.ensure_active().await?;
    if let Ok(owner) = directory::desktop_owner(cluster, session).await {
        return Ok(owner);
    }
    let before = observed(cluster.pool(), session).await?;
    if let Some(owner) = &before {
        ensure!(
            owner.group_id == cluster.local_owner().group_id && owner.executor_id.is_none(),
            "only a same-group Linux desktop can be adopted"
        );
    }
    let binding = binding(cluster.pool(), session).await?;
    let fresh = runner.probe_desktop(&binding).await?;
    let local = cluster.local_owner();
    tokio::time::timeout_at(
        fresh.deadline(),
        claim(cluster.pool(), &local, before.as_ref(), &fresh),
    )
    .await??;
    cluster.ensure_active().await?;
    directory::desktop_owner(cluster, session).await
}

async fn lock_nodes(
    tx: &mut Transaction<'_, Postgres>,
    local: &Owner,
    old: Option<&Owner>,
) -> Result<()> {
    let mut nodes = vec![local.node_id];
    if let Some(old) = old {
        nodes.push(old.node_id);
    }
    nodes.sort_unstable();
    nodes.dedup();
    // All API-node locks precede run/session/owner locks, including an old
    // process row. An expired DB lease cannot be renewed while CAS commits.
    for node in nodes {
        let row=sqlx::query("SELECT process_epoch,lease_until>clock_timestamp() AND NOT draining AS live FROM api_cluster_nodes WHERE group_id=$1 AND node_id=$2 FOR SHARE")
            .bind(local.group_id).bind(node).fetch_optional(&mut **tx).await?.context("desktop API node is missing")?;
        let epoch: Uuid = row.try_get("process_epoch")?;
        let live: bool = row.try_get("live")?;
        if node == local.node_id {
            ensure!(
                epoch == local.process_epoch && live,
                "receiving API has no current DB lease"
            );
        }
        if let Some(old) = old.filter(|old| old.node_id == node) {
            ensure!(
                old.process_epoch != epoch || !live,
                "a live desktop API owner cannot be replaced"
            );
        }
    }
    Ok(())
}

async fn claim(
    pool: &PgPool,
    local: &Owner,
    old: Option<&Owner>,
    proof: &VerifiedJob,
) -> Result<()> {
    proof.ensure_fresh()?;
    let expected = proof.binding();
    let mut tx = pool.begin().await?;
    lock_nodes(&mut tx, local, old).await?;
    let row=sqlx::query("SELECT spec,lease_token,lease_expires_at>clock_timestamp() AND state IN ('running','waiting_approval','waiting_input') AND runner_cancel_id IS NULL AS live,sandbox_workspace_id FROM runs WHERE id=$1 FOR UPDATE")
        .bind(expected.run_id).fetch_one(&mut *tx).await?;
    let spec: Value = row.try_get("spec")?;
    ensure!(
        spec["executor_target"]["kind"] == "server_linux"
            && row.try_get::<bool, _>("live")?
            && row.try_get::<Option<Uuid>, _>("lease_token")? == Some(expected.lease_token)
            && row.try_get::<Option<Uuid>, _>("sandbox_workspace_id")? == expected.workspace_id,
        "desktop run lease or workspace was fenced"
    );
    let row = sqlx::query(
        "SELECT run_id,state,runner_metadata FROM desktop_sessions WHERE id=$1 FOR UPDATE",
    )
    .bind(expected.session_id)
    .fetch_one(&mut *tx)
    .await?;
    let state: String = row.try_get("state")?;
    let metadata: Value = row.try_get("runner_metadata")?;
    ensure!(
        row.try_get::<Uuid, _>("run_id")? == expected.run_id
            && ["agent_controlled", "user_controlled", "paused"].contains(&state.as_str())
            && metadata["start_pending"] == false
            && metadata["cleanup_state"] != "stop_pending"
            && metadata["start_lease_token"] == expected.lease_token.to_string()
            && metadata["start_attempt_id"] == expected.attempt_id.to_string()
            && serde_json::from_value::<JobBinding>(metadata["runner_job_binding"].clone())?
                == *expected,
        "desktop physical identity or cleanup state changed"
    );
    let current=sqlx::query("SELECT group_id,node_id,process_epoch,executor_id,connection_epoch,credential_id FROM api_cluster_desktop_owners WHERE desktop_session_id=$1 FOR UPDATE")
        .bind(expected.session_id).fetch_optional(&mut *tx).await?;
    let current = current
        .map(|row| {
            Ok::<Owner, sqlx::Error>(Owner {
                group_id: row.try_get("group_id")?,
                node_id: row.try_get("node_id")?,
                process_epoch: row.try_get("process_epoch")?,
                executor_id: row.try_get("executor_id")?,
                connection_epoch: row.try_get("connection_epoch")?,
                credential_id: row.try_get("credential_id")?,
            })
        })
        .transpose()?;
    ensure!(current.as_ref() == old, "desktop owner changed before CAS");
    let unknown:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cluster_desktop_controls WHERE desktop_session_id=$1 AND claimed_at IS NOT NULL AND completed_at IS NULL)")
        .bind(expected.session_id).fetch_one(&mut *tx).await?;
    ensure!(
        !unknown,
        "desktop has an unresolved physical control outcome"
    );
    sqlx::query("UPDATE api_cluster_desktop_pending SET revoked_at=COALESCE(revoked_at,clock_timestamp()) WHERE desktop_session_id=$1")
        .bind(expected.session_id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO api_cluster_desktop_owners(desktop_session_id,group_id,node_id,process_epoch) VALUES($1,$2,$3,$4) ON CONFLICT(desktop_session_id) DO UPDATE SET group_id=EXCLUDED.group_id,node_id=EXCLUDED.node_id,process_epoch=EXCLUDED.process_epoch,executor_id=NULL,connection_epoch=NULL,credential_id=NULL")
        .bind(expected.session_id).bind(local.group_id).bind(local.node_id).bind(local.process_epoch).execute(&mut *tx).await?;
    proof.ensure_fresh()?;
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn guard_job<T>(
    pool: &PgPool,
    expected: &JobBinding,
    work: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    check_job(pool, expected).await?;
    tokio::pin!(work);
    let cadence = std::time::Duration::from_millis(750);
    let mut checks = tokio::time::interval_at(tokio::time::Instant::now() + cadence, cadence);
    checks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {biased;
            _=checks.tick()=>check_job(pool,expected).await?,
            result=&mut work=>return result,
        }
    }
}

async fn check_job(pool: &PgPool, expected: &JobBinding) -> Result<()> {
    let current:bool=tokio::time::timeout(std::time::Duration::from_secs(2),sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runs r JOIN desktop_sessions d ON d.run_id=r.id WHERE r.id=$1 AND d.id=$2 AND r.lease_token=$3 AND r.lease_expires_at>clock_timestamp() AND r.runner_cancel_id IS NULL AND r.state IN ('running','waiting_approval','waiting_input') AND d.state IN ('agent_controlled','user_controlled','paused') AND d.runner_metadata->'runner_job_binding'=$4::jsonb AND d.runner_metadata->>'start_pending'='false' AND COALESCE(d.runner_metadata->>'cleanup_state','')!='stop_pending' AND NOT EXISTS(SELECT 1 FROM api_cluster_desktop_controls c WHERE c.desktop_session_id=d.id AND c.claimed_at IS NOT NULL AND c.completed_at IS NULL))")
        .bind(expected.run_id).bind(expected.session_id).bind(expected.lease_token).bind(serde_json::to_value(expected)?).fetch_one(pool)).await??;
    ensure!(current, "physical desktop run lease or identity was fenced");
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) struct Fixture {
        pub pool: PgPool,
        pub first: Owner,
        pub second: Owner,
        pub job: JobBinding,
        schema: String,
        url: String,
    }
    impl Fixture {
        pub(crate) async fn new() -> Self {
            let url = std::env::var("COWORK_TEST_DATABASE_URL")
                .expect("isolated PostgreSQL test database");
            let schema = format!("cowork_linux_fixture_{}", Uuid::new_v4().simple());
            let admin = PgPool::connect(&url).await.unwrap();
            sqlx::query(&format!("CREATE SCHEMA {schema}"))
                .execute(&admin)
                .await
                .unwrap();
            admin.close().await;
            let search = format!("SET search_path TO {schema},public");
            let pool = sqlx::postgres::PgPoolOptions::new()
                .max_connections(3)
                .after_connect(move |connection, _| {
                    let search = search.clone();
                    Box::pin(async move {
                        sqlx::query(&search).execute(connection).await?;
                        Ok(())
                    })
                })
                .connect(&url)
                .await
                .unwrap();
            sqlx::raw_sql("CREATE TABLE executors(id uuid PRIMARY KEY,draining boolean NOT NULL DEFAULT false);CREATE TABLE executor_credentials(id uuid PRIMARY KEY,executor_id uuid,revoked_at timestamptz,expires_at timestamptz);CREATE TABLE desktop_sessions(id uuid PRIMARY KEY,run_id uuid,state text,runner_metadata jsonb);CREATE TABLE users(id uuid PRIMARY KEY,deleted_at timestamptz);CREATE TABLE runs(id uuid PRIMARY KEY,spec jsonb,state text,lease_token uuid,lease_expires_at timestamptz,runner_cancel_id uuid,sandbox_workspace_id uuid);CREATE TABLE auth_sessions(id uuid PRIMARY KEY,user_id uuid,revoked_at timestamptz,expires_at timestamptz);CREATE TABLE desktop_stream_tickets(token_hash bytea);")
                .execute(&pool).await.unwrap();
            sqlx::raw_sql(include_str!(
                "../../migrations/0056_api_cluster_routing.sql"
            ))
            .execute(&pool)
            .await
            .unwrap();
            sqlx::raw_sql(include_str!(
                "../../migrations/0058_api_cluster_desktop_controls.sql"
            ))
            .execute(&pool)
            .await
            .unwrap();
            let first = Owner {
                group_id: Uuid::new_v4(),
                node_id: Uuid::new_v4(),
                process_epoch: Uuid::new_v4(),
                executor_id: None,
                connection_epoch: None,
                credential_id: None,
            };
            let second = Owner {
                node_id: Uuid::new_v4(),
                process_epoch: Uuid::new_v4(),
                ..first.clone()
            };
            for owner in [&first, &second] {
                sqlx::query("INSERT INTO api_cluster_nodes(group_id,node_id,process_epoch,certificate_sha256,lease_until) VALUES($1,$2,$3,$4,clock_timestamp()+interval '60 seconds')")
                .bind(owner.group_id).bind(owner.node_id).bind(owner.process_epoch).bind(vec![1_u8;32]).execute(&pool).await.unwrap();
            }
            let job = crate::runner_desktop_proof::tests::binding();
            sqlx::query("INSERT INTO runs(id,spec,state,lease_token,lease_expires_at,sandbox_workspace_id) VALUES($1,'{\"executor_target\":{\"kind\":\"server_linux\"}}','running',$2,clock_timestamp()+interval '60 seconds',$3)")
                .bind(job.run_id).bind(job.lease_token).bind(job.workspace_id).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO desktop_sessions(id,run_id,state,runner_metadata) VALUES($1,$2,'agent_controlled',$3)")
                .bind(job.session_id).bind(job.run_id).bind(serde_json::json!({"start_pending":false,"start_lease_token":job.lease_token,"start_attempt_id":job.attempt_id,"runner_job_binding":job})).execute(&pool).await.unwrap();
            Self {
                pool,
                first,
                second,
                job,
                schema,
                url,
            }
        }
        pub(crate) async fn cleanup(self) {
            self.pool.close().await;
            let admin = PgPool::connect(&self.url).await.unwrap();
            sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
                .execute(&admin)
                .await
                .unwrap();
            admin.close().await;
        }
        async fn proof(&self) -> VerifiedJob {
            crate::runner_desktop_proof::verified_for_test(self.job.clone())
        }
    }
    pub(crate) async fn try_claim(
        fixture: &Fixture,
        local: &Owner,
        old: Option<&Owner>,
    ) -> Result<()> {
        claim(&fixture.pool, local, old, &fixture.proof().await).await
    }
    #[tokio::test]
    #[ignore = "requires COWORK_TEST_DATABASE_URL pointing to isolated PostgreSQL"]
    async fn postgres_physical_desktop_cas_has_one_winner_and_never_steals_a_live_owner() {
        let fixture = Fixture::new().await;
        let first = fixture.proof().await;
        let second = fixture.proof().await;
        let (left, right) = tokio::join!(
            claim(&fixture.pool, &fixture.first, None, &first),
            claim(&fixture.pool, &fixture.second, None, &second)
        );
        assert_ne!(
            left.is_ok(),
            right.is_ok(),
            "exactly one absent-owner contender wins"
        );
        let owner = observed(&fixture.pool, fixture.job.session_id)
            .await
            .unwrap()
            .unwrap();
        let other = if owner == fixture.first {
            &fixture.second
        } else {
            &fixture.first
        };
        assert!(
            claim(&fixture.pool, other, Some(&owner), &fixture.proof().await)
                .await
                .is_err(),
            "live owner must not be stolen even with an authentic job proof"
        );
        sqlx::query("UPDATE api_cluster_nodes SET lease_until=clock_timestamp()-interval '1 second' WHERE node_id=$1").bind(owner.node_id).execute(&fixture.pool).await.unwrap();
        claim(&fixture.pool, other, Some(&owner), &fixture.proof().await)
            .await
            .unwrap();
        assert_eq!(
            observed(&fixture.pool, fixture.job.session_id)
                .await
                .unwrap()
                .unwrap(),
            *other
        );
        fixture.cleanup().await;
    }
    #[tokio::test]
    #[ignore = "requires COWORK_TEST_DATABASE_URL pointing to isolated PostgreSQL"]
    async fn postgres_physical_desktop_cas_rejects_changed_identity_expiry_cleanup_and_cancellation(
    ) {
        for change in ["UPDATE runs SET lease_token=gen_random_uuid()","UPDATE runs SET lease_expires_at=clock_timestamp()-interval '1 second'",
            "UPDATE runs SET runner_cancel_id=gen_random_uuid()","UPDATE runs SET sandbox_workspace_id=gen_random_uuid()",
            "UPDATE desktop_sessions SET runner_metadata=runner_metadata||'{\"cleanup_state\":\"stop_pending\"}'::jsonb",
            "UPDATE desktop_sessions SET runner_metadata=runner_metadata||'{\"start_pending\":true}'::jsonb",
            "UPDATE desktop_sessions SET runner_metadata=jsonb_set(runner_metadata,'{runner_job_binding,engine_id}','\"another-engine\"')"] {
            let fixture=Fixture::new().await;sqlx::query(change).execute(&fixture.pool).await.unwrap();
            assert!(claim(&fixture.pool,&fixture.first,None,&fixture.proof().await).await.is_err(),"changed physical authority must reject CAS: {change}");
            assert!(observed(&fixture.pool,fixture.job.session_id).await.unwrap().is_none());fixture.cleanup().await;
        }
    }
    #[tokio::test]
    #[ignore = "requires COWORK_TEST_DATABASE_URL pointing to isolated PostgreSQL"]
    async fn postgres_physical_desktop_guard_fences_execution_before_expired_lease_work() {
        let fixture = Fixture::new().await;
        sqlx::query("UPDATE runs SET lease_expires_at=clock_timestamp()-interval '1 second'")
            .execute(&fixture.pool)
            .await
            .unwrap();
        let polled = std::sync::atomic::AtomicBool::new(false);
        let result: Result<()> = guard_job(&fixture.pool, &fixture.job, async {
            polled.store(true, std::sync::atomic::Ordering::SeqCst);
            std::future::pending().await
        })
        .await;
        assert!(result.is_err());
        assert!(!polled.load(std::sync::atomic::Ordering::SeqCst));
        fixture.cleanup().await;
    }
}
