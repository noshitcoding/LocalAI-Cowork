//! Native Kubernetes identity for one originally enrolled desktop pod.
//! Docker identities retain their original v1 wire format and proof domains.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KubernetesJobIdentity {
    pub version: u8,
    pub cluster_uid: Uuid,
    pub namespace_uid: Uuid,
    pub organisation_id: Uuid,
    pub instance_id: Uuid,
    pub generation: Uuid,
    pub node_name: String,
    pub node_uid: Uuid,
    pub pod_uid: Uuid,
    /// Original create-ACK resource version, never rewritten by a later probe.
    pub pod_resource_version: String,
    pub pvc_uid: Uuid,
    pub container_runtime_id: String,
    pub pod_pids_limit: u32,
}
impl KubernetesJobIdentity {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.version != 1
            || [
                self.cluster_uid,
                self.namespace_uid,
                self.organisation_id,
                self.instance_id,
                self.generation,
                self.node_uid,
                self.pod_uid,
                self.pvc_uid,
            ]
            .iter()
            .any(Uuid::is_nil)
            || self.node_name.is_empty()
            || self.node_name.len() > 253
            || !self
                .node_name
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'-' | b'.'))
            || self.pod_resource_version.is_empty()
            || self.pod_resource_version.len() > 128
            || !self
                .pod_resource_version
                .bytes()
                .all(|c| c.is_ascii_graphic())
            || self.container_runtime_id.len() > 512
            || self.pod_pids_limit == 0
            || self.pod_pids_limit > 4096
        {
            return Err("invalid original Kubernetes physical job identity");
        }
        let (runtime, id) = self
            .container_runtime_id
            .split_once("://")
            .ok_or("actual CRI container identity absent")?;
        if runtime.is_empty()
            || id.is_empty()
            || !runtime
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
            || !id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
        {
            return Err("invalid actual CRI container identity");
        }
        Ok(())
    }
}
