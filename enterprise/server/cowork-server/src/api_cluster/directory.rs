//! DB-time leases and exact process/credential/connection fencing.
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use super::{ApiCluster, LEASE_SECONDS};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Owner {
    pub group_id: Uuid,
    pub node_id: Uuid,
    pub process_epoch: Uuid,
    pub executor_id: Option<Uuid>,
    pub connection_epoch: Option<Uuid>,
    pub credential_id: Option<Uuid>,
}

const REGISTER_NODE: &str = "INSERT INTO api_cluster_nodes(group_id,node_id,process_epoch,certificate_sha256,lease_until) \
    VALUES($1,$2,$3,$4,clock_timestamp()+make_interval(secs=>$5)) \
    ON CONFLICT(group_id,node_id) DO UPDATE SET process_epoch=EXCLUDED.process_epoch,certificate_sha256=EXCLUDED.certificate_sha256,\
    lease_until=EXCLUDED.lease_until,draining=false WHERE api_cluster_nodes.lease_until<=clock_timestamp() RETURNING node_id";

const RENEW_NODE:&str="UPDATE api_cluster_nodes SET lease_until=clock_timestamp()+make_interval(secs=>$4) WHERE group_id=$1 AND node_id=$2 AND process_epoch=$3 AND NOT draining AND lease_until>clock_timestamp()";
const CLAIM_EXECUTOR:&str="INSERT INTO api_cluster_executor_owners(executor_id,group_id,node_id,process_epoch,connection_epoch,credential_id,lease_until) VALUES($1,$2,$3,$4,$5,$6,clock_timestamp()+make_interval(secs=>$7)) ON CONFLICT(executor_id) DO UPDATE SET group_id=EXCLUDED.group_id,node_id=EXCLUDED.node_id,process_epoch=EXCLUDED.process_epoch,connection_epoch=EXCLUDED.connection_epoch,credential_id=EXCLUDED.credential_id,lease_until=EXCLUDED.lease_until";

pub(super) async fn register_node(cluster: &ApiCluster) -> Result<()> {
    let mut tx = cluster.inner.pool.begin().await?;
    let group:Uuid=sqlx::query_scalar("INSERT INTO api_cluster_binding(singleton,group_id,protocol_version) VALUES(true,$1,1) ON CONFLICT(singleton) DO UPDATE SET singleton=true RETURNING group_id")
        .bind(cluster.inner.config.group_id).fetch_one(&mut *tx).await?;
    ensure!(
        group == cluster.inner.config.group_id,
        "database is bound to a different API cluster group"
    );
    let acquired = sqlx::query_scalar::<_, Uuid>(REGISTER_NODE)
        .bind(group)
        .bind(cluster.inner.config.node_id)
        .bind(cluster.inner.process_epoch)
        .bind(&cluster.inner.certificate_sha256)
        .bind(LEASE_SECONDS as f64)
        .fetch_optional(&mut *tx)
        .await?;
    ensure!(
        acquired.is_some(),
        "configured API node already has a live process owner"
    );
    tx.commit().await?;
    Ok(())
}

pub(super) async fn renew_node(cluster: &ApiCluster) -> Result<()> {
    let row = sqlx::query(RENEW_NODE)
        .bind(cluster.inner.config.group_id)
        .bind(cluster.inner.config.node_id)
        .bind(cluster.inner.process_epoch)
        .bind(LEASE_SECONDS as f64)
        .execute(&cluster.inner.pool)
        .await?;
    ensure!(
        row.rows_affected() == 1,
        "API node DB lease is no longer owned"
    );
    Ok(())
}

pub(super) async fn release_node(cluster: &ApiCluster) {
    let _=sqlx::query("UPDATE api_cluster_nodes SET draining=true,lease_until=clock_timestamp() WHERE group_id=$1 AND node_id=$2 AND process_epoch=$3")
        .bind(cluster.inner.config.group_id).bind(cluster.inner.config.node_id).bind(cluster.inner.process_epoch)
        .execute(&cluster.inner.pool).await;
}

pub(crate) async fn verify_node(pool: &PgPool, group: Uuid, node: Uuid, epoch: Uuid) -> Result<()> {
    let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cluster_nodes WHERE group_id=$1 AND node_id=$2 AND process_epoch=$3 AND NOT draining AND lease_until>clock_timestamp())")
        .bind(group).bind(node).bind(epoch).fetch_one(pool).await?;
    ensure!(
        valid,
        "private API peer process epoch or lease is no longer current"
    );
    Ok(())
}

async fn lock_node(tx: &mut Transaction<'_, Postgres>, owner: &Owner) -> Result<()> {
    let row=sqlx::query_scalar::<_,Uuid>("SELECT node_id FROM api_cluster_nodes WHERE group_id=$1 AND node_id=$2 AND process_epoch=$3 AND NOT draining AND lease_until>clock_timestamp() FOR SHARE")
        .bind(owner.group_id).bind(owner.node_id).bind(owner.process_epoch).fetch_optional(&mut **tx).await?;
    ensure!(row.is_some(), "private API node owner was fenced");
    Ok(())
}

pub(crate) async fn claim_executor(
    cluster: &ApiCluster,
    executor: Uuid,
    credential: Uuid,
    connection: Uuid,
) -> Result<Owner> {
    cluster.ensure_active().await?;
    let owner = Owner {
        executor_id: Some(executor),
        credential_id: Some(credential),
        connection_epoch: Some(connection),
        ..cluster.local_owner()
    };
    let mut tx = cluster.inner.pool.begin().await?;
    lock_node(&mut tx, &owner).await?;
    let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM executor_credentials c JOIN executors e ON e.id=c.executor_id WHERE c.id=$1 AND e.id=$2 AND c.revoked_at IS NULL AND (c.expires_at IS NULL OR c.expires_at>clock_timestamp()) AND NOT e.draining)")
        .bind(credential).bind(executor).fetch_one(&mut *tx).await?;
    ensure!(valid, "executor credential is not currently authorised");
    sqlx::query(CLAIM_EXECUTOR)
        .bind(executor)
        .bind(owner.group_id)
        .bind(owner.node_id)
        .bind(owner.process_epoch)
        .bind(connection)
        .bind(credential)
        .bind(LEASE_SECONDS as f64)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(owner)
}

pub(crate) async fn renew_executor(cluster: &ApiCluster, owner: &Owner) -> Result<()> {
    let mut tx = cluster.inner.pool.begin().await?;
    fence_owner(&mut tx, owner).await?;
    let changed=sqlx::query("UPDATE api_cluster_executor_owners SET lease_until=clock_timestamp()+make_interval(secs=>$5) WHERE executor_id=$1 AND node_id=$2 AND process_epoch=$3 AND connection_epoch=$4 AND lease_until>clock_timestamp()")
        .bind(owner.executor_id).bind(owner.node_id).bind(owner.process_epoch).bind(owner.connection_epoch).bind(LEASE_SECONDS as f64)
        .execute(&mut *tx).await?;
    ensure!(
        changed.rows_affected() == 1,
        "executor connection owner was replaced"
    );
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn release_executor(cluster: &ApiCluster, owner: &Owner) {
    let _=sqlx::query("DELETE FROM api_cluster_executor_owners WHERE executor_id=$1 AND group_id=$2 AND node_id=$3 AND process_epoch=$4 AND connection_epoch=$5 AND credential_id=$6")
        .bind(owner.executor_id).bind(owner.group_id).bind(owner.node_id).bind(owner.process_epoch).bind(owner.connection_epoch).bind(owner.credential_id)
        .execute(&cluster.inner.pool).await;
}

pub(crate) async fn executor_owner(cluster: &ApiCluster, executor: Uuid) -> Result<Owner> {
    let row=sqlx::query("SELECT group_id,node_id,process_epoch,connection_epoch,credential_id FROM api_cluster_executor_owners WHERE executor_id=$1 AND group_id=$2")
        .bind(executor).bind(cluster.inner.config.group_id).fetch_one(&cluster.inner.pool).await?;
    let owner = Owner {
        group_id: row.try_get("group_id")?,
        node_id: row.try_get("node_id")?,
        process_epoch: row.try_get("process_epoch")?,
        executor_id: Some(executor),
        connection_epoch: Some(row.try_get("connection_epoch")?),
        credential_id: Some(row.try_get("credential_id")?),
    };
    verify_owner(cluster, &owner).await?;
    Ok(owner)
}

pub(crate) async fn fence_owner(tx: &mut Transaction<'_, Postgres>, owner: &Owner) -> Result<()> {
    lock_node(tx, owner).await?;
    if let Some(executor) = owner.executor_id {
        let row=sqlx::query_scalar::<_,Uuid>("SELECT o.executor_id FROM api_cluster_executor_owners o JOIN executor_credentials c ON c.id=o.credential_id AND c.executor_id=o.executor_id JOIN executors e ON e.id=o.executor_id WHERE o.executor_id=$1 AND o.group_id=$2 AND o.node_id=$3 AND o.process_epoch=$4 AND o.connection_epoch=$5 AND o.credential_id=$6 AND o.lease_until>clock_timestamp() AND c.revoked_at IS NULL AND (c.expires_at IS NULL OR c.expires_at>clock_timestamp()) AND NOT e.draining FOR UPDATE OF o")
            .bind(executor).bind(owner.group_id).bind(owner.node_id).bind(owner.process_epoch).bind(owner.connection_epoch).bind(owner.credential_id)
            .fetch_optional(&mut **tx).await?;
        ensure!(
            row.is_some(),
            "executor credential/connection epoch is no longer owned"
        );
    } else {
        ensure!(
            owner.connection_epoch.is_none() && owner.credential_id.is_none(),
            "invalid private owner shape"
        );
    }
    Ok(())
}

pub(crate) async fn verify_owner(cluster: &ApiCluster, owner: &Owner) -> Result<()> {
    ensure!(
        owner.group_id == cluster.inner.config.group_id,
        "private owner group mismatch"
    );
    cluster.inner.config.peer(owner.node_id)?;
    let mut tx = cluster.inner.pool.begin().await?;
    fence_owner(&mut tx, owner).await?;
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn bind_desktop(
    cluster: &ApiCluster,
    session: Uuid,
    executor: Option<Uuid>,
) -> Result<Owner> {
    let owner = if let Some(executor) = executor {
        executor_owner(cluster, executor).await?
    } else {
        cluster.local_owner()
    };
    let mut tx = cluster.inner.pool.begin().await?;
    fence_owner(&mut tx, &owner).await?;
    sqlx::query("INSERT INTO api_cluster_desktop_owners(desktop_session_id,group_id,node_id,process_epoch,executor_id,connection_epoch,credential_id) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(desktop_session_id) DO NOTHING")
        .bind(session).bind(owner.group_id).bind(owner.node_id).bind(owner.process_epoch).bind(owner.executor_id).bind(owner.connection_epoch).bind(owner.credential_id)
        .execute(&mut *tx).await?;
    tx.commit().await?;
    desktop_owner(cluster, session).await
}

pub(crate) async fn desktop_owner(cluster: &ApiCluster, session: Uuid) -> Result<Owner> {
    let row=sqlx::query("SELECT group_id,node_id,process_epoch,executor_id,connection_epoch,credential_id FROM api_cluster_desktop_owners WHERE desktop_session_id=$1 AND group_id=$2")
        .bind(session).bind(cluster.inner.config.group_id).fetch_one(&cluster.inner.pool).await?;
    let owner = Owner {
        group_id: row.try_get("group_id")?,
        node_id: row.try_get("node_id")?,
        process_epoch: row.try_get("process_epoch")?,
        executor_id: row.try_get("executor_id")?,
        connection_epoch: row.try_get("connection_epoch")?,
        credential_id: row.try_get("credential_id")?,
    };
    verify_owner(cluster, &owner).await?;
    Ok(owner)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) async fn fixture() -> (PgPool, Owner) {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(
                &std::env::var("COWORK_TEST_DATABASE_URL")
                    .expect("isolated PostgreSQL test database"),
            )
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TEMP TABLE executors(id uuid PRIMARY KEY,draining boolean NOT NULL DEFAULT false); CREATE TEMP TABLE executor_credentials(id uuid PRIMARY KEY,executor_id uuid,revoked_at timestamptz,expires_at timestamptz); CREATE TEMP TABLE desktop_sessions(id uuid PRIMARY KEY); CREATE TEMP TABLE users(id uuid PRIMARY KEY); CREATE TEMP TABLE runs(id uuid PRIMARY KEY); CREATE TEMP TABLE auth_sessions(id uuid PRIMARY KEY); CREATE TEMP TABLE desktop_stream_tickets(token_hash bytea);")
            .execute(&pool).await.unwrap();
        // Run the exact production constraints in connection-local tables.
        let migration = include_str!("../../migrations/0056_api_cluster_routing.sql")
            .replace("CREATE TABLE ", "CREATE TEMP TABLE ");
        sqlx::raw_sql(&migration).execute(&pool).await.unwrap();
        let owner = Owner {
            group_id: Uuid::new_v4(),
            node_id: Uuid::new_v4(),
            process_epoch: Uuid::new_v4(),
            executor_id: Some(Uuid::new_v4()),
            connection_epoch: Some(Uuid::new_v4()),
            credential_id: Some(Uuid::new_v4()),
        };
        sqlx::query("INSERT INTO executors(id) VALUES($1)")
            .bind(owner.executor_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO executor_credentials(id,executor_id) VALUES($1,$2)")
            .bind(owner.credential_id)
            .bind(owner.executor_id)
            .execute(&pool)
            .await
            .unwrap();
        register(&pool, &owner).await.unwrap();
        claim(&pool, &owner).await;
        (pool, owner)
    }
    async fn register(pool: &PgPool, owner: &Owner) -> Option<Uuid> {
        sqlx::query_scalar(REGISTER_NODE)
            .bind(owner.group_id)
            .bind(owner.node_id)
            .bind(owner.process_epoch)
            .bind(vec![1_u8; 32])
            .bind(8_f64)
            .fetch_optional(pool)
            .await
            .unwrap()
    }
    async fn claim(pool: &PgPool, owner: &Owner) {
        sqlx::query(CLAIM_EXECUTOR)
            .bind(owner.executor_id)
            .bind(owner.group_id)
            .bind(owner.node_id)
            .bind(owner.process_epoch)
            .bind(owner.connection_epoch)
            .bind(owner.credential_id)
            .bind(8_f64)
            .execute(pool)
            .await
            .unwrap();
    }
    async fn valid(pool: &PgPool, owner: &Owner) -> bool {
        let mut tx = pool.begin().await.unwrap();
        let result = fence_owner(&mut tx, owner).await.is_ok();
        tx.rollback().await.unwrap();
        result
    }
    #[tokio::test]
    #[ignore = "requires COWORK_TEST_DATABASE_URL pointing to isolated PostgreSQL"]
    async fn postgres_cluster_process_connection_and_credential_epochs_are_fenced() {
        let (pool, old) = fixture().await;
        assert!(valid(&pool, &old).await);
        let mut wrong = old.clone();
        wrong.group_id = Uuid::new_v4();
        assert!(!valid(&pool, &wrong).await);
        let mut replacement = old.clone();
        replacement.connection_epoch = Some(Uuid::new_v4());
        claim(&pool, &replacement).await;
        assert!(!valid(&pool, &old).await);
        assert!(valid(&pool, &replacement).await);
        sqlx::query("UPDATE executor_credentials SET revoked_at=clock_timestamp() WHERE id=$1")
            .bind(old.credential_id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(!valid(&pool, &replacement).await);
        sqlx::query("UPDATE executor_credentials SET revoked_at=NULL WHERE id=$1")
            .bind(old.credential_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE api_cluster_executor_owners SET lease_until=clock_timestamp()-interval '1 second'").execute(&pool).await.unwrap();
        assert!(!valid(&pool, &replacement).await);
        claim(&pool, &replacement).await;
        let mut successor = replacement.clone();
        successor.process_epoch = Uuid::new_v4();
        assert!(register(&pool, &successor).await.is_none());
        sqlx::query(
            "UPDATE api_cluster_nodes SET lease_until=clock_timestamp()-interval '1 second'",
        )
        .execute(&pool)
        .await
        .unwrap();
        let changed = sqlx::query(RENEW_NODE)
            .bind(old.group_id)
            .bind(old.node_id)
            .bind(old.process_epoch)
            .bind(8_f64)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            changed.rows_affected(),
            0,
            "expired ownership cannot revive"
        );
        assert!(register(&pool, &successor).await.is_some());
        assert!(!valid(&pool, &replacement).await);
        claim(&pool, &successor).await;
        assert!(valid(&pool, &successor).await);
        let minted:bool=sqlx::query_scalar("SELECT lease_until>clock_timestamp() AND lease_until<=clock_timestamp()+interval '8 seconds' FROM api_cluster_nodes").fetch_one(&pool).await.unwrap();
        assert!(minted);
        pool.close().await;
    }
}
