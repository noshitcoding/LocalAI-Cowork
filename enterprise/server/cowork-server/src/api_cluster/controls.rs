//! Single-use, purpose-bound mTLS control tickets. Unknown stops are not resent.
use super::{
    directory::{self, Owner},
    linux,
    transport::{self, PeerSocket, Request, Role},
    ApiCluster,
};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use uuid::Uuid;

struct Control {
    id: Uuid,
    owner: Owner,
    origin: Uuid,
    origin_epoch: Uuid,
    session: Uuid,
    run: Uuid,
    user: Uuid,
    auth: Uuid,
    binding_sha: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Outcome {
    protocol: u8,
    stopped: bool,
}

const CLAIM_CONTROL:&str="UPDATE api_cluster_desktop_controls c SET claimed_at=clock_timestamp() WHERE c.ticket_id=$1 AND c.purpose='stop' AND c.claimed_at IS NULL AND c.completed_at IS NULL AND c.expires_at>clock_timestamp() \
    AND EXISTS(SELECT 1 FROM api_cluster_nodes n WHERE n.group_id=c.group_id AND n.node_id=c.owner_node_id AND n.process_epoch=c.owner_process_epoch AND NOT n.draining AND n.lease_until>clock_timestamp()) \
    AND EXISTS(SELECT 1 FROM api_cluster_nodes n WHERE n.group_id=c.group_id AND n.node_id=c.origin_node_id AND n.process_epoch=c.origin_process_epoch AND NOT n.draining AND n.lease_until>clock_timestamp()) \
    AND EXISTS(SELECT 1 FROM auth_sessions s JOIN users u ON u.id=s.user_id WHERE s.id=c.auth_session_id AND s.user_id=c.user_id AND s.revoked_at IS NULL AND s.expires_at>clock_timestamp() AND u.deleted_at IS NULL) \
    AND EXISTS(SELECT 1 FROM api_cluster_desktop_owners o WHERE o.desktop_session_id=c.desktop_session_id AND o.group_id=c.group_id AND o.node_id=c.owner_node_id AND o.process_epoch=c.owner_process_epoch AND o.executor_id IS NULL) RETURNING c.ticket_id";

async fn load(cluster: &ApiCluster, id: Uuid) -> Result<Control> {
    let row=sqlx::query("SELECT * FROM api_cluster_desktop_controls WHERE ticket_id=$1 AND group_id=$2 AND purpose='stop'")
        .bind(id).bind(cluster.local_owner().group_id).fetch_one(cluster.pool()).await?;
    Ok(Control {
        id,
        owner: Owner {
            group_id: row.try_get("group_id")?,
            node_id: row.try_get("owner_node_id")?,
            process_epoch: row.try_get("owner_process_epoch")?,
            executor_id: None,
            connection_epoch: None,
            credential_id: None,
        },
        origin: row.try_get("origin_node_id")?,
        origin_epoch: row.try_get("origin_process_epoch")?,
        session: row.try_get("desktop_session_id")?,
        run: row.try_get("run_id")?,
        user: row.try_get("user_id")?,
        auth: row.try_get("auth_session_id")?,
        binding_sha: row.try_get("binding_sha256")?,
    })
}
async fn authorise(cluster: &ApiCluster, control: &Control) -> Result<()> {
    cluster.ensure_active().await?;
    directory::verify_node(
        cluster.pool(),
        control.owner.group_id,
        control.origin,
        control.origin_epoch,
    )
    .await?;
    directory::verify_owner(cluster, &control.owner).await?;
    let live:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM auth_sessions s JOIN users u ON u.id=s.user_id WHERE s.id=$1 AND s.user_id=$2 AND s.revoked_at IS NULL AND s.expires_at>clock_timestamp() AND u.deleted_at IS NULL)")
        .bind(control.auth).bind(control.user).fetch_one(cluster.pool()).await?;
    ensure!(live, "desktop stop user session is no longer authorised");
    crate::fleet_content::interactive_access(
        cluster.pool(),
        control.user,
        control.run,
        cowork_contracts::ProjectRole::Viewer,
    )
    .await?;
    let owner = directory::desktop_owner(cluster, control.session).await?;
    ensure!(
        owner == control.owner
            && linux::binding(cluster.pool(), control.session)
                .await?
                .digest()?
                == control.binding_sha,
        "desktop stop physical owner changed"
    );
    Ok(())
}

pub(crate) async fn stop(
    cluster: &ApiCluster,
    session: Uuid,
    run: Uuid,
    user: Uuid,
    auth: Uuid,
    runner: &crate::desktop::RunnerControl,
) -> Result<()> {
    let owner = linux::ensure_owner(cluster, session, runner).await?;
    ensure!(
        owner.executor_id.is_none(),
        "private Linux stop cannot target an executor"
    );
    let control = Control {
        id: Uuid::new_v4(),
        owner,
        origin: cluster.local_owner().node_id,
        origin_epoch: cluster.local_owner().process_epoch,
        session,
        run,
        user,
        auth,
        binding_sha: linux::binding(cluster.pool(), session).await?.digest()?,
    };
    authorise(cluster, &control).await?;
    let mut tx = cluster.pool().begin().await?;
    directory::fence_owner(&mut tx, &control.owner).await?;
    sqlx::query("INSERT INTO api_cluster_desktop_controls(ticket_id,group_id,origin_node_id,origin_process_epoch,owner_node_id,owner_process_epoch,desktop_session_id,run_id,user_id,auth_session_id,purpose,binding_sha256,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,'stop',$11,clock_timestamp()+interval '30 seconds')")
        .bind(control.id).bind(control.owner.group_id).bind(control.origin).bind(control.origin_epoch).bind(control.owner.node_id).bind(control.owner.process_epoch)
        .bind(session).bind(run).bind(user).bind(auth).bind(&control.binding_sha).execute(&mut *tx).await?;
    tx.commit().await?;
    let request = Request {
        protocol: 1,
        group_id: control.owner.group_id,
        source_node_id: control.origin,
        source_process_epoch: control.origin_epoch,
        target_node_id: control.owner.node_id,
        target_process_epoch: control.owner.process_epoch,
        stream_id: control.id,
        role: Role::DesktopStop,
        executor_id: None,
        credential_id: None,
    };
    guard(cluster, &control, async {
        let mut peer = transport::open(cluster, &request).await?;
        let result: Outcome = tokio::time::timeout(
            std::time::Duration::from_secs(18),
            transport::read_initial(&mut peer),
        )
        .await??;
        ensure!(
            result.protocol == 1 && result.stopped,
            "private desktop stop remains unconfirmed"
        );
        Ok(())
    })
    .await
}

async fn claim(cluster: &ApiCluster, control: &Control) -> Result<bool> {
    authorise(cluster, control).await?;
    let mut tx = cluster.pool().begin().await?;
    directory::fence_owner(&mut tx, &control.owner).await?;
    sqlx::query("SELECT id FROM runs WHERE id=$1 FOR UPDATE")
        .bind(control.run)
        .fetch_one(&mut *tx)
        .await?;
    let session = sqlx::query(
        "SELECT run_id,state,runner_metadata FROM desktop_sessions WHERE id=$1 FOR UPDATE",
    )
    .bind(control.session)
    .fetch_one(&mut *tx)
    .await?;
    ensure!(
        session.try_get::<Uuid, _>("run_id")? == control.run,
        "desktop stop names another run"
    );
    let row=sqlx::query("SELECT claimed_at IS NOT NULL AS claimed,completed_at IS NOT NULL AS completed,expires_at>clock_timestamp() AS live FROM api_cluster_desktop_controls WHERE ticket_id=$1 AND purpose='stop' FOR UPDATE")
        .bind(control.id).fetch_one(&mut *tx).await?;
    // Replay is a status query only. It never reissues the physical DELETE.
    if row.try_get::<bool, _>("completed")? {
        tx.commit().await?;
        return Ok(false);
    }
    ensure!(
        row.try_get::<bool, _>("live")? && !row.try_get::<bool, _>("claimed")?,
        "desktop stop ticket is expired or its outcome is unknown"
    );
    let metadata: serde_json::Value = session.try_get("runner_metadata")?;
    ensure!(
        ["agent_controlled", "user_controlled", "paused"]
            .contains(&session.try_get::<String, _>("state")?.as_str()),
        "desktop stop session is no longer active"
    );
    ensure!(
        metadata["start_pending"] == false
            && metadata["cleanup_state"] != "stop_pending"
            && serde_json::from_value::<crate::runner_desktop_proof::JobBinding>(
                metadata["runner_job_binding"].clone()
            )?
            .digest()?
                == control.binding_sha,
        "desktop stop identity is uncertain"
    );
    let unknown:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cluster_desktop_controls WHERE desktop_session_id=$1 AND claimed_at IS NOT NULL AND completed_at IS NULL)")
        .bind(control.session).fetch_one(&mut *tx).await?;
    ensure!(!unknown, "another desktop stop outcome is unknown");
    let consumed = sqlx::query_scalar::<_, Uuid>(CLAIM_CONTROL)
        .bind(control.id)
        .fetch_optional(&mut *tx)
        .await?;
    ensure!(
        consumed.is_some(),
        "desktop stop authority was fenced before claim"
    );
    tx.commit().await?;
    Ok(true)
}

pub(super) async fn handle(
    cluster: &ApiCluster,
    runner: Option<&crate::desktop::RunnerControl>,
    mut socket: PeerSocket,
    request: &Request,
) -> Result<()> {
    ensure!(
        request.executor_id.is_none() && request.credential_id.is_none(),
        "desktop stop cannot supply executor credentials"
    );
    let control = load(cluster, request.stream_id).await?;
    ensure!(
        control.owner == cluster.local_owner()
            && control.origin == request.source_node_id
            && control.origin_epoch == request.source_process_epoch,
        "desktop stop ticket belongs to another peer or process"
    );
    let runner = runner.context("desktop stop owner has no configured Runner")?;
    let dispatch = claim(cluster, &control).await?;
    transport::acknowledge(&mut socket).await?;
    if dispatch {
        guard(
            cluster,
            &control,
            crate::desktop::stop_cluster_session(
                cluster.pool(),
                runner,
                control.run,
                control.session,
                control.user,
                &control.binding_sha,
            ),
        )
        .await?;
        let mut tx = cluster.pool().begin().await?;
        directory::fence_owner(&mut tx, &control.owner).await?;
        let ended:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM desktop_sessions WHERE id=$1 AND run_id=$2 AND state='ended' AND runner_metadata->>'cleanup_state'='confirmed')")
            .bind(control.session).bind(control.run).fetch_one(&mut *tx).await?;
        ensure!(ended, "physical desktop stop has no committed completion");
        sqlx::query("UPDATE api_cluster_desktop_controls SET completed_at=clock_timestamp() WHERE ticket_id=$1 AND claimed_at IS NOT NULL AND completed_at IS NULL")
            .bind(control.id).execute(&mut *tx).await?;
        tx.commit().await?;
    }
    transport::write_initial(
        &mut socket,
        &Outcome {
            protocol: 1,
            stopped: true,
        },
    )
    .await
}

async fn guard<T>(
    cluster: &ApiCluster,
    control: &Control,
    work: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        authorise(cluster, control),
    )
    .await??;
    tokio::pin!(work);
    let cadence = std::time::Duration::from_millis(750);
    let mut checks = tokio::time::interval_at(tokio::time::Instant::now() + cadence, cadence);
    checks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {biased;
            _=cluster.wait_until_lost()=>anyhow::bail!("desktop stop API owner was lost"),
            _=checks.tick()=>tokio::time::timeout(std::time::Duration::from_secs(2),authorise(cluster,control)).await??,
            result=&mut work=>return result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::linux::tests::{try_claim, Fixture};
    use super::*;
    async fn ticket(fixture: &Fixture) -> (Uuid, Uuid) {
        try_claim(fixture, &fixture.first, None).await.unwrap();
        let user = Uuid::new_v4();
        let auth = Uuid::new_v4();
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO users(id) VALUES($1)")
            .bind(user)
            .execute(&fixture.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO auth_sessions(id,user_id,expires_at) VALUES($1,$2,clock_timestamp()+interval '60 seconds')").bind(auth).bind(user).execute(&fixture.pool).await.unwrap();
        sqlx::query("INSERT INTO api_cluster_desktop_controls(ticket_id,group_id,origin_node_id,origin_process_epoch,owner_node_id,owner_process_epoch,desktop_session_id,run_id,user_id,auth_session_id,purpose,binding_sha256,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,'stop',$11,clock_timestamp()+interval '30 seconds')")
            .bind(id).bind(fixture.first.group_id).bind(fixture.second.node_id).bind(fixture.second.process_epoch).bind(fixture.first.node_id).bind(fixture.first.process_epoch)
            .bind(fixture.job.session_id).bind(fixture.job.run_id).bind(user).bind(auth).bind(fixture.job.digest().unwrap()).execute(&fixture.pool).await.unwrap();
        (id, auth)
    }
    async fn consume(fixture: &Fixture, id: Uuid) -> bool {
        sqlx::query_scalar::<_, Uuid>(CLAIM_CONTROL)
            .bind(id)
            .fetch_optional(&fixture.pool)
            .await
            .unwrap()
            .is_some()
    }
    #[tokio::test]
    #[ignore = "requires COWORK_TEST_DATABASE_URL pointing to isolated PostgreSQL"]
    async fn postgres_desktop_stop_control_is_one_use_and_unknown_prevents_owner_adoption() {
        let fixture = Fixture::new().await;
        let (id, _) = ticket(&fixture).await;
        let (left, right) = tokio::join!(consume(&fixture, id), consume(&fixture, id));
        assert_ne!(left, right, "only one request may dispatch a physical stop");
        assert!(
            !consume(&fixture, id).await,
            "unknown control must not replay"
        );
        let polled = std::sync::atomic::AtomicBool::new(false);
        let blocked: Result<()> =
            super::super::linux::guard_job(&fixture.pool, &fixture.job, async {
                polled.store(true, std::sync::atomic::Ordering::SeqCst);
                std::future::pending().await
            })
            .await;
        assert!(blocked.is_err(), "unknown stop also fences new RFB work");
        assert!(!polled.load(std::sync::atomic::Ordering::SeqCst));
        sqlx::query("UPDATE api_cluster_nodes SET lease_until=clock_timestamp()-interval '1 second' WHERE node_id=$1").bind(fixture.first.node_id).execute(&fixture.pool).await.unwrap();
        assert!(
            try_claim(&fixture, &fixture.second, Some(&fixture.first))
                .await
                .is_err(),
            "lost owner does not resolve an unknown physical stop"
        );
        fixture.cleanup().await;
    }
    #[tokio::test]
    #[ignore = "requires COWORK_TEST_DATABASE_URL pointing to isolated PostgreSQL"]
    async fn postgres_desktop_stop_control_checks_current_user_node_epochs_and_database_expiry() {
        for change in ["UPDATE auth_sessions SET revoked_at=clock_timestamp()","UPDATE users SET deleted_at=clock_timestamp()",
            "UPDATE api_cluster_nodes SET process_epoch=gen_random_uuid()","UPDATE api_cluster_nodes SET lease_until=clock_timestamp()-interval '1 second'",
            "UPDATE api_cluster_desktop_controls SET expires_at=clock_timestamp()-interval '1 second'",
            "UPDATE api_cluster_desktop_owners SET process_epoch=gen_random_uuid()"] {
            let fixture=Fixture::new().await;let(id,_)=ticket(&fixture).await;
            sqlx::query(change).execute(&fixture.pool).await.unwrap();assert!(!consume(&fixture,id).await,"fenced control must not dispatch: {change}");fixture.cleanup().await;
        }
    }
    #[tokio::test]
    #[ignore = "requires COWORK_TEST_DATABASE_URL pointing to isolated PostgreSQL"]
    async fn postgres_desktop_stop_controls_cannot_be_repurposed_or_completed_before_claim() {
        let fixture = Fixture::new().await;
        let (id, _) = ticket(&fixture).await;
        assert!(sqlx::query(
            "UPDATE api_cluster_desktop_controls SET purpose='rfb' WHERE ticket_id=$1"
        )
        .bind(id)
        .execute(&fixture.pool)
        .await
        .is_err());
        assert!(sqlx::query("UPDATE api_cluster_desktop_controls SET completed_at=clock_timestamp() WHERE ticket_id=$1").bind(id).execute(&fixture.pool).await.is_err());
        assert!(consume(&fixture, id).await);
        sqlx::query("UPDATE api_cluster_desktop_controls SET completed_at=clock_timestamp() WHERE ticket_id=$1").bind(id).execute(&fixture.pool).await.unwrap();
        assert!(
            !consume(&fixture, id).await,
            "completed controls are queries only"
        );
        fixture.cleanup().await;
    }
}
