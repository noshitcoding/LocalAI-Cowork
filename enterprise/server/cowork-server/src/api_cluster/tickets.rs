//! Durable, single-use private slots; a failed/unknown dispatch is never retried.
use anyhow::{ensure, Result};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use super::{
    directory::{self, Owner},
    ApiCluster,
};

#[derive(Clone, Debug)]
pub(crate) struct Pending {
    pub stream_id: Uuid,
    pub owner: Owner,
    pub origin_node_id: Uuid,
    pub origin_process_epoch: Uuid,
    pub session_id: Uuid,
    pub run_id: Uuid,
    pub user_id: Uuid,
    pub auth_session_id: Uuid,
    pub control: bool,
}

pub(crate) async fn prepare(
    cluster: &ApiCluster,
    session: Uuid,
    run: Uuid,
    user: Uuid,
    auth_session: Uuid,
    control: bool,
    external: Option<Uuid>,
) -> Result<Pending> {
    cluster.ensure_active().await?;
    // A creator or the attested Linux CAS must have established this owner.
    // Configured URLs alone never grant ownership of a worker-created job.
    let owner = directory::desktop_owner(cluster, session).await?;
    ensure!(
        owner.executor_id == external,
        "desktop transport does not match its durable owner"
    );
    let pending = Pending {
        stream_id: Uuid::new_v4(),
        owner,
        origin_node_id: cluster.inner.config.node_id,
        origin_process_epoch: cluster.inner.process_epoch,
        session_id: session,
        run_id: run,
        user_id: user,
        auth_session_id: auth_session,
        control,
    };
    authorise(cluster, &pending).await?;
    let mut tx = cluster.inner.pool.begin().await?;
    directory::fence_owner(&mut tx, &pending.owner).await?;
    sqlx::query("INSERT INTO api_cluster_desktop_pending(stream_id,group_id,origin_node_id,origin_process_epoch,owner_node_id,owner_process_epoch,executor_id,connection_epoch,credential_id,desktop_session_id,run_id,user_id,auth_session_id,control,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,clock_timestamp()+interval '5 minutes')")
        .bind(pending.stream_id).bind(pending.owner.group_id).bind(pending.origin_node_id).bind(pending.origin_process_epoch)
        .bind(pending.owner.node_id).bind(pending.owner.process_epoch).bind(pending.owner.executor_id).bind(pending.owner.connection_epoch)
        .bind(pending.owner.credential_id).bind(session).bind(run).bind(user).bind(auth_session).bind(control).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(pending)
}

pub(crate) async fn load(cluster: &ApiCluster, stream: Uuid) -> Result<Pending> {
    let row=sqlx::query("SELECT * FROM api_cluster_desktop_pending WHERE stream_id=$1 AND group_id=$2 AND revoked_at IS NULL")
        .bind(stream).bind(cluster.inner.config.group_id).fetch_one(&cluster.inner.pool).await?;
    Ok(Pending {
        stream_id: stream,
        owner: Owner {
            group_id: row.try_get("group_id")?,
            node_id: row.try_get("owner_node_id")?,
            process_epoch: row.try_get("owner_process_epoch")?,
            executor_id: row.try_get("executor_id")?,
            connection_epoch: row.try_get("connection_epoch")?,
            credential_id: row.try_get("credential_id")?,
        },
        origin_node_id: row.try_get("origin_node_id")?,
        origin_process_epoch: row.try_get("origin_process_epoch")?,
        session_id: row.try_get("desktop_session_id")?,
        run_id: row.try_get("run_id")?,
        user_id: row.try_get("user_id")?,
        auth_session_id: row.try_get("auth_session_id")?,
        control: row.try_get("control")?,
    })
}

pub(crate) async fn authorise(cluster: &ApiCluster, pending: &Pending) -> Result<()> {
    cluster.ensure_active().await?;
    directory::verify_node(
        &cluster.inner.pool,
        pending.owner.group_id,
        pending.origin_node_id,
        pending.origin_process_epoch,
    )
    .await?;
    directory::verify_owner(cluster, &pending.owner).await?;
    let live:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM auth_sessions s JOIN users u ON u.id=s.user_id WHERE s.id=$1 AND s.user_id=$2 AND s.revoked_at IS NULL AND s.expires_at>clock_timestamp() AND u.deleted_at IS NULL)")
        .bind(pending.auth_session_id).bind(pending.user_id).fetch_one(&cluster.inner.pool).await?;
    ensure!(live, "desktop user session is no longer authorised");
    crate::desktop::cluster_stream_access(
        &cluster.inner.pool,
        pending.session_id,
        pending.run_id,
        pending.user_id,
        pending.control,
    )
    .await?;
    if let (Some(executor_id), Some(credential_id)) =
        (pending.owner.executor_id, pending.owner.credential_id)
    {
        crate::auth::validate_executor_principal(
            &cluster.inner.pool,
            &crate::auth::ExecutorPrincipal {
                executor_id,
                credential_id,
            },
        )
        .await?;
    }
    let bound:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cluster_desktop_owners o JOIN desktop_sessions d ON d.id=o.desktop_session_id JOIN runs r ON r.id=d.run_id WHERE o.desktop_session_id=$1 AND o.group_id=$2 AND o.node_id=$3 AND o.process_epoch=$4 AND o.executor_id IS NOT DISTINCT FROM $5 AND o.connection_epoch IS NOT DISTINCT FROM $6 AND o.credential_id IS NOT DISTINCT FROM $7 AND ($5::uuid IS NULL OR (d.executor_id=$5 AND r.assigned_executor_id=$5)))")
        .bind(pending.session_id).bind(pending.owner.group_id).bind(pending.owner.node_id).bind(pending.owner.process_epoch)
        .bind(pending.owner.executor_id).bind(pending.owner.connection_epoch).bind(pending.owner.credential_id).fetch_one(&cluster.inner.pool).await?;
    ensure!(bound, "desktop session owner was replaced");
    Ok(())
}

pub(crate) async fn guard<T>(
    cluster: &ApiCluster,
    pending: &Pending,
    operation: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    tokio::pin!(operation);
    let mut checks = tokio::time::interval(std::time::Duration::from_millis(750));
    checks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {biased;
            _=cluster.wait_until_lost()=>anyhow::bail!("cluster API ownership lost"),
            _=checks.tick()=>{
                tokio::time::timeout(std::time::Duration::from_secs(2),authorise(cluster,pending)).await??;
                let live:bool=tokio::time::timeout(std::time::Duration::from_secs(2),sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cluster_desktop_pending WHERE stream_id=$1 AND group_id=$2 AND revoked_at IS NULL)").bind(pending.stream_id).bind(pending.owner.group_id).fetch_one(&cluster.inner.pool)).await??;
                ensure!(live,"private desktop stream was revoked");
            },
            outcome=&mut operation=>return outcome,
        }
    }
}

async fn lock_pending(tx: &mut Transaction<'_, Postgres>, pending: &Pending) -> Result<()> {
    directory::fence_owner(tx, &pending.owner).await?;
    let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cluster_nodes WHERE group_id=$1 AND node_id=$2 AND process_epoch=$3 AND NOT draining AND lease_until>clock_timestamp())")
        .bind(pending.owner.group_id).bind(pending.origin_node_id).bind(pending.origin_process_epoch).fetch_one(&mut **tx).await?;
    ensure!(valid, "private desktop origin API was replaced");
    Ok(())
}

#[derive(Clone, Copy)]
enum Slot {
    User,
    Command,
    Send,
    Callback,
}
async fn consume(tx: &mut Transaction<'_, Postgres>, pending: &Pending, slot: Slot) -> Result<()> {
    lock_pending(tx, pending).await?;
    let statement=match slot {
        Slot::User=>"UPDATE api_cluster_desktop_pending SET user_claimed_at=clock_timestamp() WHERE stream_id=$1 AND group_id=$2 AND user_claimed_at IS NULL AND revoked_at IS NULL AND expires_at>clock_timestamp()",
        Slot::Command=>"UPDATE api_cluster_desktop_pending SET command_claimed_at=clock_timestamp() WHERE stream_id=$1 AND group_id=$2 AND user_claimed_at IS NOT NULL AND command_claimed_at IS NULL AND revoked_at IS NULL AND expires_at>clock_timestamp()",
        Slot::Send=>"UPDATE api_cluster_desktop_pending SET command_sent_at=clock_timestamp() WHERE stream_id=$1 AND group_id=$2 AND command_claimed_at IS NOT NULL AND command_sent_at IS NULL AND revoked_at IS NULL AND expires_at>clock_timestamp()",
        Slot::Callback=>"UPDATE api_cluster_desktop_pending SET callback_claimed_at=clock_timestamp() WHERE stream_id=$1 AND group_id=$2 AND command_sent_at IS NOT NULL AND callback_claimed_at IS NULL AND revoked_at IS NULL AND expires_at>clock_timestamp()",
    };
    let used = sqlx::query(statement)
        .bind(pending.stream_id)
        .bind(pending.owner.group_id)
        .execute(&mut **tx)
        .await?;
    ensure!(
        used.rows_affected() == 1,
        "private desktop slot already consumed, out of order, revoked, or expired"
    );
    Ok(())
}

// Hold the exact connection owner row until the one bounded WebSocket send
// finishes. A reconnect cannot commit new ownership while this command emits.
// Consume before emission, including unknown send/commit outcomes: no replay.
pub(crate) async fn dispatch(
    cluster: &ApiCluster,
    pending: &Pending,
    socket: &mut axum::extract::ws::WebSocket,
    message: &cowork_contracts::ExecutorServerMessage,
) -> Result<()> {
    authorise(cluster, pending).await?;
    let mut tx = cluster.pool().begin().await?;
    consume(&mut tx, pending, Slot::Send).await?;
    // Mark used durably first. The locked owner is retained in a second fenced
    // transaction across the send, so an unknown outcome never resets the slot.
    tx.commit().await?;
    let mut tx = cluster.pool().begin().await?;
    lock_pending(&mut tx, pending).await?;
    let still_live=sqlx::query_scalar::<_,Uuid>("SELECT stream_id FROM api_cluster_desktop_pending WHERE stream_id=$1 AND group_id=$2 AND command_sent_at IS NOT NULL AND revoked_at IS NULL AND expires_at>clock_timestamp() FOR SHARE")
        .bind(pending.stream_id).bind(pending.owner.group_id).fetch_optional(&mut *tx).await?;
    ensure!(
        still_live.is_some(),
        "private desktop stream was cancelled before dispatch"
    );
    let encoded = serde_json::to_string(message)?;
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        socket.send(axum::extract::ws::Message::Text(encoded.into())),
    )
    .await;
    tx.commit().await?;
    outcome??;
    Ok(())
}

pub(crate) async fn claim_user(
    cluster: &ApiCluster,
    pending: &Pending,
    source_node: Uuid,
    source_epoch: Uuid,
) -> Result<()> {
    ensure!(
        source_node == pending.origin_node_id && source_epoch == pending.origin_process_epoch,
        "private user bridge belongs to a different API process"
    );
    ensure!(
        pending.owner.node_id == cluster.inner.config.node_id
            && pending.owner.process_epoch == cluster.inner.process_epoch,
        "private bridge target owner changed"
    );
    authorise(cluster, pending).await?;
    let mut tx = cluster.inner.pool.begin().await?;
    consume(&mut tx, pending, Slot::User).await?;
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn claim_command(cluster: &ApiCluster, pending: &Pending) -> Result<()> {
    authorise(cluster, pending).await?;
    let mut tx = cluster.inner.pool.begin().await?;
    consume(&mut tx, pending, Slot::Command).await?;
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn claim_callback(
    cluster: &ApiCluster,
    pending: &Pending,
    principal: &crate::auth::ExecutorPrincipal,
) -> Result<()> {
    ensure!(
        pending.owner.executor_id == Some(principal.executor_id)
            && pending.owner.credential_id == Some(principal.credential_id),
        "executor callback credential does not match pending connection epoch"
    );
    authorise(cluster, pending).await?;
    let mut tx = cluster.inner.pool.begin().await?;
    consume(&mut tx, pending, Slot::Callback).await?;
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn cancel(cluster: &ApiCluster, stream: Uuid) {
    let _=tokio::time::timeout(std::time::Duration::from_secs(2),sqlx::query("UPDATE api_cluster_desktop_pending SET revoked_at=clock_timestamp() WHERE stream_id=$1 AND group_id=$2 AND revoked_at IS NULL")
        .bind(stream).bind(cluster.inner.config.group_id).execute(&cluster.inner.pool)).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn take(pool: &sqlx::PgPool, pending: &Pending, slot: Slot) -> bool {
        let mut tx = pool.begin().await.unwrap();
        let accepted = consume(&mut tx, pending, slot).await.is_ok();
        if accepted {
            tx.commit().await.unwrap();
        } else {
            tx.rollback().await.unwrap();
        }
        accepted
    }
    #[tokio::test]
    #[ignore = "requires COWORK_TEST_DATABASE_URL pointing to isolated PostgreSQL"]
    async fn postgres_cluster_desktop_slots_are_ordered_one_use_expiring_and_owner_fenced() {
        let (pool, owner) = directory::tests::fixture().await;
        let pending = Pending {
            stream_id: Uuid::new_v4(),
            origin_node_id: owner.node_id,
            origin_process_epoch: owner.process_epoch,
            owner,
            session_id: Uuid::new_v4(),
            run_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            auth_session_id: Uuid::new_v4(),
            control: true,
        };
        for (table, id) in [
            ("desktop_sessions", pending.session_id),
            ("runs", pending.run_id),
            ("users", pending.user_id),
            ("auth_sessions", pending.auth_session_id),
        ] {
            sqlx::query(&format!("INSERT INTO {table}(id) VALUES($1)"))
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
        }
        sqlx::query("INSERT INTO api_cluster_desktop_pending(stream_id,group_id,origin_node_id,origin_process_epoch,owner_node_id,owner_process_epoch,executor_id,connection_epoch,credential_id,desktop_session_id,run_id,user_id,auth_session_id,control,expires_at) VALUES($1,$2,$3,$4,$3,$4,$5,$6,$7,$8,$9,$10,$11,true,clock_timestamp()+interval '5 minutes')")
            .bind(pending.stream_id).bind(pending.owner.group_id).bind(pending.owner.node_id).bind(pending.owner.process_epoch).bind(pending.owner.executor_id)
            .bind(pending.owner.connection_epoch).bind(pending.owner.credential_id).bind(pending.session_id).bind(pending.run_id).bind(pending.user_id).bind(pending.auth_session_id).execute(&pool).await.unwrap();
        assert!(!take(&pool, &pending, Slot::Command).await);
        assert!(!take(&pool, &pending, Slot::Callback).await);
        assert!(take(&pool, &pending, Slot::User).await);
        assert!(!take(&pool, &pending, Slot::User).await);
        assert!(take(&pool, &pending, Slot::Command).await);
        assert!(!take(&pool, &pending, Slot::Command).await);
        assert!(
            !take(&pool, &pending, Slot::Callback).await,
            "callback cannot precede actual dispatch consumption"
        );
        assert!(take(&pool, &pending, Slot::Send).await);
        assert!(
            !take(&pool, &pending, Slot::Send).await,
            "unknown send outcome never authorises resend"
        );
        sqlx::query("UPDATE api_cluster_executor_owners SET connection_epoch=$1")
            .bind(Uuid::new_v4())
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            !take(&pool, &pending, Slot::Callback).await,
            "replacement fences in-flight callback"
        );
        sqlx::query("UPDATE api_cluster_executor_owners SET connection_epoch=$1")
            .bind(pending.owner.connection_epoch)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE api_cluster_desktop_pending SET expires_at=clock_timestamp()-interval '1 second'").execute(&pool).await.unwrap();
        assert!(!take(&pool, &pending, Slot::Callback).await);
        sqlx::query("UPDATE api_cluster_desktop_pending SET expires_at=clock_timestamp()+interval '1 minute',revoked_at=clock_timestamp()").execute(&pool).await.unwrap();
        assert!(!take(&pool, &pending, Slot::Callback).await);
        sqlx::query("UPDATE api_cluster_desktop_pending SET revoked_at=NULL")
            .execute(&pool)
            .await
            .unwrap();
        assert!(take(&pool, &pending, Slot::Callback).await);
        assert!(!take(&pool, &pending, Slot::Callback).await);
        pool.close().await;
    }
}
