//! Physical retirement of the original container on the positively inspected
//! current Docker engine. Never adopts a different container or replays a tool.
use super::*;
use cowork_contracts::{TeamContainerRecoveryPlan, TeamContainerRecoveryReceipt};
use cowork_storage::team_service::{ContainerClosedEvidence, ContainerRecoveryPermit};
use sha2::{Digest, Sha256};

fn source_sha(raw: &[u8]) -> String {
    format!("{:x}", Sha256::digest(raw))
}
fn binding(plan: &TeamContainerRecoveryPlan) -> HostBinding {
    HostBinding {
        format: 1,
        host_id: plan.host_id,
        engine_id: plan.engine_id.clone(),
        node_name: plan.node_name.clone(),
        node_uid: plan.node_uid,
    }
}
impl Containers {
    fn recovery_source(&self, plan: &TeamContainerRecoveryPlan) -> Result<Record> {
        let path = self
            .config
            .control_directory
            .join(format!("ct-{}.json", plan.nonce));
        let raw = docker_host::read(&path)?;
        if raw.len() > 4096 || source_sha(&raw) != plan.record_sha256 {
            return Err(Error::Conflict);
        }
        let record: Record = serde_json::from_slice(&raw).map_err(|_| invalid())?;
        if record.nonce != plan.nonce
            || record.team != plan.team_id
            || record.order != plan.order_id
            || record.run != plan.run_id
            || record.format != plan.record_format
            || record.name != format!("cowork-team-{}", plan.nonce)
            || record
                .container_id
                .as_ref()
                .is_some_and(|id| id != &plan.container_id)
            || plan.legacy_location_adoption != (record.format == 1)
            || record.format == 1 && record.host.is_some()
            || record.format == 2 && record.host.as_ref() != Some(&binding(plan))
        {
            return Err(Error::Conflict);
        }
        Ok(record)
    }
    fn recovery_host(&self, plan: &TeamContainerRecoveryPlan) -> Result<()> {
        if self.host(plan.nonce)? != binding(plan) {
            return Err(Error::UncertainEffect);
        }
        Ok(())
    }
    fn recovery_inspect(&self, plan: &TeamContainerRecoveryPlan) -> Result<Option<Inspection>> {
        let record = self.recovery_source(plan)?;
        self.recovery_host(plan)?;
        let found = self.inspect_original(&record, &plan.container_id)?;
        self.recovery_host(plan)?;
        if found.as_ref().is_some_and(|v| v.id != plan.container_id) {
            return Err(Error::Forbidden);
        }
        Ok(found)
    }
    pub(in crate::team_toolbox) fn recovery_plans(
        &self,
        team: Uuid,
        order: Uuid,
        run: Uuid,
    ) -> Result<Vec<TeamContainerRecoveryPlan>> {
        let _lease = self
            .toolbox
            .workspace_lease(&order.to_string())
            .map_err(|_| Error::Conflict)?;
        let _namespace = docker_host::hold_namespace(&self.config.control_directory)?;
        let mut plans = Vec::new();
        for record in self
            .records()?
            .into_iter()
            .filter(|r| r.team == team && r.order == order && r.run == run)
        {
            let raw = docker_host::read(&self.file(&record))?;
            let host = self.host(record.nonce)?;
            if record.format == 2 && record.host.as_ref() != Some(&host) {
                return Err(Error::UncertainEffect);
            }
            let target = record.container_id.as_deref().unwrap_or(&record.name);
            let found = self.inspect_original(&record, target)?;
            if self.host(record.nonce)? != host || docker_host::read(&self.file(&record))? != raw {
                return Err(Error::UncertainEffect);
            }
            // An absent unbound record cannot establish which old engine acted.
            if record.format == 1 && found.is_none() {
                continue;
            }
            let Some(id) = found
                .as_ref()
                .map(|v| v.id.clone())
                .or(record.container_id.clone())
            else {
                continue;
            };
            plans.push(TeamContainerRecoveryPlan {
                nonce: record.nonce,
                team_id: team,
                order_id: order,
                run_id: run,
                revision: 0,
                record_format: record.format,
                record_sha256: source_sha(&raw),
                container_id: id,
                host_id: host.host_id,
                engine_id: host.engine_id,
                node_name: host.node_name,
                node_uid: host.node_uid,
                legacy_location_adoption: record.format == 1,
                container_present: found.is_some(),
                running: found
                    .as_ref()
                    .is_some_and(|v| v.state.running || v.state.pid != 0),
                paused: found.as_ref().is_some_and(|v| v.state.paused),
                plan_sha256: String::new(),
            });
            if plans.len() > 64 {
                return Err(Error::BudgetExceeded);
            }
        }
        Ok(plans)
    }
    pub(in crate::team_toolbox) async fn retire_original(
        &self,
        permit: ContainerRecoveryPermit,
    ) -> Result<ContainerClosedEvidence> {
        let plan = permit.plan().clone();
        let containers = self.clone();
        let held = tokio::task::spawn_blocking(move || {
            let lease = containers
                .toolbox
                .workspace_lease(&plan.order_id.to_string())
                .map_err(|_| Error::Conflict)?;
            let namespace = docker_host::hold_namespace(&containers.config.control_directory)?;
            Ok::<_, Error>((lease, namespace))
        })
        .await
        .map_err(|_| Error::UncertainEffect)??;
        permit.revalidate(false).await?;
        let containers = self.clone();
        let plan = permit.plan().clone();
        let first = tokio::task::spawn_blocking(move || containers.recovery_inspect(&plan))
            .await
            .map_err(|_| Error::UncertainEffect)??;
        if let Some(first) = first {
            if first.state.running
                || first.state.pid != 0
                || first.state.paused
                || first.state.restarting
            {
                permit.revalidate(false).await?;
                let containers = self.clone();
                let plan = permit.plan().clone();
                tokio::task::spawn_blocking(move || {
                    containers.recovery_source(&plan)?;
                    containers.recovery_host(&plan)?;
                    // A paused guest is killed without unpausing its old tool.
                    let args = if first.state.paused {
                        vec![
                            "kill".into(),
                            "--signal=KILL".into(),
                            plan.container_id.clone(),
                        ]
                    } else {
                        vec![
                            "stop".into(),
                            "--time".into(),
                            "30".into(),
                            plan.container_id.clone(),
                        ]
                    };
                    let result = containers.cli(plan.nonce, args)?;
                    if result.exit_code != Some(0) || !result.exited || !result.output_complete {
                        return Err(Error::UncertainEffect);
                    }
                    containers.recovery_host(&plan)
                })
                .await
                .map_err(|_| Error::UncertainEffect)??;
            }
            let containers = self.clone();
            let plan = permit.plan().clone();
            let stopped = tokio::task::spawn_blocking(move || containers.recovery_inspect(&plan))
                .await
                .map_err(|_| Error::UncertainEffect)??;
            if let Some(stopped) = stopped {
                if stopped.state.running
                    || stopped.state.paused
                    || stopped.state.restarting
                    || stopped.state.pid != 0
                {
                    return Err(Error::UncertainEffect);
                }
                permit.revalidate(false).await?;
                let containers = self.clone();
                let plan = permit.plan().clone();
                tokio::task::spawn_blocking(move || {
                    containers.recovery_source(&plan)?;
                    containers.recovery_host(&plan)?;
                    let result =
                        containers.cli(plan.nonce, vec!["rm".into(), plan.container_id.clone()])?;
                    if result.exit_code != Some(0) || !result.exited || !result.output_complete {
                        return Err(Error::UncertainEffect);
                    }
                    containers.recovery_host(&plan)
                })
                .await
                .map_err(|_| Error::UncertainEffect)??;
            }
        }
        let containers = self.clone();
        let plan = permit.plan().clone();
        tokio::task::spawn_blocking(move || {
            if containers.recovery_inspect(&plan)?.is_some() {
                return Err(Error::UncertainEffect);
            }
            Ok::<_, Error>(())
        })
        .await
        .map_err(|_| Error::UncertainEffect)??;
        let plan = permit.plan();
        let evidence = ContainerClosedEvidence {
            plan_sha256: plan.plan_sha256.clone(),
            record_sha256: plan.record_sha256.clone(),
            container_id: plan.container_id.clone(),
            host_id: plan.host_id,
            engine_id: plan.engine_id.clone(),
        };
        drop(held);
        Ok(evidence)
    }
    pub(in crate::team_toolbox) async fn close_original_record(
        &self,
        permit: ContainerRecoveryPermit,
        receipt: TeamContainerRecoveryReceipt,
    ) -> Result<()> {
        permit.revalidate(true).await?;
        if !receipt.container_removed
            || !receipt.effects_unknown
            || receipt.operation_id != permit.operation_id()
            || receipt.record_sha256 != permit.plan().record_sha256
            || receipt.plan_sha256 != permit.plan().plan_sha256
        {
            return Err(Error::Conflict);
        }
        let containers = self.clone();
        let plan = permit.plan().clone();
        tokio::task::spawn_blocking(move || {
            let _lease = containers
                .toolbox
                .workspace_lease(&plan.order_id.to_string())
                .map_err(|_| Error::Conflict)?;
            let _namespace = docker_host::hold_namespace(&containers.config.control_directory)?;
            let archive_root = containers.config.control_directory.join("retired");
            private_directory(&archive_root)?;
            let root = archive_root.join(plan.nonce.to_string());
            private_directory(&root)?;
            let _archive_namespace = docker_host::hold_namespace(&root)?;
            let original_name = format!("original-{}.json", plan.record_sha256);
            let source = containers
                .config
                .control_directory
                .join(format!("ct-{}.json", plan.nonce));
            let target = root.join(&original_name);
            if !source.exists() {
                if source_sha(&docker_host::read(&target)?) != plan.record_sha256 {
                    return Err(Error::Conflict);
                }
                return Ok(());
            }
            let record = containers.recovery_source(&plan)?;
            // Reconfirm only by GET. No signal/delete is repeated after receipt.
            if containers.recovery_inspect(&plan)?.is_some() {
                return Err(Error::UncertainEffect);
            }
            let raw = docker_host::read(&source)?;
            docker_host::publish(&root, &original_name, &raw)?;
            if source_sha(&docker_host::read(&target)?) != plan.record_sha256
                || docker_host::read(&source)? != raw
            {
                return Err(Error::Conflict);
            }
            if record.github.is_some() {
                let secret = env_file::path(&containers.config.control_directory, plan.nonce);
                if secret.exists() {
                    regular(&secret)?;
                    docker_host::private(&secret, false)?;
                    fs::remove_file(&secret).map_err(cowork_domain::storage_error)?;
                }
            }
            // Retain the immutable original first; only the exact active record
            // is then removed. Workspaces and uncertain old outputs are retained.
            fs::remove_file(&source).map_err(cowork_domain::storage_error)?;
            #[cfg(unix)]
            fs::File::open(&containers.config.control_directory)
                .map_err(cowork_domain::storage_error)?
                .sync_all()
                .map_err(cowork_domain::storage_error)?;
            Ok(())
        })
        .await
        .map_err(|_| Error::UncertainEffect)?
    }
}
pub(super) fn private_directory(path: &Path) -> Result<()> {
    if !path.exists() {
        #[cfg(windows)]
        let created = docker_host::create_private_directory(path);
        #[cfg(unix)]
        let created = {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new().mode(0o700).create(path)
        };
        match created {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(cowork_domain::storage_error(e)),
        }
    }
    docker_host::private(path, true)
}
