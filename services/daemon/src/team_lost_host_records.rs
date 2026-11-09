//! Local original-record reads and host-retired archival. No Docker operation.
use super::*;
use cowork_contracts::TeamOriginalHostBinding;
use cowork_storage::team_service::{LostHostClosurePermit, TeamLostHostRecord};
use sha2::{Digest, Sha256};
fn hash(raw: &[u8]) -> String {
    format!("{:x}", Sha256::digest(raw))
}
fn publish_original(root: &Path, name: &str, raw: &[u8]) -> Result<()> {
    let target = root.join(name);
    match fs::symlink_metadata(&target) {
        Ok(_) => {
            if docker_host::read(&target)? != raw {
                return Err(Error::Conflict);
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => docker_host::publish(root, name, raw),
        Err(e) => Err(cowork_domain::storage_error(e)),
    }
}
impl Containers {
    pub(in crate::team_toolbox) fn operator_binding(&self) -> Result<TeamOriginalHostBinding> {
        let held = docker_host::hold_namespace(&self.config.control_directory)?;
        let actual = self.host(Uuid::new_v4())?;
        // Enrollment precedes original Create; retained legacy records never acquire new trust.
        if self
            .records()?
            .iter()
            .any(|r| r.trusted_host_admission.is_none())
        {
            return Err(Error::UncertainEffect);
        }
        drop(held);
        Ok(TeamOriginalHostBinding {
            format: actual.format,
            host_id: actual.host_id,
            engine_id: actual.engine_id,
            node_name: actual.node_name,
            node_uid: actual.node_uid,
        })
    }
    pub(in crate::team_toolbox) fn lost_records(
        &self,
        team: Uuid,
        order: Uuid,
        run: Uuid,
    ) -> Result<Vec<TeamLostHostRecord>> {
        let _lease = self
            .toolbox
            .workspace_lease(&order.to_string())
            .map_err(|_| Error::Conflict)?;
        let _namespace = docker_host::hold_namespace(&self.config.control_directory)?;
        let mut out = Vec::new();
        for record in self
            .records()?
            .into_iter()
            .filter(|r| r.team == team && r.order == order && r.run == run)
        {
            let raw = docker_host::read(&self.file(&record))?;
            let actual: Record = serde_json::from_slice(&raw).map_err(|_| invalid())?;
            if raw.len() > 4096
                || actual.nonce != record.nonce
                || actual.team != team
                || actual.order != order
                || actual.run != run
            {
                return Err(Error::Conflict);
            }
            out.push(TeamLostHostRecord {
                nonce: actual.nonce,
                raw,
                original_source: "local_record",
            });
            if out.len() > 64 {
                return Err(Error::BudgetExceeded);
            }
        }
        Ok(out)
    }
    pub(in crate::team_toolbox) async fn close_lost_record(
        &self,
        permit: LostHostClosurePermit,
    ) -> Result<()> {
        let p = permit.plan();
        let receipt = permit.receipt();
        if !receipt.host_retired
            || receipt.container_removed
            || !receipt.effects_unknown
            || receipt.workspace_promotion_allowed
            || receipt.nonce != p.nonce
            || receipt.original_record_sha256 != p.original_record_sha256
            || hash(permit.original_bytes()) != p.original_record_sha256
        {
            return Err(Error::Conflict);
        }
        let containers = self.clone();
        let order = p.order_id;
        let held = tokio::task::spawn_blocking(move || {
            let lease = containers
                .toolbox
                .workspace_lease(&order.to_string())
                .map_err(|_| Error::Conflict)?;
            let namespace = docker_host::hold_namespace(&containers.config.control_directory)?;
            Ok::<_, Error>((lease, namespace))
        })
        .await
        .map_err(|_| Error::UncertainEffect)??;
        let raw = permit.original_bytes().to_vec();
        let original: Record = serde_json::from_slice(&raw).map_err(|_| invalid())?;
        if original.format != 2
            || original.trusted_host_admission.as_ref().is_none_or(|v| {
                v.profile_id != p.profile_id || v.profile_sha256 != p.original_profile_sha256
            })
            || original.nonce != p.nonce
            || original.team != p.team_id
            || original.order != p.order_id
            || original.run != p.run_id
            || original.container_id != p.container_id
            || original.name != p.container_name
        {
            return Err(Error::Conflict);
        }
        let receipt_raw = serde_json::to_vec(receipt).map_err(cowork_domain::storage_error)?;
        let sha = p.original_record_sha256.clone();
        let containers = self.clone();
        permit
            .close_original_io(move || {
                let archive = containers.config.control_directory.join("retired");
                recovery::private_directory(&archive)?;
                let root = archive.join(original.nonce.to_string());
                recovery::private_directory(&root)?;
                let _archive_namespace = docker_host::hold_namespace(&root)?;
                let name = format!("original-{sha}.json");
                let receipt_name = format!("host-retirement-receipt-{}.json", hash(&receipt_raw));
                let source = containers.file(&original);
                match fs::symlink_metadata(&source) {
                    Ok(_) => {
                        if docker_host::read(&source)? != raw {
                            return Err(Error::Conflict);
                        }
                        publish_original(&root, &name, &raw)?;
                        publish_original(&root, &receipt_name, &receipt_raw)?;
                        if docker_host::read(&source)? != raw {
                            return Err(Error::Conflict);
                        }
                        if original.github.is_some() {
                            let secret = env_file::path(
                                &containers.config.control_directory,
                                original.nonce,
                            );
                            match fs::symlink_metadata(&secret) {
                                Ok(_) => {
                                    regular(&secret)?;
                                    docker_host::private(&secret, false)?;
                                    fs::remove_file(&secret)
                                        .map_err(cowork_domain::storage_error)?;
                                }
                                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                                Err(e) => return Err(cowork_domain::storage_error(e)),
                            }
                        }
                        fs::remove_file(&source).map_err(cowork_domain::storage_error)?;
                        #[cfg(unix)]
                        fs::File::open(&containers.config.control_directory)
                            .map_err(cowork_domain::storage_error)?
                            .sync_all()
                            .map_err(cowork_domain::storage_error)?;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        // Restore only original durable metadata; no workspace or
                        // guest bytes/physical-absence proof is invented.
                        publish_original(&root, &name, &raw)?;
                        publish_original(&root, &receipt_name, &receipt_raw)?;
                    }
                    Err(e) => return Err(cowork_domain::storage_error(e)),
                }
                Ok(())
            })
            .await?;
        drop(held);
        Ok(())
    }
}
